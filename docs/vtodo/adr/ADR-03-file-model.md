# ADR-03: Local file model and arity rules

## Status
PROPOSED (Stage 3 candidate). Settles DECISION-03, system-map Q3 and Q13. Touches Q14.

## Context

- One `.ics` file = one item is load-bearing, not incidental. `Calendar::event(slug)`
  (`caldir-core/src/calendar.rs:122-126`) resolves an item by filename; `CalendarEvent`
  owns a `path` and `delete()` is `fs::remove_file` (`calendar/event.rs:80-82`).
- The disk arity rule is `CalendarEventError::ExpectedSingleEvent` (`calendar/event.rs:40-50`,
  message `calendar/event/error.rs:10-11`). Its twin `EventError::UnexpectedEventCount`
  (`event.rs:114-119`) guards RPC wire decode (`rpc/ics.rs:34`) and sync-base files
  (`calendar/state/event_bases.rs:34`, `:60`). Confirmed: they must move together.
- Both count only `icalendar.events()` (`event.rs:101`) — the sole VEVENT filter in the
  workspace. Consequences today, both verified: a VEVENT+VTODO file loads with **no error
  and the VTODO silently dropped**, and a zero-component file reports the *identical*
  `found: 0` as a VTODO-only file, so the message cannot tell "unsupported" from "empty".
- **The writer cannot preserve what it did not change.** `write_best_event_file`
  (`calendar/event.rs:165-201`) is handed the bytes of exactly one serialized item and does
  `fs::write` / `create_new` + `write_all`. It never reads the existing file. `update`
  (`:55-78`) writes the new path and unlinks the old. A multi-component file is therefore
  destroyed on the first write to any item in it.
- A read-parse-merge-rewrite writer would not be safe either: re-serializing an untouched
  sibling runs it through known-lossy paths (BRIEF: VALARM gains a fresh random UID +
  DTSTAMP per serialize; `RELATED-TO` collapses to last; unknown `STATUS` values vanish).
- Filenames are user-visible contract (`specs/caldir.md` §"Filename Convention";
  `slugify.rs:13-19`, `:48-60`). There is no rule for an item without a start time.
- Poison pill: `calendar.rs:114` `?` fails the whole calendar; `caldir-cli/src/output/agenda.rs:35`
  `?` then blanks the whole agenda across all calendars.
- `caldir-core` is `0.14.1` — pre-1.0, so renaming a public error variant is a minor bump.

## Options considered

**A — One *supported* component per file (recommended).** Arity counts VEVENT + VTODO
together; exactly one required. Files: `event.rs`, `event/error.rs`, `calendar/event.rs`,
`calendar/event/error.rs`, plus the `doctor` warning text. No writer change.
*Benefit:* preserves the 1-file-1-item invariant every consumer already assumes; the
VEVENT+VTODO silent drop becomes a loud error instead of data loss; zero change to
VEVENT-only files on disk. *Cost:* a file that loads today (VEVENT+VTODO) starts failing.

**B — Multi-item files: a file may hold N components, each an item.** *Benefit:* matches
what CalDAV servers may legitimately PUT, and nothing on disk stops loading. *Cost:*
enormous blast radius — `CalendarEvent` gains a component index alongside `path`; `delete`
becomes a rewrite; `update` becomes read-modify-write; `Calendar::event(slug)` loses its
meaning; the collision-suffix loop and `sync_file_mtime` (one mtime, N `LAST-MODIFIED`s,
`calendar/event.rs:153-163`) both break; and the merge writer is provably lossy (Context).
The honest option for a general ICS store; caldir is not one (`specs/caldir.md`
§"Relationship to vdir" names one `.ics` file per event as a shared invariant).

**C — Keep counting VEVENTs only; ignore VTODOs on disk.** Cheapest, but the mission
("local VTODO <-> domain") fails outright and the silent drop stays. Rejected on mission.

## Decision

1. A local `.ics` file must contain **exactly one supported component**, where *supported*
   = `VEVENT` or `VTODO`. Not "one component of any kind".
2. `VTIMEZONE` is ignored for counting and is not preserved on rewrite — unchanged from
   today, and already declared out of scope (`specs/caldir.md` §Components We Skip).
   Every **other** component (`VJOURNAL`, `VFREEBUSY`, unknown/`X-`) is data-bearing that
   caldir would destroy on rewrite, so its presence is an error, never a silent drop.
