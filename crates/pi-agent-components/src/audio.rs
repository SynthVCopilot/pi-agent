use crate::{
    error::{ComponentError, Result},
    installer::ResolvedFfmpeg,
    paths::{ensure_inside, safe_output_name, validate_local_file, ComponentPaths},
    runner::{ProcessOutput, ProcessRunner},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioProbe {
    pub format_name: Option<String>,
    pub duration_seconds: Option<f64>,
    pub bit_rate: Option<u64>,
    pub audio_streams: Vec<AudioStream>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioStream {
    pub codec_name: Option<String>,
    pub sample_rate: Option<u32>,
    pub channels: Option<u8>,
    pub sample_fmt: Option<String>,
    pub bit_rate: Option<u64>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SampleFormat {
    S16,
    S24,
    F32,
}

impl SampleFormat {
    fn codec(self) -> &'static str {
        match self {
            Self::S16 => "pcm_s16le",
            Self::S24 => "pcm_s24le",
            Self::F32 => "pcm_f32le",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrepareWavRequest {
    pub input: PathBuf,
    /// A plain `.wav` filename only; it is always written below output/ffmpeg.
    pub output_name: Option<String>,
    pub start_seconds: Option<f64>,
    pub duration_seconds: Option<f64>,
    pub sample_rate: Option<u32>,
    pub channels: Option<u8>,
    pub sample_format: SampleFormat,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreparedAudio {
    pub output: PathBuf,
    pub sample_format: SampleFormat,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoudnessTarget {
    pub integrated_lufs: f64,
    pub true_peak_db: f64,
    pub loudness_range: f64,
}

impl LoudnessTarget {
    pub fn validate(&self) -> Result<()> {
        if !(-70.0..=-5.0).contains(&self.integrated_lufs) {
            return Err(ComponentError::InvalidInput(
                "loudnorm I must be within -70..=-5 LUFS".into(),
            ));
        }
        if !(-9.0..=0.0).contains(&self.true_peak_db) {
            return Err(ComponentError::InvalidInput(
                "loudnorm TP must be within -9..=0 dB".into(),
            ));
        }
        if !(1.0..=50.0).contains(&self.loudness_range) {
            return Err(ComponentError::InvalidInput(
                "loudnorm LRA must be within 1..=50 LU".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NormalizeRequest {
    pub input: PathBuf,
    pub output_name: Option<String>,
    pub target: LoudnessTarget,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoudnessMeasurement {
    pub integrated_lufs: f64,
    pub true_peak_db: f64,
    pub loudness_range: f64,
    pub threshold: f64,
    pub target_offset: f64,
    pub raw: Value,
}

/// FFmpeg operations exposed to callers. Every method constructs arguments
/// itself; there is no user-controlled argument or filter field.
pub struct AudioExecutor<R> {
    paths: ComponentPaths,
    ffmpeg: ResolvedFfmpeg,
    runner: R,
}

impl<R: ProcessRunner> AudioExecutor<R> {
    pub fn new(paths: ComponentPaths, ffmpeg: ResolvedFfmpeg, runner: R) -> Self {
        Self {
            paths,
            ffmpeg,
            runner,
        }
    }

    pub fn probe(&self, input: &Path, cancelled: &AtomicBool) -> Result<AudioProbe> {
        let input = validate_local_file(input)?;
        let args = args([
            "-v",
            "error",
            "-select_streams",
            "a:0",
            "-show_entries",
            "format=format_name,duration,bit_rate:stream=codec_type,codec_name,sample_rate,channels,sample_fmt,bit_rate",
            "-of",
            "json",
        ])
        .into_iter()
        .chain(std::iter::once(input.into_os_string()))
        .collect::<Vec<_>>();
        let output = self.runner.run(&self.ffmpeg.ffprobe, &args, cancelled)?;
        require_success(&output, "ffprobe")?;
        let value: Value = serde_json::from_str(&output.stdout)?;
        let format = value.get("format").cloned().unwrap_or(Value::Null);
        let audio_streams = value
            .get("streams")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|stream| stream.get("codec_type").and_then(Value::as_str) == Some("audio"))
            .map(|stream| AudioStream {
                codec_name: string(stream, "codec_name"),
                sample_rate: number(stream, "sample_rate"),
                channels: number(stream, "channels"),
                sample_fmt: string(stream, "sample_fmt"),
                bit_rate: number(stream, "bit_rate"),
            })
            .collect();
        Ok(AudioProbe {
            format_name: string(&format, "format_name"),
            duration_seconds: number(&format, "duration"),
            bit_rate: number(&format, "bit_rate"),
            audio_streams,
        })
    }

    pub fn prepare_wav(
        &self,
        request: &PrepareWavRequest,
        cancelled: &AtomicBool,
    ) -> Result<PreparedAudio> {
        let input = validate_local_file(&request.input)?;
        validate_segment(request.start_seconds, request.duration_seconds)?;
        if let Some(rate) = request.sample_rate {
            if !(8_000..=192_000).contains(&rate) {
                return Err(ComponentError::InvalidInput(
                    "sample rate must be 8000..=192000 Hz".into(),
                ));
            }
        }
        if let Some(channels) = request.channels {
            if !(1..=2).contains(&channels) {
                return Err(ComponentError::InvalidInput(
                    "channels must be 1 or 2".into(),
                ));
            }
        }
        let (output, temporary) = self.output_paths(request.output_name.as_deref(), "prepared")?;
        let mut command = args(["-nostdin", "-n"]);
        if let Some(start) = request.start_seconds {
            command.push(OsString::from("-ss"));
            command.push(OsString::from(decimal(start)));
        }
        command.extend([OsString::from("-i"), input.into_os_string()]);
        if let Some(duration) = request.duration_seconds {
            command.push(OsString::from("-t"));
            command.push(OsString::from(decimal(duration)));
        }
        if let Some(rate) = request.sample_rate {
            command.push(OsString::from("-ar"));
            command.push(OsString::from(rate.to_string()));
        }
        if let Some(channels) = request.channels {
            command.push(OsString::from("-ac"));
            command.push(OsString::from(channels.to_string()));
        }
        command.extend(args(["-map", "0:a:0", "-vn"]));
        command.extend(args(["-c:a", request.sample_format.codec()]));
        command.push(temporary.clone().into_os_string());
        let process = self.runner.run(&self.ffmpeg.ffmpeg, &command, cancelled);
        self.finish_output(process, "prepare WAV", &temporary, &output, cancelled)?;
        Ok(PreparedAudio {
            output,
            sample_format: request.sample_format,
        })
    }

    pub fn loudness_analyze(
        &self,
        input: &Path,
        target: &LoudnessTarget,
        cancelled: &AtomicBool,
    ) -> Result<LoudnessMeasurement> {
        let input = validate_local_file(input)?;
        target.validate()?;
        self.measure(&input, target, cancelled)
    }

    pub fn loudness_normalize(
        &self,
        request: &NormalizeRequest,
        cancelled: &AtomicBool,
    ) -> Result<PreparedAudio> {
        let input = validate_local_file(&request.input)?;
        request.target.validate()?;
        let input_probe = self.probe(&input, cancelled)?;
        let sample_rate = input_probe
            .audio_streams
            .first()
            .and_then(|stream| stream.sample_rate)
            .ok_or_else(|| ComponentError::Process {
                message: "first audio stream has no usable sample rate".into(),
                stderr: String::new(),
            })?;
        let measured = self.measure(&input, &request.target, cancelled)?;
        if cancelled.load(Ordering::Acquire) {
            return Err(ComponentError::Cancelled);
        }
        let (output, temporary) =
            self.output_paths(request.output_name.as_deref(), "normalized")?;
        let filter = format!(
            "loudnorm=I={}:TP={}:LRA={}:measured_I={}:measured_TP={}:measured_LRA={}:measured_thresh={}:offset={}:linear=true:print_format=json",
            decimal(request.target.integrated_lufs), decimal(request.target.true_peak_db), decimal(request.target.loudness_range),
            decimal(measured.integrated_lufs), decimal(measured.true_peak_db), decimal(measured.loudness_range), decimal(measured.threshold), decimal(measured.target_offset),
        );
        let command = vec![
            OsString::from("-nostdin"),
            OsString::from("-n"),
            OsString::from("-i"),
            input.into_os_string(),
            OsString::from("-map"),
            OsString::from("0:a:0"),
            OsString::from("-af"),
            OsString::from(filter),
            OsString::from("-vn"),
            OsString::from("-ar"),
            OsString::from(sample_rate.to_string()),
            OsString::from("-c:a"),
            OsString::from("pcm_s24le"),
            temporary.clone().into_os_string(),
        ];
        let process = self.runner.run(&self.ffmpeg.ffmpeg, &command, cancelled);
        self.finish_output(
            process,
            "two-pass loudness normalization",
            &temporary,
            &output,
            cancelled,
        )?;
        Ok(PreparedAudio {
            output,
            sample_format: SampleFormat::S24,
        })
    }

    fn measure(
        &self,
        input: &Path,
        target: &LoudnessTarget,
        cancelled: &AtomicBool,
    ) -> Result<LoudnessMeasurement> {
        let filter = format!(
            "loudnorm=I={}:TP={}:LRA={}:print_format=json",
            decimal(target.integrated_lufs),
            decimal(target.true_peak_db),
            decimal(target.loudness_range)
        );
        let command = vec![
            OsString::from("-nostdin"),
            OsString::from("-v"),
            OsString::from("info"),
            OsString::from("-i"),
            input.as_os_str().to_owned(),
            OsString::from("-map"),
            OsString::from("0:a:0"),
            OsString::from("-vn"),
            OsString::from("-af"),
            OsString::from(filter),
            OsString::from("-f"),
            OsString::from("null"),
            OsString::from("-"),
        ];
        let output = self.runner.run(&self.ffmpeg.ffmpeg, &command, cancelled)?;
        require_success(&output, "loudness analysis")?;
        parse_measurement(&output.stderr)
    }

    fn output_paths(&self, requested: Option<&str>, prefix: &str) -> Result<(PathBuf, PathBuf)> {
        std::fs::create_dir_all(&self.paths.output_dir)?;
        ensure_inside(&self.paths.data_root, &self.paths.output_dir)?;
        let name = match requested {
            Some(name) => safe_output_name(name, "wav")?,
            None => format!("{prefix}-{}-{}.wav", std::process::id(), unique_counter()),
        };
        let output = self.paths.output_dir.join(name);
        if output.exists() {
            return Err(ComponentError::InvalidInput(format!(
                "refusing to overwrite existing output: {}",
                output.display()
            )));
        }
        let temporary = self.paths.output_dir.join(format!(
            ".pi-agent-{}-{}.part.wav",
            std::process::id(),
            unique_counter()
        ));
        Ok((output, temporary))
    }

    fn finish_output(
        &self,
        process: Result<ProcessOutput>,
        operation: &str,
        temporary: &Path,
        output: &Path,
        cancelled: &AtomicBool,
    ) -> Result<()> {
        let outcome = process.and_then(|result| require_success(&result, operation));
        if let Err(error) = outcome {
            let _ = std::fs::remove_file(temporary);
            return Err(error);
        }
        if cancelled.load(Ordering::Acquire) {
            let _ = std::fs::remove_file(temporary);
            return Err(ComponentError::Cancelled);
        }
        if output.exists() {
            let _ = std::fs::remove_file(temporary);
            return Err(ComponentError::InvalidInput(format!(
                "refusing to overwrite existing output: {}",
                output.display()
            )));
        }
        if let Err(error) = std::fs::rename(temporary, output) {
            let _ = std::fs::remove_file(temporary);
            return Err(ComponentError::Io(error));
        }
        Ok(())
    }
}

fn validate_segment(start: Option<f64>, duration: Option<f64>) -> Result<()> {
    if start.is_some_and(|v| !v.is_finite() || v < 0.0) {
        return Err(ComponentError::InvalidInput(
            "start seconds must be finite and non-negative".into(),
        ));
    }
    if duration.is_some_and(|v| !v.is_finite() || v <= 0.0) {
        return Err(ComponentError::InvalidInput(
            "duration seconds must be finite and positive".into(),
        ));
    }
    Ok(())
}
fn decimal(value: f64) -> String {
    format!("{value:.6}")
}
fn args<I, S>(values: I) -> Vec<OsString>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    values
        .into_iter()
        .map(|v| OsString::from(v.as_ref()))
        .collect()
}
fn string(value: &Value, field: &str) -> Option<String> {
    value.get(field)?.as_str().map(ToOwned::to_owned)
}
fn number<T: std::str::FromStr>(value: &Value, field: &str) -> Option<T>
where
    T::Err: std::fmt::Debug,
{
    let source = value.get(field)?;
    source
        .as_str()
        .and_then(|s| s.parse().ok())
        .or_else(|| source.as_f64().and_then(|n| n.to_string().parse().ok()))
}
fn require_success(output: &ProcessOutput, operation: &str) -> Result<()> {
    if output.success {
        Ok(())
    } else {
        Err(ComponentError::Process {
            message: format!("{operation} exited with {:?}", output.exit_code),
            stderr: output.stderr.clone(),
        })
    }
}
fn parse_measurement(stderr: &str) -> Result<LoudnessMeasurement> {
    let marker = stderr
        .rfind("\"input_i\"")
        .ok_or_else(|| ComponentError::Process {
            message: "loudnorm did not produce JSON statistics".into(),
            stderr: stderr.into(),
        })?;
    let start = stderr[..marker]
        .rfind('{')
        .ok_or_else(|| ComponentError::Process {
            message: "loudnorm statistics JSON was malformed".into(),
            stderr: stderr.into(),
        })?;
    let end = stderr[marker..]
        .find('}')
        .map(|offset| marker + offset + 1)
        .ok_or_else(|| ComponentError::Process {
            message: "loudnorm statistics JSON was malformed".into(),
            stderr: stderr.into(),
        })?;
    let raw: Value =
        serde_json::from_str(&stderr[start..end]).map_err(|_| ComponentError::Process {
            message: "loudnorm statistics JSON was malformed".into(),
            stderr: stderr.into(),
        })?;
    let value = |key: &str| {
        number::<f64>(&raw, key).ok_or_else(|| ComponentError::Process {
            message: format!("loudnorm statistics missing {key}"),
            stderr: stderr.into(),
        })
    };
    Ok(LoudnessMeasurement {
        integrated_lufs: value("input_i")?,
        true_peak_db: value("input_tp")?,
        loudness_range: value("input_lra")?,
        threshold: value("input_thresh")?,
        target_offset: value("target_offset")?,
        raw,
    })
}
fn unique_counter() -> u64 {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{installer::FfmpegSource, runner::ProcessOutput};
    use std::{fs, sync::Mutex};

    #[derive(Default)]
    struct RecordingRunner {
        calls: Mutex<Vec<Vec<OsString>>>,
        fail_processing: bool,
    }
    impl ProcessRunner for RecordingRunner {
        fn run(
            &self,
            executable: &Path,
            args: &[OsString],
            _cancelled: &AtomicBool,
        ) -> Result<ProcessOutput> {
            self.calls.lock().unwrap().push(args.to_vec());
            if executable.file_name().and_then(|name| name.to_str()) == Some("ffprobe.exe") {
                return Ok(ProcessOutput { success: true, stdout: r#"{"format":{"duration":"1.0"},"streams":[{"codec_type":"audio","sample_rate":"48000","channels":2}]}"#.into(), ..Default::default() });
            }
            if args.iter().any(|value| value.to_string_lossy() == "null") {
                return Ok(ProcessOutput { success: true, stderr: r#"{"input_i":"-19.2","input_tp":"-1.0","input_lra":"4.0","input_thresh":"-29.0","target_offset":"0.3"}"#.into(), ..Default::default() });
            }
            if let Some(output) = args.last() {
                fs::write(PathBuf::from(output), b"output").unwrap();
            }
            Ok(ProcessOutput {
                success: !self.fail_processing,
                exit_code: self.fail_processing.then_some(1),
                ..Default::default()
            })
        }
    }

    fn test_executor(root: &Path) -> AudioExecutor<RecordingRunner> {
        AudioExecutor::new(
            ComponentPaths::from_data_root(root.to_path_buf()),
            ResolvedFfmpeg {
                source: FfmpegSource::Explicit,
                root: root.to_path_buf(),
                ffmpeg: root.join("ffmpeg.exe"),
                ffprobe: root.join("ffprobe.exe"),
                version: None,
            },
            RecordingRunner::default(),
        )
    }
    fn temporary_root(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "pi-components-audio-{label}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }
    #[test]
    fn loudness_json_is_parsed_without_executing_ffmpeg() {
        let stderr = "noise\n{\n \"input_i\": \"-19.2\", \"input_tp\": \"-1.0\", \"input_lra\": \"4.0\", \"input_thresh\": \"-29.0\", \"target_offset\": \"0.3\"\n}\n[out#0/null @ 000001] video:0KiB audio:94KiB\nsize=N/A time=00:00:01.00\n";
        assert_eq!(parse_measurement(stderr).unwrap().integrated_lufs, -19.2);
    }
    #[test]
    fn segment_validation_rejects_bad_numbers() {
        assert!(validate_segment(Some(-1.0), None).is_err());
        assert!(validate_segment(None, Some(0.0)).is_err());
        assert!(validate_segment(Some(1.0), Some(2.0)).is_ok());
    }
    #[test]
    fn target_requires_explicit_sane_values() {
        assert!(LoudnessTarget {
            integrated_lufs: f64::NAN,
            true_peak_db: -1.0,
            loudness_range: 7.0
        }
        .validate()
        .is_err());
    }
    #[test]
    fn target_enforces_documented_loudnorm_ranges() {
        assert!(LoudnessTarget {
            integrated_lufs: -71.0,
            true_peak_db: -1.0,
            loudness_range: 7.0
        }
        .validate()
        .is_err());
        assert!(LoudnessTarget {
            integrated_lufs: -16.0,
            true_peak_db: 0.1,
            loudness_range: 7.0
        }
        .validate()
        .is_err());
        assert!(LoudnessTarget {
            integrated_lufs: -16.0,
            true_peak_db: -1.0,
            loudness_range: 0.9
        }
        .validate()
        .is_err());
        assert!(LoudnessTarget {
            integrated_lufs: -16.0,
            true_peak_db: -1.0,
            loudness_range: 7.0
        }
        .validate()
        .is_ok());
    }
    #[test]
    fn urls_are_rejected_before_any_process_invocation() {
        assert!(matches!(
            validate_local_file(Path::new("https://example.invalid/audio.wav")),
            Err(ComponentError::InvalidInput(_))
        ));
    }
    #[test]
    fn prepare_explicitly_maps_first_audio_and_drops_video() {
        let root = temporary_root("prepare");
        let input = root.join("input.wav");
        fs::write(&input, b"audio").unwrap();
        let executor = test_executor(&root);
        executor
            .prepare_wav(
                &PrepareWavRequest {
                    input,
                    output_name: Some("prepared.wav".into()),
                    start_seconds: None,
                    duration_seconds: None,
                    sample_rate: None,
                    channels: None,
                    sample_format: SampleFormat::S16,
                },
                &AtomicBool::new(false),
            )
            .unwrap();
        let joined = executor.runner.calls.lock().unwrap()[0]
            .iter()
            .map(|v| v.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(joined.contains("-map 0:a:0 -vn"));
        let _ = fs::remove_dir_all(root);
    }
    #[test]
    fn normalization_preserves_the_first_audio_stream_rate() {
        let root = temporary_root("normalize");
        let input = root.join("input.wav");
        fs::write(&input, b"audio").unwrap();
        let executor = test_executor(&root);
        executor
            .loudness_normalize(
                &NormalizeRequest {
                    input,
                    output_name: Some("normalized.wav".into()),
                    target: LoudnessTarget {
                        integrated_lufs: -16.0,
                        true_peak_db: -1.0,
                        loudness_range: 7.0,
                    },
                },
                &AtomicBool::new(false),
            )
            .unwrap();
        let calls = executor.runner.calls.lock().unwrap();
        let final_call = calls.last().unwrap();
        let joined = final_call
            .iter()
            .map(|v| v.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(joined.contains("-ar 48000"));
        let measure_call = &calls[1];
        let measured = measure_call
            .iter()
            .map(|v| v.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(measured.contains("-map 0:a:0 -vn"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn failed_processing_removes_partial_output() {
        let root = temporary_root("partial-cleanup");
        let input = root.join("input.wav");
        fs::write(&input, b"audio").unwrap();
        let executor = AudioExecutor::new(
            ComponentPaths::from_data_root(root.clone()),
            ResolvedFfmpeg {
                source: FfmpegSource::Explicit,
                root: root.clone(),
                ffmpeg: root.join("ffmpeg.exe"),
                ffprobe: root.join("ffprobe.exe"),
                version: None,
            },
            RecordingRunner {
                fail_processing: true,
                ..Default::default()
            },
        );
        assert!(executor
            .prepare_wav(
                &PrepareWavRequest {
                    input,
                    output_name: Some("failed.wav".into()),
                    start_seconds: None,
                    duration_seconds: None,
                    sample_rate: None,
                    channels: None,
                    sample_format: SampleFormat::S16,
                },
                &AtomicBool::new(false),
            )
            .is_err());
        let output_dir = root.join("output").join("ffmpeg");
        assert!(!output_dir.join("failed.wav").exists());
        assert_eq!(fs::read_dir(output_dir).unwrap().count(), 0);
        let _ = fs::remove_dir_all(root);
    }
}
