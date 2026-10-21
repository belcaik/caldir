mod from_icalendar;
mod slugify;
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
/// A sibling of `Event`, never a variant of it: a task's kind comes from
/// the component it was parsed from and is preserved across every boundary.
/// Adding `VJOURNAL` later is one more struct alongside this one.
///
/// Fields are exactly spec §6's IN set. Deliberate absences:
/// - no `end`/`duration` — RFC 5545 §3.6.2 forbids `DTEND` in a VTODO, and the
///   field simply not existing is what makes emitting one impossible.
/// - `DTSTAMP` is re-stamped on serialize, never modelled.
/// - `CATEGORIES` and `RELATED-TO` are OUT: the crate corrupts the first and
///   parses the second lossily, so modelling them destroys data.
#[derive(Debug, Clone, Eq, educe::Educe)]
#[educe(PartialEq)]
pub struct Todo {
    pub uid: EventUid,
    pub summary: Option<String>,
    pub description: Option<String>,
    pub location: Option<String>,
    /// `DTSTART`. Independent of `due` — a task may have either, both or
    /// neither, and neither is ever synthesized from the other.
    pub start: Option<EventTime>,
    /// `DUE`. The task's anchor.
    pub due: Option<EventTime>,
    pub status: TodoStatus,
    /// `COMPLETED`. `EventTime`, not `DateTime<Utc>`: a floating value is legal
    /// and must survive.
    pub completed: Option<EventTime>,
    /// `PERCENT-COMPLETE`, raw. `i32` is RFC 5545's `INTEGER`, not the 0..=100
    /// the property is *supposed* to hold: out-of-range is legal input that
    /// round-trips verbatim, exactly as for `priority` (F-4).
    pub percent_complete: Option<i32>,
    /// `PRIORITY`, raw. Values outside `0..=9` are legal input and round-trip
    /// verbatim, so this must never be routed through
    /// `icalendar::Component::priority()`, which clamps to `0..=10` — and must
    /// not be a `u8` either, which cannot hold `256` or `-1` (F-4).
    pub priority: Option<i32>,
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

    /// `CREATED`. Ignored by `==`; kept as a field on data-safety
    /// grounds — it rescues an undated completed task from RFC 4791 §9.9
    /// time-range omission.
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

    /// Whether this task falls inside `[from, to]`.
    ///
    /// The "core's rule" column of RFC 4791 §9.9's VTODO time-range table,
    /// with `start = from` and `end = to`:
    ///
    /// | DTSTART | DUE | rule |
    /// |---|---|---|
    /// | Y | Y | `(from < DUE OR from <= DTSTART) AND (to > DTSTART OR to >= DUE)` |
    /// | Y | N | `from <= DTSTART AND to > DTSTART` |
    /// | N | Y | `from < DUE AND to >= DUE` |
    /// | N | N | TRUE, unconditionally |
    ///
    /// `COMPLETED` and `CREATED` are deliberately **not** consulted: core must
    /// be *more* permissive than any server, never less, or an item a §9.9
    /// server omitted would be read as absent-and-therefore-deleted (§10).
    ///
    /// No date is ever synthesized for an undated task: a
    /// fabricated epoch `DTSTART` evaluates false here and would silently make
    /// undated tasks unpushable rather than merely undeletable.
    pub fn occurs_in_range(&self, from: DateTime<Utc>, to: DateTime<Utc>) -> bool {
        match (self.start.as_ref(), self.due.as_ref()) {
            (Some(start), Some(due)) => {
                let (start, due) = (start.to_utc(), due.to_utc());
                (from < due || from <= start) && (to > start || to >= due)
            }
            (Some(start), None) => {
                let start = start.to_utc();
                from <= start && to > start
            }
            (None, Some(due)) => {
                let due = due.to_utc();
                from < due && to >= due
            }
            (None, None) => true,
        }
    }

