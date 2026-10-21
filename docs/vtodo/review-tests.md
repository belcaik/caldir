# REVIEWER 3 of 5 — Test quality / TDD

**Scope**: `main..HEAD` on `vtodo-support` (67 commits, HEAD `b225811`), 964 tests.
**Question answered**: not "do they pass" but "would they notice if the code were wrong?"
**Method**: 111 source mutations executed in a scratch copy of the tree
(`$SCRATCH/mut`, `CARGO_TARGET_DIR` redirected), never in the repo; plus an
18-timezone sweep and two simulated clock-drift experiments. Baseline in the
scratch copy: **964 passed, 0 failed**.

**Headline**: the suite is much stronger than its history suggests. 92 of 111
mutations were caught, and the mutations were *deliberately aimed at suspected
weak spots*, so the true kill rate on random mutation would be higher. Every
lock the brief named as dangerous — delete inference, the sync-window rule, the
CalDAV comp-filters, the arity rules, `withhold_task_pushes`, the exit codes —
**can fail, and I proved each one by mutation**.

**But one contract is almost entirely unlocked**: `Todo`'s `PartialEq`, which
*is* the sync predicate. 11 of its 15 content fields can be dropped from `==`
with all 964 tests green.

---

## Verdict

**The suite would catch a regression in the mechanisms this effort was built
around, and would not catch a regression in what `==` means for a task.**

- Behavioural locks (diff, window, suppression, withholding, exit codes,
  CalDAV request shapes, serializer field emission): **strong**. 92/111 caught,
  including every one of the 18 serializer-field-drop mutations and all 11
  boundary mutations on the §9.9 window rule.
- Contract locks on *derived* behaviour (`PartialEq` field participation,
  `#[non_exhaustive]` on `CalendarItem`, `CalendarListing::len`, the stale-base
  backfill branch): **weak to absent**. 19 survivors, and they cluster here.
- Zone robustness: **repaired**. 18 timezones clean, including UTC+14, UTC−12,
  Pacific/Apia, Australia/Lord_Howe, America/St_Johns.
- Time robustness: **not addressed**. 8 CLI integration tests are pinned to a
  fixed calendar date against a *rolling* ±365-day window; one of them
  degrades into a vacuous pass rather than a failure.

---

## Findings by severity

### HIGH-1 — `Todo`'s equality contract is unlocked; 11 of 15 content fields can leave `==` unnoticed

`caldir-core/src/todo.rs:28-79` derives `PartialEq` via `educe`, ignoring
`created` / `last_modified` / `sequence`. `==` on `CalendarItem` is the
predicate that decides *in-sync vs. changed* in
`caldir-core/src/diff/calendar_diff.rs:84` (`if item == remote_event.item() { continue; }`)
and *push vs. pull* in `update_direction`
(`caldir-core/src/diff/calendar_diff.rs:315-317`). A field silently absent from
`==` means a remote change to that field is **never pulled** — the file is
declared in sync forever, while the serializer still writes the stale value.

The guard test, `caldir-core/src/todo.rs:252
todo::tests::equality_mirrors_event_equality`, enumerates only three content
fields (step 4: `status`, `completed`, `percent_complete`) and asserts nothing
about the rest.

**Proof.** For each field, I inserted `#[educe(PartialEq(ignore))]` above the
declaration and ran the full workspace suite:

