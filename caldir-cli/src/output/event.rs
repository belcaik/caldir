use caldir_core::{
    CalendarItem, Event, EventTime, ParticipationStatus, Status, TimeFormat, Todo, TodoStatus,
};
use owo_colors::OwoColorize;

use crate::output::time::format_time_only;

pub fn is_visible(event: &Event) -> bool {
    event.status != Status::Cancelled
}

/// Whether an item of any kind belongs in the agenda.
///
/// A CANCELLED task is hidden exactly as a CANCELLED event is. A COMPLETED
/// task is **not** hidden: it is history for its day the way a past event is,
/// and hiding it by default would be a silent drop — of, among other things,
/// the MVP's own `STATUS:COMPLETED` fixture.
///
/// The event arm is [`is_visible`] itself, so it cannot drift.
pub fn is_item_visible(item: &CalendarItem) -> bool {
    match item {
        CalendarItem::Event(event) => is_visible(event),
        CalendarItem::Todo(todo) => todo.status != TodoStatus::Cancelled,
        // Never hide a kind this crate does not know: hiding is a silent drop,
        // showing is at worst an under-described line.
        _ => true,
    }
}

/// Format a standard event line: "  {time} {summary} [{cal_slug}]{status}"
pub fn format_event_line(
    event: &Event,
    cal_slug: &str,
    status: &str,
    time_format: TimeFormat,
) -> String {
    let time = format_time_only(&event.start, time_format);
    let cal_tag = format!("[{}]", cal_slug);

    let summary_text = &event.summary.clone().unwrap_or("(Untitled)".to_string());

    format!("  {} {} {}{}", time, summary_text, cal_tag.dimmed(), status)
}

/// Format a task line: "  {time} {box} {summary} [{cal_slug}]".
///
/// A sibling of [`format_event_line`], deliberately the same shape: the same
/// 2-space indent, the same 7-wide right-aligned time column, the same dimmed
/// calendar tag. No new indent level, no new colour role, no emoji.
///
/// `anchor` is the task's place in time (`DUE`, falling back to `DTSTART`),
/// passed in rather than recomputed so the column and the sort can never
/// disagree. `now` is the reference instant for "overdue" — injected, never
/// read from the clock here, so the rendering is deterministic.
pub fn format_task_line(
    todo: &Todo,
    anchor: Option<&EventTime>,
    cal_slug: &str,
    time_format: TimeFormat,
    now: chrono::DateTime<chrono::Utc>,
) -> String {
    let time = format_task_time(anchor, time_format);
    let cal_tag = format!("[{}]", cal_slug);
    let summary = todo.summary.clone().unwrap_or("(Untitled)".to_string());
    let summary = if todo.status == TodoStatus::Completed {
        summary.dimmed().to_string()
    } else if is_overdue(anchor, now) {
        summary.red().to_string()
    } else {
        summary
    };

    format!(
        "  {} {} {} {}",
        time,
        task_box(todo.status),
        summary,
        cal_tag.dimmed()
    )
}

/// Whether a task's anchor has passed, relative to an injected `now`.
///
/// A DATE-valued anchor is a *day*, not an instant: a task due today is not
/// overdue at midday, only once the day itself is behind us. Comparing
/// `to_utc()` (its midnight) against `now` would redden it from 00:01.
/// An undated task is never overdue — there is no date to be past.
fn is_overdue(anchor: Option<&EventTime>, now: chrono::DateTime<chrono::Utc>) -> bool {
    match anchor {
        None => false,
        Some(EventTime::Date(date)) => *date < now.with_timezone(&chrono::Local).date_naive(),
        Some(time) => time.to_utc() < now,
    }
}

/// The task time column, 7 wide like `format_time_only`'s.
///
/// A DATE-valued `DUE` is **not** all-day — a task is due *by* that day, it
/// does not occupy it — so it prints `due`, never `all-day`. An undated task
/// keeps the column empty rather than borrowing a neighbour's date.
fn format_task_time(anchor: Option<&EventTime>, time_format: TimeFormat) -> String {
    match anchor {
        None => " ".repeat(7),
        Some(EventTime::Date(_)) => format!("{:>7}", "due"),
        Some(time) => format_time_only(time, time_format),
    }
}

/// The ASCII status box. Same alphabet as `diff.rs`'s `+ ~ -`.
fn task_box(status: TodoStatus) -> &'static str {
    match status {
        TodoStatus::NeedsAction => "[ ]",
        TodoStatus::InProcess => "[~]",
        TodoStatus::Completed => "[x]",
        // Never rendered: `is_item_visible` hides a CANCELLED task.
        TodoStatus::Cancelled => "[-]",
    }
}

