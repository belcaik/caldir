use crate::todo::{Todo, TodoStatus};
use icalendar::{Component, DatePerhapsTime, Property};

// `icalendar::EventLike` is deliberately NOT imported in this module.
// `event_impl! { Todo }` (icalendar 0.17.10 `components.rs:665`) implements it
// for `icalendar::Todo`, which puts the FORBIDDEN `ends()` / `all_day()` in
// scope and lets a `DTEND` — RFC-invalid on a VTODO (R8) — compile. Keeping the
// trait out of scope is a compile-time barrier, not a convention: `DTSTART`,
// `LOCATION` and `RECURRENCE-ID` are therefore written as raw properties below
// rather than through `starts()`, `location()` and `recurrence_id()`.
// The emitted-bytes proof lives in `serializing_a_task_never_emits_dtend`.

impl From<&Todo> for icalendar::Todo {
    fn from(value: &Todo) -> Self {
        let mut todo = icalendar::Todo::new();
        todo.uid(value.uid.as_str());

        // Both anchors are independent and neither is ever synthesized from the
        // other (R9, R46). `due` must NEVER be routed through `ends()`.
        if let Some(start) = &value.start {
            todo.append_property(DatePerhapsTime::from(start).to_property("DTSTART"));
        }

        if let Some(due) = &value.due {
            todo.append_property(DatePerhapsTime::from(due).to_property("DUE"));
        }

        // Omit STATUS when it holds its RFC 5545 default (NEEDS-ACTION) so a
        // parsed-then-written task yields the bytes it came from, mirroring the
        // event codec. Written raw: `Todo::status()` takes the crate's own
        // four-valued enum, which is not caldir's.
        if value.status != TodoStatus::default() {
            todo.append_property(Property::new("STATUS", value.status.as_ics_str()));
        }

        // COMPLETED raw: `Todo::completed()` takes a `DateTime<Utc>`, so a
        // legal floating or zoned value would be coerced or lost (R22).
        if let Some(completed) = &value.completed {
            todo.append_property(DatePerhapsTime::from(completed).to_property("COMPLETED"));
        }

        if let Some(percent) = value.percent_complete {
            todo.append_property(Property::new("PERCENT-COMPLETE", percent.to_string()));
        }

        // PRIORITY raw: `Component::priority()` does `min(priority, 10)`, so an
        // out-of-range value — legal input that must round-trip verbatim — is
        // silently corrupted on the way out (spec §6 PRIORITY row, T-15).
        if let Some(priority) = value.priority {
            todo.append_property(Property::new("PRIORITY", priority.to_string()));
        }

        if let Some(recurrence) = &value.recurrence {
            recurrence.apply_to(&mut todo);
        }

        if let Some(recurrence_id) = &value.recurrence_id {
            todo.append_property(
                DatePerhapsTime::from(recurrence_id.as_event_time()).to_property("RECURRENCE-ID"),
            );
        }

        if let Some(summary) = &value.summary {
            todo.summary(summary);
        }

        if let Some(description) = &value.description {
            todo.description(description);
        }

        if let Some(location) = &value.location {
            todo.append_property(Property::new("LOCATION", location));
        }

        if let Some(created) = value.created {
            todo.created(created);
        }

        if let Some(last_modified) = value.last_modified {
            todo.last_modified(last_modified);
        }

        // Omit SEQUENCE at its RFC 5545 default (0), as the event codec does.
        // Written raw: `Component::sequence()` takes a `u32` and caldir's field
        // is an `i32`.
        if value.sequence != 0 {
            todo.append_property(Property::new("SEQUENCE", value.sequence.to_string()));
        }

        if let Some(organizer) = &value.organizer {
            todo.append_property(Property::from(organizer));
        }

        for attendee in &value.attendees {
            todo.append_multi_property(Property::from(attendee));
        }

        // Reminders are spliced straight into the ICS string by
        // `Todo::to_ics_string`, never via `todo.alarm(...)`: the crate mints a
        // fresh `Uuid::new_v4()` and `DTSTAMP` into every serialized VALARM, so
        // two generations of the same task compare unequal and sync pushes
        // forever (R24). See `Reminder::ics_block`.

        if let Some(url) = &value.url {
            todo.url(url);
        }

        for attachment in &value.attachments {
            todo.append_multi_property(Property::from(attachment));
        }

        for x in &value.x_properties {
            todo.append_property(Property::from(x));
        }

        // DTSTAMP is deliberately never set: `Component::fmt_write`
        // (`components.rs:99-101`) stamps a fresh one at write time, which is
        // exactly R25's "emitted, never modelled".

        todo.done()
    }
}

