use crate::event::{Event, EventTime};
use chrono::Local;

const EMPTY_SUMMARY_SLUG: &str = "untitled";

impl Event {
    /// Generate a slug for an event based on its start time and summary.
    /// The slug is used as the filename for the event's .ics file.
    ///
    /// Recurring events happen on many dates, so a single date prefix would be
    /// misleading. They get a `_recurring__` prefix instead, which also groups
    /// them together when browsing the directory.
    pub fn base_slug(&self) -> String {
        if self.recurrence.is_some() {
            return format!("_recurring__{}", self.summary_slug());
        }

        format!("{}__{}", self.time_slug(), self.summary_slug())
    }

    fn summary_slug(&self) -> String {
        summary_slug(self.summary.as_deref())
    }

    fn time_slug(&self) -> String {
        time_slug(&self.start)
    }
}

/// Shared by every calendar item kind, so a task and an event with the same
/// title slug identically.
pub(crate) fn summary_slug(summary: Option<&str>) -> String {
    match summary {
        Some(summary) => {
            // Strip non-alphanumeric chars (e.g. emoji) before slugifying.
            // Otherwise `slug` transliterates symbols via `deunicode` (☕ → "coffee").
            let cleaned: String = summary
                .chars()
                .filter(|c| c.is_alphanumeric() || c.is_whitespace())
                .collect();

            // Delegate to the shared slugifier so the length cap that keeps
            // filenames under the filesystem limit lives in one place.
            let slug = crate::utils::slugify(&cleaned);

            if slug.is_empty() {
                EMPTY_SUMMARY_SLUG.to_string()
            } else {
                slug
            }
        }
        None => EMPTY_SUMMARY_SLUG.to_string(),
    }
}

