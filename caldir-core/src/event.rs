mod attachment;
mod attendee;
mod availability;
pub(crate) mod equality;
mod error;
mod from_icalendar;
mod instance_id;
mod occurrences;
mod organizer;
mod recurrence;
mod reminder;
pub(crate) mod slugify;
mod status;
mod time;
mod to_icalendar;
pub mod tz_normalize;
mod visibility;
mod x_property;

// Test-only: the last production narrowing now lives in
// `CalendarItem::into_event`.
#[cfg(test)]
use crate::item::CalendarItem;
pub use attachment::Attachment;
pub use attendee::{Attendee, ParticipationStatus};
pub use availability::Availability;
use chrono::{DateTime, Utc};
use equality::{attachments_eq, x_properties_eq};
pub use error::EventError;
pub use instance_id::{ComponentKind, EventInstanceId, EventUid, RecurrenceId};
pub use occurrences::expand_in_range;
pub use organizer::Organizer;
pub use recurrence::Recurrence;
pub use reminder::Reminder;
pub use status::Status;
pub use time::EventTime;
pub use visibility::Visibility;
pub use x_property::XProperty;

pub(crate) const ICS_PRODID: &str = "CALDIR";
pub(crate) const ICS_VERSION: &str = "2.0";
const ICS_UID_DOMAIN: &str = "caldir";

#[derive(Debug, Clone, Eq, educe::Educe)]
#[educe(PartialEq)]
pub struct Event {
    pub uid: EventUid,
    pub summary: Option<String>,
    pub description: Option<String>,
    pub location: Option<String>,
    pub start: EventTime,
    pub end: Option<EventTime>,
    pub status: Status,
    pub availability: Availability,
    pub visibility: Option<Visibility>,
    pub recurrence: Option<Recurrence>,
    pub recurrence_id: Option<RecurrenceId>,
    pub organizer: Option<Organizer>,
    pub attendees: Vec<Attendee>,
    pub reminders: Vec<Reminder>,
    pub url: Option<String>,

    #[educe(PartialEq(method(attachments_eq)))]
    pub attachments: Vec<Attachment>,

    #[educe(PartialEq(method(x_properties_eq)))]
    pub x_properties: Vec<XProperty>,

    #[educe(PartialEq(ignore))]
    pub last_modified: Option<DateTime<Utc>>,

    #[educe(PartialEq(ignore))]
    pub sequence: i32,
}

impl Event {
    pub fn new(summary: impl Into<String>, start: EventTime) -> Self {
        Event {
            uid: new_uid(),
            summary: Some(summary.into()),
            description: None,
            location: None,
            start,
            end: None,
            status: Status::default(),
            availability: Availability::default(),
            visibility: None,
            recurrence: None,
            recurrence_id: None,
            last_modified: None,
            sequence: 0,
            organizer: None,
            attendees: Vec::new(),
            reminders: Vec::new(),
            url: None,
            attachments: Vec::new(),
            x_properties: Vec::new(),
        }
    }

    pub fn to_ics_string(&self) -> String {
        let ical_event: icalendar::Event = self.into();

        let ics = icalendar::Calendar::empty()
            .append_property(icalendar::Property::new("VERSION", ICS_VERSION))
            .append_property(icalendar::Property::new("PRODID", ICS_PRODID))
            .push(ical_event)
            .done()
            .to_string();

        self.splice_valarms_into_vevent(ics)
    }

    pub fn occurs_in_range(&self, from: DateTime<Utc>, to: DateTime<Utc>) -> bool {
        let event_start = self.start.to_utc();
        let event_end = self.end.as_ref().unwrap_or(&self.start).to_utc();

        // Check if event overlaps with the range [from, to]
        event_start < to && event_end > from
    }

    /// Like occurs_in_range(), but expands the RRULE for recurring events
    /// so a master with `UNTIL` in the past correctly reports no overlap.
    pub fn has_occurrence_in_range(&self, from: DateTime<Utc>, to: DateTime<Utc>) -> bool {
        if self.recurrence.is_some() {
            !occurrences::expand_master(self, from, to, &std::collections::HashMap::new())
                .is_empty()
        } else {
            self.occurs_in_range(from, to)
        }
    }

    /// True if email is an attendee but NOT the organizer
    pub fn is_invite_for(&self, email: &str) -> bool {
        let is_attendee = self.find_attendee(email).is_some();

        let is_organizer = self
            .organizer
            .as_ref()
            .is_some_and(|o| o.email.eq_ignore_ascii_case(email));

        is_attendee && !is_organizer
    }

    /// Get the user's participation status for this event
    pub fn attendee_status(&self, email: &str) -> Option<ParticipationStatus> {
        self.find_attendee(email)?.status
    }

    /// Find the attendee matching the given email (case-insensitive)
    pub fn find_attendee(&self, email: &str) -> Option<&Attendee> {
        self.attendees
            .iter()
            .find(|a| a.email.eq_ignore_ascii_case(email))
    }

    pub fn set_attendee_status(
        &mut self,
        email: &str,
        status: ParticipationStatus,
    ) -> Result<(), EventError> {
        let maybe_attendee = self
            .attendees
            .iter_mut()
            .find(|a| a.email.eq_ignore_ascii_case(email));

        match maybe_attendee {
            Some(attendee) => {
                attendee.status = Some(status);
                Ok(())
            }
            None => Err(EventError::AttendeeNotFound {
                email: email.to_string(),
            }),
        }
    }

    pub fn is_pending_invite_for(&self, email: &str) -> bool {
        self.is_invite_for(email)
            && self.attendee_status(email) == Some(ParticipationStatus::NeedsAction)
    }

    /// Return a clone of this event with a fresh UID.
    pub fn with_new_uid(&self) -> Self {
        Event {
            uid: new_uid(),
            ..self.clone()
        }
    }

    /// Find the first x-property value matching the given name.
    pub fn x_property(&self, name: &str) -> Option<&str> {
        self.x_properties
            .iter()
            .find(|x| x.name == name)
            .map(|x| x.value.as_str())
    }

    #[cfg(test)]
    pub(crate) fn add_x_property(
        mut self,
        name: impl Into<String>,
        value: impl Into<String>,
    ) -> Self {
        self.x_properties
            .push(XProperty::new(name.into(), value.into()));

        self
    }

    /// Parse an ICS document expected to contain exactly one valid event.
    /// Panics on any deviation — for use in tests with known-good fixtures.
    #[cfg(test)]
    pub(crate) fn parse_single_ics(contents: &str) -> Event {
        CalendarItem::from_single_ics_str(contents)
            .and_then(CalendarItem::into_event)
            .expect("expected exactly one valid event in ICS")
    }

