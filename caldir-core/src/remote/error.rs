use super::ProviderError;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum RemoteError {
    #[error("Provider error: {0}")]
    Provider(#[from] ProviderError),
    /// A change whose item cannot ride the event-only wire. The text is the
    /// arity error the wire already reports, not yet a user-facing story.
    #[error("{0}")]
    UnsupportedItem(#[from] crate::event::EventError),
}
