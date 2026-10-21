//! End-to-end locks on what `caldir sync`, `caldir discard` and `caldir pull`
//! tell a *script*.
//!
//! Sibling of `push_exit_status.rs`, and here for the same reason: the exit
//! code and the choice of stream do not exist below the process boundary, so
//! they can only be locked by running the real binary against a real caldir
//! and a stub provider on `PATH`.
//!
//! Why they exist: the fix that gave `caldir push` a truthful exit code was
//! scoped to `push` alone, and `sync`/`discard`/`pull` carried the identical
//! defect — the failure printed to **stdout**, the process exited **0**, and
//! (for `sync`) the refused task was painted with the green `+` that means
//! "pushed". `sync` is the command most users actually run.

use chrono::{Duration, Utc};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Fixture timestamps are anchored to *today*, never to a literal date.
///
/// `sync`/`pull` use the CLI's default ±365-day window, so a fixture pinned to
/// a literal 2026 date silently falls out of that window in 2027 — and a test
/// that passes because its subject is out of range is a green tick over zero
/// coverage, not a passing test. Anchoring to `now` makes every fixture below
/// permanently in-window.
fn day_from_now(days: i64) -> String {
    (Utc::now() + Duration::days(days))
        .format("%Y%m%d")
        .to_string()
}

fn stamp() -> String {
    Utc::now().format("%Y%m%dT%H%M%SZ").to_string()
}

/// Answers `list_events` with an empty remote (so every local file is an
/// outgoing create) and echoes back whatever `create_event` is handed.
const STUB_PROVIDER: &str = r#"#!/bin/sh
request=$(cat)
case "$request" in
  *'"list_events"'*)
    echo '{"status":"success","data":[]}'
    ;;
  *)
    # The wire form of an event is a JSON string of ICS; echo it straight back.
    event=${request#*'"event":'}
    event=${event%\}\}}
    printf '{"status":"success","data":%s}\n' "$event"
    ;;
esac
"#;

/// Fails every write verb while still listing an empty remote, so the failure
/// lands in `apply_outgoing_diff` rather than in the pre-flight.
const FAILING_PROVIDER: &str = r#"#!/bin/sh
request=$(cat)
case "$request" in
  *'"list_events"'*) echo '{"status":"success","data":[]}' ;;
  *) echo '{"status":"error","error":"the server said no"}' ;;
esac
"#;

/// Lists one remote event that is not on disk, so there is always an incoming
/// create to apply locally, and still echoes back any write it is handed.
fn serving_provider() -> String {
    format!(
        r#"#!/bin/sh
request=$(cat)
case "$request" in
  *'"list_events"'*)
    printf '{{"status":"success","data":["BEGIN:VCALENDAR\\r\\nVERSION:2.0\\r\\nPRODID:-//caldir-test//EN\\r\\nBEGIN:VEVENT\\r\\nUID:from-remote@test\\r\\nDTSTAMP:{stamp}\\r\\nDTSTART:{day}T090000Z\\r\\nDTEND:{day}T100000Z\\r\\nSUMMARY:Remote seminar\\r\\nEND:VEVENT\\r\\nEND:VCALENDAR\\r\\n"]}}\n'
    ;;
  *)
    event=${{request#*'"event":'}}
    event=${{event%\}}\}}}}
    printf '{{"status":"success","data":%s}}\n' "$event"
    ;;
esac
"#,
        stamp = stamp(),
        day = day_from_now(8),
    )
}

/// Lists `MASS_DELETE_THRESHOLD` remote events and logs every RPC method it is
/// asked for to `$HOME/rpc.log`, so a test can assert what did *not* happen.
fn logging_provider() -> String {
    format!(
        r#"#!/bin/sh
request=$(cat)
echo "$request" >> "$HOME/rpc.log"
case "$request" in
  *'"list_events"'*)
    events=""
    i=1
    while [ $i -le 10 ]; do
      events="$events${{events:+,}}\"BEGIN:VCALENDAR\\r\\nVERSION:2.0\\r\\nPRODID:-//caldir-test//EN\\r\\nBEGIN:VEVENT\\r\\nUID:doomed-$i@test\\r\\nDTSTAMP:{stamp}\\r\\nDTSTART:{day}T090000Z\\r\\nDTEND:{day}T100000Z\\r\\nSUMMARY:Doomed $i\\r\\nEND:VEVENT\\r\\nEND:VCALENDAR\\r\\n\""
      i=$((i + 1))
    done
    printf '{{"status":"success","data":[%s]}}\n' "$events"
    ;;
  *)
    echo '{{"status":"success","data":null}}'
    ;;
