use super::ProviderError;

#[derive(Debug, thiserror::Error)]
pub enum RemoteError {
    #[error("Provider error: {0}")]
    Provider(#[from] ProviderError),
    /// A change whose item cannot ride the event-only wire (R70, R71). The
    /// text is the arity error the wire already reports; `L6-01` owns turning
    /// it into a user-facing story.
    #[error("{0}")]
    UnsupportedItem(#[from] crate::event::EventError),
}
