use crate::calendar::CalendarError;
use crate::event::EventUid;
use crate::remote::RemoteError;
use std::path::PathBuf;

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

    /// An outgoing change the event-only sync wire cannot carry (R70/R71).
    ///
    /// Raised by the pre-flight, before the first RPC, for the same reason as
    /// `SameUidPair`: a half-pushed calendar is the destructive outcome. The
    /// loop used to abort *mid-push* on the same item, leaving whatever
    /// happened to sort after it silently abandoned — and, since a task fails
    /// identically forever, abandoned on every subsequent run too.
    ///
    /// Deliberately worded apart from the wire-level arity abort
    /// (`expected 1 event(s) in ICS, found 0`), which stays byte-locked at the
    /// `Remote` seam: a user who upgraded to fix that message must not be
    /// shown its twin.
    #[error(
        "Cannot push {}: caldir cannot push tasks to a remote calendar yet — the sync wire carries events only. Nothing was pushed, so this calendar is unchanged locally and remotely, and its other {pending} pending change(s) are still waiting. Move the task to a local-only calendar (one with no remote) to unblock them.",
        locate(file.as_ref(), uid)
    )]
    UnpushableTask {
        uid: EventUid,
        file: Option<PathBuf>,
        pending: usize,
    },
}

/// How the refused item is pointed at: its file when one is on disk, its UID
/// when the change is a delete and the file is already gone.
fn locate(file: Option<&PathBuf>, uid: &EventUid) -> String {
    match file.and_then(|p| p.file_name()) {
        Some(name) => name.to_string_lossy().into_owned(),
        None => format!("UID {}", uid.as_str()),
    }
}
