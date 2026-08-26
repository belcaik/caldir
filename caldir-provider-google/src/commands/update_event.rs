use anyhow::{Context, Result, anyhow, bail};
use caldir_core::provider::{ProviderStorage, classified_error};
use caldir_core::rpc::{ErrorCode, UpdateItem};
use caldir_core::{CalendarItem, Event, RemoteConfigParams, Todo};
use serde_json::Value;

use crate::app_config::AppConfigStore;
use crate::commands::create_event::merge_canonical_task_response;
use crate::commands::invite::patch_invite_status;
use crate::constants::{PROVIDER_EVENT_ID_PROPERTY, PROVIDER_NAME, PROVIDER_TASK_ID_PROPERTY};
use crate::google_event::{FromGoogle, ToGoogle};
use crate::google_tasks::{self, ToGooglePatch};
use crate::remote_config::{GoogleRemoteConfig, GoogleTaskListRemoteConfig};
use crate::session::SessionStore;

pub async fn handle(cmd: UpdateItem) -> Result<CalendarItem> {
    match cmd.event {
        CalendarItem::Event(event) => {
            Ok(CalendarItem::Event(handle_event(cmd.remote, event).await?))
        }
        CalendarItem::Todo(todo) => Ok(CalendarItem::Todo(handle_todo(cmd.remote, todo).await?)),
        _ => Err(anyhow!(
            "caldir-provider-google does not support updating this item kind"
        )),
    }
}

/// The Tasks half of the update split (§9): resolves the remote target
/// solely from `PROVIDER_TASK_ID_PROPERTY` — never by title, never by a
/// search — and returns the canonical `Todo` Google's patch response
/// describes.
async fn handle_todo(remote: RemoteConfigParams, todo: Todo) -> Result<Todo> {
    google_tasks::policy::reject_recurring(&todo).map_err(|e| {
        anyhow::Error::from_boxed(classified_error(
            ErrorCode::SemanticLimitation,
            e.to_string(),
        ))
    })?;
    if let Some(warning) = google_tasks::policy::semantic_change_warning(&todo) {
        eprintln!("{warning}");
    }

    let config = GoogleTaskListRemoteConfig::try_from(&remote)?;
    let account_email = &config.google_account;
    let task_list_id = &config.google_resource_id;

    let storage = ProviderStorage::for_provider(PROVIDER_NAME)?;
    let session_store = SessionStore::new(storage.clone());
    let app_config_store = AppConfigStore::new(storage);

    let session = session_store
        .load_valid(account_email, &app_config_store)
        .await?;
    session.require_tasks_scope()?;

    let (url, body) = task_patch_request(task_list_id, &todo)?;

    let response = google_tasks::patch_task(session.access_token(), &url, &body)
        .await
        .with_context(|| format!("Failed to update task: {}", body.title))?;

    merge_canonical_task_response(todo, response)
}

/// Pure request-building half of a task update: resolves the PATCH target
/// from the stored `X-GOOGLE-TASK-ID` and never from `summary`/`uid` — a task
/// with no stored id is a named refusal, not a search. Kept separate from
/// `patch_task`'s network call for the same reason `task_insert_request` is:
/// no HTTP mock in this crate's dev-dependencies. `pub(crate)` so
/// `google_tasks::tests` can bind this to `task_delete_request`'s target
/// without duplicating either.
pub(crate) fn task_patch_request(
    task_list_id: &str,
    todo: &Todo,
) -> Result<(String, google_tasks::GoogleTaskPatch)> {
    let task_id = google_tasks::todo_x_property(todo, PROVIDER_TASK_ID_PROPERTY)
        .ok_or_else(|| anyhow!("Cannot update task without {PROVIDER_TASK_ID_PROPERTY}"))?;

    let url = google_tasks::task_url(task_list_id, task_id);
    let body = todo.to_google_patch(&chrono::Local);

    Ok((url, body))
}

async fn handle_event(remote: RemoteConfigParams, event: Event) -> Result<Event> {
    let config = GoogleRemoteConfig::try_from(&remote)?;
    let account_email = &config.google_account;
    let calendar_id = &config.google_calendar_id;

    let storage = ProviderStorage::for_provider(PROVIDER_NAME)?;
    let session_store = SessionStore::new(storage.clone());
    let app_config_store = AppConfigStore::new(storage);

    let session = session_store
        .load_valid(account_email, &app_config_store)
        .await?;

    // Get Google's event ID from custom properties
    let google_event_id = event
        .x_property(PROVIDER_EVENT_ID_PROPERTY)
        .ok_or_else(|| anyhow!("Cannot update event without {PROVIDER_EVENT_ID_PROPERTY}"))?;

    if event.is_invite_for(account_email) {
        // Only update our own attendee status:
        let google_event = patch_invite_status(
            &session,
            calendar_id,
            google_event_id,
            &event,
            account_email,
        )
        .await?;

        Ok(Event::from_google(google_event)?)
    } else {
        // Organizer or own event: PATCH event fields, but never send attendees.
        // A single EXDATE edit is a master update; sending a full attendee list
        // here can rewrite invite state across the whole series.
        let google_event = patch_event_without_attendees(
            session.access_token(),
            calendar_id,
            google_event_id,
            &event,
        )
        .await?;

        Ok(Event::from_google(google_event)?)
    }
}

