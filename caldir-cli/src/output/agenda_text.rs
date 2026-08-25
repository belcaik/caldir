use caldir_core::{CalendarItem, Event, EventTime};
use chrono::{Duration, NaiveDate};
use owo_colors::OwoColorize;
use std::cmp::Ordering;

use crate::output::TextRender;
use crate::output::agenda::{AgendaEntry, AgendaView, anchor};
use crate::output::event::{format_event_line, render_participation_status};
use crate::output::time::{format_date_label, local_date};

impl TextRender for AgendaView {
    fn to_text(&self) -> String {
        // One entry per (day, event). A multi-day all-day event is repeated
        // under every day it spans in text output only.
        let mut display_entries: Vec<(NaiveDate, &AgendaEntry, &Event)> = self
            .entries
            .iter()
            // TODO(L7-02): tasks now carry an anchor and a place in the sorted
            // entries, but no rendered line shape yet (ADR-10 D4/D3 — the
            // `[ ]` box and the undated footer). They are narrowed out of the
            // *text view* here until L7-02 gives them one. Display-only:
            // nothing infers a deletion from this list, so a task missing from
            // the rendered agenda is never read as an absence.
            .filter_map(|entry| match &entry.item {
                CalendarItem::Event(event) => Some((entry, event)),
                CalendarItem::Todo(_) => None,
            })
            .flat_map(|(entry, event)| {
                display_days(event, self.range_start, self.range_end)
                    .into_iter()
                    .map(move |day| (day, entry, event))
            })
            .collect();

        // The second copy of the agenda's total order (`agenda.rs`'s
        // `sort_entries` is the first); the two must stay in step or text and
        // `--json` disagree. Rule 1 is the *display* day rather than the
        // anchor's day, because a multi-day all-day event is repeated here.
        display_entries.sort_by(|a, b| {
            a.0.cmp(&b.0)
                .then_with(|| match (anchor(&a.1.item), anchor(&b.1.item)) {
                    (None, None) => Ordering::Equal,
                    (None, Some(_)) => Ordering::Greater,
                    (Some(_), None) => Ordering::Less,
                    (Some(a_time), Some(b_time)) => a_time
                        .is_date()
                        .cmp(&b_time.is_date())
                        .reverse()
                        .then_with(|| a_time.to_utc().cmp(&b_time.to_utc()))
                        .then_with(|| kind_ordinal(&a.1.item).cmp(&kind_ordinal(&b.1.item))),
                })
        });

        if display_entries.is_empty() {
            return "No events found".dimmed().to_string();
        }

        let mut lines = Vec::new();
        let mut current_date = None;

        for (day, entry, event) in display_entries {
            if current_date != Some(day) {
                if current_date.is_some() {
                    lines.push(String::new());
                }
                lines.push(format_date_label(day).bold().to_string());
                current_date = Some(day);
            }

            let invite_indicator = entry
                .rsvp
                .map(|status| format!(" ({})", render_participation_status(status)))
                .unwrap_or_default();

            lines.push(format_event_line(
                event,
                entry.calendar.as_deref().unwrap_or("(Unknown calendar)"),
                &invite_indicator,
                self.time_format,
            ));
        }

        lines.join("\n")
    }
}

/// The kind tiebreak, mirroring `agenda.rs` (ADR-10 D2): at an identical
/// instant an event comes before a task. It can never reorder two events.
fn kind_ordinal(item: &CalendarItem) -> u8 {
    match item {
        CalendarItem::Event(_) => 0,
        CalendarItem::Todo(_) => 1,
    }
}