3. Both arity rules move together and gain a second variant so zero-supported is
   distinguishable from too-many:
   - `CalendarEventError::ExpectedSingleEvent` → `ExpectedSingleComponent { path, found: usize }`
     — "expected exactly one VEVENT or VTODO in {path}, found {found}; split it into one file per item"
   - new `CalendarEventError::NoSupportedComponent { path, found: String }`
     — "no VEVENT or VTODO in {path} (found: {found})", where `found` lists the component
     names present or the literal `nothing` for an empty VCALENDAR.
   - identical pair on `EventError`: `UnexpectedEventCount` → `UnexpectedComponentCount { expected, found }`,
     plus `NoSupportedComponent { found: String }`. These apply to RPC payloads and sync
     bases too, by the must-move-together constraint.
4. The writer is **not** changed. `write_best_event_file` keeps writing exactly one
   serialized item. Rule 1 is what makes that safe.

### Arity behavior table

| File contents | Today | Decided |
|---|---|---|
| 1 VEVENT | loads | loads (byte-identical, no regression) |
| 1 VTODO | `ExpectedSingleEvent { found: 0 }` | loads as a task |
| 0 components | `ExpectedSingleEvent { found: 0 }` (indistinguishable) | `NoSupportedComponent { found: "nothing" }` |
| 2 VEVENTs | `ExpectedSingleEvent { found: 2 }` | `ExpectedSingleComponent { found: 2 }` |
| 2 VTODOs | `ExpectedSingleEvent { found: 0 }` | `ExpectedSingleComponent { found: 2 }` |
| 1 VEVENT + 1 VTODO | **loads, VTODO silently dropped; rewrite deletes it** | `ExpectedSingleComponent { found: 2 }` |
| 1 VTODO + 1 VJOURNAL/VFREEBUSY | `ExpectedSingleEvent { found: 0 }` | ~~error naming the unsupported component~~ → **superseded by amendment 1**: the VTODO loads, the sibling is ignored and reported by `doctor` |
| 1 VEVENT + VTIMEZONE | loads, VTIMEZONE dropped on rewrite | unchanged: loads, VTIMEZONE dropped |

### VTODO filename rule

Extend `base_slug()` (`slugify.rs:13-19`); VEVENT output is unchanged.

- recurring task (has RRULE): `_task__recurring__{summary_slug}.ics`
- dated task: `_task__{time_slug}__{summary_slug}.ics`, where the date source is `DUE`,
  falling back to `DTSTART` if only that is present. `time_slug()` (`slugify.rs:48-60`) is
  reused verbatim: `%Y-%m-%d` for a date, local-tz `%Y-%m-%dT%H%M` for a datetime.
- undated task: `_task__{summary_slug}.ics`

Rationale: caldir already reserves the leading `_`+`__` slot for a distinct *kind* of file
(`_recurring__`, `slugify.rs:15`). Tasks are a distinct kind, so they group under `ls` the
same way, never collide with an event slug, and stay date-sorted within the group. `__` is
the only separator `summary_slug()` can never emit (`slugify.rs:26-33` yields `-`), so
`_task__recurring__x` is unambiguous against a task titled "recurring x". Requires a new
"Tasks" subsection under `specs/caldir.md` §"Filename Convention" (line 229) — an explicit
spec amendment, as the brief permits.

### Poison pill: ship separately, and ship it FIRST

Not in scope for this ADR — but a **prerequisite** for landing it. It is a pre-existing
bug independent of VTODO (`calendar.rs:114` + `agenda.rs:35`), it is a one-PR change with
its own tests, and upstream can judge it on its own merits rather than inside a feature
diff. Sequencing matters because this ADR *adds* error cases: without graceful
degradation a single VEVENT+VTODO file would blank the whole agenda instead of skipping
one row. Recommended shape: `Calendar::events()` returns the loaded items plus the
per-file errors (or skips and warns).

## Consequences

Easy: item identity stays `path`-shaped, so `delete`, `update`, `Calendar::event(slug)`,
the collision loop and `sync_file_mtime` keep working untouched. Silent data loss becomes
a loud, actionable error.

Hard: a VEVENT+VTODO file that "worked" now fails; `doctor` should gain a warning naming
the file and the "split it" remedy. A CalDAV resource holding multiple components cannot
be represented locally — out of scope here; `caldav/ops.rs:32-37` already drops all but
the first parsed component (pre-existing bug, separate ticket).

Must be tested: all eight table rows on `CalendarEvent::load`; the zero/two cases on
`Event::from_single_ics_str` (wire + bases); a VTODO `load -> update -> load` asserting the
file is still one VTODO; the four filename cases under the CI TZ matrix, since `time_slug`
is local-tz; and a regression test that VEVENT filenames are byte-identical to today's.

## Backward-compatibility impact

- **Local files:** VEVENT-only files are unaffected on read and write. Only VEVENT+VTODO
  files change, from "loads lossily" to "errors".
- **`.caldir` state:** unchanged. Base filenames are SHA-256 of `EventInstanceId`
  (`event_bases.rs:29-30`), which is already component-agnostic. Bases are always written
  by caldir, so the new arity errors cannot fire on existing bases.
