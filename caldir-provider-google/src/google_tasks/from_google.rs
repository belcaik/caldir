//! Google Task -> `caldir_core::Todo` mapping (P4-04).
//!
//! Mirrors `google_event::from_google`'s shape: a private serde mirror of the
//! remote resource (`Task`, below) and a conversion into a core type. Neither
//! `Task` nor anything Tasks-specific ever crosses out of this module —
//! `commands::list_events::handle` only ever sees `CalendarItem` (CROSS-6).

use anyhow::Result;
use caldir_core::{CalendarItem, EventTime, EventUid, Todo, TodoStatus, XProperty};

use super::client::Task;
use crate::constants::PROVIDER_TASK_ID_PROPERTY;

/// One named, deterministic, documented function producing the UID for a task
/// first seen remotely (criterion 7). Google Tasks has no iCalendar UID, so
/// caldir derives one from the Google task id — the same shape
/// `commands::list_events::cancellation_to_event` uses for a synthetic event
/// UID (`format!("{}@google.com", ge.recurring_event_id)`). A locally-created
/// task keeps its own UID on push (`P5-01`); this function only fires for a
/// task caldir has never seen before.
pub fn derive_task_uid(google_task_id: &str) -> EventUid {
    EventUid::new(format!("{google_task_id}@google-tasks.com"))
}

/// Converts a Google Task into a `Todo`, or `None` if the task was deleted
/// remotely (`deleted: true`). `hidden` is not a delete — Google hides
/// completed tasks in its own UI, but the directory should still contain
/// them.
pub fn task_to_todo(task: Task) -> Result<Option<Todo>> {
    if task.deleted {
        return Ok(None);
    }

    let status = match task.status.as_str() {
        "completed" => TodoStatus::Completed,
        // "needsAction" or anything else: Google has no IN-PROCESS/CANCELLED
        // equivalent, so neither is ever synthesized (spec §14).
        _ => TodoStatus::NeedsAction,
    };

    let completed = task
        .completed
        .as_deref()
        .and_then(|c| chrono::DateTime::parse_from_rfc3339(c).ok())
        .map(|dt| EventTime::DateTimeUtc(dt.with_timezone(&chrono::Utc)));

    let due = task.due.as_deref().and_then(google_due_to_date);

    let mut todo = Todo::new(task.title.clone());
    todo.uid = derive_task_uid(&task.id);
    todo.summary = if task.title.is_empty() {
        None
    } else {
        Some(task.title)
    };
    todo.description = if task.notes.is_empty() {
        None
    } else {
        Some(task.notes)
    };
    todo.due = due;
    todo.status = status;
    todo.completed = completed;
    todo.x_properties = vec![XProperty::new(PROVIDER_TASK_ID_PROPERTY, task.id)];

    Ok(Some(todo))
}

/// Converts a Google Task straight into a `CalendarItem::Todo` — the seam
/// `list_events`'s task path calls. `None` for a deleted task.
pub fn task_to_calendar_item(task: Task) -> Result<Option<CalendarItem>> {
    Ok(task_to_todo(task)?.map(CalendarItem::Todo))
}