impl From<Todo> for icalendar::Todo {
    fn from(value: Todo) -> Self {
        icalendar::Todo::from(&value)
    }
}

#[cfg(test)]
mod tests {
    use crate::event::{EventTime, EventUid};
    use crate::todo::{Todo, TodoStatus};
    use chrono::{TimeZone, Utc};
    use icalendar::Component;
    use pretty_assertions::assert_eq;

    fn dated_task() -> Todo {
        let mut todo = Todo::new("Ship the thing");
        todo.uid = EventUid::new("dtend-guard@caldir".to_string());
        todo.start = Some(EventTime::DateTimeUtc(
            Utc.with_ymd_and_hms(2026, 8, 10, 9, 0, 0).unwrap(),
        ));
        todo.due = Some(EventTime::DateTimeUtc(
            Utc.with_ymd_and_hms(2026, 8, 15, 23, 59, 0).unwrap(),
        ));
        todo
    }

    // PRIORITY goes out raw. `Component::priority()` does `min(priority, 10)`
    // (`components.rs:206`), so routing it through the typed setter silently
    // rewrites 42 as 10 — a corruption a round-trip-only test would not see,
    // because 10 parses back cleanly as 10.
    #[test]
    fn priority_is_emitted_verbatim_rather_than_clamped() {
        // The trap is live on the pinned 0.17.10.
        let mut clamped = icalendar::Todo::new();
        clamped.priority(42);
        assert_eq!(clamped.property_value("PRIORITY"), Some("10"));

        let mut todo = Todo::new("Out of range");
        todo.priority = Some(42);

        assert!(
            todo.to_ics_string().contains("PRIORITY:42\r\n"),
            "{}",
            todo.to_ics_string()
        );
    }

    // Properties that hold their RFC 5545 default are omitted, mirroring the
    // event codec, so a parsed-then-written task yields the bytes it came from.
    #[test]
    fn rfc_default_status_and_sequence_are_omitted() {
        let mut todo = Todo::new("Plain");
        todo.status = TodoStatus::NeedsAction;
        todo.sequence = 0;

        let ics = todo.to_ics_string();

        assert!(!ics.contains("STATUS"), "{ics}");
        assert!(!ics.contains("SEQUENCE"), "{ics}");

        todo.status = TodoStatus::Cancelled;
        todo.sequence = 4;
        let ics = todo.to_ics_string();
        assert!(ics.contains("STATUS:CANCELLED\r\n"), "{ics}");
        assert!(ics.contains("SEQUENCE:4\r\n"), "{ics}");
    }

    // An unset optional property emits no line at all — never an empty one,
    // and never a synthesized value (R46).
    #[test]
    fn unset_properties_emit_no_line() {
        let mut todo = Todo::new("Bare");
        todo.summary = None;

        let ics = todo.to_ics_string();

        for absent in [
            "SUMMARY",
            "DESCRIPTION",
            "LOCATION",
            "URL",
            "DTSTART",
            "DUE",
            "COMPLETED",
            "PERCENT-COMPLETE",
            "PRIORITY",
            "CREATED",
            "LAST-MODIFIED",
            "RRULE",
            "EXDATE",
            "RDATE",
            "RECURRENCE-ID",
            "ORGANIZER",
            "ATTENDEE",
            "ATTACH",
            "VALARM",
            "CLASS",
        ] {
            assert!(!ics.contains(absent), "{absent} must be absent: {ics}");
        }
    }

