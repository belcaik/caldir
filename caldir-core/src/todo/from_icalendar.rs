use crate::event::{
    Attachment, Attendee, EventError, EventTime, EventUid, Organizer, Recurrence, RecurrenceId,
    Reminder, XProperty,
};
use crate::todo::{Todo, TodoStatus};
use chrono::{DateTime, Utc};
use icalendar::{CalendarDateTime, Component, DatePerhapsTime};

impl TryFrom<&icalendar::Todo> for Todo {
    type Error = EventError;

    fn try_from(value: &icalendar::Todo) -> Result<Self, Self::Error> {
        let uid = value.get_uid().ok_or(EventError::MissingUid)?.to_string();

        // No `MissingStart` mirror: a task legitimately has neither DTSTART nor
        // DUE. `EventTime::from` is what normalizes an inbound TZID to IANA, so
        // both go through it.
        //
        // `present_or_unparseable` is what keeps *absent* (legal, `None`) apart
        // from *present but unreadable* (an error, F-2). The crate leaves the
        // raw property on the component either way, so without it a malformed
        // DUE turned a dated task into an undated one — silently, and undated
        // is unconditionally in-window.
        let start =
            present_or_unparseable(value, "DTSTART", value.get_start().map(EventTime::from))?;
        let due = present_or_unparseable(value, "DUE", value.get_due().map(EventTime::from))?;

        // COMPLETED is read from the raw property: `get_completed()` returns
        // `None` for a non-UTC value while leaving the property in place, so a
        // floating COMPLETED would vanish.
        let completed =
            present_or_unparseable(value, "COMPLETED", raw_date_time(value, "COMPLETED"))?;

        // STATUS is read from the raw property: `get_status()` silently drops
        // any value outside its own enum. NEEDS-ACTION is the RFC 5545 default
        // for a task, so an absent line and an explicit NEEDS-ACTION are the
        // same state. An unrecognised value is DROPPED, not round-tripped —
        // `TodoStatus` is strictly four-valued.
        let status = value
            .property_value("STATUS")
            .and_then(TodoStatus::from_ics_str)
            .unwrap_or_default();

        // PRIORITY raw: `Component::priority()` clamps to 0..=10 and
        // `get_priority()` returns None above it, so an out-of-range value
        // (legal input, RFC range is 0..=9) would be destroyed. The field is an
        // `i32` — RFC 5545 `INTEGER` — so `256` and `-1` round-trip rather than
        // failing to fit the type and vanishing (F-4).
        let priority = integer_property(value, "PRIORITY")?;

        // The same filter events use. Only `X-` properties are
        // captured; nothing else is ever put into a passthrough bag.
        let x_properties = value
            .properties()
            .iter()
            .filter(|(name, _)| name.starts_with("X-"))
            .map(|(_, prop)| XProperty::from(prop))
            .collect();

        let recurrence = Recurrence::from_ical_event(value);

        let recurrence_id = value
            .get_recurrence_id()
            .map(EventTime::from)
            .map(RecurrenceId::from_event_time);

        let organizer = value.properties().get("ORGANIZER").map(Organizer::from);

        let attendees = value
            .multi_properties()
            .get("ATTENDEE")
            .map(|props| props.iter().map(Attendee::from).collect())
            .unwrap_or_default();

        let reminders = Reminder::from_ical_event(value);

        // ATTACH is a multi-property (RFC 5545); URI links are kept, inline
        // binary blobs are dropped — see `Attachment::from_property`.
        let attachments = value
            .multi_properties()
            .get("ATTACH")
            .map(|props| props.iter().filter_map(Attachment::from_property).collect())
            .unwrap_or_default();

        Ok(Todo {
            uid: EventUid::new(uid),
            summary: non_empty(value.get_summary()),
            description: non_empty(value.get_description()),
            // Read raw rather than via `get_location()`, so `EventLike` — which
            // also carries `ends()`/`get_end()` — never has to be in scope here.
            location: non_empty(value.property_value("LOCATION")),
            start,
            due,
            status,
            completed,
            // Raw, and never synthesized from STATUS — nor STATUS from it. Same
            // `INTEGER` treatment as PRIORITY, so the two behave identically at
            // the edge (F-4).
            percent_complete: integer_property(value, "PERCENT-COMPLETE")?,
            priority,
            url: value.property_value("URL").map(ToString::to_string),
            recurrence,
            recurrence_id,
            organizer,
            attendees,
            reminders,
            attachments,
            x_properties,
            created: raw_utc_date_time(value, "CREATED"),
            last_modified: value.get_last_modified(),
            // SEQUENCE defaults to 0 per RFC 5545; treat missing or
            // unparseable values the same as an explicit 0.
            sequence: value
                .property_value("SEQUENCE")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0),
        })
    }
}