/// The day(s) an event should be listed under, clamped to `[range_start, range_end]`.
/// Most events render once, on their start day.
/// A multi-day all-day event renders under every day it covers
fn display_days(event: &Event, range_start: NaiveDate, range_end: NaiveDate) -> Vec<NaiveDate> {
    if let (EventTime::Date(start), Some(EventTime::Date(end))) = (&event.start, &event.end) {
        // All-day DTEND is exclusive, so the last day covered is `end - 1`.
        let last_day = *end - Duration::days(1);
        if last_day > *start {
            let first = (*start).max(range_start);
            let last = last_day.min(range_end);
            let mut days = Vec::new();
            let mut day = first;
            while day <= last {
                days.push(day);
                day += Duration::days(1);
            }
            return days;
        }
    }

    vec![local_date(&event.start)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::agenda::AgendaEntry;
    use caldir_core::{ParticipationStatus, TimeFormat};
    use chrono::{Duration as ChronoDuration, TimeZone, Utc};

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn entry(event: Event, rsvp: Option<ParticipationStatus>) -> AgendaEntry {
        AgendaEntry {
            item: event.into(),
            calendar: Some("work".to_string()),
            rsvp,
        }
    }

    fn all_day(start: NaiveDate, end_exclusive: NaiveDate) -> Event {
        let mut event = Event::new("Trip", EventTime::Date(start));
        event.end = Some(EventTime::Date(end_exclusive));
        event
    }

    #[test]
    fn single_day_all_day_event_shows_on_its_start_day() {
        // Spans one day (DTEND is exclusive): May 27 only.
        let event = all_day(date(2026, 5, 27), date(2026, 5, 28));

        let days = display_days(&event, date(2026, 5, 25), date(2026, 6, 1));

        assert_eq!(days, vec![date(2026, 5, 27)]);
    }

    #[test]
    fn multi_day_all_day_event_shows_on_every_spanned_day() {
        // May 27 through May 29 inclusive (DTEND May 30 exclusive).
        let event = all_day(date(2026, 5, 27), date(2026, 5, 30));

        let days = display_days(&event, date(2026, 5, 25), date(2026, 6, 1));

        assert_eq!(
            days,
            vec![date(2026, 5, 27), date(2026, 5, 28), date(2026, 5, 29)]
        );
    }

    #[test]
    fn multi_day_event_starting_before_window_is_clamped_to_window_start() {
        // The reported bug: trip began May 27 but today is June 2. It should
        // appear from the window start onward, not under the past start day.
        let event = all_day(date(2026, 5, 27), date(2026, 6, 5));

        let days = display_days(&event, date(2026, 6, 2), date(2026, 6, 7));

        assert_eq!(
            days,
            vec![date(2026, 6, 2), date(2026, 6, 3), date(2026, 6, 4)]
        );
    }

    #[test]
    fn multi_day_event_extending_past_window_is_clamped_to_window_end() {
        let event = all_day(date(2026, 6, 1), date(2026, 6, 20));

        let days = display_days(&event, date(2026, 6, 1), date(2026, 6, 3));

        assert_eq!(
            days,
            vec![date(2026, 6, 1), date(2026, 6, 2), date(2026, 6, 3)]
        );
    }

    #[test]
    fn timed_event_shows_only_on_its_start_day() {
        let mut event = Event::new(
            "Meeting",
            EventTime::DateTimeUtc(Utc.with_ymd_and_hms(2026, 6, 2, 14, 0, 0).unwrap()),
        );
        event.end = Some(EventTime::DateTimeUtc(
            Utc.with_ymd_and_hms(2026, 6, 2, 15, 0, 0).unwrap(),
        ));

        let days = display_days(&event, date(2026, 6, 1), date(2026, 6, 7));

        assert_eq!(days, vec![local_date(&event.start)]);
    }

    /// T-45 (C13, INVIOLABLE). An event-only fixture must render exactly the
    /// bytes it rendered before tasks existed — day labels, the 7-wide time
    /// column, the dimmed calendar tag, the RSVP indicator, the blank line
    /// between days and the absence of a trailing newline, ANSI codes and all.
    ///
    /// The literal is the guard: `anchor` returns an event's own `start`, and
    /// the kind tiebreak can never reorder two events, so this is unchanged by
    /// construction — this test is what makes "by construction" checkable.
    ///
    /// TZ-safe: the days come from the local clock and the timed events are
    /// floating, so every zone renders the same labels and the same wall time.
    #[test]
    fn an_event_only_fixture_renders_byte_identically() {
        let today = chrono::Local::now().date_naive();
        let tomorrow = today + ChronoDuration::days(1);
        // Deliberately not in display order: the sort has to produce it.
        let entries = vec![
            entry(
                Event::new(
                    "Design review",
                    EventTime::DateTimeFloating(today.and_hms_opt(14, 0, 0).unwrap()),
                ),
                Some(ParticipationStatus::NeedsAction),
            ),
            entry(Event::new("Conference", EventTime::Date(tomorrow)), None),
            entry(Event::new("Sprint demo", EventTime::Date(today)), None),
            entry(
                Event::new(
                    "Standup",
                    EventTime::DateTimeFloating(today.and_hms_opt(9, 0, 0).unwrap()),
                ),
                None,
            ),
        ];

        let view = AgendaView {
            time_format: TimeFormat::H24,
            entries,
            range_start: today,
            range_end: tomorrow,
        };

        assert_eq!(
            view.to_text(),
            concat!(
                "\u{1b}[1mToday\u{1b}[0m\n",
                "  all-day Sprint demo \u{1b}[2m[work]\u{1b}[0m\n",
                "    09:00 Standup \u{1b}[2m[work]\u{1b}[0m\n",
                "    14:00 Design review \u{1b}[2m[work]\u{1b}[0m (\u{1b}[33mpending\u{1b}[39m)\n",
                "\n",
                "\u{1b}[1mTomorrow\u{1b}[0m\n",
                "  all-day Conference \u{1b}[2m[work]\u{1b}[0m",
            )
        );
    }

    /// The other half of C13 at this seam: a task in `entries` must not
    /// perturb the event lines. Tasks have no rendered shape until L7-02
    /// (ADR-10 D4), so today they are simply absent — never a blank line, a
    /// stray day header or a reordered event.
    #[test]
    fn a_task_in_the_entries_does_not_disturb_the_event_lines() {
        let today = chrono::Local::now().date_naive();
        let tomorrow = today + ChronoDuration::days(1);
        let events = vec![
            entry(Event::new("Sprint demo", EventTime::Date(today)), None),
            entry(Event::new("Conference", EventTime::Date(tomorrow)), None),
        ];

        let mut todo = caldir_core::Todo::new("Ship crawler patch");
        todo.due = Some(EventTime::Date(today));
        let mut undated = caldir_core::Todo::new("Someday: rewrite installer");
        undated.due = None;

        let mut with_tasks = vec![
            entry(Event::new("Sprint demo", EventTime::Date(today)), None),
            AgendaEntry {
                item: todo.into(),
                calendar: Some("work".to_string()),
                rsvp: None,
            },
            entry(Event::new("Conference", EventTime::Date(tomorrow)), None),
            AgendaEntry {
                item: undated.into(),
                calendar: Some("work".to_string()),
                rsvp: None,
            },
        ];
        with_tasks.rotate_left(1);

        let view = |entries| AgendaView {
            time_format: TimeFormat::H24,
            entries,
            range_start: today,
            range_end: tomorrow,
        };

        assert_eq!(view(with_tasks).to_text(), view(events).to_text());
    }

    /// A calendar holding nothing but tasks renders the same empty-agenda text
    /// as an empty one — not a bare day header with no lines under it.
    #[test]
    fn a_task_only_view_renders_the_empty_agenda_text() {
        let today = chrono::Local::now().date_naive();
        let mut todo = caldir_core::Todo::new("Ship crawler patch");
        todo.due = Some(EventTime::Date(today));

        let view = AgendaView {
            time_format: TimeFormat::H24,
            entries: vec![AgendaEntry {
                item: todo.into(),
                calendar: Some("work".to_string()),
                rsvp: None,
            }],
            range_start: today,
            range_end: today,
        };

        assert_eq!(
            view.to_text(),
            AgendaView {
                time_format: TimeFormat::H24,
                entries: Vec::new(),
                range_start: today,
                range_end: today,
            }
            .to_text()
        );
    }
}
