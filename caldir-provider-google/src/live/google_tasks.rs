//! The Google Tasks live tier: two gates against a real Google account.
//!
//! # How to run them
//!
//! Gated at **runtime**, never with `#[ignore]` — the mirror of
//! `caldir-provider-caldav/src/live/radicale.rs`'s `live_enabled`/`live_gate`
//! shape, for the same reason its doc comment gives: the index's verify
//! command does not pass `--ignored`, so an ignored test would silently
//! never run. With the gate unset, `cargo test --workspace` runs these as
//! no-ops and stays green with no Google account reachable.
//!
//! Unlike Radicale, there is no throwaway container to stand up: Google
//! Tasks needs a real OAuth session, which means an account that has
//! already run `caldir-provider-google connect` (hosted or self-hosted) and
//! a real task list id to point at. Three environment variables carry that
//! in:
//!
//! ```text
//! CALDIR_LIVE_GOOGLE=1                                   # the opt-in gate
//! CALDIR_LIVE_GOOGLE_ACCOUNT=you@gmail.com                # a connected account
//! CALDIR_LIVE_GOOGLE_TASK_LIST=<task list id>             # a real, disposable list
//! CALDIR_LIVE_GOOGLE=1 CALDIR_LIVE_GOOGLE_ACCOUNT=you@gmail.com \
//!   CALDIR_LIVE_GOOGLE_TASK_LIST=<id> \
//!   cargo test -p caldir-provider-google -- live::google_tasks
//! ```
//!
//! Each test titles its task uniquely (a timestamp suffix) and deletes it
//! itself, on both the success and the failure path — a run that panics
//! between create and delete would otherwise leave a duplicate sitting in
//! the account for the next run to trip over, silently invalidating the
//! next gate's cardinality assertion.
//!
//! **Necessary conditions vs. the proof:** `google_tasks::tests` (the
//! non-live unit tests) checks each of `P5-01`/`P5-02`/`P5-03`'s seams in
//! isolation — necessary conditions, not proof that a real second sync
//! against Google cannot duplicate a task. Nothing in this crate can drive
//! `caldir-core`'s actual sync loop (`Connection::diff`, `apply_outgoing_diff`
//! — its mocks are `#[cfg(test)]`/`pub(crate)` and unreachable from a
//! provider crate), so "sync" here is modeled at the provider boundary: the
//! *create* seam for a cycle with no known remote id, the *update* seam for
//! every cycle after — because a correct sync only ever calls create once
//! per item, before an id exists (`P2-02` owns that decision, not this
//! crate). `create_sync_sync_yields_exactly_one_remote_task` is the proof
//! that routing the same canonical item through create once and update
//! any number of times after never produces a second remote task, live
//! against Google — the one thing no pure function can observe, since the
//! duplicate this ticket exists to catch is created by what a *second* call
//! decides, not by anything the first call returns.

use anyhow::{Context, Result, bail, ensure};
use caldir_core::provider::ProviderStorage;
use caldir_core::rpc::{CreateItem, DeleteItem, UpdateItem};
use caldir_core::{CalendarItem, EventTime, Todo};

use crate::app_config::AppConfigStore;
use crate::commands;
use crate::constants::{PROVIDER_NAME, PROVIDER_TASK_ID_PROPERTY};
use crate::google_tasks::{self, todo_x_property};
use crate::remote_config::GoogleTaskListRemoteConfig;
use crate::session::SessionStore;

// ============================================================================
// Gating
// ============================================================================

const GATE_VAR: &str = "CALDIR_LIVE_GOOGLE";
const ACCOUNT_VAR: &str = "CALDIR_LIVE_GOOGLE_ACCOUNT";
const TASK_LIST_VAR: &str = "CALDIR_LIVE_GOOGLE_TASK_LIST";

struct LiveConfig {
    account_email: String,
    task_list_id: String,
}

/// Is the live tier opted into at all? Separate from [`live_gate`] so a
/// missing-but-required account/task-list variable, once opted in, is a
/// hard failure rather than a silent skip.
fn live_enabled() -> bool {
    std::env::var(GATE_VAR).as_deref() == Ok("1")
}

/// Start a config, or print why this test is a no-op and yield `None`.
fn live_gate(test: &str) -> Option<LiveConfig> {
    if !live_enabled() {
        println!(
            "SKIP live::google_tasks::{test}: set {GATE_VAR}=1, {ACCOUNT_VAR} and \
             {TASK_LIST_VAR} to run the live tier (see this module's doc comment)"
        );
        return None;
    }

    let account_email = std::env::var(ACCOUNT_VAR).unwrap_or_else(|_| {
        panic!("{GATE_VAR}=1 requires {ACCOUNT_VAR} (a connected Google account)")
    });
    let task_list_id = std::env::var(TASK_LIST_VAR).unwrap_or_else(|_| {
        panic!("{GATE_VAR}=1 requires {TASK_LIST_VAR} (a real, disposable task list id)")
    });

    Some(LiveConfig {
        account_email,
        task_list_id,
    })
}

