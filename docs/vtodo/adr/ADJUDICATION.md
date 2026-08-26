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

## RULING EXTENDED — #[non_exhaustive] also covers CalendarItem
L2-01 correctly flagged that `CalendarItem` is a THIRD growable public
enum in the same position as `CalendarError` and `ComponentKind`: R93
plans to grow it with `VJournal`, and adding the attribute later is
itself breaking. L2-01 left it bare because its AC said "exactly two
variants" — right call for the ticket, wrong outcome for the release.
RULING: all THREE get `#[non_exhaustive]` in the 0.15.0 window:
`CalendarError`, `ComponentKind`, `CalendarItem`. Assigned to L8-05.

## CARRY-FORWARD — public-but-unnameable error types (now TWICE reported)
`CalendarEventError` is `pub(crate) use`d in `calendar.rs` and never
re-exported from `lib.rs`, yet it is the error type of the PUBLIC
`CalendarItem::load` and `CalendarEvent::load`. A downstream cannot
match on it. Same class as the L0-01 carry-forward for `CalendarError`.
Resolve BOTH together, in the 0.15.0 window.

## OPEN — `EventError::UnsupportedComponent` name is unpinned
R76 forbids duplicating the "none" variant on `EventError`, so R19's
carrier there is a distinct `UnsupportedComponent(String)`. Neither the
spec nor any ADR pins that name, and the wire error text is USER-VISIBLE.
Confirm before L2-04/L2-05 wire the RPC codec to it.

## BLOCKER FOR L2-06 — two legacy event() sites are still unowned
L2-03 proved its own completeness by renaming the accessor: exactly TWO
production errors remain, and BOTH are outside its scope:
  caldir-core/src/calendar/event.rs:135  update_attendee_status
  caldir-core/src/calendar/event.rs:147  add_exdate
My L2-02 brief said calendar/event.rs was "already handled". That was
WRONG - L2-02 ADAPTED three field reads INTO `self.event()` calls, so it
created these, it did not resolve them.
L2-06 MUST own these two sites before it deletes the accessor, or the
crate will not compile. Added to L2-06's scope explicitly.

## SCOPE-BRIEF CORRECTION — ".event()" grep over-counts
`.event()` exists on TWO types. `RemoteEvent::event()` is NOT part of the
CalendarItem migration. In caldir-core the split was:
  CalendarEvent::event()  26 production (the migration)
  RemoteEvent::event()    16 (14 calendar_diff, 2 connection) - NOT ours
Any future "N call sites" figure must say WHICH type.

## OPEN, pre-existing, NOT introduced here (own ticket)
`delete_recurring_instance` master/override precedence (L2-03 M7): a
degenerate file that is BOTH a master (recurrence.is_some()) and the
target override hits both branches. Current code prefers master
(EXDATE); nothing locks that choice and no fixture has such a file.
L2-03 preserved the order exactly and did NOT repair it. Needs either a
degenerate fixture or an explicit ruling that the shape is illegal.

## OPEN — CalendarError needs a "not an event" variant
Two unreachable narrowings in calendar.rs fall back to `MasterNotFound`,
which is semantically wrong if it ever fires. When L3-03 gives tasks
real paths this wants a proper variant - and it should ride the same
0.15.0 window as the `#[non_exhaustive]` change.

## L2-05 = NO-OP, but its probe is BINDING INPUT for L2-06
L2-05 changed zero bytes. It proved insulation by RUNNING the L2-06
retype in an isolated probe tree and collecting real compiler errors.

### Exactly what breaks at L2-06 (compiler-verified, 6 errors, 2 crates)
caldir-provider-caldav (1, lib only):
  ops.rs:33  E0308 expected Option<Event>, found Option<CalendarItem>
    Narrow with: .find_map(|r| match r { Ok(CalendarItem::Event(e)) => Some(e), _ => None })
    This WIDENS the existing silent drop from "malformed" to "malformed
    or non-event". L2-06 MUST leave a TODO(L5-01) there, not pretend it
    is clean. Its 3 callers keep compiling once parse_event still
    returns Option<Event>.
