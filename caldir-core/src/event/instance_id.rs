mod event_uid;
mod recurrence_id;

pub use event_uid::EventUid;
pub use recurrence_id::RecurrenceId;

use crate::EventTime;
use chrono::{NaiveDate, NaiveDateTime};
use std::fmt;

/// Which iCalendar component an identity belongs to. A VEVENT and a VTODO may
/// legitimately share a UID, so the kind is part of the key — without it they
/// collide in every state map, the diff, and the sync bases.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum ComponentKind {
    #[default]
    Event,
    Todo,
}

// (kind, UID, RecurrenceId) = the actual unique ID per item
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EventInstanceId {
    kind: ComponentKind,
    uid: EventUid,
    recurrence_id: Option<RecurrenceId>,
}

impl EventInstanceId {
    /// An event identity. Two-arg shape kept so every existing event call site
    /// reads unchanged; use `todo` / `with_kind` for tasks.
    pub fn new(uid: EventUid, recurrence_id: Option<RecurrenceId>) -> Self {
        Self::with_kind(ComponentKind::Event, uid, recurrence_id)
    }

    pub fn todo(uid: EventUid, recurrence_id: Option<RecurrenceId>) -> Self {
        Self::with_kind(ComponentKind::Todo, uid, recurrence_id)
    }

    pub fn with_kind(
        kind: ComponentKind,
        uid: EventUid,
        recurrence_id: Option<RecurrenceId>,
    ) -> Self {
        EventInstanceId {
            kind,
            uid,
            recurrence_id,
        }
    }

    pub fn kind(&self) -> ComponentKind {
        self.kind
    }

    pub fn uid(&self) -> &EventUid {
        &self.uid
    }

    pub fn recurrence_id(&self) -> Option<&RecurrenceId> {
        self.recurrence_id.as_ref()
    }
}

// Stable, round-trippable string form. Format:
//   non-recurring:        {uid}
//   recurring instance:   {uid}__{recurrence_id}
// Tasks carry a leading kind sigil; the event form is unprefixed and stays
// byte-identical to what caldir has always written to disk:
//   task:                 VTODO:{uid}[__{recurrence_id}]
// Recurrence id sub-formats:
//   YYYYMMDD                       — all-day
//   YYYYMMDDTHHMMSSZ               — UTC
//   YYYYMMDDTHHMMSS                — floating
//   TZID={tzid}:YYYYMMDDTHHMMSS    — zoned
//
// Conversion from `&str` is infallible: UIDs may legitimately contain `__`
// (RFC 5545 allows any text), so when the suffix after the last `__` isn't a
// parseable recurrence id, the whole string is treated as the UID.
//
// Same accepted ambiguity for the sigil: a VEVENT whose UID literally starts
// with `VTODO:` round-trips as a task. UID is TEXT, so no character is illegal
// and no encoding removes this — it is recorded, not escaped.
const RID_SEPARATOR: &str = "__";
const TZID_PREFIX: &str = "TZID=";
const TODO_SIGIL: &str = "VTODO:";

impl fmt::Display for EventInstanceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sigil = match self.kind() {
            ComponentKind::Event => "",
            ComponentKind::Todo => TODO_SIGIL,
        };
        let uid_str = self.uid().as_str();
        match self.recurrence_id() {
            Some(rid) => write!(
                f,
                "{}{}{}{}",
                sigil,
                uid_str,
                RID_SEPARATOR,
                format_recurrence_id(rid.as_event_time())
            ),
            None => write!(f, "{}{}", sigil, uid_str),
        }
    }
}

impl From<&str> for EventInstanceId {
    fn from(s: &str) -> Self {
        // Sigil first, then the rid split. (`rsplit_once` takes the *last*
        // `__` and the sigil holds none, so the two orders agree today — this
        // order keeps holding if the split ever becomes non-greedy.)
        let (kind, rest) = match s.strip_prefix(TODO_SIGIL) {
            Some(rest) => (ComponentKind::Todo, rest),
            None => (ComponentKind::Event, s),
        };

        if let Some((uid_str, rid_str)) = rest.rsplit_once(RID_SEPARATOR)
            && let Some(event_time) = parse_recurrence_id(rid_str)
        {
            return EventInstanceId::with_kind(
                kind,
                EventUid::new(uid_str),
                Some(RecurrenceId::from_event_time(event_time)),
            );
        }

        EventInstanceId::with_kind(kind, EventUid::new(rest), None)
    }
}

