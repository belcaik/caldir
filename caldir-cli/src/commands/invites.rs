use anyhow::Result;
use caldir_core::{Caldir, Calendar, CalendarItem, DateBounds, Event, ParticipationStatus};
use chrono::{DateTime, Duration, Utc};
use owo_colors::OwoColorize;

use crate::output::event::{format_event_line, is_visible, render_participation_status};
use crate::output::time::format_date_only;
use crate::utils::{require_calendars, resolve_calendars};

pub fn run(caldir: &Caldir, calendar: Option<String>, all: bool) -> Result<()> {
    require_calendars(caldir)?;
    let calendars = resolve_calendars(caldir, calendar.as_deref())?;

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

    let mut invites = collect_invites(&calendars, from, to, all)?;

    invites.sort_by_key(|(_, event, _)| event.start.to_utc());

    if invites.is_empty() {
        println!("{}", "No pending invites.".dimmed());
        return Ok(());
    }

    let mut current_date: Option<String> = None;

    for (cal_slug, event, email) in &invites {
        let date_label = format_date_only(&event.start);
        if current_date.as_ref() != Some(&date_label) {
            if current_date.is_some() {
                println!();
            }
            println!("{}", date_label.bold());
            current_date = Some(date_label);
        }

        let status_suffix = event
            .attendee_status(email)
            .map(|s| format!(" ({})", render_participation_status(s)))
            .unwrap_or_default();
        println!(
            "{}",
            format_event_line(
                event,
                cal_slug,
                &status_suffix,
                caldir.config().time_format(),
            )
        );

        if let Some(organizer) = event.organizer.as_ref().filter(|o| !o.email.is_empty()) {
            println!("       {} {}", "from:".dimmed(), organizer.email.dimmed());
        }
    }

    println!();
    println!("Respond with: caldir rsvp");

    Ok(())
}

/// The invites to display, in discovery order.
///
/// A calendar with no remote account has no invites by construction: there is
/// nobody for an invitation to be addressed to.
fn collect_invites(
    calendars: &[Calendar],
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    all: bool,
) -> Result<Vec<(String, Event, String)>> {
    let mut invites: Vec<(String, Event, String)> = Vec::new();

    for cal in calendars {
        let Some(email) = cal.remote_email() else {
            continue;
        };

        let items = cal.expanded_events_in_range(from, to)?;
        let cal_slug = cal.slug().unwrap_or("(Unknown calendar)").to_string();

        for item in items {
            // An invitation is meaningless on a task (R81): a VTODO carries no
            // invite to answer, whether or not it holds an ATTENDEE — the
            // property is in the MVP set and round-trips. So this narrowing is
            // permanent, not deferred. Display-only: the task stays in the
            // calendar listing, and no deletion is inferred from its absence
            // here (R19).
            let CalendarItem::Event(event) = item else {
                continue;
            };

            if !is_visible(&event) {
                continue;
            }
            let is_invite = event.is_invite_for(email);
            let matches = if all {
                is_invite
            } else {
                is_invite && event.attendee_status(email) == Some(ParticipationStatus::NeedsAction)
            };
            if matches {
                invites.push((cal_slug.clone(), event, email.to_string()));
            }
        }
    }

    Ok(invites)
}

#[cfg(test)]
mod tests {
    use super::collect_invites;
    use caldir_core::Calendar;
    use chrono::{DateTime, Utc};
    use indoc::indoc;