esac
"#,
        stamp = stamp(),
        day = day_from_now(8),
    )
}

/// Cannot even describe the remote, so `Connection::diff` fails and no diff
/// ever exists — the path that used to `println!` and fall through to exit 0.
const UNREADABLE_PROVIDER: &str = r#"#!/bin/sh
cat > /dev/null
echo '{"status":"error","error":"the remote is unreachable"}'
"#;

fn event_ics(slug: &str) -> String {
    let stamp = stamp();
    let day = day_from_now(7);
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//caldir-test//EN\r\n\
BEGIN:VEVENT\r\nUID:{slug}@test\r\nDTSTAMP:{stamp}\r\n\
DTSTART:{day}T090000Z\r\nDTEND:{day}T100000Z\r\nSUMMARY:Lecture {slug}\r\n\
END:VEVENT\r\nEND:VCALENDAR\r\n"
    )
}

/// The id `known_event_ids` records for this task — `VTODO:` sigil included.
const TASK_ID: &str = "VTODO:hand-in@u-crawler";

fn task_ics() -> String {
    let stamp = stamp();
    let day = day_from_now(11);
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//caldir-test//EN\r\n\
BEGIN:VTODO\r\nUID:hand-in@u-crawler\r\nDTSTAMP:{stamp}\r\n\
DUE:{day}T170000Z\r\nSUMMARY:Hand in the assignment\r\n\
END:VTODO\r\nEND:VCALENDAR\r\n"
    )
}

struct Sandbox {
    _tmp: tempfile::TempDir,
    root: PathBuf,
}

impl Sandbox {
    fn new(with_task: bool) -> Self {
        Self::with_provider(with_task, "stub", STUB_PROVIDER)
    }

    /// No local files at all, so the outgoing half of a sync is empty and any
    /// failure must have come from the incoming half.
    fn remote_only(script: &str) -> Self {
        Self::build(false, "stub", script, false)
    }

    fn with_provider(with_task: bool, slug: &str, script: &str) -> Self {
        Self::build(with_task, slug, script, true)
    }

    fn build(with_task: bool, slug: &str, script: &str, with_events: bool) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();

        fs::create_dir_all(root.join("xdg/caldir")).unwrap();
        fs::create_dir_all(root.join("bin")).unwrap();
        fs::create_dir_all(root.join("data/work/.caldir")).unwrap();

        fs::write(
            root.join("xdg/caldir/config.toml"),
            format!("calendar_dir = \"{}\"\n", root.join("data").display()),
        )
        .unwrap();
        fs::write(
            root.join("data/work/.caldir/config.toml"),
            format!("name = \"Work\"\n[remote]\nprovider = \"{slug}\"\n"),
        )
        .unwrap();

        let stub = root.join("bin/caldir-provider-stub");
        fs::write(&stub, script).unwrap();
        make_executable(&stub);

        // The task is written first so it also sorts first in `read_dir`: the
        // original defect abandoned whatever followed it.
        if with_task {
            fs::write(root.join("data/work/hand-in.ics"), task_ics()).unwrap();
        }
        if with_events {
            for slug in ["a", "b"] {
                fs::write(
                    root.join(format!("data/work/lecture-{slug}.ics")),
                    event_ics(slug),
                )
                .unwrap();
            }
        }

        Self { _tmp: tmp, root }
    }

    fn run(&self, args: &[&str]) -> Output {
        let path = format!(
            "{}:{}",
            self.root.join("bin").display(),
            std::env::var("PATH").unwrap_or_default()
        );

        Command::new(env!("CARGO_BIN_EXE_caldir"))
            .args(args)
            .env("XDG_CONFIG_HOME", self.root.join("xdg"))
            .env("HOME", &self.root)
            .env("PATH", path)
            .env("NO_COLOR", "1")
            .output()
            .expect("the caldir binary runs")
    }

    fn sync(&self) -> Output {
        self.run(&["sync", "--verbose"])
    }

    fn discard(&self) -> Output {
        self.run(&["discard", "--verbose", "--force"])
    }

    fn pull(&self) -> Output {
        self.run(&["pull", "--verbose"])
    }

    /// The ids caldir recorded as synced — i.e. what actually reached the
    /// remote. Absent means nothing was pushed.
    fn pushed_ids(&self) -> String {
        fs::read_to_string(self.root.join("data/work/.caldir/state/known_event_ids"))
            .unwrap_or_default()
    }

    fn exists(&self, file: &str) -> bool {
        self.root.join("data/work").join(file).exists()
    }

    fn calendar_dir(&self) -> PathBuf {
        self.root.join("data/work")
    }

    /// Pre-seeds the sync state so `ids` count as previously synced: a remote
    /// event with no local file is then an outgoing *delete*, not an incoming
    /// create.
    fn already_synced(self, ids: &[String]) -> Self {
        let state = self.root.join("data/work/.caldir/state");
        fs::create_dir_all(&state).unwrap();
        fs::write(
            state.join("known_event_ids"),
            format!("{}\n", ids.join("\n")),
        )
        .unwrap();
        self
    }

    fn rpc_log(&self) -> String {
        fs::read_to_string(self.root.join("rpc.log")).unwrap_or_default()
    }
}