/// Render a participation status as colored text (e.g. "accepted" in green, "pending" in yellow)
pub fn render_participation_status(status: ParticipationStatus) -> String {
    let label = status.to_string();

    match status {
        ParticipationStatus::Accepted => label.green().to_string(),
        ParticipationStatus::Declined => label.red().to_string(),
        ParticipationStatus::Tentative | ParticipationStatus::NeedsAction => {
            label.yellow().to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caldir_core::{EventTime, Todo};
    use chrono::NaiveDate;

    /// Midday on 2026-08-14, *local*, as the reference instant. Local rather
    /// than UTC so "the day before at 09:00" is in the past in every CI zone.
    fn task_now() -> chrono::DateTime<chrono::Utc> {
        NaiveDate::from_ymd_opt(2026, 8, 14)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap()
            .and_local_timezone(chrono::Local)
            .unwrap()
            .with_timezone(&chrono::Utc)
    }

    fn due_task(summary: &str, due: EventTime) -> Todo {
        let mut todo = Todo::new(summary);
        todo.due = Some(due);
        todo
    }

    /// ADR-10 D4, the base shape: 2-space indent, the same 7-wide right-aligned
    /// time column as an event line, the ASCII box, the dimmed calendar tag.
    /// Nothing else is coloured.
    #[test]
    fn a_task_line_has_the_event_line_shape_with_a_status_box() {
        let due = EventTime::DateTimeFloating(
            NaiveDate::from_ymd_opt(2026, 8, 14)
                .unwrap()
                .and_hms_opt(16, 30, 0)
                .unwrap(),
        );

        assert_eq!(
            format_task_line(
                &due_task("Draft Q3 report", due),
                Some(&EventTime::DateTimeFloating(
                    NaiveDate::from_ymd_opt(2026, 8, 14)
                        .unwrap()
                        .and_hms_opt(16, 30, 0)
                        .unwrap()
                )),
                "work",
                TimeFormat::H24,
                task_now(),
            ),
            "    16:30 [ ] Draft Q3 report \u{1b}[2m[work]\u{1b}[0m"
        );
    }

    /// AC 3, the `all-day` trap: a DATE-valued `DUE` prints `due`, never
    /// `all-day`. A task is due *by* a day; it does not occupy it.
    #[test]
    fn a_date_valued_due_prints_due_not_all_day() {
        let due = EventTime::Date(NaiveDate::from_ymd_opt(2026, 8, 20).unwrap());

        let line = format_task_line(
            &due_task("Ship crawler patch", due.clone()),
            Some(&due),
            "work",
            TimeFormat::H24,
            task_now(),
        );

        assert_eq!(
            line,
            "      due [ ] Ship crawler patch \u{1b}[2m[work]\u{1b}[0m"
        );
        assert!(!line.contains("all-day"));
    }

    /// AC 3: an undated task keeps the 7-wide column empty rather than
    /// borrowing a date it does not have.
    #[test]
    fn an_undated_task_leaves_the_time_column_blank() {
        assert_eq!(
            format_task_line(
                &Todo::new("Someday: rewrite installer"),
                None,
                "personal",
                TimeFormat::H24,
                task_now(),
            ),
            "          [ ] Someday: rewrite installer \u{1b}[2m[personal]\u{1b}[0m"
        );
    }

    /// AC 2: the three rendered statuses get the three ASCII boxes. No emoji.
    #[test]
    fn each_status_gets_its_ascii_box() {
        let due = EventTime::Date(NaiveDate::from_ymd_opt(2026, 8, 20).unwrap());
        let lines: Vec<String> = [
            TodoStatus::NeedsAction,
            TodoStatus::InProcess,
            TodoStatus::Completed,
        ]
        .into_iter()
        .map(|status| {
            let mut todo = due_task("Ship crawler patch", due.clone());
            todo.status = status;
            format_task_line(&todo, Some(&due), "work", TimeFormat::H24, task_now())
        })
        .collect();

        let boxes: Vec<&str> = lines.iter().map(|line| &line[10..13]).collect();
        assert_eq!(boxes, vec!["[ ]", "[~]", "[x]"]);
        assert!(
            lines.iter().all(|line| line.is_ascii()),
            "boxes must stay ASCII: {lines:?}"
        );
    }

    /// ADR-10 D4/D5: COMPLETED dims the summary — the signal that it is
    /// history, not a silent drop. A past `DUE` does not turn it red.
    #[test]
    fn a_completed_task_with_a_past_due_is_dimmed_and_never_red() {
        let due = EventTime::DateTimeFloating(
            NaiveDate::from_ymd_opt(2026, 8, 13)
                .unwrap()
                .and_hms_opt(9, 0, 0)
                .unwrap(),
        );
        let mut todo = due_task("Crawl u_crawler archive", due.clone());
        todo.status = TodoStatus::Completed;

        let line = format_task_line(&todo, Some(&due), "research", TimeFormat::H24, task_now());

        assert!(
            line.contains("\u{1b}[2mCrawl u_crawler archive\u{1b}[0m"),
            "expected a dimmed summary, got {line:?}"
        );
        assert!(
            !line.contains("\u{1b}[31m"),
            "completed must not be red: {line:?}"
        );
    }

    /// ADR-10 D4: a `DUE` in the past on a task that is not COMPLETED turns the
    /// summary red — the existing "wrong/gone" colour role, nothing new.
    ///
    /// TZ-safe: the `DUE` is floating (so it renders 09:00 in every zone) and
    /// `now` is midday of the *following* local day, so it is past everywhere.
    #[test]
    fn an_overdue_task_has_a_red_summary() {
        let due = EventTime::DateTimeFloating(
            NaiveDate::from_ymd_opt(2026, 8, 13)
                .unwrap()
                .and_hms_opt(9, 0, 0)
                .unwrap(),
        );

        let line = format_task_line(
            &due_task("Ship crawler patch", due.clone()),
            Some(&due),
            "work",
            TimeFormat::H24,
            task_now(),
        );

        assert_eq!(
            line,
            "    09:00 [ ] \u{1b}[31mShip crawler patch\u{1b}[39m \u{1b}[2m[work]\u{1b}[0m"
        );
    }

    /// The boundary, and the reason "overdue" is date-aware rather than a bare
    /// instant compare: a task due *today* by DATE is not overdue at midday.
    /// Comparing `to_utc()` (midnight) against `now` would redden it.
    #[test]
    fn a_task_due_today_by_date_is_not_overdue_until_the_day_has_passed() {
        let today = EventTime::Date(chrono::Local::now().date_naive());
        let yesterday =
            EventTime::Date(chrono::Local::now().date_naive() - chrono::Duration::days(1));

        let due_today = format_task_line(
            &due_task("Ship crawler patch", today.clone()),
            Some(&today),
            "work",
            TimeFormat::H24,
            chrono::Utc::now(),
        );
        let due_yesterday = format_task_line(
            &due_task("Ship crawler patch", yesterday.clone()),
            Some(&yesterday),
            "work",
            TimeFormat::H24,
            chrono::Utc::now(),
        );

        assert!(!due_today.contains("\u{1b}[31m"), "{due_today:?}");
        assert!(due_yesterday.contains("\u{1b}[31m"), "{due_yesterday:?}");
    }

    /// An undated task can never be overdue: there is no date to be past.
    #[test]
    fn an_undated_task_is_never_overdue() {
        let line = format_task_line(
            &Todo::new("Someday: rewrite installer"),
            None,
            "personal",
            TimeFormat::H24,
            task_now(),
        );

        assert!(!line.contains("\u{1b}[31m"), "{line:?}");
    }

    #[test]
    fn confirmed_events_are_visible_cancelled_are_not() {
        let start = EventTime::Date(NaiveDate::from_ymd_opt(2026, 5, 27).unwrap());
        let confirmed = Event::new("Standup", start.clone());
        let mut cancelled = Event::new("yolo", start);
        cancelled.status = Status::Cancelled;

        assert!(is_visible(&confirmed));
        assert!(!is_visible(&cancelled));
    }

    /// ADR-10 D5: CANCELLED tasks hide, the other three statuses show —
    /// COMPLETED included, by default.
    #[test]
    fn only_cancelled_tasks_are_hidden_completed_ones_still_show() {
        let visible = [
            TodoStatus::NeedsAction,
            TodoStatus::InProcess,
            TodoStatus::Completed,
        ];

        for status in visible {
            let mut todo = Todo::new("Ship crawler patch");
            todo.status = status;
            assert!(
                is_item_visible(&CalendarItem::Todo(todo)),
                "{status:?} should be visible"
            );
        }

        let mut cancelled = Todo::new("Ship crawler patch");
        cancelled.status = TodoStatus::Cancelled;
        assert!(!is_item_visible(&CalendarItem::Todo(cancelled)));
    }

    /// The item-typed check must keep hiding a CANCELLED *event* — the agenda
    /// calls `is_item_visible` now, so a hole here is a hole in the real path.
    #[test]
    fn the_item_check_hides_a_cancelled_event_exactly_as_before() {
        let start = EventTime::Date(NaiveDate::from_ymd_opt(2026, 5, 27).unwrap());
        let confirmed = Event::new("Standup", start.clone());
        let mut cancelled = Event::new("yolo", start);
        cancelled.status = Status::Cancelled;

        assert!(is_item_visible(&CalendarItem::Event(confirmed)));
        assert!(!is_item_visible(&CalendarItem::Event(cancelled)));
    }
}
