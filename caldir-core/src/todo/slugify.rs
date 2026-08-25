use crate::event::EventTime;
use crate::event::slugify::{summary_slug, time_slug};
use crate::todo::Todo;

impl Todo {
    /// Generate a slug for a task, used as the filename for its .ics file.
    ///
    /// Tasks take the reserved `_task__` prefix, extending the same convention
    /// `_recurring__` already uses for events: a leading `_…__` marks a kind of
    /// file. `__` is the one separator the summary slugifier can never emit, so
    /// `_task__recurring__x` can't be forged by a task titled "recurring x".
    ///
    /// Recurring tasks get no date component (a single date would be
    /// misleading), and an undated task simply has none — never a synthesized
    /// one (R46).
    pub fn base_slug(&self) -> String {
        let summary = summary_slug(self.summary.as_deref());

        if self.recurrence.is_some() {
            return format!("_task__recurring__{summary}");
        }

        match self.filename_anchor() {
            Some(anchor) => format!("_task__{}__{summary}", time_slug(anchor)),
            None => format!("_task__{summary}"),
        }
    }

    /// `DUE` is the task's anchor; `DTSTART` is the fallback. Independent
    /// fields — neither is ever synthesized from the other (R9, R46).
    fn filename_anchor(&self) -> Option<&EventTime> {
        self.due.as_ref().or(self.start.as_ref())
    }
}
