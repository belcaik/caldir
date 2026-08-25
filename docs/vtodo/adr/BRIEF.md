# Shared brief for Stage 3 decision agents

## Mission
Add first-class VTODO to caldir. Local VTODO <-> domain <-> provider RPC <->
generic CalDAV (Radicale). Tasks must NEVER be converted into fake events.
Zero VEVENT regressions.

## You are a DECISION agent
Write an ADR candidate. Do NOT write production code. Do NOT modify source.
You MAY grep/read source to ground claims, and MAY write scratch experiments
under the scratchpad (never in the caldir workspace).

Prefer the SMALLEST change that satisfies the mission. caldir is a lean
upstream project (t4t5/caldir); this work is intended as an upstream PR.
Unnecessary abstraction is a defect, not a virtue.

## Required reading (in order)
- docs/vtodo/context/system-map.md   (start here; has the master assumption table)
- docs/vtodo/reproduction.md         (verified failure + RED tests)
- then ONLY the context docs your ADR names.

## Verified ground truth (do not re-litigate)
- Sole VEVENT filter: `caldir-core/src/event.rs:101` `icalendar.events()`.
  On a VTODO doc: components=1 events=0 todos=1. Parsing is fine; the filter drops it.
- Poison pill: `calendar.rs:114` `?` fails the whole calendar;
  `caldir-cli/src/output/agenda.rs:35` `?` then fails the WHOLE agenda across all calendars.
- Two arity rules must move together: `CalendarEventError::ExpectedSingleEvent`
  (disk, calendar/event.rs:45) and `EventError::UnexpectedEventCount`
  (wire+bases, event.rs:116).
- Wire carries OPAQUE ICS already (`rpc/ics.rs`). No new RPC method is needed.
  `Method` is a closed serde enum with NO `#[serde(other)]` -> a NEW RPC VERB
  IS A HARD BREAKING CHANGE against installed provider binaries. Forbidden.
- `deny_unknown_fields` appears NOWHERE -> adding OPTIONAL fields is wire-safe.
- Trait is `Handler` (`provider/handler.rs:27`), NOT `ProviderHandler`.
  CLAUDE.md:52 is wrong (wrong name + dead path). Use the real name.
- Sync direction: three-way vs stored base FIRST (`diff/calendar_diff.rs:153`),
  mtime-vs-LAST-MODIFIED only as fallback (`:180`). CLAUDE.md is stale; specs/caldir.md is right.
- `Event` PartialEq is educe-tuned: ignores last_modified+sequence,
  order-independent x_properties, attachments by uri. `==` MEANS "content changed?".
- Identity: `EventInstanceId((EventUid, Option<RecurrenceId>))`, Eq+Hash,
  `{uid}__{rid}` string round-trip. Component-agnostic today.
- Delete inference keys on `sync_bases.get(&id).is_some()` — component-agnostic.
- Window guard is the ONLY delete guard: `occurs_in_range` =
  `start < to && end.unwrap_or(start) > from` (event.rs:136). Undefined without DTSTART.
- CalDAV hard-codes `<C:comp-filter name="VEVENT">` in exactly 2 places:
  `ops/list_events.rs:103`, `ops.rs:107`. PUT/DELETE never inspect payload.
  `supported-calendar-component-set` is never read.
- CalDAV `ops.rs:32-37` `find_map(Result::ok)` silently drops all but the first
  parsed component. Pre-existing bug.
- iCloud has NO mapping layer; delegates entirely to caldav ops.
- WebCal already receives VTODO and SILENTLY DROPS it (zero items, no error).
- Google `from_google.rs:17-22` hard-fails without start+end. Outlook `GraphEvent`
  has no due/status/completed. Their task APIs are DIFFERENT APIs needing NEW
  OAuth scopes (outlook consent: Calendars.ReadWrite User.Read offline_access)
  => re-consent for every existing user.
- icalendar 0.17.10, features: default ["parser"] only (recurrence/chrono-tz OFF).
  `Todo` and `Event` differ ONLY in the STATUS enum + Todo's 3 extra props.
  `.todos()`, `.as_todo()`, `CalendarComponent{Todo|Event|Venue|Other}` (#[non_exhaustive]) exist.
  TodoStatus = NeedsAction/Completed/InProcess/Cancelled.
  VERIFIED: arbitrary X-* props AND their params round-trip verbatim.
  VERIFIED LIMITS: RELATED-TO collapses to last (not in MULTIS) => hierarchies lossy;
  VALARM in VTODO gains synthesized DTSTAMP + FRESH RANDOM UID each serialize
  (round-trip not byte-stable); EventLike leaks ends()/DTEND onto Todo (RFC-invalid);
  unknown STATUS values vanish; floating COMPLETED reads None; priority() clamps 0-10 (RFC 0-9);
  no typed API for DURATION/GEO/CATEGORIES/ORGANIZER/RELATED-TO.
  NOTE: caldir already hand-rolls the VEVENT alarm block (`Reminder::ics_block`,
  spliced on literal "END:VEVENT\r\n" at event.rs:249), so it ALREADY sidesteps
  the crate's alarm handling. Weigh that before excluding VALARM.
- `specs/caldir.md` "Fields We Skip" currently declares PRIORITY, CATEGORIES,
  RESOURCES, COMMENT, CONTACT out of scope. USER DECISION: specs/caldir.md WILL be
  amended as part of this work. So "the spec says no" is not a blocker — but any
  amendment must be stated explicitly in your ADR.
- Tests: NO tests/ dirs, NO .ics fixture files, all inline #[cfg(test)].
  test_utils.rs is crate-private (cli/providers cannot reuse it).
  CI = TZ matrix UTC/Europe-Stockholm/America-New_York with -D warnings.
  Baseline 686 pass. `Connection` is the highest no-network seam.
- Toolchain pinned 1.97.0. NEVER run parallel cargo invocations.
- USER DECISION: fixture is synthetic now; real u_crawler bytes swap in at Stage 8.

## Observed-but-undecided behaviors (may be YOUR decision to make)
- VEVENT+VTODO in one file: currently NO error, VTODO silently dropped, and a
  rewrite would drop it on disk.
- Zero components: same `found: 0` error as a VTODO file — the message cannot
  distinguish "unsupported component" from "empty file".

## ADR output format
Write `docs/vtodo/adr/<your-adr-filename>`:

# ADR-NN: <title>
## Status
PROPOSED (Stage 3 candidate)
## Context
<the forces, with file:line evidence>
## Options considered
<at least 2 real options, each with cost/benefit/blast radius in files. Be fair
to the option you reject.>
## Decision
<the single recommendation, stated so an implementer cannot misread it>
## Consequences
<what becomes easy, what becomes hard, what must be tested>
## Backward-compatibility impact
<explicit: local files on disk, .caldir state, RPC wire, installed provider binaries>
## Rejected because
<one line per rejected option>
## Open sub-questions
<anything you could NOT settle, and the cheapest experiment that would>

Keep it under 150 lines. Ground every claim in file:line or a run experiment.
Mark anything unverified as "UNVERIFIED:".