    // icalendar library adds UID to every VALARM
    // we don't want that, so we construct them with `Reminder::ics_block` instead:
    fn splice_valarms_into_vevent(&self, ics: String) -> String {
        if self.reminders.is_empty() {
            return ics;
        }

        let valarms: String = self.reminders.iter().map(Reminder::ics_block).collect();

        ics.replacen("END:VEVENT\r\n", &format!("{valarms}END:VEVENT\r\n"), 1)
    }

    pub fn event_instance_id(&self) -> EventInstanceId {
        EventInstanceId::new(self.uid.clone(), self.recurrence_id.clone())
    }

    /// Make sure local copy doesn't strip provider-managed metadata from remote
    pub fn with_x_properties_merged_from(mut self, base: &Event) -> Self {
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

pub(crate) fn new_uid() -> EventUid {
    let uid = format!("{}@{}", uuid::Uuid::new_v4(), ICS_UID_DOMAIN);
    EventUid::new(uid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use pretty_assertions::assert_eq;

    #[test]
    fn new_generates_uid_with_caldir_domain() {
        let event = Event::new(
            "Test",
            time::EventTime::Date(chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()),
        );

        assert!(event.uid.as_str().ends_with("@caldir"));
        let prefix = event.uid.as_str().trim_end_matches("@caldir");
        assert!(uuid::Uuid::parse_str(prefix).is_ok());
    }

    #[test]
    fn new_generates_unique_uids() {
        let start = time::EventTime::Date(chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap());
        let a = Event::new("Test", start.clone());
        let b = Event::new("Test", start);

        assert_ne!(a.uid, b.uid);
    }

    #[test]
    fn rejects_invalid_ics() {
        // Missing "END:VCALENDAR"
        let ics = r"BEGIN:VCALENDAR
VERSION:2.0
BEGIN:VEVENT
UID:test-uid@caldir
DTSTART:20240101T120000Z
SUMMARY:Test Event
END:VEVENT"
            .replace('\n', "\r\n");

        let result = CalendarItem::from_ics_str(&ics);
        assert!(matches!(result, Err(EventError::InvalidIcs(_, _))));
    }

    #[test]
    fn rejects_event_without_start() {
        let result = Event::try_from(&icalendar::Event::new().done());

        assert!(matches!(result, Err(EventError::MissingStart)));
    }

    #[test]
    fn normalizes_windows_tzid_to_iana_on_parse_and_write() {
        let ics = r"BEGIN:VCALENDAR
VERSION:2.0
BEGIN:VEVENT
UID:test-uid@caldir
DTSTART;TZID=Pacific Standard Time:20240101T120000
SUMMARY:Test
END:VEVENT
END:VCALENDAR"
            .replace('\n', "\r\n");

        let event = Event::parse_single_ics(&ics);

        assert_eq!(
            event.start,
            EventTime::DateTimeZoned {
                datetime: chrono::NaiveDate::from_ymd_opt(2024, 1, 1)
                    .unwrap()
                    .and_hms_opt(12, 0, 0)
                    .unwrap(),
                tzid: "America/Los_Angeles".to_string(),
            }
        );
        assert!(
            event
                .to_ics_string()
                .contains("DTSTART;TZID=America/Los_Angeles:20240101T120000")
        );
    }

    #[test]
    fn normalizes_offset_tzid_preserving_instant_and_round_trip() {
        let ics = r"BEGIN:VCALENDAR
VERSION:2.0
BEGIN:VEVENT
UID:test-uid@caldir
DTSTART;TZID=GMT+0100:20260724T190200
SUMMARY:Trip to Sheffield
END:VEVENT
END:VCALENDAR"
            .replace('\n', "\r\n");

        let event = Event::parse_single_ics(&ics);

        assert_eq!(
            event.start,
            EventTime::DateTimeZoned {
                datetime: chrono::NaiveDate::from_ymd_opt(2026, 7, 24)
                    .unwrap()
                    .and_hms_opt(19, 2, 0)
                    .unwrap(),
                tzid: "Etc/GMT-1".to_string(),
            }
        );
        assert_eq!(
            event.start.to_utc(),
            DateTime::parse_from_rfc3339("2026-07-24T18:02:00Z")
                .unwrap()
                .with_timezone(&Utc)
        );

        let serialized = event.to_ics_string();
        assert!(serialized.contains("DTSTART;TZID=Etc/GMT-1:20260724T190200"));
        assert_eq!(Event::parse_single_ics(&serialized), event);
    }

    #[test]
    fn to_ics_string_sets_calendar_headers() {
        let event = Event::new(
            "Test",
            EventTime::Date(chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()),
        );

        let ics = event.to_ics_string();

        assert!(ics.contains("VERSION:2.0"));
        assert!(ics.contains("PRODID:CALDIR"));
    }

    #[test]
    fn to_ics_string_updates_dtstamp() {
        let original_ics = r"BEGIN:VCALENDAR
VERSION:2.0
BEGIN:VEVENT
UID:test@caldir
DTSTAMP:20200101T000000Z
DTSTART:20260101
SUMMARY:Test
END:VEVENT
END:VCALENDAR
"
        .replace('\n', "\r\n");

        let event = Event::parse_single_ics(&original_ics);
        let serialized = event.to_ics_string();

        assert_ne!(dtstamp_line(&original_ics), dtstamp_line(&serialized));
    }

    #[test]
    fn round_trips_advanced_event_without_data_loss() {
        let original_ics = r"BEGIN:VCALENDAR
VERSION:2.0
PRODID:CALDIR
BEGIN:VEVENT
CLASS:CONFIDENTIAL
DTSTAMP:20260502T173914Z
DESCRIPTION:https://docs.example.com/document/d/abc123def456ghijklmnopqrstu
 v/edit?usp=sharing\n
DTEND;TZID=Europe/Oslo:20260515T164500
DTSTART;TZID=Europe/Oslo:20260515T160000
LAST-MODIFIED:20260502T173914Z
LOCATION:Conference Room A
ORGANIZER:mailto:alice@example.com
RECURRENCE-ID;TZID=Europe/Oslo:20260515T160000
RRULE:FREQ=WEEKLY;BYDAY=FR
SEQUENCE:1
SUMMARY:Friday retro
TRANSP:TRANSPARENT
UID:event-uid-123@example.com
URL:https://meet.example.com/abc-defg-hij
X-HOOLI-CONFERENCE:https://meet.example.com/abc-defg-hij
X-HOOLI-EVENT-ID:event-uid-123_20260515T140000Z
ATTENDEE;PARTSTAT=ACCEPTED:mailto:bob@example.com
ATTENDEE;PARTSTAT=DECLINED:mailto:alice@example.com
ATTENDEE;PARTSTAT=NEEDS-ACTION:mailto:carol@example.com
EXDATE;TZID=Europe/Oslo:20260522T160000
EXDATE;TZID=Europe/Oslo:20260529T160000
BEGIN:VALARM
ACTION:DISPLAY
DESCRIPTION:Reminder
TRIGGER;RELATED=START:-PT1H
END:VALARM
BEGIN:VALARM
ACTION:DISPLAY
DESCRIPTION:Reminder
TRIGGER;RELATED=START:-PT30M
END:VALARM
END:VEVENT
END:VCALENDAR
"
        .replace('\n', "\r\n");

        let event = Event::parse_single_ics(&original_ics);
        let serialized_ics = event.to_ics_string();

        assert_eq!(strip_dtstamp(&original_ics), strip_dtstamp(&serialized_ics));
    }

    #[test]
    fn occurs_in_range_returns_true_for_event_inside_range() {
        let mut event = Event::new(
            "Test",
            EventTime::DateTimeUtc(Utc.with_ymd_and_hms(2026, 5, 15, 12, 0, 0).unwrap()),
        );
        event.end = Some(EventTime::DateTimeUtc(
            Utc.with_ymd_and_hms(2026, 5, 15, 13, 0, 0).unwrap(),
        ));

        let from = Utc.with_ymd_and_hms(2026, 5, 15, 0, 0, 0).unwrap();
        let to = Utc.with_ymd_and_hms(2026, 5, 16, 0, 0, 0).unwrap();

        assert!(event.occurs_in_range(from, to));
    }

    #[test]
    fn occurs_in_range_returns_false_for_event_before_range() {
        let mut event = Event::new(
            "Test",
            EventTime::DateTimeUtc(Utc.with_ymd_and_hms(2026, 5, 14, 12, 0, 0).unwrap()),
        );
        event.end = Some(EventTime::DateTimeUtc(
            Utc.with_ymd_and_hms(2026, 5, 14, 13, 0, 0).unwrap(),
        ));

        let from = Utc.with_ymd_and_hms(2026, 5, 15, 0, 0, 0).unwrap();
        let to = Utc.with_ymd_and_hms(2026, 5, 16, 0, 0, 0).unwrap();

        assert!(!event.occurs_in_range(from, to));
    }

    #[test]
    fn occurs_in_range_returns_false_for_event_after_range() {
        let mut event = Event::new(
            "Test",
            EventTime::DateTimeUtc(Utc.with_ymd_and_hms(2026, 5, 17, 12, 0, 0).unwrap()),
        );
        event.end = Some(EventTime::DateTimeUtc(
            Utc.with_ymd_and_hms(2026, 5, 17, 13, 0, 0).unwrap(),
        ));

        let from = Utc.with_ymd_and_hms(2026, 5, 15, 0, 0, 0).unwrap();
        let to = Utc.with_ymd_and_hms(2026, 5, 16, 0, 0, 0).unwrap();

        assert!(!event.occurs_in_range(from, to));
    }

    #[test]
    fn occurs_in_range_returns_true_for_event_overlapping_range_start() {
        let mut event = Event::new(
            "Test",
            EventTime::DateTimeUtc(Utc.with_ymd_and_hms(2026, 5, 14, 23, 0, 0).unwrap()),
        );
        event.end = Some(EventTime::DateTimeUtc(
            Utc.with_ymd_and_hms(2026, 5, 15, 1, 0, 0).unwrap(),
        ));

        let from = Utc.with_ymd_and_hms(2026, 5, 15, 0, 0, 0).unwrap();
        let to = Utc.with_ymd_and_hms(2026, 5, 16, 0, 0, 0).unwrap();

        assert!(event.occurs_in_range(from, to));
    }

    #[test]
    fn occurs_in_range_returns_true_for_event_overlapping_range_end() {
        let mut event = Event::new(
            "Test",
            EventTime::DateTimeUtc(Utc.with_ymd_and_hms(2026, 5, 15, 23, 0, 0).unwrap()),
        );
        event.end = Some(EventTime::DateTimeUtc(
            Utc.with_ymd_and_hms(2026, 5, 16, 1, 0, 0).unwrap(),
        ));

        let from = Utc.with_ymd_and_hms(2026, 5, 15, 0, 0, 0).unwrap();
        let to = Utc.with_ymd_and_hms(2026, 5, 16, 0, 0, 0).unwrap();

        assert!(event.occurs_in_range(from, to));
    }

    #[test]
    fn occurs_in_range_uses_start_as_end_when_end_missing() {
        let event = Event::new(
            "Test",
            EventTime::DateTimeUtc(Utc.with_ymd_and_hms(2026, 5, 15, 12, 0, 0).unwrap()),
        );

        let from = Utc.with_ymd_and_hms(2026, 5, 15, 0, 0, 0).unwrap();
        let to = Utc.with_ymd_and_hms(2026, 5, 16, 0, 0, 0).unwrap();

        assert!(event.occurs_in_range(from, to));
    }

    #[test]
    fn occurs_in_range_excludes_event_ending_exactly_at_range_start() {
        let mut event = Event::new(
            "Test",
            EventTime::DateTimeUtc(Utc.with_ymd_and_hms(2026, 5, 14, 23, 0, 0).unwrap()),
        );
        event.end = Some(EventTime::DateTimeUtc(
            Utc.with_ymd_and_hms(2026, 5, 15, 0, 0, 0).unwrap(),
        ));

        let from = Utc.with_ymd_and_hms(2026, 5, 15, 0, 0, 0).unwrap();
        let to = Utc.with_ymd_and_hms(2026, 5, 16, 0, 0, 0).unwrap();

        assert!(!event.occurs_in_range(from, to));
    }

    #[test]
    fn occurs_in_range_excludes_event_starting_exactly_at_range_end() {
        let mut event = Event::new(
            "Test",
            EventTime::DateTimeUtc(Utc.with_ymd_and_hms(2026, 5, 16, 0, 0, 0).unwrap()),
        );
        event.end = Some(EventTime::DateTimeUtc(
            Utc.with_ymd_and_hms(2026, 5, 16, 1, 0, 0).unwrap(),
        ));

        let from = Utc.with_ymd_and_hms(2026, 5, 15, 0, 0, 0).unwrap();
        let to = Utc.with_ymd_and_hms(2026, 5, 16, 0, 0, 0).unwrap();

        assert!(!event.occurs_in_range(from, to));
    }

    #[test]
    fn is_invite_for_returns_true_when_attendee_and_not_organizer() {
        let mut event = Event::new(
            "Test",
            EventTime::Date(chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()),
        );
        event.organizer = Some(Organizer::new("alice@example.com"));
        event.attendees = vec![Attendee::new("bob@example.com")];

        assert!(event.is_invite_for("bob@example.com"));
    }

    #[test]
    fn is_invite_for_returns_false_when_email_is_organizer() {
        let mut event = Event::new(
            "Test",
            EventTime::Date(chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()),
        );
        event.organizer = Some(Organizer::new("alice@example.com"));
        event.attendees = vec![Attendee::new("alice@example.com")];

        assert!(!event.is_invite_for("alice@example.com"));
    }

    #[test]
    fn is_invite_for_returns_false_when_email_is_not_an_attendee() {
        let mut event = Event::new(
            "Test",
            EventTime::Date(chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()),
        );
        event.organizer = Some(Organizer::new("alice@example.com"));
        event.attendees = vec![Attendee::new("bob@example.com")];

        assert!(!event.is_invite_for("carol@example.com"));
    }

    #[test]
    fn is_invite_for_returns_false_when_event_has_no_attendees() {
        let event = Event::new(
            "Test",
            EventTime::Date(chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()),
        );

        assert!(!event.is_invite_for("bob@example.com"));
    }

    #[test]
    fn is_invite_for_returns_true_when_organizer_is_missing() {
        let mut event = Event::new(
            "Test",
            EventTime::Date(chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()),
        );
        event.attendees = vec![Attendee::new("bob@example.com")];

        assert!(event.is_invite_for("bob@example.com"));
    }

    #[test]
    fn is_invite_for_matches_email_case_insensitively() {
        let mut event = Event::new(
            "Test",
            EventTime::Date(chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()),
        );
        event.organizer = Some(Organizer::new("alice@example.com"));
        event.attendees = vec![Attendee::new("Bob@Example.com")];

        assert!(event.is_invite_for("bob@example.com"));
        assert!(event.is_invite_for("BOB@EXAMPLE.COM"));
    }

    #[test]
    fn is_invite_for_matches_organizer_email_case_insensitively() {
        let mut event = Event::new(
            "Test",
            EventTime::Date(chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()),
        );
        event.organizer = Some(Organizer::new("Alice@Example.com"));
        event.attendees = vec![Attendee::new("alice@example.com")];

        assert!(!event.is_invite_for("ALICE@example.com"));
    }

    #[test]
    fn attendee_status_returns_status_for_matching_attendee() {
        let mut event = Event::new(
            "Test",
            EventTime::Date(chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()),
        );
        event.attendees = vec![Attendee {
            email: "bob@example.com".to_string(),
            name: None,
            status: Some(ParticipationStatus::Accepted),
        }];

        assert_eq!(
            event.attendee_status("bob@example.com"),
            Some(ParticipationStatus::Accepted)
        );
    }

    #[test]
    fn attendee_status_returns_none_when_email_is_not_an_attendee() {
        let mut event = Event::new(
            "Test",
            EventTime::Date(chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()),
        );
        event.attendees = vec![Attendee {
            email: "bob@example.com".to_string(),
            name: None,
            status: Some(ParticipationStatus::Accepted),
        }];

        assert_eq!(event.attendee_status("carol@example.com"), None);
    }

    #[test]
    fn attendee_status_returns_none_when_attendee_has_no_status() {
        let mut event = Event::new(
            "Test",
            EventTime::Date(chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()),
        );
        event.attendees = vec![Attendee::new("bob@example.com")];

        assert_eq!(event.attendee_status("bob@example.com"), None);
    }

    #[test]
    fn set_attendee_status_updates_matching_attendee() {
        let mut event = Event::new(
            "Test",
            EventTime::Date(chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()),
        );
        event.attendees = vec![Attendee::new("bob@example.com")];

        let result = event.set_attendee_status("BOB@example.com", ParticipationStatus::Accepted);

        assert!(result.is_ok());
        assert_eq!(
            event.attendee_status("bob@example.com"),
            Some(ParticipationStatus::Accepted)
        );
    }

    #[test]
    fn set_attendee_status_errors_when_email_is_not_an_attendee() {
        let mut event = Event::new(
            "Test",
            EventTime::Date(chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()),
        );
        event.attendees = vec![Attendee::new("bob@example.com")];

        let result = event.set_attendee_status("carol@example.com", ParticipationStatus::Accepted);

        assert!(matches!(
            result,
            Err(EventError::AttendeeNotFound { email }) if email == "carol@example.com"
        ));
    }

    #[test]
    fn with_new_uid_changes_the_uid() {
        let mut event = Event::new(
            "Test",
            EventTime::Date(chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()),
        );
        event.uid = EventUid::new("original@caldir");

        let cloned = event.with_new_uid();

        assert_ne!(cloned.uid, event.uid);
    }

    #[test]
    fn with_new_uid_preserves_other_fields() {
        let mut event = Event::new(
            "Annual review",
            EventTime::Date(chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()),
        );
        event.description = Some("notes".to_string());
        event.location = Some("HQ".to_string());

        let cloned = event.with_new_uid();

        assert_eq!(cloned.summary, event.summary);
        assert_eq!(cloned.description, event.description);
        assert_eq!(cloned.location, event.location);
        assert_eq!(cloned.start, event.start);
    }

    #[test]
    fn is_pending_invite_for_true_when_attendee_needs_action() {
        let mut event = Event::new(
            "Test",
            EventTime::Date(chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()),
        );
        event.organizer = Some(Organizer::new("alice@example.com"));
        event.attendees = vec![Attendee {
            email: "bob@example.com".to_string(),
            name: None,
            status: Some(ParticipationStatus::NeedsAction),
        }];

        assert!(event.is_pending_invite_for("bob@example.com"));
    }

    #[test]
    fn is_pending_invite_for_false_when_already_accepted() {
        let mut event = Event::new(
            "Test",
            EventTime::Date(chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()),
        );
        event.attendees = vec![Attendee {
            email: "bob@example.com".to_string(),
            name: None,
            status: Some(ParticipationStatus::Accepted),
        }];

        assert!(!event.is_pending_invite_for("bob@example.com"));
    }

    #[test]
    fn is_pending_invite_for_false_when_email_is_organizer() {
        let mut event = Event::new(
            "Test",
            EventTime::Date(chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()),
        );
        event.organizer = Some(Organizer::new("alice@example.com"));
        event.attendees = vec![Attendee {
            email: "alice@example.com".to_string(),
            name: None,
            status: Some(ParticipationStatus::NeedsAction),
        }];

        assert!(!event.is_pending_invite_for("alice@example.com"));
    }

    #[test]
    fn attendee_status_matches_email_case_insensitively() {
        let mut event = Event::new(
            "Test",
            EventTime::Date(chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()),
        );
        event.attendees = vec![Attendee {
            email: "Bob@Example.com".to_string(),
            name: None,
            status: Some(ParticipationStatus::Tentative),
        }];

        assert_eq!(
            event.attendee_status("bob@example.com"),
            Some(ParticipationStatus::Tentative)
        );
    }

    #[test]
    fn with_x_properties_merged_from_preserves_base_only_params() {
        // Example of local file written by old parser that stripped X-APPLE-STRUCTURED-LOCATION params.
        // Make sure we don't strip them from the remote
        let mut local = Event::new(
            "Flight",
            EventTime::Date(chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()),
        );
        local.x_properties = vec![XProperty::new(
            "X-APPLE-STRUCTURED-LOCATION",
            "geo:51.47,-0.45",
        )];

        let mut remote = local.clone();
        remote.x_properties = vec![XProperty {
            name: "X-APPLE-STRUCTURED-LOCATION".to_string(),
            value: "geo:51.47,-0.45".to_string(),
            params: vec![
                ("VALUE".to_string(), "URI".to_string()),
                ("X-TITLE".to_string(), "London Heathrow".to_string()),
            ],
        }];

        let merged = local.with_x_properties_merged_from(&remote);

        assert_eq!(merged.x_properties.len(), 1);
        assert_eq!(
            merged.x_properties[0].params,
            vec![
                ("VALUE".to_string(), "URI".to_string()),
                ("X-TITLE".to_string(), "London Heathrow".to_string()),
            ]
        );
    }

    #[test]
    fn with_x_properties_merged_from_local_params_win_shared_keys() {
        let mut local = Event::new(
            "Test",
            EventTime::Date(chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()),
        );
        local.x_properties = vec![XProperty {
            name: "X-APPLE-STRUCTURED-LOCATION".to_string(),
            value: "geo:0,0".to_string(),
            params: vec![("X-TITLE".to_string(), "New title".to_string())],
        }];

        let mut remote = local.clone();
        remote.x_properties = vec![XProperty {
            name: "X-APPLE-STRUCTURED-LOCATION".to_string(),
            value: "geo:0,0".to_string(),
            params: vec![("X-TITLE".to_string(), "Old title".to_string())],
        }];

        let merged = local.with_x_properties_merged_from(&remote);

        assert_eq!(
            merged.x_properties[0].params,
            vec![("X-TITLE".to_string(), "New title".to_string())]
        );
    }

    #[test]
    fn with_x_properties_merged_from_carries_through_base_only_properties() {
        // If local is missing the whole property, preserve it from base.
        let mut local = Event::new(
            "Test",
            EventTime::Date(chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()),
        );
        local.x_properties = vec![];

        let mut remote = local.clone();
        remote.x_properties = vec![XProperty::new("X-GOOGLE-EVENT-ID", "abc123")];

        let merged = local.with_x_properties_merged_from(&remote);

        assert_eq!(merged.x_properties.len(), 1);
        assert_eq!(merged.x_properties[0].name, "X-GOOGLE-EVENT-ID");
        assert_eq!(merged.x_properties[0].value, "abc123");
    }

    #[test]
    fn from_ics_str_returns_empty_for_calendar_without_events() {
        let ics = r"BEGIN:VCALENDAR
VERSION:2.0
END:VCALENDAR
"
        .replace('\n', "\r\n");
        let events = CalendarItem::from_ics_str(&ics).unwrap();
        assert!(events.is_empty());
    }

    #[test]
    fn from_ics_str_parses_single_event() {
        let ics = r"BEGIN:VCALENDAR
VERSION:2.0
BEGIN:VEVENT
UID:only@caldir
DTSTART:20260301T100000Z
SUMMARY:Only event
END:VEVENT
END:VCALENDAR
"
        .replace('\n', "\r\n");

        let events = CalendarItem::from_ics_str(&ics).unwrap();

        assert_eq!(events.len(), 1);
        let event = events.into_iter().next().unwrap().unwrap();
        assert_eq!(event.uid().as_str(), "only@caldir");
        assert_eq!(event.summary(), Some("Only event"));
    }

    #[test]
    fn from_ics_str_returns_events_in_document_order() {
        let ics = r"BEGIN:VCALENDAR
VERSION:2.0
BEGIN:VEVENT
UID:first@caldir
DTSTART:20260301T100000Z
SUMMARY:First
END:VEVENT
BEGIN:VEVENT
UID:second@caldir
DTSTART:20260302T100000Z
SUMMARY:Second
END:VEVENT
BEGIN:VEVENT
UID:third@caldir
DTSTART:20260303T100000Z
SUMMARY:Third
END:VEVENT
END:VCALENDAR
"
        .replace('\n', "\r\n");

        let events: Vec<CalendarItem> = CalendarItem::from_ics_str(&ics)
            .unwrap()
            .into_iter()
            .map(Result::unwrap)
            .collect();

        assert_eq!(events.len(), 3);
        assert_eq!(events[0].uid().as_str(), "first@caldir");
        assert_eq!(events[1].uid().as_str(), "second@caldir");
        assert_eq!(events[2].uid().as_str(), "third@caldir");
    }

    #[test]
    fn from_ics_str_handles_vtimezone_and_tzid_events() {
        let ics = r"BEGIN:VCALENDAR
VERSION:2.0
BEGIN:VTIMEZONE
TZID:Europe/Stockholm
BEGIN:STANDARD
DTSTART:19701025T030000
TZOFFSETFROM:+0200
TZOFFSETTO:+0100
RRULE:FREQ=YEARLY;BYDAY=-1SU;BYMONTH=10
TZNAME:CET
END:STANDARD
BEGIN:DAYLIGHT
DTSTART:19700329T020000
TZOFFSETFROM:+0100
TZOFFSETTO:+0200
RRULE:FREQ=YEARLY;BYDAY=-1SU;BYMONTH=3
TZNAME:CEST
END:DAYLIGHT
END:VTIMEZONE
BEGIN:VEVENT
UID:zoned@caldir
DTSTART;TZID=Europe/Stockholm:20260615T100000
SUMMARY:Zoned event
END:VEVENT
END:VCALENDAR
"
        .replace('\n', "\r\n");

        let event = Event::parse_single_ics(&ics);

        assert!(matches!(
            event.start,
            EventTime::DateTimeZoned { ref tzid, .. } if tzid == "Europe/Stockholm"
        ));
    }

    #[test]
    fn from_ics_str_rejects_malformed_top_level() {
        // Missing END:VCALENDAR
        let ics = r"BEGIN:VCALENDAR
VERSION:2.0
BEGIN:VEVENT
UID:x@caldir
DTSTART:20260301T100000Z
END:VEVENT
"
        .replace('\n', "\r\n");

        let result = CalendarItem::from_ics_str(&ics);

        assert!(matches!(result, Err(EventError::InvalidIcs(_, _))));
    }

    #[test]
    fn from_ics_str_surfaces_per_event_parse_errors() {
        // Second VEVENT is missing UID — surfaces as an inner Err, while the
        // first event still parses. Callers (e.g. webcal) decide whether to
        // skip or fail.
        let ics = r"BEGIN:VCALENDAR
VERSION:2.0
BEGIN:VEVENT
UID:good@caldir
DTSTART:20260301T100000Z
SUMMARY:Good
END:VEVENT
BEGIN:VEVENT
DTSTART:20260302T100000Z
SUMMARY:Missing UID
END:VEVENT
END:VCALENDAR
"
        .replace('\n', "\r\n");

        let events = CalendarItem::from_ics_str(&ics).unwrap();

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].as_ref().unwrap().uid().as_str(), "good@caldir");
        assert!(matches!(events[1], Err(EventError::MissingUid)));
    }

    fn dtstamp_line(ics: &str) -> &str {
        ics.lines()
            .find(|line| line.starts_with("DTSTAMP:"))
            .expect("DTSTAMP line should be present")
    }

    // DTSTAMP updates every time ICS is written
    // so we need to ignore it when comparing the original and serialized ICS.
    fn strip_dtstamp(ics: &str) -> String {
        ics.lines()
            .filter(|line| !line.starts_with("DTSTAMP:"))
            .collect::<Vec<_>>()
            .join("\r\n")
            + "\r\n"
    }

    // --- VTODO support (RED regression tests) ---

    #[test]
    fn from_ics_str_parses_a_vtodo_document() {
        use crate::test_utils::VTODO_ICS;

        let parsed = CalendarItem::from_ics_str(VTODO_ICS)
            .unwrap_or_else(|err| panic!("VTODO document failed to parse: {err:?}"));

        assert_eq!(
            parsed.len(),
            1,
            "VTODO document yielded {} entries; icalendar::Calendar::events() \
             (caldir-core/src/event.rs:101) filters to VEVENT only",
            parsed.len()
        );

        let event = parsed
            .into_iter()
            .next()
            .unwrap()
            .unwrap_or_else(|err| panic!("VTODO could not be converted to an Event: {err:?}"));

        assert_eq!(event.uid().as_str(), "assignment-123@u-crawler");
        assert_eq!(
            event.summary(),
            Some("Assignment 3 - Recursive descent parser")
        );
    }

    #[test]
    fn from_single_ics_str_parses_a_vtodo_document() {
        use crate::test_utils::VTODO_ICS;

        match CalendarItem::from_single_ics_str(VTODO_ICS) {
            Ok(event) => assert_eq!(event.uid().as_str(), "assignment-123@u-crawler"),
            Err(EventError::UnexpectedComponentCount { expected, found }) => panic!(
                "VTODO rejected by arity check at caldir-core/src/event.rs:116 \
                 (expected {expected}, found {found})"
            ),
            Err(err) => panic!("VTODO rejected for an unexpected reason: {err:?}"),
        }
    }

    /// Control: the VEVENT half of the same u_crawler output must keep working.
    #[test]
    fn from_single_ics_str_parses_the_vevent_control_document() {
        use crate::test_utils::VEVENT_CONTROL_ICS;

        let event = CalendarItem::from_single_ics_str(VEVENT_CONTROL_ICS).unwrap();

        assert_eq!(event.uid().as_str(), "window-123@u-crawler");
        assert_eq!(event.summary(), Some("Assignment 3 - submission window"));
    }

    /// Observed, and correct, behaviour for a component-less document: nothing
    /// to parse. Documented so a VTODO fix does not accidentally change it.
    #[test]
    fn from_ics_str_yields_nothing_for_a_component_less_document() {
        use crate::test_utils::EMPTY_VCALENDAR_ICS;

        let parsed = CalendarItem::from_ics_str(EMPTY_VCALENDAR_ICS).unwrap();

        assert!(parsed.is_empty());
        assert!(matches!(
            CalendarItem::from_single_ics_str(EMPTY_VCALENDAR_ICS),
            Err(EventError::UnexpectedComponentCount {
                expected: 1,
                found: 0
            })
        ));
    }

    // --- VEVENT no-regression control lock (T-5, R2/R45/R104, C2) ---

    /// One corpus entry: the bytes on disk, the bytes caldir must write back,
    /// and the answers the two range predicates must keep giving.
    struct VeventControlCase {
        name: &'static str,
        /// The document as it exists on disk.
        input: String,
        /// The exact bytes `to_ics_string` must emit for it (DTSTAMP aside).
        expected_ics: String,
        /// `(from, to, expected)` for `occurs_in_range`.
        occurs_probes: Vec<(DateTime<Utc>, DateTime<Utc>, bool)>,
        /// `(from, to, expected)` for `has_occurrence_in_range`.
        occurrence_probes: Vec<(DateTime<Utc>, DateTime<Utc>, bool)>,
    }

    fn crlf(ics: &str) -> String {
        ics.replace('\n', "\r\n")
    }

    fn utc(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, h, min, 0).unwrap()
    }

