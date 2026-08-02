use std::fmt;

/// Pi Agent 统一错误类型（保持轻量，不引入 anyhow/thiserror）。
#[derive(Debug)]
pub struct PiError(pub String);

impl PiError {
    pub fn new(msg: impl Into<String>) -> Self {
        PiError(msg.into())
    }
}

impl fmt::Display for PiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for PiError {}

impl From<serde_json::Error> for PiError {
    fn from(e: serde_json::Error) -> Self {
        PiError(format!("json: {e}"))
    }
}

impl From<std::io::Error> for PiError {
    fn from(e: std::io::Error) -> Self {
        PiError(format!("io: {e}"))
    }
}

pub type Result<T> = std::result::Result<T, PiError>;
