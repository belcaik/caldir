# Testing context (branch `vtodo-support`)

Survey of the existing test infrastructure. Reference only — no new test code here.

## 1. Test commands that actually work

`justfile` (repo root) has exactly one test recipe:

```
test:
  cargo test
```

There are **no `tests/` directories anywhere in the workspace** and **no `.ics` fixture files
on disk**. Every test is an inline `#[cfg(test)] mod tests` unit test compiled into its crate.
So `cargo test` == `just test` == the whole suite.

Observed run (`cargo test`, this machine, 2026-08-25 — all green):

| Target | Passed |
| --- | --- |
| `caldir-cli` (unittests src/main.rs) | 69 |
| `caldir-core` (unittests src/lib.rs) | 462 |
| `caldir-provider-caldav` (lib) | 15 |
| `caldir-provider-caldav` (bin) | 14 |
| `caldir-provider-google` (bin) | 49 |
| `caldir-provider-icloud` (bin) | 14 |
| `caldir-provider-outlook` (bin) | 49 |
| `caldir-provider-webcal` (bin) | 14 |
| Doc-tests (caldir_core, caldir_provider_caldav) | 0 |
| **Total** | **686 passed; 0 failed; 0 ignored** |

Gotcha hit while doing this: `rust-toolchain.toml` pins `1.97.0` (profile `minimal`,
components `clippy`+`rustfmt`). If that toolchain isn't installed, cargo downloads it first;
running two cargo commands in parallel races rustup and corrupts the toolchain. Install
serially with `rustup toolchain install 1.97.0 --profile minimal -c clippy,rustfmt` first.

Other commands: `just check` (`cargo check --workspace` + `cargo clippy --workspace -- -D warnings`).

CI (`.github/workflows/ci.yml`): three jobs — `fmt` (`cargo fmt --all -- --check`),
`check` (`just check` with `RUSTFLAGS: -D warnings`), and `test` which runs `just test`
across a **timezone matrix** (`TZ` = `UTC`, `Europe/Stockholm`, `America/New_York`,
`fail-fast: false`) specifically to catch timezone-dependent bugs. Any VTODO work touching
DUE/DTSTART/COMPLETED must survive all three zones.

## 2. `Caldir` in-memory test-state injection

There is no fluent builder type; the "builder" CLAUDE.md refers to is a test-only
constructor plus `test_utils` factories.

`caldir-core/src/caldir.rs:20-27`:

```rust
#[cfg(test)]
pub(crate) fn new(config: CaldirConfig, providers: ProviderRegistry) -> Self {
    Caldir {
        config,
        config_path: None,
        providers,
    }
}
```

Production path is `Caldir::load()` (`caldir-core/src/caldir.rs:29`), which reads
`CaldirConfig::default_system_config_path()` and `ProviderRegistry::from_system_path()`.
`config_path: None` in the test ctor means test instances never write back to a real config file.

The wrapper tests actually call is `test_caldir()` — `caldir-core/src/test_utils.rs:14-20`:

```rust
pub fn test_caldir() -> (TempDir, Caldir) {
    let (tmp_data_dir, test_caldir_config) = test_caldir_config();
    let caldir = Caldir::new(test_caldir_config, ProviderRegistry::new());
    (tmp_data_dir, caldir)
}
```

`test_caldir_config()` (`test_utils.rs:22-29`) builds a `TempDir` and points
`CaldirConfig::new(data_dir, TimeFormat::default(), None, None)` at `<tmp>/test-caldir`.

**Convention: the `TempDir` must be bound** (usually `let (_tmp, caldir) = ...`) or the
directory is dropped/deleted mid-test.

Real usage, `caldir-core/src/connection.rs` test module:

```rust
fn writable_connection() -> (tempfile::TempDir, MockProvider, Connection) {
    let (tmp, caldir) = test_caldir();
    let calendar = caldir
        .create_calendar("writable-cal", Some(calendar_config(Some(false))))
        .unwrap();
    let mock = test_mock_provider();
    let remote = Remote::new(mock.provider(), test_remote_params());
    let connection = Connection::new(calendar, remote);
    (tmp, mock, connection)
}
```

## 3. `MockProvider` — the fake remote

`caldir-core/src/provider/mock_provider.rs` (`pub(crate)`, gated by nothing but only
reachable inside the crate; module declared at `caldir-core/src/provider.rs:10`).

Full API:

| Item | Line | Meaning |
| --- | --- | --- |
| `MockProvider::new(slug: impl Into<ProviderSlug>)` | `mock_provider.rs:16` | Empty transport, no queued responses |
| `reply::<C: Rpc>(&self, response: C::Response)` | `mock_provider.rs:24` | Queue one typed success, wrapped as `{"status":"success","data":…}` |
| `reply_error(&self, error: ProviderTransportError)` | `mock_provider.rs:34` | Queue one transport-level failure |
| `provider(&self) -> Provider` | `mock_provider.rs:38` | `Provider::with_transport(slug, transport)` — the injection seam |
| `captured_request::<C: Rpc + DeserializeOwned>(&self) -> C` | `mock_provider.rs:46` | Decode the last request, **asserting `request.method == C::METHOD`** |

