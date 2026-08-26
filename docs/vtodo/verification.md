# VTODO support — ship verification

**Branch:** `vtodo-support` · **HEAD:** `7a4444f` · **Base:** `main` (`2ed74fa`) · **69 commits**
**Working tree:** clean (`git status --porcelain` empty).

Every row below was checked against the tree at `7a4444f`. Where a claim came from one of the
five reviews or three QA sweeps — all of which were written at `5a2f756` or earlier, four
commits behind HEAD — it was re-derived here rather than carried forward. Four review findings
are recorded as **closed by re-verification**; three are recorded as **still open** despite
having been reported as fixable; one requirement (**R19**) is **downgraded** from the spec
review's verdict, because the fix for a real regression deliberately departed from its text.

A green suite is not evidence in this document. Independent QA found seven user-visible defects
after 906 tests were green; the five reviewers found four more after that. Each row cites a
file, a test, or a command that was run.

---

## 1. Acceptance table

### Result rules

- **PASS** — the requirement as written is met, with evidence.
- **PARTIAL** — some of it is met. The row states exactly what is missing. Nothing is rounded up.
- **FAIL** — not met, and not deliberately.
- **REFUSED** — the shipped behaviour deliberately departs from the requirement text, the
  departure was verified before it was accepted, and §1.4 gives the reasoning. Only rows where
  a refusal is the reason the requirement text is not literally met are marked REFUSED; the
  other six refusals corrected *ticket* text rather than a requirement, and their rows are PASS.

### 1.1 R1 – R108