#[cfg(unix)]
fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) {}

fn streams(output: &Output) -> (String, String) {
    (
        String::from_utf8(output.stdout.clone()).unwrap(),
        String::from_utf8(output.stderr.clone()).unwrap(),
    )
}

// ---------------------------------------------------------------- sync

/// The pipeline-visible half of the fix, for the command most users run.
///
/// **Replaces `a_sync_holding_a_task_exits_non_zero_reports_on_stderr_and_
/// pushes_nothing`, which locked the whole-push refusal.** That test required
/// a non-zero exit, stderr naming the file, and zero ids pushed — and, because
/// `sync` renders one combined diff, the refusal aborted the *pull* half too.
/// The user's meetings stopped syncing in both directions until they moved the
/// task out of the calendar. Tasks are now never outgoing, so a sync holding
/// one is an ordinary successful sync that says what it kept back.
#[test]
fn a_sync_holding_a_task_pushes_every_event_and_reports_the_withheld_task() {
    let sandbox = Sandbox::new(true);
    let output = sandbox.sync();
    let (stdout, stderr) = streams(&output);

    assert!(
        output.status.success(),
        "a task may not fail the sync; got {:?}\nstdout: {stdout}\nstderr: {stderr}",
        output.status,
    );
    assert_eq!(stderr, "", "nothing failed, so nothing belongs on stderr");

    assert!(
        stdout.contains("1 task change not pushed"),
        "the withheld task must surface, got {stdout:?}",
    );
    assert!(
        !stdout.contains("Hand in the assignment"),
        "a withheld item must never get the green `+`, got {stdout:?}",
    );

    // The whole point: the events around the task synced.
    assert!(
        stdout.contains("Pushed: 2 created, 0 updated, 0 deleted"),
        "got {stdout:?}",
    );
    let ids = sandbox.pushed_ids();
    let mut pushed: Vec<&str> = ids.lines().collect();
    pushed.sort_unstable();
    assert_eq!(pushed, vec!["a@test", "b@test"]);
}

/// R1/R2: an event-only sync is untouched by any of this. Same exit code, same
/// summary, nothing on stderr.
#[test]
fn an_event_only_sync_still_exits_zero_and_summarises_what_it_pushed() {
    let sandbox = Sandbox::new(false);
    let output = sandbox.sync();
    let (stdout, stderr) = streams(&output);

    assert!(
        output.status.success(),
        "stdout: {stdout}\nstderr: {stderr}"
    );
    assert_eq!(stderr, "", "a clean sync says nothing on stderr");
    assert!(
        stdout.contains("Pushed: 2 created, 0 updated, 0 deleted"),
        "got {stdout:?}",
    );
    let ids = sandbox.pushed_ids();
    let mut pushed: Vec<&str> = ids.lines().collect();
    pushed.sort_unstable();
    assert_eq!(pushed, vec!["a@test", "b@test"]);
}

/// The exit contract is not task-specific: *any* failed sync must be visible
/// to a script. Here the provider itself rejects the write, so the failure
/// happens inside `apply_outgoing_diff`, past the pre-flight.
#[test]
fn a_sync_the_provider_rejects_also_exits_non_zero() {
    let sandbox = Sandbox::with_provider(false, "stub", FAILING_PROVIDER);
    let output = sandbox.sync();
    let (stdout, stderr) = streams(&output);

    assert!(
        !output.status.success(),
        "a rejected write must not exit 0; stdout: {stdout}\nstderr: {stderr}",
    );
    assert!(
        stderr.contains("the server said no"),
        "the provider's reason belongs on stderr, got {stderr:?}",
    );
    assert_eq!(sandbox.pushed_ids(), "", "nothing was accepted");
}

