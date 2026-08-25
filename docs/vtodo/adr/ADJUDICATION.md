# Stage 3 adjudication — lead engineer rulings

Inputs: 9 ADRs, GRILL-FINDINGS.md (455 lines), context/radicale-probe.md
(Radicale 3.7.6, live). Rulings below are BINDING on Stage 4 (spec) and
Stage 5 (tickets). Where a ruling contradicts an ADR, THIS FILE WINS.

## Status of each ADR
| ADR | status |
|---|---|
| 01 component abstraction | ACCEPTED with amendments (strike #3, #7, and step-5 `_ => skip`) |
| 02 identity | ACCEPTED with amendments (rationale corrected; strike rule #4 href) |
| 03 file model | ACCEPTED |
| 04 todo properties | AMENDED (see C-2 ruling — property set widened) |
| 05 round-trip preservation | ACCEPTED (scope confirmed X-* only, by measurement) |
| 06 change detection | ACCEPTED with amendment (acceptance gate must be structural) |
| 07 provider capabilities | **REJECTED / CUT from MVP** (user decision) |
| 08 window semantics | ACCEPTED — best-evidenced decision in the set |
| 09 backward compatibility | ACCEPTED with amendments (C4 and C12 amended, see below) |
| 10 CLI display | **TO BE WRITTEN** (gap found by grill B-5) |

## C-1 — EventInstanceId gains ComponentKind. RULED FOR ADR-02.
Evidence: grill independently re-traced diff mis-keying
(`calendar_diff.rs:25-28` -> `:33` -> `:36` -> `:38` -> `:63` emits Update)
and confirmed hazards 3,4,5 incl. `doctor/warning/duplicate_file.rs:14-20`.
ADR-01 #7 mitigates only hazard 4 -> a gap, not a rival.
- STRIKE ADR-01 decision #3 ("unchanged key type") and #7.
- STRIKE ADR-02 rule #4 (kind-distinct href): Radicale returns
  409 no-uid-conflict for same UID at a different href. Href stays
  `{uid}.ics` for BOTH kinds, unchanged from today.
- ADR-02's premise ("a collection may legitimately hold both sharing a UID")
  is FALSE for Radicale (409 / 400 / 204). Severity re-derived from the
  LOCAL trigger alone: a local mixed directory (`calendar.rs:113` globs
  `*.ics`) or two calendars. User-triggered, silent, destructive.
  One enum field closes all five hazards. ComponentKind STANDS.
- NEW REQUIREMENTS (from live probe, own tickets):
  - handle `409 no-uid-conflict` on push (a local RENAME changes the slug,
    not the UID, and produces exactly the rejected pattern)
  - never PUT a multi-component VCALENDAR (Radicale: 400)

## C-2 — property set widened. ADR-04 AMENDED.
ADR-05's passthrough is X-* ONLY (measured), so ADR-04's conditional fails.
ADR-05's mitigation is TRUE but bounded: after a pull, local/base/remote all
lack the property equally so nothing pushes — but CalDAV update is a
whole-resource PUT (`update_event.rs:33-38`), so the FIRST GENUINE EDIT
destroys it. Radicale is a faithful store (holds both ATTACH, both
RELATED-TO with params, ATTENDEE, CATEGORIES unescaped), therefore
everything caldir drops is caldir DESTROYING CORRECT SERVER DATA.
Cost of flipping IN is ~zero: `to_icalendar.rs:37-39,56,72-78,87-89` already
emits these via `EventLike`, which `Todo` implements, and ADR-01 already
places all five on `Todo`.

FLIP IN: ATTACH, ATTENDEE, ORGANIZER, LOCATION, RRULE/EXDATE/RDATE,
         PERCENT-COMPLETE, CREATED.
- RRULE was UNFLAGGED by ADR-04, which claimed A05 preserved it. FALSE.
  Every recurring task's RRULE would die on first edit.
- CREATED flips IN on DATA-SAFETY grounds (see C-4 below), not fidelity.
STAYS OUT: CATEGORIES — crate corrupts it (`WORK,URGENT` ->
  `WORK\,URGENT`, verified on pinned 0.17.10), and a todo-only exception
  inside a table named "Fields We Skip" is incoherent.
STAYS OUT: RELATED-TO (lossy in parser; single value survives via A05).
FORBIDDEN: DTEND — `Todo::ends()` verified to really emit it on 0.17.10.

## C-3 — probe hygiene. VALARM STAYS IN.
`valarm-probe` resolved icalendar **0.17.13**, not 0.17.10 — ADR-04's cited
version was FALSE (caret requirement drift; the exact trap ADR-05 found).
Re-run PINNED `=0.17.10`: Path B byte-stable across 3 generations, Path A
regenerates a random VALARM UID. Conclusion survives, evidence replaced.
RULE FOR ALL FUTURE PROBES: pin `icalendar = "=0.17.10"` AND print the
resolved version AND echo the exact input bytes sent. Two probes in this
effort produced a wrong answer from an unexamined input (the other:
grill's own bash `printf` `\r\n` fixture, retracted as F-1).

## C-4 — CREATED and the manufactured-vulnerability interaction. NEW.
Live probe: Radicale implements RFC 4791 §9.9 correctly over 8 fixtures.
An undated task with only `COMPLETED` older than window start is OMITTED by
a time-range query. A task carrying CREATED is RESCUED by the `end > CREATED`
row. ADR-05 drops CREATED => **caldir's own writes manufacture the
vulnerable shape.** Hence CREATED IN. This is defence in depth, NOT a
substitute for ADR-08 D2 (no time-range on the VTODO query), which remains
the primary fix.

## B-3 — unknown components MUST error, never skip.
ADR-01 step 5's `_ => skip` breaches ADR-09 C10 (INVIOLABLE) and contradicts
ADR-03 rule 2; a silent drop also feeds C7's delete path. Verified
implementable: unknown components surface as `Other(kind)` with a readable
name on 0.17.10, so ADR-03's `NoSupportedComponent { found }` can name it.

## B-4 — ADR-09 C4 IS AMENDED.
The `VTODO:` sigil breaches C4 as written ("with no new sigil"). Ruled FOR
ADR-02: without a per-kind string `known_event_ids` collapses the pair and
there is no alternative fix. C4 is amended to permit a kind sigil, and its
test is rewritten to assert: existing unprefixed lines still parse as
`kind: Event`, and an older caldir treats a `VTODO:`-prefixed line as an
inert unknown id that cannot trigger a delete.
Residual accepted: a VEVENT whose UID literally begins with `VTODO:` would
round-trip as a todo. Same class of ambiguity upstream already accepts for
`__` (`instance_id.rs:38-40`); no encoding removes it (UID is TEXT).

## B-5 — USER DECISION: tasks appear in the agenda, anchored on DUE.
ADR-09 C12 IS AMENDED: DUE anchors a task in the agenda as DTSTART anchors
an event. Undated tasks get their own section. Requires an owning ADR-10
(dispatched) covering agenda placement, sort order, undated handling, the
`--json` schema change, and which existing output/scripts are affected.

## ADR-07 — USER DECISION: CUT ENTIRELY from the MVP.
No config field, no builder, no provider-crate edits, no third diff bucket,
no renderer/pluralization, no `is_empty()` fix. Rationale: it bought a nicer
Google/Outlook error while every other ADR places those providers outside
the MVP, AND its own default meant every existing caldav/iCloud user got
zero task support after upgrading until re-running `connect` — worse than
the message it prevented.
ACCEPTED CONSEQUENCE: pushing a task to Google/Outlook aborts the rest of
that calendar's push at `connection.rs:231` with
`expected 1 event(s) in ICS, found 0`. Ugly, NOT data loss — ADR-08 D3 and
ADR-09 C7 cover the destructive variants.
PRESERVE FOR FOLLOW-UP (do not implement): `supported-calendar-component-set`
IS advertised by Radicale on every collection and is bulk-readable via ONE
Depth-1 PROPFIND on the home-set — but it is ADVISORY ONLY (a VEVENT PUT
into a VTODO-only collection returned 201), so it can never be an
enforcement point. Record in spec §Non-goals.

## ADR-06 amendment — acceptance gate must be STRUCTURAL.
Radicale does NOT echo PUTs byte-for-byte: properties sorted alphabetically
within each component, lines re-folded at 75 octets, VTIMEZONE synthesized
for any TZID. It IS idempotent (2nd round trip byte-identical).
=> "sync twice, second diff empty" MUST be parsed-domain `==`, or
GET-pass-2 vs GET-pass-1. NEVER raw bytes against what was PUT.
ETag is invariant to property reordering but DTSTAMP changes it, so every
caldir push mutates the ETag: ETag can prove "remote changed", never
"caldir's push was a no-op".

## Carried-forward corrections to project docs (separate commits)
- CLAUDE.md sync description is STALE (says mtime; real algorithm is
  three-way vs stored base first, `calendar_diff.rs:153`).
- CLAUDE.md:52 names `ProviderHandler` and links `caldir-core/src/rpc/handler.rs`.
  Real trait is `Handler` at `provider/handler.rs:27`; that path does not exist.
- specs/caldir.md lists RDATE as skipped, but it IS implemented
  (`event/recurrence.rs:5-17`).
- Pre-existing bug, own ticket: CalDAV `ops.rs:32-37` `find_map(Result::ok)`
  silently drops all but the first parsed component.

## Unverified premise, still open
u_crawler's `windows/`/`deadlines/` layout is described by the user but was
NOT found on disk (no .ics files in /home/belcaik/Dev/u_crawler). Treated as
accurate for design purposes; the Stage 8 fixture swap settles it.

## RULING (post-DAG) — adaptation of the 5 immutable RED tests
Raised by the DAG architect on ticket L2-06: the five Stage-2 RED tests
cannot COMPILE against the ADR-01 `CalendarItem` API. They call
`Event::from_ics_str` / `CalendarEvent::event()` and read `.uid`/`.summary`
directly off an `Event` — precisely the signatures ADR-01 changes.

RULING (lead engineer):
- The ASSERTIONS are immutable. No assertion may be weakened, removed,
  reordered into irrelevance, or have its expected value changed.
- The ACCESSORS may be adapted. Only the call syntax used to REACH the
  asserted value may change.
- Every adaptation MUST appear in the diff as a mechanical rename with the
  assertion line itself untouched.
- L2-06's reviewer MUST diff the five tests before/after and confirm no
  assertion drifted. This is a named review gate, not a courtesy check.
- Test names MUST NOT change (they are cited in spec §18 and reproduction.md).

Rationale: the user's intent in declaring them immutable was that the
acceptance BAR cannot move. A compile-driven accessor rename does not move
the bar; a changed assertion does. Preserving compiling shims purely to
avoid touching the files was considered and rejected as dead production
code that would itself need removal later — but it remains available if the
user prefers the stronger guarantee.

## SEQUENCING NOTE (post-DAG)
PR-1 (L0-01/02/03) turns NONE of the five RED tests green. All five assert
the VTODO LOADS AND IS VISIBLE, not that it degrades softly. PR-1 therefore
ships with its own tests (T-29 + degradation/doctor locks). The five RED
tests go green at L2-06 (three) and L2-07 (two).
Do NOT accept "the reproduction passes" as PR-1's exit criterion.

## RULING EXTENDED (post-ticket-bodies) — scope of the RED-test adaptation rule
The post-DAG ruling above was scoped to L2-06. The L0+L1 ticket author found
that **L0-01 also breaks two of the five RED tests** (`calendar.rs:1161`
`events_lists_a_vtodo_file_alongside_its_vevent_sibling`, and the control
`caldir.rs:417` `vevent_calendar_lists_independently_of_a_sibling_vtodo_calendar`),
because retyping `Calendar::events()` changes a signature they call. That is
in PR-1, not the deep refactor.

RULING: the adaptation rule is GLOBAL, not L2-06-specific. It applies to
ANY ticket in ANY layer that breaks compilation of the five RED tests:
- assertions immutable (no weakening, removal, or changed expected values)
- test NAMES immutable (cited in spec §18 and reproduction.md)
- accessors adaptable, as a mechanical rename, assertion line untouched
- the ticket's reviewer MUST diff the affected RED tests before/after
Every such ticket carries the named review gate, not just L2-06.

## KNOWN DOC DEFECT — line numbers drift 1-2 lines
Spec/ADRs cite `calendar_diff.rs:78-90` (actual `:76-90`) and
`connection.rs:231` (actual `:232`). Systematic across the doc set.
IMPLEMENTERS: grep the SYMBOL, never trust the line number.

## CORRECTION — the 409 no-uid-conflict trigger (lead engineer error)
I stated in three ticket briefs that a local file RENAME produces
`409 no-uid-conflict`. THAT IS WRONG. The href is UID-derived
(`event_url`, `client.rs:73-76`) and used unconditionally at
`create_event.rs:26`, so a rename yields the SAME href -> 412 under
`If-None-Match: *`, or a 204 overwrite. The reachable 409 is a
SERVER-ASSIGNED FILENAME (a resource at another href holding that UID) —
which is what `find_event_location`'s REPORT fallback exists for.
The requirement stands; the trigger in spec/tickets is wrong. Tests MUST
assert the OUTCOME, not the status code.

## RULING — cargo fmt vs the RED-test immutability gate
Raised by the L0-01 implementer: `cargo fmt --all -- --check` is RED on this
branch from the STAGE-2 RED-test code (pre-existing; committed HEAD is
fmt-clean). rustfmt wants to reflow an `assert_eq!` line inside an
immutable RED test.

RULING: reformatting is NOT an assertion change. Whitespace and line
wrapping alter neither asserted values, expected results, nor test names.
`cargo fmt --all` IS PERMITTED on the RED tests, and CI fmt MUST be green.
The immutability gate protects SEMANTICS, not formatting.
Whoever next touches those files runs `cargo fmt --all` and commits it.

## CARRY-FORWARD from L0-01 (small, own ticket)
`CalendarError` is not re-exported from `caldir_core`'s root, so
`SkippedEvent::error()` returns a public-but-unnameable type. Works today
(Display via method resolution) but a consumer cannot match on it.

## NOTE for L0-02 — doctor already carries the UnreadableEvents line
L0-01 was FORCED to touch `doctor.rs`: the mechanical retype silently
regressed `treats_unreadable_events_as_warnings` to zero warnings, which is
exactly the R77 violation. L0-01 added one
`DoctorWarning::UnreadableEvents(...)` per skipped file, rendering
byte-identically to before. L0-02 must REFINE that, not re-add it.

## RULING — CLASS is not a modelled field on Todo (raised by L1-01)
Spec §6 says CLASS "MAY be inherited, MUST NOT be added deliberately", but
does not say whether it is a FIELD. L1-01 omitted `visibility` from `Todo`.
RULING: CONFIRMED. CLASS is NOT a modelled field on `Todo`. An inherited
CLASS survives only if the unmodelled-property path carries it, which per
ADR-05 (measured) is `X-*` ONLY — so in practice CLASS on a VTODO IS
DROPPED on first caldir write. This is consistent with the §Non-goals
fidelity guarantee and must be stated there, not discovered at codec time.
The codec ticket MUST NOT invent a field for it.

## PINNED — TodoStatus serde labels (raised by L1-01)
L1-01 inferred `needs_action`/`in_process`/`completed`/`cancelled` by
analogy with `Status`. ADR-10 independently specifies the `--json`
`task_status` key with exactly those four values. PINNED — they agree.
The renderer ticket (L7-03) MUST NOT re-derive them, and MUST NOT retype
the existing `status` key (C12).
NOTE: L1-01's report calls ADR-10 "TO BE WRITTEN" — that is STALE text in
the ticket bodies. ADR-10 EXISTS at docs/vtodo/adr/ADR-10-cli-display.md
and owns all CLI/JSON decisions.

## NOTE — stale baseline numbers in ticket ACs
Several tickets say "686 baseline". Real baselines by phase:
  committed HEAD           686 pass / 0 fail
  + Stage-2 RED tests      691 pass / 5 fail
  + PR-1 (L0-01..03)       697 pass / 5 fail
  + L1-01                  699 pass / 5 fail
Implementers: use the CURRENT measured tree count, not the AC's number.

## CORRECTION — DTEND reaches icalendar::Todo via TWO traits, not one
The spec, tickets and briefs say `EventLike` leaks `ends()`/DTEND onto
`Todo`. That is INCOMPLETE and partly wrong (verified against the pinned
0.17.10 registry source by the L1-03 agent):
- `Component::get_end` at `components.rs:199` is the GETTER.
- `EventLike::ends` at `components.rs:394` is the SETTER.
- `component_impl! { Todo }` (`:664`) and `event_impl! { Todo }` (`:665`)
  mean BOTH traits apply to `Todo`.
So DTEND is reachable from two traits. Any ticket guarding against DTEND
must block both. L1-03 confirmed the parse side is clean via three
barriers; the SERIALIZE-side proof (emitted bytes contain no `DTEND:`)
belongs to L1-04 and MUST be written there.

## ACCEPTED NON-LOCK — M8 (`get_status()` vs raw STATUS read)
L1-03 mutation-tested 18 mutations; 17 are caught. M8 (swapping the raw
STATUS read for the crate's `get_status()`) SLIPS, and is UNOBSERVABLE at
that seam: `icalendar::TodoStatus::from_str` (`properties.rs:303-311`) is
a byte-for-byte match of caldir's `TodoStatus::from_ics_str` — same four
literals, same `_ => None` — and both call sites `.unwrap_or_default()`.
Since R14 rules the outcome is DROP, the two are behaviourally identical
on 0.17.10.
RULING: this is a REASONED NON-LOCK, not a testing gap. Reading raw is
future-proofing (the crate's enum is not caldir's, and its commented-out
`//Custom(&str)` shows upstream intends to widen it). Do NOT invent an
assertion that reaches into implementation detail to "catch" M8.
Reviewers: this is known and accepted. See follow-up ticket idea:
a crate-version-pinned guard for the day upstream adds Custom.

## RELEASE IMPACT — PR-1 needs a MINOR bump (0.14.1 -> 0.15.0)
L0-05 adds `CalendarError::IncompleteListing { unreadable, first }`.
`CalendarError` is NOT `#[non_exhaustive]`, so a downstream that matches
it exhaustively WITHOUT a wildcard arm stops compiling. Anything using
`?`, `Box<dyn Error>`, or a `_` arm is unaffected.
Per .agents/skills/bump/SKILL.md the project is pre-1.0 and gives
breaking changes a MINOR bump => 0.15.0.
OPEN CALL for whoever cuts the release: marking `CalendarError`
`#[non_exhaustive]` would prevent this class of break in future, but is
itself breaking, so L0-05 deliberately did NOT do it.

## KNOWN REMAINING HOLE — `Calendar::master_event_for` (deliberate)
Same absence-inference shape as the three L0-05 fixed: returns `Ok(None)`
when the master's file is unreadable. L0-05 left it because it NEVER
WRITES, and its one internal caller (`update_recurring_instance`'s None
branch) is now only reachable on a complete listing. But a GUI calling
`master_event_for` directly still cannot distinguish "no master" from
"unreadable master". Follow-up if the whole absence-query family should
be made coherent. Not a data-loss path.

## PR-1 FINAL STATE
Branch `pr1-graceful-degradation`, 2 commits off main:
  f5517c1 graceful degradation + delete suppression
  68bf616 do not infer "absent" from an unreadable event file
695 passed / 0 failed. clippy -D warnings clean. fmt clean.
Zero VTODO bytes. Description at docs/vtodo/PR1-DESCRIPTION.md.
Ready to open upstream.

## RULING — filename for a RECURRING TASK INSTANCE (raised by L1-07)
R34's filename table has rows for: recurring task, dated task, undated
task. It has NO row for a recurring task INSTANCE (an override:
`recurrence_id` is Some, `recurrence` is None). The spec is silent and
L1-07 correctly refused to invent one.

RULING: follow the EVENT convention exactly, which is already the
implemented behaviour — so no code change is needed.
`Event::base_slug` keys the `_recurring__` prefix on `recurrence.is_some()`,
NOT on `recurrence_id`. An event override therefore gets the DATED form,
named by its own instance time. Tasks mirror this: a task override falls
into the dated row (`_task__{time_slug}__{summary}`), or the undated row
if it has neither DUE nor DTSTART.
Rationale: the `_recurring__` prefix marks "this file is the MASTER of a
series". An override is not a master; it is one dated occurrence, and
naming it by its instance time is what lets a human find it. Diverging
from the event convention here would also break the symmetry L1-08's
control locks depend on.
This is R34's missing row; L8-01 MUST add it to specs/caldir.md.

## CARRY-FORWARD from L1-07 (own ticket, before or during L2-02/L2-06)
Slug-collision surface for UNDATED tasks. `write_best_event_file`
disambiguates with a `-{suffix}`. Two undated tasks sharing a summary
collide on `_task__{summary}.ics` far more readily than events ever do,
because an event always carries a date or the `_recurring__` prefix.
The suffix path presumably handles it, but nothing proves it for the
`_task__` shape. Needs an explicit test when the writer becomes
item-aware.

## NOTE — stale baselines, continued
The pre-L1-07 tree measured 720/5, not the 699 the earlier note stops at.
Counts move as concurrent agents land tests. IMPLEMENTERS: measure the
tree yourself and track DELTAS, never absolutes from a ticket AC.

## RULING — mark the growable public enums #[non_exhaustive] IN THIS RELEASE
Raised twice: by L0-05 (`CalendarError`) and by L1-06 (`ComponentKind`).
Both are now PUBLIC, neither is `#[non_exhaustive]`, and both are KNOWN
to be growable:
- `CalendarError` gained `IncompleteListing` in L0-05, which is already
  breaking for a downstream matching exhaustively without a wildcard.
- `ComponentKind` gains `VJournal` under R93.
Adding a variant to a non-exhaustive-less public enum is a breaking
change EVERY TIME. Adding the attribute is itself breaking - so it must
ride the breaking release we are ALREADY taking (0.14.1 -> 0.15.0),
or we pay the same cost again later for no benefit.

RULING: mark BOTH `#[non_exhaustive]` in this release cycle.
- `CalendarError` -> on the PR-1 branch (it ships first).
- `ComponentKind` -> on vtodo-support.
Assigned to ticket L8-05 (release prep) if not done sooner; it must not
be dropped, because after 0.15.0 ships the window closes.

## CORRECTION — L1-06's parse-order note was WRONG (strike it)
L1-06's "Notes for the implementer" claimed parse order matters because
stripping the sigil after the rid split would mangle a UID containing
`__`. FALSE: `rsplit_once` takes the LAST `__` and the sigil is a prefix
containing none, so both orders yield identical (kind, uid, rid) for
every input, including the empty-UID and `VTODO:__20260109` edges.
It would matter only for `split_once`. Strike the claim from the ticket
and from any downstream brief repeating it. The implemented order is
sigil-first anyway, for clarity and future-proofing.

## L1 COMPLETE
L1-01..L1-08 all landed. 731 passed / 5 failed, all three zones.
Cumulative mutation testing across L1: 55 applied, 54 caught, 1
equivalent. TWO locks were found unable to fail and were REPAIRED
rather than banked (L1-08's M7 superset hole in x_properties_eq;
L1-06's M8 non-colliding task fixture).
