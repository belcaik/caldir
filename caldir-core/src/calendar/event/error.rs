use std::path::PathBuf;

use crate::event::EventError;

#[derive(Debug, thiserror::Error)]
pub enum CalendarEventError {
    #[error("invalid event in ICS file {0}: {1}")]
    InvalidEvent(PathBuf, EventError),

    #[error("expected exactly one event in {path}, found {found}")]
    ExpectedSingleEvent { path: PathBuf, found: usize },

    #[error(
        "expected exactly one VEVENT or VTODO in {path}, found {found}; \
         split it into one file per item"
    )]
    ExpectedSingleComponent { path: PathBuf, found: usize },

    #[error("no VEVENT or VTODO in {path} (found: {found})")]
    NoSupportedComponent { path: PathBuf, found: String },

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("event file not found: {0}")]
    NotFound(PathBuf),

    #[error(transparent)]
    Event(#[from] EventError),

    #[error("event {0} is not a recurring master")]
    NotRecurring(String),
}