// ============================================================================
// Harness
// ============================================================================

fn unique_title(label: &str) -> String {
    let now = chrono::Utc::now();
    format!(
        "caldir-live-google-tasks {label} {}",
        now.timestamp_nanos_opt().unwrap_or_default()
    )
}

fn remote_config(config: &LiveConfig) -> caldir_core::RemoteConfigParams {
    GoogleTaskListRemoteConfig::new(&config.account_email, &config.task_list_id)
        .into_remote_config_params()
}

async fn access_token(config: &LiveConfig) -> Result<String> {
    let storage = ProviderStorage::for_provider(PROVIDER_NAME)?;
    let session_store = SessionStore::new(storage.clone());
    let app_config_store = AppConfigStore::new(storage);

    let session = session_store
        .load_valid(&config.account_email, &app_config_store)
        .await
        .context("loading a session for the live tier — did you run `connect`?")?;

    Ok(session.access_token().to_string())
}

/// Every non-deleted remote task with an exact title match — the harness's
/// own read path, deliberately going through `google_tasks::list_tasks`
/// directly rather than the provider's `list_events` RPC boundary, so the
/// cardinality check does not depend on the same canonicalization this
/// ticket is proving.
async fn matching_tasks(
    config: &LiveConfig,
    access_token: &str,
    title: &str,
) -> Result<Vec<google_tasks::Task>> {
    let tasks = google_tasks::list_tasks(access_token, &config.task_list_id).await?;
    Ok(tasks
        .into_iter()
        .filter(|t| !t.deleted && t.title == title)
        .collect())
}

fn require_task_id(todo: &Todo) -> Result<String> {
    todo_x_property(todo, PROVIDER_TASK_ID_PROPERTY)
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("canonical Todo carries no {PROVIDER_TASK_ID_PROPERTY}"))
}

fn expect_todo(item: CalendarItem) -> Result<Todo> {
    match item {
        CalendarItem::Todo(todo) => Ok(todo),
        other => bail!("expected a Todo back from the provider, got {other:?}"),
    }
}

/// Best-effort delete by id, tolerant of the task already being gone
/// (`delete_event`'s own classifier already treats a 404 as success) —
/// called on both the success and the failure path so a panicking
/// assertion never leaves a duplicate sitting in the account.
async fn cleanup(config: &LiveConfig, task_id: &str) {
    let mut todo = Todo::new("cleanup");
    todo.x_properties.push(caldir_core::XProperty::new(
        PROVIDER_TASK_ID_PROPERTY,
        task_id,
    ));

    let outcome = commands::delete_event::handle(DeleteItem {
        remote: remote_config(config),
        event: CalendarItem::Todo(todo),
    })
    .await;

    if let Err(error) = outcome {
        eprintln!("live::google_tasks: cleanup failed for task {task_id}: {error:#}");
    }
}

/// Carries the id as soon as it is known, independent of whether the rest
/// of the lifecycle passed — the reason [`cleanup`] always has something to
/// act on.
struct Outcome {
    task_id: Option<String>,
    result: Result<()>,
}

// ============================================================================
// The proof
// ============================================================================

/// **The proof.** See the module doc comment for what "sync" means at this
/// boundary and why a live test, not a `MockProvider` sequence, is the only
/// place this is observable.
#[tokio::test]
#[serial_test::serial(google_tasks)]
async fn create_sync_sync_yields_exactly_one_remote_task() {
    let Some(config) = live_gate("create_sync_sync_yields_exactly_one_remote_task") else {
        return;
    };

    let outcome = run_lifecycle(&config).await;

    if let Some(task_id) = &outcome.task_id {
        cleanup(&config, task_id).await;
    }

    outcome.result.expect("the create/update/delete lifecycle");
}

async fn run_lifecycle(config: &LiveConfig) -> Outcome {
    let mut task_id = None;
    let result = run_lifecycle_inner(config, &mut task_id).await;
    Outcome { task_id, result }
}

