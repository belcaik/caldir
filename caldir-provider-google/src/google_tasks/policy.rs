//! §12/§14 unsupported-field policy: every `Todo` property Google Tasks
//! cannot carry gets exactly one class (faithful / lossy / preserved only
//! locally / rejected), proved by one test per row below.
//!
//! The two functions here are the only NEW behaviour this classification
//! needs. Every "preserved only locally" row is proved against
//! `crate::commands::create_event::merge_canonical_task_response` — the seam
//! that decides whether a field a provider can't send actually survives a
//! push, or is silently destroyed when core overwrites the local `.ics` with
//! the provider's response (`caldir-core::connection::push_outgoing_changes`).

use caldir_core::{Todo, TodoStatus};

/// §16: a recurring VTODO (`RRULE`/`EXDATE`/`RDATE`) cannot be represented in
/// Google Tasks — there is no recurrence concept in the Tasks API. Refused by
/// name rather than flattened to a single task, which would silently drop
/// the recurrence with no error anywhere.
#[derive(Debug)]
pub(crate) struct RecurringTaskRejected;

impl std::fmt::Display for RecurringTaskRejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Google Tasks cannot represent a recurring VTODO (RRULE); push refused"
        )
    }
}

impl std::error::Error for RecurringTaskRejected {}

/// Refuses a recurring task before any network call, so the local file is
/// never touched by a push that can't represent it (§16).
pub(crate) fn reject_recurring(todo: &Todo) -> Result<(), RecurringTaskRejected> {
    if todo.recurrence.is_some() {
        Err(RecurringTaskRejected)
    } else {
        Ok(())
    }
}

/// A user-authored `STATUS` Google Tasks has no equivalent for: sent as
/// `needsAction` (never `completed` — that would be an invented equivalent),
/// the local value survives via `merge_canonical_task_response`'s carve-out,
/// but the change itself never reaches Google. `None` when the status is one
/// Google Tasks can actually represent.
pub(crate) fn semantic_change_warning(todo: &Todo) -> Option<String> {
    match todo.status {
        TodoStatus::InProcess | TodoStatus::Cancelled => Some(format!(
            "caldir-provider-google: warning: task {:?} has STATUS:{}, which Google Tasks \
             cannot represent; sent as needsAction, the local value is preserved but this \
             change will not reach Google",
            todo.summary.as_deref().unwrap_or(todo.uid.as_str()),
            todo.status.as_ics_str()
        )),
        TodoStatus::NeedsAction | TodoStatus::Completed => None,
    }
}

#[cfg(test)]
mod tests {
    use caldir_core::{
        Attachment, Attendee, CalendarItem, EventTime, Organizer, Recurrence, Reminder, TodoStatus,
        XProperty,
    };
    use chrono::Utc;

    use super::*;
    use crate::commands::create_event::merge_canonical_task_response;
    use crate::google_tasks::{Task, ToGoogle, ToGooglePatch, task_to_calendar_item};

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

    fn to_todo(task: Task) -> Todo {
        let CalendarItem::Todo(todo) = task_to_calendar_item(task).unwrap().unwrap() else {
            panic!("task_to_calendar_item always returns CalendarItem::Todo");
        };
        todo
    }

    /// `SUMMARY` <-> `title`, `DESCRIPTION` <-> `notes`: both directions,
    /// verbatim.
    #[test]
    fn summary_and_notes_round_trip_faithfully() {
        let mut todo = Todo::new("Buy milk");
        todo.description = Some("2%, not skim".into());

        let request = todo.to_google(&Utc);
        assert_eq!(request.title, "Buy milk");
        assert_eq!(request.notes, "2%, not skim");

        let mut task = minimal_task("google-id", &request.title);
        task.notes = request.notes.clone();
        let round_tripped = to_todo(task);

        assert_eq!(round_tripped.summary.as_deref(), Some("Buy milk"));
        assert_eq!(round_tripped.description.as_deref(), Some("2%, not skim"));
    }

