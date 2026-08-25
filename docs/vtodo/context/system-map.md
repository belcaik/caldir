# Stage 1 system map — VTODO support

Synthesis of the seven explorer docs in `docs/vtodo/context/`, cross-checked against
source on branch `vtodo-support`. Every non-obvious claim carries a `file:line`.
Claims I could not settle are marked **UNVERIFIED:**.

Mission constraint: local VTODO <-> domain <-> provider RPC <-> generic CalDAV (Radicale).
No fake-event conversion. No VEVENT regressions.

---

## 1. Current architecture

### 1a. Local `.ics` on disk -> domain -> remote (the outbound path)

1. `Caldir::calendars()` reads `data_dir()` and takes every non-hidden **directory**
   as a calendar — `caldir-core/src/caldir.rs:71`, `read_dir` at `:74`, hidden filter
   at `:76`, `Calendar::load` at `:77`. **Non-recursive: only direct children.**
2. `Calendar::load(path)` reads `.caldir/config.toml` and `.caldir/state/` and touches
   **no event files** — `caldir-core/src/calendar.rs:71-87`.
3. `Calendar::events()` scans the calendar dir for `*.ics` (extension only, contents
   irrelevant) — `caldir-core/src/calendar.rs:106`, filter at `:113`, and
   `CalendarEvent::load(path)?` at `:114`. The `?` aborts the whole listing on one bad file.
4. `CalendarEvent::load` reads the file, calls `Event::from_ics_str`, then narrows to
   exactly one event — `caldir-core/src/calendar/event.rs:28`, parse at `:37`,
   arity narrowing at `:40-50`, error `ExpectedSingleEvent { path, found }` at `:44-47`.
5. `Event::from_ics_str` parses the whole document then keeps **only VEVENTs**:
   `Ok(icalendar.events().map(Event::try_from).collect())` — `caldir-core/src/event.rs:101`.
   This is the single VEVENT filter in the workspace.
6. `TryFrom<&icalendar::Event> for Event` requires DTSTART and UID —
   `caldir-core/src/event/from_icalendar.rs:11` (`EventError::MissingStart`) and `:22`
   (`EventError::MissingUid`).
7. `CalendarDiff::compute(local, remote, sync_bases, range)` —
   `caldir-core/src/diff/calendar_diff.rs:14`. Keys both sides by
   `EventInstanceId = (EventUid, Option<RecurrenceId>)` (`caldir-core/src/event/instance_id.rs:13`),
   window-skips at `:80`, direction at `:61` -> `update_direction` (`:153`).
8. `Connection::apply_outgoing_diff` -> `Remote::{create,update,delete}_event`
   (`caldir-core/src/remote.rs:40-91`) -> RPC.
9. RPC serializes the `Event` as an opaque ICS string —
   `caldir-core/src/rpc/ics.rs:28-30` calling `Event::to_ics_string()`
   (`caldir-core/src/event.rs:123`), VALARMs spliced by string-replacing
   `"END:VEVENT\r\n"` (`caldir-core/src/event.rs:249`).
10. Subprocess transport: one spawn per request, newline-delimited JSON —
    `caldir-core/src/provider/transport/subprocess_transport.rs:26-77`.
11. Provider (`caldav`) PUTs `event.to_ics_string()` verbatim as `text/calendar` —
    `caldir-provider-caldav/src/caldav/ops/create_event.rs:24`, `:34-39`.

### 1b. Remote item -> domain -> local file (the inbound path)

1. `Remote::list_events` sends `ListEvents { from, to }` as RFC-3339 strings —
   `caldir-core/src/remote.rs:24-38`, wire shape `caldir-core/src/rpc/methods/list_events.rs:5-11`.
2. CalDAV issues a `calendar-query` REPORT hard-filtered to
   `<C:comp-filter name="VEVENT">` with a `<C:time-range>` inside it —
   `caldir-provider-caldav/src/caldav/ops/list_events.rs:103-104`.
3. Response bodies go through `parse_event`, which calls `Event::from_ics_str` and
   `find_map(Result::ok)` — `caldir-provider-caldav/src/caldav/ops.rs:32-37`. Anything
   that does not parse as a VEVENT is silently dropped.
4. Provider returns `Vec<Event>`; core decodes via `Event::from_single_ics_str`
   (`caldir-core/src/rpc/ics.rs:32-35` -> `caldir-core/src/event.rs:111-121`), which
   errors `UnexpectedEventCount { expected: 1, found }` for 0 or 2+.
5. Wrapped as `RemoteEvent(Event)`; `modified_at()` = `Event::last_modified` —
   `caldir-core/src/remote/event.rs:5`, `:12`.
6. Diff (same step 7 above), then `Connection::apply_incoming_diff`
   (`caldir-core/src/connection.rs:65`) writes files via
   `CalendarEvent::create`/`update` (`caldir-core/src/calendar/event.rs:18`, `:55`),
   filename from `Event::base_slug()` (`caldir-core/src/event/slugify.rs:13`), mtime
   pinned to `LAST-MODIFIED` (`caldir-core/src/calendar/event.rs:153-163`).
7. Sync state recorded: `SyncBases::record` writes `bases/<sha256(id)>.ics` plus the
   legacy `known_event_ids` — `caldir-core/src/calendar/state/sync_bases.rs:48-66`,
   `caldir-core/src/calendar/state/event_bases.rs:29`.

### 1c. Where the two paths converge

They converge in exactly **three** places, and all three are `Event`-typed:

- **The ICS codec.** Both directions bottleneck on `Event::from_ics_str`
  (`caldir-core/src/event.rs:96`) and `Event::to_ics_string`
  (`caldir-core/src/event.rs:123`). Disk, sync bases, and the RPC wire all use them.
