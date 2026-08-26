use std::path::PathBuf;

use crate::caldir::TimeFormat;
use crate::diff::{CalendarDiff, EventChange};
use crate::item::CalendarItem;
use crate::provider::mock_provider::MockProvider;
use crate::{
    Caldir, CaldirConfig, Calendar, CalendarConfig, CalendarEvent, Event, EventTime, Provider,
    ProviderRegistry, ProviderSlug, Remote, RemoteConfig, RemoteConfigParams,
};
use chrono::NaiveDate;
use icalendar::{Component, EventLike};
use tempfile::TempDir;

pub fn test_caldir() -> (TempDir, Caldir) {
    let (tmp_data_dir, test_caldir_config) = test_caldir_config();

    let caldir = Caldir::new(test_caldir_config, ProviderRegistry::new());

    (tmp_data_dir, caldir)
}

pub fn test_caldir_config() -> (TempDir, CaldirConfig) {
    let tmp = TempDir::new().unwrap();
    let data_dir = tmp.path().join("test-caldir");

    let config = CaldirConfig::new(data_dir, TimeFormat::default(), None, None);

    (tmp, config)
}

pub fn test_calendar_path() -> (TempDir, PathBuf) {
    let tmp = TempDir::new().unwrap();
    let calendar_path = tmp.path().to_path_buf().join("test-calendar");
    (tmp, calendar_path)
}

pub fn test_calendar() -> (TempDir, Calendar) {
    let (tmp, caldir) = test_caldir();
    let calendar = caldir.create_calendar("test-calendar", None).unwrap();
    (tmp, calendar)
}

pub fn test_calendar_event() -> (TempDir, CalendarEvent) {
    let (tmp, calendar) = test_calendar();
    let event = test_event();

    let calendar_event = calendar.create_event(event.clone()).unwrap();
    (tmp, calendar_event)
}

pub fn test_event_time() -> EventTime {
    EventTime::DateTimeFloating(
        NaiveDate::from_ymd_opt(2026, 1, 1)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap(),
    )
}

pub fn test_event() -> Event {
    Event::new("Test Event", test_event_time())
}

pub fn test_icalendar_event() -> icalendar::Event {
    icalendar::Event::new()
        .uid("test-uid@caldir")
        .starts(icalendar::DatePerhapsTime::Date(
            chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
        ))
        .clone()
}

pub fn test_calendar_config() -> CalendarConfig {
    CalendarConfig::new(
        Some("Test Calendar".to_string()),
        Some("#ff0000".to_string()),
        Some(false),
        None,
    )
}

pub fn test_remote_config(provider_slug: &str) -> RemoteConfig {
    let params = RemoteConfigParams::new();
    RemoteConfig::new(ProviderSlug::from(provider_slug), params)
}

pub fn test_mock_provider() -> MockProvider {
    MockProvider::new("test-provider")
}

pub fn test_remote_params() -> RemoteConfigParams {
    let mut params = RemoteConfigParams::new();
    params.insert(
        "test_account".to_string(),
        toml::Value::String("user@example.com".to_string()),
    );
    params
}

pub fn test_remote() -> (MockProvider, Remote) {
    let mock = test_mock_provider();
    let remote = Remote::new(mock.provider(), test_remote_params());
    (mock, remote)
}

pub fn outgoing_create_diff(item: impl Into<CalendarItem>) -> CalendarDiff {
    CalendarDiff::from_changes(vec![EventChange::Create(item.into())], vec![])
}

pub fn outgoing_update_diff(
    from: impl Into<CalendarItem>,
    to: impl Into<CalendarItem>,
) -> CalendarDiff {
    CalendarDiff::from_changes(
        vec![EventChange::Update {
            from: from.into(),
            to: to.into(),
        }],
        vec![],
    )
}

pub fn outgoing_delete_diff(item: impl Into<CalendarItem>) -> CalendarDiff {
    CalendarDiff::from_changes(vec![EventChange::Delete(item.into())], vec![])
}

pub fn incoming_create_diff(item: impl Into<CalendarItem>) -> CalendarDiff {
    CalendarDiff::from_changes(vec![], vec![EventChange::Create(item.into())])
}

pub fn incoming_update_diff(
    from: impl Into<CalendarItem>,
    to: impl Into<CalendarItem>,
) -> CalendarDiff {
    CalendarDiff::from_changes(
        vec![],
        vec![EventChange::Update {
            from: from.into(),
            to: to.into(),
        }],
    )
}

pub fn incoming_delete_diff(item: impl Into<CalendarItem>) -> CalendarDiff {
    CalendarDiff::from_changes(vec![], vec![EventChange::Delete(item.into())])
}

pub fn test_provider(slug: &str) -> (TempDir, Provider) {
    let (tmp, bin_path) = test_binary(&format!("caldir-provider-{slug}"));
    let provider = Provider::from_binary_path(bin_path).unwrap();
    (tmp, provider)
}

pub fn test_binary(filename: &str) -> (TempDir, PathBuf) {
    let tmp = TempDir::new().unwrap();

    let path = tmp
        .path()
        .join(format!("{}{}", filename, std::env::consts::EXE_SUFFIX));

    std::fs::write(&path, b"").unwrap();

    // Set executable permissions to executable:
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).unwrap();
    }

    (tmp, path)
}

// --- VTODO reproduction fixtures (stage 2) ---
//
// Representative of `u_crawler` output: a course directory with
// `deadlines/<slug>.ics` (VTODO) and `windows/<slug>.ics` (VEVENT).

