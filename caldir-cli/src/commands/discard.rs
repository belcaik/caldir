use anyhow::{Result, bail};
use caldir_core::{Caldir, CalendarDiff, Connection, EventChange};
use dialoguer::Confirm;
use owo_colors::OwoColorize;

use crate::output::diff::{CalendarDiffRender, Render};
use crate::utils::{resolve_sync_range, tui};

pub async fn run(
    caldir: &Caldir,
    calendar: Option<String>,
    from: Option<String>,
    to: Option<String>,
    verbose: bool,
    force: bool,
) -> Result<()> {
    let all_connections = caldir.connections();

    let connections = match calendar {
        Some(cal) => all_connections
            .into_iter()
            .filter(|conn| conn.as_ref().ok().and_then(|c| c.local().slug()) == Some(cal.as_str()))
            .collect(),
        None => all_connections,
    };

    let range = resolve_sync_range(from, to)?;
    let mut pending: Vec<(Connection, CalendarDiff)> = Vec::new();
    let mut failed = 0usize;
    let total = connections.len();

    for (i, connection) in connections.into_iter().enumerate() {
        match connection {
            Ok(mut connection) => {
                let header = connection.local().render(caldir);
                let spinner = tui::create_spinner(header.clone());
                let result = connection.diff(&range).await;
                spinner.finish_and_clear();

                println!("{}", header);

                match result {
                    Ok(diff) => {
                        // No unpushable-task pre-flight here: `discard` only
                        // ever writes to the local calendar, and dropping the
                        // task is one of the two remedies the push refusal
                        // offers. Refusing it would strand the user.
                        println!("{}", diff.render_discard(verbose, caldir));
                        if !diff.outgoing().is_empty() {
                            pending.push((connection, diff));
                        }
                    }
                    Err(e) => {
                        report(&e);
                        failed += 1;
                    }
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

    let total_changes: usize = pending.iter().map(|(_, d)| d.outgoing().len()).sum();

    if total_changes == 0 {
        return finish(failed);
    }

    if !force {
        println!();
        let confirmed = Confirm::new()
            .with_prompt(format!(
                "Discard {} {}?",
                total_changes,
                if total_changes == 1 {
                    "change"
                } else {
                    "changes"
                }
            ))
            .default(false)
            .interact()?;

        if !confirmed {
            return finish(failed);
        }
    }

    for (connection, diff) in &pending {
        connection.discard_outgoing_diff(diff)?;
    }

    let (created, updated, deleted) = pending.iter().fold((0, 0, 0), |(c, u, d), (_, diff)| {
        diff.outgoing()
            .iter()
            .fold((c, u, d), |(c, u, d), change| match change {
                EventChange::Create(_) => (c + 1, u, d),
                EventChange::Update { .. } => (c, u + 1, d),
                EventChange::Delete(_) => (c, u, d + 1),
            })
    });

    println!(
        "\nDiscarded {} {}: {} created, {} updated, {} deleted",
        total_changes,
        if total_changes == 1 {
            "change"
        } else {
            "changes"
        },
        created,
        updated,
        deleted
    );

    finish(failed)
}

/// Failures go to stderr, so a pipeline capturing stdout still sees them and a
/// reader can tell them from the diff.
fn report(error: &impl std::fmt::Display) {
    eprintln!("   {}", error.to_string().red());
}

/// A calendar that could not be inspected had its changes left in place, which
/// a script must be able to detect: the details are already on stderr, so this
/// only carries the non-zero exit `main` turns it into.
fn finish(failed: usize) -> Result<()> {
    if failed > 0 {
        bail!("discard failed for {} calendar(s)", failed);
    }

    Ok(())
}
