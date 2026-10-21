# Review 2 of 5 — SPEC COMPLIANCE

Scope: `docs/vtodo/spec.md` R1–R108, C1–C13, §17 A-1…A-10, §18's 59-row test matrix,
audited against the tree at `5a2f756` (`vtodo-support`).

Method: every row below was checked against code or a test that I read. A green suite was
never accepted as evidence on its own. Line numbers are from HEAD.

**Tree state note.** The working tree is *dirty*: `caldir-cli/src/commands/sync.rs` is modified
and `caldir-cli/tests/sync_exit_status.rs` is untracked (the "sync and discard were left out"
follow-up the adjudication dispatched). That untracked target fails **non-deterministically**
— three consecutive `cargo test --workspace` runs gave 8, 4 and 2 failures out of 13. Committed
HEAD is green at **920 tests**; the in-flight file is not, and its flakiness is itself a defect
worth chasing before it lands.

---

## R1–R108

| R | Status | Evidence |
|---|---|---|
| R1 | Satisfied | No conversion path exists. `CalendarItem` is an enum (`caldir-core/src/item.rs:15`); `into_event()` (`item.rs:155`) returns `Err`, never a synthesized `Event`. Locked by `connection.rs:806` (T-48) asserting the on-disk file still holds `BEGIN:VTODO` and no request reached the transport. |
| R2 | **Partial** | Held for disk/filename/parse/equality/sort/text (C1,C2,C13 rows below). **Two bounded breaches:** (a) `caldir-provider-webcal/src/commands/list_events.rs:16` now routes the feed through `CalendarItem::from_ics_str`, which hard-errors on any unsupported component (`item.rs:247`) — a webcal feed carrying a `VJOURNAL` used to list its VEVENTs and now fails wholesale; (b) `fetch_events` issues a second REPORT per sync (`caldav/ops/list_events.rs:32-38`) whose failure propagates, so a server that rejects VTODO queries breaks a previously-working VEVENT sync (`docs/vtodo/BACKLOG.md:272` B-16, "VERIFIED: still undecided"). Both are mandated by R19/R58; neither is reconciled with R2 anywhere in the spec. |
| R3 | Satisfied | Kind-carrying identity (`event/instance_id.rs:24-28`); cross-calendar aggregation locked by `caldir::tests::aggregating_all_calendars_survives_a_vtodo_calendar` (`caldir.rs:424`). |
| R4 | Satisfied | `connection::tests::a_mixed_local_directory_keeps_both_items_distinct_end_to_end` (`connection.rs:1012`). |
| R5 | Satisfied | `calendar/event.rs:560` (file), `calendar.rs:1260` (sibling), `caldir.rs:424` (aggregation). |
| R6 | Satisfied | Shipped as PR-1 (`docs/vtodo/PR1-DESCRIPTION.md`), including R48 — `CalendarDiff::compute_from_listing` (`diff/calendar_diff.rs:20-37`). |
| R7 | Satisfied | Kind comes only from the parsed component (`item.rs:231-241`) and rides identity across disk, bases, wire. |
| R8 | Satisfied | `Todo` has no end/duration field (`todo.rs:30-79`); `todo::to_icalendar::tests::serializing_a_task_never_emits_dtend` (`todo/to_icalendar.rs:273`). |
| R9 | Satisfied | `start` and `due` independent `Option<EventTime>` (`todo.rs:36-39`); no synthesis anywhere. |
| R10 | Satisfied | `CalendarItem::load` arity rule (`item.rs:56-80`). |
| R11 | Satisfied | Identity derived from ICS content only; `event_url` is UID-derived (`caldav/client.rs:73-76`). |
| R12 | Satisfied | `#[educe(PartialEq(ignore))]` on `created`/`last_modified`/`sequence` (`todo.rs:71-78`); T-19 (`todo.rs:405`). |
| R13 | **Partial — spec/code contradiction** | The IN/OUT *set* is exactly as specified (`todo.rs:30-79`; T-17 at `todo.rs:656` proves set equality both ways). But the spec still types `PRIORITY` and `PERCENT-COMPLETE` as `Option<u8>` (`spec.md:184,187`) while the shipped fields are `Option<i32>` (`todo.rs:47,52`). QA defect F-4 widened them precisely because `u8` cannot hold `256`/`-1`; **spec.md was never amended**, so the requirement as written still contradicts both the code and its own "out-of-range MUST round-trip verbatim" clause. |
| R14 | Satisfied | Unknown STATUS dropped, loss pinned: `todo.rs:197` (T-14); parser at `todo/from_icalendar.rs:38-42`. |
| R15 | Satisfied | `dropped_property_names_are_exactly_the_documented_list` (`todo.rs:656`) asserts set equality in both directions, with a non-vacuity guard. |
| R16 | Satisfied | `specs/caldir.md:183` `## VTODO (Task)`; PRIORITY/CREATED scoped (`:337-338`), RELATED-TO row added (`:341`), stale RDATE row removed, Tasks filename subsection (`:361-366`). `git diff main...HEAD -- specs/caldir.md` removes exactly three lines, all in "Fields We Skip". |
| R17 | Satisfied | One ordered pass over `icalendar.components` (`item.rs:227-249`); no `.events()` call remains. |
| R18 | Satisfied | `IGNORED_COMPONENTS` in the shared parser (`item.rs:22`), used by disk, wire and base decode; `rpc/ics.rs:201` (T-10). |
| R19 | Satisfied | `_ => return Err(EventError::UnsupportedComponent(name))` (`item.rs:247`) — no skip arm; `event.rs:1508`, `rpc/ics.rs:227`. |
| R20 | Satisfied | No `MissingStart` mirror (`todo/from_icalendar.rs:16-27`). |
| R21 | Satisfied | Both go through `EventTime::from`; `todo/from_icalendar.rs:519` (T-13, TZ). |
| R22 | Satisfied | `raw_date_time` reads the raw property instead of `get_completed()` (`todo/from_icalendar.rs:143-146`). |
| R23 | Satisfied | `.filter(|(name, _)| name.starts_with("X-"))` (`todo/from_icalendar.rs:52-58`); nothing else enters a bag. |
| R24 | Satisfied | `splice_valarms_into_vtodo` on the literal `"END:VTODO\r\n"` (`todo.rs:163-171`); `todo.rs:365` (T-12). |
| R25 | Satisfied | No `dtstamp` field on `Todo`; `todo.rs:405` (T-19). |
| R26 | Satisfied | `todo/to_icalendar.rs:273`. |
| R27 | Satisfied | `Todo::to_ics_string` pushes exactly one component (`todo.rs:148-160`). |
| R28 | Satisfied | Crate `write_crlf!`; every fixture and assertion in `todo.rs` is `\r\n`. |
| R29 | Satisfied (Q-3 closed) | caldir emits no `VTIMEZONE` and tolerates Radicale adding one — measured by `live::radicale::get_pass_two_equals_get_pass_one` (`live/radicale.rs:875`) and recorded at `BACKLOG.md:456`. |
| R30 | Satisfied | T-53 compares parsed domain values (`live/radicale.rs:783-786`); T-55 compares GET-pass-2 against GET-pass-1 only. |
| R31 | Satisfied | No ETag appears in any acceptance assertion; `live/radicale.rs:866-869` states the reason. |
| R32 | Satisfied | `item.rs:56-81`; `calendar/event.rs:633` (T-8) walks all eight rows with the R75 wording asserted. |
| R33 | Satisfied | `Event::base_slug` untouched (`event/slugify.rs:13-19`); literal corpus at `event/slugify.rs:193` (T-6). |
| R34 | Satisfied | `todo/slugify.rs`; `event/slugify.rs:82` (T-7) covers all three rows plus both halves of the DUE→DTSTART date-source rule. Recurring-*instance* row added to `specs/caldir.md:366` per the adjudication. |
| R35 | Satisfied | T-7 is TZ-marked and CI runs three zones (`.github/workflows/ci.yml:36-42`). |
| R36 | Satisfied | `EventInstanceId { kind, uid, recurrence_id }` (`event/instance_id.rs:24-28`); hazards 1/3/5 locked at `known_event_ids.rs:134`, `calendar_diff.rs:1689`, `doctor/warning/duplicate_file.rs:142`. |
| R37 | Satisfied | `Display`/`From<&str>` at `event/instance_id.rs:89-131`; `VTODO:` sigil, event form unprefixed; T-20 at `event/instance_id.rs:296`. |
| R38 | Satisfied | `x_properties_eq`/`attachments_eq` moved to `event/equality.rs` and referenced by both types (`event.rs:28,63,66`; `todo.rs:11,63,66`). |
| R39 | Satisfied | All content fields participate; only `created`/`last_modified`/`sequence` are `ignore`d (`todo.rs:71-78`); `todo.rs:253` (T-18). |
| R40 | Satisfied | `update_direction` unchanged three-way-first (`calendar_diff.rs:275-301`). |
| R41 | Satisfied | `(Some(_), None) => true` kept (`calendar_diff.rs:312`); `calendar_diff.rs:1757` (T-32). |
| R42 | Satisfied | `is_cancelled()` is CANCELLED-only for both kinds (`item.rs:120-127`), used at both skip sites (`calendar_diff.rs:88`, `:174`); T-28 at `calendar_diff.rs:1638`. |
| R43 | Satisfied | `bases/<sha256(identity)>.ics` unchanged (`event_bases.rs:13,107`); T-23 at `event_bases.rs:270`, with independently-computed digests. |
| R44 | Satisfied | `Todo::occurs_in_range` (`todo.rs:126-140`) reproduces the four rows exactly; `COMPLETED`/`CREATED` are not consulted. T-24 at `calendar_diff.rs:1190`. |
| R45 | Satisfied | `git diff main...HEAD` shows no production change to `Event::occurs_in_range`/`has_occurrence_in_range`; only new test lines. |
| R46 | Satisfied | No synthesis site exists; `(None, None) => true` (`todo.rs:139`). |
| R47 | Satisfied | `!item.has_occurrence_in_range(..) => continue` before both the delete and the create (`calendar_diff.rs:132-136`). |
| R48 | Satisfied | `Deletes::Suppressed` when `local_listing.is_incomplete()` (`calendar_diff.rs:25-29`), guarding both the incoming (`:150`) and outgoing (`:181`) delete sites; T-29 at `calendar_diff.rs:1064`. |
| R49 | Satisfied | `remote_reports_tasks` computed once (`calendar_diff.rs:69-72`) and applied at `:141-149`; warning consumed by `caldir-cli/src/output/diff.rs:177,237`. T-30 at `calendar_diff.rs:1510`. |
| R50 | Satisfied | Accepted mode documented at `calendar_diff.rs:196-203`; suppression recorded only where a delete was actually withheld. |
| R51 | Satisfied | `ComponentFilter::AllTasks` emits `<C:comp-filter name="VTODO"/>` with no child (`caldav/ops/list_events.rs:127`). |
| R52 | Satisfied | `Method` unchanged; `rpc.rs:91` (T-33) pins the six verbs. |
| R53 | **Not satisfied (inbound half only)** | Inbound is item-typed: `ListEvents::Response = Vec<CalendarItem>` (`rpc/methods/list_events.rs:16`) and `ItemIcs` (`rpc/ics.rs:60`). **Outbound is not**: `CreateEvent`, `UpdateEvent` and `DeleteEvent` still carry `pub event: Event` (`rpc/methods/{create,update,delete}_event.rs:9`). R53's own consequence clause — "or a task can be neither created, updated, nor deleted" — is therefore the shipped behaviour. The refusal is verified sound (R70 forbids the provider-crate churn), but R53 as written is unmet and was never amended; T-36 (`rpc.rs:239`) pins the *opposite* of what R53 asks for. |
| R54 | Satisfied | `rpc/ics.rs` delegates to `CalendarItem::from_single_ics_str` (`item.rs:38`), VTIMEZONE exemption included; T-10/T-35. |
| R55 | Satisfied | `rpc.rs:140` and `rpc.rs:184` (T-34) — the latter greps the crate for `deny_unknown_fields`. |
| R56 | **Partial — stale mechanism** | The *outcome* is stronger than specified and safe. But R56 says the abort is the `?` at `connection.rs:231` producing `expected 1 event(s) in ICS, found 0`; after QA defect F-3 the refusal is a **whole-push pre-flight** raising `ConnectionError::UnpushableTask` before any RPC (`connection.rs:287-311`), and T-50 explicitly asserts `!message.contains("expected 1 event(s) in ICS")` (`connection.rs:910-913`). The spec's stated mechanism is now false. |
| R57 | Satisfied | One bad element fails the vector; `connection.rs:953` (T-51) proves nothing is applied, and `rpc/methods/list_events.rs:117` proves the component is named. |
| R58 | Satisfied | Bare comp-filter (`caldav/ops/list_events.rs:104-127`); T-54's request half (`live/radicale.rs:813-836`) runs with no server and fails the moment a time-range returns. |
| R59 | Satisfied | Two REPORTs (`caldav/ops/list_events.rs:66-79`); `list_events.rs:415` (T-38). |
| R60 | Satisfied | `find_event_by_uid_body(uid, kind)` (`caldav/ops.rs:229`), no time-range. |
| R61 | Satisfied | `location_at_href` verifies the decoded kind before returning the href (`caldav/ops.rs:172-194`); T-39 at `caldav/ops.rs:460`. |
| R62 | Satisfied | `event_url` unchanged and kind-free (`caldav/client.rs:73-76`). |
| R63 | Satisfied | `create_event.rs:75` falls back to update on `409` + `<C:no-uid-conflict/>`; T-40 asserts the outcome, not the code, using the verbatim probe body (`create_event.rs:175`). |
| R64 | Satisfied | `put_ics` is the crate's only PUT and calls `ensure_single_component` (`caldav/ops.rs:47-95`); T-41 at `caldav/ops.rs:671`. |
| R65 | Satisfied | `ConnectionError::SameUidPair` from the pre-flight (`connection/error.rs:24`, raised `connection.rs:291`); live gate `live/radicale.rs:948` (T-56). |
| R66 | Satisfied | `find_map(Result::ok)` gone; `parse_item` is loud (`caldav/ops.rs:43`). |
| R67 | Satisfied | `live/radicale.rs:1018` (T-57). |
| R68 | Satisfied | Neither field is derived from the other (`todo/from_icalendar.rs:95-99`). |
| R69 | Satisfied (as a disclosure) | Stated, not claimed: `ADJUDICATION.md:805-810`; website says the push limitation once (`website/src/content/docs/commands.md:132`). |
| R70 | **Partial breach** | No *mapping* work shipped for Google/Outlook, and neither can carry a task. But "**No provider-crate edits ship in this feature**" is false as shipped: `caldir-provider-google/src/main.rs`, `-outlook/src/main.rs`, `-icloud/src/main.rs` and `-webcal/src/{main.rs,commands/list_events.rs}` were all edited to satisfy the new `Handler::list_events -> Vec<CalendarItem>` signature. Recorded in the adjudication as a "bounded R70 breach"; **spec.md was never amended**. |
| R71 | **Partial — stale scoping** | The accepted consequence is real but is not what R71 says. The refusal is provider-agnostic — it fires for **CalDAV too**, not only Google/Outlook (`item.rs:155`, `connection.rs:287`) — and it no longer "aborts the rest of that calendar's push": since F-3 it refuses the whole calendar's push *before the first RPC*, leaving the other pending changes queued. Better behaviour, unamended requirement. |
| R72 | Satisfied | Webcal still drops tasks (`webcal/src/commands/list_events.rs:20-26`) and is covered by R49. Note a **stale `TODO(L3-04)`** on that arm: L3-04 shipped and does not own it. |
| R73 | Satisfied | No config field, builder, third bucket or capability renderer exists — verified by grep across `caldir-core/src/calendar/config.rs` (no diff) and `diff/calendar_diff.rs`. |
| R74 | Satisfied | A fixed by degradation + R48; B fixed by R19 + R49. `caldav/ops/list_events.rs:43-57` refuses a partial listing. |
| R75 | Satisfied | `NoSupportedComponent` vs `ExpectedSingleComponent` (`item.rs:71-79`); exact wording asserted in T-8 (`calendar/event.rs:659-682`). |
| R76 | Satisfied | `from_single_ics_str` reuses `UnexpectedComponentCount` with no "none" variant (`item.rs:38-48`). |
| R77 | Satisfied | `skipped_warnings` maps every skip to `UnreadableEvents` (`doctor/warning.rs:60-63`); `doctor.rs:83` (T-43). |
| R78 | Satisfied | `CalendarListing` carries `skipped` + `is_incomplete()` (`calendar/listing.rs:29-68`). Q-6 answered by the listing struct. |
| R79 | Satisfied | See R63. |
| R80 | Satisfied | See R64 — structural, single PUT site. |
| R81 | Satisfied | Single-file mode fails with the exact sentence (`commands/rsvp.rs:57-60`), writes nothing; list mode narrows via `ce.event()` (`rsvp.rs:110`); `invites.rs:104` narrows too. T-42 at `rsvp.rs:272`. |
| R82 | Satisfied | Each C row is evidenced below at its stated tier. |
| R83 | Satisfied | `website/src/content/docs/commands.md:198-255`, with a full task example object and the migration predicate. |
| R84 | Satisfied | `anchor()` = DUE→DTSTART (`output/agenda.rs:93-101`); `kind_ordinal` puts the event first (`:107-115`); undated sort last and get no invented date (`:129-139`). T-46 (`agenda.rs:351`), T-47 (`agenda.rs:811`). |
| R85 | Satisfied | `sort_by` is stable, no identity tiebreak added — stated and held at `output/agenda.rs:124-140`. |
| R86 | Satisfied | Non-goal; no tasks API touched in either provider. |
| R87 | Satisfied | Not modelled, dropped, loss pinned by `todo.rs:727` (T-16); spec §6's row was corrected in `bd64f4e`. |
| R88 | Satisfied | `CATEGORIES` OUT; mechanism note carried in T-17's comment (`todo.rs:651-654`). |
| R89 | Satisfied | `recurrence` modelled, never expanded (`todo.rs:54`); undated recurring task is in-window by R44 row 4. |
| R90 | Satisfied | `commands/new.rs:116` builds an `Event`; `new.rs:438` locks that it loads back as an Event. |
| R91 | Satisfied | ADR-07 absent in whole — no capability config, builder, provider edit for capability purposes, third bucket or pluralized renderer. |
| R92 | Satisfied | Recorded only, in `BACKLOG.md` and `specs`/spec §16. No code reads `supported-calendar-component-set`. |
| R93 | Satisfied | `CalendarItem` is `#[non_exhaustive]` (`item.rs:14`), `ComponentKind` likewise (`instance_id.rs:15`); adding VJournal is one variant + one arm. |
| R94 | Satisfied | `EventInstanceId`, `EventChange`, `event_bases` all keep their names. |
| R95 (A-1) | **Partial — INVIOLABLE not met** | Clauses 1, 3, 4, 5 hold. **Clause 2 is not achieved by caldir.** T-53's own doc comment says it: "The two PUTs are issued by this harness… a task cannot reach a PUT through production code at all" (`live/radicale.rs:726-733`). A-1's premise — "when caldir loads it **and pushes it** to a generic CalDAV server" — is unreachable under R70/R53, so the mandatory scenario is proven for the *pull* half only. This is disclosed honestly everywhere except in R95 itself, which still reads as satisfied. |
| R96 (A-2) | Satisfied | `connection.rs:1551` (T-49), driven from raw ICS via `reply_raw_data` (`connection.rs:1570`) exactly as the adjudication requires. |
| R97 (A-3) | Satisfied | `calendar_diff.rs:1415` (T-25) plus the live half at `live/radicale.rs:813` (T-54). |
| R98 (A-4) | Satisfied | `calendar_diff.rs:1442` (T-26). |
| R99 (A-5) | Satisfied | `calendar_diff.rs:1469` (T-27). |
| R100 (A-6) | Satisfied | `connection.rs:1012` (T-52) and `calendar_diff.rs:1689` (T-31); no doctor duplicate (`duplicate_file.rs:142`). |
| R101 (A-7) | Satisfied | T-8 row 6 (`calendar/event.rs:693-701`). |
| R102 (A-8) | Satisfied | `rpc/ics.rs:201` (T-10). |
| R103 (A-9) | **Partial by design** | Measured on the VEVENT control only (`live/radicale.rs:1117`, T-58); the task property transfers "by construction" from `event_url` being UID-derived (`caldav/client.rs:73-76`). The refusal is sound and the spec already says so; recording it here so the acceptance table does not read it as measured. The adjudication's own follow-up stands: no unit lock proves `event_url` is purely UID-derived. |
| R104 (A-10) | **Partial** | 920 tests green at HEAD across the committed tree. But "zero edits to existing event tests" has one exception: `caldav::ops::tests::parse_event_returns_none_on_invalid_ics` was **renamed and inverted** to `parse_event_errors_on_invalid_ics` (`caldav/ops.rs:352`). Forced by R66, which mandates the contract change — bounded and justified, but it is an edit to a pre-existing test and R104 admits none. |
| R105 | Satisfied | All 58 named tests exist at the pinned module paths — see §18 audit. |
| R106 | Satisfied | `.github/workflows/ci.yml:36-42` runs `UTC`, `Europe/Stockholm`, `America/New_York` with `fail-fast: false`. QA found two tests failing outside that matrix (UTC+13/-11); both fixed in `5a2f756`. |
| R107 | **Partial** | Skippable and documented: `live_gate` prints a SKIP line and returns (`live/radicale.rs:144-154`); bcrypt recipe embedded (`:166-173`). **The image is not pinned**: `const IMAGE = "tomsquest/docker-radicale:latest"` (`live/radicale.rs:161`) with the comment "Pinned by tag", while the probe measured digest `sha256:e2ef8624b215…`. A tag is not a pin; the live tier can silently start testing a different Radicale. |
| R108 | Satisfied (as a rule) | No new probe crate ships. The rule is recorded (`ADJUDICATION.md` C-3); the live tier prints its own inputs. |

