mod duplicate_file;

use anyhow::Result;
use caldir_core::{CalendarEvent, SkippedEvent};
use duplicate_file::duplicate_file_warnings;
use owo_colors::OwoColorize;
use std::io::Write;
use std::path::PathBuf;

#[derive(Debug)]
pub(crate) enum DoctorWarning {
    DuplicateFiles(Vec<PathBuf>),
    UnreadableEvents(String),
}

impl DoctorWarning {
    pub(crate) fn render(&self, out: &mut impl Write) -> Result<()> {
        match self {
            DoctorWarning::DuplicateFiles(paths) => {
                writeln!(
                    out,
                    "   {} same event saved as multiple files; delete all but one:",
                    "⚠".yellow()
                )?;
                for path in paths {
                    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("?");
                    writeln!(out, "       {}", name.dimmed())?;
                }
            }
            DoctorWarning::UnreadableEvents(error) => {
                writeln!(out, "   {}", error.red())?;
            }
        }

        Ok(())
    }
}

/// One warning per `.ics` file the listing had to skip.
///
/// Per-file degradation must not silently swallow malformed files: a file that
/// used to fail the whole calendar still reports the very same message (R77).
pub(crate) fn skipped_warnings(skipped: &[SkippedEvent]) -> Vec<DoctorWarning> {
    skipped
        .iter()
        .map(|skipped| DoctorWarning::UnreadableEvents(skipped.error().to_string()))
        .collect()
}

type EventCheck = fn(&[CalendarEvent]) -> Vec<DoctorWarning>;

const EVENT_CHECKS: &[EventCheck] = &[duplicate_file_warnings];

pub(crate) fn event_warnings(events: &[CalendarEvent]) -> Vec<DoctorWarning> {
    EVENT_CHECKS
        .iter()
        .flat_map(|check| check(events))
        .collect()
}