/// A calendar that cannot even be opened — its provider is not installed — is
/// a failed sync too. It is reported before any diff exists, which is exactly
/// the path that used to `println!` and fall through to exit 0.
#[test]
fn a_calendar_whose_provider_is_missing_exits_non_zero_on_sync() {
    let sandbox = Sandbox::with_provider(false, "not-installed", STUB_PROVIDER);
    let output = sandbox.sync();
    let (stdout, stderr) = streams(&output);

    assert!(
        !output.status.success(),
        "an unopenable calendar must not exit 0; stdout: {stdout}\nstderr: {stderr}",
    );
    assert!(
        stderr.contains("not-installed"),
        "stderr must name the missing provider, got {stderr:?}",
    );
}

/// A remote that cannot be listed fails before any diff exists.
#[test]
fn a_sync_whose_remote_is_unreadable_exits_non_zero() {
    let sandbox = Sandbox::with_provider(false, "stub", UNREADABLE_PROVIDER);
    let output = sandbox.sync();
    let (stdout, stderr) = streams(&output);

    assert!(
        !output.status.success(),
        "an unreadable remote must not exit 0; stdout: {stdout}\nstderr: {stderr}",
    );
    assert!(
        stderr.contains("the remote is unreachable"),
        "the reason belongs on stderr, got {stderr:?}",
    );
}

// ------------------------------------------------------------- discard

/// `discard` reverts the outgoing set, and a task is never in it — exactly as
/// on a read-only calendar, whose `discard_outgoing` clears the whole set and
/// leaves every file alone.
///
/// **Replaces `discard_still_drops_a_task_and_exits_zero`**, which asserted the
/// task file was deleted and `Discarded 3 changes`. That behaviour existed to
/// serve the whole-push refusal: discarding the task was one of the two
/// remedies the refusal message offered. With no refusal there is nothing to
/// escape, and silently deleting a task file on a command that only reverts
/// *pushable* work would be the surprising half. It is reported instead.
#[test]
fn discard_leaves_a_task_alone_and_says_so() {
    let sandbox = Sandbox::new(true);
    let output = sandbox.discard();
    let (stdout, stderr) = streams(&output);

    assert!(
        output.status.success(),
        "discarding is local-only work; stdout: {stdout}\nstderr: {stderr}",
    );
    assert_eq!(stderr, "", "a clean discard says nothing on stderr");

    assert!(
        sandbox.exists("hand-in.ics"),
        "a task is not part of the outgoing set, so it is not discarded",
    );
    assert!(
        stdout.contains("1 task change not discarded"),
        "the untouched task must surface, got {stdout:?}",
    );

    // The events around it are discarded exactly as they would be alone.
    assert!(stdout.contains("Discarded 2 changes"), "got {stdout:?}");
    assert!(!sandbox.exists("lecture-a.ics"));
    assert!(!sandbox.exists("lecture-b.ics"));
}

/// R1/R2: an event-only discard is untouched.
#[test]
fn an_event_only_discard_still_exits_zero_and_summarises() {
    let sandbox = Sandbox::new(false);
    let output = sandbox.discard();
    let (stdout, stderr) = streams(&output);

    assert!(
        output.status.success(),
        "stdout: {stdout}\nstderr: {stderr}"
    );
    assert_eq!(stderr, "", "a clean discard says nothing on stderr");
    assert!(
        stdout.contains("Discarded 2 changes: 2 created, 0 updated, 0 deleted"),
        "got {stdout:?}",
    );
    assert!(!sandbox.exists("lecture-a.ics"));
    assert!(!sandbox.exists("lecture-b.ics"));
}

/// The same exit-contract hole `push` had: a calendar that cannot be opened
/// was reported to stdout and the process still exited 0.
#[test]
fn a_calendar_whose_provider_is_missing_exits_non_zero_on_discard() {
    let sandbox = Sandbox::with_provider(false, "not-installed", STUB_PROVIDER);
    let output = sandbox.discard();
    let (stdout, stderr) = streams(&output);

    assert!(
        !output.status.success(),
        "an unopenable calendar must not exit 0; stdout: {stdout}\nstderr: {stderr}",
    );
    assert!(
        stderr.contains("not-installed"),
        "stderr must name the missing provider, got {stderr:?}",
    );
    assert!(
        sandbox.exists("lecture-a.ics"),
        "nothing may be discarded when the remote state is unknown",
    );
}