    /// Local midnight, as an instant. All-day events anchor to the system zone
    /// (`EventTime::to_utc` goes through `chrono::Local`), so the edges of an
    /// all-day event's range are only sharp when expressed this way — which is
    /// what makes this case meaningful in all three CI zones (R106).
    fn local_midnight(y: i32, m: u32, d: u32) -> DateTime<Utc> {
        chrono::Local
            .with_ymd_and_hms(y, m, d, 0, 0, 0)
            .single()
            .expect("local midnight should be unambiguous on these dates")
            .with_timezone(&Utc)
    }

    /// The canonical form of `VEVENT_CONTROL_ICS`: caldir rewrites the
    /// VCALENDAR headers (its own PRODID, no CALSCALE) but must not touch
    /// anything inside the VEVENT.
    const VEVENT_CONTROL_CANONICAL_ICS: &str = "BEGIN:VCALENDAR
VERSION:2.0
PRODID:CALDIR
BEGIN:VEVENT
DTSTAMP:20260801T090000Z
DESCRIPTION:Submission window for assignment 3.
DTEND:20260815T120000Z
DTSTART:20260815T100000Z
SUMMARY:Assignment 3 - submission window
UID:window-123@u-crawler
URL:https://example.edu/courses/cs101/assignments/123
END:VEVENT
END:VCALENDAR
";