/// Always uses local time (it's the most intuitive when browsing files).
/// If a co-worker on the other side of the world creates an event at 9am their time,
/// my filename should show what time it is for me, not for them.
pub(crate) fn time_slug(time: &EventTime) -> String {
    match time {
        // date only:
        EventTime::Date(date) => date.format("%Y-%m-%d").to_string(),

        // date + time:
        _ => time.to_local_tz(&Local).format("%Y-%m-%dT%H%M").to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{NaiveDate, TimeZone, Utc};
    use pretty_assertions::assert_eq;

    /// T-7 (R34, R35). One case per row of R34's table, plus the two halves of
    /// the date-source rule — the only part of R34 that can be implemented
    /// backwards while still producing plausible filenames.
    ///
    /// `base_slug()` is the filename *stem*; `.ics` is appended by the writer,
    /// exactly as for an event.
    #[test]
    fn task_filenames_cover_recurring_dated_and_undated() {
        use crate::todo::Todo;

        let date = |y, m, d| EventTime::Date(NaiveDate::from_ymd_opt(y, m, d).unwrap());
        let floating = |y, m, d, h, min| {
            EventTime::DateTimeFloating(
                NaiveDate::from_ymd_opt(y, m, d)
                    .unwrap()
                    .and_hms_opt(h, min, 0)
                    .unwrap(),
            )
        };

        // Row 1 — recurring: no date component, ever.
        let mut recurring = Todo::new("Water the plants");
        recurring.due = Some(date(2024, 1, 1));
        recurring.recurrence = Some(crate::event::Recurrence::new("FREQ=WEEKLY"));
        assert_eq!(recurring.base_slug(), "_task__recurring__water-the-plants");

        // Row 2 — dated, date value.
        let mut dated = Todo::new("File taxes");
        dated.due = Some(date(2024, 4, 15));
        assert_eq!(dated.base_slug(), "_task__2024-04-15__file-taxes");

        // Row 2 — dated, datetime value: `time_slug` reused verbatim.
        let mut dated_timed = Todo::new("File taxes");
        dated_timed.due = Some(floating(2024, 4, 15, 17, 0));
        assert_eq!(
            dated_timed.base_slug(),
            "_task__2024-04-15T1700__file-taxes"
        );

        // Row 3 — undated: no date component and none fabricated (R46).
        let undated = Todo::new("Someday maybe");
        assert_eq!(undated.base_slug(), "_task__someday-maybe");

        // Undated with no usable summary still falls back to `untitled`.
        let mut untitled = Todo::new("");
        untitled.summary = None;
        assert_eq!(untitled.base_slug(), "_task__untitled");

        // Date source: DUE wins when both are present.
        let mut both = Todo::new("Ship release");
        both.start = Some(date(2024, 1, 2));
        both.due = Some(date(2024, 3, 9));
        assert_eq!(both.base_slug(), "_task__2024-03-09__ship-release");

        // Date source: DTSTART is the fallback when DUE is absent.
        let mut start_only = Todo::new("Ship release");
        start_only.start = Some(date(2024, 1, 2));
        assert_eq!(start_only.base_slug(), "_task__2024-01-02__ship-release");

        // A task titled "recurring …" cannot forge the reserved recurring form:
        // the summary slugifier can never emit `__`.
        let forger = Todo::new("recurring water the plants");
        assert_eq!(forger.base_slug(), "_task__recurring-water-the-plants");
        assert_ne!(forger.base_slug(), recurring.base_slug());

        // …and that holds for every adversarial spelling: the whole reserved
        // prefix scheme rests on `__` being unreachable from a summary.
        for adversarial in [
            "recurring__x",
            "task__recurring__x",
            "_task__recurring__x",
            "a__b",
            "a  b",
            "a..b",
            "a--b",
            "a_b",
            "a___b",
            "a!!!b",
            "a\u{00a0}\u{00a0}b",
            "_ _ _",
            "___",
            "☕__☕",
        ] {
            let slug = summary_slug(Some(adversarial));
            assert!(
                !slug.contains("__"),
                "summary {adversarial:?} produced {slug:?}, which contains the reserved separator"
            );
            assert!(
                !Todo::new(adversarial)
                    .base_slug()
                    .starts_with("_task__recurring__"),
                "summary {adversarial:?} forged the reserved recurring prefix"
            );
        }

        // Zone-dependent DUE: expectation from chrono, not from `EventTime`.
        let instant = Utc.with_ymd_and_hms(2024, 4, 15, 17, 0, 0).unwrap();
        let mut zoned = Todo::new("File taxes");
        zoned.due = Some(EventTime::DateTimeUtc(instant));
        let expected_local = instant
            .with_timezone(&Local)
            .format("%Y-%m-%dT%H%M")
            .to_string();
        assert_eq!(
            zoned.base_slug(),
            format!("_task__{expected_local}__file-taxes")
        );
    }

    /// T-6 (R33, C1 — INVIOLABLE). A literal corpus locking `Event::base_slug()`.
    ///
    /// The expected strings are literals, never a re-invocation of `base_slug()`:
    /// this is a lock against a mass rename of every user's `.ics` files, so it
    /// must be able to disagree with the code. The two zone-dependent shapes at
    /// the end derive their expectation from `chrono` directly (the wording of
    /// R34 — "local-tz `%Y-%m-%dT%H%M`") rather than from `EventTime`.
    #[test]
    fn base_slug_is_unchanged_for_vevent_corpus() {
        let date = |y, m, d| EventTime::Date(NaiveDate::from_ymd_opt(y, m, d).unwrap());
        let floating = |y, m, d, h, min| {
            EventTime::DateTimeFloating(
                NaiveDate::from_ymd_opt(y, m, d)
                    .unwrap()
                    .and_hms_opt(h, min, 0)
                    .unwrap(),
            )
        };

        let mut no_summary = Event::new("Placeholder", date(2024, 1, 1));
        no_summary.summary = None;

        let mut recurring_timed = Event::new("Standup", floating(2024, 1, 1, 9, 0));
        recurring_timed.recurrence = Some(crate::event::Recurrence::new("FREQ=DAILY"));

        let mut recurring_all_day = Event::new("Annual Review", date(2024, 6, 30));
        recurring_all_day.recurrence = Some(crate::event::Recurrence::new("FREQ=YEARLY"));

        let mut recurring_untitled = Event::new("", date(2024, 6, 30));
        recurring_untitled.recurrence = Some(crate::event::Recurrence::new("FREQ=WEEKLY"));

        let corpus = vec![
            // all-day
            (
                Event::new("Test Event", date(2024, 1, 1)),
                "2024-01-01__test-event",
            ),
            // timed (floating: same wall clock in every zone)
            (
                Event::new("Test Event", floating(2024, 1, 1, 15, 30)),
                "2024-01-01T1530__test-event",
            ),
            // midnight timed is still a datetime, not a date
            (
                Event::new("Night Shift", floating(2024, 3, 4, 0, 0)),
                "2024-03-04T0000__night-shift",
            ),
            // recurring master (timed) — no date component at all
            (recurring_timed, "_recurring__standup"),
            // recurring master (all-day)
            (recurring_all_day, "_recurring__annual-review"),
            // recurring master with no usable summary
            (recurring_untitled, "_recurring__untitled"),
            // empty summary
            (Event::new("", date(2024, 1, 1)), "2024-01-01__untitled"),
            // absent summary
            (no_summary, "2024-01-01__untitled"),
            // emoji is stripped, not transliterated (no "coffee")
            (
                Event::new("Café ☕️ meeting", date(2024, 1, 1)),
                "2024-01-01__cafe-meeting",
            ),
            // a summary that is *only* non-alphanumeric falls back to `untitled`
            (
                Event::new("☕️ 🎉 !!!", date(2024, 1, 1)),
                "2024-01-01__untitled",
            ),
            // punctuation stripped before slugifying; runs collapse
            (
                Event::new("Q3 Planning: Budget & Roadmap!", date(2024, 1, 1)),
                "2024-01-01__q3-planning-budget-roadmap",
            ),
            // surrounding whitespace does not leak into the slug
            (
                Event::new("   Lunch   with   Bob   ", date(2024, 1, 1)),
                "2024-01-01__lunch-with-bob",
            ),
            // a summary that *looks* like the reserved prefixes cannot forge one:
            // the slugifier can never emit `__`, so this collides with nothing.
            (
                Event::new("recurring standup", date(2024, 1, 1)),
                "2024-01-01__recurring-standup",
            ),
            (
                Event::new("task recurring standup", date(2024, 1, 1)),
                "2024-01-01__task-recurring-standup",
            ),
            // >64-character summary: capped at 50 chars, trailing `-` preserved
            (
                Event::new(
                    "Quarterly all hands meeting about the roadmap and the hiring plans for next year",
                    date(2024, 1, 1),
                ),
                "2024-01-01__quarterly-all-hands-meeting-about-the-roadmap-and-",
            ),
        ];

        for (event, expected) in corpus {
            assert_eq!(
                event.base_slug(),
                expected,
                "base_slug drifted for summary {:?}",
                event.summary
            );
        }

        // Zone-dependent shapes. Expectation comes from chrono, not from
        // `EventTime::to_local_tz`, so the test still disagrees if the
        // conversion or the format string moves.
        let instant = Utc.with_ymd_and_hms(2024, 1, 1, 15, 30, 0).unwrap();
        let utc_event = Event::new("Test Event", EventTime::DateTimeUtc(instant));
        let expected_local = instant
            .with_timezone(&Local)
            .format("%Y-%m-%dT%H%M")
            .to_string();
        assert_eq!(
            utc_event.base_slug(),
            format!("{expected_local}__test-event")
        );

        let zoned = EventTime::DateTimeZoned {
            datetime: NaiveDate::from_ymd_opt(2024, 1, 1)
                .unwrap()
                .and_hms_opt(15, 30, 0)
                .unwrap(),
            tzid: "Europe/Stockholm".to_string(),
        };
        let expected_zoned = chrono_tz::Europe::Stockholm
            .with_ymd_and_hms(2024, 1, 1, 15, 30, 0)
            .unwrap()
            .with_timezone(&Local)
            .format("%Y-%m-%dT%H%M")
            .to_string();
        assert_eq!(
            Event::new("Test Event", zoned).base_slug(),
            format!("{expected_zoned}__test-event")
        );
    }

    #[test]
    fn generates_expected_slug_for_emoji_summary() {
        let event = Event::new(
            "Café ☕️ meeting",
            EventTime::Date(NaiveDate::from_ymd_opt(2024, 1, 1).unwrap()),
        );

        assert_eq!(event.summary_slug(), "cafe-meeting");
    }

    #[test]
    fn generates_expected_slug_for_empty_summary() {
        let event = Event::new(
            "",
            EventTime::Date(NaiveDate::from_ymd_opt(2024, 1, 1).unwrap()),
        );

        assert_eq!(event.summary_slug(), "untitled");
    }

    #[test]
    fn caps_long_summary_slug_to_avoid_filename_too_long() {
        // A pathologically long title must not produce a filename that exceeds
        // the filesystem's per-component limit (otherwise: ENAMETOOLONG on write).
        let event = Event::new(
            "a ".repeat(200),
            EventTime::Date(NaiveDate::from_ymd_opt(2024, 1, 1).unwrap()),
        );

        assert_eq!(event.summary_slug().chars().count(), 50);
    }

    #[test]
    fn generates_correct_base_slug_for_all_day_event() {
        let event = Event::new(
            "Test Event",
            EventTime::Date(NaiveDate::from_ymd_opt(2024, 1, 1).unwrap()),
        );

        assert_eq!(event.base_slug(), "2024-01-01__test-event");
    }

    #[test]
    fn generates_correct_base_slug_for_timed_event() {
        let event = Event::new(
            "Test Event",
            EventTime::DateTimeFloating(
                NaiveDate::from_ymd_opt(2024, 1, 1)
                    .unwrap()
                    .and_hms_opt(15, 30, 20)
                    .unwrap(),
            ),
        );

        assert_eq!(event.base_slug(), "2024-01-01T1530__test-event");
    }

    #[test]
    fn uses_recurring_prefix_instead_of_date_for_recurring_events() {
        let mut event = Event::new(
            "Standup",
            EventTime::DateTimeFloating(
                NaiveDate::from_ymd_opt(2024, 1, 1)
                    .unwrap()
                    .and_hms_opt(9, 0, 0)
                    .unwrap(),
            ),
        );
        event.recurrence = Some(crate::event::Recurrence::new("FREQ=DAILY"));

        assert_eq!(event.base_slug(), "_recurring__standup");
    }

    #[test]
    fn generates_untitled_base_slug_for_event_without_summary() {
        let event = Event::new(
            "",
            EventTime::Date(NaiveDate::from_ymd_opt(2024, 1, 1).unwrap()),
        );

        assert_eq!(event.base_slug(), "2024-01-01__untitled");
    }
}
