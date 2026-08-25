# ADR-04: Which properties belong in the Todo MVP

## Status
PROPOSED (Stage 3 candidate)

## Context

MVP target = the `u_crawler` `deadlines/` fixture (`docs/vtodo/reproduction.md:14-28`):
`UID DTSTAMP DUE SUMMARY DESCRIPTION URL PRIORITY STATUS:COMPLETED COMPLETED`. Non-negotiably IN.
Everything else must justify itself against *silent data loss on write* — a PUT that omits a
property the server holds deletes it (`specs/caldir.md` §ATTACH, RFC 8607).

**Assumption A05 (stated, not designed here).** ADR-05 gives verbatim round-trip of *unmodelled
properties*, single- and multi-valued, with params. The crate supports it: unknown props+params
survive verbatim, and `MULTIS` (`src/parser/properties.rs:27-44`) keeps `ATTACH ATTENDEE
CATEGORIES COMMENT CONTACT EXDATE FREEBUSY RDATE RESOURCES` multi-valued. I assume A05 does
**not** cover child *components* — the crate exposes them as `Other`, which is not publicly
nameable (`icalendar-capabilities.md` LIMITATION 11). *If A05 lands X-`*`-only, `ATTACH`,
`ATTENDEE`, `CATEGORIES` flip OUT→IN. That is the only conditional here.*

**Requirement on ADR-05:** passthrough must **exclude** every modelled property. Emission is a
`BTreeMap` keyed by name (`src/components.rs:27`), so a duplicate silently overwrites in insert
order rather than emitting twice — a coin-flip bug.

## IN / OUT

| Property | MVP | Reason |
|---|---|---|
| `UID` | **IN** | Identity half. Fixture. |
| `DTSTAMP` | **IN, emitted not modelled** | As VEVENT: crate stamps `Utc::now()` at write, never a domain field, comparison never sees it (`specs/caldir.md` §DTSTAMP). |
| `DUE` | **IN** | The task's date anchor. Fixture. |
| `DTSTART` | **IN** (`Option`) | Radicale/Apple emit DTSTART+DUE. Modelling it gives Q11 a real window anchor (`start.or(due)`); passthrough would not. |
| `DURATION` | **OUT** | caldir never uses DURATION for events; crate emits `PT7200S` not `PT2H` (capabilities probe 2). A05. |
| `SUMMARY` | **IN** | Fixture; filename slug source. |
| `DESCRIPTION` | **IN** | Fixture. |
| `URL` | **IN** | Fixture. |
| `PRIORITY` | **IN** | Fixture. **Not via `Component::priority()`** — clamps to `0..=10`, `get_priority()` returns `None` above 10 (capabilities 15). Raw property, `Option<u8>`, out-of-range preserved verbatim. |
| `STATUS` | **IN** as new `TodoStatus` | Fixture. Q4 below. Parse from the **raw** property — `get_status()` drops unknown values (capabilities 14). |
| `COMPLETED` | **IN** | Fixture. **Not via `get_completed()`** — returns `None` for a non-UTC value while the property is retained (capabilities 19). Use caldir's own `EventTime` parser. |
| `PERCENT-COMPLETE` | **OUT** | Not in the fixture; single-valued so A05 is lossless. *Fair counter:* coupled to STATUS — a hand-edit to `NEEDS-ACTION` leaves a passed-through `PERCENT-COMPLETE:100`. Inconsistency, not loss. First v2 candidate. |
| `LAST-MODIFIED` | **IN** | Fallback direction rule reads it (`diff/calendar_diff.rs:180`). Ignored by `PartialEq`, like `Event`. |
| `SEQUENCE` | **IN** | Conflict tie-break when remote has no LAST-MODIFIED (`specs/caldir.md` §SEQUENCE). Ignored by `PartialEq`. |
| `CREATED` | **OUT** | Already skipped by the spec; informational. |
| `CLASS` | **OUT** | Not needed. Free to inherit if ADR-01 picks a shared struct — harmless either way; don't add deliberately. |
| `CATEGORIES` | **OUT** | Spec skips it; in `MULTIS`, so A05 keeps it. **Flip IN if A05 is X-`*`-only.** |
| `LOCATION` | **OUT** | Rare on tasks; single-valued; A05-safe. |
| `GEO` | **OUT** | Already skipped by the spec; no typed API; A05-safe. |
| `ATTENDEE` | **OUT** | Assigned tasks are a non-goal. Multi-valued — **highest-risk OUT**: dropping it on PUT unassigns people server-side. **Flip IN if A05 is X-`*`-only.** |
| `ORGANIZER` | **OUT** | Same, single-valued, lower risk. |
| `ATTACH` | **OUT** | In `MULTIS`. Second-highest risk: under RFC 8607 a PUT omitting `ATTACH` *deletes the attachment*. **Flip IN if A05 is X-`*`-only.** |
| `RRULE`/`EXDATE`/`RDATE` | **OUT** | Recurring tasks are a non-goal; `recurrence` feature off (capabilities 17); `expand_master` needs DTSTART. A05 preserves them; caldir shows the master only. No loss. |
| `RECURRENCE-ID` | **IN** | Not for recurrence — it is the **other half of identity** (`EventInstanceId((uid, Option<RecurrenceId>))`). Parsing it as `None` collapses two instances of one recurring task onto one identity → base collision (R4). Near-free: type exists, `recurrence_id()` works on `Todo` (capabilities probe 11). |
| `RELATED-TO` | **OUT** | Q6 below. |
| `VALARM` | **IN** | Q5 below. |
| `X-*` | **IN** | Already caldir's provider-ID carrier (`event/x_property.rs`); `XProperty` + order-independent `PartialEq` exist. Known loss unchanged from VEVENT: repeated same-name `X-*` collapse (capabilities 2). |
| `DTEND` | **FORBIDDEN** | Not merely OUT: `EventLike` leaks `ends()` onto `Todo` (capabilities 13) and will write a `DTEND` that RFC 5545 §3.6.2 forbids in a VTODO. Needs a test. |