**Counts: 92 fully satisfied · 11 partial · 1 not satisfied (R53) · 0 superseded-and-unhandled.**

---

## C1–C13

| C | Tier | Status | Evidence |
|---|---|---|---|
| C1 | INVIOLABLE | Satisfied | `Event::base_slug` untouched (`event/slugify.rs:13-19`); literal-expectation corpus `base_slug_is_unchanged_for_vevent_corpus` (`event/slugify.rs:193`), which can disagree with the code by construction. |
| C2 | INVIOLABLE | Satisfied | `vevent_control_round_trips_identically_after_vtodo_support` (`event.rs:1368`); event equality helpers only *moved* (`event/equality.rs`), semantics unchanged. |
| C3 | INVIOLABLE | Satisfied | `git diff main...HEAD -- caldir-core/src/calendar/config.rs caldir-core/src/caldir/config.rs` is empty. No key added, renamed or retyped; no `deny_unknown_fields`. |
| C4 (amended) | INVIOLABLE for the event form | Satisfied | Event lines unprefixed (`instance_id.rs:89-108`), one id per line, sorted (`known_event_ids.rs:64-72`); both halves locked at `known_event_ids.rs:134`, and the downgrade-inertness half at `calendar_diff.rs:334`. |
| C5 | INVIOLABLE | Satisfied | `else { continue }` skip preserved (`event_bases.rs:42,61,71`); hash scheme unchanged (`event_bases.rs:107-109`); T-23 (`event_bases.rs:270`). |
| C6 | INVIOLABLE | Satisfied | An old caldir hitting a VTODO file gets `ExpectedSingleEvent { found: 0 }` — a hard error before any write. Unchanged by this branch (that is the failure §3 documents). |
| C7 | INVIOLABLE | Satisfied | R48 — `Deletes::Suppressed` guards both delete sites (`calendar_diff.rs:25-29,150,181`). |
| C8 | INVIOLABLE | Satisfied | `rpc.rs:91` (T-33). |
| C9 | INVIOLABLE | Satisfied | `rpc.rs:140` + `rpc.rs:184` (T-34). |
| C10 | INVIOLABLE | Satisfied | `item.rs:247`; `rpc/ics.rs:227` (T-35). |
| C11 | SOFT | Satisfied | 0.14.1 → 0.15.0 with all dependant pins updated (e.g. `caldir-provider-webcal/Cargo.toml`). The adjudication's seventh pin class (`caldir-provider-icloud/Cargo.toml:21`, a cross-provider pin) was found and fixed; `.agents/skills/bump/SKILL.md` still does not cover that class. |
| C12 (amended) | INVIOLABLE as amended | **Code satisfied; spec row is unsatisfiable** | The shipped schema meets the honest guarantee: five keys appended in order, `start`/`availability` null only on `kind == "todo"`, `status` not retyped (`output/agenda_json.rs:641-645,901-920`); T-44 at `agenda_json.rs:826`; predicate documented at `commands.md:250`. **But `spec.md:656` still says "Every `kind == "event"` entry stays byte-identical"** — which the adjudication itself ruled arithmetically impossible and ordered replaced by the `del(...)` predicate. Commit `e6cd79c` recorded the correction **in ADJUDICATION.md only**; spec.md was never touched. |
| C13 | INVIOLABLE | **Partial** | Event-only rendering byte-identical (`agenda_text.rs:709`, T-45); doctor still reports a broken VEVENT (`doctor.rs:83`, T-43); no sort determinism added (R85). Same two bounded breaches as R2: a webcal feed carrying an unsupported component now errors where it previously listed its events, and a CalDAV server that rejects the new VTODO REPORT turns a working sync into an error (BACKLOG B-16, undecided). |

