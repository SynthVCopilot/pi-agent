use std::{fmt, io};

#[derive(Debug)]
pub enum ComponentError {
    InvalidInput(String),
    NotFound(String),
    Io(io::Error),
    Download(String),
    Integrity { expected: String, actual: String },
    Archive(String),
    Process { message: String, stderr: String },
    Cancelled,
    State(String),
    Json(serde_json::Error),
}

impl fmt::Display for ComponentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInput(v) => write!(f, "invalid input: {v}"),
            Self::NotFound(v) => write!(f, "not found: {v}"),
            Self::Io(v) => write!(f, "I/O error: {v}"),
            Self::Download(v) => write!(f, "download error: {v}"),
            Self::Integrity { expected, actual } => {
                write!(f, "SHA-256 mismatch (expected {expected}, got {actual})")
            }
            Self::Archive(v) => write!(f, "unsafe or invalid archive: {v}"),
            Self::Process { message, stderr } => {
                write!(f, "FFmpeg process failed: {message}; {stderr}")
            }
            Self::Cancelled => write!(f, "operation cancelled"),
            Self::State(v) => write!(f, "invalid component state: {v}"),
            Self::Json(v) => write!(f, "JSON error: {v}"),
        }
    }
}

impl std::error::Error for ComponentError {}
impl From<io::Error> for ComponentError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}
impl From<serde_json::Error> for ComponentError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

impl ComponentError {
    /// Stable machine-readable codes for FFI and desktop callers.
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidInput(_) => "invalid_input",
            Self::NotFound(_) => "not_found",
            Self::Io(_) => "io_error",
            Self::Download(_) => "download_failed",
            Self::Integrity { .. } => "integrity_mismatch",
            Self::Archive(_) => "unsafe_archive",
            Self::Process { .. } => "process_failed",
            Self::Cancelled => "cancelled",
            Self::State(_) => "invalid_state",
            Self::Json(_) => "json_error",
        }
    }

    pub fn details(&self) -> Option<serde_json::Value> {
        match self {
            Self::Integrity { expected, actual } => {
                Some(serde_json::json!({ "expected": expected, "actual": actual }))
            }
            // SystemProcessRunner keeps this bounded to the final 8 KiB.
            Self::Process { stderr, .. } if !stderr.is_empty() => {
                Some(serde_json::json!({ "stderr_tail": stderr }))
            }
            _ => None,
        }
    }
}

pub type Result<T> = std::result::Result<T, ComponentError>;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn integrity_error_has_stable_code_and_structured_details() {
        let error = ComponentError::Integrity {
            expected: "a".into(),
            actual: "b".into(),
        };
        assert_eq!(error.code(), "integrity_mismatch");
        assert_eq!(
            error.details(),
            Some(serde_json::json!({ "expected": "a", "actual": "b" }))
        );
    }
}