## Q4 — Status model

`caldir::Status` = `Confirmed|Tentative|Cancelled`, `#[default] Confirmed` (`event/status.rs:5-13`),
and is `serde::Serialize`d into CLI/JSON output. `TodoStatus` (crate) =
`NeedsAction|Completed|InProcess|Cancelled` (`src/properties.rs:288-301`).

**Decision: a separate `TodoStatus` in caldir-core**, mirroring `status.rs`
(`as_ics_str`/`from_ics_str`/`Display`), `#[default] NeedsAction`. Not shared, not generic. RFC
5545 defines disjoint value sets per component; `Confirmed` is meaningless for a task and
`NeedsAction` for an event. Merging adds impossible variants to every existing VEVENT match site
and changes the public JSON enum for events — a user-visible break for zero benefit. Generic
buys trait-object churn for a four-variant enum. Cost: ~40 lines, one new file, no VEVENT edit.

**Both `Cancelled` skip-rules must key on `Cancelled` ONLY — never on `Completed`.**

- `calendar_diff.rs:106` (missing locally + remote CANCELLED ⇒ skip): if `Completed` shared it, a
  remote completed task would **never be created locally** — and the u_crawler fixture *is*
  `STATUS:COMPLETED`. The MVP's own fixture would silently fail to pull. Decisive.
- `calendar_diff.rs:45` (both CANCELLED ⇒ ignore field drift): cancelled is a tombstone;
  completed is an accomplished record users still edit and legitimately reopen. Freezing drift
  would silently discard those edits.

`Cancelled` keeps both rules for todos — an abandoned task is a tombstone exactly like a
cancelled event.

## Q5 — VALARM: verdict **IN**

Experiment `<scratchpad>/valarm-probe`, icalendar 0.17.10, default features (same as caldir).
Path A hands the parsed `Todo` (alarm child included) to the crate. Path B replicates caldir's
VEVENT trick — a `Reminder::ics_block`-shaped block (`event/reminder.rs:85-97`) spliced on the
literal `"END:VTODO\r\n"`, mirroring `event.rs:241-249`. Real output:

```
=== PATH A (crate serializes the VALARM child) ===
BEGIN:VTODO … BEGIN:VALARM\r\n
DTSTAMP:20260825T054401Z\r\n                       <- SYNTHESIZED
ACTION:DISPLAY / DESCRIPTION:Reminder / TRIGGER;RELATED=START:-PT30M
UID:095066dd-85ca-4269-9f51-9000572d0d09\r\n       <- SYNTHESIZED, fresh every call
A stable across two serializes? false
PATH A: two generations structurally equal (incl. VALARM child)? false
PATH A: VALARM UIDs -> Some("260a3f8a-…") vs Some("1abcfef4-…")

=== PATH B (hand-rolled splice on "END:VTODO\r\n") ===
extracted reminder minutes = Some(30)
BEGIN:VTODO … BEGIN:VALARM\r\n ACTION:DISPLAY\r\n DESCRIPTION:Reminder\r\n
TRIGGER;RELATED=START:-PT30M\r\n END:VALARM\r\n END:VTODO\r\n   (no UID, no DTSTAMP)
B stable across two serializes? true
gen1 == gen2 bytes? true      gen2 == gen3 bytes? true
reminder survives 3 generations? Some(30) Some(30) Some(30)
=== CHURN CRITERION (DTSTAMP excluded, as caldir's PartialEq does) ===
bytes equal WITHOUT dtstamp? true
rebuilt Todo (no DTSTAMP) equal across generations? true
```

Path A reproduces R2 exactly: a fresh UUID per serialize ⇒ two generations structurally unequal
⇒ a Push every sync, forever. Path B is byte-stable across three parse→serialize generations and
emits no VALARM `UID`/`DTSTAMP`. **The hand-rolled approach removes the churn for VTODO as it
already does for VEVENT.**

So IN, and the argument is *safety*, not cheapness: under A05 a VALARM is a child component, not
a property, so passthrough does not cover it — leaving it OUT means caldir's first write to a
Radicale task silently **destroys** the user's reminder. Cost: one extra splice literal, plus
generalising `Reminder::from_ical_event(&icalendar::Event)` (`event/reminder.rs:61`) over
`C: Component` — its helper `from_valarm` is already generic (`:72`) and reads only
`.components()` and `TRIGGER`.

## Q6 — RELATED-TO / subtasks: **non-goal**

Lossy before caldir sees the data: `MULTIS` lists the ABNF placeholder `"RELATED"`, not
`RELATED-TO` (`src/parser/properties.rs:40`), so two `RELATED-TO` lines parse to one, last wins,
`PARENT` silently gone (capabilities probe output). No cheap safe path: the x-property mechanism
round-trips *params* verbatim but repeated `X-*` collapse identically (capabilities 2), so
re-encoding hierarchies as `X-CALDIR-RELATED-TO` inherits the same defect.

Decision: not modelled. A *single* `RELATED-TO` survives for free as an unmodelled property under
A05 — that answers Q6's narrow form ("preserve one opaquely, or drop it?") with **preserve, at
zero cost, by doing nothing**. Residual R9 stands and caldir cannot detect it (the loss is inside
the parser). Cheap mitigation I recommend but do not design here: count `RELATED-TO` lines in the
**raw text** pre-parse and refuse to rewrite that file, warning via doctor. Either way, gate the
loss with a test.

## Q15 — The exact `specs/caldir.md` edit

Three changes, nothing more:

1. **§"Fields We Skip" — modify one row.** `PRIORITY`'s reason becomes:
   `Skipped on VEVENT—almost never used. Used on VTODO (see §VTODO).` Do **not** delete the row;
   VEVENT scope is unchanged.
2. **§"Fields We Skip" — add one row.** `` | `RELATED-TO` | Task hierarchies—the parser collapses
   repeated values, so modelling them would be lossy. A single value is preserved verbatim. | ``
3. **Add a `## VTODO (Task)` section** after the VEVENT section documenting only the IN set:
   `UID DTSTAMP DTSTART DUE SUMMARY DESCRIPTION URL PRIORITY STATUS
   (NEEDS-ACTION/COMPLETED/IN-PROCESS/CANCELLED) COMPLETED LAST-MODIFIED SEQUENCE RECURRENCE-ID
   VALARM X-*`, plus one explicit sentence: *"VTODO uses DUE, never DTEND (RFC-invalid) and never
   DURATION."*

`CREATED CATEGORIES GEO RESOURCES RDATE CONTACT COMMENT VTIMEZONE` rows are **unchanged**.
§"Filename Convention" also needs a line, but that is Q3's decision, not this ADR's.

## Options considered

