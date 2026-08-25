use caldir_core::{CalendarItem, Event, ParticipationStatus, Status, TimeFormat, TodoStatus};
use owo_colors::OwoColorize;

use crate::output::time::format_time_only;

pub fn is_visible(event: &Event) -> bool {
    event.status != Status::Cancelled
}

/// Whether an item of any kind belongs in the agenda (ADR-10 D5).
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
