pub use crate::paths::ComponentPaths;
use crate::{
    error::{ComponentError, Result},
    manifest::{FfmpegManifest, FfmpegRelease},
    paths::{ensure_inside, safe_relative_path},
    runner::{os_args, ProcessRunner},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    env,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

static UNIQUE: AtomicU64 = AtomicU64::new(1);
const MAX_DOWNLOAD_BYTES: u64 = 2 * 1024 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Architecture {
    X64,
    Arm64,
}

impl Architecture {
    pub fn host() -> Result<Self> {
        if cfg!(target_arch = "x86_64") {
            Ok(Self::X64)
        } else if cfg!(target_arch = "aarch64") {
            Ok(Self::Arm64)
        } else {
            Err(ComponentError::State(
                "FFmpeg managed builds are available only for x64 and arm64".into(),
            ))
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::X64 => "x64",
            Self::Arm64 => "arm64",
        }
    }
    pub fn release(self) -> FfmpegRelease {
        match self {
            Self::X64 => FfmpegManifest::x64(),
            Self::Arm64 => FfmpegManifest::arm64(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FfmpegSource {
    Explicit,
    Managed,
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedFfmpeg {
    pub source: FfmpegSource,
    pub root: PathBuf,
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct CurrentPointer {
    version: String,
    architecture: Architecture,
    install_dir: String,
}

pub trait Downloader: Send + Sync {
    fn download(
        &self,
        url: &str,
        target: &Path,
        cancelled: &AtomicBool,
        progress: &mut dyn FnMut(u64, Option<u64>),
    ) -> Result<()>;
}

#[derive(Debug, Default)]
pub struct HttpDownloader;

impl Downloader for HttpDownloader {
    fn download(
        &self,
        url: &str,
        target: &Path,
        cancelled: &AtomicBool,
        progress: &mut dyn FnMut(u64, Option<u64>),
    ) -> Result<()> {
        if cancelled.load(Ordering::Acquire) {
            return Err(ComponentError::Cancelled);
        }
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(15))
            .timeout_read(Duration::from_secs(15))
            .timeout_write(Duration::from_secs(15))
            .build();
        let response = agent
            .get(url)
            .call()
            .map_err(|error| ComponentError::Download(error.to_string()))?;
        let total = response
            .header("Content-Length")
            .and_then(|v| v.parse::<u64>().ok());
        if total.is_some_and(|bytes| bytes > MAX_DOWNLOAD_BYTES) {
            return Err(ComponentError::Download(
                "FFmpeg archive exceeds the 2 GiB component limit".into(),
            ));
        }
        let mut reader = response.into_reader();
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(target)?;
        let mut bytes = 0_u64;
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            if cancelled.load(Ordering::Acquire) {
                drop(file);
                let _ = fs::remove_file(target);
                return Err(ComponentError::Cancelled);
            }
            let read = reader.read(&mut buffer).map_err(ComponentError::Io)?;
            if read == 0 {
                break;
            }
            file.write_all(&buffer[..read])?;
            bytes += read as u64;
            if bytes > MAX_DOWNLOAD_BYTES {
                drop(file);
                let _ = fs::remove_file(target);
                return Err(ComponentError::Download(
                    "FFmpeg archive exceeds the 2 GiB component limit".into(),
                ));
            }
            progress(bytes, total);
        }
        file.sync_all()?;
        Ok(())
    }
}

pub struct FfmpegResolver {
    paths: ComponentPaths,
}

impl FfmpegResolver {
    pub fn new(paths: ComponentPaths) -> Self {
        Self { paths }
    }
    pub fn paths(&self) -> &ComponentPaths {
        &self.paths
    }

    /// Resolve an explicitly supplied pair first, then the active private pair,
    /// then the user's existing PATH. This checks file presence only; callers
    /// must run a bounded health check before use. It never edits PATH.
    pub fn resolve(&self, explicit: Option<&Path>) -> Result<ResolvedFfmpeg> {
        if let Some(path) = explicit {
            return self.resolve_at(path, FfmpegSource::Explicit, None);
        }
        match self.resolve_managed() {
            Ok(resolved) => return Ok(resolved),
            Err(ComponentError::NotFound(_)) => {}
            Err(error) => return Err(error),
        }
        self.resolve_system()
    }

    /// Resolve only the private, hash-verified component selected by
    /// `current.json`; never falls back to PATH.
    pub fn resolve_managed(&self) -> Result<ResolvedFfmpeg> {
        let Some((version, _architecture, dir)) = self.read_current()? else {
            return Err(ComponentError::NotFound(
                "managed FFmpeg is not installed".into(),
            ));
        };
        ensure_inside(&self.paths.component_root, &dir)?;
        self.resolve_at(&dir, FfmpegSource::Managed, Some(version))
    }

    pub fn read_current(&self) -> Result<Option<(String, Architecture, PathBuf)>> {
        let pointer = self.paths.current_pointer();
        self.paths.validate_component_path(&pointer)?;
        if !pointer.exists() {
            return Ok(None);
        }
        let contents = fs::read_to_string(&pointer)?;
        let current: CurrentPointer = serde_json::from_str(&contents)?;
        let install_dir = PathBuf::from(current.install_dir);
        ensure_inside(&self.paths.component_root, &install_dir)?;
        Ok(Some((current.version, current.architecture, install_dir)))
    }

    fn resolve_at(
        &self,
        supplied: &Path,
        source: FfmpegSource,
        version: Option<String>,
    ) -> Result<ResolvedFfmpeg> {
        let directory = if supplied.is_file() {
            supplied.parent().ok_or_else(|| {
                ComponentError::InvalidInput("FFmpeg executable has no parent directory".into())
            })?
        } else {
            supplied
        };
        let candidates = [directory.join("bin"), directory.to_path_buf()];
        for bin in candidates {
            let ffmpeg = bin.join(exe_name("ffmpeg"));
            let ffprobe = bin.join(exe_name("ffprobe"));
            if ffmpeg.is_file() && ffprobe.is_file() {
                return Ok(ResolvedFfmpeg {
                    source,
                    root: directory.to_path_buf(),
                    ffmpeg,
                    ffprobe,
                    version,
                });
            }
        }
        Err(ComponentError::NotFound(format!(
            "ffmpeg and ffprobe not found beneath {}",
            supplied.display()
        )))
    }

    /// Resolve only an existing FFmpeg pair on the process PATH.
    pub fn resolve_system(&self) -> Result<ResolvedFfmpeg> {
        let path = env::var_os("PATH").ok_or_else(|| {
            ComponentError::NotFound("PATH is not set and no managed FFmpeg exists".into())
        })?;
        for dir in env::split_paths(&path) {
            if let Ok(resolved) = self.resolve_at(&dir, FfmpegSource::System, None) {
                return Ok(resolved);
            }
        }
        Err(ComponentError::NotFound(
            "FFmpeg not found; install the managed component or choose an explicit folder".into(),
        ))
    }
}

pub struct FfmpegInstaller<D, R> {
    paths: ComponentPaths,
    downloader: D,
    runner: R,
}

impl<D: Downloader, R: ProcessRunner> FfmpegInstaller<D, R> {
    pub fn new(paths: ComponentPaths, downloader: D, runner: R) -> Self {
        Self {
            paths,
            downloader,
            runner,
        }
    }
    pub fn paths(&self) -> &ComponentPaths {
        &self.paths
    }

    pub fn install(
        &self,
        architecture: Architecture,
        cancelled: &AtomicBool,
        report: &mut dyn FnMut(&str, f32),
    ) -> Result<ResolvedFfmpeg> {
        let release = architecture.release();
        self.paths.validate_component_root()?;
        self.paths.validate_downloads_dir()?;
        let staging_root = self.paths.staging_root();
        self.paths.validate_component_path(&staging_root)?;
        fs::create_dir_all(&self.paths.component_root)?;
        fs::create_dir_all(&self.paths.downloads_dir)?;
        fs::create_dir_all(&staging_root)?;
        self.paths.validate_component_root()?;
        self.paths.validate_downloads_dir()?;
        self.paths.validate_component_path(&staging_root)?;
        let token = unique_token();
        let download = self
            .paths
            .downloads_dir
            .join(format!("{}.{}.part", release.asset, token));
        let staging = self
            .paths
            .staging_root()
            .join(format!("{}.{}", release.version, token));
        let install_dir = self.paths.component_root.join(format!(
            "{}-{}",
            release.version,
            architecture.as_str()
        ));
        let result = (|| {
            report("downloading", 0.0);
            self.downloader
                .download(release.url, &download, cancelled, &mut |done, total| {
                    let fraction = total
                        .filter(|n| *n > 0)
                        .map(|n| done as f32 / n as f32)
                        .unwrap_or(0.0);
                    report("downloading", fraction.clamp(0.0, 0.8));
                })?;
            check_cancelled(cancelled)?;
            report("verifying", 0.82);
            verify_sha256(&download, release.sha256, cancelled)?;
            report("extracting", 0.86);
            fs::create_dir_all(&staging)?;
            ensure_inside(&self.paths.component_root, &staging)?;
            extract_zip_safely(&download, &staging, cancelled)?;
            check_cancelled(cancelled)?;
            let extracted_root = find_root_with_tools(&staging)?;
            report("checking", 0.94);
            let candidate = FfmpegResolver::new(self.paths.clone()).resolve_at(
                &extracted_root,
                FfmpegSource::Managed,
                Some(release.version.into()),
            )?;
            self.health_check(&candidate, cancelled)?;
            // Last cancellable point before replacing the active version and
            // atomically publishing current.json.
            check_cancelled(cancelled)?;
            let resolved = self.activate_verified(
                &release,
                &extracted_root,
                &install_dir,
                &format!("activate-{token}"),
            )?;
            report("complete", 1.0);
            let _ = fs::remove_dir_all(&staging);
            Ok(resolved)
        })();
        let _ = fs::remove_file(&download);
        if result.is_err() {
            let _ = fs::remove_dir_all(&staging);
        }
        result
    }

    /// Activate only the directory extracted from the checksum-verified
    /// archive. A pre-existing version directory is moved aside and is never
    /// executed or trusted as a substitute for the verified staging tree.
    fn activate_verified(
        &self,
        release: &FfmpegRelease,
        extracted_root: &Path,
        install_dir: &Path,
        token: &str,
    ) -> Result<ResolvedFfmpeg> {
        self.paths.validate_component_root()?;
        ensure_inside(&self.paths.component_root, extracted_root)?;
        ensure_inside(&self.paths.component_root, install_dir)?;
        let backup = self.paths.component_root.join(format!(".previous-{token}"));
        ensure_inside(&self.paths.component_root, &backup)?;
        let had_previous = install_dir.exists();
        if had_previous {
            fs::rename(install_dir, &backup)?;
        }
        if let Err(error) = fs::rename(extracted_root, install_dir) {
            if had_previous {
                if let Err(rollback) = fs::rename(&backup, install_dir) {
                    return Err(ComponentError::State(format!(
                        "failed to activate verified FFmpeg ({error}) and restore the previous install ({rollback})"
                    )));
                }
            }
            return Err(ComponentError::Io(error));
        }

        let activation = (|| {
            let resolved = FfmpegResolver::new(self.paths.clone()).resolve_at(
                install_dir,
                FfmpegSource::Managed,
                Some(release.version.into()),
            )?;
            self.record_provenance(release, install_dir)?;
            self.write_current(release, install_dir)?;
            Ok(resolved)
        })();
        match activation {
            Ok(resolved) => {
                if had_previous {
                    let _ = fs::remove_dir_all(backup);
                }
                Ok(resolved)
            }
            Err(error) => {
                let failed = self.paths.component_root.join(format!(".failed-{token}"));
                let move_failed = fs::rename(install_dir, &failed);
                let restore_failed = if had_previous {
                    fs::rename(&backup, install_dir).err()
                } else {
                    None
                };
                if move_failed.is_ok() {
                    let _ = fs::remove_dir_all(failed);
                }
                if let Err(move_error) = move_failed {
                    return Err(ComponentError::State(format!(
                        "activation failed ({error}) and the verified install could not be moved aside ({move_error})"
                    )));
                }
                if let Some(restore_error) = restore_failed {
                    return Err(ComponentError::State(format!(
                        "activation failed ({error}) and the previous install could not be restored ({restore_error})"
                    )));
                }
                Err(error)
            }
        }
    }

    pub fn uninstall_managed(&self) -> Result<bool> {
        let resolver = FfmpegResolver::new(self.paths.clone());
        let Some((_version, _architecture, install_dir)) = resolver.read_current()? else {
            return Ok(false);
        };
        ensure_inside(&self.paths.component_root, &install_dir)?;
        let pointer = self.paths.current_pointer();
        if install_dir.exists() {
            let tombstone = self
                .paths
                .component_root
                .join(format!(".uninstall-{}", unique_token()));
            ensure_inside(&self.paths.component_root, &tombstone)?;
            // Rename first. If this fails, current.json still points at the
            // untouched active installation. Once it succeeds, resolution can
            // safely fall back even if best-effort tombstone cleanup fails.
            fs::rename(&install_dir, &tombstone)?;
            if pointer.exists() {
                if let Err(error) = fs::remove_file(&pointer) {
                    if let Err(rollback) = fs::rename(&tombstone, &install_dir) {
                        return Err(ComponentError::State(format!(
                            "failed to remove current pointer ({error}) and restore managed FFmpeg ({rollback})"
                        )));
                    }
                    return Err(ComponentError::Io(error));
                }
            }
            let _ = fs::remove_dir_all(tombstone);
        } else if pointer.exists() {
            fs::remove_file(pointer)?;
        }
        Ok(true)
    }

    fn health_check(&self, resolved: &ResolvedFfmpeg, cancelled: &AtomicBool) -> Result<()> {
        for (name, executable) in [("ffmpeg", &resolved.ffmpeg), ("ffprobe", &resolved.ffprobe)] {
            let result = self.runner.run_with_timeout(
                executable,
                &os_args(["-hide_banner", "-version"]),
                cancelled,
                Duration::from_secs(5),
            )?;
            if !result.success {
                return Err(ComponentError::Process {
                    message: format!("{name} -version failed health check"),
                    stderr: result.stderr,
                });
            }
        }
        Ok(())
    }

    fn record_provenance(&self, release: &FfmpegRelease, install_dir: &Path) -> Result<()> {
        self.paths.validate_component_root()?;
        let license_dir = self.paths.component_root.join("license");
        ensure_inside(&self.paths.component_root, &license_dir)?;
        fs::create_dir_all(&license_dir)?;
        let provenance = serde_json::json!({
            "provider": "BtbN/FFmpeg-Builds",
            "release_tag": FfmpegManifest::RELEASE_TAG,
            "version": release.version,
            "architecture": release.architecture,
            "asset": release.asset,
            "url": release.url,
            "sha256": release.sha256,
            "license_variant": FfmpegManifest::LICENSE,
            "build_project": FfmpegManifest::BUILD_PROJECT_URL,
            "ffmpeg_source": FfmpegManifest::SOURCE_URL,
            "ffmpeg_legal": "https://ffmpeg.org/legal.html"
        });
        fs::write(
            license_dir.join("provenance.json"),
            format!("{}\n", serde_json::to_string_pretty(&provenance)?),
        )?;
        fs::write(
            license_dir.join("NOTICE.txt"),
            "This managed component uses an LGPL build of FFmpeg.\r\n\
             FFmpeg legal information: https://ffmpeg.org/legal.html\r\n\
             Build project: https://github.com/BtbN/FFmpeg-Builds\r\n\
             FFmpeg source: https://github.com/FFmpeg/FFmpeg/tree/9b6c8969e0\r\n",
        )?;
        for name in [
            "LICENSE.txt",
            "LICENSE",
            "COPYING.LGPLv3",
            "COPYING.LGPLv2.1",
        ] {
            let source = install_dir.join(name);
            if source.is_file() {
                fs::copy(&source, license_dir.join(name))?;
            }
        }
        Ok(())
    }

    fn write_current(&self, release: &FfmpegRelease, install_dir: &Path) -> Result<()> {
        self.paths.validate_component_root()?;
        ensure_inside(&self.paths.component_root, install_dir)?;
        let current_pointer = self.paths.current_pointer();
        self.paths.validate_component_path(&current_pointer)?;
        let pointer = CurrentPointer {
            version: release.version.into(),
            architecture: if release.architecture == "x64" {
                Architecture::X64
            } else {
                Architecture::Arm64
            },
            install_dir: install_dir.to_string_lossy().into_owned(),
        };
        let temporary = self
            .paths
            .component_root
            .join(format!("current.{}.next.json", unique_token()));
        self.paths.validate_component_path(&temporary)?;
        {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            serde_json::to_writer_pretty(&mut file, &pointer)?;
            file.write_all(b"\n")?;
            file.sync_all()?;
        }
        self.paths.validate_component_path(&current_pointer)?;
        replace_file_atomically(&temporary, &current_pointer)
    }
}

fn exe_name(base: &str) -> String {
    if cfg!(windows) {
        format!("{base}.exe")
    } else {
        base.into()
    }
}
fn check_cancelled(cancelled: &AtomicBool) -> Result<()> {
    if cancelled.load(Ordering::Acquire) {
        Err(ComponentError::Cancelled)
    } else {
        Ok(())
    }
}
fn unique_token() -> String {
    let sequence = UNIQUE.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{nanos}-{sequence}")
}

fn verify_sha256(path: &Path, expected: &str, cancelled: &AtomicBool) -> Result<()> {
    if expected.len() != 64 || !expected.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err(ComponentError::State("manifest SHA-256 is invalid".into()));
    }
    let mut file = File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        check_cancelled(cancelled)?;
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        digest.update(&buffer[..n]);
    }
    let actual = format!("{:x}", digest.finalize());
    if actual.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        Err(ComponentError::Integrity {
            expected: expected.into(),
            actual,
        })
    }
}

fn extract_zip_safely(archive_path: &Path, staging: &Path, cancelled: &AtomicBool) -> Result<()> {
    const MAX_FILES: usize = 10_000;
    const MAX_UNCOMPRESSED: u64 = 1_000_000_000;
    let file = File::open(archive_path)?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|e| ComponentError::Archive(e.to_string()))?;
    if archive.len() > MAX_FILES {
        return Err(ComponentError::Archive("too many archive entries".into()));
    }
    let mut uncompressed = 0_u64;
    for index in 0..archive.len() {
        check_cancelled(cancelled)?;
        let mut entry = archive
            .by_index(index)
            .map_err(|e| ComponentError::Archive(e.to_string()))?;
        uncompressed = uncompressed
            .checked_add(entry.size())
            .ok_or_else(|| ComponentError::Archive("uncompressed size overflow".into()))?;
        if uncompressed > MAX_UNCOMPRESSED {
            return Err(ComponentError::Archive(
                "archive exceeds uncompressed size limit".into(),
            ));
        }
        if entry
            .unix_mode()
            .map(|mode| mode & 0o170000 == 0o120000)
            .unwrap_or(false)
        {
            return Err(ComponentError::Archive(
                "symbolic links are not allowed".into(),
            ));
        }
        let name = entry.enclosed_name().ok_or_else(|| {
            ComponentError::Archive(format!("unsafe entry name: {}", entry.name()))
        })?;
        let relative = safe_relative_path(&name)?;
        let target = staging.join(relative);
        ensure_inside(staging, &target)?;
        if entry.is_dir() {
            fs::create_dir_all(&target)?;
        } else {
            let parent = target
                .parent()
                .ok_or_else(|| ComponentError::Archive("archive entry has no parent".into()))?;
            fs::create_dir_all(parent)?;
            let mut output = File::create(&target)?;
            let mut buffer = [0_u8; 64 * 1024];
            loop {
                check_cancelled(cancelled)?;
                let read = entry.read(&mut buffer)?;
                if read == 0 {
                    break;
                }
                output.write_all(&buffer[..read])?;
            }
        }
    }
    Ok(())
}

fn find_root_with_tools(staging: &Path) -> Result<PathBuf> {
    fn visit(dir: &Path, depth: u8) -> Option<PathBuf> {
        if depth > 4 {
            return None;
        }
        let bin = dir.join("bin");
        if bin.join(exe_name("ffmpeg")).is_file() && bin.join(exe_name("ffprobe")).is_file() {
            return Some(dir.to_path_buf());
        }
        let entries = fs::read_dir(dir).ok()?;
        for entry in entries.flatten() {
            if entry.file_type().ok()?.is_dir() {
                if let Some(found) = visit(&entry.path(), depth + 1) {
                    return Some(found);
                }
            }
        }
        None
    }
    visit(staging, 0).ok_or_else(|| {
        ComponentError::Archive("archive does not contain ffmpeg and ffprobe under bin/".into())
    })
}

#[cfg(windows)]
fn replace_file_atomically(source: &Path, destination: &Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    unsafe extern "system" {
        fn MoveFileExW(existing: *const u16, new: *const u16, flags: u32) -> i32;
    }
    const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;
    let old: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let new: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    if unsafe {
        MoveFileExW(
            old.as_ptr(),
            new.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        return Err(ComponentError::Io(std::io::Error::last_os_error()));
    }
    Ok(())
}

#[cfg(not(windows))]
fn replace_file_atomically(source: &Path, destination: &Path) -> Result<()> {
    fs::rename(source, destination)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::ProcessOutput;
    use std::{
        fs,
        io::Write,
        sync::{atomic::AtomicBool, Mutex},
    };

    fn temp_root(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("pi-components-{label}-{}", unique_token()));
        fs::create_dir_all(&path).unwrap();
        path
    }
    fn tools(dir: &Path) {
        let bin = dir.join("bin");
        fs::create_dir_all(&bin).unwrap();
        fs::write(bin.join(exe_name("ffmpeg")), b"x").unwrap();
        fs::write(bin.join(exe_name("ffprobe")), b"x").unwrap();
    }
    struct FakeDownloader {
        source: PathBuf,
    }
    impl Downloader for FakeDownloader {
        fn download(
            &self,
            _url: &str,
            target: &Path,
            _cancelled: &AtomicBool,
            _progress: &mut dyn FnMut(u64, Option<u64>),
        ) -> Result<()> {
            fs::copy(&self.source, target)?;
            Ok(())
        }
    }
    struct FakeRunner;
    impl ProcessRunner for FakeRunner {
        fn run(
            &self,
            _e: &Path,
            _a: &[std::ffi::OsString],
            _c: &AtomicBool,
        ) -> Result<ProcessOutput> {
            Ok(ProcessOutput {
                success: true,
                ..Default::default()
            })
        }
    }
    #[derive(Default)]
    struct CountingRunner {
        executables: Mutex<Vec<PathBuf>>,
    }
    impl ProcessRunner for CountingRunner {
        fn run(
            &self,
            executable: &Path,
            _a: &[std::ffi::OsString],
            _c: &AtomicBool,
        ) -> Result<ProcessOutput> {
            self.executables
                .lock()
                .unwrap()
                .push(executable.to_path_buf());
            Ok(ProcessOutput {
                success: true,
                ..Default::default()
            })
        }
    }

    #[cfg(windows)]
    fn create_junction(link: &Path, target: &Path) {
        fs::create_dir_all(link.parent().unwrap()).unwrap();
        fs::create_dir_all(target).unwrap();
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
    fn installer_for_unreachable_download(
        paths: ComponentPaths,
        base: &Path,
    ) -> FfmpegInstaller<FakeDownloader, FakeRunner> {
        FfmpegInstaller::new(
            paths,
            FakeDownloader {
                source: base.join("downloader-must-not-run"),
            },
            FakeRunner,
        )
    }

    #[cfg(windows)]
    #[test]
    fn install_rejects_component_root_junction_before_writing() {
        let base = temp_root("component-root-junction");
        let data_root = base.join("data");
        let outside = base.join("outside");
        let paths = ComponentPaths::from_data_root(data_root);
        create_junction(&paths.component_root, &outside);

        let result = installer_for_unreachable_download(paths.clone(), &base).install(
            Architecture::X64,
            &AtomicBool::new(false),
            &mut |_, _| {},
        );
        assert!(matches!(result, Err(ComponentError::InvalidInput(_))));
        assert!(fs::read_dir(&outside).unwrap().next().is_none());

        fs::remove_dir(&paths.component_root).unwrap();
        fs::remove_dir_all(base).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn install_rejects_downloads_junction_before_writing() {
        let base = temp_root("downloads-junction");
        let data_root = base.join("data");
        let outside = base.join("outside");
        let paths = ComponentPaths::from_data_root(data_root);
        create_junction(&paths.downloads_dir, &outside);

        let result = installer_for_unreachable_download(paths.clone(), &base).install(
            Architecture::X64,
            &AtomicBool::new(false),
            &mut |_, _| {},
        );
        assert!(matches!(result, Err(ComponentError::InvalidInput(_))));
        assert!(fs::read_dir(&outside).unwrap().next().is_none());
        assert!(!paths.component_root.exists());

        fs::remove_dir(&paths.downloads_dir).unwrap();
        fs::remove_dir_all(base).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn install_rejects_staging_junction_before_writing() {
        let base = temp_root("staging-junction");
        let data_root = base.join("data");
        let outside = base.join("outside");
        let paths = ComponentPaths::from_data_root(data_root);
        fs::create_dir_all(&paths.component_root).unwrap();
        let staging_root = paths.staging_root();
        create_junction(&staging_root, &outside);

        let result = installer_for_unreachable_download(paths.clone(), &base).install(
            Architecture::X64,
            &AtomicBool::new(false),
            &mut |_, _| {},
        );
        assert!(matches!(result, Err(ComponentError::InvalidInput(_))));
        assert!(fs::read_dir(&outside).unwrap().next().is_none());

        fs::remove_dir(&staging_root).unwrap();
        fs::remove_dir_all(base).unwrap();
    }

    #[cfg(any(unix, windows))]
    fn create_file_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(target, link)
        }
        #[cfg(windows)]
        {
            std::os::windows::fs::symlink_file(target, link)
        }
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn current_pointer_file_symlink_outside_component_root_is_rejected() {
        let base = temp_root("current-symlink");
        let data_root = base.join("data");
        let paths = ComponentPaths::from_data_root(data_root);
        fs::create_dir_all(&paths.component_root).unwrap();
        let external_pointer = base.join("outside-current.json");
        fs::write(
            &external_pointer,
            serde_json::to_string(&CurrentPointer {
                version: "outside".into(),
                architecture: Architecture::X64,
                install_dir: paths
                    .component_root
                    .join("outside")
                    .to_string_lossy()
                    .into_owned(),
            })
            .unwrap(),
        )
        .unwrap();
        if let Err(error) = create_file_symlink(&external_pointer, &paths.current_pointer()) {
            #[cfg(windows)]
            if error.raw_os_error() == Some(1314) {
                fs::remove_dir_all(base).unwrap();
                return;
            }
            panic!("failed to create test file symlink: {error}");
        }

        assert!(matches!(
            FfmpegResolver::new(paths.clone()).read_current(),
            Err(ComponentError::InvalidInput(_))
        ));
        fs::remove_file(paths.current_pointer()).unwrap();
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn resolver_prefers_explicit_then_managed() {
        let root = temp_root("resolve");
        let paths = ComponentPaths::from_data_root(root.clone());
        let explicit = root.join("explicit");
        tools(&explicit);
        let managed = paths.component_root.join("managed");
        tools(&managed);
        fs::create_dir_all(&paths.component_root).unwrap();
        fs::write(
            paths.current_pointer(),
            serde_json::to_string(&CurrentPointer {
                version: "v".into(),
                architecture: Architecture::X64,
                install_dir: managed.to_string_lossy().into_owned(),
            })
            .unwrap(),
        )
        .unwrap();
        let resolver = FfmpegResolver::new(paths);
        assert_eq!(
            resolver.resolve(Some(&explicit)).unwrap().source,
            FfmpegSource::Explicit
        );
        assert_eq!(
            resolver.resolve(None).unwrap().source,
            FfmpegSource::Managed
        );
        assert_eq!(
            resolver.resolve_managed().unwrap().source,
            FfmpegSource::Managed
        );
        let _ = fs::remove_dir_all(root);
    }
    #[test]
    fn current_pointer_cannot_escape_component_root() {
        let root = temp_root("escape");
        let paths = ComponentPaths::from_data_root(root.clone());
        fs::create_dir_all(&paths.component_root).unwrap();
        fs::write(
            paths.current_pointer(),
            serde_json::to_string(&CurrentPointer {
                version: "v".into(),
                architecture: Architecture::X64,
                install_dir: root.join("outside").to_string_lossy().into_owned(),
            })
            .unwrap(),
        )
        .unwrap();
        assert!(FfmpegResolver::new(paths).read_current().is_err());
        let _ = fs::remove_dir_all(root);
    }
    #[test]
    fn managed_preference_never_falls_back_to_path() {
        let root = temp_root("managed-only");
        let paths = ComponentPaths::from_data_root(root.clone());
        let resolver = FfmpegResolver::new(paths);
        assert!(matches!(
            resolver.resolve_managed(),
            Err(ComponentError::NotFound(_))
        ));
        let _ = fs::remove_dir_all(root);
    }
    #[test]
    fn installer_accepts_injected_downloader_and_runner_types() {
        let root = temp_root("injected");
        let paths = ComponentPaths::from_data_root(root.clone());
        let installer = FfmpegInstaller::new(
            paths,
            FakeDownloader {
                source: root.join("fixture.zip"),
            },
            FakeRunner,
        );
        // Compile-time API check: network/process behavior remains injectable.
        assert!(installer.paths().component_root.ends_with("ffmpeg"));
        let _ = fs::remove_dir_all(root);
    }
    #[test]
    fn health_check_runs_both_ffmpeg_and_ffprobe() {
        let root = temp_root("health");
        let paths = ComponentPaths::from_data_root(root.clone());
        let install = paths.component_root.join("installed");
        tools(&install);
        let installer = FfmpegInstaller::new(
            paths,
            FakeDownloader {
                source: root.join("fixture.zip"),
            },
            CountingRunner::default(),
        );
        let resolved = FfmpegResolver::new(installer.paths().clone())
            .resolve_at(&install, FfmpegSource::Managed, Some("v".into()))
            .unwrap();
        installer
            .health_check(&resolved, &AtomicBool::new(false))
            .unwrap();
        assert_eq!(installer.runner.executables.lock().unwrap().len(), 2);
        let _ = fs::remove_dir_all(root);
    }
    #[test]
    fn checksum_mismatch_fails_closed() {
        let root = temp_root("hash");
        let file = root.join("x");
        fs::write(&file, b"not the expected archive").unwrap();
        assert!(matches!(
            verify_sha256(&file, &"0".repeat(64), &AtomicBool::new(false)),
            Err(ComponentError::Integrity { .. })
        ));
        let _ = fs::remove_dir_all(root);
    }
    #[test]
    fn zip_traversal_is_rejected_before_writing_outside_staging() {
        use zip::write::SimpleFileOptions;
        let root = temp_root("zip-slip");
        let archive = root.join("bad.zip");
        let mut writer = zip::ZipWriter::new(File::create(&archive).unwrap());
        writer
            .start_file("../outside.txt", SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"nope").unwrap();
        writer.finish().unwrap();
        let staging = root.join("staging");
        fs::create_dir_all(&staging).unwrap();
        assert!(extract_zip_safely(&archive, &staging, &AtomicBool::new(false)).is_err());
        assert!(!root.join("outside.txt").exists());
        let _ = fs::remove_dir_all(root);
    }
    #[test]
    fn uninstall_removes_only_current_managed_install() {
        let root = temp_root("uninstall");
        let paths = ComponentPaths::from_data_root(root.clone());
        let managed = paths.component_root.join("managed");
        tools(&managed);
        fs::create_dir_all(&paths.component_root).unwrap();
        fs::write(
            paths.current_pointer(),
            serde_json::to_string(&CurrentPointer {
                version: "v".into(),
                architecture: Architecture::X64,
                install_dir: managed.to_string_lossy().into_owned(),
            })
            .unwrap(),
        )
        .unwrap();
        let installer = FfmpegInstaller::new(
            paths.clone(),
            FakeDownloader {
                source: root.join("fixture.zip"),
            },
            FakeRunner,
        );
        assert!(installer.uninstall_managed().unwrap());
        assert!(!managed.exists());
        assert!(!paths.current_pointer().exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn verified_staging_replaces_an_existing_version_directory() {
        let root = temp_root("replace-existing");
        let paths = ComponentPaths::from_data_root(root.clone());
        let install = paths.component_root.join("release-x64");
        let extracted = paths.staging_root().join("verified");
        tools(&install);
        tools(&extracted);
        fs::write(install.join("bin").join(exe_name("ffmpeg")), b"old").unwrap();
        fs::write(extracted.join("bin").join(exe_name("ffmpeg")), b"verified").unwrap();
        let installer = FfmpegInstaller::new(
            paths,
            FakeDownloader {
                source: root.join("fixture.zip"),
            },
            FakeRunner,
        );
        installer
            .activate_verified(
                &FfmpegManifest::x64(),
                &extracted,
                &install,
                "replacement-test",
            )
            .unwrap();
        assert_eq!(
            fs::read(install.join("bin").join(exe_name("ffmpeg"))).unwrap(),
            b"verified"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(windows)]
    #[test]
    fn uninstall_restores_install_when_pointer_cannot_be_removed() {
        use std::os::windows::fs::OpenOptionsExt;

        let root = temp_root("uninstall-rollback");
        let paths = ComponentPaths::from_data_root(root.clone());
        let managed = paths.component_root.join("managed");
        tools(&managed);
        fs::create_dir_all(&paths.component_root).unwrap();
        fs::write(
            paths.current_pointer(),
            serde_json::to_string(&CurrentPointer {
                version: "v".into(),
                architecture: Architecture::X64,
                install_dir: managed.to_string_lossy().into_owned(),
            })
            .unwrap(),
        )
        .unwrap();
        let pointer = paths.current_pointer();
        let delete_blocker = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&pointer)
            .unwrap();
        let installer = FfmpegInstaller::new(
            paths,
            FakeDownloader {
                source: root.join("fixture.zip"),
            },
            FakeRunner,
        );
        assert!(installer.uninstall_managed().is_err());
        assert!(managed.exists());
        assert!(pointer.exists());
        drop(delete_blocker);
        let _ = fs::remove_dir_all(root);
    }
}
