//! End-to-end locks on what `caldir push` tells a *script*.
//!
//! These drive the real binary against a real caldir and a stub provider on
//! `PATH`, because the properties under test — the process exit code and which
//! stream the failure lands on — do not exist below the process boundary.
//!
//! Why they exist: QA reproduced a push against Radicale that abandoned
//! changes, printed its error to **stdout**, and exited **0**. The automated
//! pipeline caldir serves could not detect the failure at all.

use chrono::{Duration, Utc};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

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

/// Fixture timestamps are anchored to *today*, never to a literal date.
///
/// `push` diffs against the CLI's default ±365-day window, so a fixture pinned
/// to a literal 2026 date silently falls out of that window in 2027 — and a
/// test that passes because its subject is out of range is a green tick over
/// zero coverage, not a passing test.
fn day_from_now(days: i64) -> String {
    (Utc::now() + Duration::days(days))
        .format("%Y%m%d")
        .to_string()
}

fn stamp() -> String {
    Utc::now().format("%Y%m%dT%H%M%SZ").to_string()
}

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

/// `P6-04`: as `STUB_PROVIDER`, but also answers `capabilities` with
/// `tasks.write: true` (so a task is no longer withheld — see
/// `Connection::diff`, `caldir-core/src/connection.rs`) and logs every
/// request it is asked for to `$HOME/rpc.log`, mirroring
/// `sync_exit_status.rs`'s `logging_provider`.
const TASK_CAPABLE_STUB_PROVIDER: &str = r#"#!/bin/sh
request=$(cat)
echo "$request" >> "$HOME/rpc.log"
case "$request" in
  *'"list_events"'*)
    echo '{"status":"success","data":[]}'
    ;;
  *'"capabilities"'*)
    echo '{"status":"success","data":{"events":{"read":true,"write":true},"tasks":{"read":true,"write":true}}}'
    ;;
  *)
    event=${request#*'"event":'}
    event=${event%\}\}}
    printf '{"status":"success","data":%s}\n' "$event"
    ;;
esac
"#;

struct Sandbox {
    _tmp: tempfile::TempDir,
    root: PathBuf,
}

/// Fails every write verb while still listing an empty remote, so the failure
/// lands in `apply_outgoing_diff` rather than in the pre-flight.
const FAILING_PROVIDER: &str = r#"#!/bin/sh
request=$(cat)
case "$request" in
  *'"list_events"'*) echo '{"status":"success","data":[]}' ;;
  *) echo '{"status":"error","error":"the server said no"}' ;;
esac
"#;

impl Sandbox {
    fn new(with_task: bool) -> Self {
        Self::with_provider(with_task, "stub", STUB_PROVIDER)
    }

    fn with_provider(with_task: bool, slug: &str, script: &str) -> Self {
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
        for slug in ["a", "b"] {
            fs::write(
                root.join(format!("data/work/lecture-{slug}.ics")),
                event_ics(slug),
            )
            .unwrap();
        }

        Self { _tmp: tmp, root }
    }

    fn push(&self) -> Output {
        let path = format!(
            "{}:{}",
            self.root.join("bin").display(),
            std::env::var("PATH").unwrap_or_default()
        );

        Command::new(env!("CARGO_BIN_EXE_caldir"))
            .arg("push")
            .arg("--verbose")
            .env("XDG_CONFIG_HOME", self.root.join("xdg"))
            .env("HOME", &self.root)
            .env("PATH", path)
            .env("NO_COLOR", "1")
            .output()
            .expect("the caldir binary runs")
    }

    /// The ids caldir recorded as synced — i.e. what actually reached the
    /// remote. Absent means nothing was pushed.
    fn pushed_ids(&self) -> String {
        fs::read_to_string(self.root.join("data/work/.caldir/state/known_event_ids"))
            .unwrap_or_default()
    }

