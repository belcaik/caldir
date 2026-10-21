use std::path::{Path, PathBuf};

use super::error::CalendarError;
use super::event::CalendarEvent;
use crate::CalendarItem;

/// An `.ics` file that could not be loaded, and why.
#[derive(Debug)]
pub struct SkippedEvent {
    path: PathBuf,
    error: CalendarError,
}

impl SkippedEvent {
    pub(crate) fn new(path: PathBuf, error: CalendarError) -> Self {
        SkippedEvent { path, error }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn error(&self) -> &CalendarError {
        &self.error
    }
}

/// The result of listing a calendar: the events that loaded, plus a record of
/// every `.ics` file that did not. A listing is never silently short — skipped
/// files are always reported.
#[derive(Debug, Default)]
pub struct CalendarListing {
    events: Vec<CalendarEvent>,
    skipped: Vec<SkippedEvent>,
}

impl CalendarListing {
    pub(crate) fn push_event(&mut self, event: CalendarEvent) {
        self.events.push(event);
    }

    pub(crate) fn push_skipped(&mut self, path: PathBuf, error: CalendarError) {
        self.skipped.push(SkippedEvent::new(path, error));
    }

    pub fn events(&self) -> &[CalendarEvent] {
        &self.events
    }

    pub fn into_events(self) -> Vec<CalendarEvent> {
        self.events
    }

    /// Both halves at once, for a caller that must not drop either.
    pub(crate) fn into_parts(self) -> (Vec<CalendarEvent>, Vec<SkippedEvent>) {
        (self.events, self.skipped)
    }

    /// The files that failed to load. Empty means the listing is complete.
    pub fn skipped(&self) -> &[SkippedEvent] {
        &self.skipped
    }

    /// Whether any file was skipped. Callers that infer deletions from absence
    /// MUST check this first.
    pub fn is_incomplete(&self) -> bool {
        !self.skipped.is_empty()
    }

    /// Number of events that loaded (skipped files are not counted).
    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}

/// The items to show for a window, plus the `.ics` files that could not be
/// read. Kept together so a view cannot render one without the other.
#[derive(Debug)]
pub struct ExpandedItems {
    pub items: Vec<CalendarItem>,
    pub skipped: Vec<SkippedEvent>,
}
