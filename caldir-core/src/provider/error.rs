use std::path::PathBuf;

use super::transport::ProviderTransportError;

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("Provider file is not executable: {0}")]
    NotExecutable(PathBuf),

    #[error("Provider filename does not match `caldir-provider-<name>`: {0}")]
    InvalidProviderFilename(PathBuf),

    #[error("Provider {0} not found")]
    ProviderNotFound(String),

    #[error("Provider transport error: {0}")]
    Transport(#[from] ProviderTransportError),

    #[error("Failed to serialize provider request: {0}")]
    Serialize(serde_json::Error),

    #[error("Failed to deserialize provider response: {0}")]
    Deserialize(serde_json::Error),

    #[error("Provider returned error: {0}")]
    Provider(String),

    /// The provider replied that it does not recognise the RPC method sent
    /// to it (`rpc::ErrorCode::UnsupportedMethod`) — distinct from
    /// `Provider` so a caller can tell "too old to answer" from "answered
    /// and failed" without matching on message text (§16, `CROSS-4`).
    #[error("Provider does not support this method: {0}")]
    UnsupportedMethod(String),
}