    const ALL_DAY_ICS: &str = "BEGIN:VCALENDAR
VERSION:2.0
PRODID:CALDIR
BEGIN:VEVENT
DTSTAMP:20260801T090000Z
DTEND;VALUE=DATE:20260816
DTSTART;VALUE=DATE:20260815
SUMMARY:Study day
UID:all-day-1@caldir
END:VEVENT
END:VCALENDAR
";

    const RECURRING_MASTER_ICS: &str = "BEGIN:VCALENDAR
VERSION:2.0
PRODID:CALDIR
BEGIN:VEVENT
DTSTAMP:20260801T090000Z
DTEND;TZID=Europe/Oslo:20260515T164500
DTSTART;TZID=Europe/Oslo:20260515T160000
RRULE:FREQ=WEEKLY;BYDAY=FR;UNTIL=20260605T140000Z
SUMMARY:Friday retro
UID:recurring-1@caldir
EXDATE;TZID=Europe/Oslo:20260522T160000
END:VEVENT
END:VCALENDAR
";

    /// The fields `L1-02` moved out of `event.rs`: `x_properties` (compared
    /// order-independently, parameters ignored) and `attachments` (compared by
    /// URI alone). A corpus without them cannot see that regression.
    const X_PROPS_AND_ATTACHMENT_ICS: &str = "BEGIN:VCALENDAR
VERSION:2.0
PRODID:CALDIR
BEGIN:VEVENT
DTSTAMP:20260801T090000Z
DTEND:20260515T144500Z
DTSTART:20260515T140000Z
SUMMARY:Design review
UID:x-props-1@caldir
X-ALT-DESC;FMTTYPE=text/html:<html>agenda</html>
X-HOOLI-EVENT-ID:hooli-abc123
ATTACH;FILENAME=agenda.pdf;FMTTYPE=application/pdf:https://drive.example.co
 m/f/abc
END:VEVENT
END:VCALENDAR
";

