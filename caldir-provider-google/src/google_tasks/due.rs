//! The one function deciding what a local `DUE` becomes on the way to
//! Google, and what a Google `due` becomes on the way back (§13). Both
//! directions funnel through here so the pair can't drift out of sync and
//! start oscillating (§7).
//!
//! `google_due_date` takes the target zone as a parameter rather than
//! reaching for `chrono::Local` itself, mirroring `EventTime::to_local_tz`'s
//! own signature (`caldir-core/src/event/time.rs`) — that keeps these tests
//! able to name a zone without mutating the process `TZ` (this crate has no
//! `serial_test` dev-dependency).

use caldir_core::EventTime;
use chrono::{NaiveDate, TimeZone};

/// A datetime `DUE`'s calendar date under `tz`, resolved through the zone
/// database. Delegates to `EventTime::to_local_tz`, which already implements
/// D-4's rule across all four `EventTime` variants: a `DateTimeZoned` value
/// resolves through its own `tzid` first, then converts into `tz`; every
/// other variant resolves directly in `tz`. **Do not** take a shortcut
/// through `EventTime::to_utc` — it is `to_local_tz(&chrono::Local)` and
/// answers a different question (an instant, not a calendar date in `tz`).
pub fn google_due_date<Tz: TimeZone>(due: &EventTime, tz: &Tz) -> NaiveDate {
    due.to_local_tz(tz).date_naive()
}