Backed by `MockTransport` (`caldir-core/src/provider/transport/mock_transport.rs`), which
implements `ProviderTransport::exchange`. Key behaviours:

- Responses are a **FIFO `VecDeque`** — one `reply()` per expected RPC call, in order.
- `exchange` **panics** if the queue is empty (`mock_transport.rs:82`), so a missing stub
  fails loudly rather than silently.
- It records every request (`captured_requests`) and every timeout (`captured_timeouts`);
  `captured_request()` / `captured_timeout()` return the **last** one (`:56`, `:60`).
- `MockTransport::with_response` / `with_error` for direct transport-level tests
  (used in `caldir-core/src/provider.rs` tests).

`test_utils.rs` shortcuts: `test_mock_provider()` (`:87`, slug `"test-provider"`),
`test_remote_params()` (`:91`, a `RemoteConfigParams` with `test_account = user@example.com`),
and `test_remote() -> (MockProvider, Remote)` (`:100`).

Real example — `caldir-core/src/connection.rs:498-510`:

```rust
#[tokio::test]
async fn apply_outgoing_diff_sends_delete_event_for_outgoing_delete() {
    let (_tmp, mock, mut connection) = writable_connection();
    let event = test_event();

    mock.reply::<rpc::DeleteEvent>(());
    connection
        .apply_outgoing_diff(&outgoing_delete_diff(event.clone()))
        .await
        .unwrap();

    assert_eq!(mock.captured_request::<rpc::DeleteEvent>().event, event);
}
```

## 4. Fixture conventions

- **No fixture files.** ICS is always an inline string literal with explicit `\r\n`, e.g.
  `caldir-core/src/event.rs:468` and `:540` — one long `"BEGIN:VCALENDAR\r\n…END:VCALENDAR\r\n"`.
- Parse helper: `Event::parse_single_ics(&str) -> Event` — `caldir-core/src/event.rs:236`,
  `#[cfg(test)] pub(crate)`, panics unless the doc holds exactly one valid event. For
  multi-event / error cases tests call the production `Event::from_ics_str`
  (`caldir-core/src/event.rs:96`).
- Structured builder fixtures instead of text where possible:
  `test_event()` (`test_utils.rs:60`, `Event::new("Test Event", test_event_time())`),
  `test_event_time()` (`:51`, floating `2026-01-01T12:00`),
  `test_icalendar_event()` (`:64`, an `icalendar::Event` with uid `test-uid@caldir`).
- Tempdirs: everything goes through `tempfile::TempDir`. `test_utils` wraps it in
  `test_caldir()`, `test_caldir_config()`, `test_calendar_path()` (`:31`),
  `test_calendar()` (`:37`), `test_calendar_event()` (`:43`), `test_binary()` (`:136`,
  writes an empty chmod-0755 file so provider discovery finds a "binary"), and
  `test_provider(slug)` (`:130`).
- CLI output tests use `indoc!` for expected multi-line text
  (`caldir-cli/src/commands/config.rs:39`, `caldir-cli/src/commands/calendars.rs:44`).
- `caldir-core/src/test_utils.rs` is `#[cfg(test)] mod test_utils;` (`caldir-core/src/lib.rs:12`)
  — it is **not** exported, so `caldir-cli` and the provider crates cannot use it. CLI tests
  build their own state locally.
- Diff constructors for tests: `outgoing_create_diff` / `outgoing_update_diff` /
  `outgoing_delete_diff` / `incoming_*` — `test_utils.rs:106-128`, all thin wrappers over
  `CalendarDiff::from_changes(outgoing, incoming)`.

## 5. Integration seams (scan -> parse -> diff -> push, no network)

The highest no-network seam is **`Connection`** (`caldir-core/src/connection.rs`), tested
directly in its own `#[cfg(test)] mod tests`. Its surface:

| Method | Line |
| --- | --- |
| `Connection::new(local: Calendar, remote: Remote)` | `connection.rs:18` |
| `read_only(&self) -> bool` | `connection.rs:30` |
| `async diff(&mut self, range: &DateRange) -> Result<CalendarDiff, _>` | `connection.rs:37` |
| `apply_incoming_diff(&mut self, diff) -> Result<(), _>` | `connection.rs:65` |
| `async apply_outgoing_diff(&mut self, diff) -> …` | `connection.rs:92` |
| `discard_outgoing_diff(&self, diff) -> …` | `connection.rs:122` |