    fn vevent_control_corpus() -> Vec<VeventControlCase> {
        vec![
            VeventControlCase {
                name: "VEVENT_CONTROL_ICS",
                input: crate::test_utils::VEVENT_CONTROL_ICS.to_string(),
                expected_ics: crlf(VEVENT_CONTROL_CANONICAL_ICS),
                occurs_probes: vec![
                    // Range ending exactly at DTSTART does not overlap.
                    (utc(2026, 8, 15, 8, 0), utc(2026, 8, 15, 10, 0), false),
                    (utc(2026, 8, 15, 8, 0), utc(2026, 8, 15, 10, 1), true),
                    (utc(2026, 8, 15, 10, 30), utc(2026, 8, 15, 11, 0), true),
                    (utc(2026, 8, 15, 11, 59), utc(2026, 8, 15, 14, 0), true),
                    // Range starting exactly at DTEND does not overlap.
                    (utc(2026, 8, 15, 12, 0), utc(2026, 8, 15, 14, 0), false),
                ],
                occurrence_probes: vec![
                    (utc(2026, 8, 15, 8, 0), utc(2026, 8, 15, 10, 0), false),
                    (utc(2026, 8, 15, 8, 0), utc(2026, 8, 15, 10, 1), true),
                    (utc(2026, 8, 15, 12, 0), utc(2026, 8, 15, 14, 0), false),
                ],
            },
            VeventControlCase {
                name: "all-day event",
                input: crlf(ALL_DAY_ICS),
                expected_ics: crlf(ALL_DAY_ICS),
                occurs_probes: vec![
                    (
                        local_midnight(2026, 8, 14),
                        local_midnight(2026, 8, 15),
                        false,
                    ),
                    (
                        local_midnight(2026, 8, 15),
                        local_midnight(2026, 8, 16),
                        true,
                    ),
                    (
                        local_midnight(2026, 8, 16),
                        local_midnight(2026, 8, 17),
                        false,
                    ),
                    (local_midnight(2026, 8, 1), local_midnight(2026, 9, 1), true),
                ],
                occurrence_probes: vec![
                    (
                        local_midnight(2026, 8, 14),
                        local_midnight(2026, 8, 15),
                        false,
                    ),
                    (
                        local_midnight(2026, 8, 15),
                        local_midnight(2026, 8, 16),
                        true,
                    ),
                    (
                        local_midnight(2026, 8, 16),
                        local_midnight(2026, 8, 17),
                        false,
                    ),
                ],
            },
            VeventControlCase {
                name: "recurring master",
                input: crlf(RECURRING_MASTER_ICS),
                expected_ics: crlf(RECURRING_MASTER_ICS),
                // occurs_in_range looks at the master's own DTSTART/DTEND only:
                // it deliberately does NOT expand the RRULE.
                occurs_probes: vec![
                    (utc(2026, 5, 15, 14, 0), utc(2026, 5, 15, 14, 45), true),
                    (utc(2026, 5, 15, 14, 45), utc(2026, 5, 16, 0, 0), false),
                    (utc(2026, 5, 29, 0, 0), utc(2026, 5, 30, 0, 0), false),
                ],
                // has_occurrence_in_range does expand it, and honours EXDATE
                // and UNTIL.
                occurrence_probes: vec![
                    (utc(2026, 5, 15, 0, 0), utc(2026, 5, 16, 0, 0), true),
                    // Excluded by EXDATE.
                    (utc(2026, 5, 22, 0, 0), utc(2026, 5, 23, 0, 0), false),
                    (utc(2026, 5, 29, 0, 0), utc(2026, 5, 30, 0, 0), true),
                    // Past UNTIL.
                    (utc(2026, 6, 19, 0, 0), utc(2026, 6, 26, 0, 0), false),
                ],
            },
            VeventControlCase {
                name: "x-properties and attachment",
                input: crlf(X_PROPS_AND_ATTACHMENT_ICS),
                expected_ics: crlf(X_PROPS_AND_ATTACHMENT_ICS),
                occurs_probes: vec![
                    (utc(2026, 5, 15, 13, 0), utc(2026, 5, 15, 14, 0), false),
                    (utc(2026, 5, 15, 14, 0), utc(2026, 5, 15, 14, 45), true),
                    (utc(2026, 5, 15, 14, 45), utc(2026, 5, 15, 15, 0), false),
                ],
                occurrence_probes: vec![
                    (utc(2026, 5, 15, 13, 0), utc(2026, 5, 15, 14, 0), false),
                    (utc(2026, 5, 15, 14, 0), utc(2026, 5, 15, 14, 45), true),
                    (utc(2026, 5, 15, 14, 45), utc(2026, 5, 15, 15, 0), false),
                ],
            },
        ]
    }

