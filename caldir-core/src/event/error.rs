use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum EventError {
    #[error("failed to read event from {0}: {1}")]
    Io(PathBuf, #[source] std::io::Error),

    #[error("failed to parse ICS {0}: {1}")]
    InvalidIcs(String, String),

    #[error("expected {expected} event(s) in ICS, found {found}")]
    UnexpectedEventCount { expected: usize, found: usize },

    #[error("expected {expected} supported component(s) in ICS, found {found}")]
    UnexpectedComponentCount { expected: usize, found: usize },

    #[error("unsupported component {0} in ICS; caldir supports VEVENT and VTODO only")]
    UnsupportedComponent(String),

    #[error("event is missing a start time (DTSTART)")]
    MissingStart,

    #[error("event is missing a UID")]
    MissingUid,

    /// A property is present but its value cannot be read. Never a silent drop
    /// (R19): on a task an unparseable `DUE` would otherwise become "no DUE",
    /// which R46 makes unconditionally in-window — a typo silently changing an
    /// item's sync visibility. The event codec's own equivalent stays
    /// `MissingStart`, which is unreachable for a task (R20), unchanged (R1/R2).
    #[error("property {name} has an unparseable value: {value}")]
    UnparseableProperty { name: String, value: String },

    #[error("no attendee matching {email}")]
    AttendeeNotFound { email: String },
}
