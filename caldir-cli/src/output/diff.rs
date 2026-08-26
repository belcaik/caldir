use crate::output::agenda::anchor;
use crate::output::time::format_datetime;
use caldir_core::{
    Attachment, Attendee, Caldir, Calendar, CalendarDiff, CalendarItem, Event, EventChange,
    Recurrence, Reminder, TimeFormat, Todo, XProperty,
};
use owo_colors::OwoColorize;
use std::collections::{HashMap, HashSet};
use std::fmt;

/// The time column of an item with no anchor at all. The diff has no footer to
/// count undated tasks into, so an undated task must still occupy a line
/// — printing nothing there is the under-described diff this
/// renderer exists to avoid.
const NO_DATE: &str = "(no date)";

/// Extension trait for TUI rendering with colors. Implementations that format
/// event times take the caldir context explicitly, so rendering has no global
/// config dependency.
pub trait Render {
    fn render(&self, caldir: &Caldir) -> String;
}

fn event_change_symbol(kind: &EventChange) -> &str {
    match kind {
        EventChange::Create(_) => "+",
        EventChange::Update { .. } => "~",
        EventChange::Delete(_) => "-",
    }
}

/// Colorize text according to the diff kind
fn colorize_diff(kind: &EventChange, text: &str) -> String {
    match kind {
        EventChange::Create(_) => text.green().to_string(),
        EventChange::Update { .. } => text.yellow().to_string(),
        EventChange::Delete(_) => text.red().to_string(),
    }
}

impl Render for EventChange {
    fn render(&self, caldir: &Caldir) -> String {
        render_change(self, caldir.config().time_format())
    }
}

/// The one-line form of a change, over the display config alone.
///
/// `Caldir` is threaded no further than the trait impl: everything below this
/// point needs only a `TimeFormat`, and taking one is what makes the renderer
/// testable at all — `Caldir` has no constructor outside `caldir-core`.
fn render_change(change: &EventChange, time_format: TimeFormat) -> String {
    let item = match change {
        EventChange::Create(item) | EventChange::Delete(item) => item,
        EventChange::Update { to, .. } => to,
    };

    let summary_text = &item.summary().unwrap_or("(Untitled)").to_string();

    let summary = colorize_diff(change, summary_text);

    let time = anchor(item).map_or_else(
        || NO_DATE.to_string(),
        |anchor| format_datetime(anchor, time_format),
    );

    let is_recurring = match item {
        CalendarItem::Event(event) => event.recurrence.is_some(),
        CalendarItem::Todo(todo) => todo.recurrence.is_some(),
        // A kind core adds later carries no recurrence the CLI can read; no
        // badge is the conservative render, never a wrong one.
        _ => false,
    };

    let recurring = if is_recurring { " 🔁" } else { "" };

    format!(
        "{} {} {}{}",
        colorize_diff(change, event_change_symbol(change)),
        summary,
        time.dimmed(),
        recurring
    )
}

impl Render for Calendar {
    fn render(&self, _caldir: &Caldir) -> String {
        format!("📅 {}", self.slug().unwrap_or(""))
    }
}

/// Threshold for compact view (show counts instead of individual events)
const COMPACT_THRESHOLD: usize = 5;

/// Render a list of diffs, using compact view if there are many events and verbose is false
fn render_diff_list(
    diffs: &[EventChange],
    verbose: bool,
    time_format: TimeFormat,
    lines: &mut Vec<String>,
) {
    if verbose || diffs.len() <= COMPACT_THRESHOLD {
        // Full view: show each event
        for diff in diffs {
            lines.push(format!("   {}", render_change(diff, time_format)));
            // Always show field diffs for updates when in full view
            if let EventChange::Update { .. } = diff {
                lines.extend(
                    render_field_diffs(diff, time_format)
                        .into_iter()
                        .map(|l| format!("     {}", l)),
                );
            }
        }
    } else {
        // Compact view: show counts by diff kind
        let creates = diffs
            .iter()
            .filter(|d| matches!(d, EventChange::Create(_)))
            .count();

        let updates = diffs
            .iter()
            .filter(|d| matches!(d, EventChange::Update { .. }))
            .count();

        let deletes = diffs
            .iter()
            .filter(|d| matches!(d, EventChange::Delete(_)))
            .count();

        if creates > 0 {
            let label = format!("({} new {})", creates, pluralize("event", creates));
            lines.push(format!("   {} {}", "+".green(), label.green()));
        }
        if updates > 0 {
            let label = format!("({} changed {})", updates, pluralize("event", updates));
            lines.push(format!("   {} {}", "~".yellow(), label.yellow()));
        }
        if deletes > 0 {
            let label = format!("({} deleted {})", deletes, pluralize("event", deletes));
            lines.push(format!("   {} {}", "-".red(), label.red()));
        }
    }
}

/// Simple pluralization helper
fn pluralize(word: &str, count: usize) -> &str {
    if count == 1 {
        word
    } else {
        match word {
            "event" => "events",
            "change" => "changes",
            _ => word,
        }
    }
}

/// Extended rendering for CalendarDiff with directional output
pub trait CalendarDiffRender {
    fn render(&self, verbose: bool, caldir: &Caldir, provider: Option<&str>) -> String;
    fn render_pull(&self, verbose: bool, caldir: &Caldir) -> String;
    fn render_push(&self, verbose: bool, caldir: &Caldir, provider: Option<&str>) -> String;
    fn render_discard(&self, verbose: bool, caldir: &Caldir, provider: Option<&str>) -> String;
}

fn render_bidirectional(
    diff: &CalendarDiff,
    verbose: bool,
    caldir: &Caldir,
    provider: Option<&str>,
    push_label: &str,
    pull_label: &str,
) -> String {
    render_sections(
        diff.outgoing(),
        diff.incoming(),
        Withheld {
            task_deletes: diff.suppressed_task_deletes(),
            task_pushes: diff.withheld_task_pushes(),
            provider,
        },
        verbose,
        caldir.config().time_format(),
        push_label,
        pull_label,
    )
}

/// What caldir kept back on this run, carried together so the two notices
/// cannot drift apart at a call site.
#[derive(Clone, Copy)]
struct Withheld<'a> {
    /// The version-skew belt withheld an incoming task delete: the remote
    /// reported no tasks at all, which core refuses to read as "every task
    /// was deleted".
    task_deletes: bool,
    /// How many outgoing changes were withheld because the remote provider
    /// does not advertise `tasks.write`.
    task_pushes: usize,
    /// The provider that could not take the task, for naming it in the
    /// notice. `None` for a local-only calendar — there is no provider to
    /// name, and `task_pushes` is always `0` in that case.
    provider: Option<&'a str>,
}

