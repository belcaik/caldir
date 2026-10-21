use std::collections::HashMap;

use anyhow::{Context, Result};
use caldir_core::provider::ProviderStorage;
use caldir_core::rpc::ListEvents;
use caldir_core::{
    Availability, CalendarItem, Event, EventTime, EventUid, RecurrenceId, Status, XProperty,
};
use google_calendar::types::OrderBy;

use crate::app_config::AppConfigStore;
use crate::constants::{PROVIDER_EVENT_ID_PROPERTY, PROVIDER_NAME};
use crate::google_event::{FromGoogle, google_dt_to_event_time};
use crate::google_tasks::{self, task_to_calendar_item};
use crate::remote_config::{
    GoogleRemoteConfig, GoogleResourceKind, GoogleTaskListRemoteConfig, resolve_resource_kind,
};
use crate::session::SessionStore;

/// Resolves the remote's resource kind and branches once, at the top: a
/// task-list remote takes the Tasks path, a calendar remote takes the
/// Calendar path. The two never call both APIs and merge (spec §9).
pub async fn handle(cmd: ListEvents) -> Result<Vec<CalendarItem>> {
    match resolve_resource_kind(&cmd.remote)? {
        GoogleResourceKind::Calendar => handle_calendar(cmd).await,
        GoogleResourceKind::TaskList => handle_task_list(cmd).await,
    }
}

async fn handle_calendar(cmd: ListEvents) -> Result<Vec<CalendarItem>> {
    let config = GoogleRemoteConfig::try_from(&cmd.remote)?;
    let account_email = &config.google_account;
    let calendar_id = &config.google_calendar_id;

    let storage = ProviderStorage::for_provider(PROVIDER_NAME)?;
    let session_store = SessionStore::new(storage.clone());
    let app_config_store = AppConfigStore::new(storage);

    let session = session_store
        .load_valid(account_email, &app_config_store)
        .await?;
    let client = session_store.client(&session, &app_config_store)?;

    let google_events = client
        .events()
        .list_all(
            calendar_id,
            "",
            0,
            OrderBy::default(),
            &[],
            "", // search query
            &[],
            true, // show_deleted: include cancelled events so they surface as
            // STATUS:CANCELLED locally rather than disappearing silently.
            false,
            false,
            &cmd.to,
            &cmd.from,
            "",
            "",
        )
        .await
        .context("Failed to fetch events")?
        .body;

    Ok(process_google_events(google_events)?
        .into_iter()
        .map(CalendarItem::Event)
        .collect())
}

/// The Tasks half of the split (spec §9): never touches the Calendar API,
/// never returns anything but `CalendarItem::Todo`.
async fn handle_task_list(cmd: ListEvents) -> Result<Vec<CalendarItem>> {
    let config = GoogleTaskListRemoteConfig::try_from(&cmd.remote)?;
    let account_email = &config.google_account;
    let task_list_id = &config.google_resource_id;

    let storage = ProviderStorage::for_provider(PROVIDER_NAME)?;
    let session_store = SessionStore::new(storage.clone());
    let app_config_store = AppConfigStore::new(storage);

    let session = session_store
        .load_valid(account_email, &app_config_store)
        .await?;
    session.require_tasks_scope()?;

    let tasks = google_tasks::list_tasks(session.access_token(), task_list_id)
        .await
        .context("Failed to fetch Google Tasks")?;

    process_google_tasks(tasks)
}

/// Converts Google Tasks into `CalendarItem::Todo`s, dropping `deleted`
/// tasks (a remote delete) but keeping `hidden` ones (Google's own
/// completed-task UI hiding, not a delete — see `google_tasks::from_google`).
fn process_google_tasks(tasks: Vec<google_tasks::Task>) -> Result<Vec<CalendarItem>> {
    let mut items = Vec::with_capacity(tasks.len());
    for task in tasks {
        if let Some(item) = task_to_calendar_item(task)? {
            items.push(item);
        }
    }
    Ok(items)
}

