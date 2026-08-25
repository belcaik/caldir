mod from_icalendar;
mod status;

pub use status::TodoStatus;

use crate::event::{
    Attachment, Attendee, EventTime, EventUid, Organizer, Recurrence, RecurrenceId, Reminder,
    XProperty,
    equality::{attachments_eq, x_properties_eq},
    new_uid,
};
use chrono::{DateTime, Utc};

/// A VTODO task.
///
/// A sibling of `Event`, never a variant of it (R7): a task's kind comes from
/// the component it was parsed from and is preserved across every boundary.
/// Adding `VJOURNAL` later is one more struct alongside this one.
///
/// Fields are exactly spec §6's IN set. Deliberate absences:
/// - no `end`/`duration` — RFC 5545 §3.6.2 forbids `DTEND` in a VTODO, and the
///   field simply not existing is what makes emitting one impossible (R8).
/// - `DTSTAMP` is re-stamped on serialize, never modelled (R25).
/// - `CATEGORIES` and `RELATED-TO` are OUT: the crate corrupts the first and
///   parses the second lossily, so modelling them destroys data (R87, R88).
#[derive(Debug, Clone, Eq, educe::Educe)]
#[educe(PartialEq)]
pub struct Todo {
    pub uid: EventUid,
    pub summary: Option<String>,
    pub description: Option<String>,
    pub location: Option<String>,
    /// `DTSTART`. Independent of `due` — a task may have either, both or
    /// neither, and neither is ever synthesized from the other (R9, R46).
    pub start: Option<EventTime>,
    /// `DUE`. The task's anchor.
    pub due: Option<EventTime>,
    pub status: TodoStatus,
    /// `COMPLETED`. `EventTime`, not `DateTime<Utc>`: a floating value is legal
    /// and must survive (R22).
    pub completed: Option<EventTime>,
    /// `PERCENT-COMPLETE`.
    pub percent_complete: Option<u8>,
    /// `PRIORITY`, raw. Values above 9 are legal input and round-trip verbatim,
    /// so this must never be routed through `icalendar::Component::priority()`,
    /// which clamps to `0..=10`.
    pub priority: Option<u8>,
    pub url: Option<String>,
    /// `RRULE`/`EXDATE`/`RDATE`, modelled but never expanded. Master only.
    pub recurrence: Option<Recurrence>,
    /// The other half of identity, not an expansion hint.
    pub recurrence_id: Option<RecurrenceId>,
    pub organizer: Option<Organizer>,
    pub attendees: Vec<Attendee>,
    /// `VALARM`.
    pub reminders: Vec<Reminder>,
    #[educe(PartialEq(method(attachments_eq)))]
    pub attachments: Vec<Attachment>,

    #[educe(PartialEq(method(x_properties_eq)))]
    pub x_properties: Vec<XProperty>,

    /// `CREATED`. Ignored by `==` (R38); kept as a field on data-safety
    /// grounds — it rescues an undated completed task from RFC 4791 §9.9
    /// time-range omission (ADJUDICATION C-4).
    #[educe(PartialEq(ignore))]
    pub created: Option<DateTime<Utc>>,

    #[educe(PartialEq(ignore))]
    pub last_modified: Option<DateTime<Utc>>,

    #[educe(PartialEq(ignore))]
    pub sequence: i32,
}

