# Provider write sync — ship verification

**Branch:** `vtodo-support` · **HEAD at verification time:** `fef6720` · **Base:** `main` (`2ed74fa`)
**Working tree:** **not clean throughout this session** — other ticket work landed on this branch
concurrently with this document being written (`caldir-core/src/provider/capabilities.rs` and
`caldir-core/src/rpc.rs` were mid-edit at various points; HEAD has since advanced past `fef6720`).
No file this document cites as evidence was among the uncommitted changes at the time its row was
checked, and none of the concurrent commits touch `caldir-provider-caldav`, `caldir-provider-google`
or the diff/capability code this document's rows depend on — but this document reflects the tree at
`fef6720`, not necessarily the branch's tip when you read it. Re-run the cited command against your
own tree before trusting a row if HEAD has moved.

A green suite is not evidence in this document. Each row below cites a test name, a file symbol, or
a command that was actually run, per the rule `docs/vtodo/verification.md` establishes for this
project. Nothing here is rounded up: PARTIAL rows say exactly what's missing.

### Result rules

- **PASS** — the requirement as written is met, with evidence actually run or read at HEAD.
- **PARTIAL** — some of it is met; the row states exactly what's missing.
- **FAIL** — not met.
- **REFUSED** — shipped behaviour deliberately departs from the requirement text for a reason
  recorded here.

---

## 1. Acceptance table

### 1.1 CalDAV acceptance criteria (§21)