/// Convert Google's raw events list into caldir Events.
///
/// Google returns cancelled instances of recurring events as bare tombstones:
/// status="cancelled" with no start/end, but with recurringEventId +
/// originalStartTime. We split these out and convert each into a cancelled
/// instance override Event so the recurrence engine can skip the corresponding
/// occurrence.
///
/// Standalone deleted events arrive as tombstones too (no recurringEventId);
/// those are dropped — there's nothing local to attach them to.
fn process_google_events(google_events: Vec<google_calendar::types::Event>) -> Result<Vec<Event>> {
    let mut master_info_by_google_id: HashMap<String, (String, String)> = HashMap::new();
    let mut cancellations: Vec<google_calendar::types::Event> = Vec::new();
    let mut events: Vec<Event> = Vec::with_capacity(google_events.len());

    for ge in google_events {
        let has_start = ge
            .start
            .as_ref()
            .is_some_and(|s| s.date_time.is_some() || s.date.is_some());

        if !has_start && ge.status == "cancelled" {
            if !ge.recurring_event_id.is_empty() {
                cancellations.push(ge);
            }
            continue;
        }

        if !ge.i_cal_uid.is_empty() {
            master_info_by_google_id
                .insert(ge.id.clone(), (ge.i_cal_uid.clone(), ge.summary.clone()));
        }
        events.push(Event::from_google(ge)?);
    }

    for ge in cancellations {
        if let Some(event) = cancellation_to_event(&ge, &master_info_by_google_id) {
            events.push(event);
        }
    }

    Ok(events)
}

