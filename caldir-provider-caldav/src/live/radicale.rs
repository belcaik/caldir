//! Six gates against a real Radicale 3.7.6 in Docker.
//!
//! # How to run them
//!
//! They are gated at **runtime**, never with `#[ignore]`, so the plain
//! `cargo test --workspace` used by CI runs them as no-ops and stays green on
//! a machine with no Docker:
//!
//! ```text
//! cargo test --workspace                                            # skipped
//! CALDIR_LIVE_RADICALE=1 cargo test -p caldir-provider-caldav -- live::radicale
//! docker ps -a | grep caldir-live-radicale                          # prints nothing
//! ```
//!
//! [`live_gate`] requires `CALDIR_LIVE_RADICALE=1` and a working `docker` and
//! `curl`; otherwise every test prints one skip line and returns.
//!
//! # The container
//!
//! Reproduced from `docs/vtodo/context/radicale-probe.md` §Reproduction:
//! `tomsquest/docker-radicale:latest` (3.7.6), a bcrypt htpasswd for
//! `testuser:testpass`, `[rights] type = owner_only`,
//! `[storage] filesystem_folder = /data/collections`, published on an
//! ephemeral loopback port, collections created with `MKCALENDAR`. Base URL is
//! plain `http://127.0.0.1:{port}/testuser/{collection}/`, which
//! `create_caldav_client` allows via `https_or_http()`. Only `/config` is
//! bind-mounted; `/data` is the image's own volume, so a torn-down container
//! takes its collections with it and no test can see another's state.
//!
//! The harness speaks HTTP with `curl` — deliberately **not** caldir's own
//! client, so a transport bug cannot cancel out on both sides of a gate. See
//! [`RadicaleServer::try_request`].
//!
//! [`RadicaleServer`]'s `Drop` runs `docker rm -f` — on success, on failure and
//! on panic, since a panicking test still unwinds through the guard. The name
//! is unique per test (`caldir-live-radicale-{pid}-{nanos}`) and a pre-flight
//! `docker rm -f` clears a stale leftover. Every test is
//! `#[serial_test::serial(radicale)]` so containers never race for a port.
//! Startup is health-polled with a `PROPFIND`, never a fixed sleep.
//!
//! # One test was renamed
//!
//! `T-58`'s pinned name, `renaming_a_local_task_file_updates_the_same_resource`,
//! is now reachable and is still **declined, deliberately** — see the refusal
//! note on [`renaming_a_local_file_updates_the_same_resource_and_never_a_second_one`],
//! which the gate still ships as, on the VEVENT control. The other five keep
//! their R105-pinned names.
//!
//! # Two forbidden assertions
//!
//! **R30** — no gate compares the raw bytes of what was PUT against what the
//! server GETs back. Radicale sorts properties alphabetically inside each
//! component, re-folds lines at 75 octets and synthesizes a `VTIMEZONE` for
//! any `TZID`, so that comparison is permanently red. It *is* idempotent,
//! which is what makes [`get_pass_two_equals_get_pass_one`]'s comparison of
//! two server outputs the one legal byte comparison here. Everything else
//! compares parsed domain values.
//!
//! **R31** — no gate uses an `ETag` as proof that a push was a no-op. caldir
//! re-stamps `DTSTAMP` on every serialize (R25), so every push mutates the
//! ETag even when nothing changed. Grepping this module for `etag` finds no
//! assertion of that shape.
//!
//! # Q-3, DECIDED: caldir does not emit `VTIMEZONE`; it tolerates Radicale's
//!
//! caldir emits a zoned `DUE`/`DTSTART` as a bare `TZID` parameter and no
//! `VTIMEZONE` component (`caldir-core/src/todo/to_icalendar.rs`), and
//! tolerates the server adding one. Measured here, not assumed:
//! [`get_pass_two_equals_get_pass_one`] PUTs a `DUE;TZID=Europe/Stockholm`
//! task carrying no `VTIMEZONE`, and Radicale accepts it `201` and returns it
//! with a full synthesized `VTIMEZONE` sibling; caldir then parses that
//! two-component document back to the same zoned `DUE`, because `VTIMEZONE` is
//! exempt from the single-component arity rule (R18, R102).
//! [`a_u_crawler_task_round_trips_through_radicale_preserving_identity_and_semantics`]
//! closes the loop: the document caldir re-serializes from a server answer is
//! accepted again and parses back to the same domain values. **Hand this
//! paragraph to `L8-06` for `BACKLOG.md`.**
//!
//! # R69, the iCloud gap — UNMEASURED, and not claimed
//!
//! Every gate below ran against **Radicale 3.7.6 only**.
//! `caldir-provider-icloud` rides this identical `caldav::ops` code path with
//! no mapping layer of its own, but that is a code-sharing fact, not a
//! measurement. Whether iCloud answers a same-UID pair with `409
//! <C:no-uid-conflict/>`, whether it implements the RFC 4791 §9.9 VTODO
//! time-range table, and whether it injects non-`X-` properties onto a task
//! are all **unknown**. Nothing here supports a claim of iCloud task support;
//! R69 is settled only by running these same probes against a real iCloud
//! account.
//!
//! # R70 / R69: what round-trips, and what still does not
//!
//! caldir can now both **pull** and **push** tasks through production code:
//! `ops::create_event::create_item` / `ops::update_event::update_item` /
//! `ops::delete_event` are item-typed and kind-aware, so a `Todo` reaches a
//! real `PUT`/`DELETE` the same way an `Event` does. The gates below this
//! point exercise that write path directly; the six gates above it predate
//! it and still seed/mutate tasks with the harness's raw `curl`, which is
//! noted on each of them individually.

use std::io::Write as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use caldir_core::{CalendarItem, ComponentKind, EventTime, EventUid, Todo, TodoStatus};
use chrono::{Duration as ChronoDuration, SecondsFormat, Utc};
use http::StatusCode;

use crate::caldav::ops;
use crate::caldav::ops::test_dav::vtodo_property_names;
use crate::caldav::url_to_href;

// ============================================================================
// Gating
// ============================================================================

/// The environment variable that opts a run into the live tier.
const GATE_VAR: &str = "CALDIR_LIVE_RADICALE";

/// Is the live tier enabled *and* runnable here?
///
/// Three conditions, all runtime: the opt-in variable is exactly `"1"`, and
/// both `docker --version` and `curl --version` succeed — `curl` is the
/// harness's HTTP client, deliberately not caldir's own (see
/// [`RadicaleServer::try_request`]). `#[ignore]` would not do — the index's verify
/// command (`cargo test … -- live::radicale`) does not pass `--ignored`, so an
/// ignored test would silently never run.
fn live_enabled() -> bool {
    if std::env::var(GATE_VAR).as_deref() != Ok("1") {
        return false;
    }

    ["docker", "curl"].into_iter().all(|tool| {
        Command::new(tool)
            .arg("--version")
            .output()
            .is_ok_and(|out| out.status.success())
    })
}

/// Start a server, or print why this test is a no-op and yield `None`.
fn live_gate(test: &str) -> Option<RadicaleServer> {
    if !live_enabled() {
        println!(
            "SKIP live::radicale::{test}: set {GATE_VAR}=1 with a working \
             `docker` and `curl` to run the live tier"
        );
        return None;
    }

    Some(RadicaleServer::start())
}

// ============================================================================
// The container
// ============================================================================

/// Radicale 3.7.6, pinned by digest so a tag repoint can never make a green
/// gate meaningless or a red one unreproducible. Resolved from
/// `tomsquest/docker-radicale:latest` on 2026-08-26 via:
///
/// ```text
/// docker pull tomsquest/docker-radicale:latest
/// docker inspect --format='{{index .RepoDigests 0}}' tomsquest/docker-radicale:latest
/// ```
const IMAGE: &str = "tomsquest/docker-radicale@sha256:45f92956a16dc139ab741a56612082723bfd09977b4b97764760532767be7448";

const USERNAME: &str = "testuser";
const PASSWORD: &str = "testpass";

