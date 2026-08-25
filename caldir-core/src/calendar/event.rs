mod error;

use crate::{Calendar, CalendarItem, Event, EventTime, ParticipationStatus};
use std::fs::OpenOptions;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
pub use error::CalendarEventError;

#[derive(Debug)]
pub struct CalendarEvent {
    item: CalendarItem,
    path: PathBuf,
}

impl CalendarEvent {
    pub fn create(
        calendar: &Calendar,
        item: impl Into<CalendarItem>,
    ) -> Result<Self, CalendarEventError> {
        let item = item.into();
        let base_slug = item.base_slug();
        let contents = item.to_ics_string();

        let path = write_best_event_file(calendar.path(), &base_slug, None, contents.as_bytes())?;
        sync_file_mtime(&path, item.last_modified())?;

        Ok(CalendarEvent { item, path })
    }

    /// Load whichever supported item the file holds (R5, R32).
    pub fn load(path: impl Into<PathBuf>) -> Result<Self, CalendarEventError> {
        let path = path.into();
        let item = CalendarItem::load(&path)?;

        Ok(CalendarEvent { item, path })
    }

    pub fn update(&mut self, item: impl Into<CalendarItem>) -> Result<(), CalendarEventError> {
        let item = item.into();
        let base_slug = item.base_slug();
        let contents = item.to_ics_string();
        let dir = self.path.parent().unwrap_or_else(|| Path::new("."));

        let new_path =
            write_best_event_file(dir, &base_slug, Some(&self.path), contents.as_bytes())?;
        sync_file_mtime(&new_path, item.last_modified())?;

        if new_path == self.path {
            self.item = item;
            return Ok(());
        }

        if let Err(err) = std::fs::remove_file(&self.path) {
            let _ = std::fs::remove_file(&new_path);
            return Err(err.into());
        }

        self.item = item;
        self.path = new_path;

        Ok(())
    }

    pub fn delete(self) -> Result<(), CalendarEventError> {
        std::fs::remove_file(self.path).map_err(Into::into)
    }

    /// The stored item, whatever its kind.
    pub fn item(&self) -> &CalendarItem {
        &self.item
    }