/// The bidirectional body, over the diff's contents rather than the diff.
///
/// `CalendarDiff` has no constructor outside `caldir-core`, so this signature
/// is the only seam at which the "No changes" early return and the suppression
/// belt can be locked at all.
fn render_sections(
    outgoing: &[EventChange],
    incoming: &[EventChange],
    withheld: Withheld,
    verbose: bool,
    time_format: TimeFormat,
    push_label: &str,
    pull_label: &str,
) -> String {
    // The belt is checked *before* the empty test, not after: it suppresses
    // an incoming delete, so the diff it leaves behind can be entirely empty.
    // Reporting "No changes" for a run that withheld one is the silent
    // suppression the accessor exists to prevent.
    let warnings: Vec<String> = suppression_warning(withheld.task_deletes)
        .into_iter()
        .chain(withheld_warning(
            withheld.task_pushes,
            "pushed",
            withheld.provider,
        ))
        .collect();

    if outgoing.is_empty() && incoming.is_empty() {
        return if warnings.is_empty() {
            "   No changes".dimmed().to_string()
        } else {
            warnings.join("\n")
        };
    }

    let mut lines = Vec::new();

    if !outgoing.is_empty() {
        lines.push(format!("   {}:", push_label).dimmed().to_string());
        render_diff_list(outgoing, verbose, time_format, &mut lines);
    }

    if !incoming.is_empty() {
        if !outgoing.is_empty() {
            lines.push(String::new());
        }
        lines.push(format!("   {}:", pull_label).dimmed().to_string());
        render_diff_list(incoming, verbose, time_format, &mut lines);
    }

    lines.extend(warnings);

    lines.join("\n")
}

/// The version-skew belt, made audible. The wording may yet improve; the
/// signal itself must be here.
///
/// Existing colour roles only: the same `⚠` in the same yellow `doctor` uses.
/// An event-only calendar can never reach this — the belt is scoped to tasks —
/// so no existing situation changes text.
fn suppression_warning(suppressed_task_deletes: bool) -> Option<String> {
    suppressed_task_deletes.then(|| {
        format!(
            "   {} task deletion withheld: the remote reported no tasks at all, which is \
indistinguishable from a provider that cannot list them. No task file was removed.",
            "⚠".yellow()
        )
    })
}

/// The withheld task pushes, made audible: caldir never drops a change in
/// silence.
///
/// Same `⚠` in the same yellow as the version-skew belt above — one affordance
/// for "caldir kept something back", not two. An event-only calendar, or one
/// whose provider advertises `tasks.write`, can never reach this, so no
/// existing situation changes text.
///
/// `provider` names the remote that could not take the task (§16). It is only
/// absent when `withheld_task_pushes` is `0` — a local-only calendar has no
/// provider and also withholds nothing — so the fallback here is defensive,
/// never the expected path.
fn withheld_warning(
    withheld_task_pushes: usize,
    verb: &str,
    provider: Option<&str>,
) -> Option<String> {
    (withheld_task_pushes > 0).then(|| {
        format!(
            "   {} {} task {} not {}: Provider \"{}\" does not support task writes.",
            "⚠".yellow(),
            withheld_task_pushes,
            pluralize("change", withheld_task_pushes),
            verb,
            provider.unwrap_or("this calendar's remote"),
        )
    })
}

impl CalendarDiffRender for CalendarDiff {
    fn render(&self, verbose: bool, caldir: &Caldir, provider: Option<&str>) -> String {
        render_bidirectional(
            self,
            verbose,
            caldir,
            provider,
            "Local changes (to push)",
            "Remote changes (to pull)",
        )
    }

    fn render_pull(&self, verbose: bool, caldir: &Caldir) -> String {
        render_incoming(
            self.incoming(),
            self.suppressed_task_deletes(),
            verbose,
            caldir.config().time_format(),
        )
    }

    fn render_push(&self, verbose: bool, caldir: &Caldir, provider: Option<&str>) -> String {
        render_outgoing(
            self.outgoing(),
            self.withheld_task_pushes(),
            verbose,
            caldir.config().time_format(),
            None,
            "pushed",
            "   No changes to push",
            provider,
        )
    }

    fn render_discard(&self, verbose: bool, caldir: &Caldir, provider: Option<&str>) -> String {
        render_outgoing(
            self.outgoing(),
            self.withheld_task_pushes(),
            verbose,
            caldir.config().time_format(),
            Some("Local changes (to discard)"),
            "discarded",
            "   Nothing to discard",
            provider,
        )
    }
}

/// The outgoing-only body, shared by `push` and `discard`.
///
/// Both surfaces act on the outgoing set alone, so both must carry the
/// withheld-task notice — and, like the version-skew belt on the incoming side, it
/// survives an otherwise-empty set: a calendar whose only pending change was
/// a task would otherwise read as "No changes to push".
#[allow(clippy::too_many_arguments)]
fn render_outgoing(
    outgoing: &[EventChange],
    withheld_task_pushes: usize,
    verbose: bool,
    time_format: TimeFormat,
    header: Option<&str>,
    verb: &str,
    empty: &str,
    provider: Option<&str>,
) -> String {
    let warning = withheld_warning(withheld_task_pushes, verb, provider);

    if outgoing.is_empty() {
        return match warning {
            Some(warning) => warning,
            None => empty.dimmed().to_string(),
        };
    }

    let mut lines = Vec::new();
    if let Some(header) = header {
        lines.push(format!("   {}:", header).dimmed().to_string());
    }
    render_diff_list(outgoing, verbose, time_format, &mut lines);
    lines.extend(warning);
    lines.join("\n")
}

/// The pull body. Only the *incoming* direction can lose a change to the
/// version-skew belt (the suppression sits on the incoming delete branch), so `pull` and the
/// bidirectional views carry the warning and `push`/`discard` do not — a
/// warning on a surface that shows only outgoing changes is noise.
fn render_incoming(
    incoming: &[EventChange],
    suppressed_task_deletes: bool,
    verbose: bool,
    time_format: TimeFormat,
) -> String {
    let warning = suppression_warning(suppressed_task_deletes);

    if incoming.is_empty() {
        return match warning {
            Some(warning) => warning,
            None => "   No changes to pull".dimmed().to_string(),
        };
    }

    let mut lines = Vec::new();
    render_diff_list(incoming, verbose, time_format, &mut lines);
    lines.extend(warning);
    lines.join("\n")
}