/// bcrypt hash of `testpass`, generated with the image's own venv python
/// exactly as `radicale-probe.md` §Reproduction 1 prescribes:
///
/// ```text
/// docker run --rm --entrypoint /venv/bin/python3 tomsquest/docker-radicale:latest \
///   -c "import bcrypt;print('testuser:'+bcrypt.hashpw(b'testpass',bcrypt.gensalt()).decode())"
/// ```
///
/// Embedded rather than regenerated per test: a bcrypt hash is a stable input,
/// and a second `docker run` per test buys nothing.
const HTPASSWD: &str = "testuser:$2b$12$cyTecAWLl9M3XZxb6ht85ePCFX8r.akSJPlJb6xLQ2zK3mi8iGtRe\n";

/// `radicale-probe.md` §Reproduction 1, verbatim.
const CONFIG: &str = "\
[server]
hosts = 0.0.0.0:5232

[auth]
type = htpasswd
htpasswd_filename = /config/users
htpasswd_encryption = bcrypt

[storage]
filesystem_folder = /data/collections

[rights]
type = owner_only
";

/// A running Radicale container, torn down by `Drop`.
struct RadicaleServer {
    name: String,
    port: u16,
    /// Held for its lifetime: the bind-mounted `/config`.
    _config: tempfile::TempDir,
}

impl Drop for RadicaleServer {
    /// Teardown on success, on failure **and on panic** — a panicking test
    /// unwinds through this guard, so no path leaves a container behind.
    fn drop(&mut self) {
        let _ = Command::new("docker")
            .args(["rm", "-f", &self.name])
            .output();
    }
}

impl RadicaleServer {
    fn start() -> Self {
        let config = tempfile::tempdir().expect("a scratch dir for /config");
        write_world_readable(&config.path().join("config"), CONFIG);
        write_world_readable(&config.path().join("users"), HTPASSWD);
        // The image runs Radicale as an unprivileged uid; a 0700 tempdir would
        // make the bind-mounted /config unreadable to it.
        std::fs::set_permissions(config.path(), std::fs::Permissions::from_mode(0o755))
            .expect("the config dir is traversable by the container's user");

        let name = format!(
            "caldir-live-radicale-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("a clock after 1970")
                .as_nanos()
        );
        let port = free_port();

        // Pre-flight: clear a leftover from a run that was killed hard enough
        // to skip Drop (SIGKILL, a crashed harness).
        let _ = Command::new("docker").args(["rm", "-f", &name]).output();

        let run = Command::new("docker")
            .args([
                "run",
                "-d",
                "--name",
                &name,
                "-p",
                &format!("127.0.0.1:{port}:5232"),
                "-v",
                &format!("{}:/config:ro", config.path().display()),
                IMAGE,
            ])
            .output()
            .expect("docker run is executable");
        assert!(
            run.status.success(),
            "docker run failed: {}",
            String::from_utf8_lossy(&run.stderr)
        );

        let server = RadicaleServer {
            name,
            port,
            _config: config,
        };
        server.await_ready();
        server
    }

    /// Poll a `PROPFIND` until the server answers at all, ~10s budget.
    ///
    /// Any HTTP status counts as ready — a `2xx` or a `401` both prove the
    /// listener is up, and only a transport error means "not yet". Never a
    /// fixed sleep.
    fn await_ready(&self) {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut last = String::from("never attempted");

        while std::time::Instant::now() < deadline {
            match self.try_request("PROPFIND", "/", &[("Depth", "0")], String::new()) {
                Ok(_) => return,
                Err(err) => last = err,
            }
            std::thread::sleep(Duration::from_millis(100));
        }

        panic!(
            "Radicale did not become ready within 10s (last error: {last})\n\
             docker logs:\n{}",
            self.logs()
        );
    }

    fn logs(&self) -> String {
        Command::new("docker")
            .args(["logs", &self.name])
            .output()
            .map(|out| {
                format!(
                    "{}{}",
                    String::from_utf8_lossy(&out.stdout),
                    String::from_utf8_lossy(&out.stderr)
                )
            })
            .unwrap_or_default()
    }

    /// The base URL a caller hands to `caldav::ops`.
    fn collection_url(&self, collection: &str) -> String {
        format!("http://127.0.0.1:{}/{USERNAME}/{collection}/", self.port)
    }

    fn resource_url(&self, collection: &str, filename: &str) -> String {
        format!("{}{filename}", self.collection_url(collection))
    }

    /// Create a calendar collection, `radicale-probe.md` §Reproduction 3.
    fn mkcalendar(&self, collection: &str) {
        const BODY: &str = r#"<?xml version="1.0" encoding="utf-8" ?>
<C:mkcalendar xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <D:set><D:prop><D:displayname>Live</D:displayname></D:prop></D:set>
</C:mkcalendar>"#;

        let (status, body) = self.request(
            "MKCALENDAR",
            &url_to_href(&self.collection_url(collection)),
            &[("Content-Type", "application/xml")],
            BODY.to_string(),
        );
        assert_eq!(status, StatusCode::CREATED, "MKCALENDAR failed: {body}");
    }

    /// PUT raw ICS.
    ///
    /// **The harness does this, not `caldav::ops`.** No production path
    /// accepts a `Todo` (R70), and seeding from raw bytes is also what keeps
    /// the remote an independent store rather than the encoder under test
    /// replaying itself.
    fn put_ics(&self, collection: &str, filename: &str, ics: &str) -> (StatusCode, String) {
        self.request(
            "PUT",
            &url_to_href(&self.resource_url(collection, filename)),
            &[("Content-Type", "text/calendar")],
            ics.to_string(),
        )
    }

    /// GET one resource's bytes exactly as the server holds them.
    fn get_ics(&self, collection: &str, filename: &str) -> String {
        let (status, body) = self.request(
            "GET",
            &url_to_href(&self.resource_url(collection, filename)),
            &[],
            String::new(),
        );
        assert!(status.is_success(), "GET {filename} -> {status}: {body}");
        body
    }

    /// Every resource href directly inside a collection, via
    /// `PROPFIND Depth: 1`, **percent-decoded**.
    ///
    /// Radicale escapes the `@` of a UID-derived filename as `%40` in the
    /// href it reports; decoding keeps the assertion about the *identity* in
    /// the href rather than about the server's escaping.
    fn resource_hrefs(&self, collection: &str) -> Vec<String> {
        let collection_href = url_to_href(&self.collection_url(collection));
        let (status, body) = self.request(
            "PROPFIND",
            &collection_href,
            &[("Depth", "1"), ("Content-Type", "application/xml")],
            String::new(),
        );
        assert!(status.is_success(), "PROPFIND -> {status}: {body}");

        let doc = roxmltree::Document::parse(&body).expect("a multistatus document");
        doc.descendants()
            .filter(|node| node.tag_name().name() == "href")
            .filter_map(|node| node.text())
            .map(str::to_string)
            .map(|href| percent_decode(&href))
            .filter(|href| href.ends_with(".ics"))
            .collect()
    }

    fn request(
        &self,
        method: &str,
        href: &str,
        headers: &[(&str, &str)],
        body: String,
    ) -> (StatusCode, String) {
        self.try_request(method, href, headers, body)
            .unwrap_or_else(|err| panic!("{method} {href} failed: {err}"))
    }