    /// Serialize to a single-component `VCALENDAR` document, CRLF throughout.
    /// `DTSTAMP` is stamped by the crate at write time and is never a field; no
    /// `DTEND` and no `DURATION` are ever emitted.
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
    // event path does — see `Reminder::ics_block`.
    fn splice_valarms_into_vtodo(&self, ics: String) -> String {
        if self.reminders.is_empty() {
            return ics;
        }

        let valarms: String = self.reminders.iter().map(Reminder::ics_block).collect();

        ics.replacen("END:VTODO\r\n", &format!("{valarms}END:VTODO\r\n"), 1)
    }

    /// The task-side sibling of `Event::with_x_properties_merged_from`.
    /// Same union-of-params merge, so an item-typed update never strips
    /// provider-managed `X-` properties just because the item is a task.
    pub fn with_x_properties_merged_from(mut self, base: &Todo) -> Self {
        for local in &mut self.x_properties {
            if let Some(base_prop) = base.x_properties.iter().find(|p| p.name == local.name) {
                for (k, v) in &base_prop.params {
                    if !local.params.iter().any(|(lk, _)| lk == k) {
                        local.params.push((k.clone(), v.clone()));
                    }
                }
            }
        }
        for base_prop in &base.x_properties {
            if !self.x_properties.iter().any(|p| p.name == base_prop.name) {
                self.x_properties.push(base_prop.clone());
            }
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{
        VTODO_COMPLETED_PRODUCTION_ICS, VTODO_OPEN_PRODUCTION_ICS, parse_icalendar_todo,
    };
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

    /// One row of the equality table: the field's name, and a mutation that
    /// changes only that field.
    type Mutation = (&'static str, fn(&mut Todo));

    /// The whole of `==`, locked field by field: every content field
    /// participates, and the four deliberate exclusions stay exactly as wide
    /// as they are.
    ///
    /// `Todo`'s `PartialEq` *is* the sync predicate — `==` means "content
    /// unchanged", and that is what picks push over pull. A field silently
    /// falling out of it makes a real edit invisible and the task never
    /// syncs. `equality_mirrors_event_equality` above pins only `status`,
    /// `completed` and `percent_complete`; the other content fields —
    /// `due`, the task's own anchor, included — could all be dropped from the
    /// `educe` attributes with the rest of the suite still green.
    ///
    /// The exhaustive destructure is the second half of the lock: it has no
    /// `..`, so adding a field to `Todo` stops this test compiling until
    /// someone decides which side of `==` the new field belongs on and adds
    /// the matching row below.
    #[test]
    fn every_content_field_participates_in_equality() {
        let base = reparse(FULL_VTODO_ICS);

        let Todo {
            uid,
            summary,
            description,
            location,
            start,
            due,
            status,
            completed,
            percent_complete,
            priority,
            url,
            recurrence,
            recurrence_id,
            organizer,
            attendees,
            reminders,
            attachments,
            x_properties,
            created,
            last_modified,
            sequence,
        } = base.clone();

        // Non-vacuity: every row below mutates an already-populated value, so
        // a failing row can never be failing because the base was empty.
        assert!(!uid.as_str().is_empty());
        assert!(summary.is_some() && description.is_some() && location.is_some());
        assert!(start.is_some() && due.is_some() && completed.is_some());
        assert_eq!(status, TodoStatus::InProcess);
        assert!(percent_complete.is_some() && priority.is_some() && url.is_some());
        assert!(recurrence.is_some() && recurrence_id.is_some() && organizer.is_some());
        assert!(!attendees.is_empty() && !reminders.is_empty());
        assert!(!attachments.is_empty() && !x_properties.is_empty());
        assert!(created.is_some() && last_modified.is_some());
        assert_eq!(sequence, 4);

        // One row per content field, one field changed per row.
        let mutations: [Mutation; 18] = [
            ("uid", |t| t.uid = EventUid::new("someone-else@caldir")),
            ("summary", |t| t.summary = Some("Not everything".into())),
            ("description", |t| {
                t.description = Some("Some other rows".into())
            }),
            ("location", |t| t.location = Some("Room 4".into())),
            ("start", |t| {
                t.start = Some(EventTime::Date(day(2027, 1, 2)))
            }),
            ("due", |t| t.due = Some(EventTime::Date(day(2027, 3, 4)))),
            ("status", |t| t.status = TodoStatus::Completed),
            ("completed", |t| {
                t.completed = Some(EventTime::Date(day(2027, 5, 6)))
            }),
            ("percent_complete", |t| t.percent_complete = Some(26)),
            ("priority", |t| t.priority = Some(43)),
            ("url", |t| t.url = Some("https://example.edu/a/2".into())),
            ("recurrence", |t| {
                t.recurrence.as_mut().unwrap().rrule = "FREQ=DAILY".into()
            }),
            ("recurrence_id", |t| {
                t.recurrence_id = Some(RecurrenceId::from_event_time(EventTime::Date(day(
                    2027, 7, 8,
                ))))
            }),
            ("organizer", |t| {
                t.organizer = Some(Organizer::new("someone.else@example.com"))
            }),
            ("attendees", |t| {
                t.attendees.push(Attendee::new("bo@example.com"))
            }),
            ("reminders", |t| {
                t.reminders.push(Reminder::from_minutes(60))
            }),
            ("attachments", |t| {
                t.attachments
                    .push(Attachment::new("https://example.com/b.pdf"))
            }),
            ("x_properties", |t| {
                t.x_properties.push(XProperty::new("X-OTHER", "2"))
            }),
        ];

        for (field, mutate) in mutations {
            let mut changed = base.clone();
            mutate(&mut changed);
            assert_ne!(
                base, changed,
                "`{field}` is content: changing it must make the task unequal, \
                 or an edit to it never syncs",
            );
        }

        // ---- the four deliberate exclusions, pinned so they cannot widen ----

        // 1. created, last_modified and sequence are ignored (R38).
        let mut bookkeeping = base.clone();
        bookkeeping.created = Some(Utc.with_ymd_and_hms(2001, 1, 1, 0, 0, 0).unwrap());
        bookkeeping.last_modified = Some(Utc.with_ymd_and_hms(2002, 2, 2, 0, 0, 0).unwrap());
        bookkeeping.sequence = sequence + 99;
        assert_eq!(
            base, bookkeeping,
            "created/last_modified/sequence are bookkeeping: drift in them must \
             not read as a content change",
        );

        // 2. attachments compare by URI alone, so volatile params drifting is
        //    not a content change.
        let mut reparametrized = base.clone();
        for attachment in &mut reparametrized.attachments {
            attachment.params.push(("SIZE".into(), "8192".into()));
        }
        assert_ne!(
            base.attachments, reparametrized.attachments,
            "non-vacuity: the attachment vectors really do differ",
        );
        assert_eq!(
            base, reparametrized,
            "an attachment's identity is its URI; params must not churn sync",
        );

        // 3. attachments compare order-independently.
        let mut two = base.clone();
        two.attachments
            .push(Attachment::new("https://example.com/b.pdf"));
        let mut reversed = two.clone();
        reversed.attachments.reverse();
        assert_eq!(two, reversed, "attachment order is not content");

        // 4. x_properties compare order-independently.
        let mut two = base.clone();
        two.x_properties.push(XProperty::new("X-OTHER", "2"));
        let mut reversed = two.clone();
        reversed.x_properties.reverse();
        assert_eq!(two, reversed, "x-property order is not content");
    }

    fn day(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).expect("a real calendar date")
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
    // --- L1-05: the documented loss set, pinned ---
    //
    // These two tests assert CURRENT behaviour on the pinned crate
    // (`icalendar = 0.17.10`). They exist so that a change in either
    // direction — a property we start preserving, or one we start losing —
    // fails loudly instead of drifting. Neither is a bug report: both losses
    // are ruled OUT by ADJUDICATION C-2 and MUST NOT be "fixed" here.

    /// R15's user guarantee, verbatim: the properties caldir promises to drop.
    /// Spelled out as a literal list so `specs/caldir.md`'s amendment (L8-01)
    /// can be diffed against it.
    const R15_DROPPED_PROPERTIES: &[&str] = &[
        "CATEGORIES",
        "GEO",
        "RELATED-TO",
        "RESOURCES",
        "COMMENT",
        "CONTACT",
    ];

    /// The open half of R15 — *"any other IANA or vendor property caldir does
    /// not model"*. `CLASS` is here rather than in the list above because
    /// spec §6 records it as "MAY be inherited"; the ADJUDICATION ruling
    /// ("CLASS is not a modelled field on Todo") settles that in practice it
    /// is dropped on the first caldir write, and this is where that is pinned.
    /// `DURATION` is spec §6's own OUT row; `REQUEST-STATUS` stands in for an
    /// arbitrary unmodelled IANA property.
    const OTHER_UNMODELLED_PROPERTIES: &[&str] = &["CLASS", "DURATION", "REQUEST-STATUS"];

    /// Every modelled property (spec §6's IN set) that a serialized task must
    /// still carry. `DTSTAMP` is here because it is emitted though not
    /// modelled (R25); `VALARM` is a component, not a property, and is
    /// asserted separately.
    const MODELLED_PROPERTIES: &[&str] = &[
        "UID",
        "DTSTAMP",
        "DTSTART",
        "DUE",
        "COMPLETED",
        "SUMMARY",
        "DESCRIPTION",
        "LOCATION",
        "URL",
        "STATUS",
        "PRIORITY",
        "PERCENT-COMPLETE",
        "CREATED",
        "LAST-MODIFIED",
        "SEQUENCE",
        "RECURRENCE-ID",
        "ORGANIZER",
        "ATTENDEE",
        "ATTACH",
        "RRULE",
        "EXDATE",
        "RDATE",
        "X-GOOGLE-ID",
    ];

    /// One instance of every modelled property, every OUT property, and an
    /// `X-` property carrying a parameter.
    const LOSS_SET_ICS: &str = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//u_crawler//EN\r\n\
BEGIN:VTODO\r\n\
UID:loss@caldir\r\n\
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
CATEGORIES:WORK,URGENT\r\n\
GEO:37.386013;-122.082932\r\n\
RELATED-TO;RELTYPE=PARENT:parent@caldir\r\n\
RESOURCES:PROJECTOR\r\n\
COMMENT:A comment\r\n\
CONTACT:Jim Dolittle\r\n\
CLASS:PRIVATE\r\n\
DURATION:PT2H\r\n\
REQUEST-STATUS:2.0;Success\r\n\
BEGIN:VALARM\r\n\
ACTION:DISPLAY\r\n\
DESCRIPTION:Reminder\r\n\
TRIGGER;RELATED=START:-PT15M\r\n\
END:VALARM\r\n\
END:VTODO\r\n\
END:VCALENDAR\r\n";

    /// Property names on the VTODO itself, excluding the nested VALARM block.
    /// Names, not bytes: `DTSTAMP` is re-stamped on every serialize (R25), so
    /// a byte comparison is permanently red.
    fn vtodo_property_names(ics: &str) -> std::collections::BTreeSet<String> {
        let body = ics
            .split_once("BEGIN:VTODO\r\n")
            .expect("document holds a VTODO")
            .1
            .split_once("END:VTODO\r\n")
            .expect("document holds a VTODO")
            .0;

        let mut names = std::collections::BTreeSet::new();
        let mut in_valarm = false;
        for line in body.lines() {
            assert!(
                !line.starts_with(' ') && !line.starts_with('\t'),
                "folded line would defeat name extraction: {line}"
            );
            match line {
                "BEGIN:VALARM" => in_valarm = true,
                "END:VALARM" => in_valarm = false,
                _ if in_valarm => {}
                _ => {
                    let name = line.split([';', ':']).next().unwrap();
                    names.insert(name.to_string());
                }
            }
        }
        names
    }

    // T-17 (R15, R23). R15 is a promise to users about a *set*, so the only
    // checkable form is set equality in both directions: no surprise drop, and
    // no property we believe is dropped that is really being mangled instead.
    // A containment spot-check would pass while an eleventh property quietly
    // vanished.
    //
    // Mechanism note for whoever revisits R88: `CATEGORIES` reaches the crate
    // via `multi_properties()`, not `properties()`, so widening the parser's
    // `X-` filter is not what would start preserving it — and re-emitting it
    // corrupts it (`WORK,URGENT` -> `WORK\,URGENT`, re-verified on 0.17.10).
    #[test]
    fn dropped_property_names_are_exactly_the_documented_list() {
        let input_names = vtodo_property_names(LOSS_SET_ICS);
        let expected_dropped: std::collections::BTreeSet<String> = R15_DROPPED_PROPERTIES
            .iter()
            .chain(OTHER_UNMODELLED_PROPERTIES)
            .map(|name| name.to_string())
            .collect();

        // Non-vacuity, both halves: the fixture really carries every property
        // the assertion is about. Without this the set equality below would
        // pass trivially for any property the fixture forgot.
        let must_be_in_fixture = expected_dropped
            .iter()
            .cloned()
            .chain(MODELLED_PROPERTIES.iter().map(|name| name.to_string()));
        for name in must_be_in_fixture {
            assert!(
                input_names.contains(&name),
                "fixture must carry {name} for this pin to mean anything"
            );
        }

        let todo = Todo::try_from(&parse_icalendar_todo(LOSS_SET_ICS)).unwrap();
        let output = todo.to_ics_string();
        let output_names = vtodo_property_names(&output);

        // 1. Exactly R15's list is dropped — no more, no fewer.
        let dropped: std::collections::BTreeSet<String> =
            input_names.difference(&output_names).cloned().collect();
        assert_eq!(dropped, expected_dropped);

        // 2. And nothing is invented on the way out: the surviving set is the
        //    input minus the dropped set, exactly.
        let survivors: std::collections::BTreeSet<String> =
            input_names.difference(&dropped).cloned().collect();
        assert_eq!(output_names, survivors);

        // 3. Every modelled property survives (2 says the *names* balance;
        //    this says the balance is the one spec §6 promises).
        for name in MODELLED_PROPERTIES {
            assert!(
                output_names.contains(*name),
                "modelled property {name} must survive"
            );
        }

        // 4. R23: the `X-` property survives verbatim, parameters included —
        //    the one thing R15 promises beyond the modelled set.
        assert!(
            output.contains("X-GOOGLE-ID;X-SOURCE=api:abc123\r\n"),
            "X- property must survive with its parameters: {output}"
        );

        // 5. Names surviving is not values surviving: compare domain values.
        assert_eq!(
            todo,
            Todo::try_from(&parse_icalendar_todo(&output)).unwrap()
        );
        assert_eq!(todo.reminders.len(), 1, "the VALARM survives too");
    }

    // T-16 (R87). A PINNED KNOWN LOSS, not a bug under repair. Probe Q-F2
    // shows Radicale stores both values with their `RELTYPE` parameters
    // intact, so the loss is 100% caldir's parser: `MULTIS` lists the ABNF
    // placeholder "RELATED", not "RELATED-TO", so the crate files the property
    // in its single-valued map and the second line overwrites the first —
    // `RELTYPE=PARENT` is gone before caldir sees the document. caldir then
    // drops the survivor as well, because `RELATED-TO` is not modelled
    // (ADJUDICATION C-2, STAYS OUT). Re-encoding as `X-CALDIR-RELATED-TO`
    // inherits the identical defect (R87) — do not attempt it.
    #[test]
    fn two_related_to_lines_collapse_to_one() {
        use icalendar::Component;

        const TWO_RELATED_TO_ICS: &str = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//u_crawler//EN\r\n\
BEGIN:VTODO\r\n\
UID:subtask@caldir\r\n\
DTSTAMP:20260801T090000Z\r\n\
SUMMARY:Chapter 3\r\n\
RELATED-TO;RELTYPE=PARENT:parent@caldir\r\n\
RELATED-TO;RELTYPE=SIBLING:sibling@caldir\r\n\
END:VTODO\r\n\
END:VCALENDAR\r\n";

        // Non-vacuity: two distinct lines really do go in.
        assert_eq!(TWO_RELATED_TO_ICS.matches("RELATED-TO;").count(), 2);
        assert!(TWO_RELATED_TO_ICS.contains("RELTYPE=PARENT"));

        let ical = parse_icalendar_todo(TWO_RELATED_TO_ICS);

        // The mechanism: not a multi-property, so the map holds one value.
        assert!(
            ical.multi_properties().get("RELATED-TO").is_none(),
            "RELATED-TO is absent from MULTIS — that is the whole bug"
        );
        let survivor = ical
            .properties()
            .get("RELATED-TO")
            .expect("one value survives the crate's parser");
        assert_eq!(survivor.value(), "sibling@caldir");

        // Two in, one out at the crate layer, with PARENT gone.
        let crate_output = Component::to_string(&ical);
        assert_eq!(crate_output.matches("RELATED-TO;").count(), 1);
        assert!(
            !crate_output.contains("RELTYPE=PARENT"),
            "the first line's RELTYPE is destroyed by the crate: {crate_output}"
        );

        // And at caldir's own layer the survivor goes too: RELATED-TO is not
        // a modelled field and the passthrough bag is `X-` only (R23).
        let todo = Todo::try_from(&ical).unwrap();
        assert!(
            !todo
                .x_properties
                .iter()
                .any(|x| x.name.contains("RELATED") || x.value.contains("@caldir")),
            "RELATED-TO must not be smuggled through the X- bag"
        );

        let output = todo.to_ics_string();
        assert_eq!(output.matches("RELATED-TO").count(), 0);
        assert!(!output.contains("parent@caldir"));
        assert!(!output.contains("sibling@caldir"));

        // Non-vacuity: the rest of the task really did round-trip, so the
        // absence above is a dropped property and not a failed parse.
        assert_eq!(todo.summary.as_deref(), Some("Chapter 3"));
        assert!(output.contains("SUMMARY:Chapter 3\r\n"));
    }

    // P1-05 / R68. The shipped VTODO_ICS already carries COMPLETED, so R68's
    // synthesis failure is invisible against it. This is production's actual
    // shape: STATUS:COMPLETED alone, with neither COMPLETED nor
    // PERCENT-COMPLETE. Re-serialization must not invent either.
    #[test]
    fn the_completed_production_fixture_carries_no_completed_and_no_percent_complete() {
        let todo = Todo::try_from(&parse_icalendar_todo(VTODO_COMPLETED_PRODUCTION_ICS)).unwrap();

        assert_eq!(todo.status, TodoStatus::Completed);
        assert_eq!(todo.completed, None);
        assert_eq!(todo.percent_complete, None);

        let output = todo.to_ics_string();
        assert!(output.contains("STATUS:COMPLETED\r\n"));
        assert!(!output.contains("COMPLETED:"));
        assert!(!output.contains("PERCENT-COMPLETE:"));
    }

    // P1-05 / Correction 1. The executable form of the production census: no
    // shipped fixture may quietly grow a property the real u_crawler pipeline
    // never emits. A well-meaning later edit that "enriches" one of these
    // fixtures for realism would falsify the evidence P3-06/P5-06 rely on.
    #[test]
    fn the_production_fixtures_carry_no_property_outside_the_census() {
        for forbidden in [
            "DESCRIPTION",
            "COMPLETED:",
            "PERCENT-COMPLETE",
            "DTSTART",
            "RRULE",
            "X-",
        ] {
            assert!(
                !VTODO_OPEN_PRODUCTION_ICS.contains(forbidden),
                "open fixture must not contain {forbidden}"
            );
            assert!(
                !VTODO_COMPLETED_PRODUCTION_ICS.contains(forbidden),
                "completed fixture must not contain {forbidden}"
            );
        }
    }
}