/// `due`'s wire type is RFC 3339, but per the Tasks API reference "the due
/// date only records date information; the time portion... is discarded"
/// (`docs/vtodo/provider-sync/providers/google-tasks.md`). §13 clause 2 reads
/// it as a date-only `DUE`, via `super::due::date_to_due` — the same
/// function the write direction's inverse funnels through, so the pair
/// can't drift (§7).
///
/// The calendar date comes from the timestamp's **own** offset (RFC 3339
/// requires one) — never from `chrono::Local`/the system zone (C-18). Google
/// always sends an explicit `Z`, so this never needs a system-zone fallback;
/// the parsed offset is used as-is via `date_naive()`.
fn google_due_to_date(due: &str) -> Option<EventTime> {
    let dt = chrono::DateTime::parse_from_rfc3339(due).ok()?;
    Some(super::due::date_to_due(dt.date_naive()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_task(id: &str, title: &str) -> Task {
        Task {
            id: id.into(),
            title: title.into(),
            notes: String::new(),
            status: "needsAction".into(),
            due: None,
            completed: None,
            deleted: false,
            hidden: false,
        }
    }

    #[test]
    fn a_google_task_becomes_a_todo_never_an_event() {
        let item = task_to_calendar_item(minimal_task("task1", "Buy milk"))
            .unwrap()
            .unwrap();

        let CalendarItem::Todo(ref todo) = item else {
            panic!("expected a Todo, got {item:?}");
        };
        // CROSS-6, on the serialized bytes: a Google Task never becomes a
        // VEVENT on the wire, not just at the Rust type level.
        let ics = todo.to_ics_string();
        assert!(ics.contains("BEGIN:VTODO"));
        assert!(!ics.contains("BEGIN:VEVENT"));
    }

    #[test]
    fn title_and_notes_become_summary_and_description() {
        let mut task = minimal_task("task1", "Buy milk");
        task.notes = "2%, not skim".into();

        let todo = task_to_todo(task).unwrap().unwrap();

        assert_eq!(todo.summary.as_deref(), Some("Buy milk"));
        assert_eq!(todo.description.as_deref(), Some("2%, not skim"));

        let empty = minimal_task("task2", "");
        let todo = task_to_todo(empty).unwrap().unwrap();
        assert_eq!(todo.summary, None);
        assert_eq!(todo.description, None);
    }

    #[test]
    fn a_completed_google_task_carries_its_status_and_time() {
        let mut task = minimal_task("task1", "Buy milk");
        task.status = "completed".into();
        task.completed = Some("2026-06-10T19:17:57.000Z".into());

        let todo = task_to_todo(task).unwrap().unwrap();

        assert_eq!(todo.status, TodoStatus::Completed);
        assert_eq!(
            todo.completed,
            Some(EventTime::DateTimeUtc(
                "2026-06-10T19:17:57Z".parse().unwrap()
            ))
        );

        let mut task = minimal_task("task2", "Buy milk");
        task.status = "needsAction".into();

        let todo = task_to_todo(task).unwrap().unwrap();

        assert_eq!(todo.status, TodoStatus::NeedsAction);
        assert_eq!(todo.completed, None);
    }

    /// TZ-marked: run under `TZ=UTC`, `TZ=Europe/Stockholm` and
    /// `TZ=America/New_York` (C-18). Google always sends midnight UTC for a
    /// date-only `due`; a buggy implementation that reaches for
    /// `EventTime::to_utc`/`chrono::Local` shifts the date backward one day
    /// in a negative-offset zone like `America/New_York`, and this fixture
    /// (`2026-09-02T00:00:00.000Z`) is chosen exactly to expose that: local
    /// midnight UTC is still `2026-09-01` evening in New York.
    #[test]
    fn a_google_due_becomes_a_date_only_due() {
        let mut task = minimal_task("task1", "Submit report");
        task.due = Some("2026-09-02T00:00:00.000Z".into());

        let todo = task_to_todo(task).unwrap().unwrap();

        assert_eq!(
            todo.due,
            Some(EventTime::Date("2026-09-02".parse().unwrap()))
        );
    }

    /// §13 clause 2, exhaustively: reusing `google_dt_to_event_time`
    /// (`google_event/from_google.rs`) on this path is the honest-looking
    /// implementation that fails this — it produces `DateTimeUtc(midnight)`,
    /// which then differs from the local file's date-only `DUE` on every
    /// comparison forever. Only an exhaustive match, not `assert_eq!`
    /// against one expected variant, catches every way `due` could
    /// accidentally carry a time-of-day.
    #[test]
    fn a_google_due_becomes_a_date_only_due_never_a_midnight_timestamp() {
        let mut task = minimal_task("task1", "Submit report");
        task.due = Some("2026-09-01T00:00:00.000Z".into());

        let todo = task_to_todo(task).unwrap().unwrap();

        match todo.due {
            Some(EventTime::Date(date)) => {
                assert_eq!(date, "2026-09-01".parse().unwrap());
            }
            other => panic!("expected a date-only DUE, got {other:?}"),
        }
    }

    #[test]
    fn the_task_id_is_an_x_property_and_never_the_uid() {
        let todo = task_to_todo(minimal_task("abcXYZ123", "Buy milk"))
            .unwrap()
            .unwrap();

        assert!(
            todo.x_properties
                .contains(&XProperty::new(PROVIDER_TASK_ID_PROPERTY, "abcXYZ123"))
        );
        assert_ne!(todo.uid.as_str(), "abcXYZ123");
    }

    #[test]
    fn a_deleted_google_task_is_dropped_from_the_listing() {
        let mut deleted = minimal_task("task1", "Buy milk");
        deleted.deleted = true;

        assert!(task_to_todo(deleted).unwrap().is_none());

        let mut hidden_completed = minimal_task("task2", "Buy milk");
        hidden_completed.status = "completed".into();
        hidden_completed.hidden = true;

        assert!(task_to_todo(hidden_completed).unwrap().is_some());
    }
}