impl Todo {
    pub fn new(summary: impl Into<String>) -> Self {
        Todo {
            uid: new_uid(),
            summary: Some(summary.into()),
            description: None,
            location: None,
            start: None,
            due: None,
            status: TodoStatus::default(),
            completed: None,
            percent_complete: None,
            priority: None,
            url: None,
            recurrence: None,
            recurrence_id: None,
            organizer: None,
            attendees: Vec::new(),
            reminders: Vec::new(),
            attachments: Vec::new(),
            x_properties: Vec::new(),
            created: None,
            last_modified: None,
            sequence: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::parse_icalendar_todo;
    use chrono::{NaiveDate, TimeZone};
    use pretty_assertions::assert_eq;

    fn todo_from_vtodo_body(body: &str) -> Todo {
        let ics = format!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//caldir//EN\r\nBEGIN:VTODO\r\nUID:t-1@caldir\r\nDTSTAMP:20260801T090000Z\r\n{body}END:VTODO\r\nEND:VCALENDAR\r\n"
        );
        Todo::try_from(&parse_icalendar_todo(&ics)).unwrap()
    }

    // T-14 (R14). `TodoStatus` is strictly four-valued, so "round-trips
    // verbatim" is not available: an unrecognised STATUS is DROPPED, and this
    // test is the pin on that documented loss. It must never become an
    // accident — the value is gone, it is not hiding in the passthrough bag,
    // and a known value still parses.
    #[test]
    fn an_unknown_status_value_is_preserved_or_explicitly_dropped() {
        let known = todo_from_vtodo_body("STATUS:IN-PROCESS\r\n");
        assert_eq!(known.status, TodoStatus::InProcess);

        let unknown = todo_from_vtodo_body("STATUS:DEFERRED\r\n");
        let absent = todo_from_vtodo_body("");

        // The loss: an unrecognised value is indistinguishable from no STATUS.
        assert_eq!(unknown.status, TodoStatus::NeedsAction);
        assert_eq!(unknown.status, absent.status);

        // And it is genuinely dropped, not smuggled through the X- bag.
        assert!(
            !unknown
                .x_properties
                .iter()
                .any(|x| x.value == "DEFERRED" || x.name == "STATUS")
        );
    }

    // R68 / R46. Radicale accepts `STATUS:COMPLETED` with no
    // `PERCENT-COMPLETE`, and the contradictory `COMPLETED` + `0` pair, and
    // stores both verbatim. A regression lock, not a red-first test: it pins
    // that caldir never "repairs" one from the other, nor fabricates a
    // COMPLETED timestamp.
    #[test]
    fn neither_status_nor_percent_complete_is_synthesized_from_the_other() {
        let no_percent = todo_from_vtodo_body("STATUS:COMPLETED\r\n");
        assert_eq!(no_percent.status, TodoStatus::Completed);
        assert_eq!(no_percent.percent_complete, None);
        assert_eq!(no_percent.completed, None);

        let contradictory = todo_from_vtodo_body("STATUS:COMPLETED\r\nPERCENT-COMPLETE:0\r\n");
        assert_eq!(contradictory.status, TodoStatus::Completed);
        assert_eq!(contradictory.percent_complete, Some(0));

        let only_percent = todo_from_vtodo_body("PERCENT-COMPLETE:100\r\n");
        assert_eq!(only_percent.status, TodoStatus::NeedsAction);
        assert_eq!(only_percent.percent_complete, Some(100));
    }

    // T-15 (R13 PRIORITY row). `Component::priority()` clamps to 0..=10 and
    // `get_priority()` returns None above it, so the natural implementation
    // destroys an out-of-range value. caldir stores PRIORITY raw.
    #[test]
    fn priority_out_of_rfc_range_round_trips_verbatim() {
        assert_eq!(todo_from_vtodo_body("PRIORITY:42\r\n").priority, Some(42));

        // The in-range values still parse, and an absent line stays None —
        // never a fabricated 0 (R46).
        assert_eq!(todo_from_vtodo_body("PRIORITY:1\r\n").priority, Some(1));
        assert_eq!(todo_from_vtodo_body("PRIORITY:0\r\n").priority, Some(0));
        assert_eq!(todo_from_vtodo_body("").priority, None);
    }

    #[test]
    fn equality_mirrors_event_equality() {
        let base = Todo::new("Submit the form");

        // 1. last_modified, sequence and created are ignored (R38).
        let mut a = base.clone();
        a.last_modified = Some(Utc.with_ymd_and_hms(2026, 1, 1, 9, 0, 0).unwrap());
        a.sequence = 3;
        a.created = Some(Utc.with_ymd_and_hms(2026, 1, 1, 8, 0, 0).unwrap());
        let mut b = base.clone();
        b.last_modified = a.last_modified;
        b.sequence = 12;
        b.created = Some(Utc.with_ymd_and_hms(2020, 5, 5, 5, 0, 0).unwrap());
        assert_eq!(a, b);

        // 2. x_properties compare order-independently.
        let mut a = base.clone();
        a.x_properties = vec![XProperty::new("X-ONE", "1"), XProperty::new("X-TWO", "2")];
        let mut b = base.clone();
        b.x_properties = vec![XProperty::new("X-TWO", "2"), XProperty::new("X-ONE", "1")];
        assert_eq!(a, b);

        // 3. attachments compare order-independently, by URI.
        let mut a = base.clone();
        a.attachments = vec![
            Attachment::new("https://example.com/a.pdf"),
            Attachment::new("https://example.com/b.pdf"),
        ];
        let mut b = base.clone();
        b.attachments = vec![
            Attachment::new("https://example.com/b.pdf"),
            Attachment::new("https://example.com/a.pdf"),
        ];
        assert_eq!(a, b);

        // 4. status, completed and percent_complete are content (R39).
        let mut changed = base.clone();
        changed.status = TodoStatus::Completed;
        assert_ne!(base, changed);

        let mut changed = base.clone();
        changed.completed = Some(EventTime::Date(
            NaiveDate::from_ymd_opt(2026, 2, 3).unwrap(),
        ));
        assert_ne!(base, changed);

        let mut changed = base.clone();
        changed.percent_complete = Some(50);
        assert_ne!(base, changed);
    }

    #[test]
    fn new_synthesizes_neither_a_start_nor_a_due() {
        let todo = Todo::new("Submit the form");

        assert_eq!(todo.summary.as_deref(), Some("Submit the form"));
        assert_eq!(todo.start, None);
        assert_eq!(todo.due, None);
        assert_eq!(todo.completed, None);
        assert_eq!(todo.status, TodoStatus::NeedsAction);
        assert!(!todo.uid.as_str().is_empty());
    }
}