`connection.diff(&DateRange::default())` performs the whole chain in one call: scan the
tempdir calendar (`Calendar::events()`, `caldir-core/src/calendar.rs:106`), parse each `.ics`
into `Event`, fetch "remote" events via the mocked `rpc::ListEvents`, then
`CalendarDiff::compute(local, remote, sync_bases, range)`
(`caldir-core/src/diff/calendar_diff.rs:14`). `apply_outgoing_diff` then drives
`rpc::CreateEvent` / `UpdateEvent` / `DeleteEvent` through `MockTransport`.

Everything is real except the transport, so read-only gating, last-write-wins direction,
`known_event_ids` delete detection, and sync-window skipping are all exercised on real files.

Lower seams if a narrower test is wanted: `CalendarDiff::compute` (pure), `Calendar` +
`CalendarEvent` (filesystem only, `caldir-core/src/calendar/event.rs`), `Event` ICS
round-trip (`event/from_icalendar.rs`, `event/to_icalendar.rs`), and the RPC wire layer
(`caldir-core/src/rpc/methods/*.rs`, which assert on serialized JSON/ICS shape).

## 6. Assertion / snapshot helpers

- **No `insta`, no snapshot testing anywhere.**
- `pretty_assertions = "1"` is a dev-dependency of `caldir-core` and `caldir-cli`; test
  modules opt in per-module with `use pretty_assertions::assert_eq;`
  (e.g. `caldir-core/src/connection.rs` test module).
- Otherwise plain `assert!` / `assert_eq!` with a message argument for the non-obvious cases.
- `indoc = "2"` (caldir-cli dev-dep) for expected CLI output blocks.
- `serial_test = "3"` (caldir-core dev-dep) — used only in
  `caldir-core/src/provider/transport/subprocess_transport.rs:100,119,137,154` via
  `#[serial_test::serial]`, because those tests mutate process-global `PATH`/cwd.
- `tempfile = "3"` is a dev-dep of every provider crate; in `caldir-core` it is a regular
  dependency (test_utils uses it).
- Async tests use `#[tokio::test]`; sync ones `#[test]`.

## 7. How a new VTODO test would be wired

Helpers to reuse, in the order a test would touch them:

1. **Parsing a VTODO string** — inline `"BEGIN:VCALENDAR\r\n…BEGIN:VTODO\r\n…"` literal, fed
   to `Event::from_ics_str` (`caldir-core/src/event.rs:96`) or, once a single-todo happy path
   exists, `Event::parse_single_ics` (`caldir-core/src/event.rs:236`). Follow the one-line
   `\r\n` literal style of `caldir-core/src/event.rs:468`.
2. **Structured fixture** — mirror `test_event()` / `test_event_time()`
   (`test_utils.rs:51-62`); a `test_todo()` alongside them is the natural home, and
   `test_icalendar_event()` (`:64`) is the model for an `icalendar::Todo` variant.
3. **On-disk calendar** — `test_caldir()` (`test_utils.rs:14`) then
   `caldir.create_calendar(slug, Some(config))`, or the one-shot `test_calendar()` (`:37`) /
   `test_calendar_event()` (`:43`). Always bind the returned `TempDir`.
4. **Filename / write assertions** — pattern in `caldir-core/src/calendar/event.rs` tests:
   `CalendarEvent::create(&calendar, event)` then assert on `cal_event.path()` and
   `cal_event.filename()`.
5. **Fake remote** — `test_mock_provider()` (`test_utils.rs:87`) + `test_remote_params()`
   (`:91`), or `test_remote()` (`:100`). Queue one `mock.reply::<rpc::ListEvents>(vec![…])`
   per expected call, then assert with `mock.captured_request::<rpc::CreateEvent>()`.
6. **Remote-configured calendar** — `test_remote_config("test-provider")`
   (`test_utils.rs:82`) inside a `CalendarConfig::new(name, color, read_only, remote)`,
   exactly as `calendar_config()` does in the `connection.rs` test module.
7. **End-to-end sync** — copy `writable_connection()` from `caldir-core/src/connection.rs`
   test module, then `connection.diff(&DateRange::default()).await` and
   `apply_outgoing_diff` / `apply_incoming_diff` with the
   `outgoing_create_diff` / `incoming_update_diff` / … constructors (`test_utils.rs:106-128`).
8. **RPC wire shape** — model on `caldir-core/src/rpc/methods/create_event.rs` tests:
   build the command, `cmd.to_json().unwrap()`, assert `json["command"]` and that the
   serialized ICS `starts_with("BEGIN:VCALENDAR")` and contains the UID.
9. **Assertions** — `use pretty_assertions::assert_eq;` at the top of the test module.
10. **Placement** — inline `#[cfg(test)] mod tests` in the file under test. Do not add a
    `tests/` directory; the workspace has none.
11. **Timezone safety** — CI runs the suite under `UTC`, `Europe/Stockholm`, and
    `America/New_York`. Prefer `EventTime::DateTimeFloating` / `EventTime::Date` fixtures
    (as `test_event_time()` does) unless the test is specifically about zone conversion.