/// Render field-by-field differences for a change (only for updates).
///
/// One arm per kind. A cross-kind `Update` is impossible by construction —
/// identity carries the kind, so a VEVENT and a VTODO never pair — but it
/// is constructible in a test, and the answer there is a summary row, never a
/// panic.
fn render_field_diffs(diff: &EventChange, time_format: TimeFormat) -> Vec<String> {
    match diff {
        EventChange::Update {
            from: CalendarItem::Event(old),
            to: CalendarItem::Event(new),
        } => render_event_field_diffs(old, new, time_format),
        EventChange::Update {
            from: CalendarItem::Todo(old),
            to: CalendarItem::Todo(new),
        } => render_todo_field_diffs(old, new, time_format),
        EventChange::Update { from, to } => render_summary_diff(from.summary(), to.summary())
            .into_iter()
            .collect(),
        EventChange::Create(_) | EventChange::Delete(_) => Vec::new(),
    }
}

/// The `summary` row, shared by every kind: identical text to the row events
/// have always printed, including the `(Untitled)` fallback (C13).
fn render_summary_diff(old: Option<&str>, new: Option<&str>) -> Option<String> {
    (old != new).then(|| {
        format!(
            "{}: {} → {}",
            "summary".dimmed(),
            old.unwrap_or("(Untitled)").red(),
            new.unwrap_or("(Untitled)").green()
        )
    })
}

/// A task's field rows.
///
/// The stated minimum is two rows, `due` and the task status, and that is what
/// ships here alongside `summary`. `percent_complete`, `priority`, `completed`
/// and the shared string fields are an explicit non-goal for now, not an
/// oversight — a task update touching only those still renders its
/// header line and no rows.
fn render_todo_field_diffs(old: &Todo, new: &Todo, time_format: TimeFormat) -> Vec<String> {
    let mut lines = Vec::new();

    lines.extend(render_summary_diff(
        old.summary.as_deref(),
        new.summary.as_deref(),
    ));

    if old.due != new.due {
        lines.push(format!(
            "{}: {} → {}",
            "due".dimmed(),
            old.due
                .as_ref()
                .map_or(NO_DATE.into(), |d| format_datetime(d, time_format))
                .red(),
            new.due
                .as_ref()
                .map_or(NO_DATE.into(), |d| format_datetime(d, time_format))
                .green()
        ));
    }

    if old.status != new.status {
        lines.push(render_display("status", &old.status, &new.status));
    }

    lines
}

fn render_event_field_diffs(old: &Event, new: &Event, time_format: TimeFormat) -> Vec<String> {
    let mut lines = Vec::new();

    lines.extend(render_summary_diff(
        old.summary.as_deref(),
        new.summary.as_deref(),
    ));
    if old.description != new.description {
        lines.push(render_optional_diff(
            "description",
            &old.description,
            &new.description,
        ));
    }
    if old.location != new.location {
        lines.push(render_optional_diff(
            "location",
            &old.location,
            &new.location,
        ));
    }
    if old.start != new.start {
        lines.push(format!(
            "{}: {} → {}",
            "start".dimmed(),
            format_datetime(&old.start, time_format).red(),
            format_datetime(&new.start, time_format).green()
        ));
    }
    if old.end != new.end {
        lines.push(format!(
            "{}: {} → {}",
            "end".dimmed(),
            old.end
                .as_ref()
                .map_or("(none)".into(), |e| format_datetime(e, time_format))
                .red(),
            new.end
                .as_ref()
                .map_or("(none)".into(), |e| format_datetime(e, time_format))
                .green()
        ));
    }
    if old.status != new.status {
        lines.push(render_display("status", &old.status, &new.status));
    }
    if old.recurrence != new.recurrence {
        lines.extend(render_recurrence_diff(&old.recurrence, &new.recurrence));
    }
    if old.recurrence_id != new.recurrence_id {
        lines.push(format!(
            "{}: {:?} → {:?}",
            "recurrence_id".dimmed(),
            old.recurrence_id,
            new.recurrence_id
        ));
    }
    if old.reminders != new.reminders {
        let reminder_lines = render_reminder_diffs(&old.reminders, &new.reminders);
        if !reminder_lines.is_empty() {
            lines.push(format!("{}:", "reminders".dimmed()));
            lines.extend(reminder_lines.into_iter().map(|l| format!("  {}", l)));
        }
    }
    if old.availability != new.availability {
        lines.push(render_display(
            "availability",
            &old.availability,
            &new.availability,
        ));
    }
    if old.visibility != new.visibility {
        lines.push(render_optional_display(
            "visibility",
            &old.visibility,
            &new.visibility,
        ));
    }
    if old.organizer != new.organizer {
        lines.push(render_optional_display(
            "organizer",
            &old.organizer,
            &new.organizer,
        ));
    }
    if old.attendees != new.attendees {
        let attendee_lines = render_attendee_diffs(&old.attendees, &new.attendees);
        if !attendee_lines.is_empty() {
            lines.push(format!("{}:", "attendees".dimmed()));
            lines.extend(attendee_lines.into_iter().map(|l| format!("  {}", l)));
        }
    }
    if old.url != new.url {
        lines.push(render_optional_diff("url", &old.url, &new.url));
    }
    if old.attachments != new.attachments {
        let attachment_lines = render_attachment_diffs(&old.attachments, &new.attachments);
        if !attachment_lines.is_empty() {
            lines.push(format!("{}:", "attachments".dimmed()));
            lines.extend(attachment_lines.into_iter().map(|l| format!("  {}", l)));
        }
    }
    let xprop_lines = render_x_property_diffs(&old.x_properties, &new.x_properties);
    if !xprop_lines.is_empty() {
        lines.push(format!("{}:", "x-properties".dimmed()));
        lines.extend(xprop_lines.into_iter().map(|l| format!("  {}", l)));
    }

    lines
}