| Requirement | Evidence | Result |
|---|---|---|
| R1 — no VTODO→VEVENT conversion path | `CalendarItem` is an enum (`caldir-core/src/item.rs:15`); `into_event()` (`item.rs:176`) returns `Err`, never a synthesized `Event`. Locked at `connection.rs` (T-48): the on-disk file still holds `BEGIN:VTODO` and no request reached the transport. | **PASS** |
| R2 — no VEVENT behaviour change | Held for disk bytes, filenames, parse, equality, sort and text output (C1/C2/C13 below, and §5). **One breach remains:** `fetch_events` issues a second REPORT per sync (`caldir-provider-caldav/src/caldav/ops/list_events.rs:32-38`) whose failure `?`-propagates, so a CalDAV server that rejects a bare `VTODO` comp-filter turns a previously-working VEVENT sync into an error. No test covers a server that rejects it. `BACKLOG.md:272` (B-16) still reads "VERIFIED: still undecided". The spec review's *second* R2 breach — a webcal feed carrying a `VJOURNAL` failing wholesale — is **closed**, see R19. | **PARTIAL** — the CalDAV second-REPORT failure surface is unreconciled and undecided |
| R3 — kind-carrying identity survives aggregation | `event/instance_id.rs:24-28`; `caldir::tests::aggregating_all_calendars_survives_a_vtodo_calendar`. | **PASS** |
| R4 — a mixed local directory keeps both kinds distinct | `connection::tests::a_mixed_local_directory_keeps_both_items_distinct_end_to_end`. | **PASS** |
| R5 — task beside event at file, sibling and aggregation level | `calendar/event.rs`, `calendar.rs`, `caldir.rs` tests. | **PASS** |
| R6 — graceful degradation on unreadable `.ics` | Shipped as the separately-described PR-1 (`docs/vtodo/PR1-DESCRIPTION.md`), including R48 via `CalendarDiff::compute_from_listing` (`diff/calendar_diff.rs`). | **PASS** |
| R7 — kind comes only from the parsed component | `item.rs:241-268`; rides identity across disk, bases and wire. | **PASS** |
| R8 — no `DTEND`/duration on a task | `Todo` has no such field (`todo.rs:30-79`); `todo::to_icalendar::tests::serializing_a_task_never_emits_dtend`. | **PASS** |
| R9 — `DTSTART` and `DUE` independent | Both `Option<EventTime>` (`todo.rs:36-39`); no synthesis site exists. | **PASS** |
| R10 — arity rule on load | `CalendarItem::load` / `load_with_ignored` (`item.rs:56-100`). | **PASS** |
| R11 — identity from ICS content only | `event_url` is UID-derived (`caldir-provider-caldav/src/caldav/client.rs:73-76`). | **PASS** |
| R12 — `created`/`last_modified`/`sequence` out of `==` | `#[educe(PartialEq(ignore))]` (`todo.rs:71-78`); T-19. | **PASS** |
| R13 — the IN/OUT property set | The *set* is exactly as specified, proven both ways with a non-vacuity guard by `dropped_property_names_are_exactly_the_documented_list` (`todo.rs`). **But `spec.md:184,187` still types `PRIORITY` and `PERCENT-COMPLETE` as `Option<u8>` while the shipped fields are `Option<i32>` (`todo.rs:47,52`).** QA defect F-4 widened them precisely because `u8` cannot hold `256`/`-1`, which is what made the spec's own "out-of-range MUST round-trip verbatim" clause self-contradictory. The correction was written into `adr/ADJUDICATION.md` only; `spec.md` was never amended. | **PARTIAL** — spec text contradicts the shipped type |
| R14 — unknown `STATUS` dropped, loss pinned | `todo/from_icalendar.rs:38-42`; T-14. | **PASS** |
| R15 — the dropped-property list is exact | Set equality asserted in both directions (`todo.rs`). | **PASS** |
| R16 — `specs/caldir.md` documents VTODO | `specs/caldir.md:183` `## VTODO (Task)`; PRIORITY/CREATED scoped; RELATED-TO row added; stale RDATE row removed; Tasks filename subsection added. `git diff main...HEAD -- specs/caldir.md` removes exactly three lines, all under "Fields We Skip". | **PASS** |
| R17 — one ordered parse pass | `item.rs:241-269` is a single pass over `icalendar.components`; no `.events()` call remains anywhere. | **PASS** |
| R18 — `VTIMEZONE` exempt from counting, on disk *and* wire | `const IGNORED_COMPONENTS: [&str; 1] = ["VTIMEZONE"]` (`item.rs:22`), used by disk, wire and base decode; `item::tests::a_vtimezone_sibling_does_not_count_towards_arity`, `a_vtimezone_is_not_reported_as_an_ignored_sibling`, `calendar::event::tests::a_vtimezone_only_file_is_none_not_unsupported`. | **PASS** |
| R19 (INVIOLABLE) — an unsupported data-bearing component MUST produce a hard error naming it | **Not as written.** `parse_document` (`item.rs:266-268`) now *records* an unsupported sibling into `parsed.ignored` and continues; the hard error fires only when **every** component is unsupported (`item.rs:275-277`). This was a deliberate change: as written, R19 made a legal `.ics` carrying a `VEVENT` beside a `VFREEBUSY`, `VJOURNAL`, stray `VALARM` or top-level `X-` component **lose its event** from every view and from sync — a straight regression from `main`, which ignored such siblings for years (compat review B1, measured end-to-end: `sync old → pushed 'Case'? 1`, `sync new → pushed 'Case'? 0`). The property R19 exists to protect — *a document never silently yields nothing*, because nothing is what feeds the delete path — is preserved and locked (`item::tests::an_unsupported_component_aborts_rather_than_shortening_the_vec`, `a_vvenue_is_unsupported_and_named`, `rpc::ics::tests::decoding_an_unknown_component_errors_rather_than_dropping`). The drop is **not silent**: ignored names are returned by `load_with_ignored` and rendered as `DoctorWarning::IgnoredComponents` (`caldir-cli/src/commands/doctor/warning.rs:90-99`). Sibling tolerance is locked by `one_supported_component_still_loads_beside_unsupported_siblings`, `two_supported_components_are_still_an_error_beside_a_sibling`, `an_ignored_sibling_is_reported_to_the_caller`, and `caldir-cli/tests/agenda_exit_status.rs:140`. **`spec.md:230` was never amended.** | **PARTIAL** — the guarantee is "never yields nothing", not "hard error naming the component"; the requirement text is stale |
| R20 — a task must not require `DTSTART` | No `MissingStart` mirror (`todo/from_icalendar.rs:16-27`). | **PASS** |
| R21 — `DUE;TZID=` round-trips with TZID intact | Both times go through `EventTime::from`; T-13, TZ-marked. | **PASS** |
| R22 — raw `COMPLETED`, not `get_completed()` | `raw_date_time` reads the raw property (`todo/from_icalendar.rs:143-146`). | **PASS** |
| R23 — only `X-` properties enter the bag | `.filter(\|(name, _)\| name.starts_with("X-"))` (`todo/from_icalendar.rs:52-58`). | **PASS** |
| R24 — hand-rolled VALARM splice into VTODO | `splice_valarms_into_vtodo` on the literal `"END:VTODO\r\n"` (`todo.rs:163-171`); T-12. | **PASS** |
| R25 — `DTSTAMP` re-stamped, never modelled | No `dtstamp` field on `Todo`; T-19. | **PASS** |
| R26 — no `DTEND` emitted | `todo/to_icalendar.rs`. | **PASS** |
| R27 — exactly one component per document | `Todo::to_ics_string` pushes one component (`todo.rs:148-160`). | **PASS** |
| R28 — CRLF throughout | Crate `write_crlf!`; every fixture and assertion in `todo.rs` is `\r\n`. | **PASS** |
| R29 — caldir emits no `VTIMEZONE`, tolerates a server adding one | Measured against real Radicale 3.7.6: `live::radicale::get_pass_two_equals_get_pass_one`, **executed** (see §2.3). Q-3 closed by measurement. | **PASS** |
| R30 — "sync twice" compares parsed domain values, never raw bytes | T-53 compares parsed values; T-55 compares GET-pass-2 against GET-pass-1. | **PASS** |
| R31 — ETag never used as proof of a no-op | No ETag appears in any acceptance assertion; the reason is written at the site. | **PASS** |
| R32 — one supported component per local file | `item.rs:56-100`; T-8 walks all eight rows with the R75 wording asserted. | **PASS** |
| R33 — `Event::base_slug` unchanged | `event/slugify.rs:13-19`; literal-expectation corpus at `event/slugify.rs:193`, which can disagree with the code by construction. | **PASS** |
| R34 — task filename rule | `todo/slugify.rs`; T-7 covers all three rows plus both halves of the `DUE`→`DTSTART` date-source rule. The recurring-*instance* row was refused as unreachable and the correction landed in `specs/caldir.md:366` (refusal 1, §1.4). | **PASS** |
| R35 — filename rules TZ-robust | T-7 is TZ-marked; CI runs three zones (`.github/workflows/ci.yml:36-42`); re-run here in all three (§2). | **PASS** |
| R36 — identity carries kind | `EventInstanceId { kind, uid, recurrence_id }` (`event/instance_id.rs:24-28`); the three collision hazards locked in `known_event_ids.rs`, `calendar_diff.rs`, `doctor/warning/duplicate_file.rs`. | **PASS** |
| R37 — `VTODO:` sigil, event form unprefixed | `Display`/`From<&str>` at `event/instance_id.rs:89-131`, read here directly; T-20. | **PASS** |
| R38 — equality helpers shared, not duplicated | `x_properties_eq`/`attachments_eq` in `event/equality.rs`, referenced by both types. | **PASS** |
| R39 — all content fields participate in `==` | `todo.rs:71-78`; and since `7a4444f` locked structurally by `todo::tests::every_content_field_participates_in_equality` and its `Event` twin (`event.rs:1580`) — exhaustive destructures with no `..`, so adding a field is a compile error until someone decides which side of equality it belongs on. | **PASS** |
| R40 — `update_direction` unchanged, three-way first | `calendar_diff.rs:319-345`. | **PASS** |
| R41 — `(Some(_), None) => true` kept | `calendar_diff.rs`; T-32. | **PASS** |
| R42 — `is_cancelled()` is CANCELLED-only for both kinds | `item.rs:143-151`, used at both skip sites; T-28. | **PASS** |
| R43 — `bases/<sha256(identity)>.ics` unchanged for events | `event_bases.rs:107`; `event_base_filenames_are_unchanged_and_a_task_never_shares_one` uses independently-computed digest literals. | **PASS** |
| R44 — the task window rule | `Todo::occurs_in_range` (`todo.rs:126-140`) reproduces the four rows exactly; `COMPLETED`/`CREATED` are not consulted. T-24. | **PASS** |
| R45 — `Event::occurs_in_range` untouched | `git diff main...HEAD` shows no production change; only new test lines. | **PASS** |
| R46 — no window synthesis | `(None, None) => true` (`todo.rs:139`); no synthesis site exists. | **PASS** |
| R47 — out-of-window items are neither deleted nor created | `!item.has_occurrence_in_range(..) => continue` before both sites (`calendar_diff.rs`). | **PASS** |
| R48 — an incomplete listing emits zero deletes | `Deletes::Suppressed` when `local_listing.is_incomplete()` (`calendar_diff.rs:25-29`), guarding both the incoming and the outgoing delete site; T-29. | **PASS** |
| R49 — a task-less remote response never deletes tasks | `remote_reports_tasks` computed once (`calendar_diff.rs:69-72`); warning consumed at `caldir-cli/src/output/diff.rs`. Live-verified by QA against real Radicale: clearing every VTODO server-side printed the withhold warning and kept all three local files. T-30. | **PASS** |
| R50 — the suppression is observable, never silent | Recorded only where a delete was actually withheld; rendered by `status`, `sync` and `pull`. | **PASS** |
| R51 — `AllTasks` emits a bare comp-filter | `<C:comp-filter name="VTODO"/>` with no child (`caldav/ops/list_events.rs:127`). | **PASS** |
| R52 (C8) — `Method` gains no variant | `rpc.rs` T-33 pins the six verbs. | **PASS** |
| R53 — the RPC method structs MUST become item-typed, **or** a task can be neither created, updated nor deleted | Inbound **is** item-typed: `ListEvents::Response = Vec<CalendarItem>` (`rpc/methods/list_events.rs:16`) with an `ItemIcs` newtype (`rpc/ics.rs:60`). Outbound is **not**: `CreateEvent`, `UpdateEvent` and `DeleteEvent` still carry `pub event: Event` (verified by grep at `rpc/methods/{create,update,delete}_event.rs:10`). R53's own second disjunct is therefore the shipped behaviour, and T-36 (`rpc::tests::a_task_payload_is_rejected_by_every_outbound_request_struct`) pins it. Retyping would require the ~25 `cmd.event` provider sites R70 forbids touching; an item-typed `Ics` would also have made `serde_json::from_str::<Ics>(VTODO)` succeed and killed the R57 tripwire. | **REFUSED** — reasoning in §1.4 (refusal 3) |
| R54 — wire decoder applies the disk rules incl. VTIMEZONE | `rpc/ics.rs` delegates to `CalendarItem::from_single_ics_str`; T-10/T-35. | **PASS** |
| R55 (C9) — RPC fields additive `Option`, no `deny_unknown_fields` | T-34 greps the crate for `deny_unknown_fields`. | **PASS** |
| R56 — new core + old provider MUST be safe | **Outcome met, and now better than specified.** R56 pins the safety to a `?` at `connection.rs:231` producing `expected 1 event(s) in ICS, found 0`. That mechanism is gone twice over: QA F-3 replaced it with a whole-push pre-flight, and `b225811` then removed the refusal entirely — tasks are dropped from the outgoing set by `withhold_task_pushes` (`diff/calendar_diff.rs:295-300`) and reported, so nothing aborts at all. `grep -rn refuse_unpushable` returns nothing. **`spec.md:480` was never amended and its cited line and error string are both false.** | **PARTIAL** — outcome met; stated mechanism no longer exists |
| R57 (INVIOLABLE) — old core + new provider is safe because a VTODO payload fails the old `Vec<Event>` decode | One bad element fails the vector; T-51 proves nothing is applied and `rpc/methods/list_events.rs` proves the component is named. A payload with only unsupported components still hard-errors (`rpc::ics::tests::decoding_an_unknown_component_errors_rather_than_dropping`), which is the case R57 depends on. | **PASS** |
| R58 — VTODO query carries no time-range | Bare comp-filter (`caldav/ops/list_events.rs:104-127`); T-54's request half runs with **no server** and fails the moment a time-range returns — so it is real coverage in default CI. Live half **executed** (§2.3). | **PASS** |
| R59 — two REPORTs, never sibling comp-filters | `calendar_queries` returns `[CalendarQuery; 2]` (`caldav/ops/list_events.rs:66-79`); T-38. | **PASS** |
| R60 — kind-aware UID lookup, no time-range | `find_event_by_uid_body(uid, kind)` (`caldav/ops.rs:229`). | **PASS** |
| R61 — the href's decoded kind is verified | `location_at_href` (`caldav/ops.rs:172-194`); T-39. | **PASS** |
| R62 — `event_url` unchanged and kind-free | `caldav/client.rs:73-76`. | **PASS** |
| R63 — 409 `no-uid-conflict` falls back to update | `caldav/ops/create_event.rs:75`; T-40 asserts the outcome using the verbatim probe body. The ticket's rename premise was refused and corrected (refusal 5, §1.4). | **PASS** |
| R64 — single PUT site, guarded | `put_ics` is the crate's only PUT and calls `ensure_single_component` (`caldav/ops.rs:47-95`); T-41. | **PASS** |
| R65 — a same-UID VEVENT/VTODO pair in one collection errors | `ConnectionError::SameUidPair` (`connection/error.rs:24`); live gate `pushing_a_same_uid_pair_into_one_collection_surfaces_an_error` **executed** (§2.3). | **PASS** |
| R66 — resource decoding is loud, never `find_map(Result::ok)` | `parse_item` (`caldav/ops.rs:43`); `decode_resources` refuses a partial listing with the reason written at the site. | **PASS** |
| R67 — the server injects no non-`X-` properties onto a task | `live::radicale::the_server_injects_no_non_x_properties_onto_a_task`, **executed** against real Radicale 3.7.6 (§2.3). | **PASS** |
| R68 — `COMPLETED` and `PERCENT-COMPLETE` are not derived from each other | `todo/from_icalendar.rs:95-99`. | **PASS** |
| R69 — iCloud is stated as UNVERIFIED, never claimed | Stated, not claimed: `adr/ADJUDICATION.md:805-810`; `website/src/content/docs/commands.md:132`; §4 of this document. | **PASS** |
| R70 — Google/Outlook out of scope; **"No provider-crate edits ship in this feature"** | No mapping work shipped for Google or Outlook, and neither can carry a task (both `list_events` return `Result<Vec<Event>>`). But the quoted clause is **false as shipped**: `git diff --stat main...HEAD -- 'caldir-provider-*/src/main.rs'` shows **five** files changed (+19/−13) — caldav, google, icloud, outlook, webcal — to satisfy the new `Handler::list_events -> Vec<CalendarItem>` signature, plus `caldir-provider-webcal/src/commands/list_events.rs`. This is the minimum possible: `list_events` is a trait method and the only inbound channel. **`spec.md:575` was never amended.** | **PARTIAL** — bounded, mechanical, accepted breach of an unamended requirement |
| R71 — pushing a task to Google/Outlook produces `expected 1 event(s) in ICS, found 0` and aborts the rest of that calendar's push | **Superseded, in the user's favour.** Since `b225811` no task ever enters the outgoing set, so nothing aborts and that string is never produced: the change is withheld and a per-calendar notice prints `N task change(s) not pushed: tasks are pull-only — caldir cannot push tasks to a remote yet.` (`caldir-cli/src/output/diff.rs:275-286`). Events in the same calendar keep syncing — locked end-to-end by `caldir-cli/tests/push_exit_status.rs:177` and `sync_exit_status.rs:297`. The requirement was also mis-scoped: the old refusal fired for CalDAV too, not only Google/Outlook. **`spec.md:579` was never amended.** | **PARTIAL** — requirement text describes behaviour that no longer exists |
| R72 — webcal's silent VTODO drop stays out of scope | Still drops (`caldir-provider-webcal/src/commands/list_events.rs:20-26`), covered by the R49 belt. The stale `TODO(L3-04)` attribution the architecture and compat reviews both flagged is **removed**; the comment now states the reason. | **PASS** |
| R73 — no capability mechanism ships | No config field, builder, third bucket or capability renderer. `git diff main...HEAD -- caldir-core/src/calendar/config.rs caldir-core/src/caldir/config.rs` is empty. | **PASS** |
| R74 — both degradation bugs fixed, and A's fix does not introduce B | A fixed by per-file degradation + R48; B fixed by R19's "never yields nothing" + R49. `decode_resources` refuses a partial listing. | **PASS** |
| R75 — "no supported component" distinct from "too many" | `NoSupportedComponent` vs `ExpectedSingleComponent` (`item.rs:92-100`); exact wording asserted in T-8. | **PASS** |
| R76 — the wire type reuses the arity variant | `from_single_ics_str` (`item.rs:44-54`) with no "none" variant. | **PASS** |
| R77 — a broken VEVENT still surfaces as `UnreadableEvents` | `doctor/warning.rs:60-63`; T-43, plus `doctor::tests::a_valid_vtodo_sibling_is_not_a_warning`. | **PASS** |
| R78 — `Calendar::events()` is never silently short | `CalendarListing { events, skipped }` with `is_incomplete()` (`calendar/listing.rs:29-72`). | **PASS** |
| R79 — 409 recovery | See R63. | **PASS** |
| R80 — structural single-PUT guarantee | See R64. | **PASS** |
| R81 — `rsvp`/`invites` narrow to events and fail cleanly on a task | Single-file mode fails with the exact sentence and writes nothing (`commands/rsvp.rs:57-60`); list mode and `invites.rs:104` narrow. T-42. | **PASS** |
| R82 — every C row is evidenced at its stated tier | See §1.2. | **PASS** |
| R83 — the `--json` widening is documented with a task example | `website/src/content/docs/commands.md:198-255`, with a task example object and the migration predicate. | **PASS** |
| R84 — tasks anchored on `DUE`→`DTSTART`, interleaved, undated last | `anchor()` (`output/agenda.rs:93-101`); `kind_ordinal` puts the event first; undated sort last and get no invented date. T-46, T-47. | **PASS** |
| R85 — sort stays stable, no identity tiebreak added | `output/agenda.rs:124-140`. | **PASS** |
| R86 — Google Tasks / MS To Do are a non-goal | No tasks API touched in either provider; scopes unchanged. | **PASS** |
| R87 — `RELATED-TO` not modelled, loss pinned | `Todo` has no `related_to` field; T-16 pins the drop. `spec.md` §6's contradicting row was corrected in `bd64f4e` after the refusal (refusal 8, §1.4). | **PASS** |
| R88 — `CATEGORIES` OUT | Mechanism note carried in T-17's comment. | **PASS** |
| R89 — recurring tasks modelled, never expanded | `todo.rs:54`; an undated recurring task is in-window by R44 row 4. | **PASS** |
| R90 — `caldir new` stays event-only | `commands/new.rs:116` builds an `Event`; `new.rs:438` locks that it loads back as an `Event`. | **PASS** |
| R91 — ADR-07 absent in whole | No capability config, builder, provider edit for capability purposes, third bucket or pluralized renderer. | **PASS** |
| R92 — `supported-calendar-component-set` recorded only | No code reads it. | **PASS** |
| R93 — VJOURNAL is one variant + one arm structurally | `CalendarItem` and `ComponentKind` are `#[non_exhaustive]` (`item.rs:14`, `instance_id.rs:15`), now locked by `tests/public_error_api.rs:222-241`. **Structural claim holds; the cost estimate does not** — see §7 and §8. | **PASS** |
| R94 — `Event*` type names unchanged | `EventInstanceId`, `EventChange`, `event_bases` all keep their names. | **PASS** |
| R95 (A-1, INVIOLABLE) — the mandatory scenario | Clauses 1, 3, 4 and 5 hold, measured against real Radicale (§2.3). **Clause 2 — "when caldir loads it *and pushes it* to a generic CalDAV server" — is not achieved by caldir.** T-53's own doc comment says so: the two PUTs are issued by the test harness, because a task cannot reach a PUT through production code at all. Under R70/R53 the push half is unreachable by design; since `b225811` it is doubly unreachable, as tasks never enter the outgoing set. Disclosed honestly everywhere except in R95 itself, which is marked INVIOLABLE and reads as satisfied. | **PARTIAL** — pull half proven live; push half not implemented |
| R96 (A-2) — idempotence at the Connection seam | T-49, driven from raw ICS via `reply_raw_data`. | **PASS** |
| R97 (A-3) — undated completed task survives a pull | Unit lock plus the live half; the request half runs with no server. **Live gate executed** (§2.3). | **PASS** |
| R98 (A-4) — window rule at the diff | `calendar_diff.rs` T-26, present at the reassigned home (refusal 2, §1.4). | **PASS** |
| R99 (A-5) — absence in a *task-carrying* response still deletes | `calendar_diff.rs` T-27. | **PASS** |
| R100 (A-6) — same-UID pair distinct end to end, no doctor false positive | T-52 and T-31; `duplicate_file.rs`. | **PASS** |
| R101 (A-7) — arity/error vocabulary | T-8 row 6. | **PASS** |
| R102 (A-8) — VTIMEZONE sibling on the wire | T-10. | **PASS** |
| R103 (A-9) — renaming a local file updates the same resource and never a second one | Measured on the **VEVENT control only** (`live::radicale::renaming_a_local_file_updates_the_same_resource_and_never_a_second_one`, **executed**, §2.3). The task property transfers *by construction* — `event_url` is UID-derived (`caldav/client.rs:73-76`) — not by measurement, because a rename reaches a server only via a push and no task push exists. No unit lock proves `event_url` is purely UID-derived; that remains open (§8). | **REFUSED** — reasoning in §1.4 (refusal 7) |
| R104 (A-10, INVIOLABLE) — the baseline stays green with zero edits to existing event tests | **968 passed / 0 failed** in all three CI zones (§2). **One exception to "zero edits":** `caldav::ops::tests::parse_event_returns_none_on_invalid_ics` was renamed *and inverted* to `parse_event_errors_on_invalid_ics` (`caldav/ops.rs:352`). Forced by R66, which mandates the contract change — bounded and justified, but it is an edit to a pre-existing test and R104 admits none. | **PARTIAL** — one pre-existing test renamed and inverted |
| R105 — all 58 named tests exist at the pinned module paths | Verified by the spec review by `grep -rn "fn <name>"` across the workspace; every hit landed in the module the spec names. Re-confirmed for the six live gates by name here (§2.3). | **PASS** |
| R106 — three-zone CI matrix | `.github/workflows/ci.yml:36-42` runs `UTC`, `Europe/Stockholm`, `America/New_York` with `fail-fast: false`. All three re-run here (§2.1). Two tests that failed outside that matrix (UTC+13/−11) were fixed in `5a2f756`. | **PASS** |
| R107 — the live tier is runnable from a documented, reproducible container | Skippable and documented: `live_gate` prints a SKIP line and returns (`live/radicale.rs:143-154`); the bcrypt recipe is embedded verbatim. **The image is not pinned:** `const IMAGE: &str = "tomsquest/docker-radicale:latest"` (`live/radicale.rs:161`), commented "Pinned by tag", while the probe measured digest `sha256:e2ef8624b215…`. A tag is not a pin; the live tier can silently start testing a different Radicale. One-line fix, still open. | **PARTIAL** — container is tag-referenced, not digest-pinned |
| R108 — probe hygiene: no new probe crate | No probe crate ships; the rule is recorded and the live tier prints its own inputs. | **PASS** |