/// A remote that cannot be listed means the local changes cannot be told
/// apart from remote ones, so discarding anything would be guesswork.
#[test]
fn a_discard_whose_remote_is_unreadable_exits_non_zero() {
    let sandbox = Sandbox::with_provider(false, "stub", UNREADABLE_PROVIDER);
    let output = sandbox.discard();
    let (stdout, stderr) = streams(&output);

    assert!(
        !output.status.success(),
        "an unreadable remote must not exit 0; stdout: {stdout}\nstderr: {stderr}",
    );
    assert!(
        stderr.contains("the remote is unreachable"),
        "the reason belongs on stderr, got {stderr:?}",
    );
    assert!(
        sandbox.exists("lecture-a.ics"),
        "nothing may be discarded when the remote state is unknown",
    );
}

// ---------------------------------------------------------------- pull

/// `pull` has no outgoing half, so the unpushable-task pre-flight does not
/// apply: a local task simply is not part of a pull. Locked so the guard is
/// not copied over.
///
/// The task is pre-seeded as already synced, so "the file is still there"
/// cannot be true for a trivial reason. A never-synced task is an *outgoing*
/// create that `pull` discards anyway; a previously-synced one missing from
/// the remote response is a candidate *incoming delete*, and only R49's
/// version-skew belt holds it back. That is the mechanism this test exists to
/// watch, and it is the same mechanism whatever today's date is.
#[test]
fn pull_ignores_a_local_task_and_exits_zero() {
    let sandbox = Sandbox::new(true).already_synced(&[TASK_ID.to_string()]);
    let output = sandbox.pull();
    let (stdout, stderr) = streams(&output);

    assert!(
        output.status.success(),
        "a task is not part of a pull; stdout: {stdout}\nstderr: {stderr}",
    );
    assert_eq!(stderr, "", "a clean pull says nothing on stderr");
    assert!(
        sandbox.exists("hand-in.ics"),
        "a pull may not touch the local task",
    );

    // Non-vacuity: the belt is what saved it, and it says so. Without the
    // suppression the task would have been deleted here, silently.
    assert!(
        stdout.contains("task deletion withheld"),
        "the withheld delete must be reported, got {stdout:?}",
    );
}

/// The belt is scoped to *tasks*: an event that has synced before and is
/// absent from the remote response is still a real delete. Sibling of the test
/// above, and the reason its green tick means something — without this, "the
/// file survived" would be satisfied by a `pull` that deletes nothing at all.
#[test]
fn pull_still_deletes_a_previously_synced_event_the_remote_no_longer_lists() {
    let sandbox = Sandbox::new(false).already_synced(&["a@test".to_string()]);
    let output = sandbox.pull();
    let (stdout, stderr) = streams(&output);

    assert!(
        output.status.success(),
        "stdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        !sandbox.exists("lecture-a.ics"),
        "a synced event gone from the remote is a real delete, got {stdout:?}",
    );
    assert!(
        !stdout.contains("task deletion withheld"),
        "the belt is task-scoped and must not fire here, got {stdout:?}",
    );
}

/// The stdout/exit-0 reporting defect is `pull`'s too, even though the
/// pre-flight is not.
#[test]
fn a_calendar_whose_provider_is_missing_exits_non_zero_on_pull() {
    let sandbox = Sandbox::with_provider(false, "not-installed", STUB_PROVIDER);
    let output = sandbox.pull();
    let (stdout, stderr) = streams(&output);

    assert!(
        !output.status.success(),
        "an unopenable calendar must not exit 0; stdout: {stdout}\nstderr: {stderr}",
    );
    assert!(
        stderr.contains("not-installed"),
        "stderr must name the missing provider, got {stderr:?}",
    );
}

/// A remote that cannot be listed pulled nothing, so it must not exit 0.
#[test]
fn a_pull_whose_remote_is_unreadable_exits_non_zero() {
    let sandbox = Sandbox::with_provider(false, "stub", UNREADABLE_PROVIDER);
    let output = sandbox.pull();
    let (stdout, stderr) = streams(&output);

    assert!(
        !output.status.success(),
        "an unreadable remote must not exit 0; stdout: {stdout}\nstderr: {stderr}",
    );
    assert!(
        stderr.contains("the remote is unreachable"),
        "the reason belongs on stderr, got {stderr:?}",
    );
}

