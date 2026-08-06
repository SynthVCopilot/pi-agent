use serde::{Deserialize, Serialize};

/// 可安装/可调用的组件类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ComponentKind {
    /// ffmpeg：音视频转码/抽取，所有音频前处理的基础。
    Ffmpeg,
    /// 本地 whisper：离线语音识别，把人声转成带时间戳的词。
    WhisperLocal,
    /// 游戏音高识别模型：从演唱/游戏音频提取音高轮廓。
    GamePitchModel,
    /// Transformer 人声分离：从混音里分出人声/伴奏 stem（Demucs 类）。
    VocalSeparation,
    /// 乐器识别：识别混音/stem 里出现了哪些乐器。
    InstrumentRecognition,
    /// 曲风/歌曲风格识别。
    GenreStyleRecognition,
    /// 拍数/速度检测（BPM、beat、downbeat）。
    TempoBeatDetection,
    /// Sound→(含词)MIDI：音频(+词时间轴)转成带音节歌词的 MIDI；也支持直接导入。
    SoundToMidi,
    /// pi-audio 音频探针：特征指纹 + PANNs 判别(乐器/genre倾向/有词无词) + 配对差分。
    AudioProbe,
    /// CVRS 跨版本渲染搬运：.svp 文件级、只写不读，静音参考音频轨。
    Cvrs,
}

/// 谁能用这个组件：AI agent、人工（桌面 UI 直接点），或两者。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Audience {
    Ai,
    Human,
    Both,
}

/// 一个组件的静态描述。URL/哈希留空由清单/设置填充。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentSpec {
    pub id: String,
    pub kind: ComponentKind,
    pub display_name: String,
    pub description: String,
    pub version: String,
    /// 面向对象：game/音高等分析模型对 AI 与人工都开放（Both）。
    pub audience: Audience,
    #[serde(default)]
    pub download_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable_relative_path: Option<String>,
}

/// 组件安装状态。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ComponentState {
    Checking,
    Downloading,
    Verifying,
    Installing,
    Updating,
    Uninstalling,
    Ready,
    Cancelled,
    Failed,
    #[default]
    NotInstalled,
}

/// Where an executable was resolved from.  Managed means the private Pi
/// installation; system and explicit remain read-only from Pi's perspective.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ComponentSource {
    Managed,
    System,
    Explicit,
    #[default]
    Unavailable,
}

/// User-initiated lifecycle operations for an installable component.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ComponentAction {
    Install,
    Update,
    Uninstall,
}

/// Runtime state returned to the Desktop UI and any host integration.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComponentStatus {
    pub id: String,
    #[serde(default)]
    pub state: ComponentState,
    #[serde(default)]
    pub source: ComponentSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installed_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub available_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable_dir: Option<String>,
    #[serde(default)]
    pub can_install: bool,
    #[serde(default)]
    pub can_update: bool,
    #[serde(default)]
    pub can_uninstall: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Static catalog information paired with its current runtime state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentView {
    pub spec: ComponentSpec,
    pub status: ComponentStatus,
}

/// A background lifecycle or FFmpeg operation state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum JobState {
    #[default]
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

/// A stable structured job failure.  Callers should branch on `code`, not a
/// rendered process message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobError {
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
}

/// Pollable task status. Progress is normalized to the inclusive 0.0–1.0
/// range by the executor.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct JobStatus {
    pub id: String,
    #[serde(default)]
    pub state: JobState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<JobError>,
}

/// Resolution policy for FFmpeg. `Auto` uses an explicit directory first,
/// then a healthy managed installation, and finally the system PATH.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FfmpegSourcePreference {
    #[default]
    Auto,
    Managed,
    System,
}

/// User-level FFmpeg discovery preferences.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FfmpegConfig {
    #[serde(default)]
    pub preference: FfmpegSourcePreference,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_bin_dir: Option<String>,
}