    /// T-5 (R2, R45, R104, C2). VTODO support must leave VEVENT behaviour
    /// observably untouched. Each of the three properties below fails silently
    /// and separately: equality drift changes sync *direction*, serialization
    /// drift rewrites every user's files on the next write, and predicate drift
    /// changes what gets deleted. So each is asserted on its own.
    #[test]
    fn vevent_control_round_trips_identically_after_vtodo_support() {
        for case in vevent_control_corpus() {
            let name = case.name;
            let event = Event::parse_single_ics(&case.input);
            let serialized = event.to_ics_string();
            let reparsed = Event::parse_single_ics(&serialized);

            // 1. Structural equality survives a full disk round trip (C2).
            assert_eq!(event, reparsed, "{name}: structural equality drifted");

            // 2. The bytes caldir writes back are the bytes it wrote before
            //    (DTSTAMP is re-stamped on every serialize, R25).
            assert_eq!(
                strip_dtstamp(&case.expected_ics),
                strip_dtstamp(&serialized),
                "{name}: serialized bytes drifted"
            );

            // 3. Both range predicates still answer the same at the edges (R45).
            for (from, to, expected) in case.occurs_probes {
                assert_eq!(
                    event.occurs_in_range(from, to),
                    expected,
                    "{name}: occurs_in_range({from}, {to})"
                );
            }

            for (from, to, expected) in case.occurrence_probes {
                assert_eq!(
                    event.has_occurrence_in_range(from, to),
                    expected,
                    "{name}: has_occurrence_in_range({from}, {to})"
                );
            }
        }
    }

