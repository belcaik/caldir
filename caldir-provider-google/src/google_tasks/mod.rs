//! Hand-rolled Google Tasks REST client (P4-01 spike decision: reqwest, no
//! new crate). Every Tasks-specific type stays private to this module —
//! nothing here may cross into caldir-core or the shared RPC (P4-01
//! Containment).

mod client;
mod from_google;
mod to_google;

pub use client::{Task, insert_task, list_task_lists, list_tasks, tasks_url};
pub use from_google::task_to_calendar_item;
pub use to_google::{GoogleTaskInsert, ToGoogle};

/// `Todo` has no `x_property()` accessor (that's `Event`-only) — this reads
/// `x_properties` directly rather than adding one to `caldir-core`.
pub(crate) fn todo_x_property<'a>(todo: &'a caldir_core::Todo, name: &str) -> Option<&'a str> {
    todo.x_properties
        .iter()
        .find(|p| p.name == name)
        .map(|p| p.value.as_str())
}