- **`EventInstanceId`.** Local set (`calendar_diff.rs:20`) and remote map (`:25`) are
  keyed identically, as are `SyncBases` / `KnownEventIds` / `EventBases`.
- **`CalendarDiff::compute`** (`calendar_diff.rs:14`) — the only place local and remote
  facts meet.

Corollary: a component-type distinction introduced at the codec plus the diff would
propagate everywhere by construction; introduced anywhere else it will not.

---

## 2. Confirmed VEVENT assumptions (deduplicated master list)

Severity: **BLOCKER** = VTODO cannot exist at all until fixed. **DEGRADED** = VTODO
exists but behaves wrongly or loses data. **COSMETIC** = naming/ergonomics only.

| id | assumption | file:line | severity | reported by |
|---|---|---|---|---|
| A1 | `Event::from_ics_str` iterates `icalendar.events()` — the **only** component source in the workspace; VTODO/VJOURNAL are dropped before any error can be raised | `caldir-core/src/event.rs:101` | BLOCKER | local-parser §3, sync-engine §9.2, provider-contract §6.4, provider-matrix (webcal 1), caldav 4, notes 3 |
| A2 | `CalendarEvent::load` requires exactly one parsed event per file; a VTODO-only file yields `ExpectedSingleEvent { found: 0 }` and the `?` at `calendar.rs:114` fails the **whole calendar listing** | `caldir-core/src/calendar/event.rs:40-50`; `caldir-core/src/calendar.rs:114` | BLOCKER | local-parser §2/§6.3-4, sync-engine §9.2, notes 1/2/4 |
| A3 | Wire decode requires exactly one event: `UnexpectedEventCount { expected: 1, found }` | `caldir-core/src/event.rs:111-121`; `caldir-core/src/rpc/ics.rs:32-35` | BLOCKER | provider-contract §5/§6.5, local-parser §5, notes 4 |
| A4 | `TryFrom<&icalendar::Event>` hard-requires DTSTART -> `EventError::MissingStart` | `caldir-core/src/event/from_icalendar.rs:11` | BLOCKER | local-parser §6.6, sync-engine §9.1, domain-model |
| A5 | `Event.start: EventTime` is non-optional; no `due` / `completed` / `percent_complete` / `priority` fields exist | `caldir-core/src/event.rs:40-68` | BLOCKER | domain-model, local-parser §6.8, sync-engine §9.3 |
| A6 | Serialization always emits a VEVENT (`icalendar::Event::new()` + unconditional `starts(...)`) | `caldir-core/src/event/to_icalendar.rs:4-8` | BLOCKER | local-parser §6.10, sync-engine §9.4, provider-contract §6.3 |
| A7 | Reminders are spliced by string-replacing the literal `"END:VEVENT\r\n"`; no VEVENT marker => VALARMs silently vanish | `caldir-core/src/event.rs:249` | DEGRADED | local-parser §4/§6.12, provider-contract §6.6, domain-model |
| A8 | CalDAV time-range REPORT hard-codes `<C:comp-filter name="VEVENT">` — a server-side VTODO is never returned | `caldir-provider-caldav/src/caldav/ops/list_events.rs:103` | BLOCKER | caldav §"Does the REPORT filter", notes 10 |
| A9 | CalDAV UID-lookup REPORT hard-codes the same filter — update/delete cannot locate a VTODO whose href is not `{uid}.ics` | `caldir-provider-caldav/src/caldav/ops.rs:107` | BLOCKER | caldav 2, notes 10 |
| A10 | `parse_event` uses `find_map(Result::ok)` — silently keeps the first parseable component and drops the rest, with no error | `caldir-provider-caldav/src/caldav/ops.rs:32-37` | DEGRADED | caldav 4/9, notes 12 |
| A11 | Webcal feeds silently yield zero items for a VTODO-only feed — connect only checks `BEGIN:VCALENDAR` | `caldir-provider-webcal/src/commands/list_events.rs:16-25`; `caldir-provider-webcal/src/commands/connect.rs:32-36` | DEGRADED | provider-matrix (webcal 1-2), notes 2 |
| A12 | Window filter is start/end-shaped: `start < to && end.unwrap_or(start) > from`; undefined without DTSTART | `caldir-core/src/event.rs:136-141` | BLOCKER | sync-engine §8/§9.5-6, provider-contract §6.12 |
| A13 | The out-of-window skip at `calendar_diff.rs:80` is the **sole** protection against a local-only item being deleted; a wrong `true` leads to `incoming Delete` at `:86` | `caldir-core/src/diff/calendar_diff.rs:80`, `:86` | BLOCKER | sync-engine §9.6, notes 6 |
| A14 | `Status` is the VEVENT triple (`Confirmed`/`Tentative`/`Cancelled`, default `Confirmed`); NEEDS-ACTION / IN-PROCESS / COMPLETED have no representation | `caldir-core/src/event/status.rs:5-13` | BLOCKER | domain-model, sync-engine §9.9, local-parser §6.9 |
| A15 | Two skip rules key on `Status::Cancelled` ("both cancelled" and "missing local + cancelled remote"); a *completed* task is not cancelled, so a locally-deleted completed task still produces an outgoing Delete and a remote completed task missing locally is re-created on pull | `caldir-core/src/diff/calendar_diff.rs:45`, `:106` | DEGRADED | sync-engine §9.9 |
| A16 | Filenames derive from DTSTART + SUMMARY (or `_recurring__` for masters); no rule exists for a component with no start | `caldir-core/src/event/slugify.rs:13-19`, `:48-60` | DEGRADED | local-parser §6.14, sync-engine §9.8, domain-model |
| A17 | mtime pinning requires `last_modified`; with `None` the file keeps its write time and `local_is_newer` returns `true` on the `(Some, None)` arm, biasing toward Push | `caldir-core/src/calendar/event.rs:153-163`; `caldir-core/src/diff/calendar_diff.rs:187` | DEGRADED | sync-engine §9.11 |
| A18 | `Event` `PartialEq` is educe-tuned: ignores `last_modified` + `sequence`, compares x-props order-independently by name+value, attachments by `uri` only. `==` **is** the "content changed?" test that drives sync direction | `caldir-core/src/event.rs:38`, `:63-67`, `:283-292` | DEGRADED | domain-model, sync-engine §6, notes 7 |
| A19 | Sync-base files are parsed as exactly one VEVENT; an unparseable base is silently skipped and degrades to a legacy known-id entry | `caldir-core/src/calendar/state/event_bases.rs:34`, `:60-62` | DEGRADED | local-parser §6.16 |
| A20 | `Method` is a closed serde enum with no `#[serde(other)]`; `#[serde(deny_unknown_fields)]` appears nowhere in the repo | `caldir-core/src/rpc/methods.rs:22-31` | BLOCKER (for new verbs) | provider-contract §5, notes 5 |
| A21 | `Handler`'s whole vocabulary is `list/create/update/delete_event` over `Event`; no component-kind discriminator anywhere on the wire | `caldir-core/src/provider/handler.rs:27-48` | BLOCKER | provider-contract §2/§6.1-2, provider-matrix |
| A22 | The only capability signal is `read_only: Option<bool>` on `CalendarConfig` — a write-direction gate with no component axis; `supported-calendar-component-set` is never requested | `caldir-core/src/calendar/config.rs:13`; caldav discovery (`ops/discover.rs`, `ops/list_calendars.rs`) | DEGRADED | provider-contract §4/§6.11, caldav 7 |
| A23 | Recurrence expansion derives instance `end` from `start` + master duration | `caldir-core/src/event/occurrences.rs:110-130` | DEGRADED | domain-model, sync-engine §9.12 |
| A24 | CalDAV create derives the href from the UID alone (`{calendar_url}/{uid}.ics`) — an event and a todo sharing a UID collide on one resource path | `caldir-provider-caldav/src/caldav/client.rs:73-76`; `create_event.rs:26` | DEGRADED | caldav 8 |
| A25 | Google/Outlook are structurally VEVENT-bound: only `/events` collections exist, mapping hard-fails without start/end, reminders are "minutes before start" | `caldir-provider-google/src/google_event/from_google.rs:17-22`; `caldir-provider-outlook/src/graph_api/types/mod.rs:42-108` | BLOCKER (for those providers) | provider-matrix |
| A26 | Type names are `Event*`-prefixed but component-agnostic in behaviour: `EventUid`, `RecurrenceId`, `EventInstanceId`, `EventTime`, `tz_normalize`, `Recurrence`, `XProperty`, `Attachment`, `Attendee`, `Organizer`, `Visibility`, and all state maps | domain-model §"Seams", `caldir-core/src/event/instance_id.rs:13` | COSMETIC | domain-model |
| A27 | `Reminder`'s field is literally `minutes_before_start` — meaningful for a VTODO only if it has a DTSTART | `caldir-core/src/event/reminder.rs:13` | COSMETIC | domain-model |

