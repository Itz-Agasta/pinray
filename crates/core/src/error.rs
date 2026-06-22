use std::time::Duration;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum PinrayError {
    #[error("invalid session configuration: {0}")]
    InvalidConfig(String),
    #[error("backend unavailable: {0}")]
    BackendUnavailable(String),
    #[error("capture backend not selected")]
    BackendNotSelected,
    #[error("capture timed out after {0:?}")]
    Timeout(Duration),
    #[error("operation not supported: {0}")]
    Unsupported(String),
    #[error("platform error: {0}")]
    Platform(String),
}

pub type Result<T> = std::result::Result<T, PinrayError>;
