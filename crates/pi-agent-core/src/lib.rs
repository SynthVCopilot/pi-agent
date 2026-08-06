//! Pi Agent v3 核心：与后端无关的 agent 循环、会话历史、组件模型。
//!
//! 版本谱系：v1 = codex skill（synthv-agent-bridge + SKILLS/synthv-agent）；
//! v2 = 魔改 codex agent（不归本项目管）；v3 = pi（本 crate 所在）。

pub mod agent;
pub mod components;
pub mod error;
pub mod history;
pub mod paths;

pub use agent::{
    AgentLoop, AgentProvider, AgentStep, ChatMessage, EchoProvider, NoTools, Role, ToolCall,
    ToolDefinition, ToolExecutor, ToolResult,
};
pub use components::{
    default_catalog, Audience, AudioAnalysis, ComponentAction, ComponentKind, ComponentSource,
    ComponentSpec, ComponentState, ComponentStatus, ComponentView, FfmpegConfig,
    FfmpegOperationResult, FfmpegProbeResult, FfmpegRequest, FfmpegSourcePreference, JobError,
    JobState, JobStatus, LoudnessAnalysisResult, SoundToMidiRequest,
};
pub use error::{PiError, Result};
pub use history::{Conversation, ConversationStore, JsonConversationStore};
pub use paths::{
    components_dir, config_path, data_root, downloads_dir, ffmpeg_output_dir, history_dir,
    models_dir, output_dir, safe_join,
};