impl TryFrom<icalendar::Todo> for Todo {
    type Error = EventError;

    fn try_from(value: icalendar::Todo) -> Result<Self, Self::Error> {
        Todo::try_from(&value)
    }
}

// CREATED is a UTC-only property per RFC 5545 §3.8.7.1, and the field is a
// `DateTime<Utc>`. Anything else is left out rather than resolved against the
// machine's local zone, which would make the result depend on `TZ`.
fn raw_utc_date_time(value: &icalendar::Todo, name: &str) -> Option<DateTime<Utc>> {
    match DatePerhapsTime::from_property(value.properties().get(name)?)? {
        DatePerhapsTime::DateTime(CalendarDateTime::Utc(datetime)) => Some(datetime),
        _ => None,
    }
}

// A bare `SUMMARY:` line carries no information `None` doesn't. Mirrors the
// event rule so cross-provider comparison stays stable; deliberately a copy
// rather than a shared helper, so the event codec stays untouched.
fn non_empty(s: Option<&str>) -> Option<String> {
    s.filter(|v| !v.is_empty()).map(ToString::to_string)
}

// Read a date(-time) property straight off the component, so a floating or
// zoned value survives instead of being dropped by a UTC-only accessor.
fn raw_date_time(value: &icalendar::Todo, name: &str) -> Option<EventTime> {
    DatePerhapsTime::from_property(value.properties().get(name)?).map(EventTime::from)
}

// `None` from a reader means one of two very different things: the property was
// absent, or it was there and could not be read. The crate keeps the raw
// property in the second case, which is how they are told apart.
fn present_or_unparseable<T>(
    value: &icalendar::Todo,
    name: &str,
    parsed: Option<T>,
) -> Result<Option<T>, EventError> {
    match (parsed, value.properties().get(name)) {
        (None, Some(property)) => Err(EventError::UnparseableProperty {
            name: name.to_string(),
            value: property.value().to_string(),
        }),
        (parsed, _) => Ok(parsed),
    }
}

// An RFC 5545 `INTEGER` property, read raw and never clamped. Out of the
// property's own documented range is legal input and round-trips verbatim;
// not an integer at all is an error, never a silent drop.
fn integer_property(value: &icalendar::Todo, name: &str) -> Result<Option<i32>, EventError> {
    value
        .property_value(name)
        .map(|raw| {
            raw.parse().map_err(|_| EventError::UnparseableProperty {
                name: name.to_string(),
                value: raw.to_string(),
            })
        })
        .transpose()
}

#[cfg(test)]
mod tests {
    use crate::event::{EventError, EventTime, Organizer, Recurrence, RecurrenceId, Reminder};
    use crate::test_utils::{
        VTODO_COMPLETED_PRODUCTION_ICS, VTODO_OPEN_PRODUCTION_ICS, VTODO_ZONED_ICS,
        parse_icalendar_todo,
    };
    use crate::todo::{Todo, TodoStatus};
    use chrono::{NaiveDate, TimeZone, Utc};
    use icalendar::Component;
    use pretty_assertions::assert_eq;