| Field ignored by `==` | Result |
|---|---|
| `summary` | caught (1 test) |
| `status` | caught (`equality_mirrors_event_equality`) |
| `completed` | caught (`equality_mirrors_event_equality`) |
| `percent_complete` | caught (`equality_mirrors_event_equality`) |
| `description` | **SURVIVED** |
| `location` | **SURVIVED** |
| `start` (`DTSTART`) | **SURVIVED** |
| `due` (`DUE` — the task's anchor) | **SURVIVED** |
| `priority` | **SURVIVED** |
| `url` | **SURVIVED** |
| `recurrence` | **SURVIVED** |
| `recurrence_id` | **SURVIVED** |
| `organizer` | **SURVIVED** |
| `attendees` | **SURVIVED** |
| `reminders` | **SURVIVED** |

`due` is the one that hurts most: the anchor the agenda sorts on, the property
`Todo::occurs_in_range` reads, and the field the u_crawler fixture exists for.

**Not confined to tasks.** The same three probes on `Event`
(`caldir-core/src/event.rs`) also survive: `location`, `description`,
`reminders`. So this is an inherited hole, not a VTODO-specific one — but
`Todo` is new code shipping a new `==`, and spec R38/R39 is the requirement the
guard test was written for.

**What would close it**: extend `equality_mirrors_event_equality` to walk every
content field (mutate one, assert `assert_ne!`), the way the serializer is
already covered by `a_fully_populated_task_survives_serialize_and_reparse`.

---

### MED-1 — the "stale base" half of `bases_to_backfill` is untested

`caldir-core/src/connection.rs:155-181`. The doc comment claims the function
backfills bases that are "missing (legacy known-id entry) **or stale**". Two
mutations of the staleness half survive the whole workspace suite:

| Mutation | Result |
|---|---|
| `if in_sync && !base_is_current {` → `if in_sync {` (`connection.rs:176`) | **SURVIVED** |
| `Some(Some(base)) => &**base == remote,` → `Some(Some(_)) => true,` (`connection.rs:172`) | **SURVIVED** |

The second is the one that matters: it makes a stale base *never* refreshed.
`update_direction` then compares both sides against a base that matches
neither, reads `(true, true)`, and falls through to the mtime/`SEQUENCE`
tiebreak — the exact path the base mechanism exists to avoid. Only the
`None` branch is covered
(`connection::tests::diff_backfills_base_for_in_sync_legacy_known_id`,
`connection.rs:361`) and its negative
(`diff_does_not_backfill_base_for_never_synced_pair`, `connection.rs:389`).

---

### MED-2 — `#[non_exhaustive]` on `CalendarItem` has no test lock; only CI's `-D warnings` catches it

`caldir-core/src/item.rs:13-18`. Removing `#[non_exhaustive]` from
`CalendarItem` — the openness the whole ADR-01 design rests on ("Adding
`VJOURNAL` later is one more variant") — leaves all 964 tests green.

`caldir-core/tests/public_error_api.rs:167
the_three_error_enums_are_open_to_downstream_additions` was written for exactly
this class of regression and is a *good* lock: removing `#[non_exhaustive]`
from `CalendarEventError` fails it as a compile error
(`error: unreachable pattern`, `public_error_api.rs:197`). `CalendarItem` was
never added to it.

It is caught in CI, but only *incidentally*: `-D warnings`
(`.github/workflows/ci.yml:20,40`) turns the now-unreachable `_` arms in
`caldir-cli` and `caldir-provider-caldav` into errors. The documented developer
command — `just test` → `cargo test`, no `RUSTFLAGS` — stays green. A lock that
depends on a lint flag set in one of two run modes is not the lock the error
enums got.

---

### MED-3 — 8 CLI integration tests are pinned to a fixed date against a rolling window; one degrades into a vacuous pass

`caldir-cli/tests/push_exit_status.rs` and `caldir-cli/tests/sync_exit_status.rs`
seed fixtures at `DTSTART:20260901T090000Z` / `DUE:20260905T170000Z` and then
drive `caldir push` / `caldir sync` with the **default** ±365-day window
(`caldir-core/src/utils/date_range.rs:18-20`, anchored on `Local::now()`).
`agenda_exit_status.rs` is immune — it passes explicit `--from`/`--to`
(`agenda_exit_status.rs:67`).

**Proof** — shifting the fixtures two years forward (equivalent to the clock
moving two years on) in the scratch copy:

- `push_exit_status`: 3 of 4 fail —
  `a_push_holding_a_task_pushes_every_event_and_reports_the_withheld_task`,
  `an_event_only_push_still_exits_zero_and_summarises_what_it_pushed`,
  `a_push_the_provider_rejects_also_exits_non_zero`.
- `sync_exit_status`: 5 of 16 fail —
  `a_sync_holding_a_task_pushes_every_event_and_reports_the_withheld_task`,
  `an_event_only_sync_still_exits_zero_and_summarises_what_it_pushed`,
  `a_sync_the_provider_rejects_also_exits_non_zero`,
  `an_event_only_discard_still_exits_zero_and_summarises`,
  `discard_leaves_a_task_alone_and_says_so`.

Those eight fail *loudly* around 2027-09-01 — annoying, not dangerous.

**The dangerous one is the ninth.** `sync_exit_status.rs
pull_ignores_a_local_task_and_exits_zero` **kept passing** under the shift. It
asserts the local task file survives a pull. Today it passes because the
version-skew belt (`calendar_diff.rs:143-151`) withholds the delete — the
property it is named for. After the drift it will pass because the task is
simply out of window and `continue`d over at `calendar_diff.rs:130-134`. Same
green tick, different mechanism, zero coverage of the belt. This is the "passes
for the wrong reason" shape the effort has already been bitten by, on a delay
fuse.

`an_event_only_pull_still_exits_zero` has a related, present-tense weakness: it
asserts only that a *locally seeded* file survives, never that the stub's
`from-remote@test` event was actually pulled in. A pull that pulled nothing at
all passes it.

---

### LOW-1 — `CalendarListing::len()` is untested

`caldir-core/src/calendar/listing.rs:76-78`. Documented as "Number of events
that loaded (skipped files are **not** counted)". Mutating it to
`self.events.len() + self.skipped.len()` survives the whole workspace suite.
It is public API on the type whose whole purpose is not conflating the two
halves.

By contrast the sibling accessors *are* locked: `is_incomplete()` → `false`
kills 3 tests, and `into_parts` dropping the skipped half kills
`output::agenda::tests::an_unreadable_file_is_counted_against_its_own_calendar`.

### LOW-2 — the `recurring` branch of the task `--json` schema is untested

`caldir-cli/src/output/agenda_json.rs:75`. Swapping
`todo.recurrence_id.is_some()` for `todo.recurrence.is_some()` survives, so a
recurring *master* task would report `recurring: true` in the public `--json`
schema with nothing to notice. The two fully-populated fixtures
(`serializes_fully_populated_task`) and the key-stability test do not vary this
field for a task.

### LOW-3 — a dead public error variant, matched inside an immutable test

`CalendarEventError::ExpectedSingleEvent` (`caldir-core/src/calendar/event/error.rs:12`)
is constructed nowhere: `item.rs:97` builds `ExpectedSingleComponent` instead.
It survives in two places — the openness probe
(`public_error_api.rs:189`, harmless and correct) and, more awkwardly, as an
**unreachable diagnostic arm inside one of the five immutable acceptance
tests**: `caldir-core/src/calendar/event.rs:596`, in `load_reads_a_vtodo_file`.
The test still fails correctly via its catch-all `Err(err) => panic!` arm, so
nothing is weakened; the arm is simply now dead text. Flagging it because the
immutability rule forbids touching it without a ruling.

### INFO-1 — five of the six live Radicale gates are CI no-ops

`caldir-provider-caldav/src/live/radicale.rs:144 live_gate`. Without
`CALDIR_LIVE_RADICALE=1` every gate prints a SKIP line and returns, so five of
the 964 "passing" tests assert nothing in CI. This is deliberate, documented at
`radicale.rs:5-16`, and the right call.

The exception is the model to copy: `an_undated_completed_task_survives_a_pull`
(`radicale.rs:813-830`) runs its **request half unconditionally**, before the
gate. That half is real coverage everywhere — I confirmed it by mutation
(re-adding a `<C:time-range>` to the VTODO filter fails it even with the gate
closed). The other five gates have no gate-independent half.

*(Docker was available on this machine, so the live tier genuinely executed for
the caldav mutations below; the RFC 4791 §9.9 gates really did fail against a
real Radicale 3.7.6.)*

### INFO-2 — one pre-existing vacuous test, not from this branch

`caldir-core/src/calendar/state.rs:79
calendar::state::tests::loads_synced_event_ids_from_state_dir` writes a
known-id file, calls `CalendarState::load(...).unwrap()`, and asserts nothing
about what was loaded. It would pass against a `load` that returned an empty
state — i.e. it cannot fail on the subject its name names. Introduced by
`e6a2cb9` on `main`, so out of this branch's scope; recorded because the brief
asked for every test I judge cannot fail.

---

## Attack-by-attack results

### 1. Vacuous tests — one found on this branch, one pre-existing, one on a fuse

- Branch: **none found** whose expected value is derived from the code under
  test. The one self-referential assertion in the tree —
  `caldir-core/src/provider/handler.rs:221`,
  `assert_eq!(parsed["data"], json!([event.to_ics_string()]))` — is *deliberate*
  and correctly reasoned in its doc comment: the property under test is "the
  item-typed wire equals the event-only encoding", so `Event::to_ics_string` is
  the reference, and the wire codec is the thing under test. The ICS bytes
  themselves are locked elsewhere (all 18 serializer mutations below).
- On a fuse: `pull_ignores_a_local_task_and_exits_zero` (MED-3).
- Pre-existing: `loads_synced_event_ids_from_state_dir` (INFO-2).
- A scan of every `#[test]` body for the absence of any
  `assert*`/`expect_err`/`panic!`/`matches!` returned exactly three hits, two of
  which are legitimate (`request_structs_ignore_unknown_fields` asserts via
  `.expect`; `rpc::ics::tests::the_serde_with_field_is_still_event_typed` is a
  deliberate compile-time type lock, `let _field: Event = cmd.event;`).

### 2. Unfailable locks — the dangerous ones all fail correctly

Every mutation below was applied to a scratch copy and the suite run. "Caught"
lists the first tests to fail (cargo aborts remaining targets after a failing
one, so the lists are lower bounds).

**Delete inference / R48 / the version-skew belt** — `caldir-core/src/diff/calendar_diff.rs`

| # | Mutation | Result |
|---|---|---|
| M4 | `suppressed_task_deletes \|= suppressed_by_version_skew` → `\|= false` | caught (3) incl. `a_task_less_feed_does_not_delete_local_tasks` |
| M5 | `remote_reports_tasks`: `.any(` → `.all(` | caught (3) incl. `an_undated_task_with_a_known_base_and_an_empty_remote_is_not_deleted` |
| M6 | incomplete listing → `Deletes::Allowed` | caught (2): `diff_emits_no_deletes_when_a_local_file_is_unreadable`, `unreadable_local_files_suppress_all_deletes` |
| K1 | unreadable file swallowed instead of recorded (`calendar.rs:121`) | caught (2) |
| K4 | `is_incomplete()` → `false` (`listing.rs:77`) | caught (3) |

**The sync-window rule (RFC 4791 §9.9 "core's rule")** — `caldir-core/src/todo.rs:127-143`

All four arms mutated at **both edges of both bounds**, 11 mutations:
`from < due`→`<=`, `from <= start`→`<`, `to > start`→`>=`, `to >= due`→`>`,
each half of each conjunct dropped, both single-anchor arms at both edges, and
`(None, None) => true`→`false`. **11 of 11 caught**, ten of them by
`diff::calendar_diff::tests::task_window_rule_covers_all_four_date_combinations_at_both_edges_and_events_are_unchanged`
(`calendar_diff.rs:1229`). That test is the best thing in this suite: 17 table
rows with **literal** expected booleans, anchors built as absolute UTC offsets
from fixed bounds (`offset_from`, `calendar_diff.rs:1213`), plus an
unconditional-`true` check against a disjoint 1990 window, plus explicit
`COMPLETED`/`CREATED`-are-never-consulted rows, plus a VEVENT control that
proves the event arm still goes through RRULE expansion.

**`PartialEq`** — see HIGH-1. The *helpers* are well locked:

| Mutation (`caldir-core/src/event/equality.rs`) | Result |
|---|---|
| `x_properties_eq` → `a == b` (order-dependent) | caught (3) |
| `x_properties_eq` drops the length check | caught |
| `attachments_eq` drops the length check | caught |
| `attachments_eq` → whole-struct `==` (volatile params) | caught (2) |

Field *participation* is what is not locked.

**CalDAV comp-filters** — `caldir-provider-caldav/src/caldav/ops/list_events.rs`

| # | Mutation | Result |
|---|---|---|
| C1 | VTODO filter regains a `<C:time-range>` | caught: `the_vtodo_report_body_contains_no_time_range` **and** `live::radicale::an_undated_completed_task_survives_a_pull` |
| C2 | second query becomes a VEVENT query | caught (3) incl. `both_reports_resources_reach_the_caller` |
| C3 | `fetch_events` consumes only the first REPORT | caught: `both_reports_resources_reach_the_caller` (the M7 lock, correctly driven from raw ICS over a wiremock double) |
| C4 | `decode_resources` reverts to drop-on-error | caught: `one_undecodable_resource_fails_the_whole_listing` |

**Arity rules** — `caldir-provider-caldav/src/caldav/ops.rs` and `caldir-core/src/item.rs`

| # | Mutation | Result |
|---|---|---|
| C5 | `top_level_components`: `depth == 2` → `>= 2` (would brick every task with a `VALARM`) | caught |
| C6 | `VTIMEZONE` no longer exempt from the PUT guard | caught |
| C7 | PUT guard accepts 0 **or** 1 components | caught: `a_component_less_vcalendar_is_never_put` |
| C8 | `component_name(Todo)` → `"VEVENT"` (points a task's PUT at somebody's meeting) | caught (2) |
| C9 | `location_at_href` ignores the kind | caught: `find_event_location_is_component_aware_on_both_paths` |
| M8 | `IGNORED_COMPONENTS` no longer exempts `VTIMEZONE` | caught (2) |
| M9 | zero-supported-components no longer errors (the shipping blocker's neighbour) | caught (7) |
| P1 | `into_event` silently fabricates an `Event` from a task | caught (4) |
| P2 | `into_event`'s error payload changes | caught (4) |

**`withhold_task_pushes`** — `caldir-core/src/diff/calendar_diff.rs:293-297`

| # | Mutation | Result |
|---|---|---|
| M1 | always reports 0 withheld | caught (8) |
| M2 | retains everything (tasks reach the wire) | caught (8) |
| M3 | `discard_outgoing` stops zeroing the count | caught: `a_read_only_calendar_reports_no_withheld_tasks` |
| A8 | `withheld_warning` never fires | caught (4) |
| A10 | `render_discard` drops the notice | caught: `discard_leaves_a_task_alone_and_says_so` |
| A11 | the notice stops surviving an empty outgoing set | caught (2) |
| K10 | read-only no longer discards outgoing | caught (2) |

`connection::tests::a_read_only_calendar_reports_no_withheld_tasks`
(`connection.rs:314`) deserves a note: I checked it is **not** vacuous — M3
proves the fixture really produces a non-zero count that `discard_outgoing` has
to clear.

**Identity / the `VTODO:` sigil** — `caldir-core/src/event/instance_id.rs`

| # | Mutation | Result |
|---|---|---|
| I1 | sigil dropped on write | caught (9) |
| I2 | sigil parsed back as an event | caught (3) |
| I3 | sigil retained inside the UID | caught (2) |
| I4 | `EventInstanceId` stops deriving `PartialEq` | compile error (caught) |
| K7 | `events_by_instance_id` keyed on UID alone | caught (2) |

**Exit codes** (`push`/`sync`/`pull`/`discard`/agenda) — all 6 caught, by the
`tests/*_exit_status.rs` integration tier driving the real binary:
`bail!` → `eprintln!` in each of the four commands (caught: 2-4 tests each);
sync no longer counting a failed calendar (caught: 3);
`Agenda::exit_status` downgraded to a warning (caught: the unit test **and**
`an_unreadable_calendar_is_named_on_the_agenda_and_exits_non_zero`, verified
separately).

**Serializer field emission** — `caldir-core/src/todo/to_icalendar.rs`, 18
mutations dropping every emitted property in turn plus the two
RFC-default omissions. **18 of 18 caught**, mostly by
`todo::tests::a_fully_populated_task_survives_serialize_and_reparse` and
`todo::tests::dropped_property_names_are_exactly_the_documented_list`. Note the
asymmetry with HIGH-1: what the serializer *writes* is fully locked; what `==`
*reads* is not.

**Agenda ordering / `IncompleteCause`** — `caldir-cli/src/output/agenda.rs`, 7
mutations (exit_status keyed on the wrong cause; `Unlistable` recorded as
`UnreadableFiles(0)`; per-file skips never recorded; `kind_ordinal` inverted;
`anchor` preferring `DTSTART` over `DUE`; undated sorted first). **7 of 7
caught.**

**`doctor` duplicate bucketing** — 3 mutations (bucket ignores the kind; task
duplicates rendered with the event sentence; threshold off by one). **3 of 3
caught.**

### 3. Happy-path bias — mostly absent, two gaps

Error paths are unusually well covered: unreadable files, unlistable
calendars, provider rejection at both the pre-flight and mid-loop, undecodable
resources, mid-loop partial failure with state flush, mass-delete guard, and
the "no supported component" family all have dedicated tests, and every
mutation I aimed at them was caught. Boundaries are covered at both edges for
the window rule, which is the place it matters most.

The two gaps are LOW-2 (`recurring` for a task) and the untested `Some(Some(stale))`
branch of MED-1 — both *branch* gaps, not missing error paths.

### 4. Mocks that test nothing — the blind pattern is gone from the places that mattered

`MockProvider::reply` (`caldir-core/src/provider/mock_provider.rs:24`) is still
used at ~30 sites, but it is no longer blind at any of them, because in every
case the assertion is on the *other* side of a real encode/decode round trip
(domain → ICS → wire → ICS → domain), so a codec that lost a field changes the
outcome. I verified this directly: the serializer drop mutations above kill
`connection::tests::syncing_twice_against_a_fake_remote_yields_an_empty_second_diff`,
which is driven through `reply`.

`reply_raw_data` is used where the brief says it should be:
`connection.rs:985`, `:1541`, `:1582` — the version-skew, churn, and
undecodable-payload cases. `both_reports_resources_reach_the_caller`
(`list_events.rs:449`) goes further and drives a real HTTP double from raw ICS.

**No remaining blind mock found.**

### 5. The five immutable acceptance tests — no assertion drifted

Diffed each body against `wip-snapshot` (`c5973ae`). Every change is a
mechanical accessor rename with the compared value byte-identical, which is
exactly what the ADJUDICATION ruling permits:

| Test | Change | Verdict |
|---|---|---|
| `event::tests::from_ics_str_parses_a_vtodo_document` (`event.rs:1089`) | `Event::from_ics_str`→`CalendarItem::from_ics_str`; `.uid`→`.uid()`; `.summary.as_deref()`→`.summary()` | **compliant** — `parsed.len()==1`, `"assignment-123@u-crawler"`, `"Assignment 3 - Recursive descent parser"` all unchanged |
| `event::tests::from_single_ics_str_parses_a_vtodo_document` (`event.rs:1117`) | same, plus `UnexpectedEventCount`→`UnexpectedComponentCount` in the `Err` arm | **compliant** — the rename is explicitly permitted; the `panic!` body is untouched |
| `calendar::event::tests::load_reads_a_vtodo_file` (`calendar/event.rs:585`) | `cal_event.event().summary.as_deref()`→`cal_event.item().summary()` | **compliant**; see LOW-3 for the now-dead `ExpectedSingleEvent` arm |
| `calendar::tests::events_lists_a_vtodo_file_alongside_its_vevent_sibling` (`calendar.rs:1260`) | `ce.event().uid.as_str()`→`ce.item().uid().as_str()`; `.events()` now yields a listing so `into_events()` is added | **compliant** — `vec!["assignment-123@u-crawler","window-123@u-crawler"]` unchanged |
| `caldir::tests::aggregating_all_calendars_survives_a_vtodo_calendar` (`caldir.rs:424`) | `e.summary`→`e.summary().map(String::from)` | **compliant** — the two-summary `assert_eq!` unchanged |

No test was renamed. No expected value moved. No assertion was removed or
reordered into irrelevance.

### 6. TZ fragility — none found

Full workspace suite run under **18 timezones** beyond the CI matrix, all
clean:

`Pacific/Kiritimati` (+14), `Etc/GMT-14`, `Etc/GMT+12`, `Pacific/Midway` (−11),
`Pacific/Apia`, `Pacific/Chatham`, `Pacific/Norfolk`, `Pacific/Auckland`,
`Australia/Lord_Howe` (30-min DST), `Asia/Kathmandu` (+05:45),
`Pacific/Marquesas` (−09:30), `America/St_Johns` (−03:30), `Asia/Tehran`,
`Asia/Jerusalem`, `Africa/Casablanca` (Ramadan DST), `America/Santiago`,
`America/Sao_Paulo`, `Antarctica/Troll`.

The previously-reported UTC+13/+14 and UTC−11 failures are repaired. The window
test's use of absolute UTC anchors (`offset_from`, `calendar_diff.rs:1210-1215`)
with an explicit comment about why is the right pattern and appears to have been
applied consistently.

**Recommendation**: add one extreme zone to the CI matrix
(`Pacific/Kiritimati` or `Etc/GMT-14`) — it costs one runner and closes the gap
that let the earlier two through.

### 7. The newest code — weighted, and it holds up

Mutation coverage of the last ~15 commits:

- **never-outgoing redesign** (`withhold_task_pushes`, `discard_outgoing`,
  `render_outgoing`): 7 mutations, 7 caught.
- **`IncompleteCause`** (`agenda.rs:45`): 4 mutations, 4 caught — including
  the wording trap the type exists for (`Unlistable` recorded as
  `UnreadableFiles(0)` is caught by
  `a_calendar_that_cannot_be_listed_is_reported_not_silently_dropped`).
- **`#[non_exhaustive]` sweep**: 1 of 2 caught — see MED-2.
- **`IGNORED_COMPONENTS` / `load_with_ignored`** (`item.rs:22,71`): 2
  mutations, 2 caught; the sibling-tolerance regression suite
  (`item.rs:481-547`) is thorough, covering `VFREEBUSY`, `VJOURNAL`, a stray
  top-level `VALARM` and an `X-` component, plus both arity rules and the
  `VTIMEZONE` exemption on both sides.
- **exit-code changes across `push`/`sync`/`pull`/`discard`**: 6 mutations, 6
  caught — but see MED-3 for the date fuse under them.

---

## Every test I judge cannot fail

1. `caldir-core/src/calendar/state.rs:79
   calendar::state::tests::loads_synced_event_ids_from_state_dir` — asserts
   nothing about the loaded state; passes against an empty `load`. **Pre-existing
   (`e6a2cb9`, on `main`), not this branch.**
2. `caldir-provider-caldav/src/live/radicale.rs` — five of six gates
   (`a_u_crawler_task_round_trips_...`, `get_pass_two_equals_get_pass_one`,
   `pushing_a_same_uid_pair_...`, `the_server_injects_no_non_x_properties_onto_a_task`,
   `renaming_a_local_file_updates_the_same_resource_...`) assert nothing in CI,
   by design. Only `an_undated_completed_task_survives_a_pull` has a
   gate-independent half.
3. `caldir-cli/tests/sync_exit_status.rs pull_ignores_a_local_task_and_exits_zero`
   — **will become** unfailable-on-its-subject around 2027-09-05 (MED-3).
   Proven by clock-drift simulation.
4. `caldir-core/src/calendar/event.rs:596` — the
   `Err(CalendarEventError::ExpectedSingleEvent { .. })` arm inside
   `load_reads_a_vtodo_file` is unreachable (LOW-3). The *test* still fails
   correctly; the arm is dead.

Nothing else in the 964 was found that cannot fail.

---

## Appendix — mutation ledger

111 mutations, 92 caught (83%), 19 survived. Mutations were aimed at suspected
weak spots, not sampled uniformly, so this ratio understates the suite.

| Area | Run | Caught / Total |
|---|---|---|
| diff: suppression, deletes, direction, arity | M1–M9 | 9 / 9 |
| `Todo::occurs_in_range`, both edges of all four arms | T1–T11 | 11 / 11 |
| CalDAV comp-filters, PUT guard, kind-aware lookup | C1–C9 | 9 / 9 |
| CLI agenda ordering + `IncompleteCause` + diff warnings | A1–A11 | 10 / 10 |
| `#[non_exhaustive]` | N1–N2 | 1 / 2 |
| `EventInstanceId` sigil & identity | I1–I4 | 4 / 4 |
| `Todo` `PartialEq` field participation | 15 fields | 4 / 15 |
| `equality.rs` helpers + `Event` field participation | E1–E7 | 4 / 7 |
| `todo/to_icalendar.rs` field emission | S1–S18 | 18 / 18 |
| `doctor` duplicate bucketing | D1–D3 | 3 / 3 |
| calendar / listing / connection internals | K1–K11 | 8 / 11 |
| `--json` schema + `into_event` | J1–J4, P1–P2 | 5 / 6 |
| exit-code plumbing | X1–X6 | 6 / 6 |

Plus: 18-timezone full-suite sweep (all clean), and two clock-drift simulations
on the CLI integration tier (9 date-sensitive tests identified, 1 of them
silently).

All mutations were applied to a scratch copy of the tree. The repository was
never modified; no `git checkout` or `git stash` was run.