/// The inverse: a date-only `DUE`, never a midnight timestamp (§13 clause
/// 2). Google's `due` field carries a timestamp, but the API discards its
/// time-of-day — reading that discarded midnight back as a `DateTimeUtc`
/// is how the churn starts.
pub fn date_to_due(date: NaiveDate) -> EventTime {
    EventTime::Date(date)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono_tz::Tz;

    /// `caldir-core/src/test_utils.rs`'s `VTODO_OPEN_PRODUCTION_ICS`
    /// (`test_utils` is `#[cfg(test)]`-private and unexported — this crate
    /// cannot import it, so the literal is copied verbatim per the P3-05/
    /// P3-06 precedent).
    const PRODUCTION_DUE_035959: &str = "2026-09-02T03:59:59Z";
    /// `VTODO_COMPLETED_PRODUCTION_ICS`'s `DUE`.
    const PRODUCTION_DUE_COMPLETED_035959: &str = "2026-07-08T03:59:59Z";

    fn utc_due(rfc3339: &str) -> EventTime {
        EventTime::DateTimeUtc(rfc3339.parse().unwrap())
    }

    #[test]
    fn a_late_evening_local_deadline_keeps_its_local_calendar_date() {
        // 03:59:59Z is 23:59:59 the *previous* day in America/Santiago. The
        // wrong-but-natural implementation (`due.to_utc().date_naive()`, or
        // any path through `EventTime::to_utc`) yields 2026-09-02 instead —
        // moving the entire u_crawler deadline set one calendar day late.
        let due = utc_due(PRODUCTION_DUE_035959);
        let santiago: Tz = "America/Santiago".parse().unwrap();

        let date = google_due_date(&due, &santiago);

        assert_eq!(date, NaiveDate::from_ymd_opt(2026, 9, 1).unwrap());
    }

    #[test]
    fn a_second_production_deadline_also_keeps_its_local_calendar_date() {
        let due = utc_due(PRODUCTION_DUE_COMPLETED_035959);
        let santiago: Tz = "America/Santiago".parse().unwrap();

        let date = google_due_date(&due, &santiago);

        assert_eq!(date, NaiveDate::from_ymd_opt(2026, 7, 7).unwrap());
    }

    #[test]
    fn the_offset_comes_from_the_zone_database_not_a_constant() {
        // The archive holds both T035959Z and T025959Z deadlines because
        // Santiago changes UTC offset between them (DST). A hard-coded -4h
        // gets the first right and this one wrong — a bug that only shows
        // up in September.
        let due = utc_due("2026-09-09T02:59:59Z");
        let santiago: Tz = "America/Santiago".parse().unwrap();

        let date = google_due_date(&due, &santiago);

        assert_eq!(date, NaiveDate::from_ymd_opt(2026, 9, 8).unwrap());
    }

    #[test]
    fn no_5959z_suffix_rule_holds() {
        // The measured distribution has an outlier at T025900Z (not
        // T025959Z) — asserting a "*5959Z" pattern would be false for it.
        // This test proves the conversion works from the instant, not a
        // string-shape rule.
        let due = utc_due("2026-09-09T02:59:00Z");
        let santiago: Tz = "America/Santiago".parse().unwrap();

        let date = google_due_date(&due, &santiago);

        assert_eq!(date, NaiveDate::from_ymd_opt(2026, 9, 8).unwrap());
    }

    #[test]
    fn a_zoned_due_uses_its_own_tzid_before_the_system_zone() {
        // 00:30 Stockholm (UTC+1 in January) on 2026-01-15 is 2026-01-14
        // 23:30 UTC, which is 2026-01-14 18:30 in New York (UTC-5 in
        // January). A hand-rolled match that ignores `tzid` and resolves
        // the naive datetime directly in the passed zone instead yields
        // 2026-01-15 — the wrong date.
        let due = EventTime::DateTimeZoned {
            datetime: NaiveDate::from_ymd_opt(2026, 1, 15)
                .unwrap()
                .and_hms_opt(0, 30, 0)
                .unwrap(),
            tzid: "Europe/Stockholm".to_string(),
        };
        let new_york: Tz = "America/New_York".parse().unwrap();

        let date = google_due_date(&due, &new_york);

        assert_eq!(date, NaiveDate::from_ymd_opt(2026, 1, 14).unwrap());
    }

    #[test]
    fn the_canonical_due_is_a_fixpoint() {
        // Necessary, not sufficient: this proves the conversion agrees with
        // itself under repeated application (applying it to an
        // already-canonical value is a no-op, §7). It does NOT prove that
        // Google's own truncation agrees with this conversion — Google is
        // the party that actually discards the time-of-day, and only the
        // live test (`live::google_tasks`) exercises that.
        let zones: [Tz; 4] = [
            "UTC".parse().unwrap(),
            "Europe/Stockholm".parse().unwrap(),
            "America/New_York".parse().unwrap(),
            "America/Santiago".parse().unwrap(),
        ];
        let dues = [
            utc_due(PRODUCTION_DUE_035959),
            utc_due(PRODUCTION_DUE_COMPLETED_035959),
            utc_due("2026-09-09T02:59:59Z"),
            utc_due("2026-09-09T02:59:00Z"),
            EventTime::Date(NaiveDate::from_ymd_opt(2026, 9, 1).unwrap()),
        ];

        for tz in &zones {
            for due in &dues {
                let canonical1 = date_to_due(google_due_date(due, tz));
                let canonical2 = date_to_due(google_due_date(&canonical1, tz));
                assert_eq!(canonical1, canonical2, "not a fixpoint under {tz:?}");
            }
        }
    }

    #[test]
    fn an_undated_task_stays_undated() {
        // `Todo::occurs_in_range` never synthesizes a date for an undated
        // task; inventing "today" here would push a wrong deadline to the
        // user's phone. `due: None` must send no `due` field at all (not an
        // empty string), and a Google task with no `due` must parse back to
        // `due: None`.
        use crate::google_tasks::{Task, ToGoogle, task_to_calendar_item};
        use caldir_core::{CalendarItem, Todo};

        let todo = Todo::new("Buy milk");
        let google_task = todo.to_google(&chrono::Utc);
        assert!(google_task.due.is_none());
        let json = serde_json::to_value(&google_task).unwrap();
        assert!(json.get("due").is_none());

        let task = Task {
            id: "task1".into(),
            title: "Buy milk".into(),
            notes: String::new(),
            status: "needsAction".into(),
            due: None,
            completed: None,
            deleted: false,
            hidden: false,
        };
        let CalendarItem::Todo(todo) = task_to_calendar_item(task).unwrap().unwrap() else {
            panic!("expected a Todo");
        };
        assert_eq!(todo.due, None);
    }
}
