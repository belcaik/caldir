//! Hand-rolled Google Tasks REST client (P4-01 spike decision: reqwest, no
//! new crate). Every Tasks-specific type stays private to this module —
//! nothing here may cross into caldir-core or the shared RPC (P4-01
//! Containment).

mod client;
mod due;
mod from_google;
pub(crate) mod policy;
mod to_google;

pub use client::{
    Task, delete_task, insert_task, list_task_lists, list_tasks, patch_task, task_url, tasks_url,
};
pub use from_google::task_to_calendar_item;
pub use to_google::{GoogleTaskInsert, GoogleTaskPatch, ToGoogle, ToGooglePatch};

/// `Todo` has no `x_property()` accessor (that's `Event`-only) — this reads
/// `x_properties` directly rather than adding one to `caldir-core`.
pub(crate) fn todo_x_property<'a>(todo: &'a caldir_core::Todo, name: &str) -> Option<&'a str> {
    todo.x_properties
        .iter()
        .find(|p| p.name == name)
        .map(|p| p.value.as_str())
}

/// P5-04: the lock over P5-01..03's three seams (create's response merge,
/// update's PATCH target, delete's DELETE target). **Tests 1-3 below are
/// necessary conditions, not the proof** — each checks one seam in
/// isolation. The proof that `create -> sync -> sync` cannot duplicate a
/// remote task is `live::google_tasks::create_sync_sync_yields_exactly_one_remote_task`,
/// which is the only test that runs the sequence against a real service; no
/// pure function in this crate observes a second network call deciding a
/// task is new (see that module's doc comment).
#[cfg(test)]
mod tests {
    use caldir_core::{CalendarItem, Todo, XProperty};

    use super::client::Task;
    use super::*;
    use crate::commands::create_event::merge_canonical_task_response;
    use crate::commands::delete_event::task_delete_request;
    use crate::commands::update_event::task_patch_request;
    use crate::constants::PROVIDER_TASK_ID_PROPERTY;