/// The finite audio operations exposed by Pi. This deliberately cannot encode
/// an arbitrary FFmpeg command line or filter graph.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum FfmpegRequest {
    Probe {
        input: String,
    },
    Prepare {
        input: String,
        output_name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sample_rate: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        channels: Option<u8>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sample_format: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        start_seconds: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration_seconds: Option<f64>,
    },
    LoudnessAnalyze {
        input: String,
    },
    LoudnessNormalize {
        input: String,
        output_name: String,
        target_lufs: f64,
        max_true_peak_db: f64,
        target_lra: f64,
    },
}

/// Normalized facts from `ffprobe` for the first selected audio stream.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FfmpegProbeResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codec: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_seconds: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channels: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bit_depth: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bit_rate: Option<u64>,
}

/// Measurements emitted by an EBU R128 loudness analysis.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LoudnessAnalysisResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub integrated_lufs: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub true_peak_db: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loudness_range: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub threshold: Option<f64>,
}

/// Result of an FFmpeg operation. Output paths are strings so the C ABI and
/// JSON clients receive the same platform-native representation.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FfmpegOperationResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub probe: Option<FfmpegProbeResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loudness: Option<LoudnessAnalysisResult>,
}

/// Sound→MIDI 请求。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SoundToMidiRequest {
    pub audio_path: String,
    pub output_midi_path: String,
    #[serde(default = "default_true")]
    pub include_lyrics: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lyrics_hint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
}

fn default_true() -> bool {
    true
}

/// 一次音频分析的聚合结果（乐器 / 曲风 / 速度拍点）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AudioAnalysis {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bpm: Option<f32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub beats_seconds: Vec<f32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub instruments: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub genre: Option<String>,
}

