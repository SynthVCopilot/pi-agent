//! FFmpeg component lifecycle, job C ABI, and safe Agent tools.

use std::{
    ffi::{c_char, OsString},
    path::{Path, PathBuf},
    ptr,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        OnceLock, RwLock, RwLockReadGuard, RwLockWriteGuard, TryLockError,
    },
    thread,
    time::Duration,
};

use pi_agent_components::{
    Architecture, AudioExecutor, ComponentError, ComponentPaths, FfmpegInstaller, FfmpegResolver,
    FfmpegSource, HttpDownloader, JobHandle, LoudnessTarget, NormalizeRequest, PrepareWavRequest,
    ProcessRunner, ResolvedFfmpeg, SampleFormat, SystemProcessRunner,
};
use pi_agent_core::{
    config_path, default_catalog, ComponentAction, ComponentSource, ComponentState,
    ComponentStatus, ComponentView, FfmpegConfig, FfmpegOperationResult, FfmpegProbeResult,
    FfmpegRequest, FfmpegSourcePreference, JobError as CoreJobError, JobState as CoreJobState,
    JobStatus as CoreJobStatus, LoudnessAnalysisResult, ToolCall, ToolDefinition, ToolExecutor,
    ToolResult,
};

use super::{cstr_to_str, to_cstring};

static NEXT_JOB_ID: AtomicU64 = AtomicU64::new(1);
static LIFECYCLE_ACTIVE: AtomicBool = AtomicBool::new(false);
static COMPONENT_GATE: RwLock<()> = RwLock::new(());
static FFMPEG_CONFIG_OVERRIDE: OnceLock<RwLock<Option<FfmpegConfig>>> = OnceLock::new();
const HEALTH_CHECK_TIMEOUT: Duration = Duration::from_secs(5);

struct ProcessComponentGuard {
    #[cfg(windows)]
    _file: std::fs::File,
}

struct ComponentReadGuard {
    _local: RwLockReadGuard<'static, ()>,
    _process: ProcessComponentGuard,
}

struct ComponentWriteGuard {
    _local: RwLockWriteGuard<'static, ()>,
    _process: ProcessComponentGuard,
}

/// Opaque background job handle for the C ABI.
pub struct PiJob {
    inner: JobHandle,
}

struct LifecycleGuard;

impl Drop for LifecycleGuard {
    fn drop(&mut self) {
        LIFECYCLE_ACTIVE.store(false, Ordering::Release);
    }
}

fn next_job_id() -> u64 {
    NEXT_JOB_ID.fetch_add(1, Ordering::Relaxed)
}

#[cfg(windows)]
fn process_component_lock(
    cancelled: &AtomicBool,
    wait: bool,
) -> pi_agent_components::Result<Option<ProcessComponentGuard>> {
    process_component_lock_at(&paths(), cancelled, wait)
}

#[cfg(windows)]
fn process_component_lock_at(
    component_paths: &ComponentPaths,
    cancelled: &AtomicBool,
    wait: bool,
) -> pi_agent_components::Result<Option<ProcessComponentGuard>> {
    use std::os::windows::fs::OpenOptionsExt;

    const ERROR_SHARING_VIOLATION: i32 = 32;
    const ERROR_LOCK_VIOLATION: i32 = 33;
    component_paths.validate_component_root()?;
    std::fs::create_dir_all(&component_paths.component_root)?;
    component_paths.validate_component_root()?;
    let lock_path = component_paths.component_root.join(".component.lock");
    component_paths.validate_component_path(&lock_path)?;
    loop {
        if cancelled.load(Ordering::Acquire) {
            return Err(ComponentError::Cancelled);
        }
        match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .share_mode(0)
            .open(&lock_path)
        {
            Ok(file) => {
                component_paths.validate_component_path(&lock_path)?;
                return Ok(Some(ProcessComponentGuard { _file: file }));
            }
            Err(error)
                if matches!(
                    error.raw_os_error(),
                    Some(ERROR_SHARING_VIOLATION) | Some(ERROR_LOCK_VIOLATION)
                ) =>
            {
                if !wait {
                    return Ok(None);
                }
                thread::sleep(Duration::from_millis(25));
            }
            Err(error) => return Err(ComponentError::Io(error)),
        }
    }
}

