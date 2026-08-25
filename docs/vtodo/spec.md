# Spec: first-class VTODO support in caldir

Binding inputs, in precedence order: `docs/vtodo/adr/ADJUDICATION.md` (**wins over any ADR it
contradicts**), `docs/vtodo/context/radicale-probe.md` (live Radicale 3.7.6),
`docs/vtodo/reproduction.md`, `docs/vtodo/adr/GRILL-FINDINGS.md`,
`docs/vtodo/context/system-map.md`, ADR-01..06 and ADR-08..10. **ADR-07 is CUT** and appears
only in §16.

Normative keywords are MUST / MUST NOT / SHOULD / MAY; every normative statement is numbered
(`R1`…) for citation. **INVIOLABLE** requirements may not be traded away for any benefit.
Unsettled claims are marked `UNVERIFIED:` with what would settle them.

---

## 1. Motivation

caldir's premise is that a calendar is a directory of plaintext `.ics` files you can grep,
script, and migrate. RFC 5545 defines four calendar-data components; caldir models one. A
`VTODO` — a task, a deadline — is ordinary calendar data that CalDAV servers store and serve
alongside events, and that Apple Reminders, Thunderbird, and every other CalDAV task client
produces.

Today caldir does not merely ignore a VTODO: it is a poison pill that blanks the user's entire
agenda across every calendar (§3). The promise — *your calendar is just files, so put files in
it* — is false for half the component types a CalDAV server will hand you.

The goal is a task that parses, round-trips, syncs bidirectionally with a generic CalDAV
server, survives a rewrite without destroying server-side data, and is **never** an event.

**R1 (INVIOLABLE).** caldir MUST NOT convert a VTODO into a VEVENT or vice versa at any layer —
disk, domain, sync base, RPC wire, provider. No synthesized `DTSTART`, no synthesized `DTEND`,
no fake-event representation of a task anywhere.

**R2 (INVIOLABLE).** VEVENT behaviour MUST NOT change observably: same bytes, filenames, parse
result, equality, sync direction, text output (§15 C1/C2/C13).

## 2. Real integration / use case

```
Canvas LMS -> u_crawler -> ICS files on disk -> caldir -> Radicale -> CalDAV clients
  (source)    (scraper)     (a caldir tree)     (sync)    (server)   (phone, laptop)
```

`u_crawler` scrapes a university LMS and emits `.ics` files into a caldir tree: a **VEVENT** for
the submission *window* and a **VTODO** for the *deadline*, both carrying the same `UID`
(`assignment-123@u-crawler`, verified in the Stage 2 fixture — `reproduction.md:14-33`). caldir
pushes both to a self-hosted Radicale, from which ordinary clients read them: events in the
calendar app, tasks in the task app.