    /// A VTODO that carries an `ATTENDEE` awaiting a response. `ATTENDEE` is in
    /// the MVP property set (C-2) and round-trips, so this file is well-formed:
    /// the reason it never reaches the invite list is that a task carries no
    /// invitation to answer (R81), not that it cannot hold attendees.
    fn task_with_attendee_ics() -> String {
        indoc! {"
            BEGIN:VCALENDAR
            VERSION:2.0
            PRODID:-//caldir//EN
            BEGIN:VTODO
            UID:assignment-123@u-crawler
            DTSTAMP:20260601T090000Z
            DUE:20260615T235900Z
            SUMMARY:Assignment 3 - Recursive descent parser
            ORGANIZER:mailto:alice@example.com
            ATTENDEE;PARTSTAT=NEEDS-ACTION:mailto:bob@example.com
            END:VTODO
            END:VCALENDAR
        "}
        .replace('\n', "\r\n")
    }

    /// The control: a genuine event invite for the same address, awaiting the
    /// same response.
    fn event_invite_ics() -> String {
        indoc! {"
            BEGIN:VCALENDAR
            VERSION:2.0
            PRODID:-//caldir//EN
            BEGIN:VEVENT
            UID:standup-123@u-crawler
            DTSTAMP:20260601T090000Z
            DTSTART:20260610T100000Z
            DTEND:20260610T110000Z
            SUMMARY:Sprint standup
            ORGANIZER:mailto:alice@example.com
            ATTENDEE;PARTSTAT=NEEDS-ACTION:mailto:bob@example.com
            END:VEVENT
            END:VCALENDAR
        "}
        .replace('\n', "\r\n")
    }

    /// An event `bob@example.com` organizes: on his own calendar this is not an
    /// invite for him, in `--all` mode as much as in the default one.
    fn own_event_ics() -> String {
        indoc! {"
            BEGIN:VCALENDAR
            VERSION:2.0
            PRODID:-//caldir//EN
            BEGIN:VEVENT
            UID:own-123@u-crawler
            DTSTAMP:20260601T090000Z
            DTSTART:20260611T100000Z
            DTEND:20260611T110000Z
            SUMMARY:Focus block
            ORGANIZER:mailto:bob@example.com
            ATTENDEE;PARTSTAT=NEEDS-ACTION:mailto:bob@example.com
            END:VEVENT
            END:VCALENDAR
        "}
        .replace('\n', "\r\n")
    }

    /// A calendar whose remote account is `bob@example.com`, holding `files`.
    fn calendar_with(files: &[(&str, String)]) -> (tempfile::TempDir, Calendar) {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("work");
        std::fs::create_dir_all(path.join(".caldir")).unwrap();
        std::fs::write(
            path.join(".caldir").join("config.toml"),
            indoc! {r#"
                [remote]
                provider = "hooli"
                hooli_account = "bob@example.com"
            "#},
        )
        .unwrap();

        for (name, contents) in files {
            std::fs::write(path.join(name), contents).unwrap();
        }

        let calendar = Calendar::load(&path).unwrap();
        (tmp, calendar)
    }

    /// A window wide enough to hold both fixtures, fixed so the assertions do
    /// not move with the clock or the machine's zone.
    fn window() -> (DateTime<Utc>, DateTime<Utc>) {
        (
            "2026-01-01T00:00:00Z".parse().unwrap(),
            "2027-01-01T00:00:00Z".parse().unwrap(),
        )
    }

    #[test]
    fn a_task_never_enters_the_invite_list() {
        let (_tmp, calendar) = calendar_with(&[
            ("deadline.ics", task_with_attendee_ics()),
            ("standup.ics", event_invite_ics()),
        ]);
        let (from, to) = window();

        let invites = collect_invites(&[calendar], from, to, false).unwrap();

        assert_eq!(invites.len(), 1);
        let (cal_slug, event, email) = &invites[0];
        assert_eq!(cal_slug, "work");
        assert_eq!(email, "bob@example.com");
        assert_eq!(event.uid.as_str(), "standup-123@u-crawler");
        assert_eq!(event.summary.as_deref(), Some("Sprint standup"));
    }

    /// `--all` drops the "awaiting a response" filter, so if the task were
    /// reaching the list at all this is where it would show. It still does not:
    /// the narrowing is on the item's kind, not on its participation status.
    #[test]
    fn a_task_never_enters_the_invite_list_under_all() {
        let (_tmp, calendar) = calendar_with(&[
            ("deadline.ics", task_with_attendee_ics()),
            ("standup.ics", event_invite_ics()),
            ("focus.ics", own_event_ics()),
        ]);
        let (from, to) = window();

        let invites = collect_invites(&[calendar], from, to, true).unwrap();

        let uids: Vec<&str> = invites.iter().map(|(_, e, _)| e.uid.as_str()).collect();
        assert_eq!(uids, vec!["standup-123@u-crawler"]);
    }

    /// R19: the skip is display-only. The task is absent from the *invite list*
    /// and from nothing else — the calendar still lists it, which is what every
    /// delete path reads.
    #[test]
    fn a_skipped_task_is_still_present_in_the_calendar_listing() {
        let (_tmp, calendar) = calendar_with(&[
            ("deadline.ics", task_with_attendee_ics()),
            ("standup.ics", event_invite_ics()),
        ]);
        let (from, to) = window();

        let mut uids: Vec<String> = calendar
            .expanded_events_in_range(from, to)
            .unwrap()
            .iter()
            .map(|item| item.uid().as_str().to_string())
            .collect();
        uids.sort();

        assert_eq!(
            uids,
            vec!["assignment-123@u-crawler", "standup-123@u-crawler"]
        );
    }

    /// A calendar with no remote account has nobody for an invitation to be
    /// addressed to, so it contributes nothing — the same event file that is an
    /// invite on a connected calendar is silent on a local-only one.
    #[test]
    fn a_calendar_without_a_remote_account_contributes_no_invites() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("personal");
        std::fs::create_dir_all(path.join(".caldir")).unwrap();
        std::fs::write(path.join("standup.ics"), event_invite_ics()).unwrap();
        let calendar = Calendar::load(&path).unwrap();
        let (from, to) = window();

        let invites = collect_invites(&[calendar], from, to, true).unwrap();

        assert!(invites.is_empty());
    }
}
