use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("missing environment variable {0}")]
    MissingEnv(&'static str),

    #[error("invalid arguments: {0}")]
    InvalidArgs(String),

    #[error("date overflow while iterating run range")]
    DateOverflow,

    #[error("failed to read file {path}: {source}")]
    ReadFile {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("failed to write file {path}: {source}")]
    WriteFile {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("failed to parse Gmail export {path}: {source}")]
    ParseExport {
        path: PathBuf,
        source: serde_json::Error,
    },

    #[error("failed to serialize Gmail export: {0}")]
    SerializeExport(serde_json::Error),
}