fn cancellation_to_event(
    ge: &google_calendar::types::Event,
    master_info_by_google_id: &HashMap<String, (String, String)>,
) -> Option<Event> {
    let recurrence_id = google_dt_to_event_time(ge.original_start_time.as_ref())?;
    let (uid, summary) = master_info_by_google_id
        .get(&ge.recurring_event_id)
        .cloned()
        .unwrap_or_else(|| {
            (
                format!("{}@google.com", ge.recurring_event_id),
                String::new(),
            )
        });

    let start = recurrence_id.clone();
    let end = match &start {
        EventTime::Date(d) => EventTime::Date(*d),
        EventTime::DateTimeUtc(dt) => EventTime::DateTimeUtc(*dt),
        EventTime::DateTimeFloating(dt) => EventTime::DateTimeFloating(*dt),
        EventTime::DateTimeZoned { datetime, tzid } => EventTime::DateTimeZoned {
            datetime: *datetime,
            tzid: tzid.clone(),
        },
    };

    Some(Event {
        uid: EventUid::new(uid),
        summary: if summary.is_empty() {
            None
        } else {
            Some(summary)
        },
        description: None,
        location: None,
        start,
        end: Some(end),
        status: Status::Cancelled,
        availability: Availability::Busy,
        visibility: Default::default(),
        recurrence: None,
        recurrence_id: Some(RecurrenceId::from_event_time(recurrence_id)),
        last_modified: ge.updated,
        sequence: ge.sequence as i32,
        organizer: None,
        attendees: Vec::new(),
        reminders: Vec::new(),
        url: None,
        attachments: Vec::new(),
        x_properties: vec![XProperty::new(PROVIDER_EVENT_ID_PROPERTY, &ge.id)],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use google_calendar::types as g;

    fn empty_event() -> g::Event {
        serde_json::from_value(serde_json::json!({})).unwrap()
    }

    fn empty_dt() -> g::EventDateTime {
        serde_json::from_value(serde_json::json!({})).unwrap()
    }

    fn master(id: &str, uid: &str, summary: &str) -> g::Event {
        g::Event {
            id: id.into(),
            i_cal_uid: uid.into(),
            summary: summary.into(),
            status: "confirmed".into(),
            start: Some(g::EventDateTime {
                date_time: Some("2026-01-16T15:00:00Z".parse().unwrap()),
                time_zone: "Europe/Oslo".into(),
                ..empty_dt()
            }),
            end: Some(g::EventDateTime {
                date_time: Some("2026-01-16T15:45:00Z".parse().unwrap()),
                time_zone: "Europe/Oslo".into(),
                ..empty_dt()
            }),
            recurrence: vec!["RRULE:FREQ=WEEKLY;BYDAY=FR".into()],
            ..empty_event()
        }
    }

    fn cancelled_instance(id: &str, master_id: &str, original_start: &str) -> g::Event {
        g::Event {
            id: id.into(),
            status: "cancelled".into(),
            recurring_event_id: master_id.into(),
            original_start_time: Some(g::EventDateTime {
                date_time: Some(original_start.parse().unwrap()),
                time_zone: "Europe/Oslo".into(),
                ..empty_dt()
            }),
            ..empty_event()
        }
    }

    #[test]
    fn cancelled_recurring_instance_becomes_cancelled_override() {
        let result = process_google_events(vec![
            master("master_id", "uid@google.com", "Weekly retro"),
            cancelled_instance(
                "master_id_20260213T150000Z",
                "master_id",
                "2026-02-13T15:00:00Z",
            ),
        ])
        .unwrap();

        assert_eq!(result.len(), 2);
        let cancellation = result.iter().find(|e| e.recurrence_id.is_some()).unwrap();
        assert_eq!(cancellation.uid.as_str(), "uid@google.com");
        assert_eq!(cancellation.summary.as_deref(), Some("Weekly retro"));
        assert_eq!(cancellation.status, Status::Cancelled);
        let rid_event_time = cancellation.recurrence_id.as_ref().unwrap().as_event_time();
        assert!(matches!(
            rid_event_time,
            EventTime::DateTimeZoned { tzid, .. } if tzid == "Europe/Oslo"
        ));
    }

    #[test]
    fn cancellation_without_master_in_batch_falls_back_to_synthetic_uid() {
        // The master may sit outside the requested date range — we still want the
        // cancellation captured so the engine can skip the occurrence.
        let result = process_google_events(vec![cancelled_instance(
            "orphan_id_20260213T150000Z",
            "orphan_master",
            "2026-02-13T15:00:00Z",
        )])
        .unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].uid.as_str(), "orphan_master@google.com");
        assert_eq!(result[0].summary, None);
        assert_eq!(result[0].status, Status::Cancelled);
    }

    #[test]
    fn standalone_cancelled_tombstone_is_dropped() {
        // Status=cancelled, no recurringEventId — a deleted standalone event.
        // Nothing to attach it to locally; drop silently.
        let mut tombstone = empty_event();
        tombstone.id = "deleted_id".into();
        tombstone.status = "cancelled".into();

        let result = process_google_events(vec![tombstone]).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn cancellation_without_original_start_is_dropped() {
        // We can't attach it to a specific occurrence without originalStartTime.
        let mut bad = empty_event();
        bad.id = "x".into();
        bad.status = "cancelled".into();
        bad.recurring_event_id = "master".into();

        let result = process_google_events(vec![bad]).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn normal_events_pass_through() {
        let result =
            process_google_events(vec![master("m1", "uid1@google.com", "Weekly retro")]).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].uid.as_str(), "uid1@google.com");
        assert!(result[0].recurrence.is_some());
        assert_eq!(result[0].status, Status::Confirmed);
    }

    #[test]
    fn cancelled_event_with_full_data_passes_through_as_cancelled() {
        // With showDeleted=true, Google returns cancelled masters as full
        // events. They flow through Event::from_google and surface with
        // Status::Cancelled rather than being treated as remote deletions.
        let mut ge = master("m1", "uid1@google.com", "Weekly retro");
        ge.status = "cancelled".into();

        let result = process_google_events(vec![ge]).unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].uid.as_str(), "uid1@google.com");
        assert_eq!(result[0].status, Status::Cancelled);
    }

    /// GOOGLE-12 control: the refactor that lets `handle` return
    /// `Vec<CalendarItem>` is exactly when an event could start arriving as
    /// something else. Assert on the serialized bytes, not just the type
    /// (CROSS-6): a VEVENT never becomes a VTODO tag on the wire.
    #[test]
    fn the_event_path_still_yields_only_events() {
        let result = process_google_events(vec![master("m1", "uid1@google.com", "Weekly retro")])
            .unwrap()
            .into_iter()
            .map(CalendarItem::Event)
            .collect::<Vec<_>>();

        assert_eq!(result.len(), 1);
        assert!(
            result
                .iter()
                .all(|item| matches!(item, CalendarItem::Event(_)))
        );
        let CalendarItem::Event(ref event) = result[0] else {
            panic!("expected an Event");
        };
        let ics = event.to_ics_string();
        assert!(ics.contains("BEGIN:VEVENT"));
        assert!(!ics.contains("BEGIN:VTODO"));
    }

    fn minimal_task(id: &str, title: &str) -> google_tasks::Task {
        // `google_tasks::Task` fields are all `pub(crate)`-visible from here;
        // built directly rather than through JSON since this test exercises
        // `process_google_tasks`, not deserialization.
        serde_json::from_value(serde_json::json!({
            "id": id,
            "title": title,
        }))
        .unwrap()
    }

    /// Mirrors `standalone_cancelled_tombstone_is_dropped` for the task path.
    /// `hidden` is the trap: Google hides *completed* tasks in its own UI, so
    /// treating `hidden` as a delete would remove every completed task from
    /// the user's directory on the next sync.
    #[test]
    fn a_deleted_google_task_is_dropped_from_the_listing() {
        let mut deleted = minimal_task("t1", "Buy milk");
        deleted.deleted = true;

        let mut hidden_completed = minimal_task("t2", "File taxes");
        hidden_completed.status = "completed".into();
        hidden_completed.hidden = true;

        let result = process_google_tasks(vec![deleted, hidden_completed]).unwrap();

        assert_eq!(result.len(), 1);
        assert!(
            matches!(&result[0], CalendarItem::Todo(todo) if todo.summary.as_deref() == Some("File taxes"))
        );
    }
}
