use anyhow::Result as AnyhowResult;
use caldir_core::{Caldir, Calendar, CalendarItem, EventTime, ParticipationStatus, TimeFormat};
use chrono::{DateTime, NaiveDate, Utc};
use std::cmp::Ordering;

use crate::output::event::is_item_visible;
use crate::output::time::local_date;

pub struct AgendaView {
    pub entries: Vec<AgendaEntry>,
    pub(crate) range_start: NaiveDate,
    pub(crate) range_end: NaiveDate,
    pub(crate) time_format: TimeFormat,
    /// `--undated`: list the undated tasks in a trailing `No date` section
    /// instead of counting them in the footer (ADR-10 D3). A **text-view**
    /// concern only — `--json` always emits whatever the schema covers.
    pub(crate) undated: bool,
}

pub struct AgendaEntry {
    pub item: CalendarItem,
    pub calendar: Option<String>,
    pub rsvp: Option<ParticipationStatus>,
}

impl AgendaView {
    pub fn collect(
        caldir: &Caldir,
        calendars: Vec<Calendar>,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        undated: bool,
    ) -> AnyhowResult<Self> {
        Ok(Self::assemble(
            collect_entries(calendars, from, to),
            from,
            to,
            caldir.config().time_format(),
            undated,
        ))
    }

    /// Everything `collect` does that does not need a `Caldir`: the local
    /// window, the total order, and the view's own flags. Split out so the
    /// assembly is reachable from a test — `Caldir`'s test constructor is
    /// crate-private to `caldir-core`, so `collect` itself is not.
    fn assemble(
        mut entries: Vec<AgendaEntry>,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        time_format: TimeFormat,
        undated: bool,
    ) -> Self {
        sort_entries(&mut entries);

        Self {
            time_format,
            entries,
            range_start: from.with_timezone(&chrono::Local).date_naive(),
            range_end: to.with_timezone(&chrono::Local).date_naive(),
            undated,
        }
    }
}

/// Where an item sits in time: `DTSTART` for an event, `DUE` falling back to
/// `DTSTART` for a task (ADR-10 D1). `None` means *undated* — a task with
/// neither, which belongs to no day and is never given an invented date.
///
/// The single place the CLI answers "where does this item sit in time". For an
/// `Event` it is the event's own `start`, borrowed, so every event keeps
/// exactly the value it sorted and rendered on before tasks existed (C13).
pub(crate) fn anchor(item: &CalendarItem) -> Option<&EventTime> {
    match item {
        CalendarItem::Event(event) => Some(&event.start),
        CalendarItem::Todo(todo) => todo.due.as_ref().or(todo.start.as_ref()),
        // Undated is a real, kept state (ADR-10 D3) — an unknown kind is
        // listed in the undated tail rather than given an invented date.
        _ => None,
    }
}

/// The kind tiebreak: at an identical instant, an event comes before a task
/// (ADR-10 D2). It can never reorder two events, so it cannot perturb existing
/// output.
fn kind_ordinal(item: &CalendarItem) -> u8 {
    match item {
        CalendarItem::Event(_) => 0,
        CalendarItem::Todo(_) => 1,
        // Later kinds tie-break after both, so neither existing order moves.
        _ => 2,
    }
}

/// The agenda's total order: `local_date(anchor)` → all-day first →
/// `anchor.to_utc()` → kind ordinal.
///
/// Undated items sort *last* and are kept: `Ord for Option` would put `None`
/// first, which would open the agenda with the tasks that have no day at all.
///
/// `sort_by` is stable and that is load-bearing, not incidental: entries with
/// equal keys keep `read_dir` order, which is exactly as (non)deterministic as
/// it is today. No identity tiebreak is added — one would improve event-vs-event
/// ordering but change existing text for existing situations (R85, C13).
fn sort_entries(entries: &mut [AgendaEntry]) {
    entries.sort_by(|a, b| match (anchor(&a.item), anchor(&b.item)) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(a_time), Some(b_time)) => local_date(a_time)
            .cmp(&local_date(b_time))
            .then_with(|| a_time.is_date().cmp(&b_time.is_date()).reverse())
            .then_with(|| a_time.to_utc().cmp(&b_time.to_utc()))
            .then_with(|| kind_ordinal(&a.item).cmp(&kind_ordinal(&b.item))),
    });
}

