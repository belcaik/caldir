# ADR-08: Sync-window semantics for tasks

## Status
PROPOSED (Stage 3 candidate)

## Context

The window check is the **only** guard between a local item missing from the remote response
and an `incoming Delete` that removes the user's file
(`caldir-core/src/diff/calendar_diff.rs:78-90`): `!has_occurrence_in_range` → `continue`
(`:80`); otherwise `sync_bases.get(&id).is_some()` → `incoming Delete` (`:86`), else
`outgoing Create` (`:89`). `has_occurrence_in_range` falls through to `occurs_in_range`
(`event.rs:136-141`) = `start < to && end.unwrap_or(start) > from`; `Event.start` is
non-optional (`event.rs:137`) and DTSTART is hard-required (`event/from_icalendar.rs:11`),
so the predicate is undefined for a VTODO with DUE-only or no dates. The default range is
always bounded (`utils/date_range.rs:22-32`), so the guard is live on every `pull`/`sync`.

Note the polarity — system-map Q11 states the prior imprecisely. `in_range == false` →
`continue` → **no delete and no create**; `in_range == true` *authorizes* deletion. "Always
in-window" buys pushability for undated tasks at the price of making them deletable; both
halves of the prior do not follow from one boolean.

### RFC 4791 §9.9 — time-range for VTODO (fetched from rfc-editor.org, p.92, not recalled)

The VTODO table is keyed by (DTSTART, DURATION, DUE, COMPLETED, CREATED). Rows that matter
(DURATION absent throughout; `*` = don't care). This table is also **Decision 1's rule** —
the last three rows are the ones core cannot evaluate:

| DTSTART | DUE | COMPL | CREA | server condition (RFC) | core's rule (Decision 1) |
|---|---|---|---|---|---|
| Y | Y | * | * | `(start < DUE OR start <= DTSTART) AND (end > DTSTART OR end >= DUE)` | same |
| Y | N | * | * | `start <= DTSTART AND end > DTSTART` | same |
| N | Y | * | * | `start < DUE AND end >= DUE` | same |
| N | N | Y | N | `start <= COMPLETED AND end >= COMPLETED` | **TRUE** |
| N | N | N | Y | `end > CREATED` | **TRUE** |
| N | N | N | N | **TRUE** | **TRUE** |

Two facts follow, and they are the whole ADR: (1) a VTODO with **no dates at all** matches
*every* time-range — "undated tasks are always in-window" is the RFC's own rule, not merely
a safe prior; (2) a VTODO with no DTSTART/DUE but **COMPLETED (or CREATED) older than
`start`** evaluates FALSE and a **compliant server MUST NOT return it**, while core, seeing
no DTSTART and no DUE, calls that same task in-window. The two predicates disagree *by
specification*, with no server bug involved.

### Q12 verdict: the undated-task deletion hazard is **REAL**

Trace, entirely in real code, for `caldir pull` against a compliant CalDAV server:

1. Naive fix swaps `<C:comp-filter name="VEVENT">` → `"VTODO"` at
   `caldir-provider-caldav/src/caldav/ops/list_events.rs:103`, keeping `<C:time-range>` (`:104`).
2. Task `T` (`STATUS:COMPLETED`, `COMPLETED:20230114T…`, no DTSTART, no DUE) synced last
   year, so `state/bases/<sha>.ics` + `known_event_ids` hold its id (`connection.rs:206`,
   `:212`; `state/sync_bases.rs:48-66`). Nothing ever removes those entries.
3. §9.9 row `N N N Y N`: `start <= COMPLETED` is false for `start = today-365d`, so the
   server omits `T`; `fetch_events` returns without it (`list_events.rs:135-183`) and
   `remote_by_id` (`calendar_diff.rs:25`) has no entry.
4. `calendar_diff.rs:36` misses → falls to `:78`. At `:80` the task is undated, hence
   in-window per the RFC and per Decision 1 → **no skip**.
5. `:84` `sync_bases.get(&id).is_some()` → true (delete inference is component-agnostic) →
   `:86` `incoming Delete`. `pull` applies it (`caldir-cli/src/commands/pull.rs:69` →
   `connection.rs:66`) and the file is gone. `allow_mass_delete`
   (`caldir-cli/src/utils/guards.rs:10`) counts only **outgoing** deletes, so it never fires.

**REAL, not theoretical, for the naive implementation — the single most important finding in
the VTODO effort.** Silent, needs no server bug, and targets completed/undated tasks, i.e.
the user's archive. It becomes theoretical only once the time-range leaves the VTODO query
(Decision 2): decisions 1 and 2 are one decision, and shipping the window rule without the
query rule *creates* the bug.

## Options considered

**A. Mirror the §9.9 table in core, keep the time-range on the wire.** Core must model and
preserve CREATED and COMPLETED purely to reproduce a server-side predicate — `event.rs`,
`from_icalendar.rs`, `to_icalendar.rs`, domain type: 4+ files, and a permanent coupling of
core's window semantics to one transport's filter. Smallest payload, but correctness then
depends on every server implementing the table exactly (Radicale's fidelity: **UNVERIFIED:**).

**B. Send no `<C:time-range>` for VTODO; fetch every task; keep the local rule simple.**
Cost: the whole task collection on every list. Blast radius: `ops/list_events.rs` (one
query), `calendar_diff.rs` (one predicate), `ops.rs:107` (UID lookup — no time-range there).
Server and core cannot disagree because the server no longer filters, so absence becomes
*meaningful* — precisely what `calendar_diff.rs:84` already assumes.

**C. Two queries: time-ranged VTODO plus an `is-not-defined` query for undated tasks**
(§9.7.2 permits `<C:prop-filter name="DUE"><C:is-not-defined/>`). Two round trips, ANDed
prop-filter semantics to get right, and it still misses dated-but-out-of-window tasks and
the COMPLETED rows.

## Decision

**1. In-window rule for tasks.** Replace the call at `calendar_diff.rs:80` with a
component-aware predicate: the **"core's rule" column of the §9.9 table above**, with
`start = from`, `end = to`. DUE only → `from < DUE && to >= DUE`; DTSTART only →
`from <= DTSTART && to > DTSTART`; both → the row-1 condition; **neither → `true`,
unconditionally**. COMPLETED and CREATED are **deliberately not consulted**: core must be
*more* permissive than any server, never less. VEVENT keeps
`occurs_in_range` byte-for-byte unchanged. **No synthesized dates** — never give an undated
task a fabricated start (epoch, COMPLETED, now) to reuse the event predicate; an epoch start
evaluates false and silently makes undated tasks unpushable at `:89`.

**2. The CalDAV VTODO `calendar-query` carries NO `<C:time-range>`.** Emit the comp-filter
bare (`<C:comp-filter name="VTODO"/>`) at `ops/list_events.rs:103`; tasks are fetched
unfiltered. Issue it as a **separate REPORT** from the existing VEVENT+time-range query,
not as a sibling comp-filter: §9.7.1 says a comp-filter matches when its children "**all**
match", i.e. siblings read as AND, which would match nothing. (Most servers treat siblings
as OR — **UNVERIFIED:** for Radicale. Two REPORTs sidestep the ambiguity and leave the
VEVENT query untouched, guaranteeing zero VEVENT regression.) `ops.rs:107` (UID lookup) has
no time-range and needs only the component-name treatment.

**3. Belt for version skew: never infer a task delete from a task-less response.** In
`CalendarDiff::compute`, if the remote response contains **zero tasks** while local holds at
least one task with a known base, skip *all* task `incoming Delete`s for that run and warn.
An installed pre-VTODO caldav provider still filters on VEVENT (`list_events.rs:103`) and
returns no tasks; so does webcal, which already drops VTODO silently. Without this, a
downgrade or mixed-version tarball wipes every synced task. It is inference from data, so it
needs **no RPC change** — the response is a bare `Vec<Event>`
(`rpc/methods/list_events.rs:15`), and making it an object would hard-break installed binaries.

Decisions 2 and 3 cover each other's gap: 2 removes *partial* server-side filtering (the
COMPLETED row); 3 covers *total* absence (old binary, wrong component name).