Deduplication notes: local-parser §3, sync-engine §9.2, provider-contract §6.4,
provider-matrix (webcal 1), caldav 4 and ORCHESTRATOR-NOTES 3 all report the same
single call site and are merged into **A1**. ORCHESTRATOR-NOTES 2's "two failure modes"
is A2 (hard error) + A11 (silent drop) — both real, both confirmed, and they are
*different* code paths, so they are kept separate. ORCHESTRATOR-NOTES 4's "two parallel
arity rules" is A2 + A3.

---

## 3. Dependency boundaries — `icalendar` 0.17.10

Pinned at `0.17.10`, `caldir-core/Cargo.toml:21`; **`Cargo.lock:1194-1197`** (verified —
see §5d). Default features only (`parser`); `recurrence`, `chrono-tz`, `strict-dates`,
`serde` are **off**.

### CAN do (design may rely on these)

- `Todo` is a first-class type with the same trait stack as `Event`
  (`Todo: EventLike: Component`, `src/components.rs:664-665`).
- `Calendar::todos()` / `todos_mut()` exist (`src/calendar.rs:279-296`), and the parser
  dispatches `"VTODO" => Todo::from(...)` (`src/parser/components.rs:160-170`).
  **No new parsing infrastructure is needed to see a VTODO.**
- Typed accessors: `DUE` (`todo.rs:52,58,63`), `COMPLETED` (`:71,76,84`),
  `PERCENT-COMPLETE` (`:33,38,45`), `STATUS: TodoStatus` (`:90,95,100`), plus shared
  `PRIORITY`, `UID`, `SUMMARY`, `DESCRIPTION`, `CLASS`, `URL`, `SEQUENCE`,
  `LAST-MODIFIED`, `ATTENDEE`, `LOCATION`, `RDATE`/`EXDATE`, `RECURRENCE-ID`.
- `TodoStatus` covers all four RFC values: `NeedsAction | Completed | InProcess | Cancelled`
  (`src/properties.rs:288-301`).
- `DUE;TZID=Europe/Berlin:...` parses and re-serializes losslessly as
  `CalendarDateTime::WithTimezone` (verified, probe 8/8b) — TZID survives without `chrono-tz`.
- Arbitrary `X-*` properties with parameters round-trip verbatim (verified) — the
  existing identity-carrier mechanism works unchanged on VTODO.
