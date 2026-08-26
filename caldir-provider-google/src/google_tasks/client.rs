//! Task-list enumeration only (P4-03 scope). Task content (`tasks.list`) and
//! writes are `P4-04`/Wave 5.

use anyhow::{Context, Result};
use serde::Deserialize;

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