- **A. Mirror the full `Event` field set onto `Todo`** (CLASS, LOCATION, ATTENDEE, ORGANIZER,
  ATTACH, recurrence). Benefit: no ADR-05 dependency; every risky multi-valued property explicitly
  owned; nearly free if ADR-01 picks a shared struct. Cost: ~8 extra mappings plus tests, a much
  larger upstream PR, and it commits caldir to task semantics for fields (ATTENDEE, RRULE) whose
  provider behaviour is unverified. Blast radius: `event/from_icalendar.rs`, `to_icalendar.rs`, the
  new todo module, a much wider `specs/caldir.md` amendment.
- **B. Fixture-only** (the 9 fixture properties). Smallest possible. Drops `RECURRENCE-ID`
  (identity collision R4), `LAST-MODIFIED`/`SEQUENCE` (no conflict signal), `VALARM` (reminder
  destroyed on write).
- **C. This ADR:** B plus the properties load-bearing for *correctness* rather than for the
  fixture — RECURRENCE-ID, LAST-MODIFIED, SEQUENCE, VALARM — plus DTSTART and X-*.

## Decision

Adopt **C** — the IN column above, exactly. `TodoStatus` is a new enum in caldir-core, not an
extension of `Status`. Both diff skip-rules key on `Cancelled` only. VALARM is emitted through a
hand-rolled block spliced on `"END:VTODO\r\n"`, never through `EventLike::alarm`. `DTEND` is
never emitted on a VTODO. Everything else is OUT and relies on A05 passthrough, which must
exclude every modelled property.

## Consequences

Easy: the fixture round-trips byte-stably; a completed task pulls and stays; reminders survive;
two instances of a recurring task keep separate identities. Hard: assigned tasks, subtask trees
and recurring tasks are passthrough only — caldir will not reason about them.

Must be tested: (1) fixture parse→serialize→parse byte + structural stability across all three CI
timezones; (2) locally-deleted + remote-`COMPLETED` ⇒ outgoing `Delete`; (3) remote-`COMPLETED`,
absent locally, no base ⇒ incoming `Create`; (4) VALARM three-generation byte stability (the probe
above, ported inline); (5) no `DTEND` ever emitted on a VTODO; (6) `PRIORITY:11` and a floating
`COMPLETED` round-trip verbatim rather than vanishing; (7) `RELATED-TO` ×2 — assert the known loss.

## Backward-compatibility impact

- **Local files:** none. No VEVENT field changes; `Status` untouched.
- **`.caldir` state:** none. Bases are ICS keyed by `EventInstanceId`, which already carries
  `Option<RecurrenceId>`; no new key shape.
- **RPC wire:** none. Rides the existing opaque ICS payload (`rpc/ics.rs`); no new `Method` verb,
  no new required field.
- **Installed provider binaries:** none. Nothing here changes an existing VEVENT payload.

## Rejected because

- Shared `Status` enum: impossible variants at every VEVENT match site; breaks the public JSON enum.
- Generic status: trait-object churn for a four-variant enum.
- `Completed` sharing `Cancelled`'s skip-rules: the MVP fixture would never be created locally.
- Option A: commits to unverified provider semantics and balloons the upstream PR.
- Option B: three correctness holes (identity, conflict signal, alarm loss).
- Modelling `RELATED-TO`: lossy inside the parser; the x-property escape hatch has the same defect.
- `PERCENT-COMPLETE`: single-valued, so A05 is lossless; not needed for the fixture.

## Open sub-questions

- **A05's scope is the one live dependency.** If ADR-05 is X-`*`-only, `ATTACH`, `ATTENDEE`,
  `CATEGORIES` must flip to IN or caldir deletes server-side data on first write. *Cheapest
  experiment:* PUT a task with `ATTACH` ×2 to Radicale, let caldir rewrite it, re-GET.
- **UNVERIFIED:** whether Radicale rejects `STATUS:COMPLETED` without `PERCENT-COMPLETE:100`.
  *Experiment:* PUT both shapes, check the response. If it rejects, `PERCENT-COMPLETE` flips IN.
- **UNVERIFIED:** whether any targeted provider emits a VTODO `DURATION`. RFC 5545 forbids
  `DUE`+`DURATION` together, so passing `DURATION` through while modelling `DUE` could emit an
  invalid component. *Experiment:* grep a real Radicale/Apple task export for `DURATION`.