/// Gather the visible entries of every calendar that can be listed.
///
/// A calendar that fails to list contributes nothing and is otherwise ignored:
/// one unreadable calendar must never blank out the others' events. Entries stay
/// in calendar order so the caller's sort sees the same sequence as before.
fn collect_entries(
    calendars: Vec<Calendar>,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> Vec<AgendaEntry> {
    let mut entries = Vec::new();

    for calendar in calendars {
        let calendar_slug = calendar.slug().map(str::to_owned);
        let remote_email = calendar.remote_email().map(str::to_owned);

        let Ok(items) = calendar.expanded_events_in_range(from, to) else {
            continue;
        };

        for item in items {
            if !is_item_visible(&item) {
                continue;
            }

            let rsvp = resolve_rsvp(&item, remote_email.as_deref());

            entries.push(AgendaEntry {
                calendar: calendar_slug.clone(),
                rsvp,
                item,
            });
        }
    }

    entries
}

/// A task has no invite semantics, so it never carries an RSVP (R81, ADR-10
/// D7): narrow to an event rather than teaching this about tasks.
fn resolve_rsvp(item: &CalendarItem, remote_email: Option<&str>) -> Option<ParticipationStatus> {
    let CalendarItem::Event(event) = item else {
        return None;
    };

    remote_email
        .filter(|email| event.is_invite_for(email))
        .and_then(|email| event.attendee_status(email))
}

#[cfg(test)]
mod tests {
    use super::*;
    use caldir_core::{Attendee, Event, EventTime, Organizer, Todo};
    use chrono::TimeZone;

    fn calendar(root: &std::path::Path, slug: &str) -> Calendar {
        Calendar::create(&root.join(slug), None).unwrap()
    }

    #[test]
    fn one_broken_calendar_does_not_blank_the_agenda() {
        let tmp = tempfile::tempdir().unwrap();

        let healthy = calendar(tmp.path(), "healthy");
        let start = EventTime::Date(NaiveDate::from_ymd_opt(2026, 8, 14).unwrap());
        healthy.create_event(Event::new("Healthy", start)).unwrap();

        // A calendar whose directory is gone: listing it fails outright.
        let broken = calendar(tmp.path(), "broken");
        std::fs::remove_dir_all(broken.path()).unwrap();

        let from = Utc.with_ymd_and_hms(2026, 8, 1, 0, 0, 0).unwrap();
        let to = Utc.with_ymd_and_hms(2026, 9, 1, 0, 0, 0).unwrap();
        let entries = collect_entries(vec![broken, healthy], from, to);

        let summaries: Vec<_> = entries
            .iter()
            .filter_map(|entry| entry.item.summary())
            .collect();
        assert_eq!(summaries, vec!["Healthy"]);
    }

    /// Level 3, at the CLI seam: a task sitting in the same calendar must not
    /// cost the agenda its events. Since L7-01 the task has an anchor and joins
    /// the entries beside them; what must never happen, then or now, is the
    /// events going missing.
    #[test]
    fn a_task_in_the_calendar_does_not_blank_the_agenda() {
        const VTODO_ICS: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VTODO\r\nUID:assignment-123@u-crawler\r\nDUE:20260815T235900Z\r\n\
SUMMARY:Assignment 3\r\nEND:VTODO\r\nEND:VCALENDAR\r\n";

        let tmp = tempfile::tempdir().unwrap();
        let cal = calendar(tmp.path(), "deadlines");
        let start = EventTime::Date(NaiveDate::from_ymd_opt(2026, 8, 14).unwrap());
        cal.create_event(Event::new("Healthy", start)).unwrap();
        std::fs::write(cal.path().join("assignment-123.ics"), VTODO_ICS).unwrap();

        let from = Utc.with_ymd_and_hms(2026, 8, 1, 0, 0, 0).unwrap();
        let to = Utc.with_ymd_and_hms(2026, 9, 1, 0, 0, 0).unwrap();
        let entries = collect_entries(vec![cal], from, to);

        let summaries: Vec<_> = entries
            .iter()
            .filter_map(|entry| entry.item.summary())
            .collect();
        assert_eq!(summaries, vec!["Healthy", "Assignment 3"]);
    }

    #[test]
    fn resolves_rsvp_only_for_invites_to_calendar_account() {
        let start = EventTime::Date(NaiveDate::from_ymd_opt(2026, 8, 14).unwrap());
        let mut event = Event::new("Invite", start);
        event.organizer = Some(Organizer::new("host@example.com"));
        let mut attendee = Attendee::new("me@example.com");
        attendee.status = Some(ParticipationStatus::Accepted);
        event.attendees.push(attendee);

        let item = CalendarItem::Event(event);
        assert_eq!(
            resolve_rsvp(&item, Some("me@example.com")),
            Some(ParticipationStatus::Accepted)
        );
        assert_eq!(resolve_rsvp(&item, Some("other@example.com")), None);
        assert_eq!(resolve_rsvp(&item, None), None);
    }

    /// R81 / ADR-10 D7: a task may legally carry ATTENDEE, but it has no invite
    /// semantics, so it never gets an RSVP indicator.
    #[test]
    fn a_task_never_carries_an_rsvp() {
        let mut todo = Todo::new("Ship crawler patch");
        let mut attendee = Attendee::new("me@example.com");
        attendee.status = Some(ParticipationStatus::Accepted);
        todo.organizer = Some(Organizer::new("host@example.com"));
        todo.attendees.push(attendee);

        assert_eq!(
            resolve_rsvp(&CalendarItem::Todo(todo), Some("me@example.com")),
            None
        );
    }

    fn entry(event: Event) -> AgendaEntry {
        AgendaEntry {
            item: CalendarItem::Event(event),
            calendar: Some("work".to_string()),
            rsvp: None,
        }
    }

    fn todo_entry(todo: Todo) -> AgendaEntry {
        AgendaEntry {
            item: CalendarItem::Todo(todo),
            calendar: Some("work".to_string()),
            rsvp: None,
        }
    }

    /// `sort_entries` over borrowed entries, so a fixture can be sorted from
    /// more than one starting order without cloning it.
    fn sort_refs(entries: &mut [&AgendaEntry]) {
        let mut owned: Vec<AgendaEntry> = entries
            .iter()
            .map(|entry| AgendaEntry {
                item: entry.item.clone(),
                calendar: entry.calendar.clone(),
                rsvp: entry.rsvp,
            })
            .collect();
        sort_entries(&mut owned);
        let order: Vec<CalendarItem> = owned.into_iter().map(|entry| entry.item).collect();
        entries.sort_by_key(|entry| order.iter().position(|item| item == &entry.item).unwrap());
    }

    fn summary(entry: &AgendaEntry) -> &str {
        entry.item.summary().unwrap_or("(Untitled)")
    }

    /// T-46 (R84, ADR-10 D2). One local day carrying an all-day event, a
    /// DATE-valued `DUE` task, an 09:00 event and an 09:00 task must come out
    /// in exactly that order: all four sort rules at once, including rule 4
    /// putting the event ahead of the task at an identical instant.
    ///
    /// TZ-sensitive: rule 1 is `local_date`, so the 09:00Z pair has to land on
    /// the same local day in every CI zone. 09:00Z is 05:00 in New York and
    /// 11:00 in Stockholm, so it does.
    #[test]
    fn event_task_and_all_day_on_one_day_order_correctly() {
        const DATE_DUE_TASK_ICS: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VTODO\r\nUID:patch@u-crawler\r\nDUE;VALUE=DATE:20260814\r\n\
SUMMARY:Ship crawler patch\r\nEND:VTODO\r\nEND:VCALENDAR\r\n";
        const TIMED_TASK_ICS: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VTODO\r\nUID:report@u-crawler\r\nDUE:20260814T090000Z\r\n\
SUMMARY:Draft Q3 report\r\nEND:VTODO\r\nEND:VCALENDAR\r\n";

        let tmp = tempfile::tempdir().unwrap();
        let cal = calendar(tmp.path(), "work");
        cal.create_event(Event::new(
            "Sprint demo",
            EventTime::Date(NaiveDate::from_ymd_opt(2026, 8, 14).unwrap()),
        ))
        .unwrap();
        cal.create_event(Event::new(
            "Standup",
            EventTime::DateTimeUtc(Utc.with_ymd_and_hms(2026, 8, 14, 9, 0, 0).unwrap()),
        ))
        .unwrap();
        std::fs::write(cal.path().join("patch.ics"), DATE_DUE_TASK_ICS).unwrap();
        std::fs::write(cal.path().join("report.ics"), TIMED_TASK_ICS).unwrap();

        let from = Utc.with_ymd_and_hms(2026, 8, 1, 0, 0, 0).unwrap();
        let to = Utc.with_ymd_and_hms(2026, 9, 1, 0, 0, 0).unwrap();
        let expected = vec![
            "Sprint demo",
            "Ship crawler patch",
            "Standup",
            "Draft Q3 report",
        ];

        // Sorted from the `read_dir` order *and* from its reverse. Rules 2-4
        // separate every pair here, so the result must not depend on which
        // order the files came off the filesystem in — without this the
        // event-before-task rule can pass by accident.
        let collected = collect_entries(vec![cal], from, to);
        for mut entries in [
            collected.iter().collect::<Vec<_>>(),
            collected.iter().rev().collect::<Vec<_>>(),
        ] {
            sort_refs(&mut entries);
            assert_eq!(
                entries
                    .iter()
                    .map(|entry| summary(entry))
                    .collect::<Vec<_>>(),
                expected
            );
        }
    }

    /// The other half of R85: `sort_by`'s *stability* is load-bearing, not
    /// incidental — entries with equal keys must come out in the order they
    /// went in. A tie-only fixture cannot prove this (`sort_unstable_by`
    /// happens to preserve an all-equal slice at every size), so this one
    /// interleaves three distinct instants, which is what actually makes an
    /// unstable sort shuffle.
    #[test]
    fn ties_keep_their_input_order_even_when_other_keys_differ() {
        let names: Vec<String> = (0..45).map(|n| format!("event-{n:02}")).collect();
        let mut entries: Vec<AgendaEntry> = names
            .iter()
            .enumerate()
            .map(|(n, name)| {
                let hour = 9 + (n % 3) as u32;
                entry(Event::new(
                    name.clone(),
                    EventTime::DateTimeUtc(Utc.with_ymd_and_hms(2026, 8, 14, hour, 0, 0).unwrap()),
                ))
            })
            .collect();

        sort_entries(&mut entries);

        // The three 09:00/10:00/11:00 groups in that order, and inside each
        // group the fixture's own ascending order, untouched.
        let expected: Vec<&str> = (0..3)
            .flat_map(|group| {
                names
                    .iter()
                    .enumerate()
                    .filter(move |(n, _)| n % 3 == group)
                    .map(|(_, name)| name.as_str())
            })
            .collect();

        assert_eq!(entries.iter().map(summary).collect::<Vec<_>>(), expected);
    }

    /// ADR-10 D2 rule 4, isolated: at an identical instant the event wins,
    /// whichever order the two arrived in.
    #[test]
    fn an_event_sorts_before_a_task_at_the_same_instant() {
        let instant = Utc.with_ymd_and_hms(2026, 8, 14, 9, 0, 0).unwrap();
        let mut todo = Todo::new("Draft Q3 report");
        todo.due = Some(EventTime::DateTimeUtc(instant));

        // Task first: only rule 4 can move it back.
        let mut entries = vec![
            todo_entry(todo),
            entry(Event::new("Standup", EventTime::DateTimeUtc(instant))),
        ];

        sort_entries(&mut entries);

        assert_eq!(
            entries.iter().map(summary).collect::<Vec<_>>(),
            vec!["Standup", "Draft Q3 report"]
        );
    }

    /// Rule 1 is the local *date*, and it has to come first: rule 2 puts
    /// all-day items ahead of timed ones, so without the date rule a task due
    /// on the 20th would open the agenda above Monday's 09:00 standup.
    #[test]
    fn a_later_all_day_item_never_outranks_an_earlier_timed_one() {
        let mut todo = Todo::new("Ship crawler patch");
        todo.due = Some(EventTime::Date(
            NaiveDate::from_ymd_opt(2026, 8, 20).unwrap(),
        ));

        let mut entries = vec![
            todo_entry(todo),
            entry(Event::new(
                "Standup",
                EventTime::DateTimeUtc(Utc.with_ymd_and_hms(2026, 8, 14, 9, 0, 0).unwrap()),
            )),
        ];

        sort_entries(&mut entries);

        assert_eq!(
            entries.iter().map(summary).collect::<Vec<_>>(),
            vec!["Standup", "Ship crawler patch"]
        );
    }

    /// A task carrying only `DTSTART` is anchored on it — `DUE` wins when both
    /// are present, but it is a fallback, not the only source.
    #[test]
    fn a_task_with_only_a_dtstart_is_anchored_on_it() {
        let mut todo = Todo::new("Ship crawler patch");
        todo.start = Some(EventTime::DateTimeUtc(
            Utc.with_ymd_and_hms(2026, 8, 14, 8, 0, 0).unwrap(),
        ));

        assert_eq!(
            anchor(&CalendarItem::Todo(todo.clone())),
            todo.start.as_ref()
        );

        let mut entries = vec![
            entry(Event::new(
                "Standup",
                EventTime::DateTimeUtc(Utc.with_ymd_and_hms(2026, 8, 14, 9, 0, 0).unwrap()),
            )),
            todo_entry(todo),
        ];
        sort_entries(&mut entries);

        assert_eq!(
            entries.iter().map(summary).collect::<Vec<_>>(),
            vec!["Ship crawler patch", "Standup"]
        );
    }

    /// R85: event-vs-event ties keep `read_dir` order. Nothing here may become
    /// more deterministic than it is today — an `instance_id` (or uid, or
    /// filename) tiebreak would change existing text for existing situations
    /// and breach C13. This is the guard for that.
    #[test]
    fn two_events_at_the_same_instant_keep_read_dir_order() {
        let start = EventTime::DateTimeUtc(Utc.with_ymd_and_hms(2026, 8, 14, 9, 0, 0).unwrap());
        // Descending names, so *any* identity tiebreak (uid, summary,
        // instance_id, filename) would reorder them. Enough of them that an
        // unstable sort has something to shuffle: `sort_unstable_by` falls
        // back to a stable insertion sort on very short slices, so a
        // two-element fixture cannot tell the two apart.
        let names: Vec<String> = (0..64).rev().map(|n| format!("event-{n:02}")).collect();
        let mut entries: Vec<AgendaEntry> = names
            .iter()
            .map(|name| entry(Event::new(name.clone(), start.clone())))
            .collect();

        sort_entries(&mut entries);

        assert_eq!(
            entries.iter().map(summary).collect::<Vec<_>>(),
            names.iter().map(String::as_str).collect::<Vec<_>>()
        );
    }

    /// ADR-10 D1/D3: `DUE` anchors a task, and it wins over the task's own
    /// `DTSTART`. Anchored on `DTSTART` the task would sort before the event.
    #[test]
    fn due_anchors_a_task_ahead_of_its_own_dtstart() {
        let mut todo = Todo::new("Ship crawler patch");
        todo.start = Some(EventTime::DateTimeUtc(
            Utc.with_ymd_and_hms(2026, 8, 14, 8, 0, 0).unwrap(),
        ));
        todo.due = Some(EventTime::DateTimeUtc(
            Utc.with_ymd_and_hms(2026, 8, 14, 10, 0, 0).unwrap(),
        ));

        assert_eq!(anchor(&CalendarItem::Todo(todo.clone())), todo.due.as_ref());

        let mut entries = vec![
            todo_entry(todo),
            entry(Event::new(
                "Standup",
                EventTime::DateTimeUtc(Utc.with_ymd_and_hms(2026, 8, 14, 9, 0, 0).unwrap()),
            )),
        ];
        sort_entries(&mut entries);

        assert_eq!(
            entries.iter().map(summary).collect::<Vec<_>>(),
            vec!["Standup", "Ship crawler patch"]
        );
    }

    /// AC 6: an undated task has no anchor, so it is never placed in a day
    /// group and never given an invented date — but it is *kept*. Rust's
    /// `Ord for Option` puts `None` first; these must sort last.
    #[test]
    fn undated_tasks_are_kept_and_sorted_last() {
        let mut entries = vec![
            todo_entry(Todo::new("Someday: rewrite installer")),
            entry(Event::new(
                "Standup",
                EventTime::DateTimeUtc(Utc.with_ymd_and_hms(2026, 8, 14, 9, 0, 0).unwrap()),
            )),
        ];

        sort_entries(&mut entries);

        assert_eq!(
            entries.iter().map(summary).collect::<Vec<_>>(),
            vec!["Standup", "Someday: rewrite installer"]
        );
        assert_eq!(anchor(&entries[1].item), None);
    }

    /// AC 7: the `undated` flag has to survive assembly — a view built with it
    /// set lists its undated tasks, one built without it counts them. The flag
    /// being dropped on the floor here would look exactly like the feature
    /// working in every renderer test, because those build the view by hand.
    ///
    /// Also assembly's other two jobs, neither of which had a test: the local
    /// window (which decides whether a dated task is in range at all) and the
    /// sort. Midday UTC on two different days, so the two window dates differ
    /// — and are those two calendar days — in each of the three CI zones.
    #[test]
    fn assemble_sorts_and_carries_the_window_and_the_undated_flag() {
        use crate::output::TextRender;

        let from = Utc.with_ymd_and_hms(2026, 8, 14, 12, 0, 0).unwrap();
        let to = Utc.with_ymd_and_hms(2026, 8, 16, 12, 0, 0).unwrap();
        let timed = |day, hour| {
            EventTime::DateTimeFloating(
                NaiveDate::from_ymd_opt(2026, 8, day)
                    .unwrap()
                    .and_hms_opt(hour, 0, 0)
                    .unwrap(),
            )
        };
        let build = |undated| {
            AgendaView::assemble(
                // Deliberately out of order: assembly has to sort.
                vec![
                    entry(Event::new("Retro", timed(16, 9))),
                    todo_entry(Todo::new("Someday: rewrite installer")),
                    entry(Event::new("Standup", timed(14, 9))),
                ],
                from,
                to,
                TimeFormat::H24,
                undated,
            )
        };

        let view = build(false);
        assert_eq!(
            view.entries
                .iter()
                .filter_map(|e| e.item.summary())
                .collect::<Vec<_>>(),
            vec!["Standup", "Retro", "Someday: rewrite installer"]
        );
        assert_eq!(
            view.range_start,
            NaiveDate::from_ymd_opt(2026, 8, 14).unwrap()
        );
        assert_eq!(
            view.range_end,
            NaiveDate::from_ymd_opt(2026, 8, 16).unwrap()
        );
        assert!(
            view.to_text()
                .ends_with("+ 1 undated task (caldir events --undated)\u{1b}[0m"),
            "{:?}",
            view.to_text()
        );
        assert!(
            build(true).to_text().contains("Someday: rewrite installer"),
            "{:?}",
            build(true).to_text()
        );
    }

    /// The `Event` arm of `anchor` is the event's own `start`. That identity is
    /// what makes event output unchanged by construction (C13); the borrowed
    /// return type is what makes it a read rather than a re-derivation.
    #[test]
    fn anchor_of_an_event_is_its_own_start() {
        let event = Event::new(
            "Standup",
            EventTime::DateTimeUtc(Utc.with_ymd_and_hms(2026, 8, 14, 9, 0, 0).unwrap()),
        );

        assert_eq!(
            anchor(&CalendarItem::Event(event.clone())),
            Some(&event.start)
        );
    }

    /// AC 6 at the collection seam, not just the sort: an undated task has no
    /// anchor, and `collect_entries` must keep it anyway. Dropping it here
    /// would be a silent drop that the sort-level test cannot see.
    #[test]
    fn an_undated_task_still_reaches_the_entries() {
        const UNDATED_TASK_ICS: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VTODO\r\nUID:someday@u-crawler\r\n\
SUMMARY:Someday: rewrite installer\r\nEND:VTODO\r\nEND:VCALENDAR\r\n";

        let tmp = tempfile::tempdir().unwrap();
        let cal = calendar(tmp.path(), "work");
        std::fs::write(cal.path().join("someday.ics"), UNDATED_TASK_ICS).unwrap();

        let from = Utc.with_ymd_and_hms(2026, 8, 1, 0, 0, 0).unwrap();
        let to = Utc.with_ymd_and_hms(2026, 9, 1, 0, 0, 0).unwrap();
        let entries = collect_entries(vec![cal], from, to);

        assert_eq!(
            entries.iter().map(summary).collect::<Vec<_>>(),
            vec!["Someday: rewrite installer"]
        );
        assert_eq!(anchor(&entries[0].item), None);
    }

    /// A consequence inherited from L2-07, recorded here because it is
    /// discovered at render time otherwise: core is deliberately permissive
    /// and does **not** range-filter tasks, so every task in the calendar
    /// reaches the CLI — undated *and* dated far outside the window. R44's
    /// window predicate is applied in the diff, not in the listing.
    ///
    /// L7-01 keeps them in `entries` and simply does not render them.
    /// **L7-02's suppression rule must therefore cover dated out-of-window
    /// tasks too, not only undated ones** — an anchored task from 2019 would
    /// otherwise open a `caldir today` with a 2019 day header.
    #[test]
    fn a_task_dated_outside_the_window_still_reaches_the_entries() {
        const FAR_PAST_TASK_ICS: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VTODO\r\nUID:ancient@u-crawler\r\nDUE:20190101T090000Z\r\n\
SUMMARY:Ancient task\r\nEND:VTODO\r\nEND:VCALENDAR\r\n";

        let tmp = tempfile::tempdir().unwrap();
        let cal = calendar(tmp.path(), "work");
        std::fs::write(cal.path().join("ancient.ics"), FAR_PAST_TASK_ICS).unwrap();

        let from = Utc.with_ymd_and_hms(2026, 8, 1, 0, 0, 0).unwrap();
        let to = Utc.with_ymd_and_hms(2026, 9, 1, 0, 0, 0).unwrap();
        let entries = collect_entries(vec![cal], from, to);

        assert_eq!(
            entries.iter().map(summary).collect::<Vec<_>>(),
            vec!["Ancient task"]
        );
    }

    /// `T-47` (ADR-10 D3), the pinned name, all three halves in one place.
    /// They are asserted together because splitting them leaves a gap a
    /// regression walks through: a task dropped from `entries` outright
    /// satisfies "absent from the body" on its own, and only the footer and
    /// the `--json` stream can tell that apart from a task correctly withheld.
    ///
    /// - **absent from the body** — never placed in a day group, never given
    ///   an invented date;
    /// - **counted in the footer** — one dimmed line, singular, so it is never
    ///   a silent drop;
    /// - **present in `--json`** — the schema has carried tasks since `L7-03`,
    ///   with `start` and `due` both `null`. `--undated` is a text concern and
    ///   does not gate the stream (`Q-1`), which is why `undated: false` here
    ///   must still produce a task in the JSON array.
    ///
    /// Driven from real files through `collect_entries` + `assemble`, so the
    /// task reaches the view the way it does in production rather than being
    /// hand-placed into `entries`.
    #[test]
    fn an_undated_task_is_absent_from_the_body_counted_in_the_footer_and_present_in_json() {
        use crate::output::{Output, TextRender};

        const UNDATED_TASK_ICS: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VTODO\r\nUID:someday@u-crawler\r\n\
SUMMARY:Someday: rewrite installer\r\nEND:VTODO\r\nEND:VCALENDAR\r\n";

        let today = chrono::Local::now().date_naive();
        let tmp = tempfile::tempdir().unwrap();
        let cal = calendar(tmp.path(), "work");
        cal.create_event(Event::new("Sprint demo", EventTime::Date(today)))
            .unwrap();
        std::fs::write(cal.path().join("someday.ics"), UNDATED_TASK_ICS).unwrap();

        // Wide enough that no zone in the CI matrix can push today's all-day
        // event out of the collection window.
        let from = Utc::now() - chrono::Duration::days(3);
        let to = Utc::now() + chrono::Duration::days(3);
        let view = AgendaView::assemble(
            collect_entries(vec![cal], from, to),
            from,
            to,
            TimeFormat::H24,
            false,
        );

        // Both items reached the view: the halves below are about rendering,
        // not about a task that never arrived.
        assert_eq!(
            view.entries
                .iter()
                .filter_map(|entry| entry.item.summary())
                .collect::<Vec<_>>(),
            vec!["Sprint demo", "Someday: rewrite installer"]
        );

        let text = view.to_text();

        // Halves 1 and 2, byte-exact: one day group holding the event alone,
        // then the dimmed count. The task's summary appears nowhere in it.
        assert!(!text.contains("Someday: rewrite installer"), "{text:?}");
        assert_eq!(
            text,
            concat!(
                "\u{1b}[1mToday\u{1b}[0m\n",
                "  all-day Sprint demo \u{1b}[2m[work]\u{1b}[0m\n",
                "\n",
                "\u{1b}[2m+ 1 undated task (caldir events --undated)\u{1b}[0m",
            )
        );

        // Half 3: the same view under `--json`, with `undated` still false.
        let json = view.to_json();
        let entries = json.as_array().expect("the stream is an array");
        assert_eq!(entries.len(), 2, "{json}");

        let task = entries
            .iter()
            .find(|entry| entry["title"] == "Someday: rewrite installer")
            .expect("the undated task is in the --json stream");
        assert_eq!(task["kind"], "todo");
        assert_eq!(task["uid"], "someday@u-crawler");
        assert_eq!(task["start"], serde_json::Value::Null);
        assert_eq!(task["due"], serde_json::Value::Null);
    }
}