**UNVERIFIED: the u_crawler directory layout.** The user describes sibling `windows/` and
`deadlines/` directories under `calendar_dir`, but no such layout exists on disk —
`/home/belcaik/Dev/u_crawler` has no `windows/`, no `deadlines/`, and no `.ics` files at all;
`~/caldir` and `~/.config/caldir/` do not exist (system-map §5c; ADJUDICATION "Unverified
premise"). Treated as accurate for design purposes; nothing in this spec depends on it.
*Settled by:* the Stage 8 fixture swap.

What is verified: siblings under `calendar_dir` are **two separate calendars**
(`caldir.rs:71-83`, non-recursive, direct non-hidden subdirectories only); nested directories are
not calendars and their files are never read (`calendar.rs:109-115`). Either way the same-UID
pair lands in two calendars — the shape Radicale permits (same UID in a different collection:
`201`, probe Q-E.5).

**R3.** One calendar holding VEVENTs and a sibling holding same-UID VTODOs MUST work, with no
cross-contamination at either the core or the CLI level.

**R4.** One local calendar directory MAY hold both kinds, including a same-UID pair. Supported,
non-error local state (R41/R100).

## 3. Current observed failure

Verified by the Stage 2 RED tests, zero production lines changed. The mechanism is confirmed:
the ICS parses cleanly and the component is present —

```
PROBE vtodo components=1 events=0 todos=1
PROBE vtodo from_ics_str len=Ok(0)
PROBE vtodo from_single=Some("expected 1 event(s) in ICS, found 0")
```

The loss happens at one line — `Ok(icalendar.events().map(Event::try_from).collect())`,
`caldir-core/src/event.rs:101`, the only `.events()` call site in the workspace. It filters to
VEVENT before any error can be raised; the arity check then reports "found 0".

### The three-level blast radius (all verified by test)

1. **The file** — `ExpectedSingleEvent { path, found: 0 }` (`calendar/event.rs:40-50`).
2. **The whole enclosing calendar** — the `?` at `calendar.rs:114` makes `Calendar::events()`
   return `Err`, so the VTODO's **VEVENT siblings in the same directory become invisible too**.
3. **Every other calendar, at the CLI level** — `AgendaView::collect`
   (`caldir-cli/src/output/agenda.rs:35`) `?`-propagates, so **the aggregation aborts**:
   `caldir events` / `today` / `week` error and show no events at all, *including the events of
   unrelated, perfectly healthy calendars*. **One VTODO file anywhere under the caldir blanks
   the user's entire agenda.**

Real captured output (`reproduction.md` §6):

```
---- calendar::tests::events_lists_a_vtodo_file_alongside_its_vevent_sibling stdout ----
one VTODO file aborted the whole calendar listing, including the VEVENT sibling:
Event(ExpectedSingleEvent { path: ".../test-calendar/assignment-123.ics", found: 0 })

---- caldir::tests::aggregating_all_calendars_survives_a_vtodo_calendar stdout ----
aggregating calendar Some("deadlines") failed, so the whole agenda across every calendar
fails: Event(ExpectedSingleEvent { path: ".../deadlines/assignment-123.ics", found: 0 })

test result: FAILED. 0 passed; 5 failed; 0 ignored; 0 measured; 467 filtered out
```

### The second, quieter failure

A file holding one VEVENT **and** one VTODO produces no error — `PROBE mixed components=2
events=1 todos=1 / from_ics_str len=1 first_ok=true`. The VTODO is silently dropped, the file
looks healthy, and caldir's next write to it deletes the VTODO on disk. The same silent drop
exists remotely: `caldav/ops.rs:32-37` uses `find_map(Result::ok)` and keeps only the first
parseable component, and a VTODO-only webcal feed yields zero items with no warning
(`caldir-provider-webcal/src/commands/list_events.rs:16-25`).

**R5.** A file containing exactly one VTODO MUST load, MUST NOT error, and MUST NOT prevent any
sibling file, sibling calendar, or CLI aggregation from loading.

**R6.** Levels 2 and 3 are a pre-existing defect independent of VTODO modelling. Graceful
per-file degradation SHOULD ship first, as its own change, and that change MUST include R48 —
without it, a degrading core paired with a VTODO-aware provider deletes the user's tasks
server-side.

## 4. Terminology

| Term | Meaning |
|---|---|
| **item** | One supported component with an identity: an *event* or a *task*. |
| **event** | A `VEVENT`. The existing `Event` type, unchanged. |
| **task** | A `VTODO`. A new sibling type; never an `Event`. |
| **component kind** | The two-valued discriminator `{ Event, Todo }` carried in identity. |
| **supported component** | `VEVENT` or `VTODO`. Not "any component". |
| **calendar** | A non-hidden direct subdirectory of `calendar_dir`. |
| **item identity** | `(kind, uid, Option<recurrence_id>)` — R36. |
| **sync base** | Last-synced serialization, `state/bases/<sha256(id)>.ics`. |
| **window** | The `DateRange` a pull/sync operates over; CLI default ±365 days. |
| **in-window** | The predicate at `calendar_diff.rs:80` that *authorizes* a delete. |
| **undated task** | A task with neither `DTSTART` nor `DUE`. |
| **anchor** | A task's `DUE`, falling back to `DTSTART`; `None` when undated. |

## 5. Domain invariants

**R7 (INVIOLABLE).** An item's kind is determined solely by the component it was parsed from and
is preserved across every boundary: disk, base, wire, provider, server.

**R8.** A task MUST NOT carry `DTEND` or `DURATION`. RFC 5545 §3.6.2 forbids `DTEND` in a VTODO,
and `icalendar` 0.17.10 emits one via the leaked `EventLike::ends()` (verified on pinned
`=0.17.10`, GRILL C-3). Emitting it is a defect, not merely out of scope.

**R9.** A task MAY have `DTSTART`, `DUE`, both, or neither. All four are first-class; none may be
normalized away or given a synthesized value (R46).

**R10.** One `.ics` file holds exactly one supported component (R32).

**R11.** Identity is derived from ICS content, never from a filename; a rename MUST NOT look like
delete-plus-create to sync (`event.rs:252`, `calendar.rs:113`).

**R12.** `==` on the domain item is the sync engine's only "content changed?" test. A field that
cannot survive `serialize -> parse` losslessly MUST NOT participate in `==`, or it generates
permanent churn.

## 6. VTODO MVP representation

The set below is ADR-04 **as amended by ADJUDICATION C-2 and C-4**. Every "A05 preserves it" /
"A05-safe" / "No loss" justification in ADR-04's OUT column is **void**: ADR-05's passthrough is
`X-*` **only**, measured. An unmodelled property survives on the server until the first genuine
edit, then dies — CalDAV update is a whole-resource PUT
(`caldir-provider-caldav/src/caldav/ops/update_event.rs:33-38`). Radicale holds all of these
faithfully (probes Q-C, Q-F2), so **anything caldir drops is caldir destroying correct server
data**.

**R13.** The task type MUST model exactly the IN set and MUST NOT model the OUT set.

| Property | MVP | Requirement / reason |
|---|---|---|
| `UID` | **IN** | Half of identity. |
| `DTSTAMP` | **IN, emitted, not modelled** | Never a domain field; re-stamped on serialize, as for events (R25). |
| `DUE` | **IN** (`Option`) | The task's anchor; the fixture's own field. |
| `DTSTART` | **IN** (`Option`) | Real producers emit both; gives R44 a real anchor. |
| `SUMMARY` | **IN** | Filename slug source. |
| `DESCRIPTION`, `URL` | **IN** | Fixture. |
| `PRIORITY` | **IN** (`Option<u8>`, raw) | MUST NOT use `Component::priority()` — clamps to `0..=10`, returns `None` above. Out-of-range MUST round-trip verbatim. |
| `STATUS` | **IN**, distinct four-valued task status | `NEEDS-ACTION` (default) / `IN-PROCESS` / `COMPLETED` / `CANCELLED`. MUST parse the **raw** property — `get_status()` drops unknown values (system-map C6). Separate from the event `Status` enum: merging adds impossible variants at every event match site and changes the public JSON enum. |
| `COMPLETED` | **IN** (`Option`) | MUST NOT use `get_completed()` — returns `None` for a non-UTC value while retaining the raw property (C5). Use caldir's own time parser. |
| `PERCENT-COMPLETE` | **IN** (`Option<u8>`) | **Flipped IN by C-2**; already on the type per ADR-01 #3 and ADR-06's equality table. |
| `CREATED` | **IN** (`Option`, ignored by `==`) | **Flipped IN by C-4 on DATA-SAFETY grounds, not fidelity**: it is the property that rescues an undated completed task from RFC 4791 §9.9 omission (§10/§12). Defence in depth, **never** a substitute for R58. |
| `LAST-MODIFIED` | **IN**, ignored by `==` | Direction fallback reads it (`calendar_diff.rs:180`). |
| `SEQUENCE` | **IN**, ignored by `==` | Tie-break when the remote has no `LAST-MODIFIED`. |
| `RECURRENCE-ID` | **IN** | Not for expansion — the **other half of identity**. Parsing it as `None` collapses two instances onto one identity. |
| `LOCATION`, `ORGANIZER`, `ATTENDEE`, `ATTACH` | **IN** | **Flipped IN by C-2.** The codec already emits all four (`to_icalendar.rs:56,72-78,87-89`). Dropping `ATTENDEE` unassigns people server-side; under RFC 8607 a PUT omitting `ATTACH` **deletes the attachment**. |
| `RRULE`/`EXDATE`/`RDATE` | **IN, modelled, not expanded** | **Flipped IN by C-2.** ADR-04's "A05 preserves them, no loss" was false — every recurring task's RRULE would die on first edit. Reuses the existing `Recurrence` type (untyped rrule string, `event/recurrence.rs:5-17`). Master only. |
| `VALARM` | **IN**, hand-rolled | R24. |
| `X-*` | **IN**, preserved **and** compared | The existing provider-ID carrier. Known loss unchanged from events: repeated same-name `X-*` collapse. |
| `CLASS` | MAY be inherited | Harmless; MUST NOT be added deliberately. |
| `CATEGORIES` | **OUT** | The crate **corrupts** it: `WORK,URGENT` re-emits as `WORK\,URGENT` (verified, pinned `=0.17.10`) — two categories become one for third parties. Plus a todo-only exception in a table titled "Fields We Skip" is incoherent. |
| `RELATED-TO` | **OUT** | Parser-lossy: `MULTIS` lists the ABNF placeholder `"RELATED"`, so two lines in yield one out, `PARENT` gone. A single value survives unmodelled. |
| `GEO`, `RESOURCES`, `COMMENT`, `CONTACT` | **OUT** | Already out of scope for events; unchanged. |
| `DURATION` | **OUT** | Never used; the crate emits `PT7200S` not `PT2H`. |
| `DTEND` | **FORBIDDEN** | R8. |

**R14.** An unknown `STATUS` value MUST NOT silently become the default: it round-trips verbatim,
or it is dropped with the loss pinned by a test (T-14).

**R15.** The user guarantee, for the spec and docs, MUST be: *caldir round-trips a task's
modelled properties plus every `X-` property verbatim, parameters included; any other property —
`CATEGORIES`, `GEO`, `RELATED-TO`, `RESOURCES`, `COMMENT`, `CONTACT`, and any other IANA or
vendor property caldir does not model — is dropped the first time caldir writes that item, on
disk and on the server alike, exactly as it already is for events.*

**R16.** `specs/caldir.md` MUST be amended in the same change: scope `PRIORITY` to "skipped on
VEVENT, used on VTODO"; add a `RELATED-TO` row to "Fields We Skip"; drop the stale `RDATE` row
(RDATE **is** implemented, `event/recurrence.rs:5-17`); add a `## VTODO (Task)` section covering
the IN set plus *"VTODO uses DUE, never DTEND (RFC-invalid) and never DURATION"*; add a Tasks
subsection to §"Filename Convention". No existing VEVENT sentence may change (R2).

## 7. ICS parse rules

**R17.** Parsing MUST make **one ordered pass** over the document's components, dispatching
`VEVENT -> event`, `VTODO -> task`, everything else -> R19. One pass, not `events()` then
`todos()`, so document order survives.

**R18.** `VTIMEZONE` MUST be ignored for counting and MUST NOT be treated as unsupported — **on
the wire and sync-base decoder as well as the disk loader**. Radicale returns a sibling
`VTIMEZONE` for any `TZID` (probe Q-C), so a real zoned task payload is a multi-component
`VCALENDAR` on the wire; counting it there would make every timezone-bearing task from Radicale
fail to decode.

**R19 (INVIOLABLE).** An unsupported data-bearing component (`VJOURNAL`, `VFREEBUSY`, unknown,
`X-`) MUST produce a **hard error naming the component** — never a silent skip, never an empty
vector. A silent drop feeds the delete path (R48, §15 C7/C10). Implementable: a document with
`VEVENT`, `X-FOO`, `VJOURNAL`, `VTODO` yields four components dispatching as
`Event / Other(X-FOO) / Other(VJOURNAL) / Todo`, each nameable (verified, pinned `=0.17.10`).

**R20.** A task MUST NOT require `DTSTART`. The event rule (`EventError::MissingStart`,
`event/from_icalendar.rs:11`) stays unchanged for events and MUST NOT apply to tasks.

**R21.** `DUE;TZID=…` MUST round-trip with its TZID intact (verified: lossless without the
`chrono-tz` feature, system-map §3). Inbound TZIDs are normalized to IANA at the parse boundary,
per the existing project rule.

**R22.** A floating (non-UTC) `COMPLETED` MUST be read by caldir's own parser and MUST NOT be
silently lost — `get_completed()` returns `None` for it while the raw property is retained, so
"present but unreadable" is reachable (C5).

**R23.** Parsing MUST capture `X-*` with name, value, and parameters using the same filter events
use, and MUST NOT capture any non-`X-` property into a passthrough bag.

## 8. ICS serialization rules

**R24.** A task's `VALARM` MUST be emitted through caldir's hand-rolled block spliced on the
literal `"END:VTODO\r\n"`, mirroring the event splice (`event.rs:249`), and MUST NOT use the
crate's alarm builder. Evidence (re-run pinned `=0.17.10` after the original probe was found to
have resolved 0.17.13 — ADJUDICATION C-3): the crate path regenerates a fresh `Uuid::new_v4()`
and `Utc::now()` on **every** serialize, so two generations compare unequal and every sync pushes
forever; the splice path is byte-stable across three generations.