    /// One HTTP request, spoken by `curl`.
    ///
    /// **Deliberately not caldir's own client.** A harness that drove the
    /// remote through `create_caldav_client` would cancel a transport bug out
    /// on both sides — the remote has to be an independent store, which is
    /// what a remote *is*. It also keeps `caldav::ops::put_ics` the crate's
    /// only `request_raw` site (R64/R80), which
    /// `ops::tests::put_ics_is_the_only_raw_sender_in_the_crate` scans the
    /// source tree for.
    ///
    /// `curl` is the same client `radicale-probe.md` §Reproduction uses, so
    /// the commands below are the recorded ones.
    fn try_request(
        &self,
        method: &str,
        href: &str,
        headers: &[(&str, &str)],
        body: String,
    ) -> Result<(StatusCode, String), String> {
        let out = tempfile::NamedTempFile::new().map_err(|err| err.to_string())?;
        let mut command = Command::new("curl");
        command
            .args(["-s", "-o"])
            .arg(out.path())
            .args(["-w", "%{http_code}"])
            // Literal, never a shell variable: zsh does not word-split an
            // unquoted parameter expansion, which cost the original probe two
            // 401s.
            .args(["-u", &format!("{USERNAME}:{PASSWORD}")])
            .args(["-X", method]);
        for (name, value) in headers {
            command.args(["-H", &format!("{name}: {value}")]);
        }

        let input = if body.is_empty() {
            None
        } else {
            let mut input = tempfile::NamedTempFile::new().map_err(|err| err.to_string())?;
            input
                .write_all(body.as_bytes())
                .map_err(|err| err.to_string())?;
            input.flush().map_err(|err| err.to_string())?;
            command.arg("--data-binary");
            command.arg(format!("@{}", input.path().display()));
            Some(input)
        };

        command.arg(format!("http://127.0.0.1:{}{href}", self.port));

        let result = command.output().map_err(|err| err.to_string())?;
        drop(input);
        if !result.status.success() {
            return Err(format!(
                "curl exited {}: {}",
                result.status,
                String::from_utf8_lossy(&result.stderr)
            ));
        }

        let code: u16 = String::from_utf8_lossy(&result.stdout)
            .trim()
            .parse()
            .map_err(|_| {
                format!(
                    "curl reported no status: {}",
                    String::from_utf8_lossy(&result.stdout)
                )
            })?;
        let status = StatusCode::from_u16(code).map_err(|err| err.to_string())?;
        let response = std::fs::read(out.path()).map_err(|err| err.to_string())?;

        Ok((status, String::from_utf8_lossy(&response).into_owned()))
    }
}

/// Decode `%XX` escapes in a href. Enough for the ASCII a UID produces.
fn percent_decode(href: &str) -> String {
    let bytes = href.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let Ok(text) = std::str::from_utf8(&bytes[index + 1..index + 3])
            && let Ok(byte) = u8::from_str_radix(text, 16)
        {
            out.push(byte);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }

    String::from_utf8_lossy(&out).into_owned()
}

fn write_world_readable(path: &Path, contents: &str) {
    let mut file = std::fs::File::create(path).expect("a writable scratch file");
    file.write_all(contents.as_bytes())
        .expect("the write lands");
    drop(file);
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o644))
        .expect("the container's user can read it");
}

/// An ephemeral loopback port, claimed by binding and releasing it. Safe
/// because every live test is `#[serial_test::serial(radicale)]`.
fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("a free loopback port")
        .local_addr()
        .expect("the bound address")
        .port()
}

/// Run a future to completion on a fresh current-thread runtime.
///
/// The harness is synchronous so that `Drop` teardown is not entangled with an
/// async runtime's shutdown.
fn block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a current-thread runtime")
        .block_on(future)
}

// ============================================================================
// Fixtures — raw ICS, never domain values re-encoded by the code under test
// ============================================================================

/// The Stage 2 `u_crawler` fixture, `docs/vtodo/reproduction.md` §1.
const U_CRAWLER_TASK_ICS: &str = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//u_crawler//EN\r\n\
CALSCALE:GREGORIAN\r\n\
BEGIN:VTODO\r\n\
UID:assignment-123@u-crawler\r\n\
DTSTAMP:20260801T090000Z\r\n\
DUE:20260815T235900Z\r\n\
SUMMARY:Assignment 3 - Recursive descent parser\r\n\
DESCRIPTION:Submit the parser and a short report via the course portal.\r\n\
URL:https://example.edu/courses/cs101/assignments/123\r\n\
PRIORITY:1\r\n\
STATUS:COMPLETED\r\n\
COMPLETED:20260814T120000Z\r\n\
END:VTODO\r\n\
END:VCALENDAR\r\n";

const U_CRAWLER_UID: &str = "assignment-123@u-crawler";

/// Probe fixture `t2`: the shape RFC 4791 §9.9 omits from a time-range query.
///
/// **No `DTSTART`, no `DUE`, and deliberately no `CREATED`.** §9.9's last row
/// rescues a task carrying `CREATED` via `end > CREATED`, which would make
/// [`an_undated_completed_task_survives_a_pull`] permanently green and blind.
/// The vulnerable shape is a task whose only date property is a `COMPLETED`
/// older than the window start.
const UNDATED_COMPLETED_TASK_ICS: &str = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//u_crawler//EN\r\n\
BEGIN:VTODO\r\n\
UID:archived-1@u-crawler\r\n\
DTSTAMP:20200101T000000Z\r\n\
SUMMARY:Filed years ago\r\n\
STATUS:COMPLETED\r\n\
COMPLETED:20200101T000000Z\r\n\
END:VTODO\r\n\
END:VCALENDAR\r\n";

const UNDATED_COMPLETED_UID: &str = "archived-1@u-crawler";

/// A zoned `DUE` and **no `VTIMEZONE`** — exactly what caldir emits (Q-3).
const ZONED_TASK_ICS: &str = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//u_crawler//EN\r\n\
BEGIN:VTODO\r\n\
UID:zoned-1@u-crawler\r\n\
DTSTAMP:20260801T090000Z\r\n\
DUE;TZID=Europe/Stockholm:20260815T235900\r\n\
SUMMARY:Zoned deadline\r\n\
STATUS:NEEDS-ACTION\r\n\
END:VTODO\r\n\
END:VCALENDAR\r\n";

/// A task rich enough that a dropped property shows up as a set difference.
const RICH_TASK_ICS: &str = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//u_crawler//EN\r\n\
BEGIN:VTODO\r\n\
UID:rich-1@u-crawler\r\n\
DTSTAMP:20260801T090000Z\r\n\
CREATED:20260701T080000Z\r\n\
DTSTART:20260810T090000Z\r\n\
DUE:20260815T235900Z\r\n\
SUMMARY:Rich task\r\n\
DESCRIPTION:Every property here must survive the server.\r\n\
LOCATION:Room 3\r\n\
URL:https://example.edu/rich\r\n\
PRIORITY:2\r\n\
PERCENT-COMPLETE:40\r\n\
STATUS:IN-PROCESS\r\n\
SEQUENCE:3\r\n\
ORGANIZER:mailto:teacher@example.edu\r\n\
ATTENDEE:mailto:student@example.edu\r\n\
ATTACH:https://example.edu/rich/brief.pdf\r\n\
X-CALDIR-KEEP:kept\r\n\
END:VTODO\r\n\
END:VCALENDAR\r\n";

/// The VEVENT control for the rename gate. Raw ICS for the same reason.
const CONTROL_EVENT_ICS: &str = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//u_crawler//EN\r\n\
BEGIN:VEVENT\r\n\
UID:window-123@u-crawler\r\n\
DTSTAMP:20260801T090000Z\r\n\
DTSTART:20260815T090000Z\r\n\
DTEND:20260815T100000Z\r\n\
SUMMARY:Assignment 3 window\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

const CONTROL_EVENT_UID: &str = "window-123@u-crawler";

/// A `VEVENT` carrying the *task's* UID: the same-UID pair of R65, and the
/// shape `event_url` sends straight onto the task's href.
const SAME_UID_EVENT_ICS: &str = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//u_crawler//EN\r\n\
BEGIN:VEVENT\r\n\
UID:assignment-123@u-crawler\r\n\
DTSTAMP:20260801T090000Z\r\n\
DTSTART:20260815T090000Z\r\n\
DTEND:20260815T100000Z\r\n\
SUMMARY:Event that must not replace the task\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

// ============================================================================
// Helpers over the production pull path
// ============================================================================

/// `ops::fetch_events` over "today ±365 days", the CLI's default window.
fn pull(server: &RadicaleServer, collection: &str) -> Vec<CalendarItem> {
    let now = Utc::now();
    let from = (now - ChronoDuration::days(365)).to_rfc3339_opts(SecondsFormat::Secs, true);
    let to = (now + ChronoDuration::days(365)).to_rfc3339_opts(SecondsFormat::Secs, true);

    block_on(ops::fetch_events(
        USERNAME,
        PASSWORD,
        &server.collection_url(collection),
        &from,
        &to,
    ))
    .expect("the listing succeeds")
}