    // UID is half of identity (R13); a task without one has no identity to
    // sync against. This is the ONLY missing-property error for a task —
    // `EventError::MissingStart` must never fire here (R20).
    #[test]
    fn errors_when_the_task_has_no_uid() {
        let ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
BEGIN:VTODO\r\n\
DTSTAMP:20260801T090000Z\r\n\
SUMMARY:Anonymous\r\n\
END:VTODO\r\n\
END:VCALENDAR\r\n";

        let result = Todo::try_from(&parse_icalendar_todo(ics));

        assert!(matches!(result, Err(EventError::MissingUid)));
    }

    // R13's remaining IN rows, in one pass over a fully-populated task.
    // DTSTART carries a Windows TZID to pin the IANA normalization boundary
    // (R21) on the start half as well as on DUE.
    #[test]
    fn parses_the_rest_of_the_mvp_property_set() {
        let ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
BEGIN:VTODO\r\n\
UID:full@caldir\r\n\
DTSTAMP:20260801T090000Z\r\n\
DTSTART;TZID=W. Europe Standard Time:20260810T090000\r\n\
DUE:20260815T235900Z\r\n\
SUMMARY:Everything\r\n\
DESCRIPTION:All the rows\r\n\
LOCATION:Room 3\r\n\
URL:https://example.edu/a/1\r\n\
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
BEGIN:VALARM\r\n\
ACTION:DISPLAY\r\n\
DESCRIPTION:Reminder\r\n\
TRIGGER:-PT15M\r\n\
END:VALARM\r\n\
END:VTODO\r\n\
END:VCALENDAR\r\n";

        let todo = Todo::try_from(&parse_icalendar_todo(ics)).unwrap();

        assert_eq!(todo.uid.as_str(), "full@caldir");
        assert_eq!(
            todo.start,
            Some(EventTime::DateTimeZoned {
                datetime: NaiveDate::from_ymd_opt(2026, 8, 10)
                    .unwrap()
                    .and_hms_opt(9, 0, 0)
                    .unwrap(),
                tzid: "Europe/Berlin".to_string(),
            })
        );
        assert_eq!(todo.description.as_deref(), Some("All the rows"));
        assert_eq!(todo.location.as_deref(), Some("Room 3"));
        assert_eq!(todo.url.as_deref(), Some("https://example.edu/a/1"));
        assert_eq!(todo.percent_complete, Some(25));
        assert_eq!(
            todo.created,
            Some(Utc.with_ymd_and_hms(2026, 7, 1, 8, 0, 0).unwrap())
        );
        assert_eq!(
            todo.last_modified,
            Some(Utc.with_ymd_and_hms(2026, 8, 2, 10, 15, 0).unwrap())
        );
        assert_eq!(todo.sequence, 4);
        assert_eq!(
            todo.recurrence_id,
            Some(RecurrenceId::from_event_time(EventTime::DateTimeUtc(
                Utc.with_ymd_and_hms(2026, 8, 15, 23, 59, 0).unwrap()
            )))
        );
        assert_eq!(
            todo.organizer,
            Some(Organizer {
                email: "boss@example.com".to_string(),
                name: Some("Boss".to_string()),
            })
        );
        assert_eq!(todo.attendees.len(), 1);
        assert_eq!(todo.attendees[0].email, "ann@example.com");
        assert_eq!(todo.attachments.len(), 1);
        assert_eq!(todo.attachments[0].uri, "https://example.com/a.pdf");
        assert_eq!(
            todo.recurrence,
            Some(Recurrence {
                rrule: "FREQ=WEEKLY".to_string(),
                exdates: vec![EventTime::DateTimeUtc(
                    Utc.with_ymd_and_hms(2026, 8, 22, 23, 59, 0).unwrap()
                )],
                rdates: vec![EventTime::DateTimeUtc(
                    Utc.with_ymd_and_hms(2026, 8, 29, 23, 59, 0).unwrap()
                )],
            })
        );
        assert_eq!(todo.reminders, vec![Reminder::from_minutes(15)]);
    }