#[cfg(not(windows))]
fn process_component_lock(
    cancelled: &AtomicBool,
    _wait: bool,
) -> pi_agent_components::Result<Option<ProcessComponentGuard>> {
    if cancelled.load(Ordering::Acquire) {
        Err(ComponentError::Cancelled)
    } else {
        Ok(Some(ProcessComponentGuard {}))
    }
}

fn component_read(cancelled: &AtomicBool) -> pi_agent_components::Result<ComponentReadGuard> {
    let process = process_component_lock(cancelled, true)?
        .ok_or_else(|| ComponentError::State("component lock was unavailable".into()))?;
    loop {
        if cancelled.load(Ordering::Acquire) {
            return Err(ComponentError::Cancelled);
        }
        match COMPONENT_GATE.try_read() {
            Ok(local) => {
                return Ok(ComponentReadGuard {
                    _local: local,
                    _process: process,
                })
            }
            Err(TryLockError::WouldBlock) => thread::sleep(Duration::from_millis(25)),
            Err(TryLockError::Poisoned(_)) => {
                return Err(ComponentError::State("component gate is poisoned".into()))
            }
        }
    }
}

fn component_try_read(
    cancelled: &AtomicBool,
) -> pi_agent_components::Result<Option<ComponentReadGuard>> {
    let Some(process) = process_component_lock(cancelled, false)? else {
        return Ok(None);
    };
    match COMPONENT_GATE.try_read() {
        Ok(local) => Ok(Some(ComponentReadGuard {
            _local: local,
            _process: process,
        })),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Poisoned(_)) => {
            Err(ComponentError::State("component gate is poisoned".into()))
        }
    }
}

fn component_write(cancelled: &AtomicBool) -> pi_agent_components::Result<ComponentWriteGuard> {
    let process = process_component_lock(cancelled, true)?
        .ok_or_else(|| ComponentError::State("component lock was unavailable".into()))?;
    loop {
        if cancelled.load(Ordering::Acquire) {
            return Err(ComponentError::Cancelled);
        }
        match COMPONENT_GATE.try_write() {
            Ok(local) => {
                return Ok(ComponentWriteGuard {
                    _local: local,
                    _process: process,
                })
            }
            Err(TryLockError::WouldBlock) => thread::sleep(Duration::from_millis(25)),
            Err(TryLockError::Poisoned(_)) => {
                return Err(ComponentError::State("component gate is poisoned".into()))
            }
        }
    }
}

fn paths() -> ComponentPaths {
    ComponentPaths::from_data_root(pi_agent_core::data_root())
}

fn load_persisted_ffmpeg_config() -> FfmpegConfig {
    let Ok(text) = std::fs::read_to_string(config_path()) else {
        return FfmpegConfig::default();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return FfmpegConfig::default();
    };
    value
        .get("ffmpeg")
        .cloned()
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default()
}

pub(crate) fn configure_ffmpeg(config: FfmpegConfig) {
    let lock = FFMPEG_CONFIG_OVERRIDE.get_or_init(|| RwLock::new(None));
    if let Ok(mut current) = lock.write() {
        *current = Some(config);
    }
}

pub(crate) fn ffmpeg_config() -> FfmpegConfig {
    FFMPEG_CONFIG_OVERRIDE
        .get()
        .and_then(|lock| lock.read().ok())
        .and_then(|config| config.clone())
        .unwrap_or_else(load_persisted_ffmpeg_config)
}

fn validated_version(
    resolved: &ResolvedFfmpeg,
    cancelled: &AtomicBool,
) -> pi_agent_components::Result<String> {
    let runner = SystemProcessRunner;
    let args = vec![OsString::from("-hide_banner"), OsString::from("-version")];
    let ffmpeg =
        runner.run_with_timeout(&resolved.ffmpeg, &args, cancelled, HEALTH_CHECK_TIMEOUT)?;
    if !ffmpeg.success {
        return Err(ComponentError::Process {
            message: "ffmpeg -version failed".into(),
            stderr: ffmpeg.stderr,
        });
    }
    let ffprobe =
        runner.run_with_timeout(&resolved.ffprobe, &args, cancelled, HEALTH_CHECK_TIMEOUT)?;
    if !ffprobe.success {
        return Err(ComponentError::Process {
            message: "ffprobe -version failed".into(),
            stderr: ffprobe.stderr,
        });
    }
    Ok(resolved
        .version
        .clone()
        .or_else(|| ffmpeg.stdout.lines().next().map(str::to_owned))
        .unwrap_or_else(|| "unknown".into()))
}

