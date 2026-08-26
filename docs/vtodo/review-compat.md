# Review 4 of 5 — Backwards compatibility

**Question:** does an existing caldir user, who has never heard of tasks and has
no `.ics` file containing a `VTODO`, notice anything different?

**Answer: yes, in four ways — one of them silent data loss.**

## Method

Two release binaries, built from clean worktrees and driven against the same
fixtures:

| binary | ref | path |
| --- | --- | --- |
| old | `main` (`2ed74fa`) | `scratchpad/wt-main/target/release/caldir` |
| new | `HEAD` (`5a2f756`) | `scratchpad/wt-head/target/release/caldir` |

> The repo working tree acquired **uncommitted** edits to `commands/sync.rs`,
> `pull.rs` and `discard.rs` while this review ran (another agent's in-flight
> work; the ADJUDICATION calls it *"MY SCOPING ERROR — sync and discard were left
> out … Dispatched separately"*). Every result below was re-taken against a
> detached worktree at `5a2f756` so it describes HEAD, not that work in progress.

Fixture: an event-only caldir — two calendars, five `VEVENT` files (zoned,
all-day, recurring, `RRULE`, attendee/organizer/alarm/X-prop), one connected to
a scripted mock provider on `PATH` that logs every RPC request it receives.

## Surface-by-surface

| # | Surface | Changed? | Evidence | Documented? |
| --- | --- | --- | --- | --- |
| 1a | Event filenames (`base_slug`) | **No** | `event/slugify.rs` is a pure extract-function refactor — `Event::summary_slug`/`time_slug` delegate to free fns with identical bodies. `caldir new "Coffee ☕ with Bob"` → `2026-09-03T1600__coffee-with-bob.ics` on both; generated ICS byte-identical mod `DTSTAMP`/`UID`. | n/a |
| 1b | Existing `.ics` re-parse / rewrite | **No, for single-component files** | Full `sync` against the mock: `diff -r -I '^DTSTAMP'` over the whole caldir tree → **identical**, files and state. | n/a |
| 1c | Existing `.ics` re-parse — **file with a sibling component** | **YES — data loss** | `item.rs:248` errors the whole document on any component that is not `VEVENT`/`VTODO`/`VTIMEZONE`. `main`'s `event.rs:101` was `icalendar.events()`, which *ignored* every other component. Measured: a `VEVENT` beside `VFREEBUSY`, `VJOURNAL`, a stray top-level `VALARM`, or a top-level `X-WR-CUSTOM` → visible on old, **invisible on new**, in text, `--json` and push. | **NO** |
| 2a | `state/known_event_ids` | **No** (event-only) | Post-sync file byte-identical on both. Task lines get a `VTODO:` sigil (`instance_id.rs`), absent when no task exists. | ADR-09 C4 (amended, B-4) |
| 2b | `state/bases/<sha256>.ics` | **No** | Event identity string form is unprefixed, so digests are unchanged. Post-sync base filenames identical; contents identical mod `DTSTAMP`. | ADR-09 C5 |
| 2c | **Old caldir reads NEW state** | **No** (event-only) | Cross matrix: old-binary-on-old-state ≡ old-binary-on-new-state; new-on-old ≡ new-on-new; old ≡ new on the same state. All byte-identical stdout, no state drift. | — |
| 2d | Old caldir reads new state **with a task present** | Fails loudly, **safely** | `caldir events` → exit 1, `expected exactly one event in …_task__….ics, found 0`. `caldir sync` → per-calendar error, **zero RPC calls**, nothing deleted, `known_event_ids` untouched. The `VTODO:` line is never reached. | ADR-09 C6 |
| 3a | RPC wire — new core + **old provider** | **No** | `list_events` request bytes and `create_event` payload bytes compared from the provider's own request log: identical except the `DTSTAMP` second. No new verb (`Method` is still the six). | ADR-09 C8/C9 |
| 3b | RPC wire — old core + **new provider** | **No**, for VEVENT | Payload is still a bare ICS string; `ItemIcs` is a second newtype over the same JSON string, not a new field. | ADR-09 C10 |
| 3c | CalDAV/iCloud provider: **second REPORT** | **YES** | `caldav/ops/list_events.rs:71-80` — every `fetch_events` now issues an unconditional **second** `calendar-query` REPORT with a bare `<C:comp-filter name="VTODO"/>` and **no** time-range, and `?`-propagates its failure (`list_events.rs:33-38`), failing the entire listing. An event-only user gets an extra round-trip per sync and a new failure mode against any server that rejects that filter. No test covers a server erroring on it. | **NO** |
| 3d | webcal provider | Silent-drop TODO still open | `caldir-provider-webcal/src/commands/list_events.rs:26` `Ok(_) => None` — a feed task is dropped without a word. Its own comment says *"THIS ARM IS A SILENT VTODO DROP … L3-04 owns surfacing feed tasks"*; L3-04 landed and this is still here. Harmless for a genuinely event-only feed. | comment only |
| 4 | Public Rust API (`caldir-core` 0.14.1 → 0.15.0) | **YES, breaking** | See table below. | ADR-09 C11 allows it; minor bump taken. **The breaks themselves are not listed in any changelog** |
| 5a | CLI text output, healthy caldir | **No** | `events`, `today`, `week`, `config`, `calendars`, `invites`, `doctor` — stdout and stderr byte-identical, exit codes identical. Duplicate-file `doctor` warning byte-identical for events. `rsvp` by path and by slug, and its errors, identical. | ADR-09 C13 |
| 5b | CLI `--json`, healthy caldir | **Additive only** | Five keys added to every entry: `kind`, `due`, `completed`, `percent_complete`, `task_status`. `config`/`calendars`/`invites` `--json` byte-identical. | website `commands.md` |
| 5c | The `del`-predicate claim | **TRUE — verified** | `jq -c 'map(select(.kind=="event") \| del(.kind,.due,.completed,.percent_complete,.task_status))' new.json` is **byte-identical** to `old.json` for `events`, `today` and `week`. | website `commands.md` |
| 5d | CLI text/JSON with an **unreadable file** | **YES** | `events`/`today`/`week`: old exit **1**, nothing on stdout, the offending path named. New: exit **0**, agenda rendered, `⚠ N unreadable files in work (caldir doctor)` on stdout (text only) — and on `--json`, **nothing at all**: exit 0, a valid-looking but silently short array, empty stderr. | **NO** |
| 6 | Exit codes, event-only paths | **`push` only** | Provider-failure matrix on an event-only calendar: `push` **0 → 1**, error moved **stdout → stderr** (`commands/push.rs:64`). `pull`, `sync`, `discard`, `status` all unchanged at HEAD. Plus 5d's `events`/`today`/`week` **1 → 0**. | **NO** — no caldir doc mentions exit codes at all |

## Breaking changes, with blast radius

### B1 — A `.ics` file with any sibling component now loses its event, silently

The one that matters. `CalendarItem::parse_document` (`caldir-core/src/item.rs:220-249`)
walks **every** component in the `VCALENDAR` and returns
`Err(UnsupportedComponent)` for anything outside `VEVENT`/`VTODO`/`VTIMEZONE`.
`main` never looked: `Event::from_ics_str` was `icalendar.events().map(Event::try_from).collect()`
(`event.rs:96-102`), which iterates VEVENTs and ignores the rest.

Measured, on an otherwise ordinary event-only caldir:

```
### X-WR-CUSTOM sibling
   old: case event visible in --json = 1
   new: case event visible in --json = 0
```

Same result for `VFREEBUSY`, `VJOURNAL` and a top-level `VALARM`. RFC 5545's
`icalbody` explicitly permits `x-comp` and `iana-comp` at top level, so this is
not a malformed file — it is a legal one that exporters emit.

**Blast radius.** The event vanishes from `events`/`today`/`week`, from `--json`,
and from sync. End-to-end on the mock:

```
sync old ... Pushed: 4 created     pushed 'Case'? 1
sync new ... Pushed: 3 created     pushed 'Case'? 0
```

Exit 0, and `sync`/`push`/`status` print **no** warning — the `⚠ N unreadable
files` notice was added to the agenda renderer only (`agenda_text.rs:48-57`).
The event is never pushed and, if it had synced before, `CalendarDiff`'s
`is_incomplete` guard (`diff/calendar_diff.rs:26-30`) suppresses *all* deletes for
that calendar from then on — so the calendar quietly stops converging and
nothing says so.

This is squarely inside ADR-09 **C1/C2** ("an existing VEVENT file parses to a
structurally `==` `Event` before and after") and it is undocumented.

### B2 — `events`/`today`/`week` went fail-hard → fail-soft

Old: exit 1, empty stdout, the bad path named in the error. New: exit 0, a
partial agenda. On `--json` there is no compensating warning at all — a script
doing `caldir events --json | jq …` now silently processes a short array and
sees success. QA logged this as F2 (`qa-ucrawler-regression.md:403`); commit
`5a2f756` (F-6) added the **text** warning and left the exit code and the JSON
path as they were. ADR-09 C13 ("existing text for existing situations is
unchanged", INVIOLABLE) is breached for an event-only situation. Undocumented.

### B3 — `caldir push` exit code and error stream changed

`commands/push.rs:64` `bail!("push failed for {} calendar(s)", failed)`, and
failures moved from `println!` to `eprintln!` (`push.rs:70-72`). Measured on an
event-only calendar with a provider that returns `{"status":"error"}`:

```
push old: exit=0 stdout=97B stderr=0B
push new: exit=1 stdout=10B stderr=124B
```

Two script-visible changes: `caldir push` now fails a `set -e` pipeline on a
transient remote error, and a pipeline that captured stdout no longer captures
the error text. Defensible as a fix (QA F-3/F-7), but it is a behaviour change on
a pure-event path, it makes `push` inconsistent with `sync`/`pull`/`discard`
(still exit 0 at HEAD), and no caldir doc mentions exit codes.

### B4 — `caldir-core` Rust API (allowed by C11, but not enumerated anywhere)

| Item | main | HEAD |
| --- | --- | --- |
| `Calendar::events` | `Result<Vec<CalendarEvent>, _>` | `Result<CalendarListing, _>` |
| `CalendarEvent::event` | `&Event` | `Option<&Event>` |
| `RemoteEvent` | `RemoteEvent(Event)` | `RemoteEvent(CalendarItem)` |
| `EventChange::{Create,Update,Delete}` | carry `Event` | carry `CalendarItem` |
| `EventInstanceId` | tuple struct | named struct + `kind` |
| `Calendar::create_event`, `CalendarEvent::{create,update}`, `RemoteEvent::new` | `Event` | `impl Into<CalendarItem>` (source-compatible for `Event` callers) |

Additive: `CalendarItem`, `Todo`, `TodoStatus`, `ComponentKind`, `CalendarListing`,
`SkippedEvent`, `ExpandedItems`, and the newly re-exported `CalendarError`,
`CalendarEventError`, `RemoteError`. Nothing was removed from `lib.rs`.
`CalendarItem` and `ComponentKind` are `#[non_exhaustive]`. The 0.15.0 minor bump
is present and correct per C11 — but the break list exists only in ticket
markdown, not in a changelog a crates.io consumer would read.

### B5 — CalDAV/iCloud now issue a second, unbounded REPORT

Every `list_events` fires a bare `<C:comp-filter name="VTODO"/>` query with no
time-range, and any failure of it fails the whole listing
(`caldav/ops/list_events.rs:33-38, 71-80, 105`). An event-only user upgrading the
caldav or icloud provider takes a second round-trip per calendar per sync, plus a
new dependency on a query their server may not answer. No test exercises a server
that rejects it. Undocumented.

## Two source comments that are false

- `caldir-cli/src/output/agenda_json.rs:114` — *"Field order is the wire order
  under `#[serde(flatten)]`, so the five additive keys sit at the end"*. They do
  not: `flatten` routes through `serde_json::Map` (a `BTreeMap` without
  `preserve_order`), so real output is alphabetical and the new keys interleave
  (`…,"calendar","completed","description","due","end","instance_id","kind",…`).
  The user-facing claim in `commands.md` ("the pre-existing keys keep their
  order") happens to be true anyway, since sorting is stable across both
  versions — but the reasoning in the code is wrong.
- `caldir-provider-webcal/src/commands/list_events.rs:20-26` — `TODO(L3-04)` says
  L3-04 owns removing the silent drop. L3-04 landed; the arm did not.

## Verdict

**An existing, task-free user is disturbed — mildly in three places and
seriously in one.**

- Everything that should be invisible **is** invisible, and I verified it by
  execution rather than by reading tests: filenames, file bytes, `known_event_ids`,
  base digests, all seven text commands, four of seven `--json` commands, the full
  RPC request stream, and the complete old↔new state-interop matrix. The
  downgrade direction is genuinely safe: old caldir meeting a new task file
  refuses loudly and writes nothing.
- The `del`-predicate migration claim in `commands.md` is **true**, byte for byte,
  and I confirmed it rather than taking it.
- But **B1 is a shipping-blocker**: a legal `.ics` file that main handled fine now
  loses its event from every view and from sync, with exit 0 and — outside the
  agenda's text renderer — no warning anywhere. It needs either a per-component
  skip for unknown top-level components (matching main's behaviour) or, at
  minimum, the same loud failure `main` gave.
- **B2/B3** are exit-code changes on event-only paths that no document mentions.
  They are individually defensible; being undocumented is not.