    /// `STATUS:COMPLETED`/`NEEDS-ACTION` <-> `completed`/`needsAction`: both
    /// directions, verbatim.
    #[test]
    fn completed_and_needs_action_status_round_trip_faithfully() {
        for (status, wire) in [
            (TodoStatus::Completed, "completed"),
            (TodoStatus::NeedsAction, "needsAction"),
        ] {
            let mut todo = Todo::new("Buy milk");
            todo.status = status;

            let request = todo.to_google(&Utc);
            assert_eq!(request.status, wire);

            let mut task = minimal_task("google-id", &request.title);
            task.status = request.status.clone();
            let round_tripped = to_todo(task);

            assert_eq!(round_tripped.status, status);
        }
    }

    /// `COMPLETED` is lossy, not faithful: Google restates the instant at
    /// millisecond precision, so a sub-millisecond local value does not
    /// survive the round trip.
    #[test]
    fn the_completed_timestamp_round_trips_within_googles_precision() {
        let mut todo = Todo::new("Buy milk");
        todo.status = TodoStatus::Completed;
        let original: chrono::DateTime<Utc> = "2026-06-10T19:17:57.123456789Z".parse().unwrap();
        todo.completed = Some(EventTime::DateTimeUtc(original));

        let request = todo.to_google(&Utc);
        let mut task = minimal_task("google-id", &request.title);
        task.status = request.status.clone();
        task.completed = request.completed.clone();
        let round_tripped = to_todo(task);

        // Millisecond precision survives...
        assert_eq!(
            round_tripped.completed,
            Some(EventTime::DateTimeUtc(
                "2026-06-10T19:17:57.123Z".parse().unwrap()
            ))
        );
        // ...but the original nanosecond-precision instant does not: this is
        // the "lossy" class, not "faithful".
        assert_ne!(
            round_tripped.completed,
            Some(EventTime::DateTimeUtc(original))
        );
    }

    /// `DUE` with a time component is lossy, not faithful: it comes back
    /// date-only (`P5-05` owns the conversion; this only asserts the class).
    #[test]
    fn a_timestamped_due_is_classified_lossy_not_faithful() {
        let mut todo = Todo::new("Submit report");
        let original = EventTime::DateTimeUtc("2026-09-02T17:30:00Z".parse().unwrap());
        todo.due = Some(original.clone());

        let request = todo.to_google(&Utc);
        let response = minimal_task("google-id", &request.title);
        let mut response = response;
        response.due = request.due.clone();

        let merged = merge_canonical_task_response(todo, response).unwrap();

        assert_ne!(
            merged.due,
            Some(original),
            "a timestamped DUE must not round-trip faithfully"
        );
        assert!(matches!(merged.due, Some(EventTime::Date(_))));
    }

    /// `STATUS:IN-PROCESS` has no Google Tasks equivalent: sent as
    /// `needsAction`, but the local value must survive the push rather than
    /// collapsing to whatever Google echoes back.
    #[test]
    fn in_process_is_sent_as_needs_action_and_survives_locally() {
        let mut local = Todo::new("Buy milk");
        local.status = TodoStatus::InProcess;

        let request = local.to_google(&Utc);
        assert_eq!(request.status, "needsAction");
        let patch = local.to_google_patch(&Utc);
        assert_eq!(patch.status, "needsAction");

        let response = minimal_task("google-id", &request.title);
        let merged = merge_canonical_task_response(local, response).unwrap();

        assert_eq!(merged.status, TodoStatus::InProcess);
    }

    /// `STATUS:CANCELLED` has no Google Tasks equivalent either: sent as
    /// `needsAction`, and — the single most tempting invented equivalent in
    /// this table — never as `completed`.
    #[test]
    fn cancelled_is_sent_as_needs_action_and_never_as_completed() {
        let mut local = Todo::new("Buy milk");
        local.status = TodoStatus::Cancelled;

        let request = local.to_google(&Utc);
        assert_eq!(request.status, "needsAction");
        assert_ne!(request.status, "completed");

        let response = minimal_task("google-id", &request.title);
        let merged = merge_canonical_task_response(local, response).unwrap();

        assert_eq!(merged.status, TodoStatus::Cancelled);
        assert_ne!(merged.status, TodoStatus::Completed);
    }