// Includes attributes on the property, not just the value
fn render_x_property_diffs(old: &[XProperty], new: &[XProperty]) -> Vec<String> {
    let mut lines = Vec::new();

    let old_by_name: HashMap<&str, &XProperty> = old.iter().map(|p| (p.name.as_str(), p)).collect();
    let new_by_name: HashMap<&str, &XProperty> = new.iter().map(|p| (p.name.as_str(), p)).collect();

    for (name, old_p) in &old_by_name {
        if let Some(new_p) = new_by_name.get(name)
            && old_p != new_p
        {
            lines.extend(render_x_property_change(name, old_p, new_p));
        }
    }

    for (name, new_p) in &new_by_name {
        if !old_by_name.contains_key(name) {
            lines.push(format!(
                "{} {} {}",
                "+".green(),
                name.green(),
                new_p.value.dimmed()
            ));
        }
    }

    for (name, old_p) in &old_by_name {
        if !new_by_name.contains_key(name) {
            lines.push(format!(
                "{} {} {}",
                "-".red(),
                name.red(),
                old_p.value.dimmed()
            ));
        }
    }

    lines
}

fn render_x_property_change(name: &str, old: &XProperty, new: &XProperty) -> Vec<String> {
    let mut lines = Vec::new();
    let param_lines = render_x_property_param_diffs(&old.params, &new.params);

    if old.value != new.value {
        lines.push(format!(
            "{}: {} → {}",
            name.dimmed(),
            old.value.red(),
            new.value.green()
        ));
    } else if !param_lines.is_empty() {
        lines.push(format!("{}:", name.dimmed()));
    }

    lines.extend(param_lines.into_iter().map(|l| format!("  {}", l)));
    lines
}

fn render_x_property_param_diffs(
    old: &[(String, String)],
    new: &[(String, String)],
) -> Vec<String> {
    let mut lines = Vec::new();

    let old_params: HashMap<&str, &str> =
        old.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let new_params: HashMap<&str, &str> =
        new.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();

    for (k, old_v) in &old_params {
        if let Some(new_v) = new_params.get(k)
            && new_v != old_v
        {
            lines.push(format!(
                "{}: {} → {}",
                k.dimmed(),
                old_v.red(),
                new_v.green()
            ));
        }
    }

    for (k, new_v) in &new_params {
        if !old_params.contains_key(k) {
            lines.push(format!("{} {}={}", "+".green(), k.green(), new_v.dimmed()));
        }
    }

    for (k, old_v) in &old_params {
        if !new_params.contains_key(k) {
            lines.push(format!("{} {}={}", "-".red(), k.red(), old_v.dimmed()));
        }
    }

    lines
}

/// Render an optional string field diff
fn render_optional_diff(field: &str, old: &Option<String>, new: &Option<String>) -> String {
    let old_str = old.as_deref().unwrap_or("(none)");
    let new_str = new.as_deref().unwrap_or("(none)");
    format!(
        "{}: {} → {}",
        field.dimmed(),
        old_str.red(),
        new_str.green()
    )
}

/// Render an Option<T: Display> field diff with red/green colors and a
/// `(none)` fallback. Used for structured fields with Display (organizer).
fn render_optional_display<T: fmt::Display>(
    field: &str,
    old: &Option<T>,
    new: &Option<T>,
) -> String {
    let old_str = old.as_ref().map_or("(none)".to_string(), |v| v.to_string());
    let new_str = new.as_ref().map_or("(none)".to_string(), |v| v.to_string());
    format!(
        "{}: {} → {}",
        field.dimmed(),
        old_str.red(),
        new_str.green()
    )
}

/// Render a `T: Display` field diff with red/green colors. Used for enum-like
/// fields with defaults (status, availability).
fn render_display<T: fmt::Display>(field: &str, old: &T, new: &T) -> String {
    format!(
        "{}: {} → {}",
        field.dimmed(),
        old.to_string().red(),
        new.to_string().green()
    )
}

/// Render reminder changes as add/remove lines (similar to attendee diffs).
fn render_reminder_diffs(old: &[Reminder], new: &[Reminder]) -> Vec<String> {
    let mut lines = Vec::new();

    let old_set: HashSet<i64> = old.iter().map(|r| r.minutes_before_start).collect();
    let new_set: HashSet<i64> = new.iter().map(|r| r.minutes_before_start).collect();

    for added in new
        .iter()
        .filter(|r| !old_set.contains(&r.minutes_before_start))
    {
        lines.push(format!("{} {}", "+".green(), added.to_string().green()));
    }

    for removed in old
        .iter()
        .filter(|r| !new_set.contains(&r.minutes_before_start))
    {
        lines.push(format!("{} {}", "-".red(), removed.to_string().red()));
    }

    lines
}

/// Render attendee changes, showing only what actually changed per attendee
fn render_attendee_diffs(old: &[Attendee], new: &[Attendee]) -> Vec<String> {
    let mut lines = Vec::new();

    let old_by_email: HashMap<String, &Attendee> =
        old.iter().map(|a| (a.email.to_lowercase(), a)).collect();

    let new_by_email: HashMap<String, &Attendee> =
        new.iter().map(|a| (a.email.to_lowercase(), a)).collect();

    // Attendees in both old and new — check for status changes
    for (email, old_att) in &old_by_email {
        if let Some(new_att) = new_by_email.get(email)
            && old_att.status != new_att.status
        {
            let label = attendee_label(new_att);
            let old_status = old_att
                .status
                .map_or("(none)".to_string(), |s| s.to_string());

            let new_status = new_att
                .status
                .map_or("(none)".to_string(), |s| s.to_string());

            lines.push(format!(
                "{}: {} → {}",
                label.dimmed(),
                old_status.red(),
                new_status.green()
            ));
        }
    }

    // Added attendees
    for (email, att) in &new_by_email {
        if !old_by_email.contains_key(email) {
            lines.push(format!("{} {}", "+".green(), attendee_label(att).green()));
        }
    }

    // Removed attendees
    for (email, att) in &old_by_email {
        if !new_by_email.contains_key(email) {
            lines.push(format!("{} {}", "-".red(), attendee_label(att).red()));
        }
    }

    lines
}

/// Format an attendee as "Name (email)" or just "email"
fn attendee_label(att: &Attendee) -> String {
    match &att.name {
        Some(name) if !name.is_empty() => format!("{} ({})", name, att.email),
        _ => att.email.clone(),
    }
}

