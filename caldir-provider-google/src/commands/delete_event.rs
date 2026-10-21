use anyhow::{Context, Result, anyhow};
use caldir_core::CalendarItem;
use caldir_core::provider::ProviderStorage;
use caldir_core::rpc::DeleteItem;
use caldir_core::{Event, RemoteConfigParams, Todo};
use google_calendar::types::SendUpdates;
use reqwest::StatusCode;

use crate::app_config::AppConfigStore;
use crate::constants::{PROVIDER_EVENT_ID_PROPERTY, PROVIDER_NAME, PROVIDER_TASK_ID_PROPERTY};
use crate::google_tasks;
use crate::remote_config::{GoogleRemoteConfig, GoogleTaskListRemoteConfig};
use crate::session::SessionStore;

pub async fn handle(cmd: DeleteItem) -> Result<()> {
    match cmd.event {
        CalendarItem::Event(event) => handle_event(cmd.remote, event).await,
        CalendarItem::Todo(todo) => handle_todo(cmd.remote, todo).await,
        _ => Err(anyhow!(
            "caldir-provider-google does not support deleting this item kind"
        )),
    }
}

/// The Tasks half of the delete split (§9): resolves the remote target
/// solely from `PROVIDER_TASK_ID_PROPERTY` — never by title — and treats a
/// task already gone on the remote as success, not failure, so a delete
/// raced by the Google UI first never wedges the sync retrying it forever.
/// Whether the delete was authorized was already decided by core; this
/// deletes what it is told to delete.
async fn handle_todo(remote: RemoteConfigParams, todo: Todo) -> Result<()> {
    let config = GoogleTaskListRemoteConfig::try_from(&remote)?;
    let account_email = &config.google_account;
    let task_list_id = &config.google_resource_id;

    let url = task_delete_request(task_list_id, &todo)?;

    let storage = ProviderStorage::for_provider(PROVIDER_NAME)?;
    let session_store = SessionStore::new(storage.clone());
    let app_config_store = AppConfigStore::new(storage);

    let session = session_store
        .load_valid(account_email, &app_config_store)
        .await?;
    session.require_tasks_scope()?;

    let status = google_tasks::delete_task(session.access_token(), &url)
        .await
        .context("Failed to delete task")?;

    classify_delete_status(status)
}

/// Pure request-building half of a task delete: resolves the DELETE target
/// from the stored `X-GOOGLE-TASK-ID` and never from `summary`/`uid` — a task
/// with no stored id is a named refusal, not a search, since a guessed
/// delete is unrecoverable. Kept separate from `delete_task`'s network call
/// so routing to the Tasks endpoint, not the Calendar one, is directly
/// testable without an HTTP mock this crate does not have. `pub(crate)` so
/// `google_tasks::tests` can bind this to `task_patch_request`'s target
/// without duplicating either.
pub(crate) fn task_delete_request(task_list_id: &str, todo: &Todo) -> Result<String> {
    let task_id = google_tasks::todo_x_property(todo, PROVIDER_TASK_ID_PROPERTY)
        .ok_or_else(|| anyhow!("Cannot delete task without {PROVIDER_TASK_ID_PROPERTY}"))?;

    Ok(google_tasks::task_url(task_list_id, task_id))
}

/// A `404` means the remote already holds the state the delete asked for —
/// success, not failure. Any other non-success status is a genuine failure
/// and must propagate, or a real API error would silently masquerade as an
/// already-deleted task and leak a remote resource with no local record.
fn classify_delete_status(status: StatusCode) -> Result<()> {
    if status.is_success() || status == StatusCode::NOT_FOUND {
        Ok(())
    } else {
        Err(anyhow!("Failed to delete task: unexpected status {status}"))
    }
}

async fn handle_event(remote: RemoteConfigParams, event: Event) -> Result<()> {
    let config = GoogleRemoteConfig::try_from(&remote)?;
    let account_email = &config.google_account;
    let calendar_id = &config.google_calendar_id;

    let google_event_id = event
        .x_property(PROVIDER_EVENT_ID_PROPERTY)
        .ok_or_else(|| {
            anyhow::anyhow!("Cannot delete event without {PROVIDER_EVENT_ID_PROPERTY}")
        })?;

    let storage = ProviderStorage::for_provider(PROVIDER_NAME)?;
    let session_store = SessionStore::new(storage.clone());
    let app_config_store = AppConfigStore::new(storage);

    let session = session_store
        .load_valid(account_email, &app_config_store)
        .await?;
    let client = session_store.client(&session, &app_config_store)?;

    client
        .events()
        .delete(calendar_id, google_event_id, false, SendUpdates::All)
        .await
        .context("Failed to delete event")?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use caldir_core::XProperty;

    #[test]
    fn a_task_delete_targets_the_stored_google_task_id() {
        let mut todo = Todo::new("Buy milk");
        todo.x_properties
            .push(XProperty::new(PROVIDER_TASK_ID_PROPERTY, "abc123"));

        let url = task_delete_request("task-list-123", &todo).unwrap();

        assert_eq!(
            url,
            "https://tasks.googleapis.com/tasks/v1/lists/task-list-123/tasks/abc123"
        );
    }

    #[test]
    fn a_task_without_a_stored_google_id_is_refused_by_name() {
        let todo = Todo::new("Buy milk");

        let error = task_delete_request("task-list-123", &todo).unwrap_err();

        assert_eq!(
            error.to_string(),
            format!("Cannot delete task without {PROVIDER_TASK_ID_PROPERTY}")
        );
    }

    #[test]
    fn an_already_absent_remote_task_is_a_successful_delete() {
        assert!(classify_delete_status(StatusCode::NOT_FOUND).is_ok());
    }

    #[test]
    fn a_genuine_api_failure_is_not_read_as_already_deleted() {
        assert!(classify_delete_status(StatusCode::FORBIDDEN).is_err());
        assert!(classify_delete_status(StatusCode::INTERNAL_SERVER_ERROR).is_err());
    }

    #[test]
    fn a_task_delete_never_reaches_the_calendar_events_endpoint() {
        let mut todo = Todo::new("Buy milk");
        todo.x_properties
            .push(XProperty::new(PROVIDER_TASK_ID_PROPERTY, "abc123"));

        let url = task_delete_request("task-list-123", &todo).unwrap();

        assert!(url.starts_with("https://tasks.googleapis.com/"));
        assert!(!url.contains("/calendar/"));
        assert!(!url.contains("events"));
    }
}