    /// C2 companion lock: `Event`'s `==` answers "has the content changed?",
    /// and that answer decides sync direction. `L1-02` moved the two custom
    /// comparators out of this file, so pin what they mean here.
    #[test]
    fn event_equality_semantics_are_unchanged_after_vtodo_support() {
        let base = Event::parse_single_ics(&crlf(X_PROPS_AND_ATTACHMENT_ICS));
        assert_eq!(
            base.x_properties.len(),
            2,
            "fixture must carry X- properties"
        );
        assert_eq!(
            base.attachments.len(),
            1,
            "fixture must carry an attachment"
        );
        assert!(
            base.x_properties.iter().any(|x| !x.params.is_empty()),
            "fixture must carry an X- property with parameters"
        );
        assert!(
            !base.attachments[0].params.is_empty(),
            "fixture must carry an attachment with parameters"
        );

        // Sync bookkeeping, not content: ignored by ==.
        let mut volatile = base.clone();
        volatile.last_modified = Some(utc(2099, 1, 1, 0, 0));
        volatile.sequence = base.sequence + 41;
        assert_eq!(base, volatile, "last_modified/sequence must not affect ==");

        // X- properties: order-independent, parameters ignored.
        let mut reshuffled = base.clone();
        reshuffled.x_properties.reverse();
        for x in &mut reshuffled.x_properties {
            x.params.clear();
        }
        assert_eq!(
            base, reshuffled,
            "x_properties order/params must not affect =="
        );

        // ...but a changed or dropped X- property IS a content change.
        let mut changed_x = base.clone();
        changed_x.x_properties[0].value = "drifted".to_string();
        assert_ne!(
            base, changed_x,
            "a changed X- property value is a content change"
        );

        let mut dropped_x = base.clone();
        dropped_x.x_properties.pop();
        assert_ne!(base, dropped_x, "a dropped X- property is a content change");

        // The other direction: a superset must not compare equal either.
        let mut added_x = base.clone();
        added_x
            .x_properties
            .push(XProperty::new("X-CALDIR-EXTRA", "1"));
        assert_ne!(base, added_x, "an added X- property is a content change");

        // Attachments: compared by URI alone, so volatile params don't churn.
        let mut attachment_params = base.clone();
        attachment_params.attachments[0].params.clear();
        assert_eq!(
            base, attachment_params,
            "attachment params must not affect =="
        );

        // ...but a changed or dropped attachment IS a content change.
        let mut changed_attachment = base.clone();
        changed_attachment.attachments[0].uri = "https://drive.example.com/f/other".to_string();
        assert_ne!(
            base, changed_attachment,
            "a changed attachment URI is a content change"
        );

        let mut dropped_attachment = base.clone();
        dropped_attachment.attachments.pop();
        assert_ne!(
            base, dropped_attachment,
            "a dropped attachment is a content change"
        );

        // The other direction: a superset must not compare equal either.
        let mut added_attachment = base.clone();
        added_attachment
            .attachments
            .push(Attachment::new("https://drive.example.com/f/extra"));
        assert_ne!(
            base, added_attachment,
            "an added attachment is a content change"
        );

        // Guard against == degenerating into "always equal".
        let mut retitled = base.clone();
        retitled.summary = Some("Something else".to_string());
        assert_ne!(base, retitled, "a changed summary is a content change");
    }