## Consequences

Easy: undated someday/inbox tasks sync up (`:89` is reachable), round-trip, and are never
window-deleted; remote task deletes still propagate, because an unfiltered response makes
absence meaningful. Hard: a collection with thousands of completed tasks transfers in full
every list — if payload bites, the fix is WebDAV-Sync (RFC 6578) sync tokens, **not** a
filter, since re-adding any VTODO filter reintroduces this bug. Also: deleting *every* task
remotely leaves the local files until one task exists again (Decision 3's failure mode) —
the intended safe direction.

Must be tested:
- `CalendarDiff::compute`, undated local task + known base + **empty** remote + bounded
  range ⇒ `incoming.is_empty()` **and** `outgoing.is_empty()` (Decision 3); same with a
  non-empty remote task list that omits the task ⇒ `incoming` has the Delete (deletes must
  still work); with **no** base + empty remote ⇒ exactly one `outgoing Create`.
- All four date combinations at both window edges, in all three CI timezones.
- VEVENT regression lock: an out-of-window event still produces no change.
- Provider: the emitted VTODO REPORT body contains no `time-range` substring.

## Backward-compatibility impact

- **Local files on disk:** none; this ADR rewrites no file. **`.caldir` state:** none —
  `known_event_ids` and `bases/` formats unchanged; only *when* their presence may authorize
  a delete changes.
- **RPC wire:** none. No new method (`Method` has no `#[serde(other)]`), no changed response
  shape; `ListEvents.from`/`to` are still sent and still honoured for VEVENT.
- **Installed provider binaries:** an old caldav provider returns no tasks; Decision 3 makes
  that a no-op instead of a mass delete. New provider + old core is unaffected — old core
  drops VTODO at `event.rs:101` regardless.

## Rejected because

- **A (mirror §9.9 in core):** carries CREATED/COMPLETED solely to imitate a transport
  filter and still trusts every server to match the table; failure mode stays "delete data".
- **C (two queries with `is-not-defined`):** two round trips and still leaks the COMPLETED
  row — buys payload at the price of keeping the data-loss bug.

## Open sub-questions

- **UNVERIFIED:** does Radicale implement the §9.9 VTODO table (rows `N N N Y N`,
  `N N N N N`)? *Experiment:* PUT an undated VTODO and an undated
  `COMPLETED:20200101T000000Z` VTODO, run the VTODO query with and without `<C:time-range>`,
  diff the result sets. Decision 2 is correct either way; the answer only says how bad the
  naive alternative would have been.
- **UNVERIFIED:** are sibling comp-filters under `VCALENDAR` OR'd in Radicale? If yes the two
  REPORTs could later merge into one. Not worth the risk now.
- Should Decision 3's warning be a `DoctorWarning` (`doctor.rs:55-57`) or a pull-time stderr
  line? Deferred to the CLI-surface ADR; the *skip* is not deferred.
- Does the rule apply to a task's RRULE expansion? `has_occurrence_in_range` expands masters
  (`event.rs:145`) but the `recurrence` feature is off. Proposal: a recurring undated task is
  in-window (row 6 already says TRUE); revisit if a user reports it.
