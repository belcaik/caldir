//! The public `--json` schema for agenda output.

use caldir_core::{
    Attachment, Attendee, Availability, CalendarItem, Event, EventTime, Organizer,
    ParticipationStatus, Recurrence, Reminder, Status, Todo, TodoStatus, Visibility, XProperty,
};
use chrono::SecondsFormat;
use serde::{Serialize, Serializer};

use crate::output::agenda::{Agenda, AgendaEntry, AgendaView, anchor};

/// `--json` is the item stream and nothing else: the skipped-file notice is a
/// text-view affordance, so the schema is byte-identical to before (C13).
impl Serialize for Agenda {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.view.serialize(serializer)
    }
}

impl Serialize for AgendaView {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.entries.serialize(serializer)
    }
}

impl Serialize for AgendaEntry {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        AgendaJson::new(self).serialize(serializer)
    }
}

#[derive(Serialize)]
struct AgendaJson<'a> {
    instance_id: String,
    calendar: &'a Option<String>,
    title: &'a Option<String>,
    all_day: bool,
    tzid: Option<&'a str>,
    rsvp: Option<ParticipationStatus>,
    recurring: bool,
    #[serde(flatten)]
    item: ItemJson<'a>,
}

impl<'a> AgendaJson<'a> {
    fn new(entry: &'a AgendaEntry) -> Self {
        let anchor = anchor(&entry.item);

        Self {
            instance_id: entry.item.instance_id().to_string(),
            calendar: &entry.calendar,
            title: title(&entry.item),
            // An undated task belongs to no day, so it is not all-day either.
            all_day: anchor.is_some_and(EventTime::is_date),
            tzid: anchor.and_then(event_tzid),
            rsvp: entry.rsvp,
            recurring: recurring(&entry.item),
            item: ItemJson::from(&entry.item),
        }
    }
}

/// Whether this item is one instance of a recurrence, from whichever kind
/// backs it.
fn recurring(item: &CalendarItem) -> bool {
    match item {
        CalendarItem::Event(event) => event.recurrence_id.is_some(),
        CalendarItem::Todo(todo) => todo.recurrence_id.is_some(),
        // No recurrence id the CLI can read; `false` over a guess.
        _ => false,
    }
}

/// The item's `SUMMARY`, borrowed. `CalendarItem::summary` hands back an
/// `Option<&str>`, and the `title` key has always serialized the field itself.
fn title(item: &CalendarItem) -> &Option<String> {
    match item {
        CalendarItem::Event(event) => &event.summary,
        CalendarItem::Todo(todo) => &todo.summary,
        // A kind this crate cannot read has no summary to borrow; `null` is
        // the same value an untitled item already serializes.
        _ => &UNTITLED,
    }
}

/// The `None` a kind this crate cannot read borrows its title from.
static UNTITLED: Option<String> = None;

/// The `kind` discriminator. Additive: it is the one key a
/// consumer needs to migrate, via `select(.kind == "event")`.
#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
enum ItemKind {
    Event,
    Todo,
}

/// The flattened half of the agenda object — every key that is not the
/// entry-level wrapper.
///
/// One struct for both kinds, so the object shape stays **uniform** (C12): every
/// key is present on every entry, `null` where the kind has nothing to put
/// there. No `skip_serializing_if` here, ever.
///
/// Field order is the wire order under `#[serde(flatten)]`, so the five
/// additive keys sit at the end and an event entry's pre-existing keys keep
/// their bytes and their positions.
#[derive(Serialize)]
struct ItemJson<'a> {
    uid: &'a str,
    summary: &'a Option<String>,
    description: &'a Option<String>,
    location: &'a Option<String>,
    /// `DTSTART`. Widened to `T | null` for a task, which may legitimately have
    /// none — synthesizing one from `DUE` is the forbidden fake event. `null`
    /// can only ever appear here under `kind: "todo"`.
    start: Option<String>,
    /// Always `null` on a task: RFC 5545 forbids `DTEND` in a VTODO, and `Todo`
    /// has no field to carry one.
    end: Option<String>,
    /// **Not retyped.** The non-optional event `Status`, meaning "is this item
    /// live" — the sense `is_visible` uses. `TodoStatus` maps on rather than
    /// widening the value set, which would break every consumer matching on it
    /// exhaustively; the task's real state lives in `task_status`.
    status: Status,
    /// Widened to `T | null`: `TRANSP` is an event property, so a task has none.
    availability: Option<Availability>,
    visibility: Option<Visibility>,
    recurrence: Option<RecurrenceJson<'a>>,
    recurrence_id: Option<String>,
    organizer: Option<OrganizerJson<'a>>,
    attendees: Vec<AttendeeJson<'a>>,
    reminders: Vec<ReminderJson>,
    url: &'a Option<String>,
    attachments: Vec<AttachmentJson<'a>>,
    x_properties: Vec<XPropertyJson<'a>>,
    last_modified: Option<String>,
    sequence: i32,
    kind: ItemKind,
    /// `DUE`, same encoding as `start`. `null` on an event.
    due: Option<String>,
    /// `COMPLETED`. `null` on an event.
    completed: Option<String>,
    /// `PERCENT-COMPLETE`. Normally 0–100, but the raw RFC 5545 `INTEGER` is
    /// what the task carries: an out-of-range value round-trips verbatim
    /// rather than being clamped or dropped (F-4). `null` on an event.
    percent_complete: Option<i32>,
    /// The task's own `STATUS`, which is where the truth lives once `status`
    /// has been mapped onto the event value set. `null` on an event.
    task_status: Option<TodoStatus>,
}

