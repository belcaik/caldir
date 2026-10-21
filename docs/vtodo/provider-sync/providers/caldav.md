# CalDAV: the task provider reference

This is the provider page for `caldir-provider-caldav`'s VTODO support: what it round-trips, how a
mixed collection behaves, and how to stand up the Radicale environment its gates run against. For
the ship-record — what was actually run and its verdict — see `../verification.md`. This page
never duplicates that record; it explains the shipped behaviour that record verifies.

## Capability negotiation

`tasks.read`/`tasks.write` are advertised **per remote**, not per binary (`Capabilities::handle`
in `caldir-provider-caldav/src/commands/capabilities.rs`). The signal comes from
`caldav_supports_tasks`, a provider-private key `list_calendars` records at connect/discovery time
from the collection's advertised `supported-calendar-component-set`. The check fails closed: an
explicit `true` is the only value that turns tasks on. Absent (never determined), unreadable
(corrupt config), or explicit `false` all report `tasks.read = false` and `tasks.write = false`.
A collection whose component set omits `VTODO` therefore never has its VTODO REPORT issued, and
never has a rejected REPORT misread as "the remote has no tasks" — the two are kept distinct so a
provider that can't list tasks never triggers the delete path against a local VTODO holding a sync
base.

Events are unconditionally `read = true, write = true` for every CalDAV remote — task support is
the only axis capability negotiation varies on for this provider.

## Round-trip fidelity

CalDAV is the highest-fidelity task provider (spec §8.4): every property the core task codec
models is asserted, by name, to survive the PUT body and the read-back. The enumerated set is
`CALDAV_ROUND_TRIP_PROPERTIES` in
`caldir-provider-caldav/src/caldav/ops/create_event.rs`, proven complete against `caldir_core::Todo`
itself (adding a field to `Todo` without adding it here fails a test):

| Property | Notes |
|---|---|
| `UID` | Identity, half of it. |
| `DTSTAMP` | Emitted, never modelled — re-stamped on every serialize. |
| `DTSTART`, `DUE` | Both independent `Option`s; no synthesis between them. |
| `SUMMARY`, `DESCRIPTION`, `LOCATION`, `URL` | Plain fidelity. |
| `STATUS` | Four-valued task status, parsed from the raw property, never `get_status()`. |
| `PRIORITY`, `PERCENT-COMPLETE` | Raw `i32`/`Option`, never `Component::priority()` — an out-of-range value round-trips verbatim rather than clamping to `None`. |
| `COMPLETED` | Read via caldir's own time parser, never `get_completed()`. |
| `CREATED`, `LAST-MODIFIED`, `SEQUENCE` | Ignored by `Todo::==`; `LAST-MODIFIED`/`SEQUENCE` are the version-skew fallback when a base is unreadable. |
| `RECURRENCE-ID` | The other half of identity, not an expansion input. |
| `ORGANIZER`, `ATTENDEE`, `ATTACH` | A PUT omitting `ATTACH` deletes the attachment server-side (RFC 8607) — the codec always re-emits what it read. |
| `RRULE`/`EXDATE`/`RDATE` | Modelled, never expanded; master occurrence only. |
| `VALARM` | Hand-rolled splice into the `VTODO` body, not modelled as a core field. |
| `X-*` | Preserved **and** compared, parameters included — the existing provider-ID carrier. |

Out of scope, deliberately: `CATEGORIES` (the underlying icalendar crate corrupts multi-value
lists on re-emission), `RELATED-TO` (parsed lossily), `GEO`/`RESOURCES`/`COMMENT`/`CONTACT`
(already out for events), `DURATION` (never used), `DTEND` (forbidden on a task — R8). The user
guarantee is exactly this: modelled properties plus every `X-` property round-trip verbatim: any
other property is dropped the first time caldir writes the item, on disk and on the server alike,
same as it already is for events. This is proven mocked in
`caldir-provider-caldav/src/caldav/ops/create_event.rs`'s test module and confirmed against a real
Radicale 3.7.6 by `the_server_injects_no_non_x_properties_onto_a_task` in
`caldir-provider-caldav/src/live/radicale.rs`.

## Mixed collections

A single CalDAV collection may hold both `VEVENT`s and `VTODO`s, and an event and a task may share
one UID because component kind is part of caldir's identity (`EventInstanceId`). This already
shipped on the read side before task writes existed; what task writes add is the risk that a task
`PUT` or `DELETE` reaches into a collection full of someone's meetings and disturbs them. That risk
is closed, live, by two gates in `caldir-provider-caldav/src/live/radicale.rs`:

- `task_writes_leave_a_sibling_event_in_the_same_collection_untouched` — seeds a control `VEVENT`
  through the production create path, then creates, updates and deletes a task with a *different*
  UID in the same collection through the production write paths, asserting the event resource's
  server bytes never change.
- `a_same_uid_event_and_task_in_separate_collections_stay_distinct_through_a_task_write` — an event
  and a task sharing one UID, in two different collections, stay two distinct resources through a
  task write.

Sharing a UID between an event and a task **inside the same collection** is a server-enforced
hazard, not a caldir one: Radicale hard-rejects it (`409` + `<C:no-uid-conflict/>` across two
resource paths in the same collection; `400 Bad Request` if both components land in one resource).
The conflict ADR-02 worried about cannot arise inside a single Radicale collection at all — it is a
local-directory and cross-collection concern only, per `docs/vtodo/context/radicale-probe.md`
probe E.

## The Radicale environment

The live tier (`caldir-provider-caldav/src/live/radicale.rs`) runs a real Radicale 3.7.6 in
Docker, pinned by **digest** so a tag repoint can never make a green gate meaningless:

```
tomsquest/docker-radicale@sha256:45f92956a16dc139ab741a56612082723bfd09977b4b97764760532767be7448
```

Run it with:

```sh
CALDIR_LIVE_RADICALE=1 cargo test -p caldir-provider-caldav -- live::radicale --test-threads=1
```

Gated at **runtime**, never with `#[ignore]` — `live_enabled()` requires `CALDIR_LIVE_RADICALE=1`
exactly, plus a working `docker --version` and `curl --version`. Without the gate, every test
prints one `SKIP live::radicale::{test}` line and returns, and the plain `cargo test --workspace`
CI uses stays green with no Docker present. `curl` is the harness's HTTP client everywhere,
deliberately never caldir's own — so a transport bug in caldir cannot cancel out on both sides of
a gate.

Each test starts its own container (`caldir-live-radicale-{pid}-{nanos}`), health-polls it with a
`PROPFIND` rather than a fixed sleep, and tears it down in `Drop` — on success, on failure, and on
panic. Tests are `#[serial_test::serial(radicale)]` so containers never race for a port.

See `../verification.md` for the gates by name, the exact command run, and its observed result.
