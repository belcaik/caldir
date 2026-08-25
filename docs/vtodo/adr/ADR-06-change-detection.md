# ADR-06: Change detection and equality for Todo

## Status
PROPOSED (Stage 3 candidate)

## Context

`==` on the domain component is the sync engine's *only* "has the content changed?"
test. It is consulted four times per run:

- pass 1 short-circuit `event == remote_event.event()` (`diff/calendar_diff.rs:38`)
- three-way base compare, both arms (`calendar_diff.rs:155`, `:156`)
- base-backfill "both sides agree" (`connection.rs:177`, `:183`)
- base-file rewrite skip (`state/event_bases.rs:34`)

`Event` tunes it with educe (`event.rs:38`, `:63`–`:67`, `:283`, `:290`): ignore
`last_modified` + `sequence`, order-independent `x_properties`, attachments by `uri`.

**DTSTAMP, verified:** `Event` has no `dtstamp` field at all. `from_icalendar.rs`
never reads it (only `X-*` props are captured verbatim, `:60`–`:65`), and
`to_ics_string` lets the icalendar crate auto-stamp a fresh one on every serialize
(`specs/caldir.md:34`, `:36`; the base-writer comments on exactly this at
`state/event_bases.rs:33`). So DTSTAMP is already *dropped*, not merely ignored —
a strictly stronger guarantee than `#[educe(PartialEq(ignore))]`, because it also
keeps DTSTAMP out of `Debug`, out of base files' meaning, and out of the wire codec.

Direction: `update_direction` (`calendar_diff.rs:153`) is three-way against the base
first; the mtime/`LAST-MODIFIED` fallback (`:180`) runs only when both sides changed
or no base exists.

Todo pressure: a task may carry no `LAST-MODIFIED` and no `SEQUENCE` (both optional
in RFC 5545; Radicale stores what the client PUT); `COMPLETED`/`PERCENT-COMPLETE`/
`STATUS` are content a user edits; icalendar 0.17.10 loses floating `COMPLETED` and
unknown `STATUS` values (BRIEF:64) and synthesizes a fresh random VALARM UID + DTSTAMP
per serialize (BRIEF:62).

## Options considered

**A. Todo replicates `Event`'s educe tuning exactly; direction logic unchanged.**
Cost: none beyond the derive; Todo's ignore-list must be maintained in parallel with
`Event`'s (two places to forget). Blast radius: the Todo struct file only.

**B. A `ContentEq` trait / shared macro so Event and Todo cannot drift.** Fair case:
the ignore-list is a real invariant and a field added to one but not the other is a
silent churn bug. Cost: a new abstraction in a lean upstream crate, touching
`event.rs`, the todo module, and every `==` call site; educe already says this
declaratively. **C. Content-hash both sides** (hash in the base, not full ICS). Cost:
rewrites `state/event_bases.rs`, loses the three-way *content* compare that produces
readable diffs, hides the ignore-list. Blast radius: the state format.

## Decision

**A.** Todo gets the same educe treatment as `Event`, no new abstraction, and the
direction/base machinery is used **unchanged**.

Exact ignore-list for Todo equality:

| Field | In `==`? | Why |
| --- | --- | --- |
| `last_modified` | **ignored** (`educe(PartialEq(ignore))`) | provider clock, not content — mirrors `event.rs:63` |
| `sequence` | **ignored** | revision counter, not content — mirrors `event.rs:67` |
| `dtstamp` | **field does not exist** | see verdict below |
| `x_properties` | order-independent (`x_properties_eq`) | mirrors `event.rs:283` |
| `attachments` (if modelled) | by `uri` only (`attachments_eq`) | mirrors `event.rs:290` |
| `status` (`TodoStatus`) | **participates** | user-visible state; NEEDS-ACTION→COMPLETED *is* a change |
| `completed` | **participates** | user-visible; the timestamp of the state change is content |
| `percent_complete` | **participates** | user-visible; 0→50 is a real edit that must sync |
| `due`, `start`, `priority`, `summary`, `description`, … | participate | ordinary content |

**DTSTAMP verdict: do not add a `dtstamp` field to Todo.** Follow `Event` exactly —
drop it at the parse boundary and let serialization re-stamp it. This is the trap the
question names: DTSTAMP changes on every emit for many producers (u_crawler included),
so a `dtstamp` field — even one marked `ignore` — is a liability, because it would
still ride into base files and the RPC codec where a byte-level comparison could
resurface it. Not-a-field beats ignored-field.

**`COMPLETED`/`PERCENT-COMPLETE`/`STATUS` must all participate.** They *look* like
`last_modified` because they move over time, but they move only when a human or agent
acts — never as a side effect of serialization. Ignoring any of them would mean "task
completed" never syncs: the worst possible silent failure for VTODO.

**Direction fallback, no `LAST-MODIFIED` / no `SEQUENCE` — traced behavior:**
`local_is_newer` (`calendar_diff.rs:180`): `remote.modified_at()` is `None` but both
sequences are `0`, so the sequence tiebreak at `:181` is skipped; the match then hits
`(Some(_), None) => true` at `:187` (local mtime always exists — the file is on disk),
so `local_is_newer == true` → **Push**. Compounding it, `sync_file_mtime`
(`calendar/event.rs:153`) is a *no-op* when `last_modified` is `None`, so after every
local write the file keeps its wall-clock write time and looks even newer.

**Verdict: the Push default is acceptable; keep it.** Wrong-Pull destroys the user's
local edit with no recovery — plaintext files are the source of truth. Wrong-Push
overwrites a remote edit, but is only reachable when the base is absent or both sides
changed, and is recorded back as a base on the same run (`connection.rs:248`), so it
converges after one cycle instead of oscillating. This is existing behavior, not new:
LAST-MODIFIED-less VEVENTs take the same path today.