---

## §18 test-matrix audit

**All 58 named tests exist, at the pinned module path.** Verified by `grep -rn "fn <name>"` across
the workspace; every hit landed in the module the spec names (`todo::tests`, `todo::to_icalendar::tests`,
`event::slugify::tests`, `calendar::state::*::tests`, `diff::calendar_diff::tests`, `rpc::tests`,
`rpc::ics::tests`, `caldav::ops::*::tests`, `commands::*::tests`, `output::agenda*::tests`,
`connection::tests`, `live::radicale::*`). T-59's baseline is green at 920.

The acceptance gate's two finds (T-36, T-47) are genuinely resolved:
- **T-36** → `rpc::tests::a_task_payload_is_rejected_by_every_outbound_request_struct` (`rpc.rs:239`).
- **T-47** → `output::agenda::tests::an_undated_task_is_absent_from_the_body_counted_in_the_footer_and_present_in_json` (`agenda.rs:811`).

Three rows the gate did **not** catch, where the test exists but the *spec row describing it is now false*:

1. **T-48** (`spec.md`) requires the test to assert "the byte-exact literal `expected 1 event(s) in ICS, found 0`".
   The shipped test asserts `ConnectionError::UnpushableTask` (`connection.rs:817`). Its sibling **T-50**
   goes further and asserts the literal is **absent**: `assert!(!message.contains("expected 1 event(s) in ICS"))`
   (`connection.rs:910-913`). The spec row and the test now state opposites.
