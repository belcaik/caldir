use anyhow::{Context, Result, anyhow};
use caldir_core::provider::{ProviderStorage, classified_error};
use caldir_core::rpc::{CreateItem, ErrorCode};
use caldir_core::{CalendarItem, Event, EventTime, RemoteConfigParams, Todo, TodoStatus};
use google_calendar::types::SendUpdates;

use crate::app_config::AppConfigStore;
use crate::commands::invite::patch_invite_status;
use crate::constants::{PROVIDER_EVENT_ID_PROPERTY, PROVIDER_NAME, PROVIDER_TASK_ID_PROPERTY};
use crate::google_event::{FromGoogle, ToGoogle};
use crate::google_tasks::{self, ToGoogle as ToGoogleTask};
use crate::remote_config::{GoogleRemoteConfig, GoogleTaskListRemoteConfig};
use crate::session::SessionStore;

pub async fn handle(cmd: CreateItem) -> Result<CalendarItem> {
    match cmd.event {
        CalendarItem::Event(event) => {
            Ok(CalendarItem::Event(handle_event(cmd.remote, event).await?))
        }
        CalendarItem::Todo(todo) => Ok(CalendarItem::Todo(handle_todo(cmd.remote, todo).await?)),
        _ => Err(anyhow!(
            "caldir-provider-google does not support this item kind"
        )),
    }
}

/// The Tasks half of the create split (§9): never touches the Calendar API,
/// never builds a `google_calendar::types::Event`.
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

    let (_, body) = task_insert_request(task_list_id, &todo);

    let response = google_tasks::insert_task(session.access_token(), task_list_id, &body)
        .await
        .with_context(|| format!("Failed to create task: {}", body.title))?;

    merge_canonical_task_response(todo, response)
}

/// Pure request-building half of a task create: the Tasks list-scoped
/// endpoint and the insert body, with no client-chosen `id` (same
/// "Invalid resource id value" trap the event path's insert works around).
/// Kept separate from `insert_task`'s network call so routing to the Tasks
/// endpoint, not the Calendar one, is directly testable without an HTTP
/// mock this crate does not have.
fn task_insert_request(
    task_list_id: &str,
    todo: &Todo,
) -> (String, google_tasks::GoogleTaskInsert) {
    let mut body = todo.to_google(&chrono::Local);
    body.id = None;
    (google_tasks::tasks_url(task_list_id), body)
}

/// The response seam, shared by create and update: returns the canonical
/// item Google's response describes, never the request `todo` unchanged
/// (§7, §15) — but keeps the local `uid` and every field Google cannot
/// represent, since the caller overwrites the local `.ics` file with exactly
/// what this returns. `commands::update_event` reuses this rather than
/// writing a second merge that can drift from it.
pub(crate) fn merge_canonical_task_response(
    local: Todo,
    response: google_tasks::Task,
) -> Result<Todo> {
    let canonical = google_tasks::task_to_calendar_item(response)?
        .ok_or_else(|| anyhow!("Google task create response reported the task as deleted"))?;

    let CalendarItem::Todo(canonical) = canonical else {
        unreachable!("task_to_calendar_item always returns CalendarItem::Todo");
    };

    let local_status = local.status;
    let mut merged = local;
    merged.summary = canonical.summary;
    merged.description = canonical.description;
    merged.status = canonical.status;
    merged.completed = canonical.completed;
    merged.due = canonical.due;
    merged
        .x_properties
        .retain(|p| p.name != PROVIDER_TASK_ID_PROPERTY);
    merged.x_properties.extend(canonical.x_properties);

    // Google Tasks has no IN-PROCESS/CANCELLED equivalent, so
    // `to_google`/`to_google_patch` already downgrade both to `needsAction`
    // on the wire. `canonical.status` here is only Google echoing that
    // downgrade back, never new information — overwriting `merged.status`
    // with it would silently collapse the "preserved only locally" row to
    // NeedsAction on every push.
    if matches!(local_status, TodoStatus::InProcess | TodoStatus::Cancelled) {
        merged.status = local_status;
    }

    Ok(merged)
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
    let client = session_store.client(&session, &app_config_store)?;

    // Recurring instance override:
    // Shares the master's iCalUID, so creating via events().insert() trips Google's "duplicate identifier" check.
    // Google's data model treats an override as a modification of an existing auto-expanded instance.
    // PUT the synthetic instance id `{master_id}_{rid}` instead.
    if let Some(rid) = event.recurrence_id.as_ref() {
        let instance_id = override_instance_id(&event, rid.as_event_time())?;

        // If it's just an RSVP status update, use PATCH instead of PUT:
        if event.is_invite_for(account_email) {
            let google_event =
                patch_invite_status(&session, calendar_id, &instance_id, &event, account_email)
                    .await?;

            return Event::from_google(google_event);
        } else {
            let mut google_event = event.to_google();

            google_event.id = instance_id.clone();

            // Overrides never carry their own RRULE in Google's model.
            google_event.recurrence = Vec::new();

            let response = client
                .events()
                .update(
                    calendar_id,
                    &instance_id,
                    0,
                    0,
                    false,
                    SendUpdates::All,
                    false,
                    &google_event,
                )
                .await
                .with_context(|| {
                    format!(
                        "Failed to update recurring instance: {}",
                        google_event.summary
                    )
                })?;

            return Event::from_google(response.body);
        }
    }

    // Let google change the ID
    // (Otherwise we'll get "Invalid resource id value")
    let mut google_event = event.to_google();
    google_event.id = String::new();

    let response = match client
        .events()
        .insert(
            calendar_id,
            1,
            0,
            false,
            SendUpdates::All,
            false,
            &google_event,
        )
        .await
    {
        Ok(response) => Ok(response),
        Err(error)
            if google_event.conference_data.is_some() && is_conference_data_error(&error) =>
        {
            eprintln!(
                "caldir-provider-google: warning: Google rejected conference data; \
                 retrying without it: {error}"
            );
            google_event.conference_data = None;

            client
                .events()
                .insert(
                    calendar_id,
                    1,
                    0,
                    false,
                    SendUpdates::All,
                    false,
                    &google_event,
                )
                .await
        }
        Err(error) => Err(error),
    }
    .with_context(|| format!("Failed to create event: {}", google_event.summary))?;

    let created_event = Event::from_google(response.body)?;

    Ok(created_event)
}

