use caldir_core::{CalendarItem, Event, EventTime};
use chrono::{Duration, NaiveDate, Utc};
use owo_colors::OwoColorize;

use crate::output::TextRender;
use crate::output::agenda::{AgendaEntry, AgendaView, anchor};
use crate::output::event::{format_event_line, format_task_line, render_participation_status};
use crate::output::time::{format_date_label, local_date};

/// One rendered line: an entry, the day it is listed under, and the anchor it
/// sits at. Undated items never become one — they have no day to be listed
/// under (ADR-10 D3) — so this carries a plain `&EventTime`, and the sort below
/// needs no "no time at all" branch.
struct DisplayLine<'a> {
    day: NaiveDate,
    anchor: &'a EventTime,
    entry: &'a AgendaEntry,
}

impl TextRender for AgendaView {
    fn to_text(&self) -> String {
        // The reference instant for "overdue". Read once, so every line in one
        // rendering agrees even if the clock ticks mid-render.
        let now = Utc::now();

        // One line per (day, item). A multi-day all-day event is repeated
        // under every day it spans in text output only.
        let mut display_entries: Vec<DisplayLine> = self
            .entries
            .iter()
            .filter_map(|entry| anchor(&entry.item).map(|anchor| (entry, anchor)))
            .flat_map(|(entry, anchor)| {
                item_display_days(entry, anchor, self.range_start, self.range_end)
                    .into_iter()
                    .map(move |day| DisplayLine { day, anchor, entry })
            })
            .collect();

        // The second copy of the agenda's total order (`agenda.rs`'s
        // `sort_entries` is the first); the two must stay in step or text and
        // `--json` disagree. Rule 1 is the *display* day rather than the
        // anchor's day, because a multi-day all-day event is repeated here.
        display_entries.sort_by(|a, b| {
            a.day
                .cmp(&b.day)
                .then_with(|| a.anchor.is_date().cmp(&b.anchor.is_date()).reverse())
                .then_with(|| a.anchor.to_utc().cmp(&b.anchor.to_utc()))
                .then_with(|| kind_ordinal(&a.entry.item).cmp(&kind_ordinal(&b.entry.item)))
        });

        let mut lines = Vec::new();

        if display_entries.is_empty() {
            lines.push("No events found".dimmed().to_string());
        }

        let mut current_date = None;

        for DisplayLine { day, anchor, entry } in display_entries {
            if current_date != Some(day) {
                if current_date.is_some() {
                    lines.push(String::new());
                }
                lines.push(format_date_label(day).bold().to_string());
                current_date = Some(day);
            }

            let calendar = entry.calendar.as_deref().unwrap_or("(Unknown calendar)");

            match &entry.item {
                CalendarItem::Event(event) => {
                    let invite_indicator = entry
                        .rsvp
                        .map(|status| format!(" ({})", render_participation_status(status)))
                        .unwrap_or_default();

                    lines.push(format_event_line(
                        event,
                        calendar,
                        &invite_indicator,
                        self.time_format,
                    ));
                }
                CalendarItem::Todo(todo) => lines.push(format_task_line(
                    todo,
                    Some(anchor),
                    calendar,
                    self.time_format,
                    now,
                )),
            }
        }

        lines.extend(self.undated_lines(now));

        lines.join("\n")
    }
}