2. **T-6** is specified as "~12 pairs"; the corpus at `event/slugify.rs:193` is 10 entries plus two
   zone-derived shapes. Within "~", noted for completeness.
3. **T-53/T-58 live tier** is gated on `docker` + `curl` and is skipped in a default `just test`
   (`live/radicale.rs:144-154`). It was **not executed** in this review — no Docker gate was set.
   Every live-tier row below is therefore audited by reading the test, not by running it.

---

## §17 acceptance-scenario audit

| A | Verdict |
|---|---|
| A-1 (R95) | **Partial, INVIOLABLE.** 1/3/4/5 hold. Clause 2 — the server receives a VTODO *from caldir* — is unreachable: the PUTs in T-53 are issued by the test harness (`live/radicale.rs:726-733`, `:747`, `:775`). Pull-half proven; push-half not implemented. |
| A-2 (R96) | Satisfied. `connection.rs:1551`, raw-fixture remote (`:1570`). |
| A-3 (R97) | Satisfied. Unit `calendar_diff.rs:1415` + live/request-half `live/radicale.rs:813-836`. The request-half assertion runs without Docker, so the §9.9 lock is live in normal CI. |
| A-4 (R98) | Satisfied. `calendar_diff.rs:1442`. |
| A-5 (R99) | Satisfied. `calendar_diff.rs:1469` — absence in a *task-carrying* response still deletes. |
| A-6 (R100) | Satisfied. `connection.rs:1012` + `calendar_diff.rs:1689`; no doctor false positive (`duplicate_file.rs:142`). |
| A-7 (R101) | Satisfied. `calendar/event.rs:693`. |
| A-8 (R102) | Satisfied. `rpc/ics.rs:201`. |
| A-9 (R103) | **Partial by design.** VEVENT control only; task property by construction. Sound refusal, correctly disclosed; no unit lock on `event_url` being purely UID-derived (the adjudication's own open item). |
| A-10 (R104) | **Partial.** Green in all three CI zones with one edited pre-existing test (`caldav/ops.rs:352`), forced by R66. |

---

## Findings

### F-1 — R53 is not satisfied; the requirement and R70 contradict each other (**highest**)
`CreateEvent`/`UpdateEvent`/`DeleteEvent` still carry `pub event: Event`
(`rpc/methods/create_event.rs:9` and siblings). R53 says these "MUST become item-typed **or a task can
be neither created, updated, nor deleted**" — and that is exactly what shipped. Making them item-typed
would require the provider-crate edits R70 forbids; the two requirements cannot both be met.
The refusal (ADJUDICATION, T-36) is **sound engineering** — an item-typed `Ics` would have made
`serde_json::from_str::<Ics>(VTODO)` succeed and killed the tripwire. But the spec still carries R53
unamended, so the acceptance table would otherwise round it up.

### F-2 — A-1/R95, an INVIOLABLE scenario, is met only for the pull half
Clause 2 requires caldir to push the fixture to a CalDAV server. No production path can (`item.rs:155`).
T-53's PUTs come from the harness and its doc comment says so. The disclosure is honest and repeated
(`ADJUDICATION.md:805-810`, `commands.md:132`), but **R95 itself was never amended**, and it is marked
INVIOLABLE. Ship reporting must not present A-1 as fully met.

### F-3 — Spec text still contradicts shipped types (R13) and shipped guarantees (C12)
Both were identified during implementation and both corrections were written **only into
ADJUDICATION.md**:
- `spec.md:184,187` types PRIORITY/PERCENT-COMPLETE as `Option<u8>`; `todo.rs:47,52` ships `Option<i32>`
  (QA F-4). The `u8` typing is what made "out-of-range MUST round-trip verbatim" self-contradictory.
- `spec.md:656` still claims every event entry stays "byte-identical"; commit `e6cd79c` proved that
  arithmetically impossible and substituted the `select(.kind=="event") | del(...)` predicate — in the
  adjudication only. The website docs carry the correct predicate (`commands.md:250`); the spec does not.

### F-4 — R56/R71 describe a mechanism the code no longer has, and §18's T-48/T-50 rows now state the opposite of the tests
QA defect F-3 replaced the mid-loop `?` abort with a whole-push pre-flight
(`connection.rs:287-311`, `connection/error.rs:33-49`). The behaviour is strictly better — nothing is
half-pushed and the other pending changes survive. But R56 still cites `connection.rs:231`, R71 still
scopes the abort to Google/Outlook (it fires for CalDAV too), and §18's T-48 row demands a byte-exact
literal that T-50 now asserts is **absent**.

### F-5 — R70's "No provider-crate edits ship in this feature" is false as shipped
Four provider crates were edited (`google/src/main.rs`, `outlook/src/main.rs`, `icloud/src/main.rs`,
`webcal/src/main.rs` + `webcal/src/commands/list_events.rs`) to satisfy the new
`Handler::list_events -> Vec<CalendarItem>` signature. Bounded and mechanical; recorded in the
adjudication; unamended in the spec.

### F-6 — Two R2/C13 breaches that no requirement reconciles
Both are consequences of INVIOLABLE requirements, and neither is discussed as an R2 exception anywhere:
- A webcal feed containing a `VJOURNAL` (or any unsupported component) now fails wholesale
  (`webcal/src/commands/list_events.rs:16` → `item.rs:247`), where it previously listed its VEVENTs.
- The second VTODO REPORT propagates its failure (`caldav/ops/list_events.rs:32-38`), so a CalDAV server
  that rejects VTODO queries breaks a previously-working VEVENT sync. `BACKLOG.md:272` marks this
  "**VERIFIED: still undecided**" — an open, user-visible regression risk shipping undecided.

### F-7 — R107's container is not pinned
`const IMAGE: &str = "tomsquest/docker-radicale:latest"` (`live/radicale.rs:161`), commented "Pinned by
tag", while the probe measured digest `sha256:e2ef8624b215…`. The live tier can silently change server
version under a reproducibility requirement. One-line fix.

### F-8 — Two modelled properties have no `--json` surface
`priority` and `created` are destructured as `_` in the task arm (`output/agenda_json.rs:251,261`).
ADR-10 D6 says PRIORITY "rides whatever surface ADR-04's property set gives it, or gains a key". It has
none. Flagged as owed by the adjudication and still open at ship.

### F-9 — Working tree is dirty and its untracked test target is flaky
`caldir-cli/tests/sync_exit_status.rs` (untracked) + `commands/sync.rs` (modified) fail
non-deterministically: 8, then 4, then 2 failures of 13 across three consecutive `cargo test --workspace`
runs. Whatever the underlying `sync`/`discard` exit-code fix is worth, a test target whose failure count
varies run to run is not a gate.

### F-10 — Stale `TODO(L3-04)` in shipped code
`webcal/src/commands/list_events.rs:21-25` attributes the silent VTODO drop to a ticket that shipped and
does not own it. R72 rules the drop out of scope; the comment should cite R72/R49.

### Refusals judged
All eight refusals-with-verification are **sound**, and I re-derived each rather than accepting the
record: the recurring-instance filename row (`Event::base_slug` keys on `recurrence.is_some()`,
`event/slugify.rs:14`); T-26's reassignment (now present at `calendar_diff.rs:1442`); L4-01's AC 1
(`WireValue::from_wire` infallibility — the two-newtype split at `rpc/ics.rs:33,60` is the correct
resolution); T-48's name; the 409 rename-premise correction (`event_url` is UID-derived,
`caldav/client.rs:73-76`); L6-01's unreachable error-envelope AC (the pre-flight aborts before any RPC);
T-58's name; and "RELATED-TO preserved verbatim" (`Todo` has no such field; T-17 pins the drop).
**No unsound refusal found.** What the refusals do produce is F-1 and F-2: two spec requirements left
standing that the shipped design cannot meet.