impl<'a> From<&'a CalendarItem> for ItemJson<'a> {
    fn from(item: &'a CalendarItem) -> Self {
        match item {
            CalendarItem::Event(event) => Self::from_event(event),
            CalendarItem::Todo(todo) => Self::from_todo(todo),
            // There is no honest JSON for a kind whose fields this crate has
            // never seen, and inventing one would corrupt the stream C12's
            // DEL predicate is pinned against. caldir-cli depends on
            // caldir-core by path, so this cannot occur without a new variant
            // being added in this same tree.
            _ => unreachable!(
                "caldir_core::CalendarItem grew a variant; give it a `kind` \
                 discriminator and a field set here"
            ),
        }
    }
}

impl<'a> ItemJson<'a> {
    fn from_event(event: &'a Event) -> Self {
        let Event {
            uid,
            summary,
            description,
            location,
            start,
            end,
            status,
            availability,
            visibility,
            recurrence,
            recurrence_id,
            organizer,
            attendees,
            reminders,
            url,
            attachments,
            x_properties,
            last_modified,
            sequence,
        } = event;

        Self {
            uid: uid.as_str(),
            summary,
            description,
            location,
            start: Some(serialize_time(start)),
            end: end.as_ref().map(serialize_time),
            status: *status,
            availability: Some(*availability),
            visibility: *visibility,
            recurrence: recurrence.as_ref().map(RecurrenceJson::from),
            recurrence_id: recurrence_id
                .as_ref()
                .map(|id| serialize_time(id.as_event_time())),
            organizer: organizer.as_ref().map(OrganizerJson::from),
            attendees: attendees.iter().map(AttendeeJson::from).collect(),
            reminders: reminders.iter().map(ReminderJson::from).collect(),
            url,
            attachments: attachments.iter().map(AttachmentJson::from).collect(),
            x_properties: x_properties.iter().map(XPropertyJson::from).collect(),
            last_modified: last_modified
                .map(|timestamp| timestamp.to_rfc3339_opts(SecondsFormat::AutoSi, true)),
            sequence: *sequence,
            kind: ItemKind::Event,
            due: None,
            completed: None,
            percent_complete: None,
            task_status: None,
        }
    }

    /// The task arm, destructured exhaustively like the event arm above, so a
    /// field added to `Todo` is a compile error here rather than a silent
    /// omission from the public schema.
    fn from_todo(todo: &'a Todo) -> Self {
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
            // No key of its own yet: PRIORITY is not part of the documented
            // JSON schema. Adding one obeys the same additive rule.
            priority: _,
            url,
            recurrence,
            recurrence_id,
            organizer,
            attendees,
            reminders,
            attachments,
            x_properties,
            // `CREATED` has no key on either kind; `Event` does not model it.
            created: _,
            last_modified,
            sequence,
        } = todo;

        Self {
            uid: uid.as_str(),
            summary,
            description,
            location,
            start: start.as_ref().map(serialize_time),
            end: None,
            status: event_status_of(*status),
            availability: None,
            visibility: None,
            recurrence: recurrence.as_ref().map(RecurrenceJson::from),
            recurrence_id: recurrence_id
                .as_ref()
                .map(|id| serialize_time(id.as_event_time())),
            organizer: organizer.as_ref().map(OrganizerJson::from),
            attendees: attendees.iter().map(AttendeeJson::from).collect(),
            reminders: reminders.iter().map(ReminderJson::from).collect(),
            url,
            attachments: attachments.iter().map(AttachmentJson::from).collect(),
            x_properties: x_properties.iter().map(XPropertyJson::from).collect(),
            last_modified: last_modified
                .map(|timestamp| timestamp.to_rfc3339_opts(SecondsFormat::AutoSi, true)),
            sequence: *sequence,
            kind: ItemKind::Todo,
            due: due.as_ref().map(serialize_time),
            completed: completed.as_ref().map(serialize_time),
            percent_complete: *percent_complete,
            task_status: Some(*status),
        }
    }
}

