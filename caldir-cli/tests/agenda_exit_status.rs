//! End-to-end locks on what `caldir events` tells a *script* when a calendar
//! cannot be read.
//!
//! These drive the real binary against a real caldir, because the properties
//! under test — the process exit code and which stream the notice lands on —
//! do not exist below the process boundary.
//!
//! Why they exist: an independent review found a whole calendar being dropped
//! from the agenda with no notice and exit 0. The per-file skip already had a
//! notice; the directory-level failure had nothing at all.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output};

const EVENT_ICS: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//caldir-test//EN\r\n\
BEGIN:VEVENT\r\nUID:lecture@test\r\nDTSTAMP:20260801T120000Z\r\n\
DTSTART:20260901T090000Z\r\nDTEND:20260901T100000Z\r\nSUMMARY:Lecture A\r\n\
END:VEVENT\r\nEND:VCALENDAR\r\n";

struct Sandbox {
    _tmp: tempfile::TempDir,
    root: PathBuf,
}

impl Sandbox {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();

        fs::create_dir_all(root.join("xdg/caldir")).unwrap();
        fs::create_dir_all(root.join("data/work")).unwrap();
        fs::create_dir_all(root.join("data/personal")).unwrap();

        fs::write(
            root.join("xdg/caldir/config.toml"),
            format!("calendar_dir = \"{}\"\n", root.join("data").display()),
        )
        .unwrap();
        fs::write(root.join("data/work/lecture.ics"), EVENT_ICS).unwrap();

        Self { _tmp: tmp, root }
    }

    /// Make `personal` impossible to list: `read_dir` on a directory with no
    /// `r`/`x` bits fails outright, which is the directory-level failure the
    /// agenda used to swallow.
    fn seal_personal(&self) {
        fs::set_permissions(
            self.root.join("data/personal"),
            fs::Permissions::from_mode(0o000),
        )
        .unwrap();
    }

    fn unseal_personal(&self) {
        let _ = fs::set_permissions(
            self.root.join("data/personal"),
            fs::Permissions::from_mode(0o755),
        );
    }

    fn events(&self) -> Output {
        self.run(&["events", "--from", "2026-09-01", "--to", "2026-09-02"])
    }

    fn events_json(&self) -> Output {
        self.run(&[
            "events",
            "--from",
            "2026-09-01",
            "--to",
            "2026-09-02",
            "--json",
        ])
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_caldir"))
            .args(args)
            .env("XDG_CONFIG_HOME", self.root.join("xdg"))
            .env("HOME", &self.root)
            .env("NO_COLOR", "1")
            .output()
            .expect("the caldir binary runs")
    }

    fn write_work(&self, name: &str, contents: &str) {
        fs::write(self.root.join("data/work").join(name), contents).unwrap();
    }
}

/// The titles `caldir events --json` says are visible in the window.
fn json_titles(output: &Output) -> Vec<String> {
    let stdout = String::from_utf8(output.stdout.clone()).unwrap();
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).unwrap_or_else(|err| panic!("{err}: {stdout:?}"));

    parsed
        .as_array()
        .expect("the agenda schema is an array")
        .iter()
        .map(|entry| entry["title"].as_str().unwrap_or("").to_string())
        .collect()
}

/// A `VEVENT` beside each component RFC 5545 §3.4 lets an `icalbody` carry.
/// `main` ignored all four; erroring the document lost the event with it.
const UNSUPPORTED_SIBLINGS: [(&str, &str); 4] = [
    (
        "VFREEBUSY",
        "BEGIN:VFREEBUSY\r\nUID:fb@test\r\nDTSTAMP:20260801T120000Z\r\nEND:VFREEBUSY\r\n",
    ),
    (
        "VJOURNAL",
        "BEGIN:VJOURNAL\r\nUID:j@test\r\nDTSTAMP:20260801T120000Z\r\nEND:VJOURNAL\r\n",
    ),
    (
        "VALARM",
        "BEGIN:VALARM\r\nACTION:DISPLAY\r\nTRIGGER:-PT15M\r\nDESCRIPTION:Ping\r\nEND:VALARM\r\n",
    ),
    (
        "X-WR-CUSTOM",
        "BEGIN:X-WR-CUSTOM\r\nX-THING:1\r\nEND:X-WR-CUSTOM\r\n",
    ),
];

