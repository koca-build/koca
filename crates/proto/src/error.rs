use thiserror::Error;

use crate::types::ProtocolError;

#[derive(Debug, Error)]
pub enum ProtoError {
    #[error("I/O error: {0}")]
    Io(std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("socket error: {0}")]
    Socket(std::io::Error),

    #[error("connection closed unexpectedly")]
    ConnectionClosed,

    #[error("backend error: {0}")]
    Backend(ProtocolError),
}
