use crate::commands::doctor::warning::DoctorWarning;
use caldir_core::{CalendarEvent, EventInstanceId};
use std::collections::HashMap;
use std::path::PathBuf;

pub(crate) fn duplicate_file_warnings(events: &[CalendarEvent]) -> Vec<DoctorWarning> {
    duplicate_file_sets(events)
        .into_iter()
        .map(DoctorWarning::DuplicateFiles)
        .collect()
}

fn duplicate_file_sets(events: &[CalendarEvent]) -> Vec<Vec<PathBuf>> {
    let mut by_id: HashMap<EventInstanceId, Vec<PathBuf>> = HashMap::new();
    for ce in events {
        // Bucket the item, never a narrowed event: the identity carries the
        // component kind (R36), so a VEVENT and a VTODO sharing a UID land in
        // two buckets, while two files of the same kind still collide.
        by_id
            .entry(ce.item().instance_id())
            .or_default()
            .push(ce.path().to_path_buf());
    }

    let mut sets: Vec<Vec<PathBuf>> = by_id
        .into_values()
        .filter(|paths| paths.len() > 1)
        .collect();

    for paths in &mut sets {
        paths.sort();
    }
    sets.sort();
    sets
}

#[cfg(test)]
mod tests {
    use super::duplicate_file_warnings;
    use crate::commands::doctor::warning::DoctorWarning;
    use caldir_core::{Calendar, Event, EventTime};
    use chrono::NaiveDate;
    use indoc::formatdoc;
    use pretty_assertions::assert_eq;

    fn test_calendar() -> (tempfile::TempDir, Calendar) {
        let tmp = tempfile::tempdir().unwrap();
        let calendar = Calendar::create(&tmp.path().join("work"), None).unwrap();
        (tmp, calendar)
    }

    fn test_event(summary: &str) -> Event {
        Event::new(
            summary,
            EventTime::DateTimeFloating(
                NaiveDate::from_ymd_opt(2026, 1, 1)
                    .unwrap()
                    .and_hms_opt(12, 0, 0)
                    .unwrap(),
            ),
        )
    }

    #[test]
    fn warns_about_multiple_files_with_the_same_event_identity() {
        let (_tmp, calendar) = test_calendar();
        let event = test_event("Standup");

        calendar.create_event(event.clone()).unwrap();
        calendar.create_event(event).unwrap();

        let warnings = duplicate_file_warnings(calendar.events().unwrap().events());

        assert_eq!(warnings.len(), 1);
        let DoctorWarning::DuplicateFiles(paths) = &warnings[0] else {
            panic!("expected duplicate file warning");
        };
        assert_eq!(paths.len(), 2);
    }

    #[test]
    fn ignores_distinct_events_with_the_same_filename_slug() {
        let (_tmp, calendar) = test_calendar();

        calendar.create_event(test_event("Standup")).unwrap();
        calendar.create_event(test_event("Standup")).unwrap();

        let warnings = duplicate_file_warnings(calendar.events().unwrap().events());

        assert!(warnings.is_empty());
    }

    /// Writes a raw `.ics` fixture into the calendar directory. The CLI crate
    /// cannot reach `caldir-core`'s `#[cfg(test)]` fixtures, so tasks are
    /// authored here as bytes.
    fn write_ics(calendar: &Calendar, filename: &str, contents: &str) {
        std::fs::write(calendar.path().join(filename), contents).unwrap();
    }