/// `TodoStatus` projected onto the `status` key's existing value set. Only
/// `CANCELLED` is not live; the other three are ordinary items that a consumer
/// filtering on `status` must keep seeing.
fn event_status_of(status: TodoStatus) -> Status {
    match status {
        TodoStatus::NeedsAction | TodoStatus::InProcess | TodoStatus::Completed => {
            Status::Confirmed
        }
        TodoStatus::Cancelled => Status::Cancelled,
    }
}

#[derive(Serialize)]
struct RecurrenceJson<'a> {
    rrule: &'a str,
    exdates: Vec<String>,
    rdates: Vec<String>,
}

impl<'a> From<&'a Recurrence> for RecurrenceJson<'a> {
    fn from(recurrence: &'a Recurrence) -> Self {
        Self {
            rrule: &recurrence.rrule,
            exdates: recurrence.exdates.iter().map(serialize_time).collect(),
            rdates: recurrence.rdates.iter().map(serialize_time).collect(),
        }
    }
}

#[derive(Serialize)]
struct OrganizerJson<'a> {
    email: &'a str,
    name: &'a Option<String>,
}

impl<'a> From<&'a Organizer> for OrganizerJson<'a> {
    fn from(organizer: &'a Organizer) -> Self {
        Self {
            email: &organizer.email,
            name: &organizer.name,
        }
    }
}

#[derive(Serialize)]
struct AttendeeJson<'a> {
    email: &'a str,
    name: &'a Option<String>,
    status: Option<ParticipationStatus>,
}

impl<'a> From<&'a Attendee> for AttendeeJson<'a> {
    fn from(attendee: &'a Attendee) -> Self {
        Self {
            email: &attendee.email,
            name: &attendee.name,
            status: attendee.status,
        }
    }
}

#[derive(Serialize)]
struct ReminderJson {
    minutes_before_start: i64,
}

impl From<&Reminder> for ReminderJson {
    fn from(reminder: &Reminder) -> Self {
        Self {
            minutes_before_start: reminder.minutes_before_start,
        }
    }
}

#[derive(Serialize)]
struct AttachmentJson<'a> {
    uri: &'a str,
    params: Vec<PropertyParameterJson<'a>>,
}

impl<'a> From<&'a Attachment> for AttachmentJson<'a> {
    fn from(attachment: &'a Attachment) -> Self {
        Self {
            uri: &attachment.uri,
            params: property_parameters(&attachment.params),
        }
    }
}

#[derive(Serialize)]
struct XPropertyJson<'a> {
    name: &'a str,
    value: &'a str,
    params: Vec<PropertyParameterJson<'a>>,
}

impl<'a> From<&'a XProperty> for XPropertyJson<'a> {
    fn from(property: &'a XProperty) -> Self {
        Self {
            name: &property.name,
            value: &property.value,
            params: property_parameters(&property.params),
        }
    }
}

#[derive(Serialize)]
struct PropertyParameterJson<'a> {
    name: &'a str,
    value: &'a str,
}

fn property_parameters(params: &[(String, String)]) -> Vec<PropertyParameterJson<'_>> {
    params
        .iter()
        .map(|(name, value)| PropertyParameterJson { name, value })
        .collect()
}

fn serialize_time(time: &EventTime) -> String {
    match time {
        EventTime::Date(date) => date.format("%Y-%m-%d").to_string(),
        EventTime::DateTimeUtc(datetime) => datetime.to_rfc3339_opts(SecondsFormat::AutoSi, true),
        EventTime::DateTimeFloating(_) => time
            .to_local_tz(&chrono::Local)
            .to_rfc3339_opts(SecondsFormat::AutoSi, false),
        EventTime::DateTimeZoned { tzid, .. } => tzid
            .parse::<chrono_tz::Tz>()
            .map(|tz| {
                time.to_local_tz(&tz)
                    .to_rfc3339_opts(SecondsFormat::AutoSi, false)
            })
            .unwrap_or_else(|_| {
                time.to_local_tz(&chrono::Local)
                    .to_rfc3339_opts(SecondsFormat::AutoSi, false)
            }),
    }
}