impl From<String> for EventInstanceId {
    fn from(s: String) -> Self {
        EventInstanceId::from(s.as_str())
    }
}

fn format_recurrence_id(event_time: &EventTime) -> String {
    match event_time {
        EventTime::Date(date) => date.format("%Y%m%d").to_string(),
        EventTime::DateTimeUtc(datetime) => datetime.format("%Y%m%dT%H%M%SZ").to_string(),
        EventTime::DateTimeFloating(datetime) => datetime.format("%Y%m%dT%H%M%S").to_string(),
        EventTime::DateTimeZoned { datetime, tzid } => {
            format!("{TZID_PREFIX}{tzid}:{}", datetime.format("%Y%m%dT%H%M%S"))
        }
    }
}

fn parse_recurrence_id(s: &str) -> Option<EventTime> {
    if let Some(rest) = s.strip_prefix(TZID_PREFIX) {
        let (tzid, dt_str) = rest.split_once(':')?;
        let datetime = NaiveDateTime::parse_from_str(dt_str, "%Y%m%dT%H%M%S").ok()?;

        return Some(EventTime::DateTimeZoned {
            datetime,
            tzid: tzid.to_string(),
        });
    }

    if s.ends_with('Z') {
        let datetime = NaiveDateTime::parse_from_str(s, "%Y%m%dT%H%M%SZ").ok()?;
        return Some(EventTime::DateTimeUtc(datetime.and_utc()));
    }

    if !s.contains('T') {
        let date = NaiveDate::parse_from_str(s, "%Y%m%d").ok()?;
        return Some(EventTime::Date(date));
    }

    let datetime = NaiveDateTime::parse_from_str(s, "%Y%m%dT%H%M%S").ok()?;
    Some(EventTime::DateTimeFloating(datetime))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use pretty_assertions::assert_eq;

    fn uid() -> EventUid {
        EventUid::new("abc123@hooli.com".to_string())
    }

    fn assert_round_trip(id: &EventInstanceId, expected_str: &str) {
        let s = id.to_string();
        assert_eq!(s, expected_str);
        let parsed = EventInstanceId::from(s);
        assert_eq!(&parsed, id);
    }

    #[test]
    fn round_trips_non_recurring_event() {
        let id = EventInstanceId::new(uid(), None);
        assert_round_trip(&id, "abc123@hooli.com");
    }

    #[test]
    fn round_trips_date_recurrence_id() {
        let id = EventInstanceId::new(
            uid(),
            Some(RecurrenceId::from_event_time(EventTime::Date(
                NaiveDate::from_ymd_opt(2026, 1, 9).unwrap(),
            ))),
        );
        assert_round_trip(&id, "abc123@hooli.com__20260109");
    }

    #[test]
    fn round_trips_utc_recurrence_id() {
        let id = EventInstanceId::new(
            uid(),
            Some(RecurrenceId::from_event_time(EventTime::DateTimeUtc(
                chrono::Utc.with_ymd_and_hms(2026, 1, 1, 17, 0, 0).unwrap(),
            ))),
        );
        assert_round_trip(&id, "abc123@hooli.com__20260101T170000Z");
    }

    #[test]
    fn round_trips_floating_recurrence_id() {
        let datetime = NaiveDate::from_ymd_opt(2026, 1, 9)
            .unwrap()
            .and_hms_opt(10, 0, 0)
            .unwrap();
        let id = EventInstanceId::new(
            uid(),
            Some(RecurrenceId::from_event_time(EventTime::DateTimeFloating(
                datetime,
            ))),
        );
        assert_round_trip(&id, "abc123@hooli.com__20260109T100000");
    }

    #[test]
    fn round_trips_zoned_recurrence_id() {
        let datetime = NaiveDate::from_ymd_opt(2026, 1, 9)
            .unwrap()
            .and_hms_opt(10, 0, 0)
            .unwrap();
        let id = EventInstanceId::new(
            uid(),
            Some(RecurrenceId::from_event_time(EventTime::DateTimeZoned {
                datetime,
                tzid: "Europe/Stockholm".to_string(),
            })),
        );
        assert_round_trip(
            &id,
            "abc123@hooli.com__TZID=Europe/Stockholm:20260109T100000",
        );
    }

    #[test]
    fn round_trips_uid_containing_double_underscore_with_recurrence_id() {
        // Some providers emit UIDs with embedded "__" — rsplit_once on the
        // last occurrence keeps the rid intact.
        let weird_uid = EventUid::new("event__weird@example.com".to_string());
        let id = EventInstanceId::new(
            weird_uid,
            Some(RecurrenceId::from_event_time(EventTime::Date(
                NaiveDate::from_ymd_opt(2026, 1, 9).unwrap(),
            ))),
        );
        assert_round_trip(&id, "event__weird@example.com__20260109");
    }

    #[test]
    fn round_trips_uid_containing_double_underscore_without_recurrence_id() {
        // Without a trailing rid, the suffix after the last "__" isn't a
        // valid recurrence id, so the whole string is the UID.
        let id = EventInstanceId::new(EventUid::new("event__weird@example.com".to_string()), None);
        assert_round_trip(&id, "event__weird@example.com");
    }

    #[test]
    fn treats_malformed_recurrence_suffix_as_part_of_uid() {
        let id = EventInstanceId::from("abc__not-a-date");
        assert_eq!(id.uid().as_str(), "abc__not-a-date");
        assert!(id.recurrence_id().is_none());
    }

    #[test]
    fn treats_zoned_suffix_without_datetime_as_part_of_uid() {
        let id = EventInstanceId::from("abc__TZID=Europe/Stockholm");
        assert_eq!(id.uid().as_str(), "abc__TZID=Europe/Stockholm");
        assert!(id.recurrence_id().is_none());
    }

    // T-20 (R36/R37). Identity is a HashMap key across the sync engine and the
    // state files, so a non-injective round trip silently merges two items —
    // R36 hazards 1-4. Covers all four rid sub-forms, a UID containing "__",
    // and the pathological `UID:VTODO:x` event R37 explicitly accepts.
    #[test]
    fn parse_of_format_is_identity_for_both_kinds() {
        let date = EventTime::Date(NaiveDate::from_ymd_opt(2026, 1, 9).unwrap());
        let utc =
            EventTime::DateTimeUtc(chrono::Utc.with_ymd_and_hms(2026, 1, 1, 17, 0, 0).unwrap());
        let floating = EventTime::DateTimeFloating(
            NaiveDate::from_ymd_opt(2026, 1, 9)
                .unwrap()
                .and_hms_opt(10, 0, 0)
                .unwrap(),
        );
        let zoned = EventTime::DateTimeZoned {
            datetime: NaiveDate::from_ymd_opt(2026, 1, 9)
                .unwrap()
                .and_hms_opt(10, 0, 0)
                .unwrap(),
            tzid: "Europe/Stockholm".to_string(),
        };

        let rid = |t: &EventTime| Some(RecurrenceId::from_event_time(t.clone()));

        let cases: Vec<(EventInstanceId, &str)> = vec![
            // Event forms — byte-identical to the pre-ComponentKind strings.
            (
                EventInstanceId::new(EventUid::new("abc123@hooli.com"), None),
                "abc123@hooli.com",
            ),
            (
                EventInstanceId::new(EventUid::new("abc123@hooli.com"), rid(&date)),
                "abc123@hooli.com__20260109",
            ),
            (
                EventInstanceId::new(EventUid::new("abc123@hooli.com"), rid(&utc)),
                "abc123@hooli.com__20260101T170000Z",
            ),
            (
                EventInstanceId::new(EventUid::new("abc123@hooli.com"), rid(&floating)),
                "abc123@hooli.com__20260109T100000",
            ),
            (
                EventInstanceId::new(EventUid::new("abc123@hooli.com"), rid(&zoned)),
                "abc123@hooli.com__TZID=Europe/Stockholm:20260109T100000",
            ),
            (
                EventInstanceId::new(EventUid::new("event__weird@example.com"), rid(&date)),
                "event__weird@example.com__20260109",
            ),
            (
                EventInstanceId::new(EventUid::new("event__weird@example.com"), None),
                "event__weird@example.com",
            ),
            // Task forms — the same shapes behind the VTODO: sigil.
            (
                EventInstanceId::todo(EventUid::new("abc123@hooli.com"), None),
                "VTODO:abc123@hooli.com",
            ),
            (
                EventInstanceId::todo(EventUid::new("abc123@hooli.com"), rid(&date)),
                "VTODO:abc123@hooli.com__20260109",
            ),
            (
                EventInstanceId::todo(EventUid::new("abc123@hooli.com"), rid(&utc)),
                "VTODO:abc123@hooli.com__20260101T170000Z",
            ),
            (
                EventInstanceId::todo(EventUid::new("abc123@hooli.com"), rid(&floating)),
                "VTODO:abc123@hooli.com__20260109T100000",
            ),
            (
                EventInstanceId::todo(EventUid::new("abc123@hooli.com"), rid(&zoned)),
                "VTODO:abc123@hooli.com__TZID=Europe/Stockholm:20260109T100000",
            ),
            // Sigil is stripped BEFORE the rid split, so a task UID containing
            // "__" survives intact.
            (
                EventInstanceId::todo(EventUid::new("task__weird@example.com"), rid(&date)),
                "VTODO:task__weird@example.com__20260109",
            ),
            (
                EventInstanceId::todo(EventUid::new("task__weird@example.com"), None),
                "VTODO:task__weird@example.com",
            ),
        ];

        for (id, expected) in &cases {
            assert_eq!(&id.to_string(), expected, "format of {id:?}");
            assert_eq!(
                &EventInstanceId::from(expected.to_string()),
                id,
                "parse of {expected}"
            );
        }

        // Kinds are distinct identities: same uid + same rid, different key.
        let event = EventInstanceId::new(EventUid::new("shared@u-crawler"), None);
        let task = EventInstanceId::todo(EventUid::new("shared@u-crawler"), None);
        assert_ne!(event, task);
        assert_eq!(event.kind(), ComponentKind::Event);
        assert_eq!(task.kind(), ComponentKind::Todo);

        // ACCEPTED RESIDUAL (R37): a VEVENT whose UID literally starts with
        // "VTODO:" formats byte-identically to today and therefore re-parses as
        // a task. UID is TEXT per RFC 5545, so no encoding removes this — it is
        // the same ambiguity class already accepted for "__" above.
        let pathological = EventInstanceId::new(EventUid::new("VTODO:x"), None);
        assert_eq!(pathological.to_string(), "VTODO:x");
        assert_eq!(
            EventInstanceId::from("VTODO:x"),
            EventInstanceId::todo(EventUid::new("x"), None)
        );
    }

    // C4 downgrade half (ADJUDICATION B-4): an OLDER caldir has no sigil, so it
    // reads a "VTODO:…" line as one opaque UID. That id is inert — see
    // `diff::calendar_diff::tests::an_id_known_only_in_the_bases_triggers_no_delete`
    // for the delete half, and `state::event_bases` for the filename half.
    #[test]
    fn an_older_caldir_reads_a_task_line_as_one_opaque_event_uid() {
        // What today's code writes for a task:
        let task = EventInstanceId::todo(EventUid::new("assignment-123@u-crawler"), None);
        let line = task.to_string();
        assert_eq!(line, "VTODO:assignment-123@u-crawler");

        // What an older caldir (no sigil handling) makes of that line: the
        // whole literal is the UID, kind Event.
        let downgraded = EventInstanceId::new(EventUid::new(line.clone()), None);
        assert_eq!(downgraded.uid().as_str(), "VTODO:assignment-123@u-crawler");
        assert_eq!(downgraded.kind(), ComponentKind::Event);

        // It cannot collide with a real event id: the string an older caldir
        // hashes for its base filename still contains the sigil, so it differs
        // from every event whose UID does not literally start with "VTODO:".
        let real_event = EventInstanceId::new(EventUid::new("assignment-123@u-crawler"), None);
        assert_ne!(downgraded.to_string(), real_event.to_string());
        assert_eq!(downgraded.to_string(), line);
    }

    #[test]
    fn kind_defaults_to_event() {
        assert_eq!(ComponentKind::default(), ComponentKind::Event);
    }
}