/// RFC 5545 valid VTODO-only document, CRLF line endings.
pub const VTODO_ICS: &str = "BEGIN:VCALENDAR\r\n\
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

/// Control fixture: the same assignment expressed as a VEVENT (the
/// `windows/` half of the u_crawler output). Must keep working.
pub const VEVENT_CONTROL_ICS: &str = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//u_crawler//EN\r\n\
CALSCALE:GREGORIAN\r\n\
BEGIN:VEVENT\r\n\
UID:window-123@u-crawler\r\n\
DTSTAMP:20260801T090000Z\r\n\
DTSTART:20260815T100000Z\r\n\
DTEND:20260815T120000Z\r\n\
SUMMARY:Assignment 3 - submission window\r\n\
DESCRIPTION:Submission window for assignment 3.\r\n\
URL:https://example.edu/courses/cs101/assignments/123\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

/// Probe fixture: a valid VCALENDAR with no components at all.
pub const EMPTY_VCALENDAR_ICS: &str = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//u_crawler//EN\r\n\
END:VCALENDAR\r\n";

/// Write a raw ICS document into a directory, returning its path.
pub fn write_ics(dir: &std::path::Path, filename: &str, contents: &str) -> PathBuf {
    let path = dir.join(filename);
    std::fs::write(&path, contents).unwrap();
    path
}

/// A VTODO whose `DUE` carries a TZID and whose `COMPLETED` is floating
/// (non-UTC) — the two properties the icalendar crate's typed accessors
/// silently lose (`get_completed()` returns `None` for a non-UTC value while
/// retaining the raw property).
pub const VTODO_ZONED_ICS: &str = "BEGIN:VCALENDAR\r\n\
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

// --- P1-05 production fixtures ---
//
// Extracted and redacted from `docs/vtodo/caldir-vtodo-fixtures.tar.gz` (a
// real u_crawler archive, not committed). Redaction replaces the account
// identifier, the institution hostname, and the internal course/assignment
// numbers; it changes no property shape. `SUMMARY` text, `UID` domain and
// every `DUE`/`DTSTAMP`/`DTSTART`/`DTEND` value are preserved byte-for-byte.

/// `deadlines/assignment-1571380.ics`, course `202615_2562_...`. Production's
/// open (non-completed) task shape: `UID`, `DTSTAMP`, `DUE`, `PRIORITY`,
/// `SUMMARY`, `URL` and nothing else — the 57/57 census in P1-05's
/// Correction 1. `DUE` is preserved byte-for-byte: a UTC instant at
/// `T035959Z`, 23:59:59 on the previous local `America/Santiago` day (C-15).
/// Redacted: account/course/assignment identifiers in `URL`; the institution
/// hostname (a LMS product name) -> `lms.example.edu`.
pub const VTODO_OPEN_PRODUCTION_ICS: &str = "BEGIN:VCALENDAR\r\n\
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

/// `windows/assignment-1571380.ics`, the `VEVENT` matching
/// `VTODO_OPEN_PRODUCTION_ICS`. Production keeps distinct UIDs across the
/// `deadlines/`/`windows/` pair for the same assignment (P1-05 Correction 2)
/// — this is NOT a same-UID `VEVENT`+`VTODO` case. Redacted identically to
/// its task counterpart, with the matching assignment number.
pub const VEVENT_WINDOW_PRODUCTION_ICS: &str = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//u_crawler//calendar-sync//EN\r\n\
BEGIN:VEVENT\r\n\
UID:u_crawler-window-9001571@u-crawler.local\r\n\
DTSTAMP:20260610T191757Z\r\n\
DTSTART:20260826T040000Z\r\n\
DTEND:20260902T035959Z\r\n\
SUMMARY:Semana 11: Sumativa 5: Solemne 2\r\n\
URL:https://lms.example.edu/courses/900223/assignments/9001571\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

/// `deadlines/assignment-1562539.ics`, course `202615_3049_...`. Production's
/// completed task shape: `STATUS:COMPLETED` with **neither** `COMPLETED` nor
/// `PERCENT-COMPLETE` — R68's exact Radicale case, invisible against the
/// richer `VTODO_ICS` above. `DUE` is preserved byte-for-byte: a UTC instant
/// at `T035959Z` (C-15/C-18). `SUMMARY` carries non-ASCII UTF-8 ("aquí") —
/// true of 40/57 production summaries and of no other shipped fixture — and
/// its embedded `:` and non-ASCII characters are load-bearing, so neither is
/// touched by redaction. Redacted: account/course/assignment identifiers in
/// `UID`/`URL`; the institution hostname.
pub const VTODO_COMPLETED_PRODUCTION_ICS: &str = "BEGIN:VCALENDAR\r\n\
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

/// Parse a single-VTODO document into the crate's `icalendar::Todo`.
/// Panics unless the document holds exactly one VTODO.
pub fn parse_icalendar_todo(ics: &str) -> icalendar::Todo {
    let calendar: icalendar::Calendar = ics.parse().expect("fixture parses as a VCALENDAR");
    let todos: Vec<icalendar::Todo> = calendar
        .components
        .iter()
        .filter_map(|component| component.as_todo().cloned())
        .collect();

    assert_eq!(todos.len(), 1, "fixture must hold exactly one VTODO");
    todos.into_iter().next().unwrap()
}
