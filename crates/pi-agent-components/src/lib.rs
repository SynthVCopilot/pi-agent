//! Managed Windows FFmpeg support for Pi Agent.
//!
//! This crate deliberately has no dependency on the agent loop or C ABI.  The
//! later integration layer can expose these safe, typed operations without
//! giving an LLM a shell-command escape hatch.

mod audio;
mod error;
mod installer;
mod jobs;
mod manifest;
mod paths;
mod runner;

pub use audio::{
    AudioExecutor, AudioProbe, LoudnessMeasurement, LoudnessTarget, NormalizeRequest,
    PrepareWavRequest, PreparedAudio, SampleFormat,
};
pub use error::{ComponentError, Result};
pub use installer::{
    Architecture, ComponentPaths, FfmpegInstaller, FfmpegResolver, FfmpegSource, HttpDownloader,
    ResolvedFfmpeg,
};
pub use jobs::{JobError, JobHandle, JobProgress, JobState, JobStatus};
pub use manifest::{FfmpegManifest, FfmpegRelease};
pub use runner::{ProcessOutput, ProcessRunner, SystemProcessRunner};