**R1–R108: 97 PASS · 9 PARTIAL · 2 REFUSED · 0 FAIL.**

### 1.2 C1 – C13

| Constraint | Tier | Evidence | Result |
|---|---|---|---|
| C1 — event filenames | INVIOLABLE | `Event::base_slug` untouched (`event/slugify.rs:13-19`); `base_slug_is_unchanged_for_vevent_corpus` uses literal expectations that can disagree with the code by construction. Independently confirmed by execution in the compat sweep: `caldir new "Coffee ☕ with Bob"` produced the identical filename on both binaries. | **PASS** |
| C2 — an existing VEVENT file parses to a structurally `==` `Event` | INVIOLABLE | `vevent_control_round_trips_identically_after_vtodo_support` (`event.rs:1368`); event equality helpers only *moved* to `event/equality.rs`, semantics unchanged, and since `7a4444f` locked by an exhaustive destructure. Confirmed by execution: `diff -r -I '^DTSTAMP'` over the whole caldir tree after a full sync — identical, files and state. The one case where this was **breached** — a VEVENT beside an unsupported sibling — is fixed (R19). | **PASS** |
| C3 — config files | INVIOLABLE | `git diff main...HEAD -- caldir-core/src/calendar/config.rs caldir-core/src/caldir/config.rs` is empty. No key added, renamed or retyped; no `deny_unknown_fields`. | **PASS** |
| C4 (amended) — `known_event_ids` | INVIOLABLE for the event form | Event lines unprefixed, one id per line, sorted (`instance_id.rs:89-108`, `known_event_ids.rs:64-72`); both halves locked, plus the downgrade-inertness half at `calendar_diff.rs`. Post-sync file byte-identical between the two binaries on an event-only caldir. | **PASS** |
| C5 — `bases/<sha256>.ics` | INVIOLABLE | The `else { continue }` skip is preserved; hash scheme unchanged (`event_bases.rs:107-109`); digests asserted as independent literals. Post-sync base filenames identical; contents identical mod `DTSTAMP`. | **PASS** |
| C6 — downgrade with VTODO files present | INVIOLABLE | An old caldir hitting a VTODO file gets a hard error before any write. Measured, not inferred: `caldir events` → exit 1, `expected exactly one event in …_task__….ics, found 0`; `caldir sync` → per-calendar error, **zero RPC calls**, nothing deleted, `known_event_ids` untouched. The `VTODO:` line is never reached. | **PASS** |
| C7 — skipped file → zero deletes | INVIOLABLE | `Deletes::Suppressed` guards both delete sites (`calendar_diff.rs:25-29`). | **PASS** |
| C8 — RPC verbs | INVIOLABLE | T-33 pins the six verbs. | **PASS** |
| C9 — RPC fields | INVIOLABLE | T-34, including a crate-wide grep for `deny_unknown_fields`. | **PASS** |
| C10 — wire decode of an unknown component: hard error, never a silent drop or empty vec | INVIOLABLE | **The load-bearing half holds:** a payload whose components are *all* unsupported is a hard error naming them (`rpc/ics.rs:230`, T-35), so R57's old-core tripwire is intact and no decode ever yields an empty vec. **The sibling half does not:** an unsupported component *beside* a supported one is now ignored rather than erroring, for the reason given at R19. The wire path shares `parse_document` with disk by design (R54), so the disk fix necessarily changed both. The drop is recorded, not silent — but on the wire there is no `doctor` surface to report it, so on that path it *is* unreported. | **PARTIAL** — sibling case tolerated; unreported on the wire path |
| C11 — `caldir-core` Rust API: minor bump + all six dependants repinned | SOFT | 0.14.1 → 0.15.0, verified: `git show main:caldir-core/Cargo.toml` says `0.14.1`, HEAD says `0.15.0`, and all six dependants (`caldir-cli` + five providers) carry `version = "0.15.0"`. The seventh pin class — `caldir-provider-icloud/Cargo.toml:21`, a cross-provider pin on `caldir-provider-caldav` — was found and fixed. `.agents/skills/bump/SKILL.md` still does not cover that class. | **PASS** |
| C12 (amended) — `--json` agenda schema | INVIOLABLE as amended | **Code satisfied, and the honest guarantee was verified by execution against the pre-change binary in three timezones:** `jq -c 'map(select(.kind=="event") \| del(.kind,.due,.completed,.percent_complete,.task_status))'` over the new stream is byte-identical to the old, for `events`, `today` and `week`. Five keys added, `start`/`availability` null only on `kind == "todo"`, `status` not retyped; T-44; predicate documented at `commands.md:250`. **But `spec.md:656` still says "Every `kind == "event"` entry stays byte-identical"** — which the adjudication itself ruled arithmetically impossible (adding five keys changes every line's bytes) and ordered replaced by the `del(...)` predicate. Commit `e6cd79c` recorded the correction in `ADJUDICATION.md` only. | **PARTIAL** — spec row states a guarantee the code cannot and does not make |
| C13 — CLI text output and exit codes for existing situations | INVIOLABLE | Event-only rendering is byte-identical (T-45), and `doctor` still reports a broken VEVENT as `UnreadableEvents` (T-43). Confirmed by execution across `events`, `today`, `week`, `config`, `calendars`, `invites`, `doctor`, `rsvp` — stdout, stderr and exit codes all identical. **Two departures for existing, event-only situations:** (1) with an unreadable `.ics`, `events`/`today`/`week` went from exit 1 with empty stdout to exit 0 with a partial agenda and a `⚠` notice — this is the entire point of the graceful-degradation work (R6) and is deliberate, but it *is* a change to existing text for an existing situation; and (2) a CalDAV server that rejects the new VTODO REPORT turns a working event-only sync into an error (R2). | **PARTIAL** — one deliberate departure (graceful degradation), one undecided (R2/B-16) |

**C1–C13: 10 PASS · 3 PARTIAL · 0 FAIL.**

### 1.3 Acceptance scenarios A-1 … A-10

| Scenario | Result |
|---|---|
| A-1 (R95) — u_crawler VTODO round-trips through a generic CalDAV server | **PARTIAL** (INVIOLABLE). Clauses 1/3/4/5 proven live. Clause 2 unreachable: the PUTs are the harness's, not caldir's. |
| A-2 (R96) — sync twice, second diff empty | **PASS** |
| A-3 (R97) — undated completed task survives a pull | **PASS** (unit + live, and the request half runs with no server) |
| A-4 (R98) — window rule at the diff | **PASS** |
| A-5 (R99) — absence in a task-carrying response still deletes | **PASS** |
| A-6 (R100) — same-UID VEVENT/VTODO pair stays distinct | **PASS** |
| A-7 (R101) — arity and error vocabulary | **PASS** |
| A-8 (R102) — VTIMEZONE sibling on the wire | **PASS** |
| A-9 (R103) — rename updates the same resource | **REFUSED** — measured on the VEVENT control; the task property transfers by construction |
| A-10 (R104) — no VEVENT regression | **PARTIAL** — 968/0 in three zones, with one pre-existing test renamed and inverted |

### 1.4 The eight refusals, with reasoning

Each was refused during implementation and the refusal was verified — by re-deriving it against
code, not by accepting the record. The spec review re-derived all eight independently and found
**no unsound refusal**. Two of them are the reason a requirement row above reads REFUSED; the
other six corrected *ticket* text and left the requirement met.

1. **The recurring-instance task filename row** (R34). The ticket asked for a rule that
   `Event::base_slug` makes unreachable — it keys on `recurrence.is_some()`
   (`event/slugify.rs:14`). Correction landed in `specs/caldir.md:366`. *Row stays PASS.*
2. **T-26's reassignment.** The ticket placed the A-4 lock in the wrong module; it was moved
   and is present at `calendar_diff.rs`. *R98 stays PASS.*
3. **L4-01 AC 1 — "`Ics` wraps a `CalendarItem`".** Refused because `WireValue::from_wire` is
   infallible: the correct resolution is two newtypes, `Ics` outbound and `ItemIcs` inbound
   (`rpc/ics.rs:33,60`), so the fallible decode lives in `Deserialize`. This is the concrete
   form of the R53 refusal. *R53 = REFUSED.*
4. **T-48's name.** The ticket's name asserted a byte-exact literal the test no longer produces.
   The truthful name stands. *Feeds the R56 PARTIAL.*
5. **The 409 rename-premise correction** (R63/R103). The ticket assumed a rename changes the
   resource href; `event_url` is UID-derived (`caldav/client.rs:73-76`), so it does not.
   *R63 stays PASS.*
6. **L6-01's error-envelope AC.** Refused as unreachable: the push refusal aborted before any
   RPC, so the provider error envelope could not be exercised. (Since `b225811` there is no
   refusal at all, which makes the refusal doubly correct.) *R71 PARTIAL for other reasons.*
7. **T-58's name** (A-9/R103). `renaming_a_local_task_file_updates_the_same_resource` is
   unreachable — a rename reaches a server only via a push, and no task push exists. It ships
   on the VEVENT control. *R103 = REFUSED.*
8. **"RELATED-TO is preserved verbatim."** False: `Todo` has no `related_to` field, the
   serializer never emits one, and T-16/T-17 pin the drop. `spec.md` §6's row was corrected in
   `bd64f4e`. *R87 stays PASS.*

### 1.5 Totals

| | R1–R108 | C1–C13 | Combined |
|---|---|---|---|
| PASS | 97 | 10 | **107** |
| PARTIAL | 9 | 3 | **12** |
| REFUSED | 2 | 0 | **2** |
| FAIL | 0 | 0 | **0** |

The twelve PARTIAL rows are **R2, R13, R19, R56, R70, R71, R95, R104, R107, C10, C12, C13**.
Nine of the twelve are *documentation debt*: the code does something equal to or better than
the requirement and `spec.md` was never amended (R13, R56, R70, R71, R104's admission, C12,
and the requirement-text half of R19/C10). Three are live gaps a user can reach: **R2/C13** (a
CalDAV server that rejects the VTODO REPORT breaks an event-only sync — undecided),
**R95** (the mandatory INVIOLABLE scenario's push half is not implemented), and **R107** (the
live tier's container is not digest-pinned).

---

## 2. Test commands and counts

### 2.1 Full workspace, three timezones

```
for Z in UTC Europe/Stockholm America/New_York; do TZ=$Z cargo test --workspace; done
```

**Identical in all three zones: 968 passed, 0 failed, 0 ignored.**

| Target | Crate | Tests |
|---|---|---|
| `unittests src/main.rs` | caldir-cli | 153 |
| `tests/agenda_exit_status.rs` | caldir-cli | 6 |
| `tests/push_exit_status.rs` | caldir-cli | 4 |
| `tests/sync_exit_status.rs` | caldir-cli | 17 |
| `unittests src/lib.rs` | caldir-core | 597 |
| `tests/public_error_api.rs` | caldir-core | 8 |
| `unittests src/lib.rs` | caldir-provider-caldav | 43 |
| `unittests src/main.rs` | caldir-provider-caldav | 14 |
| `unittests src/main.rs` | caldir-provider-google | 49 |
| `unittests src/main.rs` | caldir-provider-icloud | 14 |
| `unittests src/main.rs` | caldir-provider-outlook | 49 |
| `unittests src/main.rs` | caldir-provider-webcal | 14 |
| Doc-tests (caldir_core, caldir_provider_caldav) | — | 0 |
| **Total** | | **968** |

Per crate: **caldir-cli 180** · **caldir-core 605** · **caldir-provider-caldav 57** ·
**google 49** · **outlook 49** · **icloud 14** · **webcal 14**.

Baseline on `main` was 686 (spec §18 T-59). This branch adds 282 net.

### 2.2 Lint and format

```
cargo fmt --all --check                                  → exit 0
cargo clippy --workspace --all-targets -- -D warnings     → exit 0
```

### 2.3 Live CalDAV tier — executed, not merely read

The spec review audited this tier **by reading**, with no Docker gate set. It was **executed
here** against a real container:

```
CALDIR_LIVE_RADICALE=1 cargo test -p caldir-provider-caldav --lib -- \
  live::radicale --nocapture --test-threads=1
```

```
running 6 tests
test live::radicale::a_u_crawler_task_round_trips_through_radicale_preserving_identity_and_semantics ... ok
test live::radicale::an_undated_completed_task_survives_a_pull ... ok
test live::radicale::get_pass_two_equals_get_pass_one ... ok
test live::radicale::pushing_a_same_uid_pair_into_one_collection_surfaces_an_error ... ok
test live::radicale::renaming_a_local_file_updates_the_same_resource_and_never_a_second_one ... ok
test live::radicale::the_server_injects_no_non_x_properties_onto_a_task ... ok

test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 37 filtered out; finished in 13.51s
```

Requirements: `CALDIR_LIVE_RADICALE=1`, plus `docker` and `curl` on `PATH`
(`live/radicale.rs:130-141`). Without the gate every one of the six prints a SKIP line and
returns — they are counted in the 968 but assert nothing, **except** the request half of
`an_undated_completed_task_survives_a_pull`, which runs unconditionally before the gate and is
real coverage everywhere (confirmed by mutation: re-adding a `<C:time-range>` to the VTODO
filter fails it with the gate closed). The container is `tomsquest/docker-radicale:latest` —
a tag, not a digest; see R107.

### 2.4 What the suite does *not* cover

Stated plainly, because a green number is the thing that fooled this project once already:

- **Provider `Handler` adapter layers.** `grep -rn process_request caldir-provider-*/src/`
  returns nothing. The five `main.rs` sites this branch edited have no test of their own. The
  two round-trip tests that were added live in `caldir-core` against a local test double, which
  covers the wire codec — not the risk. A dropped listing is a mass delete. (BACKLOG B-12.)
- **The staleness half of `bases_to_backfill`** (`connection.rs:172,176`): two mutations survive
  the whole suite, including one that makes a stale base never refresh.
- **`CalendarListing::len()`**, **the `recurring` branch of the task `--json` schema**, and
  `CalendarEventError::ExpectedSingleEvent` (constructed nowhere).
- **A CalDAV server that rejects the VTODO REPORT** — the R2/C13 breach has no test at all.
- **iCloud** — nothing, at any tier. See §4.

---

## 3. Architecture changes

- **`CalendarItem`** (`caldir-core/src/item.rs`) — a `#[non_exhaustive]` two-variant enum,
  `Event` | `Todo`, and the crate's single parse entry point. One ordered pass over
  `icalendar.components`; `VTIMEZONE` allow-listed out of arity counting; unsupported siblings
  recorded and reported; an all-unsupported document is a hard error naming what it held.
- **`Todo`** (`caldir-core/src/todo/`) — a sibling struct, never a variant of `Event`. 20
  modelled fields; no `end`/`duration`, because RFC 5545 §3.6.2 forbids `DTEND` on a VTODO and
  the field not existing is what makes emitting one impossible. Its own codec
  (`from_icalendar.rs`, `to_icalendar.rs`) and its own slug rule (`_task__` prefix).
  `PartialEq` ignores `created`/`last_modified`/`sequence` and is locked by an exhaustive
  destructure, because `==` **is** the sync-direction predicate.
- **Kind-carrying identity** — `EventInstanceId { kind, uid, recurrence_id }`. The string form
  gains a `VTODO:` sigil for tasks; the event form is byte-identical to before, which is what
  keeps `known_event_ids` and `bases/<sha256>.ics` compatible in both directions.
- **One item-typed diff engine** — `CalendarDiff::compute_with` is item-typed end to end. No
  second sync engine. Two belts: `Deletes::Suppressed` (an incomplete local listing emits zero
  deletes) and the task-delete suppression when a remote reports no tasks at all.
- **`withhold_task_pushes`** (`diff/calendar_diff.rs:295-300`) — drops every non-`VEVENT`
  change from the outgoing set and returns the count, keyed on `ComponentKind` rather than the
  Rust type so a future component is withheld rather than smuggled onto an event-only wire.
  The count is rendered per calendar by `status`, `push`, `sync` and `discard`.
- **Split RPC seam** — inbound `Handler::list_events` returns `Vec<CalendarItem>` (new `ItemIcs`
  newtype); outbound `create/update/delete_event` stay `Event`-typed (`Ics`). This asymmetry is
  the feature's defining cut and the reason it is pull-only.
- **Graceful degradation** (ships separately as PR 1) — `Calendar::events()` returns a
  `CalendarListing { events, skipped }` instead of all-or-nothing; `doctor` reports each skipped
  file; the agenda carries an `incomplete` footer with an `IncompleteCause` that distinguishes
  unreadable files from a calendar that would not list at all.
- **Public error API** — `EventError`, `CalendarError`, `CalendarEventError` and `RemoteError`
  re-exported and `#[non_exhaustive]`, proven from a genuine separate crate
  (`caldir-core/tests/public_error_api.rs`, 8 tests).
- **CalDAV** — strict resource decoding (a malformed resource fails the listing rather than
  being skipped into a mass delete); kind-aware UID lookup with no time-range for VTODO
  (RFC 4791 §9.9); a 409 `no-uid-conflict` fallback from create to update; a single guarded
  PUT site; a live Radicale test tier.

**Why an enum and not a widened `Event` or a trait.** `SyncBases` is **one** map per calendar
directory that must hold both kinds, and `CalendarDiff` uses structural `==` as its
"has the content changed?" test. `Box<dyn Component>` loses the derived `PartialEq` that *is*
the sync-direction test; a type parameter would have had to thread through `CalendarEvent`,
`RemoteEvent`, `EventChange`, `CalendarDiff` and `SyncBases` and still meet an enum at the
directory boundary. That is the reason. (The "41 errors across 7 files" figure that circulated
during design measured *migration friction*, not architectural necessity, and is not a
justification for anything.)

---

## 4. Provider matrix

| Provider | Task pull | Task push | Verified stance |
|---|---|---|---|
| **caldav** | **Yes** | **No** | The only provider that pulls tasks. Verified against a real Radicale 3.7.6 container, executed here: six live gates pass (§2.3), and QA's independent live run produced `_task__2026-09-05T1700__dated-task.ics`, `_task__recurring__water-the-plants.ics` and `_task__undated-task.ics` alongside the VEVENT from a `caldir pull`. Push is structurally impossible: the outbound RPC field is `pub event: Event`, and tasks are dropped from the outgoing set before any RPC. |
| **icloud** | **Unknown — UNMEASURED** | No | **Do not claim task support.** iCloud shares the CalDAV code path verbatim: all five ops call straight into `caldir_provider_caldav::caldav::ops` (`list_events.rs:19`, `create_event.rs:19`, `update_event.rs:19`, `delete_event.rs:18`, `list_calendars.rs:19`, `connect.rs:33` — a grep for `ops::` in `caldir-provider-icloud/src` returns exactly those six lines and nothing else). There is no iCloud-side ICS layer, property mapper or query builder, so the VTODO `calendar-query` iCloud would send is byte-identical to the generic one. That is a *structural* argument, not a measurement. Four load-bearing unknowns remain: does iCloud 409 a same-UID pair? does it implement RFC 4791 §9.9? does it inject non-`X-` properties? are Reminders collections returned by discovery at all? **Zero tests at any tier touch a real iCloud account.** Settled only by running the six probes against one. |
| **google** | No | No | Structurally VEVENT-bound and stronger than the spec claims: `caldir-provider-google/src/commands/list_events.rs:15` returns `Result<Vec<Event>>`, so a task cannot be represented at all. Endpoints are `/calendar/v3/calendars/{}/events` only; OAuth scopes (`connect.rs:24-25`) do not reach the Tasks API. A task file in a Google-backed calendar is withheld from the push with a notice; the calendar's events keep syncing. |
| **outlook** | No | No | Same: `list_events.rs:17` returns `Result<Vec<Event>>`; every Graph path is `/me/calendars/{}/events` or `/me/events/{}`; `SCOPES` is exactly `"Calendars.ReadWrite User.Read offline_access"`. Same withholding behaviour. |
| **webcal** | No (silently dropped) | N/A — read-only | `main.rs` implements only `connect` and `list_events`; there is no create/update/delete arm, and `connect.rs:162` pins `read_only() == Some(true)`. A `VTODO` in a feed is dropped without a word (`commands/list_events.rs:20-26`) — deliberately out of scope (R72), and the resulting "remote reports zero tasks" is covered by the R49 suppression belt so it can never delete local task files. Live-verified by QA: a feed carrying a VEVENT and a VTODO pulled the event and dropped the task. |

Five provider `main.rs` adapter sites were edited (caldav, google, icloud, outlook, webcal) to
satisfy the retyped `Handler::list_events`. No provider *logic* changed. **These adapters have
no test of their own in any provider crate** (§2.4).

---

## 5. Backward compatibility

Measured by building two release binaries from clean worktrees — `main` (`2ed74fa`) and this
branch — and driving both against the same fixtures with a scripted mock provider on `PATH`
that logs every RPC it receives. Re-checked against HEAD where the last four commits touched
the surface.

### What does not change

| Surface | Result |
|---|---|
| Event filenames (`base_slug`) | Byte-identical. `event/slugify.rs` is a pure extract-function refactor. |
| Existing `.ics` re-parse and rewrite | `diff -r -I '^DTSTAMP'` over the whole caldir tree after a full sync: identical, files and state. |
| `state/known_event_ids` | Byte-identical on an event-only caldir. Task lines take a `VTODO:` sigil, absent when no task exists. |
| `state/bases/<sha256>.ics` | Filenames and contents identical (mod `DTSTAMP`) — the event identity string form is unprefixed, so digests are unchanged. |
| RPC wire, new core + old provider | `list_events` request bytes and `create_event` payload bytes, compared from the provider's own request log: identical except the `DTSTAMP` second. No new verb. |
| RPC wire, old core + new provider | Payload is still a bare ICS string; `ItemIcs` is a second newtype over the same JSON string, not a new field. |
| CLI text on a healthy caldir | `events`, `today`, `week`, `config`, `calendars`, `invites`, `doctor`, `rsvp` (by path and by slug, including their errors): stdout, stderr and exit codes all byte-identical. |
| `--json` for `config`, `calendars`, `invites` | Byte-identical. |
| `--json` for `events`, `today`, `week` | Additive only, and the migration predicate is **verified byte-for-byte** in three timezones: `select(.kind=="event") \| del(.kind,.due,.completed,.percent_complete,.task_status)` reproduces the old stream exactly. |

### Old caldir reads new state

The full cross matrix was run: old-binary-on-old-state ≡ old-binary-on-new-state;
new-on-old ≡ new-on-new; old ≡ new on the same state. **All byte-identical stdout, no state
drift.** An older caldir treats a `VTODO:`-prefixed line in `known_event_ids` as an inert
unknown id that cannot trigger a delete, in both directions.

With a **task file** present, an old caldir fails loudly and safely: `caldir events` exits 1
with `expected exactly one event in …_task__….ics, found 0`; `caldir sync` errors per calendar,
issues **zero RPC calls**, deletes nothing and leaves `known_event_ids` untouched. This is the
documented C6 outcome, and it is the correct one.

### What does change

1. **`caldir events` / `today` / `week` on a caldir with an unreadable `.ics` file.** Old: exit
   1, empty stdout, the bad path named. New: exit 0, a partial agenda, and `⚠ N unreadable
   files in <calendar> (caldir doctor)`. `--json` names the skipped file on **stderr**
   (`agenda_exit_status.rs:169`), and a calendar that will not list at all is named and exits
   non-zero (`agenda_exit_status.rs:232`). This is the graceful-degradation work and is the
   most valuable behaviour change in the branch — but it is a change, on an event-only path,
   and no caldir document mentions exit codes anywhere.
2. **`caldir push` exit code and error stream.** Old: exit 0, error on stdout. New: exit 1,
   error on stderr (`commands/push.rs:64,70-72`). `sync` and `discard` gained the same
   treatment in `cc213c6`. `caldir push` will now fail a `set -e` pipeline on a transient
   remote error, and a pipeline capturing stdout no longer captures the error text.
3. **CalDAV and iCloud issue a second REPORT per `list_events`** — a bare
   `<C:comp-filter name="VTODO"/>` with no time-range. An event-only user takes an extra
   round-trip per calendar per sync, and any server that rejects that filter fails the whole
   listing. **No test covers a server that rejects it.** This is R2/C13's open breach and
   BACKLOG B-16, still marked undecided.
4. **`caldir-core` 0.14.1 → 0.15.0, breaking, with six dependants repinned.**

   | Item | `main` | HEAD |
   |---|---|---|
   | `Calendar::events` | `Result<Vec<CalendarEvent>, _>` | `Result<CalendarListing, _>` |
   | `CalendarEvent::event` | `&Event` | `Option<&Event>` |
   | `RemoteEvent` | `RemoteEvent(Event)` | `RemoteEvent(CalendarItem)` |
   | `EventChange::{Create,Update,Delete}` | carry `Event` | carry `CalendarItem` |
   | `EventInstanceId` | tuple struct | named struct + `kind` |
   | `CalendarDiff::compute` | — | `compute_from_listing` |
   | `Event::from_ics_str` / `from_single_ics_str` / `load_single` | present | removed |
   | `Handler::list_events` | `Vec<Event>` | `Vec<CalendarItem>` |
   | `Calendar::create_event`, `CalendarEvent::{create,update}`, `RemoteEvent::new` | `Event` | `impl Into<CalendarItem>` (source-compatible for `Event` callers) |

   Additive: `CalendarItem`, `Todo`, `TodoStatus`, `ComponentKind`, `CalendarListing`,
   `SkippedEvent`, `ExpandedItems`, plus re-exported `CalendarError`, `CalendarEventError`,
   `EventError`, `RemoteError`. Nothing was removed from `lib.rs`. **The break list is not in
   any changelog a crates.io consumer would read.**

5. **`--json` agenda entries gain five keys** — `kind`, `due`, `completed`,
   `percent_complete`, `task_status` — on every entry, so the object shape stays uniform.

### The one thing that broke and is now fixed

The compat review found, and measured end to end, that a **legal `.ics` file carrying a `VEVENT`
beside any other top-level component lost its event** — invisible in text, `--json` and sync,
with exit 0 and no warning outside the agenda renderer. RFC 5545's `icalbody` explicitly permits
`x-comp` and `iana-comp` at top level; exporters emit them. **Closed** (see R19): the sibling is
ignored and reported through `doctor`, the event loads, and the case is locked by six tests
including a CLI integration test (`agenda_exit_status.rs:140`).

---

## 6. Known limitations

From BACKLOG Part II §C, each re-verified against HEAD. Three of the thirteen changed status.

| # | Limitation | Current status |
|---|---|---|
| C-1 | **caldir pulls tasks from CalDAV and cannot push them.** | **TRUE — the largest limitation of the feature.** The *mechanism* described in BACKLOG is stale: there is no longer an abort. A task change is withheld from the outgoing set and reported per calendar (`N task change(s) not pushed: tasks are pull-only`). Local task edits stay local, permanently. Downstream: a local task-file **rename** never reaches a server. |
| C-2 | **iCloud (Apple Reminders) is UNMEASURED.** | **TRUE.** See §4. Do not claim iCloud task support anywhere. |
| C-3 | Pushing a task to Google/Outlook aborts the rest of that calendar's push | **NO LONGER TRUE — superseded.** Nothing aborts; the task change is withheld and the calendar's events keep syncing. Locked by `push_exit_status.rs:177` and `sync_exit_status.rs:297`. |
| C-4 | `CLASS` on a VTODO is dropped on caldir's first write | **TRUE.** Not modelled; the unmodelled-property passthrough is `X-*` only. |
| C-5 | caldir's first genuine edit destroys correct server data for every dropped property | **UNREACHABLE for tasks.** CalDAV update is a whole-resource PUT, so this *would* apply — but caldir cannot push a task at all, so the first genuine edit never happens. It becomes reachable the day task push lands, for `CATEGORIES`, `RELATED-TO`, `GEO`, `RESOURCES`, `COMMENT`, `CONTACT`, `CLASS` and any other non-`X-` property. |
| C-6 | Recurring tasks are master-only, never expanded | **TRUE.** |
| C-7 | A task update touching only an unrowed field renders a header line with no rows | **TRUE.** Under-described, not invisible, not data loss. |
| C-8 | VTODO `VALARM` round-trips only through caldir's hand-rolled emitter | **TRUE.** The crate's typed path regenerates a random VALARM UID on every write; the spliced block is byte-stable, verified against pinned `icalendar =0.17.10`. |
| C-9 | `--json` "byte-identical" is impossible; the `del(...)` predicate is the honest guarantee | **TRUE**, and the predicate is verified (§5). The docs state it correctly; `spec.md:656` does not (C12). |
| C-10 | `completed`'s encoding diverges from ADR-10 D6's wording | **TRUE.** A floating `COMPLETED` is legal, so `completed` is an `EventTime` and renders in the local zone rather than being coerced to UTC. The code is right; the ADR wording is narrow. |
| C-11 | A VEVENT whose UID literally begins with `VTODO:` round-trips as a task | **TRUE, residual accepted.** No encoding removes it, because UID is TEXT; it is the same class the codebase already accepts for `__`. Compatibility holds in both directions. |
| C-12 | caldir emits no `VTIMEZONE`; the pushed body and the stored body differ structurally | **TRUE**, and now measured: Radicale synthesizes one, sorts properties and re-folds lines, but *is* idempotent. Any "sync twice" check must be parsed-domain `==` or GET-pass-2 vs GET-pass-1, never raw bytes against what was PUT. |
| C-13 | `ETag` can prove "remote changed"; it can never prove "caldir's push was a no-op" | **TRUE** — `DTSTAMP` changes it on every push. |

**Four more that BACKLOG §C does not list, all verified:**

- **A local task edit always loses eventually.** A local task change can never push, so when the
  server later touches the same task the pull overwrites it. QA captured a locally-completed
  task being reverted to `needs_action` by a `caldir pull`. Shown in the diff render, so not
  silent — but inevitable rather than a tiebreak. For tasks, sync is not last-write-wins.
- **`caldir discard --force` is the only built-in way to clear a local task change, and it is
  destructive** — it deletes the file or reverts the completion. Undocumented.
- **The R49 belt is permanent once tripped.** A user who genuinely clears every task on the
  server keeps the local files forever and sees a warning that appears in no documentation.
- **A deeply nested `.ics` (~1000 nested `BEGIN:`) aborts the process** with SIGABRT. Identical
  on `main` for `VEVENT`, so pre-existing `icalendar` parser recursion — but VTODO widens the
  input surface to a CalDAV response body.

### Documentation defects found while writing this report — open

- **`website/src/content/docs/overview.md:52`** still says `caldir push` and `caldir sync`
  "stop that one calendar with an error (`expected 1 event(s) in ICS, found 0`)" and that the
  calendar "keeps failing until the task is moved somewhere else". Both halves are now false,
  and `caldir-core/src/connection.rs` asserts twice that users will *not* see that string.
  `providers.md:52` and `:90` repeat the claim for Google and Outlook. **Three stale sites; must
  be corrected before merge** — this is the first string a confused user would search for.
- **`caldir-core/src/item.rs:158-175`** — `into_event`'s doc comment still describes the removed
  abort ("fails that calendar's push, every sync, with exactly `expected 1 event(s) in ICS,
  found 0`") and names a test, `connection::tests::pushing_a_task_to_a_task_less_provider_fails_
  that_calendar_without_data_loss`, that no longer exists.
- **`BACKLOG.md` C-1 and C-3** describe the removed mechanism, and C-3 is now false.
- **`spec.md`** is unamended on R13, R19, R56, R70, R71 and C12 (§1.5).

---

## 7. Intentionally deferred RFC 5545 features

None of these is a bug. Each was ruled out, recorded, and — where a property is dropped rather
than absent — pinned by a test that fails if the drop ever becomes silent.

| Feature | Disposition |
|---|---|
| `RELATED-TO` — task hierarchies, subtasks | Not modelled. The `icalendar` crate parses it lossily; modelling it half-way destroys data. Drop pinned by T-16/T-17. |
| `CATEGORIES` | Not modelled — the crate corrupts it on round-trip. Drop pinned. |
| `CLASS` | Not modelled. Dropped on caldir's first write. |
| `GEO`, `RESOURCES`, `COMMENT`, `CONTACT` | Not modelled; same class as `CLASS`. |
| Unknown `STATUS` values (e.g. `X-WAITING`) | Collapse to the RFC default `NEEDS-ACTION` on write. Pinned by T-14. |
| Recurring-task **expansion** | `RRULE`/`EXDATE`/`RDATE` are modelled and round-trip, but a recurring task is never expanded into instances. An undated recurring task is unconditionally in-window. |
| `VJOURNAL` | Not implemented. The architecture absorbs it — diff engine, state layout, wire format, identity string and the withhold predicate all take a new variant unchanged, because the predicate keys on `ComponentKind` rather than the Rust type. The **cost estimate does not survive**: 13 exhaustive match arms in `item.rs` alone plus a `todo.rs`-sized codec module (~310 production lines) *per kind*, because the shared `read_shared`/`write_shared` codec was never built. |
| `VFREEBUSY` and other top-level components | Ignored beside a supported item, and reported by `doctor`; an error if a document holds nothing else. |
| Google Tasks / Microsoft To Do | Explicit non-goal — different products, different APIs, new OAuth scopes. |
| `caldir new --due` (creating a task locally) | Explicit non-goal. `caldir new` stays event-only, locked. |
| `supported-calendar-component-set` capability negotiation | Recorded only; no code reads it, by design. |
| A `--hide-completed` flag or config key | Deferred. Completed tasks render dimmed. |
| Typed `VALARM` emission | Deferred — the crate's typed path regenerates a random UID per write. |

---

## 8. Recommended follow-up issues

Ordered by what a user can hit.

1. **Correct the three stale website sites** (`overview.md:52`, `providers.md:52`, `:90`) and
   the stale `into_event` doc comment. Consider a test that greps the website for the error
   strings it quotes, or stop quoting error strings in the website. *(Blocking.)*
2. **Decide BACKLOG B-16 — the CalDAV second REPORT's failure surface.** Today a server that
   rejects a bare `VTODO` comp-filter turns a working event-only sync into an error, with no
   test. Either tolerate a failing VTODO REPORT (and accept that tasks silently do not appear)
   or keep failing loudly and say so in the release notes. It should not ship undecided.
   *(Blocking-ish: it is the only open R2/C13 breach.)*
3. **Test the five provider `Handler` adapters** — one `process_request` round-trip per crate
   asserting a non-empty listing survives the adapter. Five tests. The failure mode of the layer
   this PR retyped is caldir deleting the user's calendar.
4. **Digest-pin the Radicale image** (R107). One line.
5. **Amend `spec.md`** for R13 (`Option<i32>`), R19 (sibling tolerance), R56 and R71 (the
   withholding design), R70 (five adapter sites), and C12 (the `del(...)` predicate). Six
   corrections that currently live only in `ADJUDICATION.md`.
6. **Ride the 0.15.0 breaking window with the two things that need it** — public test
   constructors for `Caldir` and `CalendarDiff` (five call sites were reshaped around their
   absence), and `Ord` on `EventInstanceId` so `duplicate_file_sets` can use a `BTreeMap`.
   Both cost a second breaking release if missed.
7. **Rename the `Event*` types that now hold items** — `CalendarEvent` → `CalendarFile` or
   `StoredItem`, `RemoteEvent`, `SkippedEvent`, `CalendarListing::events()`, `EventChange`,
   `event_bases`. Deliberately out of scope here; wants doing **before** a third component kind
   lands, not after, and in the same breaking window as whatever comes next.
8. **Publish a changelog for the 0.15.0 API breaks** (§5). They exist only in ticket markdown.
9. **Close the `bases_to_backfill` staleness gap** — two surviving mutations, one of which makes
   a stale base never refresh and silently demotes every such pair to the mtime/`SEQUENCE`
   tiebreak.
10. **`EventError::NotAnEvent`** instead of reusing `UnexpectedEventCount { expected: 1,
    found: 0 }` to mean "this is a task". One line; it removes a nine-line apologetic doc
    comment and would have prevented the website defect.
11. **Fix `calendar.rs:396`'s binding wildcard** (`item => tasks.push(item)`) — inside the crate
    that defines the enum, so `#[non_exhaustive]` does not apply and a future `VJOURNAL` would
    be silently filed as a task.
12. **Decide the `#[non_exhaustive]` wildcard policy for `CalendarItem`/`ComponentKind`.** Today
    a new variant would produce zero compile errors outside `caldir-core`: four `unreachable!()`
    panics (one on the `--json` render path) and seven silent wrong-output fallthroughs. Either
    drop the attribute from those two enums until BACKLOG Q-5 ("is `caldir-core` consumed
    out-of-tree?", still **OPEN, UNVERIFIED**) is answered yes, or make every wildcard a
    `Result` — never a panic, never a silent default.
13. **Small test gaps**: `CalendarListing::len()`, the `recurring` branch of the task `--json`
    schema, the dead `CalendarEventError::ExpectedSingleEvent` variant, and a gate-independent
    half for the five live Radicale tests that have none.
14. **Answer Q-10 by measurement** — run the six CalDAV probes against a real iCloud account.
    Nothing else will move iCloud out of UNMEASURED.

---

## 9. Honest scale note

This is a **large diff for a read-only task viewer on one of five providers**. 94 code, test,
spec and website files, +17,720/−1,009 lines (plus 86 files and +18,303 lines of `docs/vtodo/`
planning prose, which does not belong in the repo), 69 commits, a 282-test increase, and a
breaking release of
`caldir-core` with six dependants. What a user gets is: tasks pulled from a CalDAV server appear
in the agenda, and `--undated` on three commands. `caldir new` cannot create one; nothing can
push one; four of the five providers cannot carry one; one of those four is untested.

The independent maintainer review's verdict was **REJECT as a single PR** — of the shape, not
the feature — with a recommended seven-PR split led by the graceful-degradation work. That
recommendation stands and is reproduced in `docs/vtodo/PR-DESCRIPTION.md`. Nothing in this
verification document should be read as arguing the branch should merge as one change.