    fn task_response(id: &str, title: &str) -> Task {
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

    /// Right proof (per the ticket): a "just use the id as the UID"
    /// implementation passes any test that reuses the same value for `uid`
    /// and the stored Google id. This uses two distinct strings and asserts
    /// they never collapse into one.
    #[test]
    fn the_google_task_id_is_provider_metadata_and_never_the_uid() {
        let local = Todo::new("Buy milk");
        let local_uid = local.uid.clone();

        let response = task_response("google-id-123", "Buy milk");
        let merged = merge_canonical_task_response(local, response).unwrap();

        assert_eq!(merged.uid, local_uid);

        let stored_ids: Vec<&str> = merged
            .x_properties
            .iter()
            .filter(|p| p.name == PROVIDER_TASK_ID_PROPERTY)
            .map(|p| p.value.as_str())
            .collect();
        assert_eq!(stored_ids, vec!["google-id-123"]);
        assert_ne!(merged.uid.as_str(), "google-id-123");
    }

    /// Right proof (per the ticket): `merge_canonical_task_response`'s
    /// `x_properties` union runs on every cycle, create and update alike. A
    /// naive union (`extend` without first `retain`-ing the old value out)
    /// duplicates `X-GOOGLE-TASK-ID` on the second pass — churn dressed up
    /// as an identity bug, since a doubled property never compares equal to
    /// the single-property base again.
    #[test]
    fn the_minted_id_appears_exactly_once_after_a_second_cycle() {
        let local = Todo::new("Buy milk");

        let canonical_1 =
            merge_canonical_task_response(local, task_response("id-1", "Buy milk")).unwrap();
        let first_ids: Vec<&str> = canonical_1
            .x_properties
            .iter()
            .filter(|p| p.name == PROVIDER_TASK_ID_PROPERTY)
            .map(|p| p.value.as_str())
            .collect();
        assert_eq!(first_ids, vec!["id-1"]);

        // Second cycle: canonical_1 -> to_google (which reads the stored id
        // back off canonical_1) -> a stubbed response echoing that same id.
        let request = canonical_1.to_google(&chrono::Utc);
        let echoed_id = request.id.clone().expect("a known task carries its id");
        assert_eq!(echoed_id, "id-1");

        let canonical_2 =
            merge_canonical_task_response(canonical_1, task_response(&echoed_id, "Buy milk"))
                .unwrap();

        let second_ids: Vec<&str> = canonical_2
            .x_properties
            .iter()
            .filter(|p| p.name == PROVIDER_TASK_ID_PROPERTY)
            .map(|p| p.value.as_str())
            .collect();
        assert_eq!(
            second_ids,
            vec!["id-1"],
            "{PROVIDER_TASK_ID_PROPERTY} must appear exactly once after a second cycle"
        );
    }

    /// Right proof (per the ticket): binds `P5-02`'s patch-URL builder and
    /// `P5-03`'s delete-URL builder to the *same* identity, rather than
    /// checking each is independently correct against its own fixture.
    #[test]
    fn the_minted_id_resolves_both_the_update_and_the_delete() {
        let local = Todo::new("Buy milk");
        let canonical =
            merge_canonical_task_response(local, task_response("shared-id", "Buy milk")).unwrap();

        let (patch_url, _body) = task_patch_request("task-list-123", &canonical).unwrap();
        let delete_url = task_delete_request("task-list-123", &canonical).unwrap();

        assert_eq!(patch_url, delete_url);
        assert!(patch_url.ends_with("/shared-id"));
    }

    // --- P1-05 VTODO fixtures, copied byte-identical ---
    //
    // `caldir_core::test_utils` is `#[cfg(test)]` and unexported, so this
    // crate cannot import them directly (`docs/vtodo/context/testing.md`
    // §4). Keeping them byte-identical to `caldir-core/src/test_utils.rs`
    // makes a divergence a visible diff rather than a silent drift.

    const P5_07_VTODO_ICS: &str = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//u_crawler//EN\r\n\
CALSCALE:GREGORIAN\r\n\
BEGIN:VTODO\r\n\
UID:assignment-123@u-crawler\r\n\
DTSTAMP:20260801T090000Z\r\n\
DUE:20260815T235900Z\r\n\
SUMMARY:Assignment 3 - Recursive descent parser\r\n\
DESCRIPTION:Submit the parser and a short report via the course portal.\r\n\
URL:https://example.edu/courses/cs101/assignments/123\r\n\
PRIORITY:1\r\n\
STATUS:COMPLETED\r\n\
COMPLETED:20260814T120000Z\r\n\
END:VTODO\r\n\
END:VCALENDAR\r\n";

    const P5_07_VTODO_ZONED_ICS: &str = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//u_crawler//EN\r\n\
CALSCALE:GREGORIAN\r\n\
BEGIN:VTODO\r\n\
UID:zoned-123@u-crawler\r\n\
DTSTAMP:20260801T090000Z\r\n\
DUE;TZID=Europe/Stockholm:20260815T235900\r\n\
COMPLETED:20260814T120000\r\n\
SUMMARY:Zoned deadline\r\n\
END:VTODO\r\n\
END:VCALENDAR\r\n";

    const P5_07_VTODO_OPEN_PRODUCTION_ICS: &str = "BEGIN:VCALENDAR\r\n\
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

    const P5_07_VTODO_COMPLETED_PRODUCTION_ICS: &str = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//u_crawler//calendar-sync//EN\r\n\
BEGIN:VTODO\r\n\
UID:u_crawler-todo-9002295@u-crawler.local\r\n\
DTSTAMP:20260701T040020Z\r\n\
DUE:20260708T035959Z\r\n\
PRIORITY:1\r\n\
STATUS:COMPLETED\r\n\
SUMMARY:Sube aquí tu Certificado: Mentalidad digital\r\n\
URL:https://lms.example.edu/courses/900222/assignments/9002295\r\n\
END:VTODO\r\n\
END:VCALENDAR\r\n";

    /// Echoes a `GoogleTaskInsert` back as the `Task` shape Google's
    /// response would carry — the stub standing in for "Google's own
    /// canonicalization", since this crate has no HTTP mock.
    fn echo_response(id: &str, request: &GoogleTaskInsert) -> Task {
        Task {
            id: id.to_string(),
            title: request.title.clone(),
            notes: request.notes.clone(),
            status: request.status.clone(),
            due: request.due.clone(),
            completed: request.completed.clone(),
            deleted: false,
            hidden: false,
        }
    }

    /// One `to_google` -> `echo_response` -> `from_google` cycle, under the
    /// caller's zone.
    fn round_trip(id: &str, todo: &Todo) -> Todo {
        let request = todo.to_google(&chrono::Local);
        let response = echo_response(id, &request);
        task_to_calendar_item(response)
            .unwrap()
            .map(|item| {
                let CalendarItem::Todo(todo) = item else {
                    unreachable!("task_to_calendar_item always returns CalendarItem::Todo");
                };
                todo
            })
            .expect("echoed response is never reported deleted")
    }

    /// **Necessary, not sufficient** (per the ticket): this is a
    /// self-consistency check on a pure function, run under
    /// `UTC`/`Europe/Stockholm`/`America/New_York`. It proves the
    /// conversion agrees with *itself* on a second pass — it does NOT prove
    /// Google's own canonicalization agrees with caldir's expectation.
    /// Only `live::google_tasks`'s gated tests, run against a real
    /// account, prove that (`GOOGLE-10`, §15).
    #[test]
    fn the_whole_fixture_set_canonicalizes_to_a_fixpoint() {
        for (name, ics) in [
            ("VTODO_ICS", P5_07_VTODO_ICS),
            ("VTODO_ZONED_ICS", P5_07_VTODO_ZONED_ICS),
            ("VTODO_OPEN_PRODUCTION_ICS", P5_07_VTODO_OPEN_PRODUCTION_ICS),
            (
                "VTODO_COMPLETED_PRODUCTION_ICS",
                P5_07_VTODO_COMPLETED_PRODUCTION_ICS,
            ),
        ] {
            let CalendarItem::Todo(local) = CalendarItem::from_single_ics_str(ics).unwrap() else {
                panic!("{name}: fixture parses as a Todo");
            };

            let first_pass = round_trip("fixpoint-task-id", &local);
            let second_pass = round_trip("fixpoint-task-id", &first_pass);

            assert_eq!(
                first_pass, second_pass,
                "{name}: a second to_google/from_google cycle must be a fixpoint"
            );
        }
    }

    #[test]
    fn helper_reads_x_property_by_name() {
        let mut todo = Todo::new("Buy milk");
        todo.x_properties
            .push(XProperty::new(PROVIDER_TASK_ID_PROPERTY, "abc123"));

        assert_eq!(
            todo_x_property(&todo, PROVIDER_TASK_ID_PROPERTY),
            Some("abc123")
        );
    }
}
