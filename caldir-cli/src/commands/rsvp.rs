use std::io::{self, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use caldir_core::{Caldir, CalendarEvent, DateBounds, Event, ParticipationStatus};
use chrono::{Duration, Utc};
use owo_colors::OwoColorize;

use crate::output::event::format_event_line;
use crate::output::time::format_date_only;
use crate::utils::require_calendars;

pub fn run(caldir: &Caldir, path: Option<String>, response: Option<String>) -> Result<()> {
    require_calendars(caldir)?;

    match (path, response) {
        (Some(path), Some(response)) => run_direct(caldir, &path, &response),
        (Some(path), None) => anyhow::bail!(
            "Missing response. Usage: caldir rsvp {} accept|decline|maybe",
            path
        ),
        _ => run_interactive(caldir),
    }
}

fn run_direct(caldir: &Caldir, path_str: &str, response_str: &str) -> Result<()> {
    let path = PathBuf::from(path_str);
    if !path.exists() {
        anyhow::bail!("File not found: {}", path.display());
    }

    let cal_slug = path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .context("Cannot determine calendar from path")?;

    let calendar = caldir
        .calendar(cal_slug)
        .with_context(|| format!("Failed to load calendar '{}'", cal_slug))?;

    let email = calendar
        .remote_email()
        .context("No account email configured for this calendar")?;

    rsvp_file(&path, email, response_str)
}

/// Answer the invite held by a single file. Split out of `run_direct` so the
/// narrowing, the write and the failure paths are reachable from a test
/// without a `Caldir` (which has no constructor outside `caldir-core`).
fn rsvp_file(path: &Path, email: &str, response_str: &str) -> Result<()> {
    let mut cal_event = CalendarEvent::load(path).context("Failed to load event")?;
    // An RSVP is meaningless on a task. Bail here, before `update`, so a
    // task file is never rewritten — and never silently no-ops: the user named
    // this file explicitly.
    let Some(event) = cal_event.event() else {
        anyhow::bail!(
            "{} is a task, not an event; tasks cannot be RSVP'd",
            path.display()
        );
    };

    if !event.is_invite_for(email) {
        anyhow::bail!("This event is not an invite for {}", email);
    }

    let status = parse_response(response_str)?;
    let summary = event.summary.clone().unwrap_or("(Untitled)".to_string());
    let updated = apply_response(event, email, status)?;
    cal_event.update(updated)?;

    println!("{} {} → {}", "✓".green(), summary, status);
    println!();
    println!("{}", "Remember to run: caldir push".dimmed());

    Ok(())
}

fn run_interactive(caldir: &Caldir) -> Result<()> {
    let tz: chrono_tz::Tz = iana_time_zone::get_timezone()?.parse()?;
    let today = Utc::now().with_timezone(&tz).date_naive();
    let from = today
        .start_of_date()
        .and_local_timezone(tz)
        .earliest()
        .unwrap()
        .with_timezone(&Utc);
    let to = (today + Duration::days(30))
        .end_of_date()
        .and_local_timezone(tz)
        .latest()
        .unwrap()
        .with_timezone(&Utc);

    // (cal_slug, email, Event, CalendarEvent) — own the CalendarEvent so we can
    // mutate, and carry the narrowed event so no later step has to re-narrow.
    let mut invites: Vec<(String, String, Event, CalendarEvent)> = Vec::new();

    for cal in caldir.calendars().into_iter().filter_map(Result::ok) {
        let Some(email) = cal.remote_email() else {
            continue;
        };
        let email = email.to_string();
        let cal_slug = cal.slug().unwrap_or("(Unknown calendar)").to_string();

        for ce in cal.events()?.into_events() {
            // Tasks have no invites to answer, so they are simply not listed
            // here.
            let Some(event) = ce.event() else {
                continue;
            };

            let in_range = event.occurs_in_range(from, to);
            let is_pending = event.is_invite_for(&email)
                && event.attendee_status(&email) == Some(ParticipationStatus::NeedsAction);
            if in_range && is_pending {
                let event = event.clone();
                invites.push((cal_slug.clone(), email.clone(), event, ce));
            }
        }
    }

    invites.sort_by_key(|(_, _, event, _)| event.start.to_utc());

    if invites.is_empty() {
        println!("{}", "No pending invites.".dimmed());
        return Ok(());
    }

    println!(
        "Found {} pending {}.\n",
        invites.len(),
        if invites.len() == 1 {
            "invite"
        } else {
            "invites"
        }
    );

    let mut responded = 0;
    let mut current_date: Option<String> = None;

    for (cal_slug, email, event, mut ce) in invites {
        let date_label = format_date_only(&event.start);
        if current_date.as_ref() != Some(&date_label) {
            if current_date.is_some() {
                println!();
            }
            println!("{}", date_label.bold());
            current_date = Some(date_label);
        }

        let organizer = event
            .organizer
            .as_ref()
            .map(|o| o.name.as_deref().unwrap_or(&o.email).to_string())
            .unwrap_or_else(|| "(unknown)".to_string());

        println!(
            "{}",
            format_event_line(&event, &cal_slug, "", caldir.config().time_format())
        );
        println!("       {} {}", "from:".dimmed(), organizer.dimmed());
        print!("  [a]ccept  [d]ecline  [m]aybe  [s]kip: ");
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let input = input.trim();

        let status = match input {
            "a" | "accept" => Some(ParticipationStatus::Accepted),
            "d" | "decline" => Some(ParticipationStatus::Declined),
            "m" | "maybe" => Some(ParticipationStatus::Tentative),
            "s" | "skip" | "" => None,
            _ => {
                println!("  {}", "Skipping (unrecognized input)".dimmed());
                None
            }
        };

        if let Some(status) = status {
            let updated = apply_response(&event, &email, status)?;
            ce.update(updated)?;
            println!("  {} → {}", "✓".green(), status);
            responded += 1;
        } else {
            println!("  {}", "skipped".dimmed());
        }
        println!();
    }

    if responded > 0 {
        println!(
            "Updated {} {}.",
            responded,
            if responded == 1 { "invite" } else { "invites" }
        );
        println!("{}", "Remember to run: caldir push".dimmed());
    } else {
        println!("{}", "No changes made.".dimmed());
    }

    Ok(())
}

fn parse_response(input: &str) -> Result<ParticipationStatus> {
    match input.to_lowercase().as_str() {
        "a" | "accept" | "accepted" | "yes" | "y" => Ok(ParticipationStatus::Accepted),
        "d" | "decline" | "declined" | "no" | "n" => Ok(ParticipationStatus::Declined),
        "m" | "maybe" | "tentative" => Ok(ParticipationStatus::Tentative),
        other => anyhow::bail!(
            "Unknown response '{}'. Use one of: accept, decline, maybe.",
            other
        ),
    }
}

fn apply_response(event: &Event, email: &str, status: ParticipationStatus) -> Result<Event> {
    let mut updated = event.clone();

    let attendee = updated
        .attendees
        .iter_mut()
        .find(|a| a.email.eq_ignore_ascii_case(email))
        .with_context(|| format!("Not an attendee: {}", email))?;

    attendee.status = Some(status);
    updated.last_modified = Some(Utc::now());

    Ok(updated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use caldir_core::{Attendee, EventTime};
    use chrono::NaiveDate;

    /// A VTODO carrying an ATTENDEE — permitted by C-2, but still not an
    /// invitation (R81).
    const TASK_WITH_ATTENDEE_ICS: &str = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//caldir//EN\r\n\
BEGIN:VTODO\r\n\
UID:assignment-123@u-crawler\r\n\
DTSTAMP:20260801T090000Z\r\n\
DUE:20260815T235900Z\r\n\
SUMMARY:Assignment 3\r\n\
ORGANIZER:mailto:alice@example.com\r\n\
ATTENDEE;PARTSTAT=NEEDS-ACTION:mailto:bob@example.com\r\n\
END:VTODO\r\n\
END:VCALENDAR\r\n";

    /// The VEVENT control: same shape, genuinely an invite.
    const EVENT_INVITE_ICS: &str = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//caldir//EN\r\n\
BEGIN:VEVENT\r\n\
UID:window-123@u-crawler\r\n\
DTSTAMP:20260801T090000Z\r\n\
DTSTART:20260815T100000Z\r\n\
DTEND:20260815T120000Z\r\n\
SUMMARY:Assignment 3 - submission window\r\n\
ORGANIZER:mailto:alice@example.com\r\n\
ATTENDEE;PARTSTAT=NEEDS-ACTION:mailto:bob@example.com\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    #[test]
    fn rsvp_on_a_task_exits_non_zero_and_leaves_the_file_unmodified() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("assignment-3.ics");
        std::fs::write(&path, TASK_WITH_ATTENDEE_ICS).unwrap();
        let bytes_before = std::fs::read(&path).unwrap();

        let err = rsvp_file(&path, "bob@example.com", "accept")
            .expect_err("an RSVP on a task must fail, not silently no-op");

        assert_eq!(
            err.to_string(),
            format!(
                "{} is a task, not an event; tasks cannot be RSVP'd",
                path.display()
            )
        );
        // The load-bearing assertion: the bail must happen before the write.
        let bytes_after = std::fs::read(&path).expect("the task file must still be there");
        assert_eq!(bytes_after, bytes_before);
        // No renamed copy and no temp file left behind either.
        assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 1);
    }

    #[test]
    fn rsvp_on_an_event_invite_still_records_the_response() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("submission-window.ics");
        std::fs::write(&path, EVENT_INVITE_ICS).unwrap();

        rsvp_file(&path, "bob@example.com", "accept").unwrap();

        let written: String = std::fs::read_dir(tmp.path())
            .unwrap()
            .map(|entry| std::fs::read_to_string(entry.unwrap().path()).unwrap())
            .collect();
        assert!(
            written.contains("PARTSTAT=ACCEPTED"),
            "event RSVP must still be written: {written}"
        );
    }

    #[test]
    fn apply_response_preserves_sequence() {
        let mut event = Event::new(
            "Meeting",
            EventTime::Date(NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()),
        );
        event.sequence = 4;
        event.attendees.push(Attendee::new("user@example.com"));

        let updated =
            apply_response(&event, "user@example.com", ParticipationStatus::Accepted).unwrap();

        assert_eq!(updated.sequence, event.sequence);
        assert_eq!(
            updated.attendees[0].status,
            Some(ParticipationStatus::Accepted)
        );
        assert!(updated.last_modified.is_some());
    }
}
