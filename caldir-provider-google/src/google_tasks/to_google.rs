//! `caldir_core::Todo` -> Google Task insert body.
//!
//! Mirrors `google_event::to_google`'s shape (a `ToGoogle` trait, imported by
//! explicit path since both crates define one) but produces a Tasks-shaped
//! wire body — never a `google_calendar::types::Event` (§9: a VTODO must
//! never travel the Calendar API path).

use caldir_core::{EventTime, Todo, TodoStatus};
use chrono::TimeZone;
use serde::Serialize;

use super::todo_x_property;
use crate::constants::PROVIDER_TASK_ID_PROPERTY;

/// Insert-request body for `tasks.insert`. Distinct from `client::Task` (the
/// response shape) since a request never carries `deleted`/`hidden`.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct GoogleTaskInsert {
    /// Left absent for a never-synced task: a client-set resource id gets
    /// "Invalid resource id value" back from Google, the same trap
    /// `create_event.rs`'s event path already works around.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub title: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub notes: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed: Option<String>,
}

/// Patch-request body for `tasks.patch`. Distinct from `GoogleTaskInsert`:
/// Google Tasks patch semantics treat an omitted field as "leave alone" and
/// require an explicit `null` to clear it, so `notes`/`due`/`completed` never
/// skip serialization here the way the insert body's do.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct GoogleTaskPatch {
    pub title: String,
    pub notes: Option<String>,
    pub status: String,
    pub due: Option<String>,
    pub completed: Option<String>,
}

pub trait ToGooglePatch {
    /// See `ToGoogle::to_google` — same `zone` parameter, same faithful set,
    /// patch shape instead of insert shape.
    fn to_google_patch<Tz: TimeZone>(&self, zone: &Tz) -> GoogleTaskPatch;
}

impl ToGooglePatch for Todo {
    fn to_google_patch<Tz: TimeZone>(&self, zone: &Tz) -> GoogleTaskPatch {
        let status = match self.status {
            TodoStatus::Completed => "completed",
            _ => "needsAction",
        }
        .to_string();

        GoogleTaskPatch {
            title: self.summary.clone().unwrap_or_default(),
            notes: self.description.clone(),
            status,
            due: self.due.as_ref().map(|due| due_to_google(due, zone)),
            completed: self
                .completed
                .as_ref()
                .map(|completed| completed_to_google(completed, zone)),
        }
    }
}

pub trait ToGoogle {
    /// `zone` is the caller's chosen zone, taken explicitly rather than the
    /// function reaching for `chrono::Local` itself — the minimum needed to
    /// send a `DUE` at all. Full timezone canonicalization of the write
    /// direction is a separate, later concern.
    fn to_google<Tz: TimeZone>(&self, zone: &Tz) -> GoogleTaskInsert;
}

impl ToGoogle for Todo {
    fn to_google<Tz: TimeZone>(&self, zone: &Tz) -> GoogleTaskInsert {
        let status = match self.status {
            TodoStatus::Completed => "completed",
            // Google Tasks has no IN-PROCESS/CANCELLED equivalent (mirrors
            // the read-direction fallback in `from_google::task_to_todo`).
            _ => "needsAction",
        }
        .to_string();

        GoogleTaskInsert {
            id: todo_x_property(self, PROVIDER_TASK_ID_PROPERTY).map(str::to_string),
            title: self.summary.clone().unwrap_or_default(),
            notes: self.description.clone().unwrap_or_default(),
            status,
            due: self.due.as_ref().map(|due| due_to_google(due, zone)),
            completed: self
                .completed
                .as_ref()
                .map(|completed| completed_to_google(completed, zone)),
        }
    }
}

/// `DUE` -> `due`: the calendar date under `zone` (`super::due::
/// google_due_date`, D-4), sent as midnight UTC — the shape Google's `due`
/// field expects (date precision only, §13 clause 1).
fn due_to_google<Tz: TimeZone>(due: &EventTime, zone: &Tz) -> String {
    let date = super::due::google_due_date(due, zone);
    format!("{}T00:00:00.000Z", date.format("%Y-%m-%d"))
}

/// `COMPLETED` -> `completed`, an RFC 3339 instant (unlike `due`, Google
/// keeps time precision here).
fn completed_to_google<Tz: TimeZone>(completed: &EventTime, zone: &Tz) -> String {
    completed
        .to_local_tz(zone)
        .with_timezone(&chrono::Utc)
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn the_faithful_set_reaches_the_google_task() {
        let mut todo = Todo::new("Buy milk");
        todo.description = Some("2%, not skim".into());
        todo.status = TodoStatus::Completed;
        todo.completed = Some(EventTime::DateTimeUtc(
            "2026-06-10T19:17:57Z".parse().unwrap(),
        ));
        todo.due = Some(EventTime::Date("2026-09-02".parse().unwrap()));

        let google_task = todo.to_google(&Utc);

        assert_eq!(google_task.title, "Buy milk");
        assert_eq!(google_task.notes, "2%, not skim");
        assert_eq!(google_task.status, "completed");
        assert_eq!(
            google_task.completed.as_deref(),
            Some("2026-06-10T19:17:57.000Z")
        );
        assert_eq!(google_task.due.as_deref(), Some("2026-09-02T00:00:00.000Z"));
    }

    #[test]
    fn a_never_synced_task_is_sent_without_a_client_chosen_id() {
        let todo = Todo::new("Buy milk");

        let google_task = todo.to_google(&Utc);

        assert!(google_task.id.is_none());

        // On the wire, not just at the Rust type level: `id` must not appear
        // at all, not merely be an empty string.
        let json = serde_json::to_value(&google_task).unwrap();
        assert!(json.get("id").is_none());
    }
}
