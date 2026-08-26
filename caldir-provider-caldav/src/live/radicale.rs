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
//! is **refused as unreachable** and the gate ships as
//! [`renaming_a_local_file_updates_the_same_resource_and_never_a_second_one`],
//! on the VEVENT control. The reason is the R70 gap below. The other five keep
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
//! # R70 / R69: what actually round-trips, and what cannot
//!
//! caldir can **pull** tasks from CalDAV and cannot **push** them. The
//! outbound RPC field is still `Event` (R70), and `ops::create_event` /
//! `ops::update_event` take an `Event`, so no task can reach a PUT through
//! production code — a task push aborts earlier with
//! `expected 1 event(s) in ICS, found 0`. Consequently every PUT of a task
//! below is issued by this harness (raw ICS, or bytes from
//! `Todo::to_ics_string`), never by `ops`, and the pull half alone is
//! caldir's transport under test. This is stated on each affected gate too.

use std::io::Write as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use caldir_core::{CalendarItem, EventTime, Todo, TodoStatus};
use chrono::{Duration as ChronoDuration, SecondsFormat, Utc};
use http::StatusCode;

use crate::caldav::ops;
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

/// Radicale 3.7.6. Pinned by tag because the probe measured this image.
const IMAGE: &str = "tomsquest/docker-radicale:latest";

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

/// A `VEVENT` carrying the *task's* UID: the same-UID pair of R65/C-1, and the
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

/// The property **names** inside the one `VTODO` component of a document, as a
/// sorted, de-duplicated set.
///
/// Only the `VTODO`: a sibling `VTIMEZONE` component is expected (R67) and
/// must not be counted, and nested `VALARM` properties belong to the alarm.
fn vtodo_property_names(ics: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut depth = 0usize;

    for line in ics.lines() {
        if let Some(component) = line.strip_prefix("BEGIN:") {
            depth += 1;
            if component == "VTODO" {
                // Reset: only the VTODO's own properties count.
                names.clear();
            }
            continue;
        }
        if line.starts_with("END:") {
            if line.starts_with("END:VTODO") {
                break;
            }
            depth = depth.saturating_sub(1);
            continue;
        }
        // Only lines directly inside the VTODO (VCALENDAR + VTODO = depth 2),
        // and never a folded continuation line.
        if depth == 2
            && !line.starts_with(' ')
            && !line.starts_with('\t')
            && let Some(name) = line.split([';', ':']).next()
            && !name.is_empty()
        {
            names.push(name.to_string());
        }
    }

    names.sort();
    names.dedup();
    names
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
/// **R70 gap.** The two PUTs are issued by this harness. `ops::create_event`
/// and `ops::update_event` take an `Event`, so a task cannot reach a PUT
/// through production code at all — a task push aborts with
/// `expected 1 event(s) in ICS, found 0`. What is proven live here is caldir's
/// **pull** path and its codec against a real server, not a caldir push.
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
/// **R70 gap.** The pair is built the only way it can be: the task is seeded
/// by the harness, and the push under test is the *event*. caldir cannot push
/// a task, so the mirror-image case — a task push landing on an event's href —
/// is unreachable and untested.
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
/// **The ticket's name was `renaming_a_local_task_file_updates_the_same_resource`
/// and is REFUSED as unreachable.** A local rename reaches a CalDAV server
/// only through a push, and caldir has no push path for a task: the outbound
/// RPC field is `Event` (R70) and `ops::create_event` / `ops::update_event`
/// take an `Event`, so a task push aborts with
/// `expected 1 event(s) in ICS, found 0` long before any href is computed.
/// Naming this test after a task would assert a capability that does not
/// exist, so it runs on the VEVENT control and says so.
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
