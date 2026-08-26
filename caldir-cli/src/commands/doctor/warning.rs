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
    /// A task duplicate is a distinct situation from an event duplicate, and
    /// says so: the event sentence is a pinned literal that must not drift
    /// into talking about tasks.
    DuplicateTaskFiles(Vec<PathBuf>),
    UnreadableEvents(String),
    /// A file that loaded, but holds components caldir does not model.
    ///
    /// Not an error: `main` ignored these silently, and failing the file would
    /// lose an event that read fine for years. But not silent either — caldir
    /// writes one component per file, so the next rewrite of this file drops
    /// them, and the user is the only one who can decide that is fine.
    IgnoredComponents {
        path: PathBuf,
        names: Vec<String>,
    },
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
                render_paths(out, paths)?;
            }
            DoctorWarning::DuplicateTaskFiles(paths) => {
                writeln!(
                    out,
                    "   {} same task saved as multiple files; delete all but one:",
                    "⚠".yellow()
                )?;
                render_paths(out, paths)?;
            }
            DoctorWarning::UnreadableEvents(error) => {
                writeln!(out, "   {}", error.red())?;
            }
            DoctorWarning::IgnoredComponents { path, names } => {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("?");
                writeln!(
                    out,
                    "   {} {} in {} is ignored; caldir will drop it if it rewrites the file",
                    "⚠".yellow(),
                    names.join(", "),
                    name,
                )?;
            }
        }

        Ok(())
    }
}

fn render_paths(out: &mut impl Write, paths: &[PathBuf]) -> Result<()> {
    for path in paths {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("?");
        writeln!(out, "       {}", name.dimmed())?;
    }
    Ok(())
}

/// One warning per `.ics` file the listing had to skip.
///
/// Per-file degradation must not silently swallow malformed files: a file that
/// used to fail the whole calendar still reports the very same message.
pub(crate) fn skipped_warnings(skipped: &[SkippedEvent]) -> Vec<DoctorWarning> {
    skipped
        .iter()
        .map(|skipped| DoctorWarning::UnreadableEvents(skipped.error().to_string()))
        .collect()
}

/// One warning per file that loaded with unsupported components beside its
/// item. The other half of sibling tolerance: the item is kept, and what was
/// ignored is still said out loud, on the surface built for saying it.
fn ignored_component_warnings(events: &[CalendarEvent]) -> Vec<DoctorWarning> {
    events
        .iter()
        .filter(|event| !event.ignored_components().is_empty())
        .map(|event| DoctorWarning::IgnoredComponents {
            path: event.path().to_path_buf(),
            names: event.ignored_components().to_vec(),
        })
        .collect()
}

type EventCheck = fn(&[CalendarEvent]) -> Vec<DoctorWarning>;

const EVENT_CHECKS: &[EventCheck] = &[duplicate_file_warnings, ignored_component_warnings];

pub(crate) fn event_warnings(events: &[CalendarEvent]) -> Vec<DoctorWarning> {
    EVENT_CHECKS
        .iter()
        .flat_map(|check| check(events))
        .collect()
}