fn validated_candidate(
    resolved: ResolvedFfmpeg,
    cancelled: &AtomicBool,
) -> pi_agent_components::Result<(ResolvedFfmpeg, String)> {
    let version = validated_version(&resolved, cancelled)?;
    Ok((resolved, version))
}

fn resolve_validated_ffmpeg(
    config: &FfmpegConfig,
    cancelled: &AtomicBool,
) -> pi_agent_components::Result<(ResolvedFfmpeg, String)> {
    let resolver = FfmpegResolver::new(paths());
    match config.preference {
        FfmpegSourcePreference::Auto => {
            if let Some(directory) = config.system_bin_dir.as_deref() {
                match resolver
                    .resolve(Some(Path::new(directory)))
                    .and_then(|resolved| validated_candidate(resolved, cancelled))
                {
                    Ok(resolved) => return Ok(resolved),
                    Err(ComponentError::Cancelled) => return Err(ComponentError::Cancelled),
                    Err(_) => {}
                }
            }
            match resolver
                .resolve_managed()
                .and_then(|resolved| validated_candidate(resolved, cancelled))
            {
                Ok(resolved) => return Ok(resolved),
                Err(ComponentError::Cancelled) => return Err(ComponentError::Cancelled),
                Err(_) => {}
            }
            validated_candidate(resolver.resolve_system()?, cancelled)
        }
        FfmpegSourcePreference::Managed => {
            validated_candidate(resolver.resolve_managed()?, cancelled)
        }
        FfmpegSourcePreference::System => match config.system_bin_dir.as_deref() {
            Some(directory) => {
                validated_candidate(resolver.resolve(Some(Path::new(directory)))?, cancelled)
            }
            None => validated_candidate(resolver.resolve_system()?, cancelled),
        },
    }
}

fn ffmpeg_status(config: &FfmpegConfig) -> ComponentStatus {
    let available_version = Architecture::host()
        .ok()
        .map(|architecture| architecture.release().version.to_string());
    let cancelled = AtomicBool::new(false);
    let _shared = match component_try_read(&cancelled) {
        Ok(Some(guard)) => guard,
        Ok(None) => {
            return ComponentStatus {
                id: "ffmpeg".into(),
                state: ComponentState::Checking,
                source: ComponentSource::Unavailable,
                installed_version: None,
                available_version,
                executable_dir: None,
                can_install: false,
                can_update: false,
                can_uninstall: false,
                error: None,
            }
        }
        Err(error) => {
            return ComponentStatus {
                id: "ffmpeg".into(),
                state: ComponentState::Failed,
                source: ComponentSource::Unavailable,
                installed_version: None,
                available_version,
                executable_dir: None,
                can_install: false,
                can_update: false,
                can_uninstall: false,
                error: Some(error.to_string()),
            }
        }
    };
    let resolver = FfmpegResolver::new(paths());
    let has_managed = resolver.read_current().ok().flatten().is_some();

    match resolve_validated_ffmpeg(config, &cancelled) {
        Ok((resolved, version)) => {
            let source = match resolved.source {
                FfmpegSource::Explicit => ComponentSource::Explicit,
                FfmpegSource::Managed => ComponentSource::Managed,
                FfmpegSource::System => ComponentSource::System,
            };
            let can_update = source == ComponentSource::Managed
                && available_version.as_deref() != Some(version.as_str());
            ComponentStatus {
                id: "ffmpeg".into(),
                state: ComponentState::Ready,
                source,
                installed_version: Some(version),
                available_version,
                executable_dir: resolved
                    .ffmpeg
                    .parent()
                    .map(|p| p.to_string_lossy().into_owned()),
                can_install: source != ComponentSource::Managed,
                can_update,
                can_uninstall: has_managed,
                error: None,
            }
        }
        Err(error) => ComponentStatus {
            id: "ffmpeg".into(),
            state: ComponentState::NotInstalled,
            source: ComponentSource::Unavailable,
            installed_version: None,
            available_version,
            executable_dir: None,
            can_install: Architecture::host().is_ok(),
            can_update: false,
            can_uninstall: has_managed,
            error: Some(error.to_string()),
        },
    }
}

