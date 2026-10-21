use anyhow::{Result, bail};
use caldir_core::{Caldir, CalendarDiff, Connection, DateRange};
use owo_colors::OwoColorize;

use crate::output::diff::{CalendarDiffRender, Render};
use crate::utils::{connections, count_changes, resolve_sync_range, tui};

pub async fn run(
    caldir: &Caldir,
    calendar_slugs: Vec<String>,
    from: Option<String>,
    to: Option<String>,
    verbose: bool,
) -> Result<()> {
    let connections = connections(caldir, &calendar_slugs);
    let range = resolve_sync_range(from, to)?;
    let mut applied: Vec<CalendarDiff> = Vec::new();
    let mut failed = 0usize;
    let total = connections.len();

    for (i, connection) in connections.into_iter().enumerate() {
        match connection {
            Ok(mut connection) => {
                let pulled =
                    pull_connection(caldir, &mut connection, &range, verbose, &mut applied).await;

                if !pulled {
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

    let (created, updated, deleted) = count_changes(applied.iter().flat_map(|d| d.incoming()));

    if created > 0 || updated > 0 || deleted > 0 {
        println!(
            "\nPulled: {} created, {} updated, {} deleted",
            created, updated, deleted
        );
    }

    // `pull` has no outgoing half, so the unpushable-task pre-flight does not
    // apply to it — but the exit contract does: a failed pull must be visible
    // to a script, not just to a reader. The details are already on stderr.
    if failed > 0 {
        bail!("pull failed for {} calendar(s)", failed);
    }

    Ok(())
}

/// Failures go to stderr, so a pipeline capturing stdout still sees them and a
/// reader can tell them from the diff.
fn report(error: &impl std::fmt::Display) {
    eprintln!("   {}", error.to_string().red());
}

/// Whether this calendar was pulled. `false` means a failure was reported and
/// the caller must not exit 0.
async fn pull_connection(
    caldir: &Caldir,
    connection: &mut Connection,
    range: &DateRange,
    verbose: bool,
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

    println!("{}", diff.render_pull(verbose, caldir));

    match connection.apply_incoming_diff(&diff) {
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