| ID | Requirement | Evidence | Result |
|---|---|---|---|
| CALDAV-1 | A real u_crawler VTODO is pushed to Radicale. | `live::radicale::a_u_crawler_task_is_created_on_radicale_through_the_production_write_path` (`caldir-provider-caldav/src/live/radicale.rs`), run in this document's own live tier (§2.1). Independently reproduced by hand in §3's manual sequence: a hand-authored task pushed through the real `caldir` CLI against a real Radicale container, confirmed on the server with `curl` afterward (§3, step 1). | **PASS** |
| CALDAV-2 | Thunderbird / another independent CalDAV client recognizes it as a task, not an event. | Human gate — not automatable (C-10). See §4. | **PARTIAL** — procedure and expected result are recorded; the check itself was not performed in this session, since no GUI client is available in this environment. Whoever runs it next records the verdict directly into §4. |
| CALDAV-3 | Changing SUMMARY locally updates the same remote object. | `live::radicale::updating_summary_due_and_status_updates_the_one_resource` (`caldir-provider-caldav/src/live/radicale.rs`) — asserts the same resource href before and after, with the new `SUMMARY` on the server. | **PASS** |
| CALDAV-4 | Changing DUE locally updates the same remote object. | Same test, same file, `DUE` half of the same assertion. | **PASS** |
| CALDAV-5 | Changing STATUS to COMPLETED is reflected remotely. | Same test's `STATUS` half, plus `live::radicale::a_completed_and_an_undated_task_survive_the_production_write_paths` (same file) for the completed-task write path specifically. | **PASS** |
| CALDAV-6 | Deleting the local VTODO deletes the remote resource when sync semantics authorize deletion. | `live::radicale::deleting_the_local_task_removes_the_radicale_resource` (`caldir-provider-caldav/src/live/radicale.rs`). | **PASS** |
| CALDAV-7 | A second sync after every operation reports no changes. | `live::radicale::get_pass_two_equals_get_pass_one` and `live::radicale::a_created_task_pulled_twice_yields_the_same_task_and_a_repush_changes_nothing` (both `caldir-provider-caldav/src/live/radicale.rs`), plus the sequence half reproduced by hand: §3's manual `caldir sync` run twice after the push, printing `No changes` both times (and a third time, run again to be sure — §3). | **PASS** |
| CALDAV-8 | Existing VEVENT sync tests remain green. | `cargo test -p caldir-provider-caldav`, run in this session: **103 passed, 0 failed** across lib + main-bin targets (78 lib unit/mocked/live-gated tests, 25 more in `session`/`remote_config` modules counted separately by the harness's per-binary split — see raw output in §2.2). No VEVENT test changed behaviour or assertion. Also directly: `live::radicale::task_writes_leave_a_sibling_event_in_the_same_collection_untouched` proves a task write leaves a sibling event's server bytes untouched. | **PASS** |

### 1.2 Google acceptance criteria (§22)

`P5-07` (Google task-write capability) has landed on this branch. `cargo test -p caldir-provider-google`
was run in this session: **127 passed, 0 failed** (mocked/unit tier only — the credential-requiring
live tier is separate, see §5). Rows below cite what exists at HEAD; none of them were exercised
against a real Google account in this session (§5 explains why and how to run that tier).

| ID | Requirement | Evidence | Result |
|---|---|---|---|
| GOOGLE-1 | Google connection can discover task lists separately from calendars. | `task_list_calendar_configs` (`caldir-provider-google/src/commands/list_calendars.rs`) distinguishes calendar and task-list resources; no test isolates this function directly, but its output shape is proven by GOOGLE-2's round-trip test. | **PARTIAL** — no test isolates task-list discovery itself, only what it produces |
| GOOGLE-2 | A task list can be connected as a caldir local calendar. | `a_task_list_config_round_trips_through_remote_config_params` (`caldir-provider-google/src/remote_config.rs`) — confirmed present. | **PASS** — mocked; not exercised against a live account |
| GOOGLE-3 | Remote Google Task becomes local VTODO. | `a_google_task_becomes_a_todo_never_an_event` (`caldir-provider-google/src/google_tasks/from_google.rs`) — confirmed present. | **PASS** — mocked only |
| GOOGLE-4 | Local representable VTODO becomes Google Task. | `the_faithful_set_reaches_the_google_task` (`caldir-provider-google/src/google_tasks/to_google.rs`) — confirmed present. | **PASS** — mocked only |
| GOOGLE-5 | SUMMARY/title round-trips. | `title_and_notes_become_summary_and_description` (`from_google.rs`); `summary_and_notes_round_trip_faithfully` (`caldir-provider-google/src/google_tasks/policy.rs`). | **PASS** — mocked only |
| GOOGLE-6 | DESCRIPTION/notes round-trips. | Same two tests as GOOGLE-5. | **PASS** — mocked only |
| GOOGLE-7 | COMPLETED/status round-trips. | `a_completed_google_task_carries_its_status_and_time` (`from_google.rs`, confirmed present); `completed_and_needs_action_status_round_trip_faithfully`, `in_process_is_sent_as_needs_action_and_survives_locally`, `cancelled_is_sent_as_needs_action_and_never_as_completed` (`policy.rs`). | **PASS** — mocked only |
| GOOGLE-8 | DUE obeys documented Google date-only canonicalization. | `a_google_due_becomes_a_date_only_due` (`from_google.rs`, confirmed present); `the_canonical_due_is_a_fixpoint`, `no_5959z_suffix_rule_holds` (`caldir-provider-google/src/google_tasks/due.rs`). Live fixed-point proof is `create_sync_sync_reports_no_changes_for_a_timestamped_due` in `caldir-provider-google/src/live/google_tasks.rs` — **credential-requiring, unrun** (§5). | **PARTIAL** — mocked half fully proven; the live proof against Google's actual canonicalization has never executed in any environment |
| GOOGLE-9 | Google-generated task ID survives create/update/delete lifecycle. | `the_task_id_is_an_x_property_and_never_the_uid` (`from_google.rs`, confirmed present) proves the carrier; live lifecycle proof is `create_sync_sync_yields_exactly_one_remote_task` and `delete_sync_reports_absent` in `caldir-provider-google/src/live/google_tasks.rs` — **credential-requiring, unrun** (§5). | **PARTIAL** — same gap as GOOGLE-8 |
| GOOGLE-10 | A second sync reports no changes after canonicalization. | `the_canonical_due_is_a_fixpoint` (`due.rs`, mocked) proves the DUE half only. Live proof is `create_sync_sync_reports_no_changes`, `create_sync_sync_reports_no_changes_for_a_timestamped_due`, `update_sync_sync_reports_no_changes` in `caldir-provider-google/src/live/google_tasks.rs` — **credential-requiring, unrun** (§5). | **PARTIAL** — written, never executed against a real account |
| GOOGLE-11 | Unsupported VTODO semantics are never silently represented incorrectly. | `a_recurring_task_is_refused_with_a_named_semantic_limitation`, `a_warning_is_emitted_when_a_user_authored_change_cannot_be_represented`, and the survives-the-push family (`percent_complete_survives_the_push_and_is_never_sent`, `priority_survives_the_push_and_never_enters_title_or_notes`, `url_survives_the_push_and_is_never_appended_to_notes`, `dtstart_survives_the_push_and_is_never_folded_into_due`, `attendees_and_organizer_survive_the_push_and_are_never_sent`, `attachments_survive_the_push_and_are_never_linkified_into_notes`, `alarms_survive_the_push_and_no_google_reminder_is_invented`, `unknown_x_properties_survive_the_push`) — all `caldir-provider-google/src/google_tasks/policy.rs`; `a_recurring_task_is_refused_with_a_named_semantic_limitation` confirmed present. Also documented in `docs/vtodo/provider-sync/providers/google-tasks.md`'s unsupported-field table (`P5-06`). | **PASS** — mocked only |
| GOOGLE-12 | Existing Google Calendar VEVENT synchronization is unchanged. | `the_event_capabilities_are_unchanged_by_this_flag`, `a_calendar_remote_advertises_no_task_support` (both `caldir-provider-google/src/commands/capabilities.rs`, confirmed present); the 127-test mocked suite (run this session) includes the pre-existing calendar tests, all green. | **PASS** |
| GOOGLE-13 | Missing Tasks OAuth scope produces an actionable reauthorization error. | `Session::require_tasks_scope` (`caldir-provider-google/src/session/types.rs`) and `tasks_scope_warning` (`caldir-provider-google/src/commands/list_calendars.rs`), proven by `a_missing_tasks_scope_produces_a_warning_naming_the_account_and_the_command`, run in this session: **1 passed**. Exact message, read from source: `"Google account {account} is not authorized for Google Tasks. Run \`caldir connect google\` to reauthorize and grant access."`, prefixed `warning: ` for stderr, and discovery still returns calendars (the warning is additive, not a hard failure). | **PASS** |

### 1.3 Cross-provider acceptance criteria (§23)

| ID | Requirement | Evidence | Result |
|---|---|---|---|
| CROSS-1 | The same local core Task type is used for CalDAV and Google. | Both providers exchange `caldir_core::CalendarItem`/`Todo` over the RPC boundary (`caldir-core/src/rpc.rs`, `caldir-core/src/item.rs`); neither provider defines its own task type. | **PASS** |
| CROSS-2 | No caldir-core match on provider name is introduced to implement task semantics. | `grep -rn 'match .*provider_slug\|match provider' caldir-core/src` and `grep -rn '== "google"\|== "caldav"' caldir-core/src`, both run this session: **no matches** for task-semantics branching (the one unrelated hit is `provider/error.rs`'s filename-prefix message, not behaviour). Capability negotiation (`ProviderCapabilities`) is the seam both providers speak through instead. | **PASS** |
| CROSS-3 | Provider capability negotiation determines whether task writes occur. | `ProviderCapabilities`/`Capability` (`caldir-core/src/provider.rs`), consumed by `CalendarDiff` to compute `withheld_task_pushes`/`suppressed_task_deletes` (`caldir-core/src/diff/calendar_diff.rs`); confirmed tests `a_task_change_is_outgoing_when_task_writes_are_advertised`, `a_task_change_is_withheld_when_task_writes_are_not_advertised` (same file) — neither test nor the production code inspects which provider produced the capabilities. | **PASS** |
| CROSS-4 | Out-of-tree providers can implement tasks through documented shared APIs. | `docs/vtodo/provider-author-guide.md` (`P6-01`) documents the write lifecycle for a provider outside this repository, without requiring the reader to read CalDAV or Google source. Proven executable against a real pre-capability binary by `a_pre_capability_binary_yields_a_version_error_for_the_capabilities_verb` and `a_pre_capability_binary_still_serves_every_original_verb` (`caldir-core/src/provider.rs`, both observed passing in the 656-test `caldir-core --lib` run, §2.2), using a real `#!/bin/sh` subprocess fixture (`caldir-core/src/test_utils.rs`) standing in for an out-of-tree binary. The guide's full content was not re-audited here — carried from `P6-01`. | **PASS** — guide content carried from `P6-01`, not re-derived; the backward-compat mechanism it depends on is independently confirmed |
| CROSS-5 | An unsupported provider never causes deletion or corruption of a local VTODO. | `suppressed_task_deletes`/`withhold_task_pushes` (`caldir-core/src/diff/calendar_diff.rs`) gate every task delete and push on the remote's advertised `tasks.read`/`tasks.write`. Confirmed tests: `an_absent_tasks_read_capability_keeps_the_belt_on` (same file). Locked by CalDAV's own fail-closed capability tests (`caldir-provider-caldav/src/commands/capabilities.rs`) and reproduced live by `task_writes_leave_a_sibling_event_in_the_same_collection_untouched`. | **PASS** |
| CROSS-6 | A VTODO is never converted into VEVENT. | `CalendarItem` is an enum, never a common struct (`caldir-core/src/item.rs`); `into_event()` returns `Err`, never a synthesized `Event`. No provider-side conversion exists in either `caldir-provider-caldav` or `caldir-provider-google`; `a_calendar_remote_still_advertises_no_task_support` (`caldir-provider-google/src/commands/capabilities.rs`, confirmed present) additionally prevents a Calendar remote from ever being offered a task write in the first place. | **PASS** |

---

## 2. Commands actually run in this session

### 2.1 The Radicale live tier

```sh
CALDIR_LIVE_RADICALE=1 cargo test -p caldir-provider-caldav -- live::radicale --test-threads=1
```

Actual output, this session, image `tomsquest/docker-radicale@sha256:45f92956a16dc139ab741a56612082723bfd09977b4b97764760532767be7448`:

```
running 14 tests
test live::radicale::a_completed_and_an_undated_task_survive_the_production_write_paths ... ok
test live::radicale::a_created_task_pulled_twice_yields_the_same_task_and_a_repush_changes_nothing ... ok
test live::radicale::a_same_uid_event_and_task_in_separate_collections_stay_distinct_through_a_task_write ... ok
test live::radicale::a_u_crawler_task_is_created_on_radicale_through_the_production_write_path ... ok
test live::radicale::a_u_crawler_task_round_trips_through_radicale_preserving_identity_and_semantics ... ok
test live::radicale::an_undated_completed_task_survives_a_pull ... ok
test live::radicale::deleting_the_local_task_removes_the_radicale_resource ... ok
test live::radicale::get_pass_two_equals_get_pass_one ... ok
test live::radicale::pushing_a_same_uid_pair_into_one_collection_surfaces_an_error ... ok
test live::radicale::renaming_a_local_file_updates_the_same_resource_and_never_a_second_one ... ok
test live::radicale::task_writes_leave_a_sibling_event_in_the_same_collection_untouched ... ok
test live::radicale::the_production_fixture_round_trips_through_the_write_paths ... ok
test live::radicale::the_server_injects_no_non_x_properties_onto_a_task ... ok
test live::radicale::updating_summary_due_and_status_updates_the_one_resource ... ok

test result: ok. 14 passed; 0 failed; 0 ignored; 0 measured; 64 filtered out; finished in 39.88s
```

No `SKIP live::radicale::` line appeared — Docker 29.7.2 and curl 8.21.0 were both present and the
gate ran for real, not as a no-op.

### 2.2 The full CalDAV and Google mocked suites

```sh
cargo test -p caldir-provider-caldav
```
`test result: ok. 78 passed; 0 failed; …` (lib target, includes the 14 live-gated tests above,
which run as no-ops here without the env var — this specific invocation was run without
`CALDIR_LIVE_RADICALE` set) and `test result: ok. 25 passed; 0 failed; …` (main-bin target:
`session`/`remote_config` unit tests).

```sh
cargo test -p caldir-provider-google
```
`test result: ok. 127 passed; 0 failed; …`

```sh
cargo test -p caldir-core --lib
```
`test result: ok. 656 passed; 0 failed; …`

This document does not claim `cargo test --workspace` was run end-to-end in this session — only
the three crates directly load-bearing for this ticket's rows were exercised. §24's full gate list
(`cargo fmt --check`, `cargo clippy --workspace --all-targets`, `cargo test --workspace`) is
`P6-04`'s mechanical-regression territory, not re-run here.

---

## 3. The §25 Baphomet sequence

Reproduced by hand against a real, throwaway Radicale 3.7.6 container (same digest as §2.1), a
task-capable collection (`supported-calendar-component-set` = `VTODO` only), and the real `caldir`
and `caldir-provider-caldav` binaries built from this branch's HEAD — not the live-tier's own test
harness, which never invokes the `caldir` CLI itself. No Baphomet hostname, account or credential
appears below; the server, calendar name and credentials shown are this session's own disposable
container.

| # | Command | Expected (§25, verbatim) | Observed |
|---|---|---|---|
| 0 | pre-feature `caldir push`, run against the retired binary | `⚠ N task changes not pushed:` / `tasks are pull-only` | Reproduced by running the actual pre-`P2-05` test at its commit (`c0f98ed`, the parent of `2853773`, the commit that retired the string): `cargo test -p caldir-cli output::diff::tests::a_withheld_task_push_is_reported_beneath_the_pushed_events` → **1 passed**, asserting the exact rendered line `   ⚠ 1 task change not pushed: tasks are pull-only — caldir cannot push tasks to a remote yet.` This is the retired string §25 refers to; `P2-05` replaced it with the provider-naming message below. Divergence from §25's literal text: §25 doesn't show the full sentence past "pull-only"; the shipped (then retired) string continued "— caldir cannot push tasks to a remote yet." |
| 1 | `caldir sync` (first run, current HEAD) | `N task changes pushed` | **Divergence from §25's literal text:** the shipped line is `Pushed: 1 created, 0 updated, 0 deleted` (from `caldir-cli/src/commands/sync.rs`), not `N task changes pushed` — the count is a total across the calendar, not task-specific, and appears after the per-calendar diff body. Full observed stdout:<br>`📅 tasksonly`<br>`   Local changes (to push):`<br>`   + P6-05 manual verification task  Tue Sep 1, 13:00`<br>(blank line)<br>`Pushed: 1 created, 0 updated, 0 deleted` |
| 2 | `caldir sync` (second run) | `No changes` | Observed, verbatim: `📅 tasksonly` then `   No changes` (dimmed). Matches §25 exactly. |
| — | `caldir sync` (third run, run again to be sure per this ticket's own instruction) | `No changes` | Same output as run 2, confirming no churn. |
| 3 | independent client | the item appears **as a task** | See §4 — human gate, not run in this session. |

Server-side confirmation (not part of the CLI's own output, cross-checked independently with
`curl`, never caldir's own HTTP client): `GET` on the pushed resource returned a `VTODO` component
(`BEGIN:VTODO` / `SUMMARY:P6-05 manual verification task` / `DUE:20260901T170000Z`), and the
collection's `PROPFIND` listing showed exactly one child resource after the push and the same one
resource after both re-syncs — no duplicate was created by the second or third run.

**Reported defect, not fixed here (non-goal, per this ticket):** §25's literal `N task changes
pushed` / `N task changes not pushed` (pre-feature half) do not match either the pre-feature or the
post-feature shipped strings. The pre-feature string additionally named the reason
("tasks are pull-only — caldir cannot push tasks to a remote yet") where §25 only shows the first
clause. This is a `P2-05`/spec-text reconciliation gap, not a regression to fix under this ticket.

---

## 4. `CALDAV-2` human gate (C-10)

This is not automatable and no `#[test]` claims to automate it — grep confirms:
`grep -rn 'fn.*thunderbird\|fn.*independent_client' caldir-provider-caldav/src` returns nothing.

**Procedure:**
1. Complete §3 step 1 so exactly one known `VTODO` exists on the server (done, this session, on a
   throwaway container since torn down — the resource itself no longer exists to inspect).
2. In an independent CalDAV client — Thunderbird with the collection subscribed as a CalDAV
   calendar is the named example; any client that renders tasks separately from events qualifies —
   open the collection.
3. **Expected:** the item appears in the client's task/to-do list with its `SUMMARY`, due date and
   completion state, and appears nowhere in the calendar/event grid.
4. Record here: client name and version, date of the check, what was seen, and the verdict.

**Status of this row:** **not performed in this session** — no GUI CalDAV client is available in
this environment. The container used for §3 was disposable and has been torn down
(`docker rm -f`); a fresh one plus a Thunderbird (or equivalent) profile is needed to close this
row. Whoever runs it next fills in the four fields above directly in this section; do not mark
`CALDAV-2` PASS anywhere in §1.1 until that's done.

---

## 5. Gated and credentialed tests

| Test | Command | Prerequisites | Expected result |
|---|---|---|---|
| All 14 gates in `caldir-provider-caldav/src/live/radicale.rs` | `CALDIR_LIVE_RADICALE=1 cargo test -p caldir-provider-caldav -- live::radicale --test-threads=1` | `CALDIR_LIVE_RADICALE=1`, a working `docker` and `curl` on `PATH`. Gated at **runtime** via `live_enabled()`/`live_gate`, never `#[ignore]` — the verify command in this ticket's own spec doesn't pass `--ignored`, so an `#[ignore]`d test would silently never run. | 14 passed, 0 failed, no `SKIP live::radicale::` lines. **Run in this session** — see §2.1 for the actual output. |
| `create_sync_sync_yields_exactly_one_remote_task` | `CALDIR_LIVE_GOOGLE=1 CALDIR_LIVE_GOOGLE_ACCOUNT=you@gmail.com CALDIR_LIVE_GOOGLE_TASK_LIST=<id> cargo test -p caldir-provider-google -- live::google_tasks::create_sync_sync_yields_exactly_one_remote_task` | `CALDIR_LIVE_GOOGLE=1`, `CALDIR_LIVE_GOOGLE_ACCOUNT` (an account that has already run `caldir-provider-google connect`), `CALDIR_LIVE_GOOGLE_TASK_LIST` (a real, disposable list id). Gated at runtime the same way as Radicale — `caldir-provider-google/src/live/google_tasks.rs`, `live_enabled`/`live_gate`. | 1 passed — routing the same canonical item through create once and update any number of times never produces a second remote task. **Never executed in any environment** — this repository has no Google OAuth session. Credential-requiring and unrun. |
| `create_sync_sync_reports_no_changes_for_a_timestamped_due` | Same three env vars, `-- live::google_tasks::create_sync_sync_reports_no_changes_for_a_timestamped_due` | Same. | 1 passed — a create-then-sync-twice cycle against a timestamped `DUE` reaches a fixed point. **Credential-requiring and unrun.** |
| `create_sync_sync_reports_no_changes` | Same three env vars, `-- live::google_tasks::create_sync_sync_reports_no_changes` | Same. | 1 passed. **Credential-requiring and unrun.** |
| `update_sync_sync_reports_no_changes` | Same three env vars, `-- live::google_tasks::update_sync_sync_reports_no_changes` | Same. | 1 passed — an update reaches a fixed point on the second sync. **Credential-requiring and unrun.** |
| `delete_sync_reports_absent` | Same three env vars, `-- live::google_tasks::delete_sync_reports_absent` | Same. | 1 passed — a deleted task reports absent on the next sync, never resurrected. **Credential-requiring and unrun.** |

The whole Google live tier can also be run in one invocation:

```sh
CALDIR_LIVE_GOOGLE=1 CALDIR_LIVE_GOOGLE_ACCOUNT=you@gmail.com \
  CALDIR_LIVE_GOOGLE_TASK_LIST=<task list id> \
  cargo test -p caldir-provider-google -- live::google_tasks
```

None of these five were run in this session — this environment has no Google OAuth session and no
account to point `CALDIR_LIVE_GOOGLE_ACCOUNT` at. They are written and gated, not skipped by
omission: `grep -rn 'CALDIR_LIVE_GOOGLE' caldir-provider-google/src` confirms the gate exists.

---

## 6. Known gaps

- **`CALDAV-2`** is `PARTIAL`: the procedure and expected result are fully specified (§4), but the
  actual client check has not been performed in any session — no GUI CalDAV client exists in this
  environment. This is the one row a human must close by hand before shipping.
- **`GOOGLE-1`** is `PARTIAL`: task-list discovery has no test isolating the discovery function
  itself, only tests of what it produces once a task list is already configured.
- **`GOOGLE-8`, `GOOGLE-9`, `GOOGLE-10`** are `PARTIAL`: their live, credential-requiring proofs
  (§5) have never executed against a real Google account. The mocked halves are green.
- **§25's literal expected strings do not match what the CLI actually prints**, on both the
  pre-feature and post-feature side (§3). The document records the real strings; the spec text
  itself is stale and out of this ticket's scope to change (non-goal).
- **This document does not re-verify `CROSS-4`** (the provider-author-guide claim) — it's carried
  forward from `P6-01` rather than re-derived, since re-auditing that guide is outside this
  ticket's scope.
- **`cargo test --workspace`, `cargo fmt --check` and `cargo clippy --workspace --all-targets`
  were not run in this session** — only the crates this ticket's rows depend on
  (`caldir-provider-caldav`, `caldir-provider-google`, `caldir-core --lib`) were exercised. The
  full §24 gate is `P6-04`'s territory.