    /// `PERCENT-COMPLETE` survives a push and is never sent to Google, and
    /// never synthesized from `STATUS` on the way back (R68).
    #[test]
    fn percent_complete_survives_the_push_and_is_never_sent() {
        let mut local = Todo::new("Buy milk");
        local.percent_complete = Some(42);

        let request = local.to_google(&Utc);
        let json = serde_json::to_value(&request).unwrap();
        assert!(json.get("percentComplete").is_none());
        assert!(json.get("percent_complete").is_none());

        let response = minimal_task("google-id", &request.title);
        let merged = merge_canonical_task_response(local, response).unwrap();

        assert_eq!(merged.percent_complete, Some(42));
    }

    /// `PRIORITY:1` is on all 57 production tasks. It survives a push and is
    /// never encoded into `title` or `notes`.
    #[test]
    fn priority_survives_the_push_and_never_enters_title_or_notes() {
        let mut local = Todo::new("Buy milk");
        local.priority = Some(1);

        let request = local.to_google(&Utc);
        assert_eq!(request.title, "Buy milk");
        assert_eq!(request.notes, "");
        let json = serde_json::to_value(&request).unwrap();
        assert!(json.get("priority").is_none());

        let response = minimal_task("google-id", &request.title);
        let merged = merge_canonical_task_response(local, response).unwrap();

        assert_eq!(merged.priority, Some(1));
    }

    /// `URL` is on all 57 production tasks. It survives a push and is never
    /// appended to `notes` — that would fabricate data the next pull reads
    /// back as user-authored `DESCRIPTION`.
    #[test]
    fn url_survives_the_push_and_is_never_appended_to_notes() {
        let mut local = Todo::new("Buy milk");
        local.url = Some("https://example.edu/rich".into());
        local.description = Some("2%, not skim".into());

        let request = local.to_google(&Utc);
        assert_eq!(request.notes, "2%, not skim");
        assert!(!request.notes.contains("https://"));
        let json = serde_json::to_value(&request).unwrap();
        assert!(json.get("url").is_none());

        let response = minimal_task("google-id", &request.title);
        let merged = merge_canonical_task_response(local, response).unwrap();

        assert_eq!(merged.url.as_deref(), Some("https://example.edu/rich"));
    }

    /// `DTSTART` survives a push and is never folded into `due`.
    #[test]
    fn dtstart_survives_the_push_and_is_never_folded_into_due() {
        let mut local = Todo::new("Buy milk");
        local.start = Some(EventTime::DateTimeUtc(
            "2026-08-10T09:00:00Z".parse().unwrap(),
        ));

        let request = local.to_google(&Utc);
        assert!(request.due.is_none());

        let response = minimal_task("google-id", &request.title);
        let merged = merge_canonical_task_response(local.clone(), response).unwrap();

        assert_eq!(merged.start, local.start);
        assert_eq!(merged.due, None);
    }

    /// `ATTENDEE`/`ORGANIZER` survive a push and are never sent to Google
    /// Tasks (no such concept in the Tasks API).
    #[test]
    fn attendees_and_organizer_survive_the_push_and_are_never_sent() {
        let mut local = Todo::new("Group project");
        local.organizer = Some(Organizer::new("teacher@example.edu"));
        local.attendees = vec![Attendee::new("student@example.edu")];

        let request = local.to_google(&Utc);
        let json = serde_json::to_value(&request).unwrap();
        assert!(json.get("organizer").is_none());
        assert!(json.get("attendees").is_none());

        let response = minimal_task("google-id", &request.title);
        let merged = merge_canonical_task_response(local.clone(), response).unwrap();

        assert_eq!(merged.organizer, local.organizer);
        assert_eq!(merged.attendees, local.attendees);
    }