- Output is always CRLF (`write_crlf!`, `src/lib.rs:24-33`), so the existing CRLF
  assumptions hold.

### CANNOT do (hard design constraints)

- **C1. No byte-stable round-trip.** Single properties re-emit alphabetically from a
  `BTreeMap` (`src/components.rs:27`). Design must compare *parsed structures*, never
  bytes — which is what caldir already does (`state/event_bases.rs:34`).
- **C2. VALARM in a VTODO is not stable.** A parsed alarm lacking UID/DTSTAMP gains a
  fresh `Uuid::new_v4()` and `Utc::now()` on **every** serialize
  (`src/components.rs:99-110`, verified). Combined with A18 structural equality, this is
  a churn generator. It also cannot be read back: `Component::components()` returns
  `&[Other]`, `Other` is not re-exported from `src/lib.rs:48-64` (verified: `icalendar::Other`
  does not compile), and there is no `From<Other> for Alarm`. Reading an alarm means
  raw-property access only. Note caldir already sidesteps this for VEVENT by hand-rolling
  the block (`Reminder::ics_block`, spliced at `caldir-core/src/event.rs:249`) — a VTODO
  equivalent would need its own splice literal.
- **C3. `RELATED-TO` collapses to the last value.** `MULTIS`
  (`src/parser/properties.rs:27-44`) lists the ABNF placeholder `"RELATED"`, not the real
  `"RELATED-TO"`. Verified: two `RELATED-TO` lines in, one out, PARENT lost. Task
  hierarchies / subtasks are lossy at the library boundary. Same for repeated `X-*`
  (placeholder `"X-PROP"`) and `REQUEST-STATUS`.
- **C4. `EventLike` leaks `DTEND` and `all_day()` onto `Todo`** (verified probe 1). The
  library will happily write a `DTEND` into a VTODO, which RFC 5545 forbids. Nothing warns.
  The design must not use `ends()` on a `Todo`.
- **C5. `COMPLETED` must be UTC to be readable.** `get_completed()` returns `None` for a
  floating value (verified probe 9) while the raw property is retained — "present but
  unreadable" is a reachable state.
- **C6. Unknown `STATUS` values vanish from the typed view** (`STATUS:BOGUS` ->
  `get_status() == None`, raw preserved). `TodoStatus` has no `Custom` variant and
  `TodoStatus::from_str` is `pub(crate)` — caldir cannot even parse a status string
  through the crate.
- **C7. No `DURATION` setter.** `append_property(Duration::hours(2))` emits `PT7200S`,
  not `PT2H` (verified probe 2).
- **C8. `PRIORITY` clamps to `0..=10`** (`src/components.rs:206-209`) though RFC 5545
  §3.8.1.9 defines `1..9`; `get_priority()` returns `None` for a stored value > 10.
- **C9. RRULE is compiled out.** `EventLike::recurrence()` is `#[cfg(feature = "recurrence")]`
  and the feature is off. A recurring VTODO's RRULE is raw-property-only. caldir already
  works around this by depending on `rrule = "0.14"` directly and keeping `Recurrence.rrule`
  as an untyped `String` (`caldir-core/src/event/recurrence.rs:5-17`) — that workaround
  transfers to VTODO unchanged.
- **C10. `Calendar::from_str`'s error type is `String`** (`src/parser/calendar.rs:133`) —
  parse failures cannot be classified programmatically.
- **C11. `CalendarComponent` is `#[non_exhaustive]`** — any `match` over it needs a `_` arm.
- **C12. Duplicate UIDs are neither detected nor deduplicated** (verified probe 7).

---

## 4. Candidate seams for VTODO

Options, not a recommendation. Stage 3 decides.

### S1 — Enum at the domain root: `enum CalendarItem { Event(Event), Todo(Todo) }`

**Means:** a sibling `Todo` struct; `CalendarEvent` becomes item-typed; `EventChange`
becomes item-typed; `CalendarDiff` carries items.
**Blast radius:** `caldir-core/src/event.rs`, a new `todo.rs` tree, `calendar.rs`,
`calendar/event.rs`, `diff/calendar_diff.rs`, `diff/event_change.rs`, `connection.rs`,
`remote.rs`, `remote/event.rs`, `rpc/ics.rs`, `rpc/wire.rs`, all six `rpc/methods/*`,
every `Calendar::events()` caller (`calendar.rs:133,145,189,255,333`;
`connection.rs:38,68,98,125`; `caldir-cli/src/commands/doctor.rs:55`;
`caldir-cli/src/commands/rsvp.rs:91`), plus the CLI render layer.
**Risk:** largest diff; highest chance of a VEVENT regression; touches every one of the
686 existing tests' surrounding code. But it is the only option where the type system
enforces "no fake events".

### S2 — Widen `Event` in place (optional `start`, add `due`/`completed`/`percent`/`priority`, a `kind` field)

**Means:** one struct, discriminated by a `kind: ComponentKind` field.
**Blast radius:** `event.rs` struct + `PartialEq` (A18), `from_icalendar.rs`,
`to_icalendar.rs`, `slugify.rs`, `occurrences.rs`, `calendar_diff.rs` window logic.
Callers mostly compile unchanged.
**Risk:** `start: EventTime` -> `Option<EventTime>` is a breaking change that ripples
through `occurs_in_range` (A12), `time_slug` (A16), `expand_master` (A23), and every
VEVENT test. It also structurally permits "a VTODO with a DTEND" (C4) and "an event with
a DUE" — the type stops enforcing the domain. Directly contradicts the mission's
"no fake events" only in spirit, not in output.

### S3 — Codec-only seam: teach `from_ics_str` / `to_ics_string` about components, keep one domain type

