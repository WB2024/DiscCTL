use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Serialize, Deserialize)]
pub struct DiscError {
    pub error: String,
    pub message: String,
    pub recoverable: bool,
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("{0}")]
    Validation(String),
    #[error("{0}")]
    Backend(String),
    #[error("{0}")]
    Device(String),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Glob(#[from] glob::PatternError),
}

impl Error {
    pub fn to_disc_error(&self) -> DiscError {
        let (code, recoverable) = match self {
            Error::Validation(_) => ("VALIDATION_ERROR", false),
            Error::Backend(_) => ("BACKEND_ERROR", true),
            Error::Device(_) => ("DEVICE_ERROR", false),
            Error::Io(_) => ("IO_ERROR", false),
            Error::Json(_) => ("JSON_PARSE_ERROR", false),
            Error::Glob(_) => ("GLOB_ERROR", false),
        };
        DiscError {
            error: code.to_string(),
            message: self.to_string(),
            recoverable,
        }
    }

    pub fn validation(msg: impl Into<String>) -> Self {
        Error::Validation(msg.into())
    }

    pub fn backend(msg: impl Into<String>) -> Self {
        Error::Backend(msg.into())
    }

    pub fn device(msg: impl Into<String>) -> Self {
        Error::Device(msg.into())
    }
}