fn event_tzid(time: &EventTime) -> Option<&str> {
    match time {
        EventTime::DateTimeZoned { tzid, .. } => Some(tzid),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::{Output, TextRender};
    use caldir_core::{EventUid, RecurrenceId, TimeFormat, TodoStatus};
    use chrono::{NaiveDate, NaiveDateTime, TimeZone, Utc};
    use pretty_assertions::assert_eq;
    use serde_json::json;

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).unwrap()
    }

    fn datetime(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> NaiveDateTime {
        date(year, month, day).and_hms_opt(hour, minute, 0).unwrap()
    }

    fn entry(event: Event) -> AgendaEntry {
        AgendaEntry {
            calendar: Some("personal".to_string()),
            rsvp: None,
            item: CalendarItem::Event(event),
        }
    }

    fn todo_entry(todo: Todo) -> AgendaEntry {
        AgendaEntry {
            calendar: Some("personal".to_string()),
            rsvp: None,
            item: CalendarItem::Todo(todo),
        }
    }

    fn view(entries: Vec<AgendaEntry>) -> AgendaView {
        AgendaView {
            undated: false,
            time_format: TimeFormat::H24,
            entries,
            range_start: date(2026, 8, 14),
            range_end: date(2026, 8, 20),
        }
    }

    #[test]
    fn serializes_fully_populated_event() {
        let start = EventTime::DateTimeZoned {
            datetime: datetime(2026, 8, 14, 16, 0),
            tzid: "Europe/Stockholm".to_string(),
        };
        let mut event = Event::new("Friday retro", start.clone());
        event.uid = EventUid::new("retro@caldir");
        event.end = Some(EventTime::DateTimeZoned {
            datetime: datetime(2026, 8, 14, 16, 30),
            tzid: "Europe/Stockholm".to_string(),
        });
        event.location = Some("Conference room".to_string());
        event.description = Some("Weekly retrospective".to_string());
        event.status = Status::Tentative;
        event.availability = Availability::Free;
        event.visibility = Some(Visibility::Confidential);
        event.recurrence = Some(Recurrence {
            rrule: "FREQ=WEEKLY;BYDAY=FR".to_string(),
            exdates: vec![
                EventTime::Date(date(2026, 8, 21)),
                EventTime::DateTimeZoned {
                    datetime: datetime(2026, 8, 28, 16, 0),
                    tzid: "Europe/Stockholm".to_string(),
                },
            ],
            rdates: vec![EventTime::DateTimeUtc(
                Utc.with_ymd_and_hms(2026, 9, 4, 14, 0, 0).unwrap(),
            )],
        });
        event.recurrence_id = Some(RecurrenceId::from_event_time(start));
        event.organizer = Some(Organizer {
            email: "host@example.com".to_string(),
            name: Some("Host Person".to_string()),
        });
        event.attendees = vec![
            Attendee {
                email: "guest@example.com".to_string(),
                name: Some("Guest Person".to_string()),
                status: Some(ParticipationStatus::NeedsAction),
            },
            Attendee::new("observer@example.com"),
        ];
        event.reminders = vec![Reminder::from_minutes(30), Reminder::from_minutes(-5)];
        event.url = Some("https://meet.google.com/abc-defg-hij".to_string());
        event.attachments = vec![
            Attachment {
                uri: "https://example.com/agenda.html".to_string(),
                params: vec![
                    ("FMTTYPE".to_string(), "text/html".to_string()),
                    ("X-TITLE".to_string(), "Agenda".to_string()),
                ],
            },
            Attachment::new("https://example.com/notes.txt"),
        ];
        event.x_properties = vec![
            XProperty::new(
                "X-GOOGLE-CONFERENCE",
                "https://meet.google.com/abc-defg-hij",
            ),
            XProperty {
                name: "X-ALT-DESC".to_string(),
                value: "<p>Weekly retrospective</p>".to_string(),
                params: vec![
                    ("FMTTYPE".to_string(), "text/html".to_string()),
                    ("X-TITLE".to_string(), "Rich notes".to_string()),
                ],
            },
        ];
        event.last_modified = Some(Utc.with_ymd_and_hms(2026, 8, 13, 10, 11, 12).unwrap());
        event.sequence = 7;

        let mut entry = entry(event);
        entry.rsvp = Some(ParticipationStatus::Accepted);

        assert_eq!(
            serde_json::to_value(entry).unwrap(),
            json!({
                "instance_id": "retro@caldir__TZID=Europe/Stockholm:20260814T160000",
                "uid": "retro@caldir",
                "calendar": "personal",
                "title": "Friday retro",
                "summary": "Friday retro",
                "all_day": false,
                "start": "2026-08-14T16:00:00+02:00",
                "end": "2026-08-14T16:30:00+02:00",
                "tzid": "Europe/Stockholm",
                "location": "Conference room",
                "description": "Weekly retrospective",
                "status": "tentative",
                "availability": "free",
                "visibility": "confidential",
                "recurrence": {
                    "rrule": "FREQ=WEEKLY;BYDAY=FR",
                    "exdates": [
                        "2026-08-21",
                        "2026-08-28T16:00:00+02:00",
                    ],
                    "rdates": ["2026-09-04T14:00:00Z"],
                },
                "recurrence_id": "2026-08-14T16:00:00+02:00",
                "organizer": {
                    "email": "host@example.com",
                    "name": "Host Person",
                },
                "attendees": [
                    {
                        "email": "guest@example.com",
                        "name": "Guest Person",
                        "status": "needs_action",
                    },
                    {
                        "email": "observer@example.com",
                        "name": null,
                        "status": null,
                    },
                ],
                "reminders": [
                    { "minutes_before_start": 30 },
                    { "minutes_before_start": -5 },
                ],
                "url": "https://meet.google.com/abc-defg-hij",
                "attachments": [
                    {
                        "uri": "https://example.com/agenda.html",
                        "params": [
                            { "name": "FMTTYPE", "value": "text/html" },
                            { "name": "X-TITLE", "value": "Agenda" },
                        ],
                    },
                    {
                        "uri": "https://example.com/notes.txt",
                        "params": [],
                    },
                ],
                "x_properties": [
                    {
                        "name": "X-GOOGLE-CONFERENCE",
                        "value": "https://meet.google.com/abc-defg-hij",
                        "params": [],
                    },
                    {
                        "name": "X-ALT-DESC",
                        "value": "<p>Weekly retrospective</p>",
                        "params": [
                            { "name": "FMTTYPE", "value": "text/html" },
                            { "name": "X-TITLE", "value": "Rich notes" },
                        ],
                    },
                ],
                "last_modified": "2026-08-13T10:11:12Z",
                "sequence": 7,
                "rsvp": "accepted",
                "recurring": true,
                "kind": "event",
                "due": null,
                "completed": null,
                "percent_complete": null,
                "task_status": null,
            })
        );
    }

    #[test]
    fn serializes_url_and_google_conference_from_ics() {
        let ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
BEGIN:VEVENT\r\n\
UID:conference@caldir\r\n\
DTSTAMP:20260814T120000Z\r\n\
DTSTART:20260814T140000Z\r\n\
SUMMARY:Work meeting\r\n\
URL:https://meet.google.com/abc-defg-hij\r\n\
X-GOOGLE-CONFERENCE:https://meet.google.com/abc-defg-hij\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";
        let event = match caldir_core::CalendarItem::from_single_ics_str(ics).unwrap() {
            caldir_core::CalendarItem::Event(event) => event,
            item => panic!("VEVENT fixture parsed as {item:?}"),
        };

        let json = serde_json::to_value(entry(event)).unwrap();

        assert_eq!(json["url"], "https://meet.google.com/abc-defg-hij");
        assert_eq!(
            json["x_properties"],
            json!([{
                "name": "X-GOOGLE-CONFERENCE",
                "value": "https://meet.google.com/abc-defg-hij",
                "params": [],
            }])
        );
    }

    #[test]
    fn serializes_nulls_and_empty_collections() {
        let mut event = Event::new("unused", EventTime::Date(date(2026, 8, 14)));
        event.uid = EventUid::new("defaults@caldir");
        event.summary = None;

        assert_eq!(
            serde_json::to_value(entry(event)).unwrap(),
            json!({
                "instance_id": "defaults@caldir",
                "uid": "defaults@caldir",
                "calendar": "personal",
                "title": null,
                "summary": null,
                "all_day": true,
                "start": "2026-08-14",
                "end": null,
                "tzid": null,
                "location": null,
                "description": null,
                "status": "confirmed",
                "availability": "busy",
                "visibility": null,
                "recurrence": null,
                "recurrence_id": null,
                "organizer": null,
                "attendees": [],
                "reminders": [],
                "url": null,
                "attachments": [],
                "x_properties": [],
                "last_modified": null,
                "sequence": 0,
                "rsvp": null,
                "recurring": false,
                "kind": "event",
                "due": null,
                "completed": null,
                "percent_complete": null,
                "task_status": null,
            })
        );
    }

    #[test]
    fn serializes_date_only_recurrence_id() {
        let mut event = Event::new("Holiday", EventTime::Date(date(2026, 8, 14)));
        event.recurrence_id = Some(RecurrenceId::from_event_time(EventTime::Date(date(
            2026, 8, 14,
        ))));

        assert_eq!(
            serde_json::to_value(entry(event)).unwrap()["recurrence_id"],
            "2026-08-14"
        );
    }

    #[test]
    fn preserves_original_agenda_keys() {
        let mut event = Event::new("Trip", EventTime::Date(date(2026, 8, 14)));
        event.uid = EventUid::new("trip@caldir");
        event.end = Some(EventTime::Date(date(2026, 8, 17)));
        event.location = Some("Sweden".to_string());
        event.description = Some("Summer trip".to_string());
        let mut entry = entry(event);
        entry.rsvp = Some(ParticipationStatus::Tentative);

        let json = serde_json::to_value(entry).unwrap();
        let compatibility_fields = json!({
            "instance_id": "trip@caldir",
            "uid": "trip@caldir",
            "calendar": "personal",
            "title": "Trip",
            "all_day": true,
            "start": "2026-08-14",
            "end": "2026-08-17",
            "tzid": null,
            "location": "Sweden",
            "description": "Summer trip",
            "status": "confirmed",
            "rsvp": "tentative",
            "recurring": false,
        });

        for (key, value) in compatibility_fields.as_object().unwrap() {
            assert_eq!(&json[key], value, "compatibility field {key}");
        }
    }

    #[test]
    fn serializes_utc_event() {
        let start = Utc.with_ymd_and_hms(2026, 8, 14, 14, 0, 0).unwrap();
        let mut event = Event::new("UTC call", EventTime::DateTimeUtc(start));
        event.uid = EventUid::new("utc@caldir");
        event.end = Some(EventTime::DateTimeUtc(
            start + chrono::Duration::minutes(30),
        ));

        let json = serde_json::to_value(entry(event)).unwrap();

        assert_eq!(json["start"], "2026-08-14T14:00:00Z");
        assert_eq!(json["end"], "2026-08-14T14:30:00Z");
        assert_eq!(json["tzid"], serde_json::Value::Null);
        assert_eq!(json["status"], "confirmed");
        assert_eq!(json["rsvp"], serde_json::Value::Null);
        assert_eq!(json["recurring"], false);
    }

    #[test]
    fn serializes_floating_event_in_system_timezone() {
        let start = datetime(2026, 8, 14, 16, 0);
        let event = Event::new("Floating", EventTime::DateTimeFloating(start));

        let json = serde_json::to_value(entry(event)).unwrap();
        let expected = EventTime::DateTimeFloating(start)
            .to_local_tz(&chrono::Local)
            .to_rfc3339_opts(SecondsFormat::AutoSi, false);

        assert_eq!(json["start"], expected);
        assert_eq!(json["tzid"], serde_json::Value::Null);
    }

    #[test]
    fn serializes_all_day_event_with_exclusive_end() {
        let mut event = Event::new("Trip", EventTime::Date(date(2026, 8, 14)));
        event.end = Some(EventTime::Date(date(2026, 8, 17)));

        let json = serde_json::to_value(entry(event)).unwrap();

        assert_eq!(json["all_day"], true);
        assert_eq!(json["start"], "2026-08-14");
        assert_eq!(json["end"], "2026-08-17");
        assert_eq!(json["tzid"], serde_json::Value::Null);
    }

    /// C12 as amended by ADR-10 D6: the public `--json` object shape is
    /// **uniform across kinds**. Both fixtures live in one test because the
    /// load-bearing property is that the objects share a key set — asserting
    /// them apart lets a uniform-shape regression through.
    ///
    /// Replaces the interim event-only lock: the stream carries tasks now.
    /// `start` and `availability` are the only two keys that widened, `null`
    /// appears on them only under `kind: "todo"`, and `status` keeps its
    /// existing value set with the task's own state in `task_status`.
    #[test]
    fn schema_keys_are_stable_over_an_event_and_a_task_fixture() {
        let mut event = Event::new("Standup", EventTime::Date(date(2026, 8, 14)));
        event.uid = EventUid::new("standup@caldir");

        let mut due_only = Todo::new("Ship crawler patch");
        due_only.uid = EventUid::new("assignment-123@u-crawler");
        due_only.due = Some(EventTime::Date(date(2026, 8, 15)));
        due_only.status = TodoStatus::Completed;
        due_only.completed = Some(EventTime::DateTimeUtc(
            Utc.with_ymd_and_hms(2026, 8, 15, 9, 30, 0).unwrap(),
        ));
        due_only.percent_complete = Some(100);

        let mut undated = Todo::new("Someday");
        undated.uid = EventUid::new("someday@u-crawler");

        let json = view(vec![
            entry(event),
            todo_entry(due_only),
            todo_entry(undated),
        ])
        .to_json();

        assert_eq!(
            json[0],
            json!({
                "instance_id": "standup@caldir",
                "calendar": "personal",
                "title": "Standup",
                "all_day": true,
                "tzid": null,
                "rsvp": null,
                "recurring": false,
                "uid": "standup@caldir",
                "summary": "Standup",
                "description": null,
                "location": null,
                "start": "2026-08-14",
                "end": null,
                "status": "confirmed",
                "availability": "busy",
                "visibility": null,
                "recurrence": null,
                "recurrence_id": null,
                "organizer": null,
                "attendees": [],
                "reminders": [],
                "url": null,
                "attachments": [],
                "x_properties": [],
                "last_modified": null,
                "sequence": 0,
                "kind": "event",
                "due": null,
                "completed": null,
                "percent_complete": null,
                "task_status": null,
            }),
            "event entry"
        );

        assert_eq!(
            json[1],
            json!({
                "instance_id": "VTODO:assignment-123@u-crawler",
                "calendar": "personal",
                "title": "Ship crawler patch",
                "all_day": true,
                "tzid": null,
                "rsvp": null,
                "recurring": false,
                "uid": "assignment-123@u-crawler",
                "summary": "Ship crawler patch",
                "description": null,
                "location": null,
                "start": null,
                "end": null,
                "status": "confirmed",
                "availability": null,
                "visibility": null,
                "recurrence": null,
                "recurrence_id": null,
                "organizer": null,
                "attendees": [],
                "reminders": [],
                "url": null,
                "attachments": [],
                "x_properties": [],
                "last_modified": null,
                "sequence": 0,
                "kind": "todo",
                "due": "2026-08-15",
                "completed": "2026-08-15T09:30:00Z",
                "percent_complete": 100,
                "task_status": "completed",
            }),
            "DUE-only task entry"
        );

        assert_eq!(
            json[2],
            json!({
                "instance_id": "VTODO:someday@u-crawler",
                "calendar": "personal",
                "title": "Someday",
                "all_day": false,
                "tzid": null,
                "rsvp": null,
                "recurring": false,
                "uid": "someday@u-crawler",
                "summary": "Someday",
                "description": null,
                "location": null,
                "start": null,
                "end": null,
                "status": "confirmed",
                "availability": null,
                "visibility": null,
                "recurrence": null,
                "recurrence_id": null,
                "organizer": null,
                "attendees": [],
                "reminders": [],
                "url": null,
                "attachments": [],
                "x_properties": [],
                "last_modified": null,
                "sequence": 0,
                "kind": "todo",
                "due": null,
                "completed": null,
                "percent_complete": null,
                "task_status": "needs_action",
            }),
            "undated task entry"
        );

        let keys = |value: &serde_json::Value| -> Vec<String> {
            value.as_object().unwrap().keys().cloned().collect()
        };
        assert_eq!(keys(&json[0]), keys(&json[1]), "event vs DUE-only task");
        assert_eq!(keys(&json[0]), keys(&json[2]), "event vs undated task");
    }

    /// The task arm of the shared shape, populated everywhere a task can be
    /// populated — the mirror of `serializes_fully_populated_event`, so a field
    /// silently dropped from the task arm alone is caught.
    #[test]
    fn serializes_fully_populated_task() {
        let due = EventTime::DateTimeZoned {
            datetime: datetime(2026, 8, 15, 23, 59),
            tzid: "Europe/Stockholm".to_string(),
        };
        let mut todo = caldir_core::Todo::new("Ship crawler patch");
        todo.uid = EventUid::new("assignment-123@u-crawler");
        todo.description = Some("Land the VTODO branch".to_string());
        todo.location = Some("Desk".to_string());
        todo.start = Some(EventTime::DateTimeZoned {
            datetime: datetime(2026, 8, 15, 9, 0),
            tzid: "Europe/Stockholm".to_string(),
        });
        todo.due = Some(due.clone());
        todo.status = TodoStatus::InProcess;
        todo.completed = Some(EventTime::DateTimeUtc(
            Utc.with_ymd_and_hms(2026, 8, 15, 20, 0, 0).unwrap(),
        ));
        todo.percent_complete = Some(60);
        todo.priority = Some(1);
        todo.url = Some("https://example.com/task".to_string());
        todo.recurrence = Some(Recurrence {
            rrule: "FREQ=WEEKLY".to_string(),
            exdates: vec![EventTime::Date(date(2026, 8, 22))],
            rdates: vec![],
        });
        todo.recurrence_id = Some(RecurrenceId::from_event_time(due));
        todo.organizer = Some(Organizer {
            email: "lead@example.com".to_string(),
            name: None,
        });
        todo.attendees = vec![Attendee::new("helper@example.com")];
        todo.reminders = vec![Reminder::from_minutes(15)];
        todo.attachments = vec![Attachment::new("https://example.com/spec.pdf")];
        todo.x_properties = vec![XProperty::new("X-APPLE-SORT-ORDER", "12")];
        todo.created = Some(Utc.with_ymd_and_hms(2026, 8, 1, 8, 0, 0).unwrap());
        todo.last_modified = Some(Utc.with_ymd_and_hms(2026, 8, 14, 7, 8, 9).unwrap());
        todo.sequence = 3;

        assert_eq!(
            serde_json::to_value(todo_entry(todo)).unwrap(),
            json!({
                "instance_id": "VTODO:assignment-123@u-crawler__TZID=Europe/Stockholm:20260815T235900",
                "calendar": "personal",
                "title": "Ship crawler patch",
                "all_day": false,
                "tzid": "Europe/Stockholm",
                "rsvp": null,
                "recurring": true,
                "uid": "assignment-123@u-crawler",
                "summary": "Ship crawler patch",
                "description": "Land the VTODO branch",
                "location": "Desk",
                "start": "2026-08-15T09:00:00+02:00",
                "end": null,
                "status": "confirmed",
                "availability": null,
                "visibility": null,
                "recurrence": {
                    "rrule": "FREQ=WEEKLY",
                    "exdates": ["2026-08-22"],
                    "rdates": [],
                },
                "recurrence_id": "2026-08-15T23:59:00+02:00",
                "organizer": { "email": "lead@example.com", "name": null },
                "attendees": [
                    { "email": "helper@example.com", "name": null, "status": null },
                ],
                "reminders": [{ "minutes_before_start": 15 }],
                "url": "https://example.com/task",
                "attachments": [
                    { "uri": "https://example.com/spec.pdf", "params": [] },
                ],
                "x_properties": [
                    { "name": "X-APPLE-SORT-ORDER", "value": "12", "params": [] },
                ],
                "last_modified": "2026-08-14T07:08:09Z",
                "sequence": 3,
                "kind": "todo",
                "due": "2026-08-15T23:59:00+02:00",
                "completed": "2026-08-15T20:00:00Z",
                "percent_complete": 60,
                "task_status": "in_process",
            })
        );
    }

    /// `status` is NOT retyped (C12): every task lands on a value from the
    /// existing `Status` set, and only CANCELLED is not live. The truth stays
    /// in `task_status`.
    #[test]
    fn task_status_maps_onto_the_existing_status_value_set() {
        let mapped = |status: TodoStatus| {
            let mut todo = caldir_core::Todo::new("Task");
            todo.status = status;
            let json = serde_json::to_value(todo_entry(todo)).unwrap();
            (json["status"].clone(), json["task_status"].clone())
        };

        assert_eq!(
            mapped(TodoStatus::NeedsAction),
            (json!("confirmed"), json!("needs_action"))
        );
        assert_eq!(
            mapped(TodoStatus::InProcess),
            (json!("confirmed"), json!("in_process"))
        );
        assert_eq!(
            mapped(TodoStatus::Completed),
            (json!("confirmed"), json!("completed"))
        );
        assert_eq!(
            mapped(TodoStatus::Cancelled),
            (json!("cancelled"), json!("cancelled"))
        );
    }

    #[test]
    fn serializes_empty_view_as_bare_array() {
        assert_eq!(view(Vec::new()).to_json(), json!([]));
    }

    #[test]
    fn text_repeats_multi_day_event_but_json_does_not() {
        let mut event = Event::new("Trip", EventTime::Date(date(2026, 8, 14)));
        event.end = Some(EventTime::Date(date(2026, 8, 17)));
        let view = view(vec![entry(event)]);

        assert_eq!(view.entries.len(), 1);
        assert_eq!(view.to_json().as_array().unwrap().len(), 1);
        assert_eq!(view.to_text().matches("Trip").count(), 3);
    }
}