    fn vevent_ics(uid: &str) -> String {
        formatdoc! {"
            BEGIN:VCALENDAR\r
            VERSION:2.0\r
            BEGIN:VEVENT\r
            UID:{uid}\r
            SUMMARY:Assignment window\r
            DTSTART:20260301T100000\r
            DTEND:20260301T110000\r
            END:VEVENT\r
            END:VCALENDAR\r
        "}
    }

    fn vtodo_ics(uid: &str) -> String {
        formatdoc! {"
            BEGIN:VCALENDAR\r
            VERSION:2.0\r
            BEGIN:VTODO\r
            UID:{uid}\r
            SUMMARY:Hand in assignment\r
            DUE:20260301T235900\r
            END:VTODO\r
            END:VCALENDAR\r
        "}
    }

    /// T-22 — R36 hazard 5. A VEVENT and a VTODO sharing a UID are two
    /// legitimate files (R4), not a duplicate.
    #[test]
    fn an_event_task_uid_pair_is_not_a_duplicate() {
        let (_tmp, calendar) = test_calendar();
        let uid = "assignment-123@u-crawler";

        write_ics(&calendar, "window.ics", &vevent_ics(uid));
        write_ics(&calendar, "_task__hand-in-assignment.ics", &vtodo_ics(uid));

        let listing = calendar.events().unwrap();
        assert_eq!(listing.events().len(), 2, "both files must load");
        // The fixture is only a hazard-5 proof if the two items really do
        // share a UID and differ only in kind.
        let ids: Vec<_> = listing
            .events()
            .iter()
            .map(|ce| ce.item().instance_id())
            .collect();
        assert_eq!(ids[0].uid(), ids[1].uid());
        assert_ne!(ids[0].kind(), ids[1].kind());

        let warnings = duplicate_file_warnings(listing.events());

        assert!(warnings.is_empty(), "got {warnings:?}");
    }

    /// AC4 — the fix distinguishes kinds, it does not exempt tasks.
    #[test]
    fn two_tasks_sharing_a_uid_are_still_a_duplicate() {
        let (_tmp, calendar) = test_calendar();
        let uid = "assignment-123@u-crawler";

        write_ics(&calendar, "_task__one.ics", &vtodo_ics(uid));
        write_ics(&calendar, "_task__two.ics", &vtodo_ics(uid));

        let warnings = duplicate_file_warnings(calendar.events().unwrap().events());

        assert_eq!(warnings.len(), 1);
        let DoctorWarning::DuplicateFiles(paths) = &warnings[0] else {
            panic!("expected duplicate file warning");
        };
        assert_eq!(paths.len(), 2);
    }

    /// Two independent duplicate sets come back in a deterministic order, so
    /// `doctor`'s output does not reshuffle between runs.
    #[test]
    fn duplicate_sets_are_ordered_deterministically() {
        let (_tmp, calendar) = test_calendar();

        write_ics(&calendar, "b-one.ics", &vtodo_ics("bbb@u-crawler"));
        write_ics(&calendar, "b-two.ics", &vtodo_ics("bbb@u-crawler"));
        write_ics(&calendar, "a-one.ics", &vtodo_ics("aaa@u-crawler"));
        write_ics(&calendar, "a-two.ics", &vtodo_ics("aaa@u-crawler"));
        write_ics(&calendar, "c-one.ics", &vtodo_ics("ccc@u-crawler"));
        write_ics(&calendar, "c-two.ics", &vtodo_ics("ccc@u-crawler"));

        let warnings = duplicate_file_warnings(calendar.events().unwrap().events());

        assert_eq!(warnings.len(), 3);
        let names: Vec<Vec<&str>> = warnings
            .iter()
            .map(|warning| {
                let DoctorWarning::DuplicateFiles(paths) = warning else {
                    panic!("expected duplicate file warning");
                };
                paths
                    .iter()
                    .map(|path| path.file_name().unwrap().to_str().unwrap())
                    .collect()
            })
            .collect();

        assert_eq!(
            names,
            vec![
                vec!["a-one.ics", "a-two.ics"],
                vec!["b-one.ics", "b-two.ics"],
                vec!["c-one.ics", "c-two.ics"],
            ]
        );
    }

    /// C13 control — a genuine same-kind duplicate still renders the exact
    /// text it renders today, byte for byte (ANSI codes included).
    #[test]
    fn a_genuine_duplicate_still_renders_the_existing_text() {
        let (_tmp, calendar) = test_calendar();
        let event = test_event("Standup");

        calendar.create_event(event.clone()).unwrap();
        calendar.create_event(event).unwrap();

        let warnings = duplicate_file_warnings(calendar.events().unwrap().events());

        assert_eq!(warnings.len(), 1);
        let DoctorWarning::DuplicateFiles(paths) = &warnings[0] else {
            panic!("expected duplicate file warning");
        };
        let names: Vec<&str> = paths
            .iter()
            .map(|path| path.file_name().unwrap().to_str().unwrap())
            .collect();
        // Paths are sorted, so the rendered order is deterministic rather
        // than whatever order the directory listing happened to yield.
        assert_eq!(
            names,
            [
                "2026-01-01T1200__standup-2.ics",
                "2026-01-01T1200__standup.ics"
            ]
        );

        let mut rendered = Vec::new();
        warnings[0].render(&mut rendered).unwrap();

        assert_eq!(
            String::from_utf8(rendered).unwrap(),
            format!(
                "   \u{1b}[33m\u{26a0}\u{1b}[39m same event saved as multiple files; \
                 delete all but one:\n       \u{1b}[2m{first}\u{1b}[0m\n       \
                 \u{1b}[2m{second}\u{1b}[0m\n",
                first = names[0],
                second = names[1],
            )
        );
    }
}