fn is_conference_data_error(error: &google_calendar::ClientError) -> bool {
    match error {
        google_calendar::ClientError::HttpError { error, .. } => {
            error.to_ascii_lowercase().contains("conference")
        }
        _ => false,
    }
}

/// Google API id of a recurring-instance override: `{master_id}_{instance suffix}`.
/// A downloaded override already carries its own instance id — use it as-is.
fn override_instance_id(event: &Event, rid: &EventTime) -> Result<String> {
    let provider_id = event
        .x_property(PROVIDER_EVENT_ID_PROPERTY)
        .ok_or_else(|| {
            anyhow!(
                "Cannot create recurring instance override without master's \
             {PROVIDER_EVENT_ID_PROPERTY}"
            )
        })?;

    let suffix = google_instance_suffix(rid);

    if provider_id.ends_with(&format!("_{suffix}")) {
        return Ok(provider_id.to_string());
    }

    Ok(format!("{provider_id}_{suffix}"))
}

/// Format a `recurrence_id` as the suffix Google appends to a recurring
/// event's id to identify a single instance:
/// - all-day:    `YYYYMMDD`
/// - timed:      `YYYYMMDDTHHMMSSZ` (UTC)
///
/// Zoned instances get their wallclock resolved to a UTC instant so the
/// suffix matches what Google's API auto-expansion uses.
fn google_instance_suffix(rid: &EventTime) -> String {
    match rid {
        EventTime::Date(d) => d.format("%Y%m%d").to_string(),
        EventTime::DateTimeUtc(dt) => dt.format("%Y%m%dT%H%M%SZ").to_string(),
        EventTime::DateTimeFloating(dt) => dt.format("%Y%m%dT%H%M%SZ").to_string(),
        EventTime::DateTimeZoned { datetime, tzid } => {
            let utc = if let Ok(tz) = tzid.parse::<chrono_tz::Tz>() {
                datetime
                    .and_local_timezone(tz)
                    .single()
                    .map(|zoned| zoned.with_timezone(&chrono::Utc))
                    .unwrap_or_else(|| datetime.and_utc())
            } else {
                datetime.and_utc()
            };
            utc.format("%Y%m%dT%H%M%SZ").to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caldir_core::{RecurrenceId, XProperty};
    use chrono::{TimeZone, Utc};
    use google_calendar::{ClientError, HeaderMap, StatusCode};

    #[test]
    fn synthesized_override_gets_the_suffix_appended_to_the_master_id() {
        // An override created locally inherits the MASTER's X-GOOGLE-EVENT-ID.
        let rid = EventTime::DateTimeUtc(Utc.with_ymd_and_hms(2026, 8, 14, 17, 0, 0).unwrap());
        let mut event = Event::new("Weekly sync", rid.clone());
        event.recurrence_id = Some(RecurrenceId::from_event_time(rid.clone()));
        event
            .x_properties
            .push(XProperty::new(PROVIDER_EVENT_ID_PROPERTY, "master123"));

        assert_eq!(
            override_instance_id(&event, &rid).unwrap(),
            "master123_20260814T170000Z"
        );
    }

    #[test]
    fn recreating_a_downloaded_override_must_not_double_its_google_id() {
        // When the organizer moves one occurrence of a recurring event,
        // Google stores the modified occurrence under its own event id:
        // "{master_id}_{original start as UTC}". Here the 17:00 occurrence
        // of "master123" was moved to 18:00:
        let downloaded: google_calendar::types::Event = serde_json::from_value(serde_json::json!({
            "id": "master123_20260814T170000Z",
            "iCalUID": "master123@google.com",
            "summary": "Weekly sync",
            "status": "confirmed",
            "recurringEventId": "master123",
            "originalStartTime": { "dateTime": "2026-08-14T17:00:00Z" },
            "start": { "dateTime": "2026-08-14T18:00:00Z" },
            "end": { "dateTime": "2026-08-14T19:00:00Z" },
        }))
        .unwrap();

        // Pulling it stores that id — suffix included — in X-GOOGLE-EVENT-ID:
        let event = Event::from_google(downloaded).unwrap();
        assert_eq!(
            event.x_property(PROVIDER_EVENT_ID_PROPERTY).unwrap(),
            "master123_20260814T170000Z"
        );

        // If sync state is lost, the local file is mistaken for a new event
        // and pushed through create_event (rencal#99). The PUT must target
        // the id Google already knows — a doubled suffix 404s:
        let rid = event.recurrence_id.clone().unwrap();
        assert_eq!(
            override_instance_id(&event, rid.as_event_time()).unwrap(),
            "master123_20260814T170000Z"
        );
    }

    fn http_error(message: &str) -> ClientError {
        ClientError::HttpError {
            status: StatusCode::BAD_REQUEST,
            headers: HeaderMap::new(),
            error: message.to_string(),
        }
    }

    #[test]
    fn identifies_conference_data_errors_for_insert_retry() {
        let error = http_error(r#"{"error":{"message":"Invalid conferenceData value"}}"#);

        assert!(is_conference_data_error(&error));
    }

    #[test]
    fn does_not_retry_unrelated_insert_errors() {
        let error = http_error(r#"{"error":{"message":"Invalid attendee email"}}"#);

        assert!(!is_conference_data_error(&error));
    }

    #[test]
    fn a_task_create_targets_the_tasks_endpoint_not_the_calendar_one() {
        let mut todo = Todo::new("Buy milk");
        todo.description = Some("2%, not skim".into());

        let (url, body) = task_insert_request("task-list-123", &todo);

        assert_eq!(
            url,
            "https://tasks.googleapis.com/tasks/v1/lists/task-list-123/tasks"
        );
        assert!(!url.contains("calendar"));

        // CROSS-6, on the serialized bytes: a Todo's insert body carries none
        // of the calendar-only shape.
        let json = serde_json::to_value(&body).unwrap();
        for calendar_only_key in ["summary", "start", "end", "attendees", "recurrence"] {
            assert!(
                json.get(calendar_only_key).is_none(),
                "task insert body must not carry {calendar_only_key}, got {json}"
            );
        }
        assert_eq!(json.get("title").and_then(|v| v.as_str()), Some("Buy milk"));
    }

    #[test]
    fn the_created_task_carries_googles_generated_id_back_to_core() {
        let mut local = Todo::new("Buy milk");
        local.uid = caldir_core::EventUid::new("local-uid-123");

        let response = google_tasks::Task {
            id: "google-task-abc".into(),
            title: "Buy milk".into(),
            notes: String::new(),
            status: "needsAction".into(),
            due: None,
            completed: None,
            deleted: false,
            hidden: false,
        };

        let returned = merge_canonical_task_response(local.clone(), response).unwrap();

        assert_eq!(returned.uid, local.uid);
        assert!(returned.x_properties.contains(&XProperty::new(
            PROVIDER_TASK_ID_PROPERTY,
            "google-task-abc"
        )));
    }
}