fn task_with_uid(items: &[CalendarItem], uid: &str) -> Todo {
    let item = items
        .iter()
        .find(|item| item.uid().as_str() == uid)
        .unwrap_or_else(|| {
            let seen: Vec<&str> = items.iter().map(|item| item.uid().as_str()).collect();
            panic!("no item with uid {uid}; the listing held {seen:?}")
        });

    match item {
        CalendarItem::Todo(todo) => todo.clone(),
        // R1: a task pulled back as an `Event` is the whole defect this
        // feature exists to fix.
        CalendarItem::Event(event) => panic!("{uid} came back as an Event, not a Todo: {event:?}"),
        other => panic!("{uid} came back as neither an Event nor a Todo: {other:?}"),
    }
}

// ============================================================================
// T-53
// ============================================================================

/// **T-53** — A-1 steps 1–4. The Stage 2 `u_crawler` task survives a full trip
/// through a real Radicale and comes back as a `Todo`, not an `Event` (R1).
///
/// The trip is: raw fixture → Radicale → `ops::fetch_events` (caldir's parser,
/// over the wire) → `Todo::to_ics_string` (caldir's serializer) → Radicale →
/// `ops::fetch_events` again. Both legs are compared as **parsed domain
/// values**; nothing here compares PUT bytes against GET bytes (R30), and
/// nothing looks at an ETag (R31).
///
/// **Predates the task write path.** The two PUTs here are issued by this
/// harness, not by `ops`. What is proven by this gate is caldir's **pull**
/// path and its codec against a real server; the write path is proven
/// separately by the production-write-path gates below.
#[test]
#[serial_test::serial(radicale)]
fn a_u_crawler_task_round_trips_through_radicale_preserving_identity_and_semantics() {
    let Some(server) = live_gate(
        "a_u_crawler_task_round_trips_through_radicale_preserving_identity_and_semantics",
    ) else {
        return;
    };

    server.mkcalendar("deadlines");
    let filename = format!("{U_CRAWLER_UID}.ics");
    let (status, body) = server.put_ics("deadlines", &filename, U_CRAWLER_TASK_ICS);
    assert_eq!(status, StatusCode::CREATED, "seeding the fixture: {body}");

    let pulled = task_with_uid(&pull(&server, "deadlines"), U_CRAWLER_UID);

    assert_eq!(pulled.uid.as_str(), U_CRAWLER_UID);
    assert_eq!(
        pulled.due,
        Some(EventTime::DateTimeUtc(
            "2026-08-15T23:59:00Z".parse().expect("an RFC3339 instant")
        ))
    );
    assert_eq!(
        pulled.summary.as_deref(),
        Some("Assignment 3 - Recursive descent parser")
    );
    assert_eq!(
        pulled.description.as_deref(),
        Some("Submit the parser and a short report via the course portal.")
    );
    assert_eq!(
        pulled.url.as_deref(),
        Some("https://example.edu/courses/cs101/assignments/123")
    );
    assert_eq!(pulled.priority, Some(1));
    assert_eq!(pulled.status, TodoStatus::Completed);
    assert_eq!(
        pulled.completed,
        Some(EventTime::DateTimeUtc(
            "2026-08-14T12:00:00Z".parse().expect("an RFC3339 instant")
        ))
    );

    // Second leg: what caldir would write back is accepted by the server and
    // parses back to the same task. Domain `==`, not bytes.
    let (status, body) = server.put_ics("deadlines", &filename, &pulled.to_ics_string());
    assert!(
        status.is_success(),
        "Radicale rejected caldir's own serialization ({status}): {body}"
    );

    let again = task_with_uid(&pull(&server, "deadlines"), U_CRAWLER_UID);
    assert_eq!(
        again, pulled,
        "a task caldir re-serialized and stored must parse back identically"
    );
}

// ============================================================================
// T-54
// ============================================================================

