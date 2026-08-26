//! Task-list enumeration (P4-03) and task listing (P4-04). Writes are Wave 5.

use anyhow::{Context, Result};
use serde::Deserialize;

use super::to_google::{GoogleTaskInsert, GoogleTaskPatch};

const TASKLISTS_URL: &str = "https://tasks.googleapis.com/tasks/v1/users/@me/lists";

/// A Google Tasks list (`TaskLists` resource). Private to this module and its
/// caller inside `commands::list_calendars` — never returned from a
/// `Handler` method.
#[derive(Debug, Clone, Deserialize)]
pub struct TaskList {
    pub id: String,
    pub title: String,
}

#[derive(Debug, Deserialize)]
struct TaskListsResponse {
    #[serde(default)]
    items: Vec<TaskList>,
}

/// `tasklists.list` — enumerate the caller's Google Tasks lists.
pub async fn list_task_lists(access_token: &str) -> Result<Vec<TaskList>> {
    let response = reqwest::Client::new()
        .get(TASKLISTS_URL)
        .bearer_auth(access_token)
        .send()
        .await
        .context("Failed to fetch Google Task lists")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("Failed to fetch Google Task lists ({status}): {body}");
    }

    let parsed: TaskListsResponse = response
        .json()
        .await
        .context("Failed to parse Google Task lists response")?;

    Ok(parsed.items)
}

/// A Google Tasks `Task` resource, deserialized as-is off the wire. Private
/// to this module and `from_google` — mapped to `caldir_core::Todo` before it
/// ever reaches a `Handler` method (containment, spec §9).
///
/// Field list per `docs/vtodo/provider-sync/providers/google-tasks.md` "The
/// Tasks surface caldir needs": `id`, `title`, `notes`, `status`, `due`,
/// `completed`, `deleted`, `hidden`. `updated` is read but unused by this
/// ticket's mapping (`Todo::last_modified` from this path is `P5-06` scope).
#[derive(Debug, Clone, Deserialize)]
pub struct Task {
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub status: String,
    pub due: Option<String>,
    pub completed: Option<String>,
    #[serde(default)]
    pub deleted: bool,
    /// Deliberately never read by the mapping: `hidden` means "Google's own
    /// UI hid a completed task", not "deleted" — the trap this field exists
    /// to name. Kept on the struct for completeness with the resource shape
    /// and exercised directly by tests.
    #[serde(default)]
    #[allow(dead_code)]
    pub hidden: bool,
}

#[derive(Debug, Deserialize)]
struct TasksResponse {
    #[serde(default)]
    items: Vec<Task>,
}

/// `tasks.list` — list every task in one task list, including deleted and
/// hidden ones. `showDeleted`/`showHidden` are needed so a remote delete is
/// distinguishable from "never existed" (mirrors `showDeleted` on the
/// Calendar path in `commands::list_events::handle`); `showCompleted`
/// defaults to `true` but is passed explicitly for clarity.
pub fn tasks_url(task_list_id: &str) -> String {
    format!("https://tasks.googleapis.com/tasks/v1/lists/{task_list_id}/tasks")
}

/// URL of one task within a list — the PATCH target for an update.
pub fn task_url(task_list_id: &str, task_id: &str) -> String {
    format!("{}/{task_id}", tasks_url(task_list_id))
}

pub async fn list_tasks(access_token: &str, task_list_id: &str) -> Result<Vec<Task>> {
    let response = reqwest::Client::new()
        .get(tasks_url(task_list_id))
        .bearer_auth(access_token)
        .query(&[
            ("showDeleted", "true"),
            ("showHidden", "true"),
            ("showCompleted", "true"),
        ])
        .send()
        .await
        .context("Failed to fetch Google Tasks")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("Failed to fetch Google Tasks ({status}): {body}");
    }

    let parsed: TasksResponse = response
        .json()
        .await
        .context("Failed to parse Google Tasks response")?;

    Ok(parsed.items)
}

/// `tasks.insert` — create one task in the given list. Never `events().insert()`
/// (§9: a task must never travel the Calendar API): this hits
/// `tasks.googleapis.com`, not `googleapis.com/calendar`.
pub async fn insert_task(
    access_token: &str,
    task_list_id: &str,
    body: &GoogleTaskInsert,
) -> Result<Task> {
    let response = reqwest::Client::new()
        .post(tasks_url(task_list_id))
        .bearer_auth(access_token)
        .json(body)
        .send()
        .await
        .context("Failed to create Google Task")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("Failed to create Google Task ({status}): {body}");
    }

    response
        .json()
        .await
        .context("Failed to parse Google Task create response")
}

/// `tasks.patch` — update one task, addressed by `url` (built by
/// `task_url`). Never `events().patch()` (§9): this hits
/// `tasks.googleapis.com`, not `googleapis.com/calendar`.
pub async fn patch_task(access_token: &str, url: &str, body: &GoogleTaskPatch) -> Result<Task> {
    let response = reqwest::Client::new()
        .patch(url)
        .bearer_auth(access_token)
        .json(body)
        .send()
        .await
        .context("Failed to update Google Task")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("Failed to update Google Task ({status}): {body}");
    }

    response
        .json()
        .await
        .context("Failed to parse Google Task update response")
}