/// Return static catalog entries paired with current runtime state.
#[no_mangle]
pub extern "C" fn pi_components_status_json() -> *mut c_char {
    let ffmpeg = ffmpeg_status(&ffmpeg_config());
    let views: Vec<ComponentView> = default_catalog()
        .into_iter()
        .map(|spec| {
            let status = if spec.id == "ffmpeg" {
                ffmpeg.clone()
            } else {
                ComponentStatus {
                    id: spec.id.clone(),
                    ..ComponentStatus::default()
                }
            };
            ComponentView { spec, status }
        })
        .collect();
    match serde_json::to_string(&views) {
        Ok(json) => to_cstring(json),
        Err(_) => ptr::null_mut(),
    }
}

/// Start a human-initiated FFmpeg lifecycle action.
///
/// # Safety
/// Both string arguments must be valid NUL-terminated UTF-8.
#[no_mangle]
pub unsafe extern "C" fn pi_component_action_start(
    component_id: *const c_char,
    action: *const c_char,
) -> *mut PiJob {
    let (Some(component_id), Some(action)) = (cstr_to_str(component_id), cstr_to_str(action))
    else {
        return ptr::null_mut();
    };
    if component_id != "ffmpeg" {
        return ptr::null_mut();
    }
    let action = match action {
        "install" => ComponentAction::Install,
        "update" => ComponentAction::Update,
        "uninstall" => ComponentAction::Uninstall,
        _ => return ptr::null_mut(),
    };
    if LIFECYCLE_ACTIVE
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return ptr::null_mut();
    }

    let job = JobHandle::start(next_job_id(), move |progress| {
        let _guard = LifecycleGuard;
        progress.set("waiting", 0.0);
        let _exclusive = component_write(progress.cancellation_flag())?;
        let component_paths = paths();
        let installer = FfmpegInstaller::new(component_paths, HttpDownloader, SystemProcessRunner);
        match action {
            ComponentAction::Install | ComponentAction::Update => {
                let architecture = Architecture::host()?;
                let resolved = installer.install(
                    architecture,
                    progress.cancellation_flag(),
                    &mut |stage, amount| progress.set(stage, amount),
                )?;
                Ok(serde_json::to_value(resolved)?)
            }
            ComponentAction::Uninstall => {
                progress.check_cancelled()?;
                progress.set("uninstalling", 0.25);
                let removed = installer.uninstall_managed()?;
                progress.set("uninstalling", 0.9);
                Ok(serde_json::json!({ "removed": removed }))
            }
        }
    });
    Box::into_raw(Box::new(PiJob { inner: job }))
}

fn sample_format(value: Option<&str>) -> pi_agent_components::Result<SampleFormat> {
    match value.unwrap_or("s24") {
        "s16" => Ok(SampleFormat::S16),
        "s24" => Ok(SampleFormat::S24),
        "f32" => Ok(SampleFormat::F32),
        other => Err(ComponentError::InvalidInput(format!(
            "unsupported sample_format {other}; expected s16, s24, or f32"
        ))),
    }
}

fn analysis_target() -> LoudnessTarget {
    LoudnessTarget {
        integrated_lufs: -24.0,
        true_peak_db: -2.0,
        loudness_range: 7.0,
    }
}