    /// `P6-04`: every request `TASK_CAPABLE_STUB_PROVIDER` logged, in order.
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

/// The pipeline-visible half of the fix.
///
/// **Replaces `a_push_holding_a_task_exits_non_zero_reports_on_stderr_and_
/// pushes_nothing`, which locked the whole-push refusal.** That test asserted
/// the opposite of what it now asserts — non-zero exit, stderr naming the
/// file, and *zero* ids pushed — because a task in the outgoing set failed the
/// whole calendar. CalDAV is the only provider that pulls tasks, so that was
/// the normal path: pull a mixed calendar, tick one task off, and the
/// meetings stop syncing on every run. Tasks are now never outgoing, so there
/// is nothing to refuse; what survives from the old test is the half that
/// still matters — the task is not pushed, and it is not painted as pushed.
#[test]
fn a_push_holding_a_task_pushes_every_event_and_reports_the_withheld_task() {
    let sandbox = Sandbox::new(true);
    let output = sandbox.push();

    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();

    assert!(
        output.status.success(),
        "a task may not fail the calendar's push; got {:?}\nstdout: {stdout}\nstderr: {stderr}",
        output.status,
    );
    assert_eq!(stderr, "", "nothing failed, so nothing belongs on stderr");

    // The withholding is reported, never silent.
    assert!(
        stdout.contains("1 task change not pushed"),
        "the withheld task must surface, got {stdout:?}",
    );

    // Still not painted as work that happened.
    assert!(
        !stdout.contains("Hand in the assignment"),
        "a withheld item must never get the green `+`, got {stdout:?}",
    );

    // The whole point: both events queued around the task really went out.
    let pushed = sandbox.pushed_ids();
    for slug in ["a", "b"] {
        assert!(
            pushed.contains(&format!("{slug}@test")),
            "the events around a task must push; got {pushed:?}",
        );
    }
    assert!(
        !pushed.contains("hand-in@u-crawler"),
        "the task itself must not be pushed; got {pushed:?}",
    );
}

/// R1/R2: an event-only push is untouched by any of this. Same exit code, same
/// summary, nothing on stderr.
#[test]
fn an_event_only_push_still_exits_zero_and_summarises_what_it_pushed() {
    let sandbox = Sandbox::new(false);
    let output = sandbox.push();

    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();

    assert!(
        output.status.success(),
        "stdout: {stdout}\nstderr: {stderr}"
    );
    assert_eq!(stderr, "", "a clean push says nothing on stderr");
    assert!(
        stdout.contains("Pushed: 2 created, 0 updated, 0 deleted"),
        "got {stdout:?}",
    );
    let ids = sandbox.pushed_ids();
    let mut pushed: Vec<&str> = ids.lines().collect();
    pushed.sort_unstable();
    assert_eq!(pushed, vec!["a@test", "b@test"]);
}

/// The exit contract is not task-specific: *any* failed push must be visible
/// to a script. Here the provider itself rejects the write, so the failure
/// happens inside `apply_outgoing_diff`, past the pre-flight.
#[test]
fn a_push_the_provider_rejects_also_exits_non_zero() {
    let sandbox = Sandbox::with_provider(false, "stub", FAILING_PROVIDER);
    let output = sandbox.push();

    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();

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
/// a failed push too. It is reported before any diff exists, which is exactly
/// the path that used to `println!` and fall through to exit 0.
#[test]
fn a_calendar_whose_provider_is_missing_exits_non_zero() {
    let sandbox = Sandbox::with_provider(false, "not-installed", STUB_PROVIDER);
    let output = sandbox.push();

    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();

    assert!(
        !output.status.success(),
        "an unopenable calendar must not exit 0; stdout: {stdout}\nstderr: {stderr}",
    );
    assert!(
        stderr.contains("not-installed"),
        "stderr must name the missing provider, got {stderr:?}",
    );
}

/// `P6-04`/`CALDAV-8`/`GOOGLE-12`: a calendar mixing events and a task, pushed
/// against a provider that *does* advertise `tasks.write`. The only place a
/// task write can break an event write is a shared calendar in a real push,
/// so this drives both through one process. The event half is checked
/// against exactly what `an_event_only_push_still_exits_zero_and_
/// summarises_what_it_pushed` asserts for its own two events — same ids, same
/// exit code, same stderr — so a real regression in event handling cannot
/// hide behind "the task changed the numbers".
#[test]
fn a_calendar_holding_events_and_tasks_pushes_both_and_exits_zero() {
    let sandbox = Sandbox::with_provider(true, "stub", TASK_CAPABLE_STUB_PROVIDER);
    let output = sandbox.push();

    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();

    assert!(
        output.status.success(),
        "a task-capable provider must still push events cleanly; got {:?}\nstdout: {stdout}\nstderr: {stderr}",
        output.status,
    );
    assert_eq!(stderr, "", "nothing failed, so nothing belongs on stderr");
    assert!(
        !stdout.contains("task change not pushed"),
        "a provider advertising tasks.write must not withhold the task, got {stdout:?}",
    );

    let log = sandbox.rpc_log();
    assert!(
        log.contains("create_event") && log.matches("\"event\":\"BEGIN:VCALENDAR").count() >= 3,
        "both events and the task must reach the provider as create_event calls; got {log:?}",
    );
    assert!(log.contains("BEGIN:VEVENT"), "got {log:?}");
    assert!(log.contains("BEGIN:VTODO"), "got {log:?}");

    // The event half: same two ids as the event-only push locks, unaffected
    // by the task sharing the calendar.
    let pushed = sandbox.pushed_ids();
    for slug in ["a", "b"] {
        assert!(
            pushed.contains(&format!("{slug}@test")),
            "the events must still push exactly as the event-only case does; got {pushed:?}",
        );
    }
    assert!(
        pushed.contains("VTODO:hand-in@u-crawler"),
        "a task-capable provider must actually push the task; got {pushed:?}",
    );
}