    /// `ATTACH` survives a push and is never linkified into `notes`.
    #[test]
    fn attachments_survive_the_push_and_are_never_linkified_into_notes() {
        let mut local = Todo::new("Buy milk");
        local.attachments = vec![Attachment::new("https://example.edu/rich/brief.pdf")];
        local.description = Some("2%, not skim".into());

        let request = local.to_google(&Utc);
        assert_eq!(request.notes, "2%, not skim");
        assert!(!request.notes.contains(".pdf"));

        let response = minimal_task("google-id", &request.title);
        let merged = merge_canonical_task_response(local.clone(), response).unwrap();

        assert_eq!(merged.attachments, local.attachments);
    }

    /// `VALARM` survives a push and no Google reminder/notification is
    /// invented in its place.
    #[test]
    fn alarms_survive_the_push_and_no_google_reminder_is_invented() {
        let mut local = Todo::new("Buy milk");
        local.reminders = vec![Reminder::from_minutes(30)];

        let request = local.to_google(&Utc);
        let json = serde_json::to_value(&request).unwrap();
        assert!(json.get("reminders").is_none());
        assert!(json.get("notifications").is_none());

        let response = minimal_task("google-id", &request.title);
        let merged = merge_canonical_task_response(local.clone(), response).unwrap();

        assert_eq!(merged.reminders, local.reminders);
    }

    /// Arbitrary `X-` properties survive a push (except
    /// `X-GOOGLE-TASK-ID`, which this provider owns and rewrites —
    /// see `google_tasks::tests::the_google_task_id_is_provider_metadata_and_never_the_uid`).
    #[test]
    fn unknown_x_properties_survive_the_push() {
        let mut local = Todo::new("Buy milk");
        local
            .x_properties
            .push(XProperty::new("X-CALDIR-KEEP", "kept"));

        let request = local.to_google(&Utc);
        let json = serde_json::to_value(&request).unwrap();
        assert!(json.get("X-CALDIR-KEEP").is_none());

        let response = minimal_task("google-id", &request.title);
        let merged = merge_canonical_task_response(local, response).unwrap();

        assert!(
            merged
                .x_properties
                .contains(&XProperty::new("X-CALDIR-KEEP", "kept"))
        );
    }

    /// `RRULE`/`EXDATE`/`RDATE`: the one *rejected* row. A recurring VTODO
    /// pushed as a single Google task would silently lose its recurrence
    /// with no error anywhere; refuse it by name instead.
    #[test]
    fn a_recurring_task_is_refused_with_a_named_semantic_limitation() {
        let mut todo = Todo::new("Weekly review");
        todo.recurrence = Some(Recurrence::new("FREQ=WEEKLY"));

        let error = reject_recurring(&todo).unwrap_err();

        assert!(error.to_string().contains("RRULE"));
    }

    /// A non-recurring task is never rejected.
    #[test]
    fn a_non_recurring_task_is_not_rejected() {
        let todo = Todo::new("Buy milk");
        assert!(reject_recurring(&todo).is_ok());
    }

    /// §14's fourth clause: a user-authored semantic change that cannot be
    /// represented produces a warning. "Preserved locally" is invisible to
    /// the user otherwise; the warning is the only place they learn their
    /// `IN-PROCESS` edit did not reach their phone.
    #[test]
    fn a_warning_is_emitted_when_a_user_authored_change_cannot_be_represented() {
        let mut todo = Todo::new("Buy milk");

        todo.status = TodoStatus::InProcess;
        let warning = semantic_change_warning(&todo).expect("IN-PROCESS cannot be represented");
        assert!(warning.contains("IN-PROCESS"));

        todo.status = TodoStatus::Cancelled;
        let warning = semantic_change_warning(&todo).expect("CANCELLED cannot be represented");
        assert!(warning.contains("CANCELLED"));

        todo.status = TodoStatus::NeedsAction;
        assert!(semantic_change_warning(&todo).is_none());

        todo.status = TodoStatus::Completed;
        assert!(semantic_change_warning(&todo).is_none());
    }