fn run_ffmpeg_request(
    config: &FfmpegConfig,
    request: FfmpegRequest,
    cancelled: &AtomicBool,
) -> pi_agent_components::Result<serde_json::Value> {
    let _shared = component_read(cancelled)?;
    let (resolved, _version) = resolve_validated_ffmpeg(config, cancelled)?;
    let executor = AudioExecutor::new(paths(), resolved, SystemProcessRunner);
    match request {
        FfmpegRequest::Probe { input } => {
            let probe = executor.probe(Path::new(&input), cancelled)?;
            let stream = probe.audio_streams.first();
            Ok(serde_json::to_value(FfmpegOperationResult {
                output_path: None,
                probe: Some(FfmpegProbeResult {
                    container: probe.format_name,
                    codec: stream.and_then(|value| value.codec_name.clone()),
                    duration_seconds: probe.duration_seconds,
                    sample_rate: stream.and_then(|value| value.sample_rate),
                    channels: stream.and_then(|value| value.channels),
                    bit_depth: None,
                    bit_rate: stream.and_then(|value| value.bit_rate).or(probe.bit_rate),
                }),
                loudness: None,
            })?)
        }
        FfmpegRequest::Prepare {
            input,
            output_name,
            sample_rate,
            channels,
            sample_format: requested_format,
            start_seconds,
            duration_seconds,
        } => {
            let prepared = executor.prepare_wav(
                &PrepareWavRequest {
                    input: PathBuf::from(input),
                    output_name: Some(output_name),
                    start_seconds,
                    duration_seconds,
                    sample_rate,
                    channels,
                    sample_format: sample_format(requested_format.as_deref())?,
                },
                cancelled,
            )?;
            Ok(serde_json::to_value(FfmpegOperationResult {
                output_path: Some(prepared.output.to_string_lossy().into_owned()),
                probe: None,
                loudness: None,
            })?)
        }
        FfmpegRequest::LoudnessAnalyze { input } => {
            let measured =
                executor.loudness_analyze(Path::new(&input), &analysis_target(), cancelled)?;
            Ok(serde_json::to_value(FfmpegOperationResult {
                output_path: None,
                probe: None,
                loudness: Some(LoudnessAnalysisResult {
                    integrated_lufs: Some(measured.integrated_lufs),
                    true_peak_db: Some(measured.true_peak_db),
                    loudness_range: Some(measured.loudness_range),
                    threshold: Some(measured.threshold),
                }),
            })?)
        }
        FfmpegRequest::LoudnessNormalize {
            input,
            output_name,
            target_lufs,
            max_true_peak_db,
            target_lra,
        } => {
            let prepared = executor.loudness_normalize(
                &NormalizeRequest {
                    input: PathBuf::from(input),
                    output_name: Some(output_name),
                    target: LoudnessTarget {
                        integrated_lufs: target_lufs,
                        true_peak_db: max_true_peak_db,
                        loudness_range: target_lra,
                    },
                },
                cancelled,
            )?;
            Ok(serde_json::to_value(FfmpegOperationResult {
                output_path: Some(prepared.output.to_string_lossy().into_owned()),
                probe: None,
                loudness: None,
            })?)
        }
    }
}

/// Start a safe FFmpeg processing job from a tagged JSON request.
///
/// # Safety
/// `request_json` must be valid NUL-terminated UTF-8.
#[no_mangle]
pub unsafe extern "C" fn pi_ffmpeg_job_start(request_json: *const c_char) -> *mut PiJob {
    let Some(request_json) = cstr_to_str(request_json) else {
        return ptr::null_mut();
    };
    let request = serde_json::from_str::<FfmpegRequest>(request_json)
        .map_err(|error| ComponentError::InvalidInput(format!("invalid FFmpeg request: {error}")));
    let config = ffmpeg_config();
    let job = JobHandle::start(next_job_id(), move |progress| {
        progress.set("resolving", 0.02);
        let request = request?;
        progress.set("processing", 0.1);
        let result = run_ffmpeg_request(&config, request, progress.cancellation_flag())?;
        progress.set("processing", 0.95);
        Ok(result)
    });
    Box::into_raw(Box::new(PiJob { inner: job }))
}