async fn patch_event_without_attendees(
    access_token: &str,
    calendar_id: &str,
    event_id: &str,
    event: &Event,
) -> Result<google_calendar::types::Event> {
    let body = patch_body_without_attendees(event)?;

    let url = format!(
        "https://www.googleapis.com/calendar/v3/calendars/{}/events/{}?\
         sendUpdates=all&conferenceDataVersion=1",
        calendar_id, event_id,
    );

    let response = reqwest::Client::new()
        .patch(&url)
        .bearer_auth(access_token)
        .json(&body)
        .send()
        .await?;

    if !response.status().is_success() {
        let error_text = response.text().await.unwrap_or_default();
        bail!("Error handling request: {}", error_text);
    }

    Ok(response.json().await?)
}

fn patch_body_without_attendees(event: &Event) -> Result<Value> {
    let mut body = serde_json::to_value(event.to_google())?;

    if let Value::Object(fields) = &mut body {
        fields.remove("attendees");
    }

    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use caldir_core::{Attendee, EventTime, Recurrence, Todo, XProperty};
    use chrono::NaiveDate;

    #[test]
    fn a_task_update_targets_the_stored_google_task_id() {
        let mut todo = Todo::new("Buy milk");
        todo.x_properties
            .push(XProperty::new(PROVIDER_TASK_ID_PROPERTY, "abc123"));

        let (url, _body) = task_patch_request("task-list-123", &todo).unwrap();

        assert_eq!(
            url,
            "https://tasks.googleapis.com/tasks/v1/lists/task-list-123/tasks/abc123"
        );
    }

    #[test]
    fn two_tasks_sharing_a_summary_resolve_to_different_targets() {
        // P1-05's archive holds repeated SUMMARY values across sibling
        // calendars ("Semana 11: Sumativa 5: Solemne 2"); a title-resolving
        // implementation would collapse these to one target.
        let shared_summary = "Semana 11: Sumativa 5: Solemne 2";

        let mut todo_a = Todo::new(shared_summary);
        todo_a
            .x_properties
            .push(XProperty::new(PROVIDER_TASK_ID_PROPERTY, "task-a"));

        let mut todo_b = Todo::new(shared_summary);
        todo_b
            .x_properties
            .push(XProperty::new(PROVIDER_TASK_ID_PROPERTY, "task-b"));

        let (url_a, _) = task_patch_request("task-list-123", &todo_a).unwrap();
        let (url_b, _) = task_patch_request("task-list-123", &todo_b).unwrap();

        assert_ne!(url_a, url_b);
        assert!(url_a.ends_with("/task-a"));
        assert!(url_b.ends_with("/task-b"));
    }

    #[test]
    fn a_task_without_a_stored_google_id_is_refused_by_name() {
        let todo = Todo::new("Buy milk");

        let error = task_patch_request("task-list-123", &todo).unwrap_err();

        assert_eq!(
            error.to_string(),
            format!("Cannot update task without {PROVIDER_TASK_ID_PROPERTY}")
        );
    }

    #[test]
    fn the_task_patch_body_carries_no_event_fields() {
        let mut todo = Todo::new("Buy milk");
        todo.description = Some("2%, not skim".into());
        todo.x_properties
            .push(XProperty::new(PROVIDER_TASK_ID_PROPERTY, "abc123"));

        let (_url, body) = task_patch_request("task-list-123", &todo).unwrap();
        let json = serde_json::to_value(&body).unwrap();

        assert_eq!(json.get("title").and_then(Value::as_str), Some("Buy milk"));
        assert!(json.get("notes").is_some());
        assert!(json.get("status").is_some());
        assert!(json.get("due").is_some());

        for calendar_only_key in [
            "summary",
            "start",
            "end",
            "attendees",
            "recurrence",
            "conferenceData",
        ] {
            assert!(
                json.get(calendar_only_key).is_none(),
                "task patch body must not carry {calendar_only_key}, got {json}"
            );
        }
    }

    #[test]
    fn the_updated_task_returns_googles_canonical_representation() {
        let mut local = Todo::new("Buy milk");
        local.uid = caldir_core::EventUid::new("local-uid-123");
        local
            .x_properties
            .push(XProperty::new(PROVIDER_TASK_ID_PROPERTY, "abc123"));

        let response = google_tasks::Task {
            id: "abc123".into(),
            title: "Buy milk and eggs".into(),
            notes: String::new(),
            status: "needsAction".into(),
            due: Some("2026-09-02T00:00:00.000Z".into()),
            completed: None,
            deleted: false,
            hidden: false,
        };

        let returned = merge_canonical_task_response(local.clone(), response).unwrap();

        assert_eq!(returned.uid, local.uid);
        assert_eq!(returned.summary.as_deref(), Some("Buy milk and eggs"));
        assert_eq!(
            returned.due,
            Some(EventTime::Date("2026-09-02".parse().unwrap()))
        );
        assert_eq!(
            returned
                .x_properties
                .iter()
                .filter(|p| p.name == PROVIDER_TASK_ID_PROPERTY)
                .count(),
            1
        );
        assert!(
            returned
                .x_properties
                .contains(&XProperty::new(PROVIDER_TASK_ID_PROPERTY, "abc123"))
        );
    }

    #[test]
    fn patch_body_omits_attendees() {
        let mut event = Event::new(
            "Weekly sync",
            EventTime::Date(NaiveDate::from_ymd_opt(2026, 7, 10).unwrap()),
        );
        event.recurrence = Some(Recurrence::new("FREQ=WEEKLY"));
        event.attendees = vec![Attendee::new("alice@example.com")];

        let body = patch_body_without_attendees(&event).unwrap();

        assert!(body.get("attendees").is_none());
        assert!(body.get("conferenceData").is_none());
        assert_eq!(
            body.get("summary").and_then(Value::as_str),
            Some("Weekly sync")
        );
        assert!(body.get("recurrence").is_some());
    }
}