    /// Narrowing accessor: `Some` only when the stored item is an event.
    ///
    /// A `CalendarEvent` can now hold a task, so every VEVENT-only caller has
    /// to answer the kind question here rather than assume it.
    pub fn event(&self) -> Option<&Event> {
        match &self.item {
            CalendarItem::Event(event) => Some(event),
            CalendarItem::Todo(_) => None,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn filename(&self) -> Option<&str> {
        self.path.file_name().and_then(|name| name.to_str())
    }

    // File mtime
    pub fn modified_at(&self) -> Option<DateTime<Utc>> {
        std::fs::metadata(self.path())
            .ok()
            .and_then(|m| m.modified().ok())
            .map(DateTime::<Utc>::from)
    }

    pub fn update_attendee_status(
        &mut self,
        email: &str,
        status: ParticipationStatus,
    ) -> Result<(), CalendarEventError> {
        // An RSVP is meaningless on a task (R81).
        let mut event = self.event().ok_or(CalendarEventError::NotAnEvent)?.clone();

        event.set_attendee_status(email, status)?;

        event.last_modified = Some(Utc::now());

        // Save file
        self.update(event)
    }

    /// Add an EXDATE to a recurring master
    pub(crate) fn add_exdate(&mut self, exdate: EventTime) -> Result<(), CalendarEventError> {
        // Only a recurring VEVENT master carries EXDATEs.
        let mut event = self.event().ok_or(CalendarEventError::NotAnEvent)?.clone();

        let Some(recurrence) = event.recurrence.as_mut() else {
            return Err(CalendarEventError::NotRecurring(
                event.uid.as_str().to_string(),
            ));
        };

        let exdate_utc = exdate.to_utc();

        if recurrence
            .exdates
            .iter()
            .any(|ex| ex.to_utc() == exdate_utc)
        {
            return Ok(());
        }

        recurrence.exdates.push(exdate);

        event.last_modified = Some(Utc::now());
        event.sequence += 1;

        self.update(event)
    }
}

// Pin the file mtime to the event's LAST-MODIFIED so direction detection
// reflects when the event was changed, not when bytes hit disk. Without this,
// every pull leaves the file appearing newer than its remote counterpart —
// any later code change that adds a parsed property would surface as an
// outgoing push.
fn sync_file_mtime(
    path: &Path,
    last_modified: Option<DateTime<Utc>>,
) -> Result<(), CalendarEventError> {
    let Some(ts) = last_modified else {
        return Ok(());
    };
    let ft = filetime::FileTime::from_unix_time(ts.timestamp(), ts.timestamp_subsec_nanos());
    filetime::set_file_mtime(path, ft)?;
    Ok(())
}

fn write_best_event_file(
    calendar_dir: &Path,
    base_slug: &str,
    current_path: Option<&Path>,
    contents: &[u8],
) -> Result<PathBuf, CalendarEventError> {
    let mut suffix = 1;

    loop {
        let filename = if suffix == 1 {
            format!("{base_slug}.ics")
        } else {
            format!("{base_slug}-{suffix}.ics")
        };
        let path = calendar_dir.join(filename);

        if current_path == Some(path.as_path()) {
            std::fs::write(&path, contents)?;
            return Ok(path);
        }

        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                if let Err(err) = file.write_all(contents) {
                    let _ = std::fs::remove_file(&path);
                    return Err(err.into());
                }

                return Ok(path);
            }
            Err(err) if err.kind() == ErrorKind::AlreadyExists => {
                suffix += 1;
            }
            Err(err) => return Err(err.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Attendee;
    use crate::event::EventError;
    use crate::test_utils::test_calendar;
    use crate::test_utils::test_calendar_event;
    use crate::test_utils::test_event;
    use crate::test_utils::test_event_time;
    use chrono::TimeZone;
    use std::fs;

    fn cal_event_with_attendees(attendees: Vec<Attendee>) -> (tempfile::TempDir, CalendarEvent) {
        let (tmp, calendar) = test_calendar();
        let mut event = test_event();
        event.attendees = attendees;
        let cal_event = CalendarEvent::create(&calendar, event).unwrap();
        (tmp, cal_event)
    }

    #[test]
    fn create_saves_event_to_file() {
        let (_tmp, calendar) = test_calendar();
        let cal_event = CalendarEvent::create(&calendar, test_event()).unwrap();

        assert!(cal_event.path().is_file());
        assert_eq!(
            cal_event.filename(),
            Some("2026-01-01T1200__test-event.ics")
        );
    }

    #[test]
    fn create_generates_unique_filenames_within_calendar() {
        let (_tmp, calendar) = test_calendar();

        let cal_event_1 = CalendarEvent::create(&calendar, test_event()).unwrap();

        assert_eq!(
            cal_event_1.filename(),
            Some("2026-01-01T1200__test-event.ics")
        );

        let cal_event_2 = CalendarEvent::create(&calendar, test_event()).unwrap();

        assert_eq!(
            cal_event_2.filename(),
            Some("2026-01-01T1200__test-event-2.ics")
        );

        let cal_event_3 = CalendarEvent::create(&calendar, test_event()).unwrap();

        assert_eq!(
            cal_event_3.filename(),
            Some("2026-01-01T1200__test-event-3.ics")
        );
    }

    #[test]
    fn create_keeps_base_filenames_in_different_calendars() {
        let (_tmp, calendar_1) = test_calendar();
        let cal_event_1 = CalendarEvent::create(&calendar_1, test_event()).unwrap();

        let (_tmp, calendar_2) = test_calendar();
        let cal_event_2 = CalendarEvent::create(&calendar_2, test_event()).unwrap();

        assert_eq!(
            cal_event_1.filename().unwrap(),
            cal_event_2.filename().unwrap()
        );
    }

    #[test]
    fn load_errors_on_missing_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("missing.ics");

        let err = CalendarEvent::load(path).unwrap_err();

        assert!(matches!(err, CalendarEventError::NotFound(p) if p.ends_with("missing.ics")));
    }

    #[test]
    fn load_errors_on_invalid_ics() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("test.ics");
        fs::write(&path, "BEGIN:VCALENDAR").unwrap(); // Missing END

        let err = CalendarEvent::load(path).unwrap_err();

        assert!(matches!(err, CalendarEventError::InvalidEvent(p, _) if p.ends_with("test.ics")));
    }

    #[test]
    fn load_parses_valid_ics() {
        let ics = "BEGIN:VCALENDAR\nVERSION:2.0\nBEGIN:VEVENT\nUID:test-uid@caldir\nDTSTART:20240101T120000Z\nSUMMARY:Test Event\nEND:VEVENT\nEND:VCALENDAR";
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("test.ics");
        fs::write(&path, ics).unwrap();

        assert!(CalendarEvent::load(path).is_ok());
    }

    #[test]
    fn update_renames_file_when_summary_changes() {
        let (_tmp, mut cal_event) = test_calendar_event();

        assert_eq!(
            cal_event.filename(),
            Some("2026-01-01T1200__test-event.ics")
        );

        let mut event = cal_event.event().unwrap().clone();
        event.summary = Some("Planning Session".to_string());
        cal_event.update(event).unwrap();

        assert_eq!(
            cal_event.filename(),
            Some("2026-01-01T1200__planning-session.ics")
        );
    }

    #[test]
    fn update_keeps_filename_when_other_properties_change() {
        let (_tmp, mut cal_event) = test_calendar_event();

        assert_eq!(
            cal_event.filename(),
            Some("2026-01-01T1200__test-event.ics")
        );

        let mut event = cal_event.event().unwrap().clone();
        event.location = Some("Conference Room".to_string());
        cal_event.update(event).unwrap();

        assert_eq!(
            cal_event.filename(),
            Some("2026-01-01T1200__test-event.ics")
        );

        let contents = fs::read_to_string(cal_event.path()).unwrap();
        assert!(contents.contains("LOCATION:Conference Room"));
    }

    #[test]
    fn create_pins_file_mtime_to_event_last_modified() {
        let (_tmp, calendar) = test_calendar();
        let mut event = test_event();
        let when = Utc.with_ymd_and_hms(2025, 6, 15, 12, 0, 0).unwrap();
        event.last_modified = Some(when);

        let cal_event = CalendarEvent::create(&calendar, event).unwrap();

        assert_eq!(cal_event.modified_at(), Some(when));
    }

    #[test]
    fn create_leaves_file_mtime_alone_when_last_modified_is_none() {
        // Fresh local events have no remote LAST-MODIFIED yet — fall back to
        // the OS's wall-clock write time so the file's mtime still tracks
        // local edits.
        let (_tmp, calendar) = test_calendar();
        let event = test_event();
        assert!(event.last_modified.is_none());

        let before = Utc::now();
        let cal_event = CalendarEvent::create(&calendar, event).unwrap();
        let after = Utc::now();

        let mtime = cal_event.modified_at().expect("file should have mtime");
        assert!(mtime >= before - chrono::Duration::seconds(1));
        assert!(mtime <= after + chrono::Duration::seconds(1));
    }

    #[test]
    fn update_pins_file_mtime_to_new_last_modified() {
        let (_tmp, mut cal_event) = test_calendar_event();
        let mut event = cal_event.event().unwrap().clone();
        let when = Utc.with_ymd_and_hms(2025, 6, 15, 12, 0, 0).unwrap();
        event.last_modified = Some(when);

        cal_event.update(event).unwrap();

        assert_eq!(cal_event.modified_at(), Some(when));
    }

    #[test]
    fn update_attendee_status_sets_partstat_and_persists() {
        let (_tmp, mut cal_event) =
            cal_event_with_attendees(vec![Attendee::new("bob@example.com")]);

        cal_event
            .update_attendee_status("bob@example.com", ParticipationStatus::Accepted)
            .unwrap();

        assert_eq!(
            cal_event.event().unwrap().attendees[0].status,
            Some(ParticipationStatus::Accepted)
        );

        let contents = fs::read_to_string(cal_event.path()).unwrap();
        assert!(contents.contains("PARTSTAT=ACCEPTED"));
    }

    #[test]
    fn update_attendee_status_preserves_sequence_and_sets_last_modified() {
        let (_tmp, mut cal_event) =
            cal_event_with_attendees(vec![Attendee::new("bob@example.com")]);
        let sequence_before = cal_event.event().unwrap().sequence;

        let before = Utc::now();
        cal_event
            .update_attendee_status("bob@example.com", ParticipationStatus::Declined)
            .unwrap();
        let after = Utc::now();

        assert_eq!(cal_event.event().unwrap().sequence, sequence_before);
        let last_modified = cal_event
            .event()
            .unwrap()
            .last_modified
            .expect("last_modified should be set");
        assert!(last_modified >= before);
        assert!(last_modified <= after);
    }

    #[test]
    fn update_attendee_status_matches_email_case_insensitively() {
        let (_tmp, mut cal_event) =
            cal_event_with_attendees(vec![Attendee::new("Bob@Example.com")]);

        cal_event
            .update_attendee_status("BOB@example.com", ParticipationStatus::Tentative)
            .unwrap();

        assert_eq!(
            cal_event.event().unwrap().attendees[0].status,
            Some(ParticipationStatus::Tentative)
        );
    }

    #[test]
    fn update_attendee_status_errors_on_unknown_email() {
        let (_tmp, mut cal_event) =
            cal_event_with_attendees(vec![Attendee::new("bob@example.com")]);

        let err = cal_event
            .update_attendee_status("carol@example.com", ParticipationStatus::Accepted)
            .unwrap_err();

        assert!(matches!(
            err,
            CalendarEventError::Event(EventError::AttendeeNotFound { email })
                if email == "carol@example.com"
        ));
    }

    #[test]
    fn update_updates_filename_to_base_when_base_is_available() {
        let (_tmp, calendar) = test_calendar();

        let cal_event_1 = CalendarEvent::create(&calendar, test_event()).unwrap();
        let mut cal_event_2 = CalendarEvent::create(&calendar, test_event()).unwrap();

        assert_eq!(
            cal_event_2.filename(),
            Some("2026-01-01T1200__test-event-2.ics")
        );

        // Delete original event that had "test-event" slug
        cal_event_1
            .delete()
            .expect("Failed to delete calendar event");

        // "test-event" slug is now available for cal_event_2 to use:
        cal_event_2
            .update(cal_event_2.event().unwrap().clone())
            .unwrap();

        assert_eq!(
            cal_event_2.filename(),
            Some("2026-01-01T1200__test-event.ics")
        );
    }

    fn recurring_cal_event() -> (tempfile::TempDir, CalendarEvent) {
        let (tmp, calendar) = test_calendar();
        let mut event = test_event();
        event.recurrence = Some(crate::Recurrence::new("FREQ=DAILY"));
        let cal_event = CalendarEvent::create(&calendar, event).unwrap();
        (tmp, cal_event)
    }

    #[test]
    fn add_exdate_records_exclusion_and_bumps_sequence() {
        let (_tmp, mut cal_event) = recurring_cal_event();
        let exdate = EventTime::DateTimeUtc(Utc.with_ymd_and_hms(2026, 1, 3, 12, 0, 0).unwrap());

        cal_event.add_exdate(exdate.clone()).unwrap();

        assert_eq!(
            cal_event
                .event()
                .unwrap()
                .recurrence
                .as_ref()
                .unwrap()
                .exdates,
            vec![exdate]
        );
        assert_eq!(cal_event.event().unwrap().sequence, 1);
    }

    #[test]
    fn add_exdate_is_idempotent() {
        let (_tmp, mut cal_event) = recurring_cal_event();
        let exdate = EventTime::DateTimeUtc(Utc.with_ymd_and_hms(2026, 1, 3, 12, 0, 0).unwrap());

        cal_event.add_exdate(exdate.clone()).unwrap();
        cal_event.add_exdate(exdate.clone()).unwrap();

        // No duplicate EXDATE, and the second call didn't bump SEQUENCE again.
        assert_eq!(
            cal_event
                .event()
                .unwrap()
                .recurrence
                .as_ref()
                .unwrap()
                .exdates,
            vec![exdate]
        );
        assert_eq!(cal_event.event().unwrap().sequence, 1);
    }

    #[test]
    fn add_exdate_errors_when_event_is_not_recurring() {
        let (_tmp, mut cal_event) = test_calendar_event();
        assert!(cal_event.event().unwrap().recurrence.is_none());

        let err = cal_event
            .add_exdate(EventTime::DateTimeUtc(
                Utc.with_ymd_and_hms(2026, 1, 3, 12, 0, 0).unwrap(),
            ))
            .unwrap_err();

        assert!(matches!(err, CalendarEventError::NotRecurring(_)));
    }

    // --- VTODO support (RED regression tests) ---

    #[test]
    fn load_reads_a_vtodo_file() {
        use crate::test_utils::{VTODO_ICS, write_ics};

        let (_tmp, calendar) = test_calendar();
        let path = write_ics(calendar.path(), "assignment-123.ics", VTODO_ICS);

        match CalendarEvent::load(&path) {
            Ok(cal_event) => assert_eq!(
                cal_event.item().summary(),
                Some("Assignment 3 - Recursive descent parser")
            ),
            Err(CalendarEventError::ExpectedSingleEvent { found, .. }) => panic!(
                "VTODO file rejected at caldir-core/src/calendar/event.rs:45 with found={found}: \
                 icalendar::Calendar::events() yields no VEVENT for a VTODO document"
            ),
            Err(err) => panic!("VTODO file rejected for an unexpected reason: {err:?}"),
        }
    }

    /// Control: the VEVENT half of the same u_crawler output must keep loading.
    #[test]
    fn load_reads_the_vevent_control_file() {
        use crate::test_utils::{VEVENT_CONTROL_ICS, write_ics};

        let (_tmp, calendar) = test_calendar();
        let path = write_ics(calendar.path(), "window-123.ics", VEVENT_CONTROL_ICS);

        let cal_event = CalendarEvent::load(&path).unwrap();

        assert_eq!(
            cal_event.event().unwrap().summary.as_deref(),
            Some("Assignment 3 - submission window")
        );
    }

    // --- R32/R75: the eight-row arity table, against the item loader ---

    const ONE_VEVENT: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VEVENT\r\nUID:one@caldir\r\nDTSTART:20260301T100000Z\r\nSUMMARY:One\r\n\
END:VEVENT\r\nEND:VCALENDAR\r\n";

    const ONE_VTODO: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VTODO\r\nUID:one-task@caldir\r\nSUMMARY:One task\r\n\
END:VTODO\r\nEND:VCALENDAR\r\n";

    const TWO_VEVENTS: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VEVENT\r\nUID:a@caldir\r\nDTSTART:20260301T100000Z\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:b@caldir\r\nDTSTART:20260302T100000Z\r\nEND:VEVENT\r\n\
END:VCALENDAR\r\n";

    const TWO_VTODOS: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VTODO\r\nUID:a@caldir\r\nEND:VTODO\r\n\
BEGIN:VTODO\r\nUID:b@caldir\r\nEND:VTODO\r\n\
END:VCALENDAR\r\n";

    const VEVENT_AND_VTODO: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VEVENT\r\nUID:a@caldir\r\nDTSTART:20260301T100000Z\r\nEND:VEVENT\r\n\
BEGIN:VTODO\r\nUID:b@caldir\r\nEND:VTODO\r\n\
END:VCALENDAR\r\n";

    const VTODO_AND_VJOURNAL: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VTODO\r\nUID:a@caldir\r\nEND:VTODO\r\n\
BEGIN:VJOURNAL\r\nUID:b@caldir\r\nEND:VJOURNAL\r\n\
END:VCALENDAR\r\n";

    const VEVENT_AND_VTIMEZONE: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VTIMEZONE\r\nTZID:Europe/Stockholm\r\n\
BEGIN:STANDARD\r\nDTSTART:19701025T030000\r\nTZOFFSETFROM:+0200\r\n\
TZOFFSETTO:+0100\r\nTZNAME:CET\r\nEND:STANDARD\r\nEND:VTIMEZONE\r\n\
BEGIN:VEVENT\r\nUID:zoned@caldir\r\nDTSTART;TZID=Europe/Stockholm:20260615T100000\r\n\
SUMMARY:Zoned\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    #[test]
    fn arity_table_all_eight_rows() {
        use crate::CalendarItem;
        use crate::test_utils::{EMPTY_VCALENDAR_ICS, write_ics};

        let (_tmp, calendar) = test_calendar();
        let dir = calendar.path();

        let load = |name: &str, ics: &str| CalendarItem::load(write_ics(dir, name, ics));

        // Row 1 — 1 VEVENT: loads as an event.
        assert!(matches!(
            load("one-vevent.ics", ONE_VEVENT),
            Ok(CalendarItem::Event(ref event)) if event.uid.as_str() == "one@caldir"
        ));

        // Row 2 — 1 VTODO: loads as a task.
        assert!(matches!(
            load("one-vtodo.ics", ONE_VTODO),
            Ok(CalendarItem::Todo(ref todo)) if todo.uid.as_str() == "one-task@caldir"
        ));

        // Row 3 — 0 components: "none", distinguishable from "too many".
        let err = load("empty.ics", EMPTY_VCALENDAR_ICS).unwrap_err();
        assert!(
            matches!(err, CalendarEventError::NoSupportedComponent { ref found, .. } if found == "nothing"),
            "expected NoSupportedComponent {{ found: \"nothing\" }}, got {err:?}"
        );
        assert!(
            err.to_string().starts_with("no VEVENT or VTODO in ")
                && err.to_string().ends_with(" (found: nothing)"),
            "R75 wording, got: {err}"
        );

        // Row 4 — 2 VEVENTs: too many.
        let err = load("two-vevents.ics", TWO_VEVENTS).unwrap_err();
        assert!(
            matches!(
                err,
                CalendarEventError::ExpectedSingleComponent { found: 2, .. }
            ),
            "expected ExpectedSingleComponent {{ found: 2 }}, got {err:?}"
        );
        assert!(
            err.to_string()
                .starts_with("expected exactly one VEVENT or VTODO in ")
                && err
                    .to_string()
                    .ends_with(", found 2; split it into one file per item"),
            "R75 wording, got: {err}"
        );

        // Row 5 — 2 VTODOs: too many (today this reports found: 0).
        assert!(
            matches!(
                load("two-vtodos.ics", TWO_VTODOS),
                Err(CalendarEventError::ExpectedSingleComponent { found: 2, .. })
            ),
            "two VTODOs must count as two"
        );

        // Row 6 — 1 VEVENT + 1 VTODO: too many. Today this loads and silently
        // drops the VTODO, which a rewrite then deletes (R101).
        assert!(
            matches!(
                load("mixed.ics", VEVENT_AND_VTODO),
                Err(CalendarEventError::ExpectedSingleComponent { found: 2, .. })
            ),
            "a mixed file must error on the count, never drop the VTODO"
        );

        // Row 7 — 1 VTODO + VJOURNAL: errors naming the unsupported component.
        let err = load("journal.ics", VTODO_AND_VJOURNAL).unwrap_err();
        assert!(
            err.to_string().contains("VJOURNAL"),
            "the error must name the unsupported component, got: {err}"
        );

        // Row 8 — 1 VEVENT + VTIMEZONE: unchanged; VTIMEZONE is not counted
        // and is not unsupported.
        assert!(matches!(
            load("zoned.ics", VEVENT_AND_VTIMEZONE),
            Ok(CalendarItem::Event(ref event)) if event.uid.as_str() == "zoned@caldir"
        ));
    }

    /// A VTIMEZONE-only file has no supported component, but VTIMEZONE is not
    /// an *unsupported* one — the "none" error names what was actually there.
    #[test]
    fn a_vtimezone_only_file_is_none_not_unsupported() {
        use crate::CalendarItem;
        use crate::test_utils::write_ics;

        let (_tmp, calendar) = test_calendar();
        let ics = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VTIMEZONE\r\nTZID:Europe/Stockholm\r\nEND:VTIMEZONE\r\nEND:VCALENDAR\r\n";
        let path = write_ics(calendar.path(), "tz-only.ics", ics);

        let err = CalendarItem::load(path).unwrap_err();

        assert!(
            matches!(err, CalendarEventError::NoSupportedComponent { ref found, .. } if found == "VTIMEZONE"),
            "expected NoSupportedComponent {{ found: \"VTIMEZONE\" }}, got {err:?}"
        );
    }

    // --- L2-02: the retype to `CalendarItem` must not move an event's bytes ---

    /// The event corpus for the byte-identity lock: a fixed UID so the file is
    /// reproducible, plus two properties either side of `SUMMARY`
    /// alphabetically so a reordering regression would show.
    fn byte_identity_event() -> Event {
        let mut event = Event::new("Test Event", test_event_time());
        event.uid = crate::EventUid::new("byte-identity@caldir");
        event.description = Some("Line one".to_string());
        event.location = Some("Room 1".to_string());
        event
    }

    /// The written file, with the one line that cannot be a literal (`DTSTAMP`
    /// is stamped at write time, R25) removed.
    fn bytes_without_dtstamp(path: &Path) -> String {
        let raw = fs::read_to_string(path).unwrap();
        assert!(raw.contains("DTSTAMP:"), "DTSTAMP must still be written");

        raw.lines()
            .filter(|line| !line.starts_with("DTSTAMP"))
            .map(|line| format!("{line}\r\n"))
            .collect()
    }

    /// C1/C2 (INVIOLABLE). Storing a `CalendarItem` instead of an `Event` is
    /// the change most likely to move an event's filename or file body without
    /// any existing test noticing, because every event test reaches its value
    /// through the legacy accessor and would keep passing.
    ///
    /// The expected strings are literals captured from the pre-retype writer,
    /// never a re-invocation of `base_slug()` / `to_ics_string()`.
    #[test]
    fn an_item_backed_calendar_event_is_still_byte_identical_for_events() {
        const CREATED_ICS: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:CALDIR\r\n\
BEGIN:VEVENT\r\nDESCRIPTION:Line one\r\nDTSTART:20260101T120000\r\nLOCATION:Room 1\r\n\
SUMMARY:Test Event\r\nUID:byte-identity@caldir\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

        const UPDATED_ICS: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:CALDIR\r\n\
BEGIN:VEVENT\r\nDESCRIPTION:Line one\r\nDTSTART:20260101T120000\r\nLOCATION:Room 1\r\n\
SUMMARY:Planning Session\r\nUID:byte-identity@caldir\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

        let (_tmp, calendar) = test_calendar();

        // create
        let mut cal_event = CalendarEvent::create(&calendar, byte_identity_event()).unwrap();
        assert_eq!(
            cal_event.filename(),
            Some("2026-01-01T1200__test-event.ics")
        );
        assert_eq!(bytes_without_dtstamp(cal_event.path()), CREATED_ICS);

        // update
        let mut updated = cal_event.event().unwrap().clone();
        updated.summary = Some("Planning Session".to_string());
        cal_event.update(updated).unwrap();
        assert_eq!(
            cal_event.filename(),
            Some("2026-01-01T1200__planning-session.ics")
        );
        assert_eq!(bytes_without_dtstamp(cal_event.path()), UPDATED_ICS);

        // load
        let reloaded = CalendarEvent::load(cal_event.path()).unwrap();
        assert_eq!(
            reloaded.event().unwrap().uid.as_str(),
            "byte-identity@caldir"
        );
        assert_eq!(
            reloaded.event().unwrap().summary.as_deref(),
            Some("Planning Session")
        );
    }

    /// The write path takes the filename from the item's own kind, not from
    /// the event slug rule (R33, R34). Without this, a task would be filed
    /// under an event-shaped name.
    #[test]
    fn create_files_a_task_under_the_task_slug_with_a_vtodo_body() {
        use crate::Todo;

        let (_tmp, calendar) = test_calendar();

        let mut todo = Todo::new("File taxes");
        todo.uid = crate::EventUid::new("task-write@caldir");
        todo.due = Some(test_event_time());

        let cal_event = CalendarEvent::create(&calendar, todo).unwrap();

        assert_eq!(
            cal_event.filename(),
            Some("_task__2026-01-01T1200__file-taxes.ics")
        );

        let contents = fs::read_to_string(cal_event.path()).unwrap();
        assert!(contents.contains("BEGIN:VTODO"), "got: {contents}");
        assert!(!contents.contains("BEGIN:VEVENT"), "got: {contents}");
        assert!(contents.contains("DUE:20260101T120000"), "got: {contents}");
    }

    /// `sync_file_mtime` must see the *item's* `LAST-MODIFIED`, not `None`:
    /// a task whose mtime is not pinned reads as locally-edited on every pull.
    #[test]
    fn create_pins_file_mtime_to_a_tasks_last_modified() {
        use crate::Todo;

        let (_tmp, calendar) = test_calendar();
        let when = Utc.with_ymd_and_hms(2025, 6, 15, 12, 0, 0).unwrap();

        let mut todo = Todo::new("Pinned task");
        todo.last_modified = Some(when);

        let cal_event = CalendarEvent::create(&calendar, todo).unwrap();

        assert_eq!(cal_event.modified_at(), Some(when));
    }

    /// L1-07 carry-forward. Two undated tasks sharing a summary collide on
    /// `_task__{summary}.ics` far more readily than events ever do, because an
    /// event always carries a date or the `_recurring__` prefix. Nothing
    /// proved the writer's suffix loop for the `_task__` shape until the
    /// writer became item-aware.
    #[test]
    fn two_undated_tasks_sharing_a_summary_get_distinct_files() {
        use crate::Todo;

        let (_tmp, calendar) = test_calendar();

        let first = CalendarEvent::create(&calendar, Todo::new("Someday maybe")).unwrap();
        let second = CalendarEvent::create(&calendar, Todo::new("Someday maybe")).unwrap();

        assert_eq!(first.filename(), Some("_task__someday-maybe.ics"));
        assert_eq!(second.filename(), Some("_task__someday-maybe-2.ics"));
        assert!(first.path().is_file() && second.path().is_file());
    }

    /// The narrowing accessor: `event()` answers the kind question a caller
    /// can no longer assume, and `item()` exposes the stored item itself.
    #[test]
    fn as_event_narrows_only_for_events() {
        use crate::{CalendarItem, Todo};

        let (_tmp, calendar) = test_calendar();

        let event_backed = CalendarEvent::create(&calendar, test_event()).unwrap();
        assert!(matches!(event_backed.item(), CalendarItem::Event(_)));
        assert_eq!(
            event_backed.event().map(|event| event.uid.as_str()),
            Some(event_backed.item().uid().as_str())
        );

        let task_backed = CalendarEvent::create(&calendar, Todo::new("A task")).unwrap();
        assert!(matches!(task_backed.item(), CalendarItem::Todo(_)));
        assert!(task_backed.event().is_none());
    }

    /// The two VEVENT-only mutators can now be reached with a task behind
    /// them, because `load` admits one. They must refuse it as an error —
    /// never panic, and never write an event-shaped file over a task.
    #[test]
    fn the_vevent_only_mutators_refuse_a_task() {
        use crate::Todo;

        let (_tmp, calendar) = test_calendar();
        let mut task = CalendarEvent::create(&calendar, Todo::new("A task")).unwrap();
        let bytes_before = std::fs::read(task.path()).unwrap();

        assert!(matches!(
            task.update_attendee_status("someone@example.com", ParticipationStatus::Accepted),
            Err(CalendarEventError::NotAnEvent)
        ));
        assert!(matches!(
            task.add_exdate(EventTime::DateTimeUtc(
                Utc.with_ymd_and_hms(2026, 1, 3, 12, 0, 0).unwrap()
            )),
            Err(CalendarEventError::NotAnEvent)
        ));

        assert_eq!(std::fs::read(task.path()).unwrap(), bytes_before);
    }
}