**R25.** `DTSTAMP` MUST NOT be a field on the task type: dropped at parse, re-stamped by
serialization, exactly as for events. Not-a-field beats ignored-field — a field would still ride
into base files and the RPC codec where a byte comparison could resurface it.

**R26.** Serializing a task MUST NOT emit `DTEND` or `DURATION`; a test MUST assert `DTEND`'s
absence (T-11).

**R27.** Serialization MUST emit exactly one supported component per `VCALENDAR` document.

**R28.** Output MUST use CRLF throughout (the crate's `write_crlf!` guarantees it).

**R29.** Whether caldir emits `VTIMEZONE` for a zoned `DUE`/`DTSTART` is **open** (Q-3).
Whichever is chosen, the system MUST tolerate Radicale adding one, and no gate may compare raw
bytes across the wire (R30).

**R30 (INVIOLABLE for acceptance gates).** No gate may compare **raw bytes** of what caldir PUT
against what the server GETs back. Radicale does **not** echo byte-for-byte (probe Q-C):
properties re-sorted alphabetically within each component, lines re-folded at 75 octets,
`VTIMEZONE` synthesized for any `TZID`. It **is** idempotent — the second round trip is
byte-identical. Gates MUST compare **parsed domain `==`**, or GET-pass-2 against GET-pass-1.

**R31.** ETag MUST NOT be used to prove "caldir's push was a no-op": it is invariant to property
reordering but changes with `DTSTAMP` (probe Q-C), and caldir re-stamps `DTSTAMP` on every
serialize (R25), so **every caldir push mutates the ETag even when nothing changed**. It MAY
prove "the remote changed".

## 9. File model

**R32.** A local `.ics` file MUST contain **exactly one supported component** (`VEVENT` or
`VTODO`) — not "one component of any kind". The writer is unchanged and keeps writing exactly one
serialized item; R32 is what makes that safe.

| File contents | Today | Required |
|---|---|---|
| 1 VEVENT | loads | loads, byte-identical (R2) |
| 1 VTODO | error `found: 0` | **loads as a task** |
| 0 components | error `found: 0` (indistinguishable) | distinct "no supported component (found: nothing)" |
| 2 VEVENTs | error `found: 2` | error, "expected exactly one, found 2" |
| 2 VTODOs | error `found: 0` | error, "expected exactly one, found 2" |
| 1 VEVENT + 1 VTODO | **loads, VTODO silently dropped; a rewrite deletes it** | **error, found 2** |
| 1 VTODO + VJOURNAL/VFREEBUSY/X-FOO | error `found: 0` | error naming the unsupported component (R19) |
| 1 VEVENT + VTIMEZONE | loads, VTIMEZONE dropped | unchanged (R18) |

Independently required by the server: Radicale answers **`400 Bad Request` "Multiple component
types in object"** to a VEVENT+VTODO resource (probe Q-E.4).

**R33.** Event filenames MUST be byte-identical to today for every input that is an event today
(§15 C1, INVIOLABLE).

**R34.** Task filenames MUST extend the existing reserved-prefix convention (`_recurring__`,
`slugify.rs:15`):

| Task shape | Filename |
|---|---|
| recurring (has RRULE) | `_task__recurring__{summary_slug}.ics` |
| dated | `_task__{time_slug}__{summary_slug}.ics`, date source `DUE`, falling back to `DTSTART` |
| undated | `_task__{summary_slug}.ics` |

`time_slug()` is reused verbatim (`%Y-%m-%d` for a date, local-tz `%Y-%m-%dT%H%M` for a
datetime). `__` is the only separator the summary slugifier can never emit, so
`_task__recurring__x` is unambiguous against a task titled "recurring x".

**R35.** Because `time_slug` is local-timezone-dependent, task filename tests MUST run under the
CI TZ matrix (R106).

## 10. Sync semantics

### Identity

**R36 (INVIOLABLE).** Item identity MUST carry the component kind: `(kind, uid,
Option<recurrence_id>)`. A component-agnostic key is not acceptable (ADJUDICATION C-1, with the
grill's independent re-trace). Five hazards, all real and all closed by one enum field:

1. **State-file collision** — the base map is keyed by identity and inserts plainly
   (`sync_bases.rs:12`, `:43`), so task and event overwrite each other's base; `known_event_ids`
   collapses the pair to one line (`:60`).
2. **Diff mis-keying** — the remote map is built by `.collect()` (`calendar_diff.rs:25-28`), so
   two same-UID remote items become one; pass 1 then compares a local task against a remote event
   (`:36` → `:38`) and emits an `Update` at `:63` that **overwrites one component with the
   other** — worse than a delete.
3. **Wrong-component deletion** — pass 1 runs once per local file and `sync_bases.get(&id)`
   (`:86`) is true both times, so **two** deletes fire though one identity was tracked.
4. **Base-file overwrite** — an *input* collision: both kinds hash the same id string
   (`event_bases.rs:29`) into `bases/<same>.ics` and ping-pong it every sync, feeding
   `update_direction` a base belonging to the wrong component.
5. **`doctor` false positive** — duplicate detection buckets by identity
   (`doctor/warning/duplicate_file.rs:14-20`) and tells the user to delete one of two legitimate
   files.

Severity re-derived from the **local** trigger alone (the cross-collection premise is false for
Radicale, R65): user-triggered, silent, destructive.

**R37.** The identity **string form** MUST be byte-identical to today for events (`{uid}` /
`{uid}__{rid}`) and carry a kind sigil for tasks (`VTODO:{uid}` / `VTODO:{uid}__{rid}`). Parsing
stays infallible: strip a leading `VTODO:` if present, else treat the whole string as an event
id, then apply the existing rid split. Accepted residual: a VEVENT whose UID literally begins
with `VTODO:` round-trips as a task — the same ambiguity class already accepted for `__`
(`instance_id.rs:38-40`); no encoding removes it, because UID is `TEXT`.

### Equality and direction

**R38.** Task equality MUST mirror event equality exactly: `last_modified`, `sequence` and
`created` ignored; `x_properties` compared order-independently by name+value with parameters
ignored; attachments by URI only. The two comparison helpers MUST be moved to a shared location
and referenced by **both** types, making drift on those fields structurally impossible.

**R39.** `status`, `completed`, `percent_complete`, `due`, `start`, `priority`, `summary`,
`description` and all other content fields MUST participate in `==`. Ignoring any of the first
three would mean "task completed" never syncs — the worst possible silent failure for VTODO.

**R40.** Direction MUST use the existing machinery unchanged: three-way against the stored base
first (`calendar_diff.rs:153`, `:161-167`), mtime / `LAST-MODIFIED` only as a fallback when both
sides changed or no base exists. (`CLAUDE.md`'s mtime description is **stale**; `specs/caldir.md`
is correct. Correcting it is a separate commit.)

**R41.** The Push-biased fallback for an item with no `LAST-MODIFIED` and no `SEQUENCE`
(`(Some, None) => true`, `calendar_diff.rs:187`) MUST be kept for tasks. Wrong-Pull destroys the
user's local edit with no recovery; wrong-Push is recorded as a base on the same run and
converges after one cycle. Existing VEVENT behaviour, not new.

**R42.** Both skip-rules keying on `Cancelled` (`calendar_diff.rs:45` "both cancelled" and
`:106` "missing locally + remote cancelled") MUST key on `CANCELLED` **only**, never `COMPLETED`.
If `COMPLETED` shared the `:106` rule a remote completed task would **never be created locally** —
and the MVP's own fixture is `STATUS:COMPLETED`, so the demo case would silently fail to pull.

**R43.** Task sync bases MUST use the existing layout unchanged: a single-component `VCALENDAR`
at `bases/<sha256(identity_string)>.ics`. Hard dependency on R32/R19 — until the arity rule can
return a task, a task base loads as zero components and is silently skipped, degrading every task
to a legacy known-id entry and permanently disabling the three-way compare.

### The window rule and the RFC 4791 §9.9 finding

> **The most important finding in the effort, confirmed against a real server.** RFC 4791 §9.9
> defines time-range matching for VTODO over `DTSTART`/`DURATION`/`DUE`/`COMPLETED`/`CREATED`. A
> VTODO whose **only** date property is a `COMPLETED` older than the window start evaluates FALSE
> and a compliant server **MUST NOT return it**. Core, seeing no `DTSTART` and no `DUE`, calls
> that same task in-window. The predicates disagree **by specification**, no server bug involved.
>
> **Verified against Radicale 3.7.6** (probe Q-B, 8 fixtures, all §9.9 rows matched): fixture
> `t2` (`STATUS:COMPLETED` + `COMPLETED:20200101T000000Z`, no DTSTART, no DUE) is **OMITTED** by a
> `calendar-query` carrying `<C:time-range start="20250825T000000Z" end="20270825T000000Z"/>` and
> **returned** by the same query without it. Fixture `t1` (fully undated) is returned by both.
>
> The consequence, traced in real code: the naive fix — swapping `<C:comp-filter name="VEVENT">`
> for `"VTODO"` at `caldir-provider-caldav/src/caldav/ops/list_events.rs:103` while keeping the
> `<C:time-range>` at `:104` — means the server omits a previously-synced completed task, the
> remote map has no entry (`calendar_diff.rs:25`), the task is undated and therefore in-window so
> `:80` does not skip, `sync_bases.get(&id).is_some()` is true at `:84`, and `:86` emits an
> **incoming Delete that removes the user's file**. `allow_mass_delete` (`guards.rs:10`) counts
> only *outgoing* deletes, so it never fires. Silent, and it targets the user's archive.
>
> **D1 (the window rule, R44) and D2 (no time-range on the VTODO query, R51/R58) are ONE decision
> and MUST ship in ONE commit.** D2 alone is merely inefficient. **D1 alone CREATES the bug**: D1
> is what makes an undated task in-window and therefore deletable, while a time-ranged query is
> what makes it absent. Neither half is safe without the other.

**R44 (INVIOLABLE, ships with R51/R58).** The in-window predicate for tasks MUST be the "core's
rule" column of §9.9, with `start = from`, `end = to`:

| DTSTART | DUE | core's rule |
|---|---|---|
| Y | Y | `(from < DUE OR from <= DTSTART) AND (to > DTSTART OR to >= DUE)` |
| Y | N | `from <= DTSTART AND to > DTSTART` |
| N | Y | `from < DUE AND to >= DUE` |
| N | N | **TRUE, unconditionally** |

`COMPLETED` and `CREATED` MUST NOT be consulted by core: core must be *more* permissive than any
server, never less.

**R45.** The event predicate `occurs_in_range` (`event.rs:136-141`) MUST be unchanged, byte for
byte (R2).

**R46 (INVIOLABLE).** No synthesized dates. An undated task MUST NOT be given a fabricated
`DTSTART` (epoch, `COMPLETED`, "now") to reuse the event predicate: an epoch start evaluates false
at `calendar_diff.rs:80` and would silently make undated tasks **unpushable** at `:89`.

**R47.** Polarity, because it is easy to state backwards: `in_range == false` → `continue` → **no
delete and no create**; `in_range == true` *authorizes* deletion. "Always in-window" buys
pushability at the price of deletability; neither half may be assumed independently.

### Delete inference

**R48 (INVIOLABLE).** If any `.ics` file in a calendar failed to load, that sync MUST emit **zero**
deletes for that calendar, in either direction. Without it, a gracefully-degrading core paired with
a VTODO-aware provider reaches `calendar_diff.rs:110` with the task present remotely, absent
locally (file skipped) and its base known — and deletes the user's task server-side.

**R49.** Version-skew belt: if a remote response contains **zero tasks** while the local calendar
holds at least one task with a known base, the run MUST skip **all** task incoming deletes and
warn. A pre-VTODO caldav provider still filters on VEVENT and returns no tasks; so does webcal.
Inference from data — **no** RPC change needed.

**R50.** R49's accepted failure mode: deleting *every* task remotely leaves the local files until
one task exists again — the intended safe direction. The warning *text* is deferred; the *skip* is
not.

R58 removes *partial* server-side filtering; R49 covers *total* absence. They cover each other's
gap.

## 11. Provider protocol changes

**R51 (INVIOLABLE, ships with R44).** The CalDAV VTODO `calendar-query` MUST carry **no**
`<C:time-range>` (§10, §12).

**R52 (INVIOLABLE).** `Method` MUST gain no variant — no `list_todos`, no `create_todo`. It is a
closed serde enum with no `#[serde(other)]` (`rpc/methods.rs:22-31`), so a new verb hard-breaks
every installed provider binary mid-upgrade. Component kind travels **inside the opaque ICS
payload**.

**R53.** The RPC method structs carry a **typed** item, not opaque text
(`rpc/methods/delete_event.rs:6-10` is `pub event: Event` with `#[serde(with = "crate::rpc::ics")]`;
`ListEvents`'s response is `Vec<Event>`). Those typed fields MUST become item-typed or a task can
be neither created, updated, nor deleted. The JSON on the wire stays a string, so R52/R55 are
unaffected.

**R54.** The wire decoder MUST apply the same arity and unsupported-component rules as the disk
loader (R19, R32), **including the `VTIMEZONE` exemption** (R18).

**R55 (INVIOLABLE).** RPC fields MUST be additive and `Option`-typed; `deny_unknown_fields` MUST
NOT appear anywhere.

**R56.** New core + old provider MUST be safe. Stated precisely (and *not* as ADR-09 originally
had it): pushing a task to a provider that cannot represent one produces a decode failure whose
`?` at `caldir-core/src/connection.rs:231` **aborts the rest of that calendar's push**. Ugly and
recurring; **not data loss** — R48 and R49 cover the destructive variants.

**R57 (INVIOLABLE).** Old core + new provider MUST be safe *because* R19 holds: a VTODO payload
fails the old `Vec<Event>` deserialization, one bad element fails the whole vector, the call
errors, and the sync aborts **before any apply**. The catastrophic variant is a silent drop, which
makes the remote list arrive short and feeds the delete path. **Both directions hinge on one
principle: unknown must be loud or quarantined, never forgotten.**

## 12. Generic CalDAV behavior (the reference implementation)

The only provider in MVP scope. All statements are verified against **Radicale 3.7.6**
(`tomsquest/docker-radicale`, digest `sha256:e2ef8624b215…`, probe date 2026-08-25) unless marked
otherwise.

**R58 (INVIOLABLE, ships with R44).** The VTODO `calendar-query` REPORT MUST be emitted as a
**bare comp-filter** — `<C:comp-filter name="VTODO"/>` — with **no `<C:time-range>` child**.
Restating the citation and the evidence, because this is the requirement most likely to be
"optimized" back into a bug:

- **RFC 4791 §9.9** defines the VTODO time-range table. The row `DTSTART=N, DUE=N, COMPLETED=Y,
  CREATED=N` matches only when `start <= COMPLETED AND end >= COMPLETED`.
- **Verified against Radicale 3.7.6:** all six rows implemented faithfully over 8 fixtures. An
  undated task carrying only `COMPLETED:20200101T000000Z` **is omitted** by a time-range query and
  returned without it. `DTSTAMP` correctly does not participate. **A VTODO `calendar-query`
  carrying `<C:time-range>` therefore causes silent local deletion of previously-synced undated
  completed tasks** (mechanism traced in §10).
- A task also carrying `CREATED` is rescued by the `end > CREATED` row (fixture `t5`) — which is
  why `CREATED` is modelled (R13, C-4). **Defence in depth, never a substitute for R58.**
- **R44 and R58 MUST ship in one commit.** Shipping R58 alone is merely inefficient; shipping R44
  alone *creates* the deletion bug.

**R59.** The VTODO query MUST be a **separate REPORT** from the VEVENT+time-range query, not a
sibling comp-filter: RFC 4791 §9.7.1 says a comp-filter matches when its children "**all**
match", i.e. siblings read as AND, which would match nothing. Most servers treat siblings as OR;
**UNVERIFIED: whether Radicale does.** Two REPORTs sidestep the ambiguity and guarantee zero
VEVENT regression. *Settled by:* one probe comparing a two-sibling filter's result set against
the union.

**R60.** The UID-lookup REPORT (`caldav/ops.rs:107`) MUST be component-parameterised. It has no
time-range and needs only the component-name treatment.

**R61.** `find_event_location`'s **fast path** MUST also be component-aware:
`update_event.rs:29` and `delete_event.rs:24` try `{uid}.ics` first and fall back to the
hard-coded-VEVENT REPORT, so without this a task update or delete targets an event resource or
fails to find its own.

**R62.** The href for a task MUST remain `{uid}.ics`, unchanged for both kinds. A kind-distinct
href is **unimplementable** against the MVP's own target server: Radicale returns **`409` +
`<C:no-uid-conflict/>`** for the same UID at a different href (probe Q-E.3). ADR-02 rule #4 is
struck by ADJUDICATION C-1.

**R63.** caldir MUST handle `409 <C:no-uid-conflict/>` on push. **Correction (ADJUDICATION):** the
trigger is **not** a local file rename. The href is UID-derived (`caldav/client.rs` `event_url`)
and used unconditionally on create, so renaming a local `.ics` changes the slug but not the UID
and therefore PUTs to the *same* href — a `412` under `If-None-Match: *`, or a silent `204`. The
reachable trigger is a **server-assigned filename**: the UID already lives at a href caldir did
not choose, so a create against `{uid}.ics` collides. That is exactly what `find_event_location`'s
REPORT fallback exists for. Required behaviour is unchanged: on `409`, fall back to
`find_event_location` + a conditional PUT, i.e. treat it as the update it actually is. Tests MUST
assert the **outcome** (one resource, updated), never the status code.

**R64 (INVIOLABLE).** caldir MUST **never PUT a multi-component `VCALENDAR`**. Radicale answers
`400 Bad Request` — server log: `Multiple component types in object: 'VEVENT', 'VTODO'` (probe
Q-E.4).

**R65.** A same-UID event/task pair **cannot coexist in one CalDAV collection**: Radicale enforces
UID uniqueness per collection, component-type-agnostically — `409` across hrefs, `400` within one
resource, silent `204` overwrite at the same href; same UID in a *different* collection is fine
(`201`). caldir MUST surface a **push error** for a local same-UID pair rather than invent an href
scheme. The local mixed directory (R4) stays supported; what is unsupported is pushing both to one
collection.

**R66.** The pre-existing `find_map(Result::ok)` at `caldav/ops.rs:32-37`, which silently keeps
the first parseable component, MUST be fixed to R19's loud behaviour. Own ticket — a pre-existing
bug, not a VTODO bug.

**R67.** Radicale injects **zero** non-`X-` properties onto a VTODO (probe Q-D): no `SEQUENCE`, no
`CREATED`, no `LAST-MODIFIED` rewrite, no reformatted timestamps. The only addition is the sibling
`VTIMEZONE` *component* (R18, R29).

**R68.** Radicale imposes no semantic coupling: `STATUS:COMPLETED` without `PERCENT-COMPLETE` is
accepted `201` and stored verbatim, as is the contradictory `STATUS:COMPLETED` +
`PERCENT-COMPLETE:0` (probe Q-F1/F1b). caldir MUST NOT synthesize either to "repair" the other.

**R69.** **UNVERIFIED: every statement in this section for iCloud (Apple Reminders).** iCloud
rides the same ops with no mapping layer, so it is structurally in scope — but whether it 409s a
same-UID pair (load-bearing for R62/R65), implements §9.9 (R58), or injects non-`X-` properties
(R67) is unmeasured, as is whether Reminders collections are returned by discovery at all.
*Settled by:* running the same six probes against a real iCloud account. Nothing else will do it.

## 13. Unsupported provider behavior

**R70.** Google and Outlook are structurally VEVENT-bound (only `/events` collections;
mapping hard-fails without start/end; reminders are "minutes before start") and are out of scope
(§16). No provider-crate edits ship in this feature.

**R71.** Accepted consequence of the ADR-07 cut, ruled by the user: pushing a task to a
Google/Outlook-backed calendar produces `expected 1 event(s) in ICS, found 0` and **aborts the
rest of that calendar's push** at `connection.rs:231`, every sync. Ugly and recurring; **not data
loss** — R44+R58 cover the window variant, R48 the unreadable-file variant.

**R72.** Webcal's existing silent drop for a VTODO-only feed
(`caldir-provider-webcal/src/commands/list_events.rs:16-25`; `connect.rs:32-36` only checks for
`BEGIN:VCALENDAR`) is covered by R49: a webcal calendar returning zero tasks while local holds
tasks with known bases MUST NOT delete them. Making webcal task-aware is out of scope.

**R73.** No capability negotiation ships: no config field, no builder method, no provider-crate
edit, no third diff bucket, no warning renderer (§16).

## 14. Error model

Two distinct failure modes exist today; they fail in **opposite directions**, so a fix for one can
create the other.

**Mode A — local hard error.** A VTODO file makes `CalendarEvent::load` return
`ExpectedSingleEvent { path, found: 0 }`; the `?` at `calendar.rs:114` escalates it to the whole
calendar and, via `agenda.rs:35`, to the whole agenda. **Loud, and over-broad.**

**Mode B — remote silent drop.** A webcal VTODO-only feed yields zero items with no error, and the
CalDAV `find_map(Result::ok)` keeps only the first parseable component. **Quiet, and
data-destroying** — absence is what the delete path acts on.

**R74.** Both MUST be fixed, and the fix for A MUST NOT introduce B. A is fixed by per-file
graceful degradation plus R48; B by R19 (loud on unknown) and R49 (never infer a delete from a
task-less response).

**R75.** The error vocabulary MUST distinguish "no supported component" from "too many". Today a
VTODO-only file and a genuinely empty `VCALENDAR` report the identical `found: 0`, so the message
cannot tell "unsupported" from "empty" — and `doctor.rs:75-91` already depends on the empty case.
Required: **too many** — "expected exactly one VEVENT or VTODO in `{path}`, found `{n}`; split it
into one file per item"; **none** — "no VEVENT or VTODO in `{path}` (found: `{names}`)", where
`{names}` lists the components present or the literal `nothing`.

**R76.** The wire/sync-base error type needs the arity variant and SHOULD NOT duplicate the "none"
variant: bases are always written by caldir, and the wire case is covered by R19.

**R77.** A syntactically broken **VEVENT** MUST still surface as `DoctorWarning::UnreadableEvents`
(`doctor.rs:55-57`). Graceful degradation MUST NOT start silently swallowing malformed files
(§15 C13).

**R78.** `Calendar::events()` MUST NOT return a silently-short `Ok(vec)`: either it errors, or it
returns the loaded items **plus a signal that the listing is incomplete** — the signal R48
consumes.

**R79.** `409 <C:no-uid-conflict/>` on push MUST be handled per R63, not surfaced as an opaque
HTTP failure.

**R80 (INVIOLABLE).** caldir MUST never PUT a multi-component `VCALENDAR` (R64). An error-model
rule as much as a serialization rule: the server answers `400`, and a retry loop would spin
forever.

**R81.** RSVP on a task is meaningless. Single-file mode MUST fail naming the cause (`"<path> is a
task, not an event; tasks cannot be RSVP'd"`), exit non-zero, and **write no file**. List mode MUST
narrow to events so tasks never enter the invite list; same for `invites`.

## 15. Compatibility requirements

**INVIOLABLE** rows are rejected if breached, regardless of benefit. **SOFT** rows may be broken
only with the named mitigation. **R82.** Every row is binding at the tier stated.

| # | Surface | Constraint | Tier |
|---|---|---|---|
| **C1** | Existing VEVENT `.ics` files | No VEVENT file renamed, rewritten, or re-serialized as a side effect. `base_slug()` byte-identical for every input that is an event today. | **INVIOLABLE** |
| **C2** | Existing VEVENT parse | An existing VEVENT file parses to a structurally `==` event before and after; event equality unchanged. | **INVIOLABLE** |
| **C3** | `.caldir/config.toml` | New keys `Option`, defaulted, omitted when `None`. No key renamed, retyped, or made required. Never `deny_unknown_fields`. | **INVIOLABLE** |
| **C4 (AMENDED)** | `state/known_event_ids` | Line format stays one id per line, sorted, plaintext; the file keeps being written; task ids go in the **same** file. **AMENDED per ADJUDICATION B-4: a kind sigil IS permitted** — the original "with no new sigil" wording is void, because without a per-kind string `known_event_ids` collapses the pair (R36 hazard 1) and there is no alternative fix. The constraint is now: (a) existing unprefixed lines still parse as kind `Event`, byte-identical; (b) an older caldir treats a `VTODO:`-prefixed line as an inert unknown id that cannot trigger a delete — which holds, because a delete needs the id present on exactly one side (`calendar_diff.rs:86`/`:110`) and no VEVENT produces that id. | **INVIOLABLE for the event form** |
| **C5** | `state/bases/<sha256>.ics` (downgrade) | An old caldir MUST skip, not fail on, a base it cannot parse (the `else { continue }` at `event_bases.rs:60-62`). The hash-filename scheme MUST NOT change; bases stay single-component VCALENDAR documents. | **INVIOLABLE** |
| **C6** | Downgrade with VTODO files present | An old caldir hitting a VTODO file MUST fail loudly and without writing, never proceed with a partial local set. | **INVIOLABLE** |
| **C7** | Skipped file → delete inference | R48: a calendar with any unreadable file emits zero deletes that sync. | **INVIOLABLE** |
| **C8** | RPC verbs | R52: `Method` gains no variant. | **INVIOLABLE** |
| **C9** | RPC fields | R55: additive `Option` only; no `deny_unknown_fields` anywhere. | **INVIOLABLE** |
| **C10** | Wire decode of an unknown component | R19: hard error, never a silent drop or empty vec. | **INVIOLABLE** |
| **C11** | `caldir-core` Rust API | Semver-breaking changes allowed (pre-1.0) but require a minor bump, matching bumps of all six dependants, and their `version = "…"` pins updated. | SOFT |
| **C12 (AMENDED)** | `--json` agenda schema | The original wording — "`start: String` stays non-optional, so an item with no DTSTART must not enter the agenda" — **cannot hold**: the MVP's own fixture is DUE-only and undated tasks are first-class. **AMENDED per ADJUDICATION B-5:** DUE anchors a task in the agenda as DTSTART anchors an event; undated tasks get their own section. Schema rule: no key removed, renamed, or retyped; `start` and `availability` widen to `T \| null` and `null` MAY appear **only** on `kind == "todo"` entries; five additive keys (`kind`, `due`, `completed`, `percent_complete`, task status) on every entry so the object shape stays uniform; `status` is **NOT** retyped and keeps its existing value set. Every `kind == "event"` entry stays byte-identical; consumer migration is one predicate — selecting `kind == "event"` reproduces the old stream exactly. | **INVIOLABLE as amended** |
| **C13** | CLI text output & exit codes | Existing text for existing situations unchanged; an event-only calendar renders byte-identically; `doctor` still reports a broken VEVENT as `UnreadableEvents`. | **INVIOLABLE** |

**R83.** The `--json` widening MUST be documented alongside the existing schema
(`website/src/content/docs/commands.md`) with a second example object for a task entry.

**R84.** Tasks appear in the agenda anchored on `DUE` (falling back to `DTSTART`), interleaved with
events in the existing day groups; for an event and a task at the same instant, event first.
Undated tasks are never placed in a day group and never given an invented date. This is the user's
ruling (ADJUDICATION B-5) and is what forces the C12 amendment.

**R85.** No existing sort behaviour may be made *more* deterministic here. A final identity
tiebreak would improve event-vs-event ordering but would change existing text for existing
situations, breaching C13. Own ticket.

## 16. Non-goals

**R86.** **Google Tasks / Outlook (Microsoft To Do) task support.** A different API surface
entirely — only `/events` collections exist in both providers today
(`caldir-provider-google/src/google_event/from_google.rs:17-22`,
`caldir-provider-outlook/src/graph_api/types/mod.rs:42-108`) — requiring **new OAuth scopes**:
Outlook's consent is `Calendars.ReadWrite User.Read offline_access` (`connect.rs:25`), Google's is
`calendar.calendarlist.readonly` + `calendar.events` (`connect.rs:23-26`); neither covers a tasks
API, which means **re-consent for every existing user**. `UNVERIFIED:` the exact vendor scope
names; *settled by* vendor docs, not code.

**R87.** **Task hierarchies / `RELATED-TO` / subtasks.** The parser collapses them: `MULTIS` lists
the ABNF placeholder `"RELATED"`, not `"RELATED-TO"`, so two lines in yield one out with `PARENT`
silently gone (verified). Re-encoding as `X-CALDIR-RELATED-TO` inherits the identical defect
(repeated same-name `X-*` collapse the same way). Radicale holds both values with `RELTYPE` params
intact (probe Q-F2), so the loss is **100% caldir's parser**. A single `RELATED-TO` survives
unmodelled at zero cost; the loss MUST be pinned by a test (T-16).

**R88.** **`CATEGORIES`.** Modelling it through the crate **corrupts** it: `WORK,URGENT` becomes
`WORK\,URGENT` (verified, pinned `=0.17.10`), turning two categories into one for every third
party — flipping it IN via the typed path is *worse* than dropping it; a correct implementation
needs a hand-rolled emitter. Plus `specs/caldir.md` skips it for events, and a todo-only exception
inside "Fields We Skip" is incoherent. Follow-up ADR, for both kinds together.

**R89.** **Recurring-task expansion semantics.** `RRULE`/`EXDATE`/`RDATE` are **modelled and
preserved** (R13) but never expanded: the crate's `recurrence` feature is off and expansion needs a
`DTSTART` a task may not have. Master only. Working rule, revisited only on a user report: a
recurring undated task is in-window (R44 row 4 already says TRUE).

**R90.** **`caldir new --due`.** No ADR owns it; tasks arrive by pull or hand-authored `.ics`.
`caldir new` continues to create events only.

**R91.** **The capability mechanism (ADR-07), in whole — CUT by user decision.** No config field,
no builder, no provider-crate edits, no third diff bucket, no renderer or pluralization, no
`is_empty()` fix. It bought a nicer Google/Outlook error while every other decision places those
providers outside the MVP — **and** its own default ("absent = does not support VTODO") meant every
existing caldav and iCloud user would get zero task support after upgrading until re-running
`connect`, silently. Worse than the message it prevented.

**R92 (record for follow-up, do not implement).** `supported-calendar-component-set` **is**
advertised by Radicale on every calendar collection at Depth 0 and Depth 1 (default
`VTODO,VEVENT,VJOURNAL`; a VTODO-only collection returns `VTODO`), absent (404) on the principal,
and **bulk-readable via ONE Depth-1 PROPFIND on the home-set** — so per-calendar round trips are
unnecessary. But it is **ADVISORY ONLY**: a VEVENT PUT into a VTODO-only collection returned `201`
(probe Q-A). **It can therefore never be an enforcement point**, and any future work MUST treat
the declared value as a hint that can simply be wrong.

**R93.** **VJOURNAL.** Out of scope, but the design MUST NOT preclude it: adding it later should be
one enum variant, one struct, one match arm — no second redesign.

**R94.** **Renaming the `Event*`-prefixed shared types.** Cosmetic; ~20 call sites of churn for
zero behavioural benefit. Deferred unless a maintainer asks.

## 17. Acceptance scenarios

**A-1 — the mandatory scenario. R95 (INVIOLABLE).** Given a u_crawler VTODO on disk (the Stage 2
fixture: `UID`, `DTSTAMP`, `DUE`, `SUMMARY`, `DESCRIPTION`, `URL`, `PRIORITY`, `STATUS:COMPLETED`,
`COMPLETED`), **when** caldir loads it and pushes it to a generic CalDAV server, **then**:

1. caldir loads it with no error and no effect on any sibling file, calendar, or aggregation (R5);
2. the server receives a **VTODO** — single-component, at `{uid}.ics` — preserving `UID`, `DUE`,
   `SUMMARY`, `DESCRIPTION`, `URL`, `PRIORITY`, `STATUS`, `COMPLETED` (R13, R62, R64);
3. **no VEVENT is created anywhere in the chain** (R1);
4. pulling again returns the task (R58) and caldir writes a **semantically equivalent VTODO** —
   parsed-domain `==`, never a byte comparison (R30);
5. a third sync produces an empty diff (R96).

Asserted against **Radicale 3.7.6**; equality is structural.

**A-2 — idempotence. R96.** Syncing twice against the same remote MUST produce an **empty diff**
on the second run. This is the load-bearing gate: it catches every churn mechanism at once,
including ones not enumerated. The comparison MUST be parsed-domain `==` or GET-pass-2 vs
GET-pass-1 (R30).

**A-3 — the undated completed task is not deleted. R97 (INVIOLABLE).** Given a previously-synced
undated task whose only date property is a `COMPLETED` older than the window start, a pull MUST
leave the file in place and produce no incoming delete for it. Prevented by R44 and R58 **together**.

**A-4 — undated tasks are pushable. R98.** An undated local task with no base MUST produce exactly
one outgoing create against a bounded window — never a skip (R46, R47).

**A-5 — real deletes still propagate. R99.** A task present in a non-empty remote task list on a
previous sync, absent from the current one, with a known base, MUST produce an incoming delete.
Absence must stay meaningful; R49 suppresses only the *all-tasks-absent* case.

**A-6 — mixed local directory. R100.** A directory holding a VEVENT file and a VTODO file sharing
a UID MUST produce two identities, two base entries, two `known_event_ids` lines, two base files,
**no** `doctor` duplicate warning, and — both present locally, neither remote, shared id known —
exactly **one** delete against the correct file (R36).

**A-7 — mixed-component file is a loud error. R101.** A file holding one VEVENT and one VTODO MUST
error naming the count and MUST NOT load with the VTODO silently dropped (R32).

**A-8 — zoned payload decodes. R102.** A CalDAV response body containing a `VTIMEZONE` sibling plus
one VTODO MUST decode successfully (R18, R54).

**A-9 — rename does not destroy. R103.** Renaming an item's local file, then pushing, MUST update
the same server resource and never create a second one (R11, R63). **Correction
(ADJUDICATION):** a rename cannot itself produce a `409` — the href is UID-derived, so the rename
yields the same href. What A-9 pins is the outcome: one resource, updated in place. It is measured
on the VEVENT control (`live::radicale::renaming_a_local_file_updates_the_same_resource_and_never_a_second_one`),
because a *task* rename can never reach a server at all under R70 — a task push aborts in core. The
property transfers to tasks **by construction** (`event_url` takes the UID and nothing else), not
by measurement.

**A-10 — no VEVENT regression. R104 (INVIOLABLE).** The 686-test baseline MUST stay green in all
three CI timezones with **zero edits to existing event tests**. Any required edit to an existing
event test is a design failure, not a fixup.

## 18. Test matrix

Layers: **U** = unit (pure/filesystem, in-crate); **C** = the `Connection` no-network seam
(`caldir-core/src/connection.rs` — everything real except the mocked transport); **L** = live
Radicale 3.7.6. Placement follows repo convention: inline `#[cfg(test)] mod tests`, no `tests/`
directory, no fixture files — ICS is an inline `\r\n` string literal.

**R105.** Every test below MUST exist and be named as given; the names are the citation handle for
Stage 9 review.

**R106 (TZ matrix).** Every **TZ**-marked test MUST pass under `TZ=UTC`, `TZ=Europe/Stockholm`, and
`TZ=America/New_York` — the three zones CI already runs (`.github/workflows/ci.yml`,
`fail-fast: false`). **All `DUE` / `DTSTART` / `COMPLETED` handling and all task filename tests are
in this set**, because `time_slug` is local-tz and a floating `COMPLETED` is zone-sensitive.

| # | Requirement(s) | Test name | Layer | TZ |
|---|---|---|---|---|
| T-1 | R5, R17 | `event::tests::from_ics_str_parses_a_vtodo_document` | U | |
| T-2 | R5, R32 | `calendar::event::tests::load_reads_a_vtodo_file` | U | |
| T-3 | R5 (level 2) | `calendar::tests::events_lists_a_vtodo_file_alongside_its_vevent_sibling` | U | |
| T-4 | R5 (level 3) | `caldir::tests::aggregating_all_calendars_survives_a_vtodo_calendar` | U | |
| T-5 | R2, R104 | `event::tests::vevent_control_round_trips_identically_after_vtodo_support` | U | TZ |
| T-6 | R33, C1 | `event::slugify::tests::base_slug_is_unchanged_for_vevent_corpus` (~12 pairs: all-day, recurring master, empty summary, emoji, >64-char summary) | U | TZ |
| T-7 | R34, R35 | `event::slugify::tests::task_filenames_cover_recurring_dated_and_undated` | U | **TZ** |
| T-8 | R32, R75 | `calendar::event::tests::arity_table_all_eight_rows` | U | |
| T-9 | R19 | `event::tests::an_unsupported_component_errors_and_names_it` | U | |
| T-10 | R18, R54, R102 | `rpc::ics::tests::a_vtimezone_sibling_does_not_break_vtodo_decode` | U | |
| T-11 | R8, R26 | `todo::to_icalendar::tests::serializing_a_task_never_emits_dtend` | U | |
| T-12 | R24 | `todo::tests::valarm_is_byte_stable_across_three_generations` | U | |
| T-13 | R21, R22 | `todo::from_icalendar::tests::zoned_due_and_floating_completed_round_trip` | U | **TZ** |
| T-14 | R14 | `todo::tests::an_unknown_status_value_is_preserved_or_explicitly_dropped` | U | |
| T-15 | R13 (PRIORITY) | `todo::tests::priority_out_of_rfc_range_round_trips_verbatim` | U | |
| T-16 | R87 | `todo::tests::two_related_to_lines_collapse_to_one` (pins the known loss) | U | |
| T-17 | R15, R23 | `todo::tests::dropped_property_names_are_exactly_the_documented_list` | U | |
| T-18 | R38, R39 | `todo::tests::equality_mirrors_event_equality` (same last_modified/different sequence ⇒ equal; reordered x-props ⇒ equal; reordered attachments ⇒ equal; status/completed/percent change ⇒ unequal) | U | |
| T-19 | R25 | `todo::tests::two_serializations_with_different_wall_clocks_compare_equal` | U | |
| T-20 | R36, R37 | `event::instance_id::tests::parse_of_format_is_identity_for_both_kinds` (all four rid sub-forms, a UID containing `__`, the pathological `UID:VTODO:x` event) | U | |
| T-21 | C4 amended | `calendar::state::known_event_ids::tests::event_form_lines_are_byte_identical_and_task_lines_are_prefixed` | U | |
| T-22 | R36 hazard 5 | `commands::doctor::warning::duplicate_file::tests::an_event_task_uid_pair_is_not_a_duplicate` | U | |
| T-23 | R43, C5 | `calendar::state::event_bases::tests::a_task_base_loads_and_rehashes_and_an_unparseable_base_is_skipped` | U | |
| T-24 | R44, R45 | `diff::calendar_diff::tests::task_window_rule_covers_all_four_date_combinations_at_both_edges_and_events_are_unchanged` | U | **TZ** |
| T-25 | R97, A-3 | `diff::calendar_diff::tests::an_undated_task_with_a_known_base_and_an_empty_remote_is_not_deleted` | U | |
| T-26 | R98, A-4 | `diff::calendar_diff::tests::an_undated_task_with_no_base_produces_one_outgoing_create` | U | |
| T-27 | R99, A-5 | `diff::calendar_diff::tests::a_task_omitted_from_a_non_empty_remote_list_is_still_deleted` | U | |
| T-28 | R42 | `diff::calendar_diff::tests::a_remote_completed_task_absent_locally_is_created_and_both_cancelled_drift_is_ignored` | U | |
| T-29 | R48, C7 | `diff::calendar_diff::tests::unreadable_local_files_suppress_all_deletes` | U | |
| T-30 | R49, R72 | `diff::calendar_diff::tests::a_task_less_remote_response_suppresses_all_task_deletes` | U | |
| T-31 | R36 hazard 3, A-6 | `diff::calendar_diff::tests::a_shared_uid_pair_produces_exactly_one_delete_against_the_right_file` | U | |
| T-32 | R41 | `diff::calendar_diff::tests::a_task_with_no_last_modified_and_no_sequence_pushes` | U | |
| T-33 | R52, C8 | `rpc::tests::method_set_is_exactly_the_six_known_verbs` | U | |
| T-34 | R55, C9 | `rpc::tests::request_structs_ignore_unknown_fields` + `..::deny_unknown_fields_appears_nowhere` | U | |
| T-35 | R19, C10 | `rpc::ics::tests::decoding_an_unknown_component_errors_rather_than_dropping` | U | |
| T-36 | R53 | `rpc::methods::tests::create_update_and_delete_carry_a_task_payload` | U | |
| T-37 | R51, R58 | `caldav::ops::list_events::tests::the_vtodo_report_body_contains_no_time_range` — a **named** assertion whose comment cites RFC 4791 §9.9 and this spec's §10/§12 | U | |
| T-38 | R59 | `caldav::ops::list_events::tests::the_vtodo_report_is_a_separate_request_from_the_vevent_report` | U | |
| T-39 | R60, R61 | `caldav::ops::tests::find_event_location_is_component_aware_on_both_paths` | U | |
| T-40 | R63, R79 | `caldav::ops::create_event::tests::a_409_no_uid_conflict_falls_back_to_update` | U | |
| T-41 | R64, R80 | `caldav::ops::tests::a_multi_component_vcalendar_is_never_put` | U | |
| T-42 | R81 | `commands::rsvp::tests::rsvp_on_a_task_exits_non_zero_and_leaves_the_file_unmodified` | U | |
| T-43 | R77, C13 | `commands::doctor::tests::a_malformed_vevent_is_still_a_warning_after_component_skipping` | U | |
| T-44 | C12, R83 | `output::agenda_json::tests::schema_keys_are_stable_over_an_event_and_a_task_fixture` | U | |
| T-45 | R84, C13 | `output::agenda_text::tests::an_event_only_fixture_renders_byte_identically` | U | TZ |
| T-46 | R84 | `output::agenda::tests::event_task_and_all_day_on_one_day_order_correctly` | U | **TZ** |
| T-47 | R84 | `output::agenda::tests::an_undated_task_is_absent_from_the_body_counted_in_the_footer_and_present_in_json` | U | |
| T-48 | A-1 (1-3), R1 | `connection::tests::a_task_push_aborts_and_never_reaches_the_provider_as_a_vevent` — asserts the push aborts with the byte-exact literal `expected 1 event(s) in ICS, found 0` and that **no** request reached the provider. **Name corrected (ADJUDICATION):** the original `a_task_is_pushed_as_a_vtodo_and_never_as_a_vevent` asserted a falsehood — under R70 nothing is pushed at all, so there is no `CreateEvent` payload to inspect | **C** | |
| T-49 | A-2, R96 | `connection::tests::syncing_twice_against_a_fake_remote_yields_an_empty_second_diff`. **Raw-fixture requirement (ADJUDICATION):** the remote MUST be driven from raw ICS via `MockProvider::reply_raw_data`, never from domain values via `reply`. `reply` encodes the expected value with `to_ics_string` — the encoder under test — so a lossy serialize cancels on both sides; measured, that blinded **6 of 12** mutations the raw-fixture version catches | **C** | **TZ** |
| T-50 | R56 | `connection::tests::pushing_a_task_to_a_task_less_provider_fails_that_calendar_without_data_loss` | **C** | |
| T-51 | R57 | `connection::tests::a_task_payload_from_a_newer_provider_fails_the_whole_list_before_any_apply` | **C** | |
| T-52 | A-6, R100 | `connection::tests::a_mixed_local_directory_keeps_both_items_distinct_end_to_end` | **C** | |
| T-53 | A-1 (all 5), R95 | `live::radicale::a_u_crawler_task_round_trips_through_radicale_preserving_identity_and_semantics` | **L** | |
| T-54 | A-3, R97, R58 | `live::radicale::an_undated_completed_task_survives_a_pull` — the §9.9 regression lock; MUST fail if a `<C:time-range>` is reintroduced on the VTODO query | **L** | |
| T-55 | R30, R96 | `live::radicale::get_pass_two_equals_get_pass_one` (never raw bytes vs what was PUT) | **L** | |
| T-56 | R65 | `live::radicale::pushing_a_same_uid_pair_into_one_collection_surfaces_an_error` | **L** | |
| T-57 | R67 | `live::radicale::the_server_injects_no_non_x_properties_onto_a_task` | **L** | |
| T-58 | A-9, R103 | `live::radicale::renaming_a_local_file_updates_the_same_resource_and_never_a_second_one`. **Name corrected (ADJUDICATION):** `renaming_a_local_task_file_updates_the_same_resource` is unreachable — a rename reaches a server only via a push, and R70 forbids a task push. It ships on the VEVENT control; the property transfers to tasks **by construction**, not by measurement | **L** | |
| T-59 | R104, A-10 | The 686-test baseline, unmodified | U + C | **TZ** |

**R107.** The live tier (T-53..T-58) MUST be runnable from a documented, reproducible container
setup (the probe's `docker run tomsquest/docker-radicale:latest` recipe with bcrypt htpasswd auth
and `owner_only` rights) and MUST be skippable where Docker is unavailable without failing the
default `just test`.

**R108 (probe hygiene, project rule).** Any future probe crate MUST pin `icalendar = "=0.17.10"`,
MUST print the resolved version, and MUST echo the exact input bytes it sent. Two probes in this
effort produced a wrong answer from an unexamined input: one resolved 0.17.13 behind a caret
requirement while claiming 0.17.10, and one embedded literal `\r\n` characters via bash `printf`
and measured a fixture carrying no `COMPLETED` property at all. *A probe that returns the answer
you expected is not evidence until you have inspected the input it actually sent.*

---

## Appendix: Open questions

Left genuinely open by the ADR set plus the adjudication. **Not decided here.**

**Q-1 — Should `--json` honour a `--undated` text flag, or always emit the full set?** ADR-10 ruled
"always the full set" so scripts see everything, at the cost of text and JSON disagreeing on
cardinality, and recorded this as the one sub-question it could not settle on evidence rather than
taste. *Settled by:* a maintainer preference; a one-line policy flip.

**Q-2 — Key name for the task status in `--json`: `task_status` vs `todo_status`.** Cosmetic;
deferred to spec stage against `specs/caldir.md`'s new §To-dos terminology, which does not exist
yet. *Settled by:* writing that section first (R16).

**Q-3 — Does caldir emit `VTIMEZONE`, or tolerate the server adding one?** Radicale synthesizes one
for any `TZID` (probe Q-C), so caldir's pushed body and the stored body differ structurally from
the first sync — harmless under property-level equality, fatal for any whole-document comparison.
No ADR decided it (GRILL M-7). *Settled by:* one decode test, one zoned round trip, and a decision
on the emit side.

**Q-4 — Should `percent_complete: None` and `Some(0)` compare equal?** RFC 5545 gives no default;
two providers disagreeing would be a permanent A-2 failure. *Settled by:* a round-trip probe
against a second server.

**Q-5 — Is `caldir-core` actually consumed out-of-tree?** Published (0.14.1) and presented as a
library, but no external consumer was found. If none exists, C11 relaxes further. *Settled by:*
crates.io reverse-dependencies and download counts.

**Q-6 — Where does R48's "listing is incomplete" signal live** — in `Calendar::events()`'s return
type, or in the diff computation? It must cross a seam that currently carries a flat vector.
*Settled by:* changing the return to a small listing struct and counting call-site churn (~5 core
+ 2 CLI sites).

**Q-7 — Does `caldir update` upgrade core and providers atomically?** This decides how wide the
mixed-version window is, and therefore how load-bearing R49 is. UNVERIFIED —
`caldir-cli/src/commands/update.rs` and `install.sh` have not been read end to end. *Settled by:*
ten minutes of reading.

**Q-8 — Are sibling comp-filters under `VCALENDAR` OR'd in Radicale?** If yes, R59's two REPORTs
could later merge into one. Not worth the risk now. *Settled by:* one probe.

**Q-9 — Does the real u_crawler fixture carry `LAST-MODIFIED` and/or `DTSTART`?** `LAST-MODIFIED`
decides whether R41's Push-biased fallback is on the hot path or never reached; `DTSTART` decides
whether the demo case is anchored or undated (and so whether R84's undated footer ever fires for
the demo). *Settled by:* the Stage 8 fixture swap.

**Q-10 — Does iCloud (Apple Reminders) behave like Radicale?** R69: three load-bearing behaviours
are unmeasured on the one other server riding these exact code paths. *Settled by:* running the
same six probes against a real iCloud account before Stage 8.

**Q-11 — Rename the pre-existing error variants, or keep the old names for continuity?** Pre-1.0
says rename; a maintainer preference overrides. *Settled by:* asking the maintainer.