**Means:** parse VTODO into the same `Event` with synthesized values.
**Blast radius:** `event.rs:96-134` only.
**Risk:** **this is the fake-event conversion the mission forbids.** Listed only to be
explicitly rejected: a VTODO with no DTSTART would need a synthetic start, which then
drives the filename (A16), the window filter (A12), and delete detection (A13).

### S4 — Parallel pipeline: `Todo` type + `Calendar::todos()` + `TodoDiff`, sharing identity/state/transport

**Means:** a second, structurally parallel stack. `Calendar::events()` gains a sibling
`Calendar::todos()`; the file loader classifies by component and routes. `EventInstanceId`,
`SyncBases`, `KnownEventIds`, `EventBases`, `tz_normalize`, `XProperty`, `Recurrence` are
reused verbatim (all component-agnostic per A26).
**Blast radius:** new `todo.rs` tree; `calendar.rs:106-119` split into a classifying
scan; `calendar/event.rs:40-50` arity rule generalized; `connection.rs` gains a second
diff/apply pass; `rpc/ics.rs` + `rpc/wire.rs` gain a `Todo` wire impl.
**Risk:** duplication (two near-identical diff engines that must stay in step — see A18);
delete-detection state is shared by `EventInstanceId`, so an event and a todo with the
same UID collide in `SyncBases` (see also A24). Lower regression risk for VEVENT than S1
or S2 because existing paths are untouched.

### S5 — The file-loader seam only (`CalendarEvent::load` + `Calendar::events()`)

**Means:** the narrowest possible first cut: stop the poison pill. Classify the file's
component and either skip non-VEVENTs or return a per-file warning instead of `?`-failing
the listing.
**Blast radius:** `caldir-core/src/calendar.rs:114`, `caldir-core/src/calendar/event.rs:40-50`,
`caldir-cli/src/commands/doctor.rs:55-58`.
**Risk:** very low; ships independently of any VTODO modelling. Does **not** deliver
VTODO — it only stops a VTODO from breaking a calendar. Note it changes observable
behaviour: multi-event `.ics` files currently hard-error, and any relaxation must not
start silently ignoring genuinely broken files.

### S6 — Provider-boundary seam: raw-ICS pass-through on the wire

**Means:** keep `Method` unchanged (A20 makes new verbs impossible to add compatibly) and
carry component kind *inside* the ICS payload plus an additive `CalendarConfig` /
`RemoteConfigParams` field. Providers that understand VTODO handle it; older binaries
never see one because the calendar's component set is negotiated by an additive flag.
**Blast radius:** `rpc/ics.rs`, `rpc/wire.rs`, `calendar/config.rs`,
`caldir-provider-caldav/src/caldav/ops/list_events.rs:103`,
`caldir-provider-caldav/src/caldav/ops.rs:107`.
**Risk:** low-to-moderate. Additive `CalendarConfig` fields are safe in both directions
(all fields `Option`, no `deny_unknown_fields` anywhere). The real risk is that `Ics(pub Event)`
and `impl WireValue for Event` are the only ICS wrapper (`rpc/ics.rs:7-8`,
`rpc/wire.rs:13-23`), and `WireValue`/`JsonWireValue` are `pub(crate)` — out-of-tree
providers cannot add a wire type, so whatever shape is chosen is permanent for third parties.

**Trade-off summary:** S5 is orthogonal and can ship first under any of the others.
S1 and S4 differ mainly in whether the two component types share one diff engine (S1) or
two (S4). S2 is cheapest to write and most expensive to keep correct. S3 is out of scope
by mission constraint. S6 is required regardless of which of S1/S2/S4 wins, because the
wire has no component axis today (A21).

---

## 5. Contradictions and corrections

### 5a. Is CLAUDE.md's mtime-based sync description stale? — **YES, stale.**

CLAUDE.md (and the byte-identical AGENTS.md) says: *"Direction is decided by comparing
local file mtime against the remote `LAST-MODIFIED`."*

Verified truth: `update_direction` (`caldir-core/src/diff/calendar_diff.rs:153`) does a
**three-way comparison against the sync base first** — `(false,false) => None`,
`(true,false) => Push`, `(false,true) => Pull` (`:161-167`). The mtime/`LAST-MODIFIED`
comparison in `local_is_newer` (`:180`) runs **only** when both sides changed or no base
exists (comment at `:170-171`). `local_is_newer` further prefers `SEQUENCE` when the
remote has no `LAST-MODIFIED` (`:181-183`).

