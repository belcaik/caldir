mod from_icalendar;
mod status;
mod to_icalendar;

pub use status::TodoStatus;

use crate::event::{
    Attachment, Attendee, EventTime, EventUid, ICS_PRODID, ICS_VERSION, Organizer, Recurrence,
    RecurrenceId, Reminder, XProperty,
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

    /// Serialize to a single-component `VCALENDAR` document (R27), CRLF
    /// throughout (R28). `DTSTAMP` is stamped by the crate at write time and
    /// is never a field (R25); no `DTEND` and no `DURATION` are ever emitted
    /// (R8, R26).
    pub fn to_ics_string(&self) -> String {
        let ical_todo: icalendar::Todo = self.into();

        let ics = icalendar::Calendar::empty()
            .append_property(icalendar::Property::new("VERSION", ICS_VERSION))
            .append_property(icalendar::Property::new("PRODID", ICS_PRODID))
            .push(ical_todo)
            .done()
            .to_string();

        self.splice_valarms_into_vtodo(ics)
    }

    // icalendar mints a fresh UID and DTSTAMP into every VALARM it serializes,
    // so two generations of the same task would never compare equal and every
    // sync would push forever. We hand-roll the block instead, exactly as the
    // event path does — see `Reminder::ics_block` (R24).
    fn splice_valarms_into_vtodo(&self, ics: String) -> String {
        if self.reminders.is_empty() {
            return ics;
        }

        let valarms: String = self.reminders.iter().map(Reminder::ics_block).collect();

        ics.replacen("END:VTODO\r\n", &format!("{valarms}END:VTODO\r\n"), 1)
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

    fn reparse(ics: &str) -> Todo {
        Todo::try_from(&parse_icalendar_todo(ics)).unwrap()
    }

    fn dtstamp_line(ics: &str) -> String {
        ics.split_inclusive("\r\n")
            .find(|line| line.starts_with("DTSTAMP:"))
            .expect("a serialized task always carries a DTSTAMP")
            .to_string()
    }

    fn valarm_section(ics: &str) -> String {
        let start = ics
            .find("BEGIN:VALARM")
            .expect("serialized task must contain a VALARM");
        let end = ics
            .rfind("END:VALARM\r\n")
            .expect("serialized task must contain a VALARM")
            + "END:VALARM\r\n".len();
        ics[start..end].to_string()
    }

    const FULL_VTODO_ICS: &str = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//u_crawler//EN\r\n\
BEGIN:VTODO\r\n\
UID:full@caldir\r\n\
DTSTAMP:20260801T090000Z\r\n\
DTSTART;TZID=Europe/Stockholm:20260810T090000\r\n\
DUE;TZID=Europe/Stockholm:20260815T235900\r\n\
COMPLETED:20260814T120000\r\n\
SUMMARY:Everything\r\n\
DESCRIPTION:All the rows\r\n\
LOCATION:Room 3\r\n\
URL:https://example.edu/a/1\r\n\
STATUS:IN-PROCESS\r\n\
PRIORITY:42\r\n\
PERCENT-COMPLETE:25\r\n\
CREATED:20260701T080000Z\r\n\
LAST-MODIFIED:20260802T101500Z\r\n\
SEQUENCE:4\r\n\
RECURRENCE-ID:20260815T235900Z\r\n\
ORGANIZER;CN=Boss:mailto:boss@example.com\r\n\
ATTENDEE;CN=Ann:mailto:ann@example.com\r\n\
ATTACH:https://example.com/a.pdf\r\n\
RRULE:FREQ=WEEKLY\r\n\
EXDATE:20260822T235900Z\r\n\
RDATE:20260829T235900Z\r\n\
X-GOOGLE-ID;X-SOURCE=api:abc123\r\n\
BEGIN:VALARM\r\n\
ACTION:DISPLAY\r\n\
DESCRIPTION:Reminder\r\n\
TRIGGER;RELATED=START:-PT15M\r\n\
END:VALARM\r\n\
END:VTODO\r\n\
END:VCALENDAR\r\n";

    // T-12 (R24). The crate's alarm builder mints a fresh `Uuid::new_v4()` and
    // `DTSTAMP` into every serialized VALARM, so generation 2 differs from
    // generation 1, the two parse unequal, and every sync pushes forever.
    // Three generations, not two, because the failure is per-call.
    #[test]
    fn valarm_is_byte_stable_across_three_generations() {
        let mut todo = Todo::new("Water the plants");
        todo.reminders = vec![Reminder::from_minutes(60), Reminder::from_minutes(15)];

        let generations: Vec<String> = (0..3)
            .map(|_| valarm_section(&todo.to_ics_string()))
            .collect();

        // Non-vacuity: both alarms really are in the section under comparison.
        assert_eq!(generations[0].matches("BEGIN:VALARM\r\n").count(), 2);
        assert_eq!(generations[0].matches("END:VALARM\r\n").count(), 2);

        // The two properties the crate's builder would inject per call.
        assert!(
            !generations[0].contains("UID"),
            "a hand-rolled VALARM carries no UID: {}",
            generations[0]
        );
        assert!(
            !generations[0].contains("DTSTAMP"),
            "a hand-rolled VALARM carries no DTSTAMP: {}",
            generations[0]
        );

        assert_eq!(generations[0], generations[1]);
        assert_eq!(generations[1], generations[2]);

        // And the alarms survive a round trip, so stability is not stability
        // of nothing.
        assert_eq!(
            reparse(&todo.to_ics_string()).reminders,
            vec![Reminder::from_minutes(60), Reminder::from_minutes(15)]
        );
    }

    // T-19 (R25). `DTSTAMP` is re-stamped at write time and is not a field, so
    // two writes of an unchanged task differ in bytes but MUST compare equal as
    // domain values — that is the exact churn mechanism R25 exists to prevent,
    // asserted at the domain level rather than on bytes (R30).
    #[test]
    fn two_serializations_with_different_wall_clocks_compare_equal() {
        let mut todo = Todo::new("File the return");
        todo.uid = EventUid::new("stamp@caldir".to_string());
        todo.due = Some(EventTime::DateTimeUtc(
            Utc.with_ymd_and_hms(2026, 8, 15, 23, 59, 0).unwrap(),
        ));

        let first = todo.to_ics_string();
        // A second write at a different wall clock. Editing the line rather
        // than sleeping keeps the test deterministic; the crate stamps this
        // line itself, so this is exactly the shape a later write produces.
        let second = first.replace(&dtstamp_line(&first), "DTSTAMP:20991231T235959Z\r\n");

        assert_ne!(
            dtstamp_line(&first),
            dtstamp_line(&second),
            "the two documents must genuinely differ in DTSTAMP"
        );
        assert_eq!(reparse(&first), reparse(&second));
    }

    // R27 / R28, plus the byte-stability half of R24 for the whole document:
    // two writes of the same task differ ONLY in the re-stamped DTSTAMP.
    #[test]
    fn serializing_the_same_task_twice_differs_only_in_dtstamp() {
        let mut todo = reparse(FULL_VTODO_ICS);
        todo.reminders = vec![Reminder::from_minutes(15)];

        let first = todo.to_ics_string();
        let second = todo.to_ics_string();

        let placeholder = "DTSTAMP:<stamped>\r\n";
        assert_eq!(
            first.replace(&dtstamp_line(&first), placeholder),
            second.replace(&dtstamp_line(&second), placeholder)
        );

        // Exactly one supported component (R27), wrapped correctly.
        assert_eq!(first.matches("BEGIN:VTODO\r\n").count(), 1);
        assert_eq!(first.matches("END:VTODO\r\n").count(), 1);
        assert_eq!(first.matches("BEGIN:VEVENT").count(), 0);
        assert!(first.starts_with("BEGIN:VCALENDAR\r\n"));
        assert!(first.ends_with("END:VCALENDAR\r\n"));
        assert!(first.contains("VERSION:2.0\r\n"));
        assert!(first.contains("PRODID:CALDIR\r\n"));

        // CRLF throughout (R28): no bare LF survives stripping every CRLF.
        assert!(!first.replace("\r\n", "").contains('\n'));
    }

    // The round trip the sync engine actually depends on: parse -> serialize ->
    // parse yields an equal `Todo`. Compared as parsed domain values, never as
    // raw bytes (R30) — the re-stamped DTSTAMP alone would sink a byte gate.
    #[test]
    fn a_fully_populated_task_survives_serialize_and_reparse() {
        let original = reparse(FULL_VTODO_ICS);
        let round_tripped = reparse(&original.to_ics_string());

        assert_eq!(original, round_tripped);

        // `==` deliberately ignores created / last_modified / sequence (R38),
        // so assert those separately or the round trip proves less than it
        // looks like it does.
        assert_eq!(original.created, round_tripped.created);
        assert_eq!(original.last_modified, round_tripped.last_modified);
        assert_eq!(original.sequence, round_tripped.sequence);

        // And the values that have a lying crate accessor, spelled out.
        assert_eq!(round_tripped.priority, Some(42));
        assert_eq!(round_tripped.percent_complete, Some(25));
        assert_eq!(round_tripped.status, TodoStatus::InProcess);
        assert_eq!(
            round_tripped.completed,
            Some(EventTime::DateTimeFloating(
                NaiveDate::from_ymd_opt(2026, 8, 14)
                    .unwrap()
                    .and_hms_opt(12, 0, 0)
                    .unwrap()
            ))
        );
        assert_eq!(
            round_tripped.due,
            Some(EventTime::DateTimeZoned {
                datetime: NaiveDate::from_ymd_opt(2026, 8, 15)
                    .unwrap()
                    .and_hms_opt(23, 59, 0)
                    .unwrap(),
                tzid: "Europe/Stockholm".to_string(),
            })
        );
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