/// Poll a component or FFmpeg job. Returned JSON must be freed with pi_string_free.
#[no_mangle]
pub unsafe extern "C" fn pi_job_status_json(job: *mut PiJob) -> *mut c_char {
    let Some(job) = job.as_ref() else {
        return ptr::null_mut();
    };
    let status = job.inner.status();
    let status = CoreJobStatus {
        id: status.id.to_string(),
        state: match status.state {
            pi_agent_components::JobState::Queued => CoreJobState::Queued,
            pi_agent_components::JobState::Running => CoreJobState::Running,
            pi_agent_components::JobState::Succeeded => CoreJobState::Succeeded,
            pi_agent_components::JobState::Failed => CoreJobState::Failed,
            pi_agent_components::JobState::Cancelled => CoreJobState::Cancelled,
        },
        phase: Some(status.stage),
        progress: Some(status.progress),
        result: status.result,
        error: status.error.map(|error| CoreJobError {
            code: error.code,
            message: error.message,
            details: error.details.map(|details| match details {
                serde_json::Value::String(text) => text,
                other => other.to_string(),
            }),
        }),
    };
    match serde_json::to_string(&status) {
        Ok(json) => to_cstring(json),
        Err(_) => ptr::null_mut(),
    }
}

/// Request cancellation. The worker observes the token during download and processing.
#[no_mangle]
pub unsafe extern "C" fn pi_job_cancel(job: *mut PiJob) {
    if let Some(job) = job.as_ref() {
        job.inner.cancel();
    }
}

/// Cancel, join, and free a job exactly once.
#[no_mangle]
pub unsafe extern "C" fn pi_job_destroy(job: *mut PiJob) {
    if !job.is_null() {
        let job = Box::from_raw(job);
        job.inner.cancel();
        job.inner.wait();
    }
}

/// Agent-visible, read-only FFmpeg tools. Audio-file creation remains a
/// Desktop-owned action so the user can review and confirm it in the UI.
pub(crate) struct FfmpegTools {
    config: FfmpegConfig,
}

impl FfmpegTools {
    pub(crate) fn if_ready(config: FfmpegConfig) -> Option<Self> {
        let cancelled = AtomicBool::new(false);
        let _shared = component_try_read(&cancelled).ok().flatten()?;
        resolve_validated_ffmpeg(&config, &cancelled).ok()?;
        Some(Self { config })
    }
}

fn tool_error(call: &ToolCall, error: impl ToString) -> ToolResult {
    ToolResult {
        tool_call_id: call.id.clone(),
        result_json: serde_json::json!({ "error": error.to_string() }).to_string(),
        is_error: true,
    }
}