/// R1/R2: an event-only pull is untouched.
#[test]
fn an_event_only_pull_still_exits_zero() {
    let sandbox = Sandbox::new(false);
    let output = sandbox.pull();
    let (stdout, stderr) = streams(&output);

    assert!(
        output.status.success(),
        "stdout: {stdout}\nstderr: {stderr}"
    );
    assert_eq!(stderr, "", "a clean pull says nothing on stderr");
    assert!(sandbox.exists("lecture-a.ics"));
}

// --------------------------------------------- the incoming half fails

/// Makes the calendar directory unwritable for the duration of `body`, so the
/// local write an incoming create needs fails. Permissions are restored
/// afterwards or the `TempDir` cannot clean itself up.
#[cfg(unix)]
fn while_read_only<T>(dir: &Path, body: impl FnOnce() -> T) -> T {
    use std::os::unix::fs::PermissionsExt;

    let original = fs::metadata(dir).unwrap().permissions();
    fs::set_permissions(dir, fs::Permissions::from_mode(0o555)).unwrap();
    let out = body();
    fs::set_permissions(dir, original).unwrap();
    out
}

/// Running as root ignores the mode bits, so the fixture would not fail at
/// all and the assertion would be vacuous.
#[cfg(unix)]
fn read_only_dirs_are_enforced(dir: &Path) -> bool {
    while_read_only(dir, || fs::write(dir.join(".probe"), "x").is_err())
}

/// A pull that could not write what it fetched has pulled nothing, and that
/// must be as visible to a script as a failed push is.
///
/// Found by mutation: dropping the `false` from this arm left every test
/// green, because until this fixture existed no test made the *incoming* half
/// fail.
#[cfg(unix)]
#[test]
fn a_pull_that_cannot_write_locally_exits_non_zero() {
    let sandbox = Sandbox::remote_only(&serving_provider());
    if !read_only_dirs_are_enforced(&sandbox.calendar_dir()) {
        return;
    }

    let output = while_read_only(&sandbox.calendar_dir(), || sandbox.pull());
    let (stdout, stderr) = streams(&output);

    assert!(
        !output.status.success(),
        "a pull that wrote nothing must not exit 0; stdout: {stdout}\nstderr: {stderr}",
    );
    assert_ne!(stderr, "", "the reason belongs on stderr");
    assert!(
        !sandbox.exists("from-remote.ics") && !stdout.contains("Pulled:"),
        "nothing was pulled, so there is nothing to summarise, got {stdout:?}",
    );
}

/// The same hole on `sync`'s pull half. The fixture has nothing to push, so
/// the *only* thing that can fail is the incoming apply — otherwise a failing
/// push half would mask the bug this locks.
#[cfg(unix)]
#[test]
fn a_sync_whose_pull_half_cannot_write_locally_exits_non_zero() {
    let sandbox = Sandbox::remote_only(&serving_provider());
    if !read_only_dirs_are_enforced(&sandbox.calendar_dir()) {
        return;
    }

    let output = while_read_only(&sandbox.calendar_dir(), || sandbox.sync());
    let (stdout, stderr) = streams(&output);

    assert!(
        !output.status.success(),
        "a half-failed sync must not exit 0; stdout: {stdout}\nstderr: {stderr}",
    );
    assert_ne!(stderr, "", "the reason belongs on stderr");
    assert!(
        !sandbox.exists("from-remote.ics"),
        "the incoming event was never written",
    );
}

/// The mass-delete guard is a safety net, and `sync` must still honour it now
/// that its push half is inside an `if` rather than behind an early return.
///
/// Found by mutation: forcing that condition to `true` left every other test
/// green, because none of them ever queued enough deletes to trip it.
#[test]
fn a_sync_that_would_mass_delete_warns_and_pushes_nothing() {
    let ids: Vec<String> = (1..=10).map(|i| format!("doomed-{i}@test")).collect();
    let sandbox = Sandbox::remote_only(&logging_provider()).already_synced(&ids);

    let output = sandbox.sync();
    let (stdout, stderr) = streams(&output);

    assert!(
        output.status.success(),
        "declining a mass delete is not a failure; stdout: {stdout}\nstderr: {stderr}",
    );
    assert!(
        stdout.contains("You are about to delete many events"),
        "the guard must warn, got {stdout:?}",
    );
    assert!(
        !sandbox.rpc_log().contains("delete_event"),
        "not one delete may reach the remote, got {:?}",
        sandbox.rpc_log(),
    );
}