    // COMPLETED must not go through `Todo::completed()`, which takes a
    // `DateTime<Utc>`: a floating value is legal (R22) and would be coerced
    // against the machine's zone or dropped.
    #[test]
    fn a_floating_completed_is_emitted_without_a_z_suffix() {
        let mut todo = Todo::new("Done, locally");
        todo.completed = Some(EventTime::DateTimeFloating(
            chrono::NaiveDate::from_ymd_opt(2026, 8, 14)
                .unwrap()
                .and_hms_opt(12, 0, 0)
                .unwrap(),
        ));

        let ics = todo.to_ics_string();

        assert!(ics.contains("COMPLETED:20260814T120000\r\n"), "{ics}");
        assert!(!ics.contains("COMPLETED:20260814T120000Z"), "{ics}");
    }

    // A zoned DUE keeps its TZID (R21). Whether caldir also emits a matching
    // VTIMEZONE is Q-3 and deliberately NOT decided here.
    #[test]
    fn a_zoned_due_keeps_its_tzid() {
        let mut todo = Todo::new("Deadline");
        todo.due = Some(EventTime::DateTimeZoned {
            datetime: chrono::NaiveDate::from_ymd_opt(2026, 8, 15)
                .unwrap()
                .and_hms_opt(23, 59, 0)
                .unwrap(),
            tzid: "Europe/Stockholm".to_string(),
        });

        let ics = todo.to_ics_string();

        assert!(
            ics.contains("DUE;TZID=Europe/Stockholm:20260815T235900\r\n"),
            "{ics}"
        );
    }

    // T-11 (R8, R26). DTEND is RFC-invalid on a VTODO and reaches
    // `icalendar::Todo` through TWO traits, not one: `Component::get_end`
    // (components.rs:199) is the getter and `EventLike::ends`
    // (components.rs:394) is the setter, and both `component_impl! { Todo }`
    // (:664) and `event_impl! { Todo }` (:665) apply. The only defence that
    // survives a refactor is an assertion on the EMITTED BYTES.
    #[test]
    fn serializing_a_task_never_emits_dtend() {
        // The trap is live. `EventLike` really is implemented for
        // `icalendar::Todo` on the pinned 0.17.10, and calling the FORBIDDEN
        // `ends()` really does put a `DTEND:` line into the emitted bytes.
        // Without this the assertions below could pass vacuously against a
        // crate with no way to emit DTEND at all. This is the ONLY place in
        // caldir where `EventLike` is in scope for a Todo, and it is scoped to
        // this block so it can never reach the production emitter.
        {
            use icalendar::EventLike;
            let mut leaked = icalendar::Todo::new();
            leaked.uid("trap@caldir");
            leaked.ends(Utc.with_ymd_and_hms(2026, 8, 15, 12, 0, 0).unwrap());
            assert!(
                leaked.done().to_string().contains("DTEND:"),
                "the DTEND leak must be real for this test to mean anything"
            );
        }

        let todo = dated_task();
        let ics = todo.to_ics_string();

        // A task with BOTH anchors: the shape where a naive emitter is most
        // tempted to map `due` onto `ends()`.
        assert!(
            ics.contains("DTSTART:20260810T090000Z\r\n"),
            "DTSTART must be emitted: {ics}"
        );
        assert!(
            ics.contains("DUE:20260815T235900Z\r\n"),
            "DUE must be emitted: {ics}"
        );

        assert!(!ics.contains("DTEND"), "DTEND must never be emitted: {ics}");
        assert!(
            !ics.contains("DURATION"),
            "DURATION must never be emitted: {ics}"
        );

        // And the getter half of the leak reads nothing either.
        let component: icalendar::Todo = (&todo).into();
        assert_eq!(component.get_end(), None);
    }
}