    // --- R19: a document that yields nothing is a hard error that names
    // what it found; an unsupported component *beside* a supported one is
    // ignored, exactly as `main` ignored it ---

    #[test]
    fn an_unsupported_component_errors_and_names_it() {
        use crate::item::CalendarItem;

        let vjournal = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
BEGIN:VJOURNAL\r\n\
UID:journal@caldir\r\n\
SUMMARY:A journal entry\r\n\
END:VJOURNAL\r\n\
END:VCALENDAR\r\n";

        let err = CalendarItem::from_ics_str(vjournal)
            .expect_err("a document with nothing supported must not yield an empty vec");
        assert!(
            err.to_string().contains("VJOURNAL"),
            "error must name the component, got: {err}"
        );

        let x_foo = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
BEGIN:X-FOO\r\n\
UID:foo@caldir\r\n\
END:X-FOO\r\n\
END:VCALENDAR\r\n";

        let err = CalendarItem::from_ics_str(x_foo)
            .expect_err("a document with nothing supported must not yield an empty vec");
        assert!(
            err.to_string().contains("X-FOO"),
            "error must name the component, got: {err}"
        );
    }

    /// The other half of the same rule, and the regression it was written
    /// against: both documents above used to carry a supported component
    /// beside the unsupported one, and asserting they errored is what lost a
    /// `VEVENT` next to a `VJOURNAL`. The supported component must survive.
    #[test]
    fn an_unsupported_sibling_does_not_cost_the_supported_component() {
        use crate::item::CalendarItem;

        let ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
BEGIN:VEVENT\r\n\
UID:event@caldir\r\n\
DTSTART:20260301T100000Z\r\n\
END:VEVENT\r\n\
BEGIN:X-FOO\r\n\
UID:foo@caldir\r\n\
END:X-FOO\r\n\
END:VCALENDAR\r\n";

        let items = CalendarItem::from_ics_str(ics).expect("the VEVENT must still load");

        assert_eq!(items.len(), 1);
        assert!(matches!(items[0], Ok(CalendarItem::Event(_))));
    }
}