impl AgendaView {
    /// The undated tail (ADR-10 D3). An undated task belongs to no day, so it
    /// is never placed in a day group and never given an invented date.
    ///
    /// By default it is **counted, not listed**: one dimmed footer, so a
    /// thousand-item completed archive cannot swamp the agenda and no task is
    /// ever silently dropped. `--undated` lists them instead, in one trailing
    /// section headed `No date`.
    ///
    /// Empty when there are no undated tasks — an event-only calendar gains
    /// nothing, not even a trailing blank line (C13).
    fn undated_lines(&self, now: chrono::DateTime<Utc>) -> Vec<String> {
        let undated: Vec<&AgendaEntry> = self
            .entries
            .iter()
            .filter(|entry| {
                matches!(entry.item, CalendarItem::Todo(_)) && anchor(&entry.item).is_none()
            })
            .collect();

        if undated.is_empty() {
            return Vec::new();
        }

        let mut lines = vec![String::new()];

        if !self.undated {
            let noun = if undated.len() == 1 { "task" } else { "tasks" };
            lines.push(
                format!(
                    "+ {} undated {} (caldir events --undated)",
                    undated.len(),
                    noun
                )
                .dimmed()
                .to_string(),
            );
            return lines;
        }

        lines.push("No date".bold().to_string());
        lines.extend(undated.iter().filter_map(|entry| {
            let CalendarItem::Todo(todo) = &entry.item else {
                return None;
            };
            Some(format_task_line(
                todo,
                None,
                entry.calendar.as_deref().unwrap_or("(Unknown calendar)"),
                self.time_format,
                now,
            ))
        }));
        lines
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

/// The day(s) an item should be listed under, clamped to `[range_start, range_end]`.
///
/// A task has no `end` (C-2 forbids `DTEND` in a VTODO), so it is always a
/// single day — and that day must be *inside* the window. Core is deliberately
/// permissive and does not range-filter tasks, so a task due in 2019 reaches
/// the renderer on a `caldir today`; without this it would open the agenda with
/// a 2019 day header. Suppressing it here is the window the user asked for, the
/// same window that already decides which events core returns at all.
fn item_display_days(
    entry: &AgendaEntry,
    anchor: &EventTime,
    range_start: NaiveDate,
    range_end: NaiveDate,
) -> Vec<NaiveDate> {
    if let CalendarItem::Event(event) = &entry.item {
        return display_days(event, range_start, range_end);
    }

    let day = local_date(anchor);
    if (range_start..=range_end).contains(&day) {
        vec![day]
    } else {
        Vec::new()
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

    fn todo_entry(todo: caldir_core::Todo) -> AgendaEntry {
        AgendaEntry {
            item: todo.into(),
            calendar: Some("work".to_string()),
            rsvp: None,
        }
    }

    /// L7-01 wrote the kind ordinal into this file's sort chain for parity with
    /// `agenda.rs` but could not observe it: tasks were filtered out of the
    /// text view before the sort ran. This is the lock it could not write.
    /// Event before task at an identical instant (ADR-10 D2 rule 4), asserted
    /// on the rendered bytes, from a fixture that arrives task-first.
    ///
    /// TZ-safe: both items are floating, so every zone renders 09:00 tomorrow.
    #[test]
    fn an_event_and_a_task_at_the_same_instant_render_event_first() {
        let today = chrono::Local::now().date_naive();
        let tomorrow = today + ChronoDuration::days(1);
        let instant = EventTime::DateTimeFloating(tomorrow.and_hms_opt(9, 0, 0).unwrap());

        let mut todo = caldir_core::Todo::new("Draft Q3 report");
        todo.due = Some(instant.clone());

        let view = AgendaView {
            time_format: TimeFormat::H24,
            undated: false,
            // Task first: only rule 4 can move it behind the event.
            entries: vec![
                todo_entry(todo),
                entry(Event::new("Standup", instant), None),
            ],
            range_start: today,
            range_end: tomorrow,
        };

        assert_eq!(
            view.to_text(),
            concat!(
                "\u{1b}[1mTomorrow\u{1b}[0m\n",
                "    09:00 Standup \u{1b}[2m[work]\u{1b}[0m\n",
                "    09:00 [ ] Draft Q3 report \u{1b}[2m[work]\u{1b}[0m",
            )
        );
    }

    /// T-47 (ADR-10 D3). An undated task is **absent from the body** — never
    /// placed in a day group, never given an invented date — and **counted in
    /// one dimmed footer** so it is never a silent drop.
    ///
    /// The `--json` half of the ticket's T-47 is refused here: the stream is
    /// still event-only until L7-03 widens the schema, and that is locked by
    /// `agenda_json::tests::the_json_stream_stays_event_only_until_the_schema_widens`.
    #[test]
    fn an_undated_task_is_absent_from_the_body_and_counted_in_the_footer() {
        let today = chrono::Local::now().date_naive();

        let view = AgendaView {
            time_format: TimeFormat::H24,
            undated: false,
            entries: vec![
                entry(Event::new("Sprint demo", EventTime::Date(today)), None),
                todo_entry(caldir_core::Todo::new("Someday: rewrite installer")),
                todo_entry(caldir_core::Todo::new("Crawl u_crawler archive")),
            ],
            range_start: today,
            range_end: today,
        };

        let text = view.to_text();

        assert!(!text.contains("Someday: rewrite installer"), "{text:?}");
        assert!(!text.contains("Crawl u_crawler archive"), "{text:?}");
        assert_eq!(
            text,
            concat!(
                "\u{1b}[1mToday\u{1b}[0m\n",
                "  all-day Sprint demo \u{1b}[2m[work]\u{1b}[0m\n",
                "\n",
                "\u{1b}[2m+ 2 undated tasks (caldir events --undated)\u{1b}[0m",
            )
        );
    }

    /// AC 6: the footer counts, so it must agree with reality — one task is
    /// "1 undated task", and it is not a plural template with a number in it.
    #[test]
    fn the_footer_counts_one_undated_task_in_the_singular() {
        let today = chrono::Local::now().date_naive();

        let view = AgendaView {
            time_format: TimeFormat::H24,
            undated: false,
            entries: vec![
                entry(Event::new("Sprint demo", EventTime::Date(today)), None),
                todo_entry(caldir_core::Todo::new("Someday: rewrite installer")),
            ],
            range_start: today,
            range_end: today,
        };

        assert!(
            view.to_text()
                .ends_with("\u{1b}[2m+ 1 undated task (caldir events --undated)\u{1b}[0m"),
            "{:?}",
            view.to_text()
        );
    }

    /// AC 7: `--undated` lists them instead, in one trailing section after the
    /// last dated day, headed `No date` with a day label's `.bold()`.
    #[test]
    fn the_undated_flag_lists_them_in_a_trailing_no_date_section() {
        let today = chrono::Local::now().date_naive();
        let mut completed = caldir_core::Todo::new("Crawl u_crawler archive");
        completed.status = caldir_core::TodoStatus::Completed;

        let view = AgendaView {
            time_format: TimeFormat::H24,
            undated: true,
            entries: vec![
                entry(Event::new("Sprint demo", EventTime::Date(today)), None),
                todo_entry(completed),
                AgendaEntry {
                    item: caldir_core::Todo::new("Someday: rewrite installer").into(),
                    calendar: Some("personal".to_string()),
                    rsvp: None,
                },
            ],
            range_start: today,
            range_end: today,
        };

        assert_eq!(
            view.to_text(),
            concat!(
                "\u{1b}[1mToday\u{1b}[0m\n",
                "  all-day Sprint demo \u{1b}[2m[work]\u{1b}[0m\n",
                "\n",
                "\u{1b}[1mNo date\u{1b}[0m\n",
                "          [x] \u{1b}[2mCrawl u_crawler archive\u{1b}[0m \u{1b}[2m[work]\u{1b}[0m\n",
                "          [ ] Someday: rewrite installer \u{1b}[2m[personal]\u{1b}[0m",
            )
        );
    }

    /// AC 6: no undated task, no footer — an event-only calendar must not gain
    /// a trailing blank line or anything else. (The byte-exact half of this is
    /// `an_event_only_fixture_renders_byte_identically`; this is the
    /// `--undated` half, which must be just as inert.)
    #[test]
    fn an_event_only_view_gains_nothing_even_with_the_undated_flag() {
        let today = chrono::Local::now().date_naive();
        let view = |undated| AgendaView {
            time_format: TimeFormat::H24,
            undated,
            entries: vec![entry(
                Event::new("Sprint demo", EventTime::Date(today)),
                None,
            )],
            range_start: today,
            range_end: today,
        };

        assert_eq!(
            view(false).to_text(),
            concat!(
                "\u{1b}[1mToday\u{1b}[0m\n",
                "  all-day Sprint demo \u{1b}[2m[work]\u{1b}[0m",
            )
        );
        assert_eq!(view(true).to_text(), view(false).to_text());
    }

    /// The suppression rule has to cover **dated** out-of-window tasks too, not
    /// only undated ones: core does not range-filter tasks, so a task due in
    /// 2019 arrives in `entries` on a `caldir today` and would otherwise open
    /// the agenda with a 2019 day header. It is out of the window the user
    /// asked for, so it is not listed — and it is not undated, so it is not in
    /// the undated count either, with or without `--undated`.
    #[test]
    fn a_task_dated_outside_the_window_is_not_listed_and_not_counted() {
        let today = chrono::Local::now().date_naive();
        let mut ancient = caldir_core::Todo::new("Ancient task");
        ancient.due = Some(EventTime::Date(date(2019, 1, 1)));
        let mut distant = caldir_core::Todo::new("Distant task");
        distant.due = Some(EventTime::Date(today + ChronoDuration::days(400)));

        let view = |undated| AgendaView {
            time_format: TimeFormat::H24,
            undated,
            entries: vec![
                entry(Event::new("Sprint demo", EventTime::Date(today)), None),
                todo_entry(ancient.clone()),
                todo_entry(distant.clone()),
            ],
            range_start: today,
            range_end: today,
        };

        let expected = concat!(
            "\u{1b}[1mToday\u{1b}[0m\n",
            "  all-day Sprint demo \u{1b}[2m[work]\u{1b}[0m",
        );
        assert_eq!(view(false).to_text(), expected);
        assert_eq!(view(true).to_text(), expected);
    }

    /// A window boundary day is *inside* the window: the suppression rule must
    /// not eat the last day of `caldir events --to`.
    #[test]
    fn a_task_due_on_the_last_day_of_the_window_is_still_listed() {
        let today = chrono::Local::now().date_naive();
        let tomorrow = today + ChronoDuration::days(1);
        let mut todo = caldir_core::Todo::new("Ship crawler patch");
        todo.due = Some(EventTime::Date(tomorrow));

        let view = AgendaView {
            time_format: TimeFormat::H24,
            undated: false,
            entries: vec![todo_entry(todo)],
            range_start: today,
            range_end: tomorrow,
        };

        assert_eq!(
            view.to_text(),
            concat!(
                "\u{1b}[1mTomorrow\u{1b}[0m\n",
                "      due [ ] Ship crawler patch \u{1b}[2m[work]\u{1b}[0m",
            )
        );
    }

    /// A view holding nothing but suppressed tasks still says "No events
    /// found" — and still carries the count, so the tasks are not silently
    /// gone. This replaces L7-01's `a_task_only_view_renders_the_empty_agenda_
    /// text`, whose premise (a dated in-window task renders nothing) L7-02
    /// deliberately ends.
    #[test]
    fn a_view_of_only_undated_tasks_says_no_events_found_and_still_counts_them() {
        let today = chrono::Local::now().date_naive();

        let view = AgendaView {
            time_format: TimeFormat::H24,
            undated: false,
            entries: vec![todo_entry(caldir_core::Todo::new(
                "Someday: rewrite installer",
            ))],
            range_start: today,
            range_end: today,
        };

        assert_eq!(
            view.to_text(),
            concat!(
                "\u{1b}[2mNo events found\u{1b}[0m\n",
                "\n",
                "\u{1b}[2m+ 1 undated task (caldir events --undated)\u{1b}[0m",
            )
        );
    }

    /// A CANCELLED task is filtered out upstream by `is_item_visible`, so it
    /// can never reach a line or the count. Guarded at this seam too, because
    /// the count is computed here and a `Todo`-shaped count is easy to write
    /// without the visibility check.
    #[test]
    fn a_cancelled_task_is_neither_listed_nor_counted() {
        let today = chrono::Local::now().date_naive();
        let mut cancelled = caldir_core::Todo::new("Abandoned idea");
        cancelled.status = caldir_core::TodoStatus::Cancelled;
        let mut cancelled_dated = cancelled.clone();
        cancelled_dated.summary = Some("Abandoned dated idea".to_string());
        cancelled_dated.due = Some(EventTime::Date(today));

        let entries: Vec<AgendaEntry> = vec![
            entry(Event::new("Sprint demo", EventTime::Date(today)), None),
            todo_entry(cancelled),
            todo_entry(cancelled_dated),
        ]
        .into_iter()
        .filter(|entry| crate::output::event::is_item_visible(&entry.item))
        .collect();

        let view = AgendaView {
            time_format: TimeFormat::H24,
            undated: false,
            entries,
            range_start: today,
            range_end: today,
        };

        assert_eq!(
            view.to_text(),
            concat!(
                "\u{1b}[1mToday\u{1b}[0m\n",
                "  all-day Sprint demo \u{1b}[2m[work]\u{1b}[0m",
            )
        );
    }

    /// ADR-10 D5 / ADR-10 §Consequences (v): a COMPLETED task with a past DUE
    /// renders dimmed `[x]` **and is not hidden**. Hiding it by default would
    /// hide the MVP's own `STATUS:COMPLETED` fixture — a silent drop.
    #[test]
    fn a_completed_task_with_a_past_due_is_dimmed_and_not_hidden() {
        let today = chrono::Local::now().date_naive();
        let yesterday = today - ChronoDuration::days(1);
        let mut todo = caldir_core::Todo::new("Crawl u_crawler archive");
        todo.due = Some(EventTime::Date(yesterday));
        todo.status = caldir_core::TodoStatus::Completed;

        let view = AgendaView {
            time_format: TimeFormat::H24,
            undated: false,
            entries: vec![AgendaEntry {
                item: todo.into(),
                calendar: Some("research".to_string()),
                rsvp: None,
            }],
            range_start: yesterday,
            range_end: today,
        };

        // Dimmed, not red: COMPLETED wins over overdue.
        assert_eq!(
            view.to_text(),
            concat!(
                "\u{1b}[1m",
                "%YESTERDAY%",
                "\u{1b}[0m\n",
                "      due [x] \u{1b}[2mCrawl u_crawler archive\u{1b}[0m \u{1b}[2m[research]\u{1b}[0m",
            )
            .replace("%YESTERDAY%", &format_date_label(yesterday))
        );
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
            undated: false,
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
    /// perturb the *event* lines. L7-01 could only assert that by asserting
    /// tasks rendered nothing at all; now they render, so the claim is checked
    /// the stronger way — against literal bytes whose event lines are
    /// character-for-character the ones the event-only fixture produces. The
    /// dated task takes its own line at its own anchor, the undated one is
    /// counted in the footer, and neither moves, re-indents or re-colours an
    /// event line.
    #[test]
    fn a_task_in_the_entries_does_not_disturb_the_event_lines() {
        let today = chrono::Local::now().date_naive();
        let tomorrow = today + ChronoDuration::days(1);

        let mut todo = caldir_core::Todo::new("Ship crawler patch");
        todo.due = Some(EventTime::Date(today));
        let mut undated = caldir_core::Todo::new("Someday: rewrite installer");
        undated.due = None;

        let mut with_tasks = vec![
            entry(Event::new("Sprint demo", EventTime::Date(today)), None),
            todo_entry(todo),
            entry(Event::new("Conference", EventTime::Date(tomorrow)), None),
            todo_entry(undated),
        ];
        with_tasks.rotate_left(1);

        let view = AgendaView {
            time_format: TimeFormat::H24,
            undated: false,
            entries: with_tasks,
            range_start: today,
            range_end: tomorrow,
        };

        assert_eq!(
            view.to_text(),
            concat!(
                "\u{1b}[1mToday\u{1b}[0m\n",
                "  all-day Sprint demo \u{1b}[2m[work]\u{1b}[0m\n",
                "      due [ ] Ship crawler patch \u{1b}[2m[work]\u{1b}[0m\n",
                "\n",
                "\u{1b}[1mTomorrow\u{1b}[0m\n",
                "  all-day Conference \u{1b}[2m[work]\u{1b}[0m\n",
                "\n",
                "\u{1b}[2m+ 1 undated task (caldir events --undated)\u{1b}[0m",
            )
        );
    }

    /// A calendar whose only task falls outside the requested window renders
    /// the same empty-agenda text as an empty one — not a bare day header, and
    /// not a 2019 day header on a `caldir today`.
    ///
    /// This is L7-01's `a_task_only_view_renders_the_empty_agenda_text` with
    /// its premise updated: an *in-window dated* task now renders a line
    /// (that is L7-02's whole point), so the fixture moves to the case that is
    /// still meant to render nothing. The undated case is
    /// `a_view_of_only_undated_tasks_says_no_events_found_and_still_counts_them`.
    #[test]
    fn a_view_of_only_out_of_window_tasks_renders_the_empty_agenda_text() {
        let today = chrono::Local::now().date_naive();
        let mut todo = caldir_core::Todo::new("Ship crawler patch");
        todo.due = Some(EventTime::Date(date(2019, 1, 1)));

        let view = AgendaView {
            time_format: TimeFormat::H24,
            undated: false,
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
                undated: false,
                entries: Vec::new(),
                range_start: today,
                range_end: today,
            }
            .to_text()
        );
    }
}