    // R23. Same `starts_with("X-")` filter events use: name, value and
    // parameters kept; nothing that is not an `X-` property enters the bag.
    #[test]
    fn x_properties_are_captured_with_parameters_and_nothing_else_is() {
        let ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
BEGIN:VTODO\r\n\
UID:x-props@caldir\r\n\
DTSTAMP:20260801T090000Z\r\n\
SUMMARY:Tagged\r\n\
CATEGORIES:WORK\r\n\
RELATED-TO;RELTYPE=PARENT:parent-1@caldir\r\n\
X-GOOGLE-ID;X-SOURCE=api:abc123\r\n\
END:VTODO\r\n\
END:VCALENDAR\r\n";

        let todo = Todo::try_from(&parse_icalendar_todo(ics)).unwrap();

        assert_eq!(todo.x_properties.len(), 1);
        let captured = &todo.x_properties[0];
        assert_eq!(captured.name, "X-GOOGLE-ID");
        assert_eq!(captured.value, "abc123");
        assert_eq!(
            captured.params,
            vec![("X-SOURCE".to_string(), "api".to_string())]
        );
    }

    // R20 / R8. A task legitimately has no DTSTART, and `DTEND` is
    // RFC-invalid on a VTODO: `EventLike` still leaks `ends()`/`get_end()`
    // onto `icalendar::Todo`, so this pins that caldir reads neither the
    // property into a field (there is none) nor into the passthrough bag —
    // nothing can carry it back out on serialize (L1-04).
    #[test]
    fn a_task_needs_no_dtstart_and_never_captures_a_dtend() {
        let ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
BEGIN:VTODO\r\n\
UID:no-start@caldir\r\n\
DTSTAMP:20260801T090000Z\r\n\
DUE:20260815T235900Z\r\n\
DTEND:20260815T120000Z\r\n\
SUMMARY:No start\r\n\
END:VTODO\r\n\
END:VCALENDAR\r\n";

        let component = parse_icalendar_todo(ics);

        // The trap is live: the crate really did parse the DTEND onto the
        // component and `EventLike` really does expose it here. Without this
        // the assertions below would pass vacuously on a fixture whose DTEND
        // the crate had silently discarded.
        assert!(
            component.get_end().is_some(),
            "fixture must actually carry a DTEND for this test to mean anything"
        );

        let todo = Todo::try_from(&component).unwrap();

        assert_eq!(todo.start, None);
        assert_eq!(
            todo.due,
            Some(EventTime::DateTimeUtc(
                Utc.with_ymd_and_hms(2026, 8, 15, 23, 59, 0).unwrap()
            ))
        );
        assert_eq!(todo.summary.as_deref(), Some("No start"));
        assert!(
            !todo.x_properties.iter().any(|x| x.name == "DTEND"),
            "DTEND must not be captured anywhere on a Todo"
        );
    }

    // R21, DUE half. `parses_the_rest_of_the_mvp_property_set` pins the
    // normalization boundary on DTSTART, but the T-13 fixture's DUE already
    // carries an IANA TZID, so nothing pinned it on DUE — a parser that kept
    // the inbound TZID verbatim passed both. This is the missing lock: a
    // Windows TZID on DUE must arrive as IANA, exactly as it does on DTSTART.
    #[test]
    fn a_due_with_a_windows_tzid_is_normalized_to_iana() {
        let ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
BEGIN:VTODO\r\n\
UID:windows-due@caldir\r\n\
DTSTAMP:20260801T090000Z\r\n\
DUE;TZID=W. Europe Standard Time:20260815T235900\r\n\
SUMMARY:Windows deadline\r\n\
END:VTODO\r\n\
END:VCALENDAR\r\n";

        let todo = Todo::try_from(&parse_icalendar_todo(ics)).unwrap();

        assert_eq!(
            todo.due,
            Some(EventTime::DateTimeZoned {
                datetime: NaiveDate::from_ymd_opt(2026, 8, 15)
                    .unwrap()
                    .and_hms_opt(23, 59, 0)
                    .unwrap(),
                tzid: "Europe/Berlin".to_string(),
            })
        );
    }