/// Render attachment changes as add/remove lines, keyed by URI (the
/// attachment's identity), like reminder/attendee diffs.
fn render_attachment_diffs(old: &[Attachment], new: &[Attachment]) -> Vec<String> {
    let mut lines = Vec::new();

    let old_uris: HashSet<&str> = old.iter().map(|a| a.uri.as_str()).collect();
    let new_uris: HashSet<&str> = new.iter().map(|a| a.uri.as_str()).collect();

    for added in new.iter().filter(|a| !old_uris.contains(a.uri.as_str())) {
        lines.push(format!(
            "{} {}",
            "+".green(),
            attachment_label(added).green()
        ));
    }

    for removed in old.iter().filter(|a| !new_uris.contains(a.uri.as_str())) {
        lines.push(format!("{} {}", "-".red(), attachment_label(removed).red()));
    }

    lines
}

/// Label an attachment by its filename, falling back to its URI.
fn attachment_label(att: &Attachment) -> String {
    match att.filename() {
        Some(name) if !name.is_empty() => name.to_string(),
        _ => att.uri.clone(),
    }
}

/// Render recurrence diff showing RRULE and EXDATE changes
fn render_recurrence_diff(old: &Option<Recurrence>, new: &Option<Recurrence>) -> Vec<String> {
    let mut lines = Vec::new();

    match (old, new) {
        (Some(old_rec), Some(new_rec)) => {
            if old_rec.rrule != new_rec.rrule {
                lines.push(format!(
                    "{}: {} → {}",
                    "rrule".dimmed(),
                    old_rec.rrule.red(),
                    new_rec.rrule.green()
                ));
            }
            // Show exdate changes
            let old_set: HashSet<_> = old_rec
                .exdates
                .iter()
                .map(|e| format_datetime(e, TimeFormat::H24))
                .collect();

            let new_set: HashSet<_> = new_rec
                .exdates
                .iter()
                .map(|e| format_datetime(e, TimeFormat::H24))
                .collect();

            for ex in old_set.difference(&new_set) {
                lines.push(format!("{} exdate {}", "-".red(), ex.red()));
            }

            for ex in new_set.difference(&old_set) {
                lines.push(format!("{} exdate {}", "+".green(), ex.green()));
            }
        }
        (None, Some(new_rec)) => {
            lines.push(format!("{} rrule {}", "+".green(), new_rec.rrule.green()));
            for ex in &new_rec.exdates {
                lines.push(format!(
                    "{} exdate {}",
                    "+".green(),
                    format_datetime(ex, TimeFormat::H24).green()
                ));
            }
        }
        (Some(old_rec), None) => {
            lines.push(format!("{} rrule {}", "-".red(), old_rec.rrule.red()));
            for ex in &old_rec.exdates {
                lines.push(format!(
                    "{} exdate {}",
                    "-".red(),
                    format_datetime(ex, TimeFormat::H24).red()
                ));
            }
        }
        (None, None) => {}
    }

    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use caldir_core::{
        Availability, EventTime, ParticipationStatus, Status, TodoStatus, Visibility,
    };
    use chrono::NaiveDate;

    /// Floating datetimes in a fixed past year: the rendered date label is
    /// independent of the CI timezone *and* of the day the suite runs, so a
    /// byte-exact expectation stays byte-exact.
    fn floating(y: i32, m: u32, d: u32, h: u32, min: u32) -> EventTime {
        EventTime::DateTimeFloating(
            NaiveDate::from_ymd_opt(y, m, d)
                .unwrap()
                .and_hms_opt(h, min, 0)
                .unwrap(),
        )
    }

    fn recurring(rrule: &str, exdates: Vec<EventTime>) -> Recurrence {
        Recurrence {
            rrule: rrule.to_string(),
            exdates,
            rdates: Vec::new(),
        }
    }

    /// One recurring `Create`, one many-field `Update` and one summary-less
    /// `Delete`: every colour role, the 🔁 marker and the `(Untitled)`
    /// fallback in a single fixture.
    fn event_only_outgoing() -> Vec<EventChange> {
        let mut create = Event::new("Standup", floating(2019, 3, 5, 9, 0));
        create.recurrence = Some(recurring("FREQ=WEEKLY;BYDAY=MO", Vec::new()));

        let mut old = Event::new("Old title", floating(2019, 3, 5, 14, 30));
        old.description = Some("before".into());
        old.location = Some("Room A".into());
        old.end = Some(floating(2019, 3, 5, 15, 30));
        old.status = Status::Confirmed;
        old.availability = Availability::Busy;
        old.visibility = Some(Visibility::Public);
        old.recurrence = Some(recurring("FREQ=DAILY", vec![floating(2019, 3, 7, 14, 30)]));
        old.reminders = vec![Reminder::from_minutes(10)];
        old.url = Some("https://old.example".into());
        old.attendees = vec![Attendee::new("kept@example.com")];
        old.x_properties = vec![XProperty {
            name: "X-GONE".into(),
            value: "yes".into(),
            params: Vec::new(),
        }];

        let mut new = old.clone();
        new.summary = None;
        new.description = None;
        new.location = Some("Room B".into());
        new.start = floating(2019, 3, 5, 16, 0);
        new.end = None;
        new.status = Status::Cancelled;
        new.availability = Availability::Free;
        new.visibility = None;
        new.recurrence = Some(recurring("FREQ=WEEKLY", Vec::new()));
        new.reminders = vec![Reminder::from_minutes(30)];
        new.url = None;
        let mut attendee = Attendee::new("kept@example.com");
        attendee.status = Some(ParticipationStatus::Accepted);
        new.attendees = vec![attendee];
        new.x_properties = vec![XProperty {
            name: "X-NEW".into(),
            value: "1".into(),
            params: Vec::new(),
        }];

        vec![
            EventChange::Create(create.into()),
            EventChange::Update {
                from: old.into(),
                to: new.into(),
            },
        ]
    }

    fn event_only_incoming() -> Vec<EventChange> {
        let mut untitled = Event::new(
            "x",
            EventTime::Date(NaiveDate::from_ymd_opt(2019, 3, 6).unwrap()),
        );
        untitled.summary = None;

        vec![EventChange::Delete(untitled.into())]
    }

    /// Six changes, to drive the compact (non-verbose) counting branch.
    fn many_event_changes() -> Vec<EventChange> {
        (0..6)
            .map(|i| {
                let event = Event::new(format!("Event {}", i), floating(2019, 3, 5, 9, 0));
                match i % 3 {
                    0 => EventChange::Create(event.into()),
                    1 => EventChange::Delete(event.into()),
                    _ => EventChange::Update {
                        from: event.clone().into(),
                        to: event.into(),
                    },
                }
            })
            .collect()
    }

    fn bidirectional(
        outgoing: &[EventChange],
        incoming: &[EventChange],
        suppressed: bool,
        verbose: bool,
    ) -> String {
        render_sections(
            outgoing,
            incoming,
            Withheld {
                task_deletes: suppressed,
                task_pushes: 0,
                provider: None,
            },
            verbose,
            TimeFormat::H24,
            "Local changes (to push)",
            "Remote changes (to pull)",
        )
    }

    fn task(summary: &str) -> Todo {
        Todo::new(summary)
    }

    /// C13, the whole point of the ticket's second half: an event-only diff
    /// must render byte-identically to the pre-change renderer, escapes,
    /// glyphs, ordering and `(Untitled)` fallback included.
    ///
    /// The two expectations below were captured by *running HEAD's renderer*
    /// in a scratch crate before this file was touched — never reconstructed
    /// from the implementation they are supposed to police.
    #[test]
    fn an_event_only_diff_renders_byte_identically() {
        const VERBOSE: &str = concat!(
            "\u{1b}[2m   Local changes (to push):\u{1b}[0m\n",
            "   \u{1b}[32m+\u{1b}[39m \u{1b}[32mStandup\u{1b}[39m \u{1b}[2mTue Mar 5 2019, 09:00\u{1b}[0m 🔁\n",
            "   \u{1b}[33m~\u{1b}[39m \u{1b}[33m(Untitled)\u{1b}[39m \u{1b}[2mTue Mar 5 2019, 16:00\u{1b}[0m 🔁\n",
            "     \u{1b}[2msummary\u{1b}[0m: \u{1b}[31mOld title\u{1b}[39m → \u{1b}[32m(Untitled)\u{1b}[39m\n",
            "     \u{1b}[2mdescription\u{1b}[0m: \u{1b}[31mbefore\u{1b}[39m → \u{1b}[32m(none)\u{1b}[39m\n",
            "     \u{1b}[2mlocation\u{1b}[0m: \u{1b}[31mRoom A\u{1b}[39m → \u{1b}[32mRoom B\u{1b}[39m\n",
            "     \u{1b}[2mstart\u{1b}[0m: \u{1b}[31mTue Mar 5 2019, 14:30\u{1b}[39m → \u{1b}[32mTue Mar 5 2019, 16:00\u{1b}[39m\n",
            "     \u{1b}[2mend\u{1b}[0m: \u{1b}[31mTue Mar 5 2019, 15:30\u{1b}[39m → \u{1b}[32m(none)\u{1b}[39m\n",
            "     \u{1b}[2mstatus\u{1b}[0m: \u{1b}[31mconfirmed\u{1b}[39m → \u{1b}[32mcancelled\u{1b}[39m\n",
            "     \u{1b}[2mrrule\u{1b}[0m: \u{1b}[31mFREQ=DAILY\u{1b}[39m → \u{1b}[32mFREQ=WEEKLY\u{1b}[39m\n",
            "     \u{1b}[31m-\u{1b}[39m exdate \u{1b}[31mThu Mar 7 2019, 14:30\u{1b}[39m\n",
            "     \u{1b}[2mreminders\u{1b}[0m:\n",
            "       \u{1b}[32m+\u{1b}[39m \u{1b}[32m30m before start\u{1b}[39m\n",
            "       \u{1b}[31m-\u{1b}[39m \u{1b}[31m10m before start\u{1b}[39m\n",
            "     \u{1b}[2mavailability\u{1b}[0m: \u{1b}[31mbusy\u{1b}[39m → \u{1b}[32mfree\u{1b}[39m\n",
            "     \u{1b}[2mvisibility\u{1b}[0m: \u{1b}[31mpublic\u{1b}[39m → \u{1b}[32m(none)\u{1b}[39m\n",
            "     \u{1b}[2mattendees\u{1b}[0m:\n",
            "       \u{1b}[2mkept@example.com\u{1b}[0m: \u{1b}[31m(none)\u{1b}[39m → \u{1b}[32maccepted\u{1b}[39m\n",
            "     \u{1b}[2murl\u{1b}[0m: \u{1b}[31mhttps://old.example\u{1b}[39m → \u{1b}[32m(none)\u{1b}[39m\n",
            "     \u{1b}[2mx-properties\u{1b}[0m:\n",
            "       \u{1b}[32m+\u{1b}[39m \u{1b}[32mX-NEW\u{1b}[39m \u{1b}[2m1\u{1b}[0m\n",
            "       \u{1b}[31m-\u{1b}[39m \u{1b}[31mX-GONE\u{1b}[39m \u{1b}[2myes\u{1b}[0m\n",
            "\n",
            "\u{1b}[2m   Remote changes (to pull):\u{1b}[0m\n",
            "   \u{1b}[31m-\u{1b}[39m \u{1b}[31m(Untitled)\u{1b}[39m \u{1b}[2mWed Mar 6 2019, all-day\u{1b}[0m",
        );

        const COMPACT: &str = concat!(
            "\u{1b}[2m   Local changes (to push):\u{1b}[0m\n",
            "   \u{1b}[32m+\u{1b}[39m \u{1b}[32m(2 new events)\u{1b}[39m\n",
            "   \u{1b}[33m~\u{1b}[39m \u{1b}[33m(2 changed events)\u{1b}[39m\n",
            "   \u{1b}[31m-\u{1b}[39m \u{1b}[31m(2 deleted events)\u{1b}[39m",
        );

        assert_eq!(
            bidirectional(&event_only_outgoing(), &event_only_incoming(), false, true),
            VERBOSE
        );

        assert_eq!(
            bidirectional(&many_event_changes(), &[], false, false),
            COMPACT
        );
    }

    /// The hole L3-03 left behind: a task `Update` printed its header line and
    /// no field rows at all, so a real change was pushed against an
    /// under-described diff. `!is_empty()` is asserted first on purpose — the
    /// failure message must read "empty diff", not "expected `due` in ``".
    #[test]
    fn a_task_only_update_renders_a_non_empty_field_diff() {
        // Half 1: only `DUE` moved.
        let mut from = task("Draft Q3 report");
        from.due = Some(floating(2019, 3, 5, 17, 0));
        let mut to = from.clone();
        to.due = Some(floating(2019, 3, 6, 17, 0));

        let lines = render_field_diffs(
            &EventChange::Update {
                from: from.clone().into(),
                to: to.into(),
            },
            TimeFormat::H24,
        );

        assert!(
            !lines.is_empty(),
            "a task-only `due` change rendered an empty diff"
        );
        assert_eq!(
            lines,
            vec![concat!(
                "\u{1b}[2mdue\u{1b}[0m: ",
                "\u{1b}[31mTue Mar 5 2019, 17:00\u{1b}[39m → ",
                "\u{1b}[32mWed Mar 6 2019, 17:00\u{1b}[39m"
            )]
        );

        // Half 2: only the task status moved, NEEDS-ACTION → COMPLETED.
        let mut done = from.clone();
        done.status = TodoStatus::Completed;

        let lines = render_field_diffs(
            &EventChange::Update {
                from: from.into(),
                to: done.into(),
            },
            TimeFormat::H24,
        );

        assert!(
            !lines.is_empty(),
            "a task-only status change rendered an empty diff"
        );
        assert_eq!(
            lines,
            vec![concat!(
                "\u{1b}[2mstatus\u{1b}[0m: ",
                "\u{1b}[31mneeds_action\u{1b}[39m → ",
                "\u{1b}[32mcompleted\u{1b}[39m"
            )]
        );
    }

    /// A task summary change must name the field, exactly as an event's does.
    #[test]
    fn a_task_summary_change_gets_the_same_row_an_event_gets() {
        let from = task("Old task");
        let mut to = from.clone();
        to.summary = None;

        assert_eq!(
            render_field_diffs(
                &EventChange::Update {
                    from: from.into(),
                    to: to.into(),
                },
                TimeFormat::H24,
            ),
            vec![concat!(
                "\u{1b}[2msummary\u{1b}[0m: ",
                "\u{1b}[31mOld task\u{1b}[39m → ",
                "\u{1b}[32m(Untitled)\u{1b}[39m"
            )]
        );
    }

    /// A task `Create` uses the shared anchor: `DUE` fills the time column that
    /// L3-03 left blank.
    #[test]
    fn a_task_create_renders_its_due_date_in_the_time_column() {
        let mut todo = task("File taxes");
        todo.due = Some(floating(2019, 3, 5, 17, 0));

        assert_eq!(
            render_change(&EventChange::Create(todo.into()), TimeFormat::H24),
            concat!(
                "\u{1b}[32m+\u{1b}[39m \u{1b}[32mFile taxes\u{1b}[39m ",
                "\u{1b}[2mTue Mar 5 2019, 17:00\u{1b}[0m"
            )
        );
    }

    /// The diff has no footer to count undated tasks into, so an undated task
    /// about to be created must still occupy a line — `(no date)`, never an
    /// empty column and never a skipped row.
    ///
    /// Asserted at both seams on purpose: the one-line form alone leaves the
    /// list free to drop the anchor-less row on its way out, which is the
    /// silent-drop shape of the very bug this ticket closes.
    #[test]
    fn an_undated_task_renders_no_date_rather_than_an_empty_column() {
        const LINE: &str = concat!(
            "\u{1b}[32m+\u{1b}[39m ",
            "\u{1b}[32mSomeday: rewrite installer\u{1b}[39m ",
            "\u{1b}[2m(no date)\u{1b}[0m"
        );

        let change = EventChange::Create(task("Someday: rewrite installer").into());

        assert_eq!(render_change(&change, TimeFormat::H24), LINE);

        assert_eq!(
            bidirectional(&[change], &[], false, true),
            concat!(
                "\u{1b}[2m   Local changes (to push):\u{1b}[0m\n",
                "   \u{1b}[32m+\u{1b}[39m ",
                "\u{1b}[32mSomeday: rewrite installer\u{1b}[39m ",
                "\u{1b}[2m(no date)\u{1b}[0m"
            )
        );
    }

    /// AC 3: identity carries the kind, so the sync layer cannot build this —
    /// but a test can, and the renderer must answer with a row rather than a
    /// panic.
    #[test]
    fn a_cross_kind_update_renders_a_summary_row_rather_than_panicking() {
        let change = EventChange::Update {
            from: Event::new("Was a meeting", floating(2019, 3, 5, 9, 0)).into(),
            to: task("Now a task").into(),
        };

        assert_eq!(
            render_field_diffs(&change, TimeFormat::H24),
            vec![concat!(
                "\u{1b}[2msummary\u{1b}[0m: ",
                "\u{1b}[31mWas a meeting\u{1b}[39m → ",
                "\u{1b}[32mNow a task\u{1b}[39m"
            )]
        );
    }

    /// R49's belt withholds an *incoming* task delete. R50 says the price must
    /// be visible; MAJOR-2 says an invisible guard is the defect. This is the
    /// warning half of "suppress and warn".
    #[test]
    fn a_suppressed_task_delete_is_warned_about_beneath_the_changes() {
        let rendered = bidirectional(&[], &event_only_incoming(), true, true);

        assert_eq!(
            rendered,
            concat!(
                "\u{1b}[2m   Remote changes (to pull):\u{1b}[0m\n",
                "   \u{1b}[31m-\u{1b}[39m \u{1b}[31m(Untitled)\u{1b}[39m ",
                "\u{1b}[2mWed Mar 6 2019, all-day\u{1b}[0m\n",
                "   \u{1b}[33m⚠\u{1b}[39m task deletion withheld: the remote reported no ",
                "tasks at all, which is indistinguishable from a provider that cannot list ",
                "them. No task file was removed."
            )
        );
    }

    /// The trap ADR-10 flagged: the belt suppresses the only change there was,
    /// so `is_empty()` is true and the calendar would print "No changes" and
    /// nothing else — silent suppression, exactly what the accessor exists to
    /// prevent.
    #[test]
    fn a_suppressed_task_delete_is_not_swallowed_by_no_changes() {
        let rendered = bidirectional(&[], &[], true, false);

        assert_eq!(
            rendered,
            concat!(
                "   \u{1b}[33m⚠\u{1b}[39m task deletion withheld: the remote reported no ",
                "tasks at all, which is indistinguishable from a provider that cannot list ",
                "them. No task file was removed."
            )
        );
        assert!(!rendered.contains("No changes"));
    }

    /// An empty diff with the belt off is still "No changes", byte for byte.
    #[test]
    fn an_empty_diff_without_suppression_still_says_no_changes() {
        assert_eq!(
            bidirectional(&[], &[], false, false),
            "\u{1b}[2m   No changes\u{1b}[0m"
        );
    }

    /// `pull` is the other surface an incoming delete would have appeared on,
    /// and the one a user runs precisely to find out what the remote holds.
    #[test]
    fn a_suppressed_task_delete_is_warned_about_on_pull() {
        const WARNING: &str = concat!(
            "   \u{1b}[33m⚠\u{1b}[39m task deletion withheld: the remote reported no ",
            "tasks at all, which is indistinguishable from a provider that cannot list ",
            "them. No task file was removed."
        );

        assert_eq!(render_incoming(&[], true, false, TimeFormat::H24), WARNING);

        assert_eq!(
            render_incoming(&event_only_incoming(), true, true, TimeFormat::H24),
            concat!(
                "   \u{1b}[31m-\u{1b}[39m \u{1b}[31m(Untitled)\u{1b}[39m ",
                "\u{1b}[2mWed Mar 6 2019, all-day\u{1b}[0m\n",
                "   \u{1b}[33m⚠\u{1b}[39m task deletion withheld: the remote reported no ",
                "tasks at all, which is indistinguishable from a provider that cannot list ",
                "them. No task file was removed."
            )
        );
    }

    /// The belt's off-polarity at the render seam: without this the warning
    /// could be a constant and every pull would carry it.
    #[test]
    fn an_unsuppressed_pull_says_no_changes_to_pull() {
        assert_eq!(
            render_incoming(&[], false, false, TimeFormat::H24),
            "\u{1b}[2m   No changes to pull\u{1b}[0m"
        );
    }
    const WITHHELD_PUSH: &str = concat!(
        "   \u{1b}[33m⚠\u{1b}[39m 1 task change not pushed: ",
        "Provider \"webcal\" does not support task writes."
    );

    /// R70's withholding, on the surface the wedge happened on. The events go
    /// out and the task is named as kept back, in the same `⚠` affordance the
    /// R49 belt uses — one "caldir kept something back" line, not two. The
    /// old sentence must be gone entirely, not merely joined by the new one.
    #[test]
    fn a_withheld_task_push_names_the_provider_that_cannot_take_it() {
        let rendered = render_outgoing(
            &event_only_incoming(),
            1,
            true,
            TimeFormat::H24,
            None,
            "pushed",
            "   No changes to push",
            Some("webcal"),
        );

        assert_eq!(
            rendered,
            concat!(
                "   \u{1b}[31m-\u{1b}[39m \u{1b}[31m(Untitled)\u{1b}[39m ",
                "\u{1b}[2mWed Mar 6 2019, all-day\u{1b}[0m\n",
                "   \u{1b}[33m⚠\u{1b}[39m 1 task change not pushed: ",
                "Provider \"webcal\" does not support task writes."
            )
        );
        assert!(!rendered.contains("cannot push tasks to a remote yet"));
    }

    /// A **task-only** calendar: the withholding empties the outgoing set, so
    /// "No changes to push" would be the silent drop R19 forbids.
    #[test]
    fn a_task_only_calendar_is_not_swallowed_by_no_changes_to_push() {
        let rendered = render_outgoing(
            &[],
            1,
            false,
            TimeFormat::H24,
            None,
            "pushed",
            "   No changes to push",
            Some("webcal"),
        );
        assert_eq!(rendered, WITHHELD_PUSH);
        assert!(!rendered.contains("No changes to push"));
    }

    /// Off-polarity, both surfaces: without this the notice could be a
    /// constant and every push would wear it (R1/R2).
    #[test]
    fn an_event_only_push_and_discard_carry_no_withheld_notice() {
        assert_eq!(
            render_outgoing(
                &[],
                0,
                false,
                TimeFormat::H24,
                None,
                "pushed",
                "   No changes to push",
                Some("webcal"),
            ),
            "\u{1b}[2m   No changes to push\u{1b}[0m"
        );
        assert_eq!(
            render_outgoing(
                &[],
                0,
                false,
                TimeFormat::H24,
                Some("Local changes (to discard)"),
                "discarded",
                "   Nothing to discard",
                Some("webcal"),
            ),
            "\u{1b}[2m   Nothing to discard\u{1b}[0m"
        );
    }

    /// AC 3: a provider that advertises `tasks.write` never withholds a task
    /// push, so `withheld_task_pushes` is always `0` there — this is the
    /// count's off-polarity, at the trait-adjacent seam a caller actually
    /// uses. No provider name should ever be visible when nothing was kept
    /// back.
    #[test]
    fn a_provider_that_supports_task_writes_prints_no_withheld_notice() {
        let rendered = render_outgoing(
            &[],
            0,
            false,
            TimeFormat::H24,
            None,
            "pushed",
            "   No changes to push",
            None,
        );
        assert_eq!(rendered, "\u{1b}[2m   No changes to push\u{1b}[0m");
        assert!(!rendered.contains("Provider"));
    }

    /// `discard` reverts the outgoing set, so a withheld task is untouched
    /// there too — and the verb has to say so, not claim a push. A count of 2
    /// exercises pluralization alongside the provider name in the same line.
    #[test]
    fn discard_names_the_provider_and_the_task_it_did_not_discard() {
        assert_eq!(
            render_outgoing(
                &[],
                2,
                false,
                TimeFormat::H24,
                Some("Local changes (to discard)"),
                "discarded",
                "   Nothing to discard",
                Some("webcal"),
            ),
            concat!(
                "   \u{1b}[33m⚠\u{1b}[39m 2 task changes not discarded: ",
                "Provider \"webcal\" does not support task writes."
            )
        );
    }

    /// The bidirectional surface carries both belts, and R49's comes first:
    /// it is about files caldir refused to delete, which is the scarier one.
    /// Threading a provider name through `render_sections` is exactly the
    /// edit that could reorder or drop one of the two lines.
    #[test]
    fn both_withholding_notices_can_appear_together() {
        assert_eq!(
            render_sections(
                &[],
                &[],
                Withheld {
                    task_deletes: true,
                    task_pushes: 1,
                    provider: Some("webcal"),
                },
                false,
                TimeFormat::H24,
                "Local changes (to push)",
                "Remote changes (to pull)",
            ),
            concat!(
                "   \u{1b}[33m⚠\u{1b}[39m task deletion withheld: the remote reported no ",
                "tasks at all, which is indistinguishable from a provider that cannot list ",
                "them. No task file was removed.\n",
                "   \u{1b}[33m⚠\u{1b}[39m 1 task change not pushed: ",
                "Provider \"webcal\" does not support task writes."
            )
        );
    }
}
