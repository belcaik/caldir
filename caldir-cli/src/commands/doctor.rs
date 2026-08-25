mod warning;

use crate::output::diff::Render;
use crate::utils::require_calendars;
use anyhow::Result;
use caldir_core::{Caldir, Calendar};
use owo_colors::OwoColorize;
use std::io::{self, Write};
use warning::{DoctorWarning, event_warnings, skipped_warnings};

/// Checks local caldir for bad calendar data:
pub fn run(caldir: &Caldir) -> Result<()> {
    require_calendars(caldir)?;

    let reports = calendar_reports(caldir);
    let mut out = io::stdout().lock();

    render(&mut out, caldir, &reports)
}

fn render(out: &mut impl Write, caldir: &Caldir, reports: &[CalendarReport]) -> Result<()> {
    let warning_count: usize = reports.iter().map(|report| report.warnings.len()).sum();

    for report in reports.iter().filter(|report| !report.warnings.is_empty()) {
        writeln!(out, "{}", report.calendar.render(caldir))?;
        for warning in &report.warnings {
            warning.render(out)?;
        }
        writeln!(out)?;
    }

    if warning_count == 0 {
        writeln!(out, "{} No problems found.", "✓".green())?;
    }

    Ok(())
}

#[derive(Debug)]
struct CalendarReport {
    calendar: Calendar,
    warnings: Vec<DoctorWarning>,
}

fn calendar_reports(caldir: &Caldir) -> Vec<CalendarReport> {
    caldir
        .calendars()
        .into_iter()
        .filter_map(Result::ok)
        .map(calendar_report)
        .collect()
}

fn calendar_report(calendar: Calendar) -> CalendarReport {
    let warnings = match calendar.events() {
        Ok(listing) => {
            let mut warnings = skipped_warnings(listing.skipped());
            warnings.extend(event_warnings(listing.events()));
            warnings
        }
        Err(err) => vec![DoctorWarning::UnreadableEvents(err.to_string())],
    };

    CalendarReport { calendar, warnings }
}

#[cfg(test)]
mod tests {
    use super::calendar_report;
    use super::warning::DoctorWarning;
    use caldir_core::Calendar;

    fn test_calendar() -> (tempfile::TempDir, Calendar) {
        let tmp = tempfile::tempdir().unwrap();
        let calendar = Calendar::create(&tmp.path().join("work"), None).unwrap();
        (tmp, calendar)
    }

    /// T-43 (R77): unlike `treats_unreadable_events_as_warnings`, whose document
    /// holds no component at all, this file *has* a VEVENT — the component itself
    /// is broken. Skipping the file must still be audible, never silent.
    #[test]
    fn a_malformed_vevent_is_still_a_warning_after_component_skipping() {
        let (_tmp, calendar) = test_calendar();
        std::fs::write(
            calendar.path().join("broken.ics"),
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:broken@caldir\r\nDTSTART:not-a-timestamp\r\nSUMMARY:Broken\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
        )
        .unwrap();

        let report = calendar_report(calendar);

        assert_eq!(report.warnings.len(), 1);
        assert!(matches!(
            report.warnings[0],
            DoctorWarning::UnreadableEvents(_)
        ));
    }

    #[test]
    fn treats_unreadable_events_as_warnings() {
        let (_tmp, calendar) = test_calendar();
        std::fs::write(
            calendar.path().join("bad.ics"),
            "BEGIN:VCALENDAR\nVERSION:2.0\nEND:VCALENDAR",
        )
        .unwrap();

        let report = calendar_report(calendar);

        assert_eq!(report.warnings.len(), 1);
        assert!(matches!(
            report.warnings[0],
            DoctorWarning::UnreadableEvents(_)
        ));
    }
}