**Sync bases work for todos as-is, with one dependency.** `bases/<sha256(id)>.ics`
holds a full serialized component keyed by `EventInstanceId`, which is
component-agnostic (BRIEF:41). The write path (`event_bases.rs:34`) and the load path
(`:60`) both call `Event::load_single` → `from_single_ics_str` → the
`UnexpectedEventCount` arity rule (`event.rs:116`). **Hard dependency on ADR-03:**
until that rule can return a Todo, a todo base file loads as zero components and is
silently skipped at `:60`, degrading every todo to a legacy known-id entry — i.e. the
three-way compare never engages and *every* todo falls to the Push-biased fallback
forever. ADR-06 requires ADR-03 to land first; it adds no state-format change of its own.

## Consequences

Easy: todos ride the existing diff, base, backfill and delete-inference machinery with
zero new sync code. Hard: every Todo field added later must be classified into the table
above and must survive `to_ics_string` → `from_ics_str` losslessly, or it churns.

### Churn scenarios and the test that catches each

Churn = the item is perceived as changed on every run. **All churn is content-inequality
churn**: pass 1 short-circuits on `==` (`calendar_diff.rs:38`) *before* direction is
consulted, so mtime/sequence can never by themselves cause churn — they only pick which
way an already-detected difference ping-pongs.

| # | Mechanism | Test |
| --- | --- | --- |
| 1 | DTSTAMP treated as content | serialize one Todo twice with different wall clocks, parse both, assert `==` |
| 2 | Lossy serialize of `COMPLETED`/`PERCENT-COMPLETE`/`STATUS:IN-PROCESS` | round-trip property test: `Todo → to_ics_string → parse` asserts `==` for every field combination |
| 3 | Floating `COMPLETED` reads `None` (BRIEF:64) — remote keeps its value, local writes nothing | parse a fixture with `COMPLETED:20260101T090000` (no `Z`), round-trip, assert no drift; if lossy, normalize at the parse boundary |
| 4 | Unknown `STATUS` value vanishes (BRIEF:64) — local re-emits the default, remote keeps its own | fixture with `STATUS:X-DEFERRED`, round-trip, assert preserved or explicitly dropped on both sides |
| 5 | VALARM in VTODO gains fresh random UID + DTSTAMP per serialize (BRIEF:62, ORCHESTRATOR-NOTES:8) | serialize a Todo with a reminder twice, parse both, assert `==`. **Constraint:** if reminders are on Todo they MUST use caldir's hand-rolled `Reminder::ics_block` splice (`event.rs:249`), retargeted at `END:VTODO`, never the crate's alarm builder |
| 6 | `EventLike` leaks `ends()`/DTEND onto Todo (BRIEF:63) — RFC-invalid line some parsers drop | assert `to_ics_string` for a Todo contains no `DTEND` |
| 7 | Provider-managed `X-*` props stripped by a local write | Todo must go through the `with_x_properties_merged_from` path (`remote.rs:51`); test: remote-only `X-` prop survives an outgoing Update |
| 8 | Any of the above end-to-end | **the load-bearing test**: run `sync` twice against a fake remote; assert the second `CalendarDiff::is_empty()`. One test that catches every mechanism, including ones not listed |
| 9 | Both sides `TodoStatus::Cancelled` with field drift | mirror the pass-1 skip (`calendar_diff.rs:45`) for `TodoStatus::Cancelled`; test drifted-but-both-cancelled produces no change |

Scenario 8 is the acceptance gate. Scenarios 1–7 are the diagnostics that say *why* it failed.

Not decided here: the window guard and delete inference for undated/DUE-only todos —
including whether pass 2's "missing locally + remote CANCELLED" skip
(`calendar_diff.rs:106`) extends to `COMPLETED`. **ADR-08 owns that.**

## Backward-compatibility impact

- **Local files on disk:** none. VEVENT equality, serialization and slugs are untouched.
- **`.caldir` state:** none. Same `bases/<sha256(id)>.ics` layout, same `known_event_ids`
  format, same key type. Existing bases keep loading. Contingent on ADR-03 widening the
  arity rule rather than changing the file format.
- **RPC wire:** none. Equality is a core-local concern; the wire stays opaque ICS.
- **Installed provider binaries:** none. No new verb, no new required field.

## Rejected because

- **B (shared `ContentEq` trait/macro):** buys a guarantee educe already expresses, at
  the cost of an abstraction across every `==` call site in a deliberately lean crate.
- **C (content hashing):** rewrites the state format, destroys the three-way content
  compare, and hides the ignore-list where nobody will find it.

## Open sub-questions

1. **Does Radicale echo a PUT byte-for-byte?** If it normalizes anything caldir models
   (property order is fine — `x_properties_eq` is order-independent — but a rewritten
   `COMPLETED` form or an injected `SEQUENCE` is not), scenario 8 fails for a reason no
   unit test sees. *Cheapest experiment:* PUT a VTODO with COMPLETED + PERCENT-COMPLETE +
   an `X-` prop to a local Radicale, GET it back, diff the bytes.
2. **UNVERIFIED: does u_crawler emit `LAST-MODIFIED` on its VTODOs?** If yes, the fallback
   is never reached and the Push-bias question is moot; if no, every u_crawler task takes
   the `(Some, None) → Push` path. *Cheapest experiment:* grep one real u_crawler
   `deadlines/*.ics` for `LAST-MODIFIED` (blocked until the Stage 8 fixture swap, BRIEF:78).
3. **Should `percent_complete: None` vs `Some(0)` compare equal?** RFC 5545 gives no
   default. Two providers disagreeing here is a permanent scenario-8 failure.
   *Cheapest experiment:* covered by sub-question 1's round-trip.