- **RPC wire:** unchanged. No new verb, no new required field; only the error text a
  malformed payload produces changes.
- **Installed provider binaries:** unaffected — decode-side change only.
- **API:** two public error variants renamed, two added. Minor bump from `0.14.1`.

## Rejected because

- **B (multi-item files):** the writer cannot preserve untouched siblings without
  re-serializing them through known-lossy paths, and it breaks `path`-as-identity everywhere.
- **C (VEVENT-only counting):** fails the mission and keeps the silent VTODO drop.
- **Preserving VJOURNAL/VFREEBUSY alongside a supported component:** same lossy-rewrite
  problem as B, for components caldir has no model for.

## Open sub-questions

1. **UNVERIFIED:** does `icalendar` 0.17.10 surface `BEGIN:X-FOO` as `CalendarComponent::Other`
   (so it can be named in the error) or drop it before we see it? *Experiment:* parse a
   VCALENDAR with one VEVENT + one `X-FOO` component and print the component list.
2. Should `VTIMEZONE` be *preserved* rather than tolerated-and-dropped? Unchanged from
   today, so out of scope, but worth confirming. *Experiment:* pull from Radicale and
   Google; check whether either payload carries a VTIMEZONE that reaches disk.
3. The concrete return type of `Event::from_ics_str` once it yields both kinds is ADR-01's
   (one type vs two). This ADR only fixes its *arity contract*; it is written to hold under
   either shape.
4. Is renaming `ExpectedSingleEvent` / `UnexpectedEventCount` acceptable upstream, or should
   the old names be kept for continuity? Pre-1.0 says rename; a maintainer preference
   overrides. *Experiment:* none — ask t4t5.


---

## Amendment 1 — unsupported *siblings* are ignored, not fatal

**Status:** ACCEPTED. Supersedes Decision rule 2's second sentence and row 7 of the
arity table.

### What went wrong

Rule 2 said every component other than `VEVENT`, `VTODO` and `VTIMEZONE` makes the file
an error, "never a silent drop", on R19's reasoning that a silently dropped component
reads to the sync engine as a user deletion.

That reasoning is sound for a **supported** component: a `VTODO` that vanishes from a
parse is a task the diff will propose deleting. It does not transfer to a `VFREEBUSY`
sitting *beside* a `VEVENT`. That sibling carries no identity the diff could mistake for
anything, and `Event::from_ics_str` on `main` was `icalendar.events()`, which ignored it —
harmlessly, for years.

Measured old-binary vs new-binary, a `.ics` holding one `VEVENT` beside a `VFREEBUSY`, a
`VJOURNAL`, a top-level `VALARM` or an `X-WR-CUSTOM` — all legal `icalbody` content per
RFC 5545 §3.4 — went from `--json` visibility **1 to 0**. Exit 0, no warning on
`sync`/`push`/`status`. The now-incomplete listing then suppressed every delete for that
calendar (C7), so it silently stopped converging. This hit users with **no tasks at all**.

### The rule as amended

| File contents | Amended |
|---|---|
| 1 supported + N unsupported siblings | **loads**; the siblings are ignored and named by `doctor` |
| 2 supported (+ any siblings) | unchanged — `ExpectedSingleComponent { found: 2 }` |
| 0 supported, ≥1 unsupported | unchanged — error naming every unsupported component found |
| 0 components | unchanged — `NoSupportedComponent { found: "nothing" }` |
| 1 supported + `VTIMEZONE` | unchanged — loads, `VTIMEZONE` not reported |

`VTIMEZONE` was already exempted this way. The exemption was simply too narrow.

The same amendment applies at the RPC wire seam (`rpc::ics::decode_item`), for the same
reason and by the "both arity rules move together" constraint in the Context.

### What R19 still buys

R19's real property is that a document **never silently yields nothing**. That is the
zero-supported row, and it stays an error naming what was there.

For the ignored siblings, R19 is honoured by *reporting*, not by failing:
`CalendarEvent::ignored_components()` carries the names up, and `caldir doctor` prints
`⚠ VFREEBUSY in sibling.ics is ignored; caldir will drop it if it rewrites the file`.

`doctor` is the right surface and the only one that changes. The asymmetry is deliberate:
`main` ignored these components silently, so matching `main` is not a regression, but
inventing a new loud failure — or a new warning on the agenda, or a non-zero exit — for a
file that worked before **is**. `doctor` is the surface built for "tell me about my
files"; it is non-fatal, it does not change any exit code, and it says the one thing the
user can act on, which is that a rewrite of that file will drop the component
(`write_best_event_file` serializes one item and nothing else — Context, unchanged by
rule 4).