    /// `caldir_core::test_utils::VTODO_OPEN_PRODUCTION_ICS` (`P1-05`), copied
    /// byte-identical: cannot be imported directly, `test_utils` is a
    /// private `#[cfg(test)]` module in `caldir-core`. Every production
    /// `deadlines/*.ics` file carries this exact shape: `UID`, `DTSTAMP`,
    /// `DUE`, `PRIORITY`, `SUMMARY`, `URL`. §14's regression: a push must
    /// leave `PRIORITY`/`URL` intact locally while sending neither.
    const P1_05_OPEN_TASK_ICS: &str = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//u_crawler//calendar-sync//EN\r\n\
BEGIN:VTODO\r\n\
UID:u_crawler-todo-9001571@u-crawler.local\r\n\
DTSTAMP:20260610T191757Z\r\n\
DUE:20260902T035959Z\r\n\
PRIORITY:1\r\n\
SUMMARY:Semana 11: Sumativa 5: Solemne 2\r\n\
URL:https://lms.example.edu/courses/900223/assignments/9001571\r\n\
END:VTODO\r\n\
END:VCALENDAR\r\n";

    #[test]
    fn the_ucrawler_fixture_survives_a_push_with_priority_and_url_intact() {
        let CalendarItem::Todo(local) =
            CalendarItem::from_single_ics_str(P1_05_OPEN_TASK_ICS).unwrap()
        else {
            panic!("fixture parses as a Todo");
        };
        assert_eq!(local.priority, Some(1));
        assert!(local.url.is_some());

        let request = local.to_google(&Utc);
        let json = serde_json::to_value(&request).unwrap();
        assert!(json.get("priority").is_none());
        assert!(json.get("url").is_none());

        let response = minimal_task("google-id", &request.title);
        let merged = merge_canonical_task_response(local, response).unwrap();

        assert_eq!(merged.priority, Some(1));
        assert_eq!(
            merged.url.as_deref(),
            Some("https://lms.example.edu/courses/900223/assignments/9001571")
        );
    }

    /// Exhaustive destructuring, no `..`: adding a field to
    /// `caldir_core::Todo` fails to compile here, forcing this table to be
    /// revisited rather than silently classifying a new field as "dropped"
    /// by omission. This is the table's real long-term failure mode, and no
    /// per-row test above catches it.
    #[test]
    fn every_todo_field_appears_in_the_classification() {
        let todo = Todo::new("Buy milk");
        let Todo {
            uid: _,              // identity, out of this table
            summary: _,          // faithful
            description: _,      // faithful
            location: _,         // preserved only locally (not sent; no Tasks location concept)
            start: _,            // preserved only locally (DTSTART)
            due: _,              // lossy (date-only) / faithful for date-only input
            status: _,           // faithful (NEEDS-ACTION/COMPLETED) / lossy (IN-PROCESS/CANCELLED)
            completed: _,        // lossy (millisecond precision)
            percent_complete: _, // preserved only locally
            priority: _,         // preserved only locally
            url: _,              // preserved only locally
            recurrence: _,       // rejected
            recurrence_id: _,    // out of this table (VTODO masters only; no per-instance concept)
            organizer: _,        // preserved only locally
            attendees: _,        // preserved only locally
            reminders: _,        // preserved only locally (VALARM)
            attachments: _,      // preserved only locally (ATTACH)
            x_properties: _,     // preserved only locally (except X-GOOGLE-TASK-ID)
            created: _,          // preserved only locally, not classified in the ticket's table
            last_modified: _,    // preserved only locally, not classified in the ticket's table
            sequence: _,         // preserved only locally, not classified in the ticket's table
        } = todo;
    }
}
