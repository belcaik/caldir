//! Task-list enumeration and task listing. Writes are a later addition.

use std::collections::HashSet;
use std::future::Future;

use anyhow::{Context, Result};
use serde::Deserialize;

use super::to_google::{GoogleTaskInsert, GoogleTaskPatch};

const TASKLISTS_URL: &str = "https://tasks.googleapis.com/tasks/v1/users/@me/lists";
const MAX_LIST_PAGES: usize = 1000;

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
    #[serde(default, rename = "nextPageToken")]
    next_page_token: Option<String>,
}

/// `tasklists.list` — enumerate the caller's Google Tasks lists.
pub async fn list_task_lists(access_token: &str) -> Result<Vec<TaskList>> {
    let client = reqwest::Client::new();

    collect_pages(|page_token| {
        let mut request = client
            .get(TASKLISTS_URL)
            .bearer_auth(access_token)
            .query(&[("maxResults", "100")]);
        if let Some(page_token) = page_token.as_deref() {
            request = request.query(&[("pageToken", page_token)]);
        }

        async move {
            let response = request
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

            Ok((parsed.items, parsed.next_page_token))
        }
    })
    .await
}

/// A Google Tasks `Task` resource, deserialized as-is off the wire. Private
/// to this module and `from_google` — mapped to `caldir_core::Todo` before it
/// ever reaches a `Handler` method (containment, spec §9).
///
/// Field list per `docs/vtodo/provider-sync/providers/google-tasks.md` "The
/// Tasks surface caldir needs": `id`, `title`, `notes`, `status`, `due`,
/// `completed`, `deleted`, `hidden`. `updated` is read but unused by this
/// mapping (`Todo::last_modified` from this path is out of scope here).
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
    #[serde(default, rename = "nextPageToken")]
    next_page_token: Option<String>,
}

async fn collect_pages<T, Fetch, FetchFuture>(mut fetch_page: Fetch) -> Result<Vec<T>>
where
    Fetch: FnMut(Option<String>) -> FetchFuture,
    FetchFuture: Future<Output = Result<(Vec<T>, Option<String>)>>,
{
    let mut items = Vec::new();
    let mut page_token = None;
    let mut seen_tokens = HashSet::new();

    for _ in 0..MAX_LIST_PAGES {
        let (page_items, next_page_token) = fetch_page(page_token).await?;
        items.extend(page_items);

        let Some(next_page_token) = next_page_token else {
            return Ok(items);
        };
        if !seen_tokens.insert(next_page_token.clone()) {
            anyhow::bail!("Google Tasks listing repeated page token {next_page_token:?}");
        }
        page_token = Some(next_page_token);
    }

    anyhow::bail!("Google Tasks listing exceeded {MAX_LIST_PAGES} pages")
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
    let client = reqwest::Client::new();

    collect_pages(|page_token| {
        let mut request = client
            .get(tasks_url(task_list_id))
            .bearer_auth(access_token)
            .query(&[
                ("showDeleted", "true"),
                ("showHidden", "true"),
                ("showCompleted", "true"),
                ("maxResults", "100"),
            ]);
        if let Some(page_token) = page_token.as_deref() {
            request = request.query(&[("pageToken", page_token)]);
        }

        async move {
            let response = request
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

            Ok((parsed.items, parsed.next_page_token))
        }
    })
    .await
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

/// `tasks.delete` — remove one task, addressed by `url` (built by
/// `task_url`). Returns the raw response status; whether that status counts
/// as success (including an already-gone task) is the caller's pure
/// classifier, not this network wrapper. Never `events().delete()` (§9):
/// this hits `tasks.googleapis.com`, not `googleapis.com/calendar`.
pub async fn delete_task(access_token: &str, url: &str) -> Result<reqwest::StatusCode> {
    let response = reqwest::Client::new()
        .delete(url)
        .bearer_auth(access_token)
        .send()
        .await
        .context("Failed to delete Google Task")?;

    Ok(response.status())
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

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::future::ready;

    use anyhow::anyhow;

    use super::*;

    #[tokio::test]
    async fn collect_pages_concatenates_pages_and_passes_tokens_in_order() {
        let mut received_tokens = Vec::new();
        let mut pages = VecDeque::from([
            Ok((vec![1, 2], Some("page-2".to_string()))),
            Ok((vec![3], Some("page-3".to_string()))),
            Ok((vec![4, 5], None)),
        ]);

        let items = collect_pages(|page_token| {
            received_tokens.push(page_token);
            ready(pages.pop_front().expect("unexpected page request"))
        })
        .await
        .expect("pagination should succeed");

        assert_eq!(items, vec![1, 2, 3, 4, 5]);
        assert_eq!(
            received_tokens,
            vec![None, Some("page-2".to_string()), Some("page-3".to_string())]
        );
    }

    #[tokio::test]
    async fn collect_pages_returns_error_when_second_page_fails() {
        let mut pages = VecDeque::from([
            Ok((vec![1], Some("page-2".to_string()))),
            Err(anyhow!("page 2 failed")),
        ]);

        let error = collect_pages(|_| ready(pages.pop_front().expect("unexpected page request")))
            .await
            .expect_err("pagination should fail");

        assert_eq!(error.to_string(), "page 2 failed");
    }

    #[tokio::test]
    async fn collect_pages_returns_error_when_a_page_token_repeats() {
        let mut pages = VecDeque::from([
            Ok((vec![1], Some("same-token".to_string()))),
            Ok((vec![2], Some("same-token".to_string()))),
        ]);

        let error = collect_pages(|_| ready(pages.pop_front().expect("unexpected page request")))
            .await
            .expect_err("pagination should fail");

        assert!(error.to_string().contains("repeated page token"));
    }

    #[test]
    fn tasks_response_deserializes_next_page_token() {
        let response: TasksResponse =
            serde_json::from_str(r#"{"items":[{"id":"task-1"}],"nextPageToken":"page-2"}"#)
                .expect("response should deserialize");

        assert_eq!(response.next_page_token.as_deref(), Some("page-2"));
    }
}
