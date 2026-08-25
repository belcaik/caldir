use crate::calendar::CalendarError;
use crate::event::EventUid;
use crate::remote::RemoteError;

#[derive(Debug, thiserror::Error)]
pub enum ConnectionError {
    #[error("Remote error: {0}")]
    Remote(#[from] RemoteError),

    #[error("Local calendar error: {0}")]
    Calendar(#[from] CalendarError),

    /// A same-UID event/task pair, both outgoing to one collection (R65).
    ///
    /// The local mixed directory itself stays fully supported (R4); what
    /// cannot work is putting both into one CalDAV collection, where UID
    /// uniqueness is enforced component-agnostically and the same-href case is
    /// a *silent* overwrite. caldir refuses rather than inventing an href
    /// scheme (R62) — the error is the feature.
    #[error(
        "Cannot push {}: the calendar holds both an event and a task with that UID, and one remote collection cannot hold both. Change one of the UIDs, or keep the pair in a local-only calendar.",
        uid.as_str()
    )]
    SameUidPair { uid: EventUid },
}
