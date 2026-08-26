use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub enum ProviderTransportError {
    #[error("Failed to spawn provider: {0}")]
    Spawn(std::io::Error),

    #[error("I/O error during provider exchange: {0}")]
    Io(std::io::Error),

    #[error("Provider response was not valid UTF-8")]
    BadUtf8,

    #[error("Provider returned no response")]
    EmptyResponse,

    #[error("Provider exited with status {code:?}")]
    NonZeroExit { code: Option<i32> },

    #[error("Provider timed out after {0:?}")]
    Timeout(Duration),

    /// §16 class 6: the provider ran the RPC but its own call to the remote
    /// API failed for network reasons. Distinct from the other variants
    /// here, which are all failures in the local subprocess exchange itself
    /// — this is the provider *reporting* a network failure, not this
    /// process hitting one. Kept in this enum rather than a second one in
    /// `ProviderError` so there is exactly one place the network class lives.
    #[error("Provider reported a network/API failure: {0}")]
    Network(String),
}
