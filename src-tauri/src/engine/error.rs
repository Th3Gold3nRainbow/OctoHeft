use thiserror::Error;

#[derive(Debug, Error)]
pub enum EngineError {
    #[error("Network communication error: {0}")]
    Network(String),

    #[error("Server returned HTTP {status}: {message}")]
    Http { status: u16, message: String },

    #[error("Corrupted or invalid byte stream: {0}")]
    CorruptedStream(String),

    #[error("File system I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Configuration or state parsing error: {0}")]
    StateParse(String),

    #[error("Task or resource not found: {0}")]
    NotFound(String),

    #[error("Operation cancelled by user or scheduler")]
    Cancelled,
}

impl From<EngineError> for String {
    fn from(err: EngineError) -> Self {
        err.to_string()
    }
}

