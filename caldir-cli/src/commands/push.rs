use anyhow::{Result, bail};
use caldir_core::{Caldir, CalendarDiff, Connection, DateRange};
use owo_colors::OwoColorize;

use crate::output::diff::{CalendarDiffRender, Render};
use crate::utils::{allow_mass_delete, connections, count_changes, resolve_sync_range, tui};

pub async fn run(
    caldir: &Caldir,
    calendar: Option<String>,
    from: Option<String>,
    to: Option<String>,
    verbose: bool,
    force: bool,
) -> Result<()> {
    let calendar_slugs: Vec<String> = calendar.into_iter().collect();
    let connections = connections(caldir, &calendar_slugs);
    let range = resolve_sync_range(from, to)?;
    let mut applied: Vec<CalendarDiff> = Vec::new();
    let mut failed = 0usize;
    let total = connections.len();

    for (i, connection) in connections.into_iter().enumerate() {
        match connection {
            Ok(mut connection) => {
                let pushed = push_connection(
                    caldir,
                    &mut connection,
                    &range,
                    verbose,
                    force,
                    &mut applied,
                )
                .await;

                if !pushed {
                    failed += 1;
                }
            }
            Err(e) => {
                report(&e);
                failed += 1;
            }
        }

        if i < total - 1 {
            println!();
        }
    }

    let (created, updated, deleted) = count_changes(applied.iter().flat_map(|d| d.outgoing()));

    if created > 0 || updated > 0 || deleted > 0 {
        println!(
            "\nPushed: {} created, {} updated, {} deleted",
            created, updated, deleted
        );
    }

    // A failed push must be visible to a script, not just to a reader: the
    // details are already on stderr above, so this only carries the non-zero
    // exit `main` turns it into.
    if failed > 0 {
        bail!("push failed for {} calendar(s)", failed);
    }

    Ok(())
}

/// Failures go to stderr, so a pipeline capturing stdout still sees them and a
/// reader can tell them from the diff.
fn report(error: &impl std::fmt::Display) {
    eprintln!("   {}", error.to_string().red());
}

/// Whether this calendar was pushed. `false` means a failure was reported and
/// the caller must not exit 0.
async fn push_connection(
    caldir: &Caldir,
    connection: &mut Connection,
    range: &DateRange,
    verbose: bool,
    force: bool,
    applied: &mut Vec<CalendarDiff>,
) -> bool {
    let header = connection.local().render(caldir);
    let spinner = tui::create_spinner(header.clone());
    let result = connection.diff(range).await;
    spinner.finish_and_clear();

    println!("{}", header);

    let diff = match result {
        Ok(diff) => diff,
        Err(e) => {
            report(&e);
            return false;
        }
    };

    // Asked *before* the diff is rendered. A refused push applies nothing, so
    // painting its items with the green `+` that means "pushed" would describe
    // work that never happens — the failure then reads as a success followed
    // by a confusing error.
    if let Err(e) = connection.check_outgoing(&diff) {
        report(&e);
        return false;
    }

    println!("{}", diff.render_push(verbose, caldir));

    if !allow_mass_delete(&diff, force) {
        return true;
    }

    match connection.apply_outgoing_diff(&diff).await {
        Ok(()) => {
            applied.push(diff);
            true
        }
        Err(e) => {
            report(&e);
            false
        }
    }
}
