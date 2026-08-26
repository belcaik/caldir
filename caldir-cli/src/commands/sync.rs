use anyhow::{Result, bail};
use caldir_core::{Caldir, Connection, DateRange};
use owo_colors::OwoColorize;

use crate::output::diff::{CalendarDiffRender, Render};
use crate::utils::{allow_mass_delete, connections, count_changes, resolve_sync_range, tui};

type Counts = (usize, usize, usize);

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
    let mut pulled: Counts = (0, 0, 0);
    let mut pushed: Counts = (0, 0, 0);
    let mut failed = 0usize;
    let total = connections.len();

    for (i, connection) in connections.into_iter().enumerate() {
        match connection {
            Ok(mut connection) => {
                let synced = sync_connection(
                    caldir,
                    &mut connection,
                    &range,
                    verbose,
                    force,
                    &mut pulled,
                    &mut pushed,
                )
                .await;

                if !synced {
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

    if pulled != (0, 0, 0) || pushed != (0, 0, 0) {
        println!();
    }

    if pulled != (0, 0, 0) {
        println!(
            "Pulled: {} created, {} updated, {} deleted",
            pulled.0, pulled.1, pulled.2
        );
    }

    if pushed != (0, 0, 0) {
        println!(
            "Pushed: {} created, {} updated, {} deleted",
            pushed.0, pushed.1, pushed.2
        );
    }

    // A failed sync must be visible to a script, not just to a reader: the
    // details are already on stderr above, so this only carries the non-zero
    // exit `main` turns it into.
    if failed > 0 {
        bail!("sync failed for {} calendar(s)", failed);
    }

    Ok(())
}

/// Failures go to stderr, so a pipeline capturing stdout still sees them and a
/// reader can tell them from the diff.
fn report(error: &impl std::fmt::Display) {
    eprintln!("   {}", error.to_string().red());
}

/// Whether this calendar was synced. `false` means a failure was reported and
/// the caller must not exit 0.
#[allow(clippy::too_many_arguments)]
async fn sync_connection(
    caldir: &Caldir,
    connection: &mut Connection,
    range: &DateRange,
    verbose: bool,
    force: bool,
    pulled: &mut Counts,
    pushed: &mut Counts,
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

    println!("{}", diff.render(verbose, caldir));

    let mut ok = true;

    match connection.apply_incoming_diff(&diff) {
        Ok(()) => add_counts(pulled, count_changes(diff.incoming())),
        Err(e) => {
            report(&e);
            ok = false;
        }
    }

    // Guarded rather than returned early: a sync has two halves, so a failed
    // pull must still reach the caller even when the push half is skipped.
    if allow_mass_delete(&diff, force) {
        match connection.apply_outgoing_diff(&diff).await {
            Ok(()) => add_counts(pushed, count_changes(diff.outgoing())),
            Err(e) => {
                report(&e);
                ok = false;
            }
        }
    }

    ok
}

fn add_counts(acc: &mut Counts, delta: Counts) {
    acc.0 += delta.0;
    acc.1 += delta.1;
    acc.2 += delta.2;
}