/// The shipping blocker, at the surface a script reads.
///
/// A `.ics` holding one `VEVENT` beside a component caldir does not model is
/// legal ICS and loaded on `main`. Erroring the whole document took the event
/// with it: `--json` visibility went 1 -> 0, exit 0, nothing on stderr — and
/// the resulting incomplete listing then suppressed every delete for that
/// calendar. One supported component plus any number of unsupported siblings
/// must load.
#[test]
fn an_event_beside_an_unsupported_sibling_is_still_visible_in_json() {
    for (name, sibling) in UNSUPPORTED_SIBLINGS {
        let sandbox = Sandbox::new();
        sandbox.write_work(
            "sibling.ics",
            &format!(
                "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//caldir-test//EN\r\n\
BEGIN:VEVENT\r\nUID:sibling@test\r\nDTSTAMP:20260801T120000Z\r\n\
DTSTART:20260901T110000Z\r\nDTEND:20260901T120000Z\r\nSUMMARY:Beside {name}\r\n\
END:VEVENT\r\n{sibling}END:VCALENDAR\r\n"
            ),
        );

        let output = sandbox.events_json();
        let titles = json_titles(&output);

        assert!(
            titles.contains(&format!("Beside {name}")),
            "a VEVENT beside a {name} must still be visible; got {titles:?}"
        );
        assert!(output.status.success(), "{name}: {:?}", output.status);
    }
}

/// B2: `--json` is the item stream and nothing else (C12), so a short listing
/// has no key to announce itself with. Without a channel a script cannot tell
/// a partial agenda from a complete one — stderr is not part of the JSON
/// contract, so the notice goes there.
#[test]
fn a_json_agenda_announces_a_skipped_file_on_stderr() {
    let sandbox = Sandbox::new();
    sandbox.write_work(
        "broken.ics",
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:broken@test\r\n\
DTSTART:not-a-timestamp\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
    );

    let output = sandbox.events_json();
    let stderr = String::from_utf8(output.stderr.clone()).unwrap();

    assert!(
        stderr.contains("1 unreadable file in work"),
        "a partial --json agenda must be detectable; got stderr {stderr:?}"
    );
    assert!(
        json_titles(&output).contains(&"Lecture A".to_string()),
        "the healthy event must still be listed"
    );
}

/// The stderr notice is the `--json` channel *only*. Text output already
/// prints it above the agenda; printing it on stderr too would double-report
/// the same skip to anyone reading both streams.
#[test]
fn a_text_agenda_keeps_its_skip_notice_on_stdout_only() {
    let sandbox = Sandbox::new();
    sandbox.write_work(
        "broken.ics",
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:broken@test\r\n\
DTSTART:not-a-timestamp\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
    );

    let output = sandbox.events();
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();

    assert!(
        stdout.contains("1 unreadable file in work"),
        "the text notice stays above the agenda, got {stdout:?}"
    );
    assert_eq!(
        stderr, "",
        "text output must not repeat the notice on stderr"
    );
}

/// C13: a caldir that reads cleanly gains nothing on stderr under `--json`
/// either — the notice exists only when something was actually lost.
#[test]
fn a_clean_json_agenda_says_nothing_on_stderr() {
    let sandbox = Sandbox::new();
    let output = sandbox.events_json();

    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        "",
        "a clean --json agenda says nothing on stderr"
    );
    assert!(output.status.success());
}

#[test]
fn an_unreadable_calendar_is_named_on_the_agenda_and_exits_non_zero() {
    let sandbox = Sandbox::new();
    sandbox.seal_personal();
    let output = sandbox.events();
    sandbox.unseal_personal();

    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();

    assert!(
        stdout.contains("personal could not be read at all"),
        "the dropped calendar must be named above the agenda, got: {stdout:?}"
    );
    assert!(
        !stdout.contains("unreadable file"),
        "no file of it was reached, so it must not be reported as a file skip: {stdout:?}"
    );
    assert!(
        stdout.contains("Lecture A"),
        "the healthy calendar's events must still list (F-6), got: {stdout:?}"
    );
    assert!(
        !output.status.success(),
        "a partial agenda must be detectable by a script; got {:?}\nstdout: {stdout}\nstderr: {stderr}",
        output.status,
    );
    assert!(
        stderr.contains("personal"),
        "the failure must reach stderr too, got: {stderr:?}"
    );
}

/// C13 / R1: a caldir every calendar of which reads cleanly is untouched by
/// any of this — same agenda, exit 0, nothing on stderr.
#[test]
fn a_readable_caldir_still_exits_zero_and_says_nothing_extra() {
    let sandbox = Sandbox::new();
    let output = sandbox.events();

    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();

    assert!(
        output.status.success(),
        "stdout: {stdout}\nstderr: {stderr}"
    );
    assert_eq!(stderr, "", "a clean agenda says nothing on stderr");
    assert!(!stdout.contains("⚠"), "no notice is owed: {stdout:?}");
    assert!(stdout.contains("Lecture A"), "{stdout:?}");
}