`specs/caldir.md` is **correct** and already documents this ("Sync bases determine
direction for one-sided edits. Local file mtime and remote `LAST-MODIFIED` are only used
to break a conflict..."). So the contradiction is CLAUDE.md vs specs, and specs wins.
ORCHESTRATOR-NOTES 13 is **CONFIRMED**. Own ticket; does not block VTODO, but any Stage 3
design reading CLAUDE.md alone will get the sync model wrong.

CLAUDE.md is also stale about `known_event_ids` being the state store — `bases/` is now
primary and `known_event_ids` is legacy (`caldir-core/src/calendar/state/sync_bases.rs:83-97`,
`specs/caldir.md` §"Sync State").

### 5b. Is the trait `Handler` or `ProviderHandler`? — **`Handler`.**

Verified: `pub trait Handler: Send + Sync` at `caldir-core/src/provider/handler.rs:27`,
exported as `pub use handler::{Error, Handler, Result, process_request, run_provider};`
at `caldir-core/src/provider.rs:19`. There is no `ProviderHandler` symbol anywhere in the
Rust source.

CLAUDE.md:52 (and AGENTS.md:52) is wrong **twice**: it names `ProviderHandler` and links
it to `caldir-core/src/rpc/handler.rs`, a path that **does not exist** (`caldir-core/src/rpc/`
contains only `ics.rs`, `methods/`, `methods.rs`, `wire.rs`).

provider-contract.md hedges — its §2 heading says "`ProviderHandler` trait" but the code
block it quotes correctly shows `pub trait Handler`. Treat the heading as an echo of
CLAUDE.md, not an independent claim. Verified truth: **`caldir_core::provider::Handler`,
defined at `caldir-core/src/provider/handler.rs:27`.**

### 5c. Do u_crawler's `windows/` and `deadlines/` constitute separate calendars, and what does that do to ORCHESTRATOR-NOTES item 1?

**The premise does not hold in this environment.** Verified:

- `u_crawler` appears nowhere in this repo except `ORCHESTRATOR-NOTES.md:8`.
- `/home/belcaik/Dev/u_crawler` exists but contains **no** `windows/` or `deadlines/`
  directory (`find -maxdepth 4 -type d` returns nothing) and **no `.ics` files at all**.
- `/home/belcaik/.config/u_crawler/config.toml` has no caldir/calendar keys.
- `~/caldir` does not exist; `~/.config/caldir/` does not exist.

**UNVERIFIED:** whether u_crawler is *intended* to emit into a caldir tree. Settled by
asking the orchestrator/user for the actual `calendar_dir` path, or by `grep -rn "ics\|caldir"`
over the u_crawler source.

The **general rule** is verified, and it is what matters for the blast radius:

- `Caldir::calendars()` enumerates only **direct, non-hidden subdirectories** of
  `data_dir()` — `caldir-core/src/caldir.rs:71-83`, `is_dir() && !starts_with('.')` at `:76`.
  **No recursion.**
- `Calendar::events()` scans only its own directory, non-recursively —
  `caldir-core/src/calendar.rs:109-115`.

Therefore:

1. If `windows/` and `deadlines/` are **siblings under `calendar_dir`**, they are two
   separate calendars. A VTODO in `deadlines/` cannot break `windows/`: `Calendar::load`
   never reads event files (`calendar.rs:71-87`), the failure occurs per-calendar at
   `.events()` time, and `Caldir::calendars()` returns a `Vec<Result<_>>` so one failing
   calendar does not abort the others.
2. If they are **nested inside one calendar directory**, they are not calendars *and their
   `.ics` files are never read at all* — `Calendar::events()` only sees files in the top
   level of the calendar dir. No poison pill, but also no data.

**Correction to ORCHESTRATOR-NOTES item 1:** the poison pill is real (A2 + `calendar.rs:114`)
but its blast radius is **exactly one calendar directory**, not the whole caldir and not a
sibling calendar. Observably it surfaces as a single `DoctorWarning::UnreadableEvents` for
that calendar (`caldir-cli/src/commands/doctor.rs:55-57`). The specific u_crawler
cross-contamination scenario as written is **REFUTED for the sibling-directory case** and
**not applicable** for the nested case.

### 5d. `Cargo.lock` line numbers for `icalendar` — icalendar-capabilities.md is wrong

icalendar-capabilities.md cites `Cargo.lock:1181-1184`; local-parser.md cites
`Cargo.lock:1194-1197`. Verified: `grep -n '^name = "icalendar"' Cargo.lock` -> **1194**,
version `0.17.10` at 1195, checksum at 1197. **local-parser.md is correct.** The version,
checksum, and every other fact in icalendar-capabilities.md are unaffected.

### 5e. Where `Method` lives — provider-contract.md cites two locations

It cites the enum at `caldir-core/src/rpc/methods.rs:22-31` and, in §6.2, at
`caldir-core/src/provider/handler.rs:101-108`. Verified: the **enum definition** is at
`caldir-core/src/rpc/methods.rs:22-31` (confirmed byte-for-byte); `provider/handler.rs:98-109`
is the exhaustive `match` that dispatches it. Both are true; §6.2's phrasing conflates them.

### 5f. Non-contradiction worth recording

`specs/caldir.md` §"Fields We Skip" explicitly lists `PRIORITY`, `CATEGORIES`, `RESOURCES`,
`COMMENT`, `CONTACT` as intentionally unsupported. Four of those are core VTODO properties.
This is not a contradiction with the explorer docs, but it means adding VTODO **requires a
spec amendment**, not just code — Stage 3 must decide whether `PRIORITY` stays skipped for
VEVENT while being carried for VTODO.

---

## 6. Unresolved questions

Numbered, mapped to the user's DECISION-01..10 where the mapping is inferable from the
subject matter. **UNVERIFIED:** I was not given the text of DECISION-01..10, so the
mappings below are proposals, not confirmations — settle by pasting that list.

**Q1 — One type or two? (S1 vs S2 vs S4).** Maps to the domain-model decision.
*Open because:* the docs establish that identity/state/transport are component-agnostic
(A26) but that `start`, `Status`, and the window filter are not (A4, A12, A14). Both
shapes are defensible. *Cheapest experiment:* write the `Todo` struct and its
`PartialEq` (replicating A18 exactly) in isolation, then attempt `CalendarDiff::compute`
generic over it. If the generic signature is clean, S1/S4 win; if it forces trait-object
churn, S2's cost looks better.

**Q2 — Does `Event.start` become `Option`?** *Open because:* it is the single field whose
optionality decides S2's viability, and it ripples to `occurs_in_range`, `time_slug`,
and `expand_master`. *Experiment:* `sed` `start: EventTime` -> `Option<EventTime>` and run
`cargo check -p caldir-core`; count the errors. Serialize the cargo invocation
(ORCHESTRATOR-NOTES 15).

**Q3 — File naming for an undated / DUE-only task (A16).** *Open because:* `base_slug()`
has no defined behaviour without a start, and the filename is user-visible contract
(`specs/caldir.md` §"Filename Convention", README's directory listing). Candidates:
`_task__{slug}`, `{due_date}__{slug}`, `_undated__{slug}`. *Experiment:* none needed —
this is a product decision; but whatever is chosen must not collide with the existing
`_recurring__` prefix (`slugify.rs:15`).

**Q4 — Status model (A14, C6).** Extend `Status` with the VTODO values, or a separate
`TodoStatus`? *Open because:* the two diff skip-rules at `calendar_diff.rs:45` and `:106`
key on `Status::Cancelled` and would silently mis-handle `COMPLETED` (A15).
*Experiment:* write a failing test asserting that a locally-deleted, remote-`COMPLETED`
task does **not** produce an outgoing Delete.

**Q5 — VALARM in VTODO: in or out of MVP?** ORCHESTRATOR-NOTES 8 argues out. *Verified
support for that argument:* C2 (fresh UUID + DTSTAMP on every serialize) combined with
A18 (structural equality drives direction) is a concrete churn mechanism. *But note* caldir
already avoids this for VEVENT by hand-rolling the alarm block (`Reminder::ics_block`,
spliced at `event.rs:249`), so the same trick is available. *Open because:* the workaround's
cost is a second splice literal (`"END:VTODO\r\n"`), which is small.
*Experiment:* round-trip a VTODO-with-VALARM through `to_ics_string` twice and assert
byte equality; it will fail today, and pass with a splice.

**Q6 — Subtasks / `RELATED-TO` hierarchies.** ORCHESTRATOR-NOTES 9 calls them a non-goal.
**CONFIRMED** by C3 (verified: PARENT silently lost). *Open only as:* do we preserve a
*single* `RELATED-TO` as an opaque field, or drop it entirely? *Experiment:* none;
decide and document in `specs/caldir.md`.

**Q7 — Does fixing the two CalDAV comp-filter literals light up iCloud tasks for free?**
ORCHESTRATOR-NOTES 10. *Verified:* iCloud delegates every op to
`caldir_provider_caldav::caldav::ops` and has no mapping layer of its own
(provider-matrix, iCloud section). So yes *structurally* — but only after the core
`Event`-typed boundary is solved (A1, A21), which is the bigger blocker.
**UNVERIFIED:** whether Apple's Reminders collections are even returned by the current
`FindCalendars` discovery. *Experiment:* run `list_calendars` against a live iCloud
account and check whether Reminders lists appear.

**Q8 — Google / Outlook tasks: confirmed non-goal?** ORCHESTRATOR-NOTES 11.
*Verified in-repo:* Outlook's consent is `Calendars.ReadWrite User.Read offline_access`
(`caldir-provider-outlook/src/commands/connect.rs:25`) and Google's is
`calendar.calendarlist.readonly` + `calendar.events`
(`caldir-provider-google/src/commands/connect.rs:23-26`); neither covers a tasks API.
**UNVERIFIED:** the exact vendor scope names for Google Tasks / Graph To Do — provider-matrix
explicitly declines to assert them. *Experiment:* vendor docs, not code.

**Q9 — Component-capability negotiation on the wire (A22, S6).** How does core learn a
calendar accepts VTODO? *Open because:* there is no negotiation mechanism at all
(provider-contract §4), `supported-calendar-component-set` is never requested (caldav 7),
and `Method` cannot gain a verb compatibly (A20). *Experiment:* add an `Option<Vec<String>>`
component-set field to `CalendarConfig`, round-trip it through an old provider binary,
and confirm it is ignored rather than erroring (predicted safe — no `deny_unknown_fields`
anywhere in the repo).

**Q10 — UID collision between an event and a todo (A24).** *Open because:* CalDAV create
derives the href from the UID alone, and `SyncBases` is keyed by `EventInstanceId` which
has no component axis. *Experiment:* PUT a VEVENT and a VTODO with the same UID to
Radicale and observe whether the second overwrites the first.

**Q11 (NEW — orchestrator's) — Sync-window semantics for undated / DUE-only items.**
*Open because:* `occurs_in_range` is `start < to && end.unwrap_or(start) > from`
(`caldir-core/src/event.rs:136-141`), undefined without a start; and the check at
`calendar_diff.rs:80` is the **only** thing standing between a local-only item and an
`incoming Delete` at `:86` (A13). The orchestrator's safe prior — undated items are
always in-window and never window-deleted — is sound but is a *behaviour choice*, not a
derivation. *Experiment:* a `CalendarDiff::compute` unit test with an undated local task,
an empty remote, and a bounded `DateRange`, asserting `incoming.is_empty()`.

**Q12 (NEW) — CalDAV `<C:time-range>` semantics for VTODO.** *Open because:* RFC 4791
defines time-range matching for VTODO over DTSTART/DUE/DURATION/COMPLETED/CREATED, and a
VTODO with **no** date properties matches no time-range at all (caldav doc, item 3). A
naive swap of `VEVENT` -> `VTODO` in the comp-filter at `ops/list_events.rs:103` would make
undated tasks permanently invisible — which, combined with A13/Q11, is a delete hazard.
*Experiment:* against Radicale, issue a `calendar-query` with a VTODO comp-filter both
with and without `<C:time-range>` and diff the result sets.

**Q13 (NEW) — Error model for a mixed-component `.ics` file.** *Open because:* the two
arity rules (A2 disk, A3 wire) must move together (ORCHESTRATOR-NOTES 4 — **CONFIRMED**),
and today they express "exactly one VEVENT". The replacement must decide: is a file with
one VEVENT + one VTODO an error, or two items? Is a file with two VTODOs an error?
*Experiment:* enumerate the four cases as failing tests before writing any code.

**Q14 (NEW) — Does the poison-pill fix (S5) ship separately?** *Open because:* S5 is
independently valuable, low-risk, and changes user-visible error behaviour for
already-broken files. Shipping it first de-risks every later stage.
*Experiment:* none — a sequencing decision.

**Q15 (NEW) — Spec amendment scope.** `specs/caldir.md` currently declares `PRIORITY`,
`CATEGORIES`, `RESOURCES`, `COMMENT`, `CONTACT` out of scope (§"Fields We Skip"), and its
whole VEVENT section is component-specific. *Open because:* the spec is described as
canonical in CLAUDE.md, so VTODO cannot ship without amending it. *Experiment:* none —
a documentation decision that should be made *before* field selection, not after.

---

## 7. Risk register

| # | Risk | Concrete failure scenario | Early-warning test |
|---|---|---|---|
| R1 | **Mass delete of undated tasks** | `occurs_in_range` returns `false` for a DUE-only task; `calendar_diff.rs:80` does *not* skip it (or the remote never returned it because of Q12); `sync_bases` knows the id -> `incoming Delete` at `:86` wipes the file. With `allow_mass_delete` bypassed on a first sync this is silent data loss. | `CalendarDiff::compute` with an undated local task + empty remote + bounded `DateRange`: assert `incoming.is_empty()` and `outgoing.is_empty()`. Also assert the `guards.rs:10` mass-delete counter trips before any apply. |
| R2 | **Infinite sync churn** | A VTODO round-trips non-identically (VALARM UUID regeneration, C2; or alphabetical property reorder, C1) so `local != base` on every diff, producing a Push every single sync, forever. | Serialize the same `Todo` twice and assert byte equality; then a `parse -> serialize -> parse` test asserting structural `==`. Run in all three CI timezones. |
| R3 | **VEVENT regression from the arity/loader change** | Relaxing `ExpectedSingleEvent` (A2, S5) to "skip non-events" starts silently ignoring genuinely malformed VEVENT files, so a user's real event disappears from `caldir status` with no warning. | A test asserting a syntactically-broken VEVENT file still produces `DoctorWarning::UnreadableEvents` (`doctor.rs:55-57`) and is *not* silently skipped. Keep the existing `doctor.rs:75-91` empty-VCALENDAR test green. |
| R4 | **UID collision overwrites data** | A todo and an event share a UID; CalDAV create PUTs both to `{calendar_url}/{uid}.ics` (A24) and the second silently replaces the first on the server. Locally, both map to the same `EventInstanceId` and collide in `SyncBases`. | Integration test against Radicale: create an event and a todo with the same UID, then list and assert both survive. Unit test: assert `SyncBases` keys distinguish them. |
| R5 | **Silent empty result** | The CalDAV comp-filter is switched to VTODO but `<C:time-range>` excludes undated tasks (Q12), so `caldir pull` reports "no changes" while the server holds 200 tasks. Same failure shape already exists for webcal (A11). | Radicale test: PUT an undated VTODO, then assert `fetch_events`-equivalent returns it. Plus a unit assertion that a zero-item response on a calendar with known bases raises a warning rather than proceeding to delete. |
| R6 | **Wire incompatibility with shipped provider binaries** | A design adds a `list_todos` verb; `Method` has no `#[serde(other)]` (A20), so every already-installed provider binary fails with `"Failed to parse request: …"` (`provider/handler.rs:75-78`) — a hard break for all users mid-upgrade. | Test: send a JSON request with an unknown `command` to `process_request` and assert the error shape; and a test that an old-shaped `CalendarConfig` (missing any new field) still deserializes. |
| R7 | **Equality drift between `Event` and `Todo`** | `Todo`'s `PartialEq` forgets to ignore `last_modified`/`sequence` or to compare x-props order-independently (A18). Every sync sees a phantom change; direction flaps; `bases/` is rewritten every run. | Mirror the existing `Event` equality tests for `Todo`: same `last_modified`, different `sequence` -> equal; reordered x-props -> equal; reordered attachments -> equal. |
| R8 | **Timezone-dependent DUE handling** | `DUE;TZID=...` or a floating `COMPLETED` (C5) is interpreted against the system zone; tests pass in UTC and fail in `America/New_York`. CI runs all three (`.github/workflows/ci.yml`). | Every DUE/COMPLETED test written with explicit `EventTime::Date` / `DateTimeFloating` fixtures per testing §7.11, plus at least one deliberately zoned case asserted identical across the TZ matrix. |
| R9 | **Data loss on RELATED-TO / repeated X-\*** | A user's task hierarchy or a provider's repeated `X-*` identity carrier collapses to the last value (C3) on the first caldir write, permanently destroying server-side data. | Round-trip test with two `RELATED-TO` lines and two same-named `X-*` lines; assert what survives, and gate on that assertion so the loss is *known* rather than discovered. |
| R10 | **Poison pill re-introduced elsewhere** | A new VTODO code path adds its own `?`-propagating listing (mirroring `calendar.rs:114`) so one bad task file breaks the whole task listing. | A test that a calendar containing one valid task and one corrupt `.ics` still lists the valid task. |
| R11 | **Toolchain corruption during parallel work** | Two agents run cargo simultaneously; rustup races on the pinned `1.97.0` toolchain and corrupts it, producing spurious build failures that get mis-diagnosed as code bugs (ORCHESTRATOR-NOTES 15, testing §1). | Pre-install serially: `rustup toolchain install 1.97.0 --profile minimal -c clippy,rustfmt`. Establish the 686-passed baseline before any change. |