async fn run_lifecycle_inner(config: &LiveConfig, task_id_out: &mut Option<String>) -> Result<()> {
    let title = unique_title("create-sync-sync");
    let local = Todo::new(title.clone());

    // Cycle 1 ("sync #1"): no known remote id — the create seam.
    let created = commands::create_event::handle(CreateItem {
        remote: remote_config(config),
        event: CalendarItem::Todo(local),
    })
    .await
    .context("create")?;
    let canonical_1 = expect_todo(created)?;
    let id_1 = require_task_id(&canonical_1)?;
    *task_id_out = Some(id_1.clone());

    let token = access_token(config).await?;
    let after_create = matching_tasks(config, &token, &title).await?;
    ensure!(
        after_create.len() == 1,
        "expected exactly one remote task titled {title:?} after create, found {}",
        after_create.len()
    );
    ensure!(
        after_create[0].id == id_1,
        "the listed task must carry the id create returned"
    );

    // Cycle 2 ("sync #2"): a known id always resolves through update, never
    // a second insert. This is the assertion the whole ticket exists for —
    // exercised live, against the second call that could actually create a
    // duplicate.
    let resynced = commands::update_event::handle(UpdateItem {
        remote: remote_config(config),
        event: CalendarItem::Todo(canonical_1.clone()),
    })
    .await
    .context("update (second sync, unchanged content)")?;
    let canonical_2 = expect_todo(resynced)?;
    let id_2 = require_task_id(&canonical_2)?;
    ensure!(
        id_2 == id_1,
        "the second cycle must resolve to the id the first cycle minted"
    );

    let after_second_sync = matching_tasks(config, &token, &title).await?;
    ensure!(
        after_second_sync.len() == 1,
        "a second cycle must not create a duplicate remote task; found {}",
        after_second_sync.len()
    );
    ensure!(after_second_sync[0].id == id_1);

    // Criterion 5, first half: update -> sync -> sync leaves one task with
    // the new content, same id.
    let edited_title = format!("{title} (edited)");
    let mut edited = canonical_2.clone();
    edited.summary = Some(edited_title.clone());

    let updated = commands::update_event::handle(UpdateItem {
        remote: remote_config(config),
        event: CalendarItem::Todo(edited),
    })
    .await
    .context("update (content change)")?;
    let canonical_3 = expect_todo(updated)?;
    ensure!(require_task_id(&canonical_3)? == id_1);
    ensure!(canonical_3.summary.as_deref() == Some(edited_title.as_str()));

    let resynced_again = commands::update_event::handle(UpdateItem {
        remote: remote_config(config),
        event: CalendarItem::Todo(canonical_3.clone()),
    })
    .await
    .context("update (second sync after the content change)")?;
    let canonical_4 = expect_todo(resynced_again)?;
    ensure!(
        canonical_4 == canonical_3,
        "a no-op second sync must report no changes"
    );

    let after_edit = matching_tasks(config, &token, &edited_title).await?;
    ensure!(
        after_edit.len() == 1,
        "expected exactly one task after the edit, found {}",
        after_edit.len()
    );

    // Criterion 5, second half: delete -> sync leaves none.
    commands::delete_event::handle(DeleteItem {
        remote: remote_config(config),
        event: CalendarItem::Todo(canonical_4),
    })
    .await
    .context("delete")?;

    let after_delete = matching_tasks(config, &token, &edited_title).await?;
    ensure!(
        after_delete.is_empty(),
        "expected no remaining task after delete, found {}",
        after_delete.len()
    );

    Ok(())
}

// ============================================================================
// GOOGLE-10 / P5-05 criterion 7
// ============================================================================

/// Closes `P5-05`'s deferred acceptance criterion 7 and `GOOGLE-10`.
/// `P5-05` proved `google_due_date`/`date_to_due` are a fixpoint
/// **in-process**; its own test comment states plainly that this cannot
/// show Google's own truncation agrees. Pushing `DUE:20260902T035959Z`
/// through a real create and a real second sync, and comparing the two
/// canonical `Todo`s with `Todo`'s own `PartialEq` (the same comparison the
/// diff engine uses), is what closes that gap.
#[tokio::test]
#[serial_test::serial(google_tasks)]
async fn create_sync_sync_reports_no_changes_for_a_timestamped_due() {
    let Some(config) = live_gate("create_sync_sync_reports_no_changes_for_a_timestamped_due")
    else {
        return;
    };

    let outcome = run_due_fixpoint(&config).await;

    if let Some(task_id) = &outcome.task_id {
        cleanup(&config, task_id).await;
    }

    outcome
        .result
        .expect("the due fixpoint across two live cycles");
}

async fn run_due_fixpoint(config: &LiveConfig) -> Outcome {
    let mut task_id = None;
    let result = run_due_fixpoint_inner(config, &mut task_id).await;
    Outcome { task_id, result }
}

async fn run_due_fixpoint_inner(
    config: &LiveConfig,
    task_id_out: &mut Option<String>,
) -> Result<()> {
    let title = unique_title("due-fixpoint");
    let mut local = Todo::new(title.clone());
    local.due = Some(EventTime::DateTimeUtc(
        "2026-09-02T03:59:59Z".parse().context("fixture DUE")?,
    ));

    let created = commands::create_event::handle(CreateItem {
        remote: remote_config(config),
        event: CalendarItem::Todo(local),
    })
    .await
    .context("create")?;
    let canonical_1 = expect_todo(created)?;
    *task_id_out = Some(require_task_id(&canonical_1)?);

    let resynced = commands::update_event::handle(UpdateItem {
        remote: remote_config(config),
        event: CalendarItem::Todo(canonical_1.clone()),
    })
    .await
    .context("update (second sync, unchanged content)")?;
    let canonical_2 = expect_todo(resynced)?;

    ensure!(
        canonical_2 == canonical_1,
        "a second sync of an unchanged timestamped DUE must report no changes; \
         first cycle due={:?}, second cycle due={:?}",
        canonical_1.due,
        canonical_2.due
    );

    Ok(())
}
