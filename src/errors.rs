//! Small error type shared across the app.

use std::fmt;

#[derive(Debug)]
pub enum AppError {
    Io(String),
    Power(String),
    Config(String),
    NotFound(String),
    Denied(String),
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AppError::Io(m) => write!(f, "file error: {m}"),
            AppError::Power(m) => write!(f, "power setting error: {m}"),
            AppError::Config(m) => write!(f, "configuration error: {m}"),
            AppError::NotFound(m) => write!(f, "not found: {m}"),
            AppError::Denied(m) => write!(f, "not permitted: {m}"),
        }
    }
}

impl std::error::Error for AppError {}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        AppError::Io(e.to_string())
    }
}

pub type AppResult<T> = Result<T, AppError>;