caldir-provider-webcal (5; 3 production + 3 in #[cfg(test)]):
  commands/list_events.rs:27,28  E0609 no field last_modified
  commands/list_events.rs:32     E0277 Vec<Event> from Iterator<CalendarItem>
  commands/list_events.rs:87,88,92 same, in the test helper
    Narrow at the existing filter_map :20-26 with an
    Ok(CalendarItem::Event(event)) => Some(event) arm plus Ok(_) => None.
    THAT Ok(_) => None IS A SILENT VTODO DROP. It is R49/L3-04's to fix,
    but L2-06 physically cannot avoid writing the arm, so L2-06 MUST
    mark it TODO(L3-04) or the drop lands unannotated.
google / outlook / icloud: ZERO errors, compiler-confirmed.

### HARD CONSTRAINT L2-06 must not breach — keep the RPC field type Event
Providers are insulated ONLY because the Rust-side RPC struct field
stays `Event`: rpc/methods/create_event.rs:10 `pub event: Event`,
decoded at rpc/ics.rs:32 via the crate-private
Event::from_single_ics_str. There are 25 `cmd.event` sites across all
five provider crates (google 8, outlook 10, caldav 3, icloud 3).
Retyping that field to CalendarItem takes the provider blast radius
from 6 errors to ~25 sites and kills R70 ("no provider-crate edits ship
in this feature").
=> L2-06 retypes ics::deserialize INTERNALLY and narrows back to Event
at that boundary, keeping `expected 1 event(s) in ICS, found 0`
(EventError::UnexpectedEventCount) as the accepted task-push abort per
R71/L6-01. Worth a standalone lock asserting the field type.

### iCloud delegation — VERIFIED
All four ops are pure pass-throughs to caldav::ops with NO mapping layer
(icloud commands/{list,create,update,delete}_event.rs:19-22; the crate
is 686 lines, all session/credential handling). Both hard-coded
<C:comp-filter name="VEVENT"> literals live in caldav
(ops/list_events.rs:103, ops.rs:107) and iCloud reaches them via
ops::fetch_events. L3-02/L5-02 therefore light up iCloud tasks with NO
icloud-crate edit.

### Ticket factual corrections from L2-05
- caldav ops.rs:7 (`use caldir_core::Event;`) does NOT break and must not
  be removed - parse_event's return type still names Event.
- webcal list_events.rs:4 does NOT break; the real breaks are the
  CONSUMERS of the parse result, and 3 of the 6 are in the test module
  the ticket never mentions.

## RULING — doctor's "same event" duplicate wording (raised by L2-08)
The duplicate warning reads "same event saved as multiple files". Now
that the check buckets tasks too, that is inaccurate for a task pair.
RULING: C13 protects EXISTING text for EXISTING situations. An
event-duplicate warning MUST stay byte-identical. A TASK duplicate is a
NEW situation, so new phrasing there breaches nothing. L2-08 correctly
declined to invent it unprompted.
ASSIGNED to L7-05 (doctor surface), which owns doctor wording: give a
task duplicate its own sentence, leave the event sentence untouched,
and pin BOTH with byte-exact locks.

## CARRY-FORWARD — duplicate_file_sets should bucket into a BTreeMap
L2-08's M6 is only ~50% catchable because the MUTANT is
nondeterministic: without the trailing sets.sort() the order comes from
randomized HashMap iteration and lands correct about half the time. The
honest fix is a BTreeMap so ordering is intrinsic and the sort
disappears - but EventInstanceId is not Ord, so this needs a derive on
a public type. Belongs in the 0.15.0 breaking window with the
#[non_exhaustive] work already assigned to L8-05.

## L2 COMPLETE
L2-01..L2-08 all landed. 765 passed / 0 failed, all three zones.
ALL FIVE IMMUTABLE ACCEPTANCE TESTS ARE GREEN, every one verified by the
orchestrator against wip-snapshot rather than accepted from a report.
Cumulative L1+L2 mutation testing: 120 applied, 113 caught, 6 locks
found unable to fail and REPAIRED rather than banked, 1 residual
reported honestly as probabilistic.

## RULING — T-26 moves from L3-02 to L3-03
L3-02 correctly REFUSED to implement T-26 and reported why rather than
writing a test weaker than its name. `EventChange` is still
Create(Event)/Update{from,to:Event}/Delete(Event), so a Todo cannot be
placed in one - the test cannot compile, let alone pass. Retyping
EventChange is L3-03's declared scope, and forcing it into L3-02 would
have dragged in L6-01 (the RPC push abort, since the RPC field must stay
Event) and L7-07 (a task diff currently renders "nothing changed").
RULING: T-26 is REASSIGNED to L3-03, alongside T-27, as part of the
EventChange retype. L3-03's brief must carry it.
R98/A-4's predicate half is already locked by T-24's N/N => TRUE case
plus its disjoint-1990-range assertion; mutation M11 proves it fails
under epoch synthesis.

## NOTE for L3-03's reviewer — T-25 cannot fail YET
T-25 is written, named and green, but it passes because tasks `continue`
at the pass-1 narrowing before any branch, so no mutation at the L3-02
layer can make it emit a delete. It becomes a REAL lock the moment
L3-03 lets tasks into the diff. L3-03's reviewer MUST re-run a mutation
against it then.

## NEW FAILURE SURFACE introduced by R58 - needs a decision
fetch_events now issues a SECOND REPORT per sync against every
CalDAV/iCloud calendar, and a failure on it propagates. Radicale accepts
a bare VTODO query (probe Q-B), but a server that REJECTS VTODO queries
would turn a previously-working VEVENT sync into an error.
Decide fail-loud vs tolerate, alongside L5-01/L5-05.

## ⚠ OPEN THREAD, HIGHEST PRIORITY FOR L7 — the belt is still silent
L3-04 landed `CalendarDiff::suppressed_task_deletes()` but NOTHING READS
IT. The accessor is public and marked TODO(L7). Until the CLI renders
it, task-delete suppression is silent to the user in practice - which is
EXACTLY the criticism PR-1's adversarial reviewer levelled at the
delete-suppression guard (MAJOR-2: "invisible, permanent,
self-perpetuating, and the discovery path is a command the user has no
reason to run").
R49's "and warn" is only HALF discharged. L7 MUST consume this signal.
If L7 ships without it, the feature reintroduces the same defect the
review already flagged once.
ALSO for L7: Connection::diff returns one CalendarDiff PER CALENDAR.
Decide warn-per-calendar vs warn-once BEFORE render time.

## L3 COMPLETE
L3-01..L3-04 landed. 786 passed / 0 failed, all three zones.
Cumulative L1-L3 mutation testing: 184 applied, 173 caught, 11 locks
found unable to fail and REPAIRED rather than banked, residuals reported
honestly rather than padded into clean sweeps.
Notable: L3-03's M14 would have been SILENT WIRE CORRUPTION - a task
pushed to a remote as a synthesized meeting - and nothing asserted the
abort until the lock was repaired.

## RULING — L4-01's AC 1 and AC 2 are RE-HOMED to L4-02
L4-01 refused AC 1 ("Ics wraps a CalendarItem") and verified why:
1. `WireValue::from_wire` is INFALLIBLE (rpc/wire.rs). An item-typed Ics
   forces CalendarItem -> Event with NO error channel => panic or silent
   drop, breaching R19.
2. It does not compile against `pub event: Event`.
3. It makes `serde_json::from_str::<Ics>(VTODO)` SUCCEED, which kills the
   tripwire `a_task_cannot_ride_the_event_only_wire` outright - no
   accessor rename saves a test whose premise is inverted.
RULING: correct refusal. AC 1/AC 2 move to L4-02, which already owns
`provider/handler.rs` and `list_events.rs`. The structural reason is
`WireValue::from_wire`'s infallibility - L4-02 must address that FIRST
or it will hit the same wall.
L4-01 left an item-typed `serde(with)`-compatible module so L4-02 flips
the fields with a ONE-LINE attribute change.

## NEW for the 0.15.0 breaking window (add to L8-05's list)
`WireValue::from_wire` should return `Result`. It is infallible today,
which is exactly why an item-typed response has nowhere to put a decode
failure. Now FIVE items ride that window: #[non_exhaustive] on
CalendarError / ComponentKind / CalendarItem, RemoteError::UnsupportedItem
joining that list, and this signature change.

## PINNED — the wire arity error text is now a byte-exact lock
"expected 1 supported component(s) in ICS, found {n}"
L8's docs must quote it; any rename must move the lock.

## STRIKE from the 0.15.0 window — WireValue::from_wire stays infallible
L4-02 made the Result-returning change UNNECESSARY, not merely deferred.
The inbound direction gets its own newtype (`ItemIcs`) whose
`Deserialize` carries the fallible decode, so `from_wire` only ever
unwraps an already-validated value. The invariant is documented ON THE
TRAIT: *`Wire` must decode straight into the domain value; every fallible
step belongs in `Wire`'s `Deserialize`.*
L8-05's breaking-window list drops from five items to four:
#[non_exhaustive] on CalendarError / ComponentKind / CalendarItem, plus
RemoteError::UnsupportedItem joining that list.

## R70 STATUS — breached, bounded, accepted
L4-02 touched 5 provider files (one adapter each in
caldir-provider-{caldav,google,icloud,outlook,webcal}/src/main.rs).
This is the MINIMUM possible: Handler::list_events is a trait method and
the only inbound channel, so any return-type change forces every
implementor. No provider LOGIC changed; every
commands::list_events::handle still returns Vec<Event>.
The 25 cmd.event sites R70 primarily protects remain UNTOUCHED.
A zero-breach alternative (defaulted `list_items` delegating to
`list_events`) was considered and REJECTED: two overlapping methods where
overriding the wrong one silently drops data is a worse trap than five
identical adapter lines. Concur.
The PR description must state this honestly rather than claiming zero
provider edits.

## OPEN, PRE-EXISTING, now a ticket — provider Handler adapters are untested
L4-02's M12 slipped: replacing a provider's `list_events` body with
`Ok(Vec::new())` passes that crate's suite. NO provider crate has any
test over its Handler impl's adapter layer, and the pre-change one-line
body was equally untested. A dropped listing is a MASS DELETE.
Fix: one `process_request` round-trip test per provider (5 tests).
Assign to L5/L8 QA.

## L8 DOC DEBT — stale inbound contract in the context docs
provider-contract.md (lines 15, 65, 173), system-map.md:58 and
provider-matrix.md:128 all describe `Vec<Event>` as the inbound response
type. That is now `Vec<CalendarItem>`. L8's doc pass must correct them.

## PROCESS — commit after every ticket, without exception
L4-02 ran `git checkout` on a path during a mutation revert and WIPED
L4-01's uncommitted module. It caught this via its own scratchpad backup
and restored byte-for-byte (verified by diff -rq plus the full suite),
but the near-miss was only possible because a completed ticket's work sat
uncommitted while the next ticket ran. Commit immediately on every
ticket completion.

## RULING — T-48's name: the truthful name wins, the spec citation is STALE
L4-03 refused the ticket's test name `a_task_is_pushed_as_a_vtodo_and_
never_as_a_vevent` because it asserts a FALSEHOOD. Nothing is pushed as
a VTODO; nothing is pushed at all. R70 keeps the outbound field `Event`,
so a task cannot ride `CreateEvent` by construction, and both the
ticket's own R56 section and the cut-ADR-07 ruling say the push FAILS
with `expected 1 event(s) in ICS, found 0`.
RULING: correct refusal. The test on disk is
`connection::tests::a_task_push_aborts_and_never_reaches_the_provider_as_a_vevent`
and that name STANDS.
spec §18 and reproduction.md still cite the old name. They are now
STALE. L8-01 MUST update both citations - recorded here the same way
T-26's reassignment was, so the doc set carries the refusal rather than
silently disagreeing with the code.

## OPEN — a pure task-only outgoing Delete has no lock
`[Delete(todo)]` aborts inside `into_event()` before `apply_change` can
return, so the `unreachable!("apply_change returns None for Delete")`
invariant holds by luck of ordering, not by assertion. Cheap to add.
L6-01 territory.

## NOTE for L8's doc pass
`mock_provider::reply_raw_data` (added by L4-03) is now the only
raw-wire escape hatch in the crate - the only way to stub a wire shape
no typed response can express, i.e. a provider NEWER than this core.
Document it in context/testing.md alongside the MockTransport FIFO and
last-request traps.

## ⚠ METHODOLOGICAL FINDING — MockProvider::reply blinds round-trip gates
L4-04's most transferable result. `MockProvider::reply` encodes the
expected value using `to_ics_string` - THE ENCODER UNDER TEST - so a
lossy serialize cancels on both sides and the assertion goes blind.
MEASURED: under a `reply`-based idempotence gate, 6 of 12 mutations that
the raw-fixture version catches SURVIVED (dropped COMPLETED, stripped
X-*, flattened TZID, coerced floating COMPLETED, synthesized DTSTART,
DESCRIPTION drift).
RULE for every future round-trip / idempotence / fidelity assertion:
drive the remote from RAW ICS via `reply_raw_data`, never from domain
values via `reply`. The remote must be an independent store, which is
what a remote IS.
L8 MUST add this to context/testing.md beside the FIFO and
last-request traps, and spec §18's T-49 row must note the raw-fixture
requirement - the ticket's literal "re-serialize the items caldir wrote"
reading produces a gate six mutations walk straight through.

## ANSWERED — open question Q-4
`percent_complete: None` vs `Some(0)` is NOT a churn source today.
`PERCENT-COMPLETE:0` parses to Some(0), re-serializes to
`PERCENT-COMPLETE:0`, parses back to Some(0) - stable. `None != Some(0)`
under Todo's PartialEq, but nothing in the pipeline converts one to the
other, so the distinction never produces a diff. T-49's fixture carries
PERCENT-COMPLETE:0 permanently, so an asymmetric future collapse is
caught. Q-4 CLOSED.

## OPEN — Connection has no "fresh process" seam
T-49 and T-52 both reach into `connection.local` to reload the calendar,
because a second `caldir pull` is a fresh process that reads bases from
the state dir rather than an in-memory map. Two sites now touch a
private field. A small `Connection::reload_local()` would keep future
two-pass tests off it.

## L4 COMPLETE
L4-01..L4-04 landed. 807 passed / 0 failed, all three zones.
Cumulative L1-L4: 227 mutations, 211 caught, 12 locks found unable to
fail and REPAIRED, 3 acceptance criteria REFUSED with verification (one
because the criterion would have asserted a falsehood).

## RULING — wiremock 0.6 IS AUTHORIZED as a caldav dev-dependency
Deferred three times as "a maintainer decision point". Deciding it now,
because it is blocking coverage of a DATA-LOSS path.

Evidence: L5-01's mutation M7 - making `fetch_events` consume only the
FIRST of the two REPORTs - passes the ENTIRE 812-test suite. That is
silent task loss re-created one layer above where L5-01 just fixed it.
M8 (create_event read-back) and M9 (a Handler adapter returning an empty
Vec, = L4-02's M12) slip for the same reason: no HTTP double exists.
A provider adapter that returns an empty listing is a MASS DELETE.

RULING: add `wiremock = "0.6"` to `caldir-provider-caldav`'s
[dev-dependencies]. It is dev-only, never ships in a binary, and is the
standard choice for this in the Rust ecosystem.
Constraints:
- dev-dependencies ONLY, in the caldav crate. Do not add it to core or
  to any other provider without a fresh ruling.
- Prefer a pure function over a double where one suffices - L5-01 showed
  `decode_resources` needed none.
- The PR description MUST disclose the new dev-dependency, with M7 as the
  justification, so a maintainer can judge it on the evidence.
If upstream rejects it, the fallback is a pure response-classifier unit
test plus live-tier coverage, and the M7 gap returns - say so.

## MUST-FIX before ship — the two-REPORT merge (L5-01 M7)
`fetch_events` issues two REPORTs and merges the resources. Nothing locks
the merge. Assign to L5-02 (now that wiremock is authorized) or L5-05's
live tier. This is the highest-value remaining coverage gap in the
provider layer.

## L8 DOC DEBT, added
- tickets/L5-02.md:64, context/system-map.md:55,105 and
  context/local-parser.md:66 all cite `parse_event` / hazard A10 as a
  LIVE defect. A10 is FIXED and the symbol is now `parse_item`.
- caldir-provider-caldav/CLAUDE.md says shared ops live in `src/ops.rs`;
  the real path is `src/caldav/ops.rs` (pre-existing drift).

## PR-DESCRIPTION MUST DISCLOSE — two items from L5-02
1. **New dev-dependency**: `wiremock = "0.6"` in caldir-provider-caldav's
   [dev-dependencies]. Dev-only, never ships in a binary. Justification:
   L5-01's mutation M7 - consuming only the first of the two REPORTs
   passed the ENTIRE 812-test suite, i.e. silent task loss with zero
   coverage. Now locked.
2. **An observable request change on the fast path**: `find_event_location`'s
   fast path was a `GetEtag` PROPFIND on the resource; it is now a
   `calendar-multiget` REPORT on the collection. Outcome-identical for
   events and locked as such, but it IS a different request on the wire.
   Anyone with a server treating the two differently should know.

## CORRECTION — L5-02's AC 5 premise was stale
AC 5 said "caldir-core's DeleteEvent already carries the item after
L4-02, so the kind is available". FALSE: it carries an `Event`
(rpc/methods/delete_event.rs), per R70. The kind is plumbed but
constant-Event end to end today. The outbound wire change belongs with
L6-01's story, not here.

## L8 DOC DEBT, added
`caldir-provider-caldav/src/caldav/ops/test_dav.rs` is now the crate's
HTTP-double seam. context/testing.md must list it beside the
MockTransport FIFO/last-request traps, WITH the raw-ICS rule attached
(drive the remote from raw ICS, never from domain values re-encoded by
the code under test).

## SPEC CORRECTION REQUIRED — R63/R103's rename premise is WRONG
docs/vtodo/spec.md §12 and §17 still say a LOCAL FILE RENAME triggers
`409 no-uid-conflict`. It cannot. The href is UID-derived (`event_url`)
and used unconditionally, so a rename yields the SAME href - a 412 or a
silent 204. The reachable trigger is a SERVER-ASSIGNED FILENAME.
The requirement stands and the handler ships; only the explanation is
wrong. L8-01 MUST fix both sections. Tests assert the OUTCOME, never the
status code. (I introduced this error myself earlier in the effort; it
has now been caught three times by three different agents.)

## OPEN — no recovery path for 412
A lost race on a conditional PUT surfaces as a raw error with no
guidance. Explicitly a non-goal in L5-03. Needs a decision before ship.

## OPEN — update_event has no direct test
It is exercised only transitively via create_event's 409 fallback, so
M10/M11 fail by proxy rather than for the right reason. A dedicated
round-trip test would fix that. Cheap now that the wiremock seam exists.

## ANSWERED — open question Q-3 (VTIMEZONE), by measurement
caldir emits a zoned DUE/DTSTART as a bare TZID parameter with NO
VTIMEZONE component, and TOLERATES the server adding one. Measured live:
Radicale accepts a VTODO with DUE;TZID=Europe/Stockholm and no VTIMEZONE
(201), returns it with a full synthesized VTIMEZONE sibling, and caldir
parses that two-component document back to the same zoned DUE because
VTIMEZONE is exempt from the single-component arity rule (R18/R102).
Q-3 CLOSED. L8-06 records it in BACKLOG.md.

## R69 — STATED, NOT CLAIMED
caldir PULLS tasks from CalDAV but CANNOT PUSH them (R70 keeps the
outbound field Event; a task push aborts with the byte-exact arity
error). iCloud shares the caldav::ops code path but was NEVER MEASURED.
The ship report and PR description MUST say both plainly. Do not claim
iCloud task support anywhere.

## RULING — T-58's name refused, correctly
`renaming_a_local_task_file_updates_the_same_resource` is unreachable: a
rename reaches a server only via a push, and R70 forbids a task push.
Ships as `renaming_a_local_file_updates_the_same_resource_and_never_a_
second_one` on the VEVENT control. The property transfers to tasks BY
CONSTRUCTION (event_url takes the UID and nothing else), not by
measurement - say so rather than implying it was tested.
spec §18 / reproduction.md citations are STALE. L8-01 fixes them.

## NEW dev-dep, must be disclosed with wiremock
`serial_test = "3"` in caldir-provider-caldav [dev-dependencies], to keep
live-tier container ports from colliding. Dev-only.
ALSO: the live tier requires `curl` on PATH. If unwanted, the
alternative is a direct hyper client needing http-body-util as a dev-dep.

## OPEN — find_event_location's fallback masks a broken event_url
L5-05's M4b: a CLOCK-DERIVED href still produced a correct-looking
outcome, because the REPORT fallback rescues it. T-58 now has an outcome
assertion on the href itself, but a dedicated unit lock on event_url
being purely UID-derived would be stronger.

## DOC-SET CORRECTION — L6-01's RED test 1 / AC 6 encode an unreachable path
They specify driving the test from a provider ERROR ENVELOPE
({"status":"error","error":"expected 1 event(s) in ICS, found 0"}) via a
new MockProvider::reply_provider_error. Unreachable under R70: core
aborts in into_event() BEFORE any RPC is serialized, so an old provider
never receives the BEGIN:VTODO payload and never emits that envelope.
Injecting it would test a fabricated path AND assert a request reached
the wire, contradicting the locked !mock.received_a_request().
Correct refusal. L8-01 fixes the ticket text, as it did for T-48's name
and the 409 rename trigger.

## L8 DOC DEBT — the website has no task documentation at all
Before L6-01, `grep -rn "VTODO\|task" website/src/content/docs/`
returned NOTHING. The Google/Outlook notes it added are now the site's
ONLY mention of tasks, which reads oddly. L8's doc pass must introduce
tasks properly and state the push limitation ONCE, globally (R69),
rather than leaving it implied by two provider footnotes.

## OPEN, unrelated to tasks — ProviderError::Provider is untested
The provider error envelope is genuinely reachable in production and has
no test anywhere in core. Found by L6-01 while establishing that its own
AC 6 path was unreachable.

## L6 COMPLETE
839 passed / 0 failed, all three zones, plus 6 live Radicale gates.
Cumulative L1-L6: 300 mutations, 278 caught, 16 locks found unable to
fail and REPAIRED, 6 acceptance criteria REFUSED with verification.

## CORRECTION (mine) — the two arity literals are DISTINCT, not one lock
I recorded these as if they were the same byte-exact lock. They are two
different errors on two different paths:
  `expected 1 event(s) in ICS, found 0`
      = EventError::UnexpectedEventCount
      = the OUTBOUND task-push abort (asserted connection.rs:764)
  `expected 1 supported component(s) in ICS, found {n}`
      = EventError::UnexpectedComponentCount
      = the INBOUND / local decode path (item.rs, rpc/ics.rs)
Both are real locks. L8-01 documented them as two literals rather than
conflating them. Any future brief must name WHICH.

## CORRECTION (mine) — RELATED-TO is DROPPED, not "preserved verbatim"
I instructed L8-01 to write that a single RELATED-TO value "is preserved
verbatim". FALSE, and it correctly refused. `Todo` has no `related_to`
field, `to_icalendar.rs` never emits one, and R15_DROPPED_PROPERTIES
includes RELATED-TO with T-17 asserting the drop.
The phrasing came from ADR-05's justification, which C-2 VOIDED when it
widened the property set. I propagated it without re-tracing it to code.
STILL OPEN: `docs/vtodo/spec.md` §6's RELATED-TO row says "A single value
survives unmodelled", which contradicts R15, T-17 and the shipped codec.
It must be corrected or explicitly scoped to "survives ON THE SERVER
until caldir's first write". Assign to L8-02/L8-04.

## CORRECTION (mine) — the reproduction.md staleness claim was itself stale
I recorded that reproduction.md cites T-48/T-58. It does not - it covers
only the five immutable RED tests, and grep finds no T-48/T-58/T-49
reference. Only spec §18 and the ticket bodies needed fixing, and L8-01
fixed those.

## STILL OPEN after L8-01 (correctly out of its scope)
- context/testing.md additions: `reply_raw_data`, `ops/test_dav.rs`, the
  raw-ICS rule, MockTransport's FIFO/last-request traps, and Radicale
  percent-encoding `@` in hrefs. Real debt; belongs to another L8 ticket.
- Website task documentation (L8-02) and supported-calendar-component-set
  (L8-06) - explicit non-goals of L8-01.

## RESOLVED — the L3-03 / L7-07 disagreement on task field rows
L3-03's TODO text named `due`, `status`, `percent-complete` and
`completed` as the missing rows. L7-07's AC 2 and Non-goals scope only
`summary`, `due` and `status`. L7-07 flagged the disagreement rather
than assuming it handled.
RULING: L7-07's narrower set IS the MVP. Rationale: `status` already
carries the completion signal a user acts on (needs_action -> completed),
and `completed` is only its timestamp. A task update touching ONLY an
unrowed field renders a header line with no rows - under-described, not
invisible, and not data loss.
DEFERRED to a named follow-up, NOT silently dropped: rows for
percent_complete, priority, completed, description, location, url,
reminders, attendees, attachments, x_properties. This is the same defect
class one field further out and must appear in the ship report's known
limitations.

## CLOSED — the highest-priority open thread
`suppressed_task_deletes()` is consumed by caldir-cli/src/output/diff.rs
(L7-07), per calendar, on incoming surfaces only, evaluated before the
is_empty() early return. R49's "and warn" half is discharged and PR-1's
MAJOR-2 is answered.

## NEW for the 0.15.0 breaking window (add to L8-05's list)
`CalendarDiff` needs a non-test-gated constructor for downstream crates.
`compute`/`from_changes` are both `#[cfg(test)] pub(crate)`, so no
consumer outside caldir-core can build one - which is what leaves
L7-07's M14/M15 uncatchable and blocks EVERY future CLI-level lock over
a whole diff, not just this one.

## CORRECTION — C12's "byte-identical" is arithmetically impossible; the
## honest guarantee is the DEL predicate
L7-03 caught this rather than claiming the impossible. Adding five keys
to every entry necessarily changes every line's bytes. What IS true, and
is now proven against bytes captured from the pre-change tree in all
three zones (1653 bytes each):
  select(.kind == "event") | del(.kind,.due,.completed,.percent_complete,.task_status)
  => byte-for-byte identical to the old stream, 26 keys, same order.
The five additive keys are APPENDED at the end, so the pre-existing key
ORDER is preserved.
spec §15's C12 row and L8-02's docs MUST state this predicate rather
than the word "byte-identical", which cannot be satisfied.

## FLAGGED — `completed`'s encoding diverges from ADR-10 D6's wording
ADR-10 D6 tabulates `completed` as "RFC3339 UTC". `Todo::completed` is an
`EventTime` because a FLOATING COMPLETED is legal (R22), so L7-03 encodes
it through the same `serialize_time` rule as every other timestamp: a UTC
value serializes exactly as the ADR says; a floating one renders in the
local zone rather than being coerced, because coercing would INVENT an
offset. Correct call. L8-02's docs must state the actual rule.

## OPEN — two properties have no --json surface
`priority` and `created` are destructured as `_` in the task arm.
ADR-10 D6 says PRIORITY "rides whatever surface ADR-04's property set
gives it, or gains a key under the same rule" - it currently has none.
`created` is unmodelled on Event, so a key would be non-uniform.
Both were explicit ticket non-goals. Decide before ship; they are owed,
not silently dropped.

## CORRECTION (mine) — the accessor is `event()`, not `as_event()`
I have written `as_event()` in several briefs. WRONG since L2-06, which
deleted the temporary infallible `event()` and gave its NAME to the
narrowing accessor. The code is:
    CalendarEvent::event() -> Option<&Event>
`as_event()` survives only as `#[cfg(test)] pub(crate)` on CalendarItem
in core. L7-04's body and ADR-10 D7 also say `ce.event()` - they were
right and my briefs were stale. L8's doc pass should pick one name and
use it everywhere.

## CONSOLIDATE — three separate notes are one ticket
The "no test seam" gaps recorded against output/diff.rs, output/agenda.rs
and commands/rsvp.rs all have ONE root cause: `Caldir::new` is
`#[cfg(test)] pub(crate)` in caldir-core, so no downstream crate can
construct one. Same shape as the already-recorded `CalendarDiff`
constructor gap.
Fold all four into ONE 0.15.0-window ticket: "test constructors for
downstream crates" (Caldir + CalendarDiff). This unblocks every future
CLI-level lock, not just the ones found so far.

## UPSTREAM FINDING (independent of VTODO) — the bump skill misses a pin class
L8-04 found a SEVENTH pin that neither `.agents/skills/bump/SKILL.md`'s
crate table nor constraint C11 mentions:
    caldir-provider-icloud/Cargo.toml:21
    caldir-provider-caldav = { path = "...", version = "0.2.0" }
That is a CROSS-PROVIDER pin, a different class from the six
caldir-core pins both documents cover. `cargo check` HARD-FAILED on it
(requirement ^0.2.0, candidate 0.3.0) until bumped.
Worth fixing in the bump skill regardless of this feature. Mention in
the PR description as a drive-by finding.

## RELEASE ANCHOR — no v* tag exists, locally or on the remote
`git tag` lists only `wip-snapshot`; `git ls-remote --tags origin`
returns nothing. L8-04 used commit `2019223 "Bump versions"` (61 commits
back) as the anchor and derived the per-crate change map from it. That is
the best available choice, but the maintainer should confirm whether
tags were never pushed or this clone fetched without them.

## ⛔ L8-05 IS NOW LOAD-BEARING FOR THE RELEASE
L8-04 correctly declined `#[non_exhaustive]` - the adjudication assigns
it to L8-05 - and flagged the consequence: THE WINDOW CLOSES WHEN 0.15.0
SHIPS. If L8-05 does not land the attribute on all four enums, every
future variant addition is a fresh breaking release.
Exact sites, verified by L8-04 (attribute goes above each, after derives):
  caldir-core/src/calendar/error.rs:8      pub enum CalendarError
  caldir-core/src/event/instance_id.rs:15  pub enum ComponentKind
  caldir-core/src/item.rs:14               pub enum CalendarItem
  caldir-core/src/remote/error.rs:4        pub enum RemoteError
None currently carries it - verified, not assumed.

## STILL OPEN, now unowned — spec.md §6's RELATED-TO row
L8-04 flagged it as outside its file scope. §6 still says "A single value
survives unmodelled", contradicting R15, T-17 and the shipped codec
(RELATED-TO is DROPPED). Route to whoever closes out L8-02/L8-03.

## QA STAGE — seven defects found by independent agents, all invisible to 906 green tests
Four independent QA agents drove the REAL BINARY (one built a baseline
binary from the merge-base to separate new defects from pre-existing
behaviour). Every serious defect sat in a gap between what a test
asserted and what a user would actually do.

FIXED:
 F-1 expanded_events_in_range never applied its window to tasks, so
     --json leaked tasks outside --from/--to. This was MY unclosed
     decision: at L2-07 I accepted core being permissive because
     "suppression is L7's job", and L7-02 implemented that for the TEXT
     renderer only. Todo::occurs_in_range already existed and was never
     called on that path.
 F-2 unparseable DUE/DTSTART/COMPLETED/PRIORITY/PERCENT-COMPLETE
     silently discarded; a malformed DUE turned a DATED task into an
     UNDATED one, which is unconditionally in-window. Events errored
     loudly for the same input. SEQUENCE deliberately stays lenient
     BECAUSE the event codec is lenient - symmetry in both directions.
 F-3 one task silently abandoned every other pending push in that
     calendar, chosen by inode order; exit 0; error on stdout; the task
     rendered with a green + as if it succeeded.
 F-4 PRIORITY:256/-1 silently dropped. R13 says out-of-range MUST
     round-trip verbatim while typing the field Option<u8>, which cannot
     hold either value - the requirement and the type contradicted each
     other. Widened to Option<i32>, and percent_complete with it, since
     fixing only one replaces an old inconsistency with a new one.
 F-6 skipped files were silently swallowed in the agenda -
     is_incomplete() had ZERO CLI consumers.
 TZ  two tests failed at UTC+13/+14 and UTC-11, outside CI's matrix.
     One caught the Local->Utc mutation in ZERO zones before the fix.

NOT FIXED, deliberately:
 F-8 ~1000 nested BEGIN: -> stack overflow, SIGABRT, core dump.
     Reproduced identically with VEVENT on the BASELINE binary, so
     pre-existing and unrelated. Fixing it means changing parser
     recursion for all component types. Ship report, known limitations.

## RULING — the three inverted Connection tests are CORRECTED, not weakened
Audited by me against HEAD. `pushing_a_task_to_a_task_less_provider_...`
previously asserted `captured_request::<CreateEvent>().event == event`
and that the event's sync base WAS flushed - i.e. it ENCODED THE PARTIAL
PUSH AS EXPECTED. That is the defect QA reproduced. It now asserts
`!received_a_request()` and no base recorded. Every data-loss assertion
survived unchanged; four new ones were added. The other two swapped
`Remote` for `UnpushableTask` and gained uid/file/pending assertions.
All three are strengthenings.

## MY SCOPING ERROR — sync and discard were left out
I scoped the CLI reporting fix to `caldir push`. `sync.rs` and
`discard.rs` carry the identical shape, and `sync` is the command users
actually run. Dispatched separately.

## KNOWN CONSEQUENCE for the ship report — a refused task aborts sync's PULL half
`caldir sync` renders ONE combined diff, so once the outgoing half is
refused there is no honest way to show the incoming half while
suppressing the outgoing one. A partial render would misrepresent what
happened. Consequence: a user with a task in a synced calendar must run
`caldir pull` rather than `caldir sync` until the task is moved.
`pull` on such a calendar works and exits 0 - locked by
`pull_ignores_a_local_task_and_exits_zero`.
Recorded in a doc comment at the site, and must appear in the ship
report's known limitations.

## RULING — `discard` correctly does NOT get the pre-flight guard
`discard_outgoing_diff` is purely local and never touches the wire, AND
dropping the task is one of the two remedies the refusal message itself
offers. Guarding it would strand the user with no way out. Locked by an
INVERSE mutation (guard wrongly copied in => caught).

## CORRECTION — ADR-01's headline justification was the wrong argument
I have been citing "41 errors across 7 files holding 220 VEVENT tests"
as why CalendarItem beat widening Event. The architecture reviewer found
that measured MIGRATION FRICTION, not architectural necessity. The
enum's real justification is STORAGE: SyncBases is one
HashMap<EventInstanceId, Option<Box<...>>> that must hold both kinds,
and Box<dyn Component> would lose the derived PartialEq that IS the
sync-direction test (event_bases.rs, calendar_diff.rs).
Right conclusion, wrong argument. The PR description must not repeat the
41-error framing as the reason.

## VJOURNAL — the structural claim survives, the COST estimate does not
Structural (survives): diff engine, state layout, wire, identity format
and the push pre-flight all absorb a new variant unchanged, because the
pre-flight keys on ComponentKind rather than Rust type.
Cost (does not survive): measured 13 exhaustive match arms in item.rs
alone, plus a todo.rs-sized codec module (~310 prod lines) recurring PER
KIND, because ADR-01 step 6's shared read_shared/write_shared<C:
EventLike> was never built.
And a trap: in caldir-cli, six wildcard arms would compile SILENTLY
against the #[non_exhaustive] enum while one unreachable!() PANICS on
the --json path (agenda_json.rs). #[non_exhaustive] protects the crate
boundary but converts a compile error into a runtime panic for a
downstream consumer that guessed wrong.