    fn vtodo(body: &str) -> String {
        format!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VTODO\r\nUID:t-1@caldir\r\n\
DTSTAMP:20260801T090000Z\r\n{body}END:VTODO\r\nEND:VCALENDAR\r\n"
        )
    }

    fn vevent(body: &str) -> icalendar::Event {
        let ics = format!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:e-1@caldir\r\n\
DTSTAMP:20260801T090000Z\r\n{body}END:VEVENT\r\nEND:VCALENDAR\r\n"
        );
        let calendar: icalendar::Calendar = ics.parse().expect("fixture parses");
        calendar
            .components
            .iter()
            .find_map(icalendar::CalendarComponent::as_event)
            .expect("one VEVENT")
            .clone()
    }

    /// **F-2**, the whole asymmetry in one table. Every one of these values is
    /// retained on the component by the crate and returns `None` from the
    /// accessor caldir reads it through, so the pre-fix parser turned each one
    /// into "property absent" — and for `DUE`/`DTSTART` that silently
    /// converted a **dated** task into an **undated** one, which R46 makes
    /// unconditionally in-window. A typo must not move an item's sync
    /// visibility (R19: no silent drops).
    ///
    /// The event control is `errors_when_ical_event_missing_start`: a `VEVENT`
    /// with `DTSTART:notadate` is `Err(MissingStart)`. Tasks now error too;
    /// they need their own variant only because an *absent* `DTSTART` is legal
    /// on a task (R20) and must stay `None`.
    #[test]
    fn an_unparseable_property_is_an_error_rather_than_a_silent_drop() {
        for (name, body) in [
            ("DUE", "DUE:notadate\r\n"),
            ("DUE", "DUE:20260815T2359\r\n"),
            ("DTSTART", "DTSTART:garbage\r\n"),
            ("COMPLETED", "COMPLETED:oops\r\n"),
            ("PRIORITY", "PRIORITY:high\r\n"),
            ("PERCENT-COMPLETE", "PERCENT-COMPLETE:most\r\n"),
        ] {
            let ics = vtodo(body);
            let component = parse_icalendar_todo(&ics);

            // The trap is live: the crate really kept the raw property, so a
            // parser that dropped it dropped data rather than reading nothing.
            assert!(
                component.properties().contains_key(name),
                "fixture must actually carry {name} for this test to mean anything"
            );

            let err = Todo::try_from(&component)
                .expect_err(&format!("{body:?} must not parse into a task"));

            match &err {
                EventError::UnparseableProperty { name: got, .. } => assert_eq!(got, name),
                other => panic!("expected UnparseableProperty for {body:?}, got {other:?}"),
            }
            assert!(err.to_string().contains(name), "{err}");
        }
    }

    /// The one property that stays lenient, and why: the **event** path already
    /// treats an unparseable `SEQUENCE` as the RFC 5545 default of `0`
    /// (`event/from_icalendar.rs`, `.and_then(|s| s.parse().ok()).unwrap_or(0)`).
    /// `SEQUENCE` carries no user data — it is a sync tie-break — and erroring
    /// on a task where an event shrugs would be the same asymmetry F-2 is
    /// about, pointing the other way. Both halves are asserted here so the two
    /// cannot drift apart.
    #[test]
    fn an_unparseable_sequence_is_lenient_on_a_task_exactly_as_it_is_on_an_event() {
        let todo = Todo::try_from(&parse_icalendar_todo(&vtodo("SEQUENCE:x\r\n"))).unwrap();
        assert_eq!(todo.sequence, 0);

        let event =
            crate::event::Event::try_from(&vevent("DTSTART:20260810T090000Z\r\nSEQUENCE:x\r\n"))
                .unwrap();
        assert_eq!(event.sequence, 0);
    }

    /// **F-4** (R13 PRIORITY row: *"Out-of-range MUST round-trip verbatim"*).
    /// `T-15` only ever used `42`, which fits `u8`; the two values that do not
    /// were dropped. `PERCENT-COMPLETE` is asserted alongside because the two
    /// raw `INTEGER` fields must behave identically at the edge.
    #[test]
    fn an_out_of_range_integer_round_trips_verbatim_rather_than_vanishing() {
        for (body, priority, percent) in [
            ("PRIORITY:256\r\n", Some(256), None),
            ("PRIORITY:-1\r\n", Some(-1), None),
            ("PERCENT-COMPLETE:250\r\n", None, Some(250)),
            ("PERCENT-COMPLETE:-5\r\n", None, Some(-5)),
        ] {
            let todo = Todo::try_from(&parse_icalendar_todo(&vtodo(body))).unwrap();

            assert_eq!(todo.priority, priority, "{body:?}");
            assert_eq!(todo.percent_complete, percent, "{body:?}");
            assert!(
                todo.to_ics_string().contains(body.trim_end()),
                "{body:?} must survive back out verbatim, got {}",
                todo.to_ics_string()
            );
        }
    }

    // T-13 (R21/R22). TZ-marked: must pass under UTC, Europe/Stockholm and
    // America/New_York. Both properties have a crate accessor that quietly
    // returns `None` for exactly these inputs, so the failure mode is silent
    // data loss and it is zone-sensitive.
    #[test]
    fn zoned_due_and_floating_completed_round_trip() {
        let todo = Todo::try_from(&parse_icalendar_todo(VTODO_ZONED_ICS)).unwrap();

        assert_eq!(
            todo.due,
            Some(EventTime::DateTimeZoned {
                datetime: NaiveDate::from_ymd_opt(2026, 8, 15)
                    .unwrap()
                    .and_hms_opt(23, 59, 0)
                    .unwrap(),
                tzid: "Europe/Stockholm".to_string(),
            })
        );

        assert_eq!(
            todo.completed,
            Some(EventTime::DateTimeFloating(
                NaiveDate::from_ymd_opt(2026, 8, 14)
                    .unwrap()
                    .and_hms_opt(12, 0, 0)
                    .unwrap()
            ))
        );
    }

    // P1-05. u_crawler's open task, redacted from the production archive.
    // Every property the census found on 57/57 tasks must survive: SUMMARY,
    // the UTC-datetime DUE, PRIORITY:1, URL, and a needs-action status. A
    // redaction that quietly dropped URL or rounded DUE to a date would be
    // invisible in review and would silently retune P5-06's classification
    // table.
    #[test]
    fn the_open_production_fixture_parses_with_every_property_intact() {
        let todo = Todo::try_from(&parse_icalendar_todo(VTODO_OPEN_PRODUCTION_ICS)).unwrap();

        assert_eq!(
            todo.summary.as_deref(),
            Some("Semana 11: Sumativa 5: Solemne 2")
        );
        assert_eq!(
            todo.due,
            Some(EventTime::DateTimeUtc(
                Utc.with_ymd_and_hms(2026, 9, 2, 3, 59, 59).unwrap()
            ))
        );
        assert_eq!(todo.priority, Some(1));
        assert_eq!(
            todo.url.as_deref(),
            Some("https://lms.example.edu/courses/900223/assignments/9001571")
        );
        assert_eq!(todo.status, TodoStatus::NeedsAction);
    }

    // P1-05 / C-11 non-ASCII. 40 of 57 production summaries carry non-ASCII
    // UTF-8; no shipped fixture did before this one. A CalDAV PUT whose
    // Content-Length is computed in chars, or a Google Tasks title truncated
    // on a byte boundary, cannot be reproduced with an ASCII fixture at all.
    #[test]
    fn a_production_summary_survives_its_non_ascii_text() {
        let todo = Todo::try_from(&parse_icalendar_todo(VTODO_COMPLETED_PRODUCTION_ICS)).unwrap();

        assert_eq!(
            todo.summary.as_deref(),
            Some("Sube aquí tu Certificado: Mentalidad digital")
        );
        assert!(
            todo.to_ics_string()
                .contains("SUMMARY:Sube aquí tu Certificado: Mentalidad digital"),
            "non-ASCII SUMMARY must round-trip byte-equal, got {}",
            todo.to_ics_string()
        );
    }
}