/// **T-54** — the RFC 4791 §9.9 regression lock, in two halves.
///
/// *The request half, no server needed:* the VTODO REPORT body caldir puts on
/// the wire carries no `<C:time-range>`. This is the assertion that fails the
/// moment one is reintroduced, wherever it is reintroduced, because it reads
/// the body back out of `prepare_request`.
///
/// *The server half:* Radicale implements §9.9 faithfully — a task whose only
/// date property is a `COMPLETED` older than the window start evaluates FALSE
/// and is omitted from a time-range query. With the bare VTODO filter it comes
/// back, so the pull sees it. If a time-range ever returns, the remote map
/// loses this entry and `calendar_diff.rs` emits an **incoming delete against
/// the user's file**.
///
/// The fixture carries no `CREATED` on purpose — see
/// [`UNDATED_COMPLETED_TASK_ICS`].
#[test]
#[serial_test::serial(radicale)]
fn an_undated_completed_task_survives_a_pull() {
    // The request-body half runs everywhere, gate or no gate.
    let bodies = crate::caldav::ops::list_events::report_request_bodies(
        "/testuser/deadlines/",
        "20250825T000000Z",
        "20270825T000000Z",
    );
    let task_query = bodies
        .iter()
        .find(|body| body.contains(r#"comp-filter name="VTODO""#))
        .expect("one of the two REPORTs filters on VTODO");
    assert!(
        !task_query.contains("time-range"),
        "RFC 4791 §9.9: the VTODO calendar-query MUST carry no <C:time-range> \
         — a compliant server omits an undated task whose only date property \
         is an old COMPLETED, and core reads that absence as a delete of the \
         user's file. Got:\n{task_query}"
    );

    let Some(server) = live_gate("an_undated_completed_task_survives_a_pull") else {
        return;
    };

    server.mkcalendar("deadlines");
    let (status, body) = server.put_ics("deadlines", "archived-1.ics", UNDATED_COMPLETED_TASK_ICS);
    assert_eq!(status, StatusCode::CREATED, "seeding the fixture: {body}");

    let items = pull(&server, "deadlines");
    let uids: Vec<&str> = items.iter().map(|item| item.uid().as_str()).collect();

    assert!(
        uids.contains(&UNDATED_COMPLETED_UID),
        "the undated COMPLETED task must be in the listing; a <C:time-range> \
         on the VTODO query makes Radicale omit it and core delete the user's \
         file. Got {uids:?}"
    );
    assert_eq!(
        task_with_uid(&items, UNDATED_COMPLETED_UID).status,
        TodoStatus::Completed
    );
}

// ============================================================================
// T-55
// ============================================================================

/// **T-55** — the one legal byte comparison in this module.
///
/// Both sides are **server-normalized output** (R30's second clause): PUT,
/// `GET` (pass 1), PUT the same bytes again, `GET` (pass 2), assert pass 2 ==
/// pass 1. Neither pass is ever compared against what was PUT — Radicale sorts
/// properties alphabetically, re-folds at 75 octets and synthesizes a
/// `VTIMEZONE`, so that comparison is permanently red. No ETag is consulted
/// (R31): caldir re-stamps `DTSTAMP` on every serialize, so an ETag can only
/// ever prove "the remote changed".
///
/// The fixture is zoned and carries **no `VTIMEZONE`**, which is what caldir
/// emits. The extra assertions below are the Q-3 evidence: Radicale accepts
/// that document, adds a `VTIMEZONE` sibling, and caldir parses the result
/// back to the same zoned `DUE`.
#[test]
#[serial_test::serial(radicale)]
fn get_pass_two_equals_get_pass_one() {
    let Some(server) = live_gate("get_pass_two_equals_get_pass_one") else {
        return;
    };

    server.mkcalendar("deadlines");

    let (status, body) = server.put_ics("deadlines", "zoned-1.ics", ZONED_TASK_ICS);
    assert_eq!(
        status,
        StatusCode::CREATED,
        "Q-3: Radicale must accept a zoned DUE with no VTIMEZONE: {body}"
    );
    let pass_one = server.get_ics("deadlines", "zoned-1.ics");

    let (status, body) = server.put_ics("deadlines", "zoned-1.ics", ZONED_TASK_ICS);
    assert!(status.is_success(), "the second PUT: {status} {body}");
    let pass_two = server.get_ics("deadlines", "zoned-1.ics");

    assert_eq!(
        pass_two, pass_one,
        "the server's normalized form must be a fixed point"
    );

    // Q-3 evidence, recorded in the module doc comment.
    assert!(
        pass_one.contains("BEGIN:VTIMEZONE"),
        "R29/R67: Radicale synthesizes a VTIMEZONE for a TZID; got:\n{pass_one}"
    );
    assert!(
        !ZONED_TASK_ICS.contains("VTIMEZONE"),
        "the fixture must mirror caldir, which emits no VTIMEZONE"
    );

    // …and caldir tolerates it: the two-component document still parses, and
    // the zone survives.
    let pulled = task_with_uid(&pull(&server, "deadlines"), "zoned-1@u-crawler");
    assert_eq!(
        pulled.due,
        Some(EventTime::DateTimeZoned {
            datetime: "2026-08-15T23:59:00"
                .parse()
                .expect("a naive local datetime"),
            tzid: "Europe/Stockholm".to_string(),
        }),
        "R18/R102: a synthesized VTIMEZONE must not disturb the zoned DUE"
    );
}

// ============================================================================
// T-56
// ============================================================================

/// **T-56** — R65 / `L5-04`. A `VEVENT` and a `VTODO` sharing one UID in one
/// collection surfaces a caldir-level error, and the resource already there is
/// **unchanged**.
///
/// The collection holds the task at `{uid}.ics`. caldir then pushes an event
/// with the same UID; `event_url` is UID-derived, so it addresses that same
/// href, and Q-E.2 measured Radicale silently `204`-overwriting it. What stops
/// the task being destroyed is the `If-None-Match: *` on the create: the
/// server answers `412` and `create_event` bails. Deleting that one header
/// turns the create into a blind overwrite, and this gate is what catches it.
///
/// Asserted as an **outcome** (an error, and the task still intact), never as
/// a status code.
///
/// **Scope.** The push under test here is the *event*; the mirror-image
/// case — a task push landing on an event's href — is reachable now that
/// tasks have a write path, but is not exercised by this gate.
#[test]
#[serial_test::serial(radicale)]
fn pushing_a_same_uid_pair_into_one_collection_surfaces_an_error() {
    let Some(server) = live_gate("pushing_a_same_uid_pair_into_one_collection_surfaces_an_error")
    else {
        return;
    };

    server.mkcalendar("deadlines");

    // The task that must survive, at the href the event push will address.
    let task_ics = U_CRAWLER_TASK_ICS;
    let filename = format!("{U_CRAWLER_UID}.ics");
    let (status, body) = server.put_ics("deadlines", &filename, task_ics);
    assert_eq!(status, StatusCode::CREATED, "seeding the task: {body}");
    let before = server.get_ics("deadlines", &filename);

    // An event sharing that UID, parsed from raw ICS and pushed through the
    // production create path.
    let CalendarItem::Event(event) = CalendarItem::from_single_ics_str(SAME_UID_EVENT_ICS)
        .expect("the colliding fixture parses")
    else {
        panic!("the colliding fixture is a VEVENT");
    };

    let err = block_on(ops::create_event(
        USERNAME,
        PASSWORD,
        &server.collection_url("deadlines"),
        event,
    ))
    .expect_err(
        "R65: pushing an event onto a UID a task already holds must fail, not \
         silently overwrite the task",
    );
    let rendered = format!("{err:#}");
    assert!(
        rendered.contains("Failed to create event"),
        "the failure must be caldir's, got: {rendered}"
    );

    // The outcome that matters: the task is still there, byte-for-byte as the
    // server held it before the push. Both sides are server output, so this is
    // not a PUT-vs-GET comparison.
    let after = server.get_ics("deadlines", &filename);
    assert_eq!(after, before, "the pre-existing task must be untouched");

    let pulled = task_with_uid(&pull(&server, "deadlines"), U_CRAWLER_UID);
    assert_eq!(
        pulled.summary.as_deref(),
        Some("Assignment 3 - Recursive descent parser"),
        "the task, not the event, must still own this UID"
    );
}

// ============================================================================
// T-57
// ============================================================================

/// **T-57** — R67 / spec §17 A-3. Radicale injects **no** non-`X-` property
/// onto a `VTODO`, and drops none.
///
/// A set difference over the property **names** inside the `VTODO` component,
/// pushed versus fetched. The sibling `VTIMEZONE` is a *component*, not a
/// property, and [`vtodo_property_names`] never looks outside the `VTODO`, so
/// it is correctly not counted.
///
/// This is the gate behind ADR-05's "passthrough is `X-*` only": if a server
/// ever did inject something, caldir would drop it on the next whole-resource
/// PUT and destroy correct server data.
#[test]
#[serial_test::serial(radicale)]
fn the_server_injects_no_non_x_properties_onto_a_task() {
    let Some(server) = live_gate("the_server_injects_no_non_x_properties_onto_a_task") else {
        return;
    };

    server.mkcalendar("deadlines");
    let (status, body) = server.put_ics("deadlines", "rich-1.ics", RICH_TASK_ICS);
    assert_eq!(status, StatusCode::CREATED, "seeding the fixture: {body}");

    let pushed = vtodo_property_names(RICH_TASK_ICS);
    let fetched = vtodo_property_names(&server.get_ics("deadlines", "rich-1.ics"));

    // Non-vacuity: two empty sets have an empty difference. If
    // `vtodo_property_names` ever stopped seeing properties, the two
    // assertions below would go permanently, silently green.
    for expected in [
        "UID",
        "SUMMARY",
        "DUE",
        "DTSTART",
        "PERCENT-COMPLETE",
        "ATTACH",
        "ORGANIZER",
        "X-CALDIR-KEEP",
    ] {
        assert!(
            pushed.iter().any(|name| name == expected),
            "the fixture's own {expected} must be seen, else this gate is \
             vacuous; got {pushed:?}"
        );
    }

    // Non-vacuity: two empty sets have an empty difference. If
    // `vtodo_property_names` ever stopped seeing properties, the two
    // assertions below would go permanently, silently green.
    for expected in [
        "UID",
        "SUMMARY",
        "DUE",
        "DTSTART",
        "PERCENT-COMPLETE",
        "ATTACH",
        "ORGANIZER",
        "X-CALDIR-KEEP",
    ] {
        assert!(
            pushed.iter().any(|name| name == expected),
            "the fixture's own {expected} must be seen, else this gate is \
             vacuous; got {pushed:?}"
        );
    }

    let added: Vec<&String> = fetched
        .iter()
        .filter(|name| !pushed.contains(name))
        .collect();
    let removed: Vec<&String> = pushed
        .iter()
        .filter(|name| !fetched.contains(name))
        .collect();

    assert!(
        added.is_empty(),
        "R67: the server must inject no property onto a VTODO, got {added:?}\n\
         pushed: {pushed:?}\nfetched: {fetched:?}"
    );
    assert!(
        removed.is_empty(),
        "the server must drop no property from a VTODO, got {removed:?}\n\
         pushed: {pushed:?}\nfetched: {fetched:?}"
    );
}

// ============================================================================
// T-58 (renamed — see the refusal note)
// ============================================================================

/// **T-58, renamed.** A-9 / R103: a local `.ics` rename never creates a second
/// resource for the same UID; the push updates the one already there.
///
/// **The name `renaming_a_local_task_file_updates_the_same_resource` is now
/// reachable and is still declined, deliberately.** A task push now reaches a
/// real `PUT` through `ops::update_event::update_item`, so the mechanism
/// could be re-run on a `Todo`. It is not, because the href in both cases
/// comes from `event_url(calendar_url, uid)` alone — the local filename never
/// crosses the provider boundary for either kind (see below) — so a
/// task-flavoured copy of this gate would re-run the identical assertion
/// against the identical function and prove nothing a VEVENT run does not
/// already prove. `updating_summary_due_and_status_updates_the_one_resource`
/// separately pins the href-stability invariant for tasks under an update.
///
/// What transfers to tasks, and what does not: the href comes from
/// `event_url(calendar_url, uid)`, which takes the UID and nothing else — the
/// local filename never crosses the provider boundary at all, for either kind.
/// That is **kind-independent by construction**, not by measurement here.
///
/// Asserted as an outcome — one resource, right content — never as an HTTP
/// status: the reachable `409` trigger is a server-assigned href, not a
/// rename.
#[test]
#[serial_test::serial(radicale)]
fn renaming_a_local_file_updates_the_same_resource_and_never_a_second_one() {
    let Some(server) =
        live_gate("renaming_a_local_file_updates_the_same_resource_and_never_a_second_one")
    else {
        return;
    };

    server.mkcalendar("windows");
    let collection_url = server.collection_url("windows");

    // Seed through the production create path, from raw ICS parsed into the
    // domain — the remote is never handed a value the encoder invented.
    let CalendarItem::Event(created) =
        CalendarItem::from_single_ics_str(CONTROL_EVENT_ICS).expect("the control fixture parses")
    else {
        panic!("the control fixture is a VEVENT");
    };
    block_on(ops::create_event(
        USERNAME,
        PASSWORD,
        &collection_url,
        created.clone(),
    ))
    .expect("the create lands");

    let hrefs_before = server.resource_hrefs("windows");
    // The mechanism, asserted as an outcome — what the collection holds, never
    // an HTTP status. `event_url` takes the UID and nothing else, so the href
    // is `{uid}.ics` whatever the local file is called. Without this line the
    // gate survives an href derived from a clock, because
    // `find_event_location`'s REPORT fallback silently rescues it.
    assert_eq!(
        hrefs_before,
        vec![url_to_href(&server.resource_url(
            "windows",
            &format!("{CONTROL_EVENT_UID}.ics")
        ))],
        "R103: the href is UID-derived; the local filename never reaches the \
         server"
    );

    // The rename: the local file is now `renamed.ics`. Nothing about that
    // reaches the provider, which is exactly the property under test. The push
    // that follows a rename is an update.
    let mut renamed = created.clone();
    renamed.summary = Some("Assignment 3 window (renamed locally)".to_string());
    block_on(ops::update_event(
        USERNAME,
        PASSWORD,
        &collection_url,
        renamed.clone(),
    ))
    .expect("the post-rename push lands");

    let hrefs_after = server.resource_hrefs("windows");
    assert_eq!(
        hrefs_after, hrefs_before,
        "R103: a rename must update the same resource, never add a second one"
    );

    let items = pull(&server, "windows");
    let matching: Vec<&CalendarItem> = items
        .iter()
        .filter(|item| item.uid().as_str() == CONTROL_EVENT_UID)
        .collect();
    assert_eq!(matching.len(), 1, "exactly one item holds the UID");

    let CalendarItem::Event(fetched) = matching[0] else {
        panic!("the control must come back as an Event");
    };
    assert_eq!(
        fetched.summary.as_deref(),
        Some("Assignment 3 window (renamed locally)"),
        "the resource must hold the post-rename content"
    );
    assert_eq!(fetched.start, renamed.start);
    assert_eq!(fetched.end, renamed.end);
}

// ============================================================================
// Mixed-component behaviour survives the write paths
// ============================================================================

/// The UID a sibling task uses in
/// [`task_writes_leave_a_sibling_event_in_the_same_collection_untouched`] —
/// deliberately not the control event's UID: these two are not the
/// same-UID case, just neighbours in one collection.
const SIBLING_TASK_UID: &str = "sibling-task-1@u-crawler";

/// A task create, update and delete, all through the
/// production write paths, leave a sibling `VEVENT` in the same collection
/// byte-identical on the server.
///
/// A task reaches a `PUT` or `DELETE` the same way an event does: both
/// compute an href from a UID and locate resources by REPORT, both
/// collection-wide operations, and a mis-scoped one takes the neighbour with
/// it. This is a regression lock, not a claim about production shape: caldir
/// never puts a task and an event in the same directory in practice.
///
/// R30 binding: only server output is compared (a `get_ics` against an
/// earlier `get_ics`), never a PUT against a GET.
#[test]
#[serial_test::serial(radicale)]
fn task_writes_leave_a_sibling_event_in_the_same_collection_untouched() {
    let Some(server) =
        live_gate("task_writes_leave_a_sibling_event_in_the_same_collection_untouched")
    else {
        return;
    };

    server.mkcalendar("deadlines");
    let collection_url = server.collection_url("deadlines");

    // The sibling VEVENT, seeded through the production create path.
    let CalendarItem::Event(event) =
        CalendarItem::from_single_ics_str(CONTROL_EVENT_ICS).expect("the control fixture parses")
    else {
        panic!("the control fixture is a VEVENT");
    };
    block_on(ops::create_event(
        USERNAME,
        PASSWORD,
        &collection_url,
        event,
    ))
    .expect("the sibling event create lands");

    let event_filename = format!("{CONTROL_EVENT_UID}.ics");
    let baseline = server.get_ics("deadlines", &event_filename);

    let event_is_untouched = |server: &RadicaleServer| {
        assert_eq!(
            server.get_ics("deadlines", &event_filename),
            baseline,
            "a task write must not disturb the sibling event's resource"
        );
        let CalendarItem::Event(fetched) = pull(server, "deadlines")
            .into_iter()
            .find(|item| item.uid().as_str() == CONTROL_EVENT_UID)
            .unwrap_or_else(|| panic!("the sibling event must still be in the listing"))
        else {
            panic!("the sibling event must still come back as an Event");
        };
        assert_eq!(fetched.summary.as_deref(), Some("Assignment 3 window"));
    };

    // Create a task with a *different* UID through the production path.
    let mut todo = Todo::new("Sibling task");
    todo.uid = EventUid::new(SIBLING_TASK_UID);
    block_on(crate::caldav::ops::create_event::create_item(
        USERNAME,
        PASSWORD,
        &collection_url,
        CalendarItem::Todo(todo.clone()),
    ))
    .expect("the task create lands");
    event_is_untouched(&server);

    // Update the task through the production path.
    todo.summary = Some("Sibling task, updated".to_string());
    block_on(crate::caldav::ops::update_event::update_item(
        USERNAME,
        PASSWORD,
        &collection_url,
        CalendarItem::Todo(todo.clone()),
    ))
    .expect("the task update lands");
    event_is_untouched(&server);

    // Delete the task through the production path.
    block_on(ops::delete_event(
        USERNAME,
        PASSWORD,
        &collection_url,
        SIBLING_TASK_UID,
        ComponentKind::Todo,
    ))
    .expect("the task delete lands");
    event_is_untouched(&server);
}

/// A `VEVENT` and a `VTODO` sharing one UID, in separate
/// collections, stay distinct through a task create and update via the
/// production path.
///
/// Separate collections is the *deliberate* shape here: production's
/// task and its window event never share a UID and live in different
/// calendar directories, so §8.5's same-UID pair is synthetic, not something
/// production exercises. It is pinned anyway because it is the case that
/// silently destroys data if identity resolution regresses. One collection is
/// deliberately not attempted: Radicale enforces UID uniqueness per
/// collection component-agnostically, and the shipped
/// [`pushing_a_same_uid_pair_into_one_collection_surfaces_an_error`] already
/// records that a same-UID push there is an error, not a second resource.
#[test]
#[serial_test::serial(radicale)]
fn a_same_uid_event_and_task_in_separate_collections_stay_distinct_through_a_task_write() {
    let Some(server) = live_gate(
        "a_same_uid_event_and_task_in_separate_collections_stay_distinct_through_a_task_write",
    ) else {
        return;
    };

    server.mkcalendar("windows");
    server.mkcalendar("deadlines");

    // The event, in `windows`, seeded through the production create path.
    let CalendarItem::Event(event) = CalendarItem::from_single_ics_str(SAME_UID_EVENT_ICS)
        .expect("the colliding event fixture parses")
    else {
        panic!("the colliding fixture is a VEVENT");
    };
    block_on(ops::create_event(
        USERNAME,
        PASSWORD,
        &server.collection_url("windows"),
        event,
    ))
    .expect("the event create lands");

    // The task sharing that UID, in `deadlines`, created then updated through
    // the production path.
    let CalendarItem::Todo(todo) =
        CalendarItem::from_single_ics_str(U_CRAWLER_TASK_ICS).expect("the task fixture parses")
    else {
        panic!("the task fixture is a VTODO");
    };
    block_on(crate::caldav::ops::create_event::create_item(
        USERNAME,
        PASSWORD,
        &server.collection_url("deadlines"),
        CalendarItem::Todo(todo.clone()),
    ))
    .expect("the task create lands");

    let mut updated = todo.clone();
    updated.summary = Some("Deadline, updated".to_string());
    block_on(crate::caldav::ops::update_event::update_item(
        USERNAME,
        PASSWORD,
        &server.collection_url("deadlines"),
        CalendarItem::Todo(updated),
    ))
    .expect("the task update lands");

    let task_hrefs = server.resource_hrefs("deadlines");
    assert_eq!(
        task_hrefs.len(),
        1,
        "the task's collection must hold exactly one resource, got {task_hrefs:?}"
    );

    let event_hrefs = server.resource_hrefs("windows");
    assert_eq!(
        event_hrefs.len(),
        1,
        "the event's collection must be unchanged, got {event_hrefs:?}"
    );

    let windows_items = pull(&server, "windows");
    let CalendarItem::Event(fetched_event) = windows_items
        .iter()
        .find(|item| item.uid().as_str() == U_CRAWLER_UID)
        .unwrap_or_else(|| panic!("the event is still in its own collection"))
    else {
        panic!("the windows collection must still hold an Event for this UID");
    };
    assert_eq!(
        fetched_event.summary.as_deref(),
        Some("Event that must not replace the task")
    );

    let fetched_task = task_with_uid(&pull(&server, "deadlines"), U_CRAWLER_UID);
    assert_eq!(fetched_task.summary.as_deref(), Some("Deadline, updated"));
}

// ============================================================================
// The release gate — production write paths against real Radicale
// ============================================================================

/// Parse a single-`VTODO` document into the domain `Todo`. Panics unless the
/// document holds exactly one `VTODO` — every fixture below does.
fn todo_from_ics(ics: &str) -> Todo {
    match CalendarItem::from_single_ics_str(ics).expect("the fixture parses") {
        CalendarItem::Todo(todo) => todo,
        other => panic!("the fixture must be a VTODO, got {other:?}"),
    }
}

/// `deadlines/assignment-1562539.ics`, byte-identical to
/// `caldir_core::test_utils::VTODO_COMPLETED_PRODUCTION_ICS`. Copied
/// rather than imported: the core fixture lives behind a `#[cfg(test)]`
/// private module and cannot cross the crate boundary (the same reason
/// `ops::create_event` copies `P1_05_OPEN_TASK_ICS` in). This is
/// production's completed-task shape: `STATUS:COMPLETED` alone, with
/// **neither** `COMPLETED` nor `PERCENT-COMPLETE` — the R68 Radicale case.
const P1_05_COMPLETED_TASK_ICS: &str = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//u_crawler//calendar-sync//EN\r\n\
BEGIN:VTODO\r\n\
UID:u_crawler-todo-9002295@u-crawler.local\r\n\
DTSTAMP:20260701T040020Z\r\n\
DUE:20260708T035959Z\r\n\
PRIORITY:1\r\n\
STATUS:COMPLETED\r\n\
SUMMARY:Sube aquí tu Certificado: Mentalidad digital\r\n\
URL:https://lms.example.edu/courses/900222/assignments/9002295\r\n\
END:VTODO\r\n\
END:VCALENDAR\r\n";

/// **R95 clauses 2–3.** The Stage 2 `u_crawler` task, pushed
/// through caldir's own production create path for the first time in the
/// project's history — every earlier live gate seeded a task with the
/// harness's `curl` because no task could reach a `PUT` through production
/// code until now. The stored resource is a single-component `VTODO` at
/// `{uid}.ics`, and `pull` reads back every property R95 clause 2 names.
#[test]
#[serial_test::serial(radicale)]
fn a_u_crawler_task_is_created_on_radicale_through_the_production_write_path() {
    let Some(server) =
        live_gate("a_u_crawler_task_is_created_on_radicale_through_the_production_write_path")
    else {
        return;
    };

    server.mkcalendar("deadlines");
    let collection_url = server.collection_url("deadlines");

    let todo = todo_from_ics(U_CRAWLER_TASK_ICS);
    block_on(ops::create_event::create_item(
        USERNAME,
        PASSWORD,
        &collection_url,
        CalendarItem::Todo(todo),
    ))
    .expect("the production create lands");

    let filename = format!("{U_CRAWLER_UID}.ics");
    assert_eq!(
        server.resource_hrefs("deadlines"),
        vec![url_to_href(&server.resource_url("deadlines", &filename))],
        "the collection must hold exactly one resource, at {{uid}}.ics"
    );

    let stored = server.get_ics("deadlines", &filename);
    assert!(
        stored.contains("BEGIN:VTODO"),
        "the stored resource must be a VTODO, got:\n{stored}"
    );
    assert!(
        !stored.contains("BEGIN:VEVENT"),
        "CROSS-6: no VTODO push may become a VEVENT anywhere in the chain, got:\n{stored}"
    );

    let pulled = task_with_uid(&pull(&server, "deadlines"), U_CRAWLER_UID);
    assert_eq!(pulled.uid.as_str(), U_CRAWLER_UID);
    assert_eq!(
        pulled.due,
        Some(EventTime::DateTimeUtc(
            "2026-08-15T23:59:00Z".parse().expect("an RFC3339 instant")
        ))
    );
    assert_eq!(
        pulled.summary.as_deref(),
        Some("Assignment 3 - Recursive descent parser")
    );
    assert_eq!(
        pulled.description.as_deref(),
        Some("Submit the parser and a short report via the course portal.")
    );
    assert_eq!(
        pulled.url.as_deref(),
        Some("https://example.edu/courses/cs101/assignments/123")
    );
    assert_eq!(pulled.priority, Some(1));
    assert_eq!(pulled.status, TodoStatus::Completed);
    assert_eq!(
        pulled.completed,
        Some(EventTime::DateTimeUtc(
            "2026-08-14T12:00:00Z".parse().expect("an RFC3339 instant")
        ))
    );
}

/// **The create half of the round trip.** Create through production, pull, re-push
/// the pulled item through the production *update* path, pull again — the
/// two pulled `Todo`s must be `==`. The equality is over **parsed domain
/// values** (R30: Radicale re-sorts and re-folds, so a byte comparison is
/// permanently red) and never over an `ETag` (R31: caldir re-stamps
/// `DTSTAMP` on every serialize, so an ETag moves on every push regardless of
/// content). This is the sync-base round trip's live proof: a second pull
/// after a push that changed nothing must converge, not churn.
#[test]
#[serial_test::serial(radicale)]
fn a_created_task_pulled_twice_yields_the_same_task_and_a_repush_changes_nothing() {
    let Some(server) =
        live_gate("a_created_task_pulled_twice_yields_the_same_task_and_a_repush_changes_nothing")
    else {
        return;
    };

    server.mkcalendar("deadlines");
    let collection_url = server.collection_url("deadlines");

    block_on(ops::create_event::create_item(
        USERNAME,
        PASSWORD,
        &collection_url,
        CalendarItem::Todo(todo_from_ics(U_CRAWLER_TASK_ICS)),
    ))
    .expect("the production create lands");

    let first_pull = task_with_uid(&pull(&server, "deadlines"), U_CRAWLER_UID);

    block_on(crate::caldav::ops::update_event::update_item(
        USERNAME,
        PASSWORD,
        &collection_url,
        CalendarItem::Todo(first_pull.clone()),
    ))
    .expect("re-pushing the unchanged pulled item lands");

    let second_pull = task_with_uid(&pull(&server, "deadlines"), U_CRAWLER_UID);

    assert_eq!(
        first_pull, second_pull,
        "a repush of an unchanged pulled task must converge, not churn"
    );
    assert_eq!(
        server.resource_hrefs("deadlines").len(),
        1,
        "the repush must update the one resource, never add a second one"
    );
}

/// **The update half of the round trip.** After a
/// create, update `SUMMARY`, `DUE` and `STATUS:COMPLETED` through the
/// production update path. The href-count assertion is what catches an
/// update that silently became a create.
#[test]
#[serial_test::serial(radicale)]
fn updating_summary_due_and_status_updates_the_one_resource() {
    let Some(server) = live_gate("updating_summary_due_and_status_updates_the_one_resource") else {
        return;
    };

    server.mkcalendar("deadlines");
    let collection_url = server.collection_url("deadlines");

    block_on(ops::create_event::create_item(
        USERNAME,
        PASSWORD,
        &collection_url,
        CalendarItem::Todo(todo_from_ics(U_CRAWLER_TASK_ICS)),
    ))
    .expect("the production create lands");

    let filename = format!("{U_CRAWLER_UID}.ics");
    let hrefs_before = server.resource_hrefs("deadlines");
    assert_eq!(
        hrefs_before,
        vec![url_to_href(&server.resource_url("deadlines", &filename))]
    );

    let mut todo = task_with_uid(&pull(&server, "deadlines"), U_CRAWLER_UID);
    todo.summary = Some("Assignment 3 - resubmission".to_string());
    todo.due = Some(EventTime::DateTimeUtc(
        "2026-09-01T23:59:00Z".parse().expect("an RFC3339 instant"),
    ));
    todo.status = TodoStatus::Completed;
    block_on(crate::caldav::ops::update_event::update_item(
        USERNAME,
        PASSWORD,
        &collection_url,
        CalendarItem::Todo(todo),
    ))
    .expect("the production update lands");

    let hrefs_after = server.resource_hrefs("deadlines");
    assert_eq!(
        hrefs_after, hrefs_before,
        "an update must touch the one existing resource, never add a second one"
    );

    let first_update_pull = task_with_uid(&pull(&server, "deadlines"), U_CRAWLER_UID);
    assert_eq!(
        first_update_pull.summary.as_deref(),
        Some("Assignment 3 - resubmission")
    );
    assert_eq!(
        first_update_pull.due,
        Some(EventTime::DateTimeUtc(
            "2026-09-01T23:59:00Z".parse().expect("an RFC3339 instant")
        ))
    );
    assert_eq!(first_update_pull.status, TodoStatus::Completed);

    let second_update_pull = task_with_uid(&pull(&server, "deadlines"), U_CRAWLER_UID);
    assert_eq!(
        first_update_pull, second_update_pull,
        "a second pull after the update must converge, not churn"
    );
}

/// **§8.6's "delete -> pull -> absent".** Create, then delete
/// through the production delete path. Asserted as an outcome — what the
/// collection holds — never as an HTTP status.
#[test]
#[serial_test::serial(radicale)]
fn deleting_the_local_task_removes_the_radicale_resource() {
    let Some(server) = live_gate("deleting_the_local_task_removes_the_radicale_resource") else {
        return;
    };

    server.mkcalendar("deadlines");
    let collection_url = server.collection_url("deadlines");

    block_on(ops::create_event::create_item(
        USERNAME,
        PASSWORD,
        &collection_url,
        CalendarItem::Todo(todo_from_ics(U_CRAWLER_TASK_ICS)),
    ))
    .expect("the production create lands");
    assert_eq!(server.resource_hrefs("deadlines").len(), 1);

    block_on(ops::delete_event(
        USERNAME,
        PASSWORD,
        &collection_url,
        U_CRAWLER_UID,
        ComponentKind::Todo,
    ))
    .expect("the production delete lands");

    assert!(
        server.resource_hrefs("deadlines").is_empty(),
        "the collection must hold no resource after the delete"
    );

    let items = pull(&server, "deadlines");
    assert!(
        !items
            .iter()
            .any(|item| item.uid().as_str() == U_CRAWLER_UID),
        "the deleted task must be absent from a pull, got {items:?}"
    );
}

/// §8.6's "completed VTODO" and "undated VTODO" rows, through a *write*
/// cycle. `U_CRAWLER_TASK_ICS` (`STATUS:COMPLETED`) and
/// `UNDATED_COMPLETED_TASK_ICS` (no `DTSTART`, no `DUE`, no `CREATED`) are
/// both pushed through the production create path and pulled back. This
/// re-proves the bare-`VTODO` `calendar-query` (RFC 4791 §9.9) through a
/// write cycle: the undated fixture's only date property is a `COMPLETED`
/// older than the window start, so a time-ranged query would make a
/// compliant server omit it — and core would read that absence as a delete
/// of the user's file.
#[test]
#[serial_test::serial(radicale)]
fn a_completed_and_an_undated_task_survive_the_production_write_paths() {
    let Some(server) =
        live_gate("a_completed_and_an_undated_task_survive_the_production_write_paths")
    else {
        return;
    };

    server.mkcalendar("deadlines");
    let collection_url = server.collection_url("deadlines");

    block_on(ops::create_event::create_item(
        USERNAME,
        PASSWORD,
        &collection_url,
        CalendarItem::Todo(todo_from_ics(U_CRAWLER_TASK_ICS)),
    ))
    .expect("the completed task's production create lands");
    block_on(ops::create_event::create_item(
        USERNAME,
        PASSWORD,
        &collection_url,
        CalendarItem::Todo(todo_from_ics(UNDATED_COMPLETED_TASK_ICS)),
    ))
    .expect("the undated task's production create lands");

    let items = pull(&server, "deadlines");

    let completed = task_with_uid(&items, U_CRAWLER_UID);
    assert_eq!(completed.status, TodoStatus::Completed);

    let undated = task_with_uid(&items, UNDATED_COMPLETED_UID);
    assert_eq!(undated.status, TodoStatus::Completed);
    assert_eq!(undated.due, None);
}

/// §8.6's "u_crawler fixture" row and §20's production-fixture-archive
/// requirement: the redacted production fixture, pushed through the production create
/// path, pulled, re-pushed through the production update path, pulled
/// again — the two pulls must be `==`. Every mapping claim in this effort is
/// made against this file rather than a hypothetical, so this is the gate
/// that runs it against a real server rather than only wiremock.
#[test]
#[serial_test::serial(radicale)]
fn the_production_fixture_round_trips_through_the_write_paths() {
    let Some(server) = live_gate("the_production_fixture_round_trips_through_the_write_paths")
    else {
        return;
    };

    server.mkcalendar("deadlines");
    let collection_url = server.collection_url("deadlines");
    let todo = todo_from_ics(P1_05_COMPLETED_TASK_ICS);
    let uid = todo.uid.as_str().to_string();

    block_on(ops::create_event::create_item(
        USERNAME,
        PASSWORD,
        &collection_url,
        CalendarItem::Todo(todo),
    ))
    .expect("the P1-05 fixture's production create lands");

    let first_pull = task_with_uid(&pull(&server, "deadlines"), &uid);
    assert_eq!(first_pull.status, TodoStatus::Completed);
    assert_eq!(first_pull.completed, None);
    assert_eq!(first_pull.percent_complete, None);

    block_on(crate::caldav::ops::update_event::update_item(
        USERNAME,
        PASSWORD,
        &collection_url,
        CalendarItem::Todo(first_pull.clone()),
    ))
    .expect("re-pushing the P1-05 fixture through the update path lands");

    let second_pull = task_with_uid(&pull(&server, "deadlines"), &uid);
    assert_eq!(
        first_pull, second_pull,
        "the P1-05 fixture must round-trip identically through create then update"
    );
}