/// 内置组件目录。桌面「组件」页面与 AI 工具列表都从这里读。
pub fn default_catalog() -> Vec<ComponentSpec> {
    fn spec(
        id: &str,
        kind: ComponentKind,
        name: &str,
        desc: &str,
        audience: Audience,
    ) -> ComponentSpec {
        ComponentSpec {
            id: id.to_string(),
            kind,
            display_name: name.to_string(),
            description: desc.to_string(),
            version: "latest".to_string(),
            audience,
            download_url: String::new(),
            sha256: None,
            executable_relative_path: None,
        }
    }
    vec![
        spec("ffmpeg", ComponentKind::Ffmpeg, "FFmpeg",
             "音视频转码与抽取；分离/识别/Sound→MIDI 的前处理基础。", Audience::Both),
        spec("whisper-local", ComponentKind::WhisperLocal, "Whisper（本地）",
             "离线语音识别，把人声转成带时间戳的词，喂给 Sound→MIDI 的词轨。", Audience::Both),
        spec("game-pitch", ComponentKind::GamePitchModel, "游戏音高识别模型",
             "从演唱/游戏音频提取音高轮廓；AI 与人工均可调用。", Audience::Both),
        spec("vocal-separation", ComponentKind::VocalSeparation, "人声分离（Transformer）",
             "从混音分出人声/伴奏 stem（Demucs 类 transformer 模型）。", Audience::Both),
        spec("instrument-id", ComponentKind::InstrumentRecognition, "乐器识别",
             "识别混音/stem 里的乐器构成。", Audience::Both),
        spec("genre-id", ComponentKind::GenreStyleRecognition, "曲风识别",
             "识别歌曲风格/流派，辅助选唱法与编曲判断。", Audience::Both),
        spec("tempo-beat", ComponentKind::TempoBeatDetection, "速度与拍点检测",
             "检测 BPM、beat、downbeat（拍数），供对齐与量化。", Audience::Both),
        spec("sound-to-midi", ComponentKind::SoundToMidi, "Sound→MIDI（含词）",
             "音频(+词时间轴)转带音节歌词 MIDI；也支持直接导入 MIDI/MusicXML。", Audience::Both),
        spec("pi-audio", ComponentKind::AudioProbe, "pi-audio 音频探针",
             "本仓库 components/pi-audio：probe(特征指纹+PANNs 乐器/genre倾向/有词无词判别) 与 \
              pair-diff(有词/无词配对差分→单音人声轨，可直喂 SV import)。风格命名留给上层 LLM。",
             Audience::Both),
        spec("cvrs", ComponentKind::Cvrs, "CVRS 跨版本渲染搬运",
             "本仓库 components/cvrs：.svp 文件级 SV1↔SV2 辅助工具。probe 读取版本/时代/轨信息；\
              add-ref 读取并克隆目标 schema 后写出新文件，不覆盖源工程。渲染步不含，wav 由调用方给。",
             Audience::Both),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn component_state_keeps_existing_wire_names_and_serializes_new_states() {
        assert_eq!(
            serde_json::to_string(&ComponentState::NotInstalled).unwrap(),
            "\"not-installed\""
        );
        assert_eq!(
            serde_json::to_string(&ComponentState::Downloading).unwrap(),
            "\"downloading\""
        );
        assert_eq!(
            serde_json::to_string(&ComponentState::Ready).unwrap(),
            "\"ready\""
        );
        assert_eq!(
            serde_json::to_string(&ComponentState::Checking).unwrap(),
            "\"checking\""
        );
        assert_eq!(ComponentState::default(), ComponentState::NotInstalled);
    }

    #[test]
    fn component_status_defaults_to_unavailable_not_installed() {
        let status: ComponentStatus = serde_json::from_str(r#"{"id":"ffmpeg"}"#).unwrap();
        assert_eq!(status.id, "ffmpeg");
        assert_eq!(status.state, ComponentState::NotInstalled);
        assert_eq!(status.source, ComponentSource::Unavailable);
        assert!(!status.can_install);
    }

    #[test]
    fn ffmpeg_config_defaults_to_auto() {
        let config: FfmpegConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(config.preference, FfmpegSourcePreference::Auto);
        assert_eq!(config.system_bin_dir, None);
    }

    #[test]
    fn ffmpeg_request_round_trips_all_whitelisted_operations() {
        let requests = vec![
            FfmpegRequest::Probe {
                input: r"C:\\audio\\in.wav".into(),
            },
            FfmpegRequest::Prepare {
                input: r"C:\\audio\\in.wav".into(),
                output_name: "prepared.wav".into(),
                sample_rate: Some(44_100),
                channels: Some(1),
                sample_format: Some("s24".into()),
                start_seconds: Some(1.5),
                duration_seconds: Some(12.0),
            },
            FfmpegRequest::LoudnessAnalyze {
                input: r"C:\\audio\\in.wav".into(),
            },
            FfmpegRequest::LoudnessNormalize {
                input: r"C:\\audio\\in.wav".into(),
                output_name: "normalized.wav".into(),
                target_lufs: -14.0,
                max_true_peak_db: -1.0,
                target_lra: 11.0,
            },
        ];

        for request in requests {
            let json = serde_json::to_string(&request).unwrap();
            let restored: FfmpegRequest = serde_json::from_str(&json).unwrap();
            assert_eq!(restored, request);
        }
    }

    #[test]
    fn normalize_requires_all_explicit_targets() {
        let error = serde_json::from_str::<FfmpegRequest>(
            r#"{"operation":"loudness_normalize","input":"C:\\audio\\in.wav","output_name":"out.wav"}"#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("target_lufs"));
    }

    #[test]
    fn ffmpeg_requests_reject_arbitrary_extra_arguments() {
        let error = serde_json::from_str::<FfmpegRequest>(
            r#"{"operation":"probe","input":"C:\\audio\\in.wav","arguments":"-y"}"#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("unknown field"));
    }

    #[test]
    fn job_status_round_trips_structured_error() {
        let status = JobStatus {
            id: "job-1".into(),
            state: JobState::Failed,
            phase: Some("verifying".into()),
            progress: Some(0.75),
            result: None,
            error: Some(JobError {
                code: "SHA256_MISMATCH".into(),
                message: "Downloaded archive did not match the manifest.".into(),
                details: Some("expected=abc actual=def".into()),
            }),
        };
        let json = serde_json::to_string(&status).unwrap();
        let restored: JobStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.error.unwrap().code, "SHA256_MISMATCH");
        assert_eq!(restored.state, JobState::Failed);
    }
}
