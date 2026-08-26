//! End-to-end locks on what `caldir push` tells a *script*.
//!
//! These drive the real binary against a real caldir and a stub provider on
//! `PATH`, because the properties under test — the process exit code and which
//! stream the failure lands on — do not exist below the process boundary.
//!
//! Why they exist: QA reproduced a push against Radicale that abandoned
//! changes, printed its error to **stdout**, and exited **0**. The automated
//! pipeline caldir serves could not detect the failure at all.

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

fn event_ics(slug: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//caldir-test//EN\r\n\
BEGIN:VEVENT\r\nUID:{slug}@test\r\nDTSTAMP:20260801T120000Z\r\n\
DTSTART:20260901T090000Z\r\nDTEND:20260901T100000Z\r\nSUMMARY:Lecture {slug}\r\n\
END:VEVENT\r\nEND:VCALENDAR\r\n"
    )
}

const TASK_ICS: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//caldir-test//EN\r\n\
BEGIN:VTODO\r\nUID:hand-in@u-crawler\r\nDTSTAMP:20260801T120000Z\r\n\
DUE:20260905T170000Z\r\nSUMMARY:Hand in the assignment\r\n\
END:VTODO\r\nEND:VCALENDAR\r\n";

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
            fs::write(root.join("data/work/hand-in.ics"), TASK_ICS).unwrap();
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
/// A push that cannot happen must exit non-zero and say so on **stderr**;
/// before the fix it exited 0, printed to stdout, painted the task with the
/// green `+` that means "pushed", and pushed some of the events anyway.
#[test]
fn a_push_holding_a_task_exits_non_zero_reports_on_stderr_and_pushes_nothing() {
    let sandbox = Sandbox::new(true);
    let output = sandbox.push();

    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();

    assert!(
        !output.status.success(),
        "a failed push must be detectable by a script; got {:?}\nstdout: {stdout}",
        output.status,
    );
    assert!(
        stderr.contains("hand-in.ics"),
        "stderr must name the offending file, got {stderr:?}",
    );
    assert!(
        stdout.is_empty() || !stdout.contains("hand-in.ics"),
        "the refused item must not be reported on stdout, got {stdout:?}",
    );

    // Not rendered as work about to happen: no `+` line for the task, and no
    // `Pushed:` summary, because nothing was pushed.
    assert!(
        !stdout.contains("Hand in the assignment"),
        "a refused item must never get the green `+`, got {stdout:?}",
    );
    assert!(
        !stdout.contains("Pushed:"),
        "nothing was pushed, so there is nothing to summarise, got {stdout:?}",
    );

    // And the events queued around the task really are still pending.
    assert_eq!(
        sandbox.pushed_ids(),
        "",
        "a refused push may not apply a single change",
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