impl ToolExecutor for FfmpegTools {
    fn tools(&self) -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: "ffmpeg_probe".into(),
                description: "Inspect one absolute local media file and return structured audio stream facts.".into(),
                input_schema_json: r#"{"type":"object","additionalProperties":false,"properties":{"input":{"type":"string"}},"required":["input"]}"#.into(),
            },
            ToolDefinition {
                name: "ffmpeg_loudness_analyze".into(),
                description: "Measure EBU R128 integrated loudness, true peak, loudness range, and threshold for a local file.".into(),
                input_schema_json: r#"{"type":"object","additionalProperties":false,"properties":{"input":{"type":"string"}},"required":["input"]}"#.into(),
            },
        ]
    }

    fn execute(&self, call: &ToolCall) -> Result<ToolResult, pi_agent_core::PiError> {
        let mut args: serde_json::Value = match serde_json::from_str(&call.arguments_json) {
            Ok(value) => value,
            Err(error) => return Ok(tool_error(call, error)),
        };
        let operation = match call.tool_name.as_str() {
            "ffmpeg_probe" => "probe",
            "ffmpeg_loudness_analyze" => "loudness_analyze",
            other => return Ok(tool_error(call, format!("unknown FFmpeg tool {other}"))),
        };
        let Some(object) = args.as_object_mut() else {
            return Ok(tool_error(call, "tool arguments must be a JSON object"));
        };
        object.insert(
            "operation".into(),
            serde_json::Value::String(operation.into()),
        );
        let request = match serde_json::from_value::<FfmpegRequest>(args) {
            Ok(request) => request,
            Err(error) => return Ok(tool_error(call, format!("invalid arguments: {error}"))),
        };
        let cancelled = AtomicBool::new(false);
        match run_ffmpeg_request(&self.config, request, &cancelled) {
            Ok(result) => Ok(ToolResult {
                tool_call_id: call.id.clone(),
                result_json: result.to_string(),
                is_error: false,
            }),
            Err(error) => Ok(tool_error(call, error)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    fn create_junction(link: &Path, target: &Path) {
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::fs::create_dir_all(target).unwrap();
        let status = std::process::Command::new("cmd.exe")
            .args(["/d", "/c", "mklink", "/j"])
            .arg(link)
            .arg(target)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "failed to create test junction");
    }

    #[cfg(windows)]
    #[test]
    fn component_lock_rejects_component_root_junction_before_opening() {
        let base = std::env::temp_dir().join(format!(
            "pi-component-lock-junction-{}-{}",
            std::process::id(),
            next_job_id()
        ));
        let data_root = base.join("data");
        let outside = base.join("outside");
        let component_paths = ComponentPaths::from_data_root(data_root);
        create_junction(&component_paths.component_root, &outside);

        assert!(matches!(
            process_component_lock_at(&component_paths, &AtomicBool::new(false), false),
            Err(ComponentError::InvalidInput(_))
        ));
        assert!(!outside.join(".component.lock").exists());

        std::fs::remove_dir(&component_paths.component_root).unwrap();
        std::fs::remove_dir_all(base).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn component_lock_rejects_file_symlink_outside_component_root() {
        let base = std::env::temp_dir().join(format!(
            "pi-component-lock-symlink-{}-{}",
            std::process::id(),
            next_job_id()
        ));
        let component_paths = ComponentPaths::from_data_root(base.join("data"));
        std::fs::create_dir_all(&component_paths.component_root).unwrap();
        let outside = base.join("outside.lock");
        std::fs::write(&outside, b"unchanged").unwrap();
        let lock_path = component_paths.component_root.join(".component.lock");
        if let Err(error) = std::os::windows::fs::symlink_file(&outside, &lock_path) {
            if error.raw_os_error() == Some(1314) {
                std::fs::remove_dir_all(base).unwrap();
                return;
            }
            panic!("failed to create test file symlink: {error}");
        }

        assert!(matches!(
            process_component_lock_at(&component_paths, &AtomicBool::new(false), false),
            Err(ComponentError::InvalidInput(_))
        ));
        assert_eq!(std::fs::read(&outside).unwrap(), b"unchanged");

        std::fs::remove_file(lock_path).unwrap();
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn sample_formats_are_whitelisted() {
        assert_eq!(sample_format(Some("s16")).unwrap(), SampleFormat::S16);
        assert!(sample_format(Some("aac")).is_err());
    }

    #[test]
    fn analysis_uses_documented_ffmpeg_defaults_only_for_measurement() {
        let target = analysis_target();
        assert_eq!(target.integrated_lufs, -24.0);
        assert_eq!(target.true_peak_db, -2.0);
        assert_eq!(target.loudness_range, 7.0);
    }

    #[test]
    fn cancelled_jobs_do_not_wait_for_component_gate() {
        let cancelled = AtomicBool::new(true);
        assert!(matches!(
            component_read(&cancelled),
            Err(ComponentError::Cancelled)
        ));
        assert!(matches!(
            component_write(&cancelled),
            Err(ComponentError::Cancelled)
        ));
    }

    #[test]
    fn agent_rejects_audio_creation_tools_before_execution() {
        let tools = FfmpegTools {
            config: FfmpegConfig::default(),
        };
        for tool_name in ["ffmpeg_prepare_audio", "ffmpeg_loudness_normalize"] {
            let result = tools
                .execute(&ToolCall {
                    id: "call-1".into(),
                    tool_name: tool_name.into(),
                    arguments_json: r#"{"input":"C:\\audio.wav","output_name":"out.wav"}"#.into(),
                })
                .unwrap();
            assert!(result.is_error);
            assert!(result.result_json.contains("unknown FFmpeg tool"));
        }
    }

    #[test]
    fn agent_surface_contains_read_only_tools_only() {
        let tools = FfmpegTools {
            config: FfmpegConfig::default(),
        };
        let names = tools
            .tools()
            .into_iter()
            .map(|tool| tool.name)
            .collect::<Vec<_>>();
        assert_eq!(names, ["ffmpeg_probe", "ffmpeg_loudness_analyze"]);
    }
}
