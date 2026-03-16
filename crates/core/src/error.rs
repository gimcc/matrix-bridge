use thiserror::Error;

#[derive(Debug, Error)]
pub enum BridgeError {
    #[error("platform error: {platform}: {message}")]
    Platform { platform: String, message: String },

    #[error("matrix error: {0}")]
    Matrix(String),

    #[error("config error: {0}")]
    Config(String),

    #[error("store error: {0}")]
    Store(String),

    #[error("http error: {0}")]
    Http(String),

    #[error("auth error: {0}")]
    Auth(String),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("serialization error: {0}")]
    Serialization(String),

    #[error("validation error: {0}")]
    Validation(String),

    #[error("internal error: {0}")]
    Internal(String),
}

impl From<serde_json::Error> for BridgeError {
    fn from(e: serde_json::Error) -> Self {
        Self::Serialization(e.to_string())
    }
}
