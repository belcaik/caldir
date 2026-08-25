# ADR-09: Absolute backward-compatibility constraints

## Status
PROPOSED (Stage 3 candidate). Settles user DECISION-10 and system-map Q14.

## Context

VTODO touches five surfaces that outlive a release: files users already have, `.caldir/`
state, the provider RPC wire, the published `caldir-core` crate, and CLI output scripts
parse. Providers are separately installed binaries found on `PATH`
(`provider/registry.rs:22-25`), so core/provider version drift is normal, not exceptional.

Two facts shape everything below:

- **Filenames are not identity.** Identity is `EventInstanceId` read from ICS *content*
  (`event.rs:252`); `Calendar::events()` scans `*.ics` by extension only (`calendar.rs:113`).
  A rename is invisible to `CalendarDiff::compute` — it does **not** look like delete+create
  to sync. It does break `Calendar::event(slug)` addressing (`calendar.rs:122-125`), the
  documented filename convention (`specs/caldir.md:229-243`), and user scripts.
- **Delete inference needs the remote to see the item.** `outgoing Delete` fires in pass 2
  only when an item is in the *remote* list, absent locally, and known to `sync_bases`
  (`diff/calendar_diff.rs:110`). This is what makes the poison-pill fix less risk-free than
  it looks (Q14).

`caldir-core` **is published to crates.io** — `.github/workflows/release.yml:111-145` runs
`cargo publish -p "$crate"`; no `publish = false` exists; version `caldir-core/Cargo.toml:3`
= **0.14.1**, with all six siblings pinning `version = "0.14.1"`. It carries `description`,
`keywords`, `categories` — presented as a library. But `.agents/skills/bump/SKILL.md` §4
treats all crates as pre-1.0 and gives **breaking changes a minor bump**, same as features.
The public API is therefore real but *soft*: breaking it is permitted; the binding
obligation is the release ritual.

## Options considered

**A. Hard freeze.** No observable change to any surface; VTODO purely additive.
*Benefit:* zero downgrade risk. *Cost:* forbids the S5 poison-pill fix (it changes error
behaviour), the arity relaxation, and any `--json` addition — it blocks the mission.
Blast radius: none.

**B. Tiered constraints.** Inviolable (existing bytes and names, `known_event_ids` format,
the `Method` verb set, silent-drop-free decoding) / additive-only (`config.toml`, `--json`,
`CalendarConfig`) / release-ritual (`caldir-core`'s Rust API). *Benefit:* the mission is
achievable and each tier gets its own test. *Cost:* per-change judgment. Blast radius: one
`#[cfg(test)]` module per surface, zero production files.

## Decision

Adopt **Option B**. Every row below is binding. A change that violates an
**INVIOLABLE** row is rejected regardless of its benefit; a **SOFT** row may be broken
only with the named mitigation.

| # | Surface | Constraint | Tier | Testable assertion (test name) |
|---|---|---|---|---|
| C1 | Existing VEVENT `.ics` files | No VEVENT file is renamed, rewritten, or re-serialized as a side effect of this feature. `Event::base_slug()` (`event/slugify.rs:13-19`) is byte-identical for every input that is an event today. | INVIOLABLE | `event::slugify::tests::base_slug_is_unchanged_for_vevent_corpus` — table of ~12 `(Event, expected_slug)` pairs incl. all-day, recurring master, empty summary, emoji, and a >64-char summary. Plus `calendar::event::tests::syncing_a_calendar_does_not_touch_vevent_file_mtimes_or_names`: snapshot `(name, mtime, bytes)` of a VEVENT dir, run a full no-op `Connection::diff`, assert the triple is unchanged. |
| C2 | Existing VEVENT parse | An existing VEVENT file parses to a structurally `==` `Event` before and after. Educe equality (`event.rs:38,63-67`) is unchanged for events. | INVIOLABLE | `event::tests::vevent_control_round_trips_identically_after_vtodo_support` (the reproduction.md GREEN controls, kept green) + the 686-test baseline stays green in all three CI timezones. |
| C3 | `.caldir/config.toml` | New keys are `Option`, defaulted, and omitted when `None`. No key is renamed, retyped, or made required. `CalendarConfig` never gains `deny_unknown_fields`. | INVIOLABLE | `calendar::config::tests::loads_a_config_written_by_an_older_caldir` (a 0.14 `config.toml` literal) **and** `..::an_older_caldir_loads_a_config_containing_unknown_keys` (parse a config with a fabricated future key through today's struct; assert `Ok`). |
| C4 | `state/known_event_ids` | Line format stays `{uid}` / `{uid}__{rid}`, one per line, sorted, plaintext (`state/known_event_ids.rs:23-27`). It keeps being written. Todo ids are written into the *same* file. | INVIOLABLE | `calendar::state::known_event_ids::tests::todo_ids_use_the_legacy_line_format` — write a todo's id, assert the file body matches `^[^\n]+$` with no new sigil, and that `KnownEventIds::load` on a 0.14-era file yields the same set. |
| C5 | `state/bases/<sha256>.ics` (downgrade) | An old caldir must **skip, not fail on**, a base file it cannot parse. Guaranteed today by `let Ok(event) = Event::load_single(..) else { continue }` (`state/event_bases.rs:60-62`) — so the hash-filename scheme must not change and base files must remain single-component VCALENDAR documents. | INVIOLABLE | `calendar::state::event_bases::tests::an_unparseable_base_file_is_skipped_not_fatal` (pin the existing `continue`) + `..::base_filename_hash_is_stable` (already exists at `event_bases.rs:96`, keep). |
| C6 | Downgrade with VTODO files present | Old caldir hitting a VTODO `.ics` must fail **loudly and without writing**, never proceed with a partial local set. Today `calendar.rs:114`'s `?` guarantees this; it is a data-safety fuse, not only a bug. | INVIOLABLE | `calendar::tests::an_unloadable_file_never_yields_a_partial_event_list` — assert `events()` returns `Err`, or (post-S5) that the result carries the skipped-file count; assert it is never a silently short `Ok(vec)`. |
| C7 | Skipped-file → delete inference | If any `.ics` in a calendar failed to load, that sync must emit **zero** `EventChange::Delete` for that calendar. Without this, an S5-only core + a VTODO-aware provider deletes the user's tasks server-side via `calendar_diff.rs:110`. | INVIOLABLE | `diff::calendar_diff::tests::unreadable_local_files_suppress_all_deletes` — local dir with one unreadable file, remote returning that item, base recorded; assert `diff.outgoing()` contains no `Delete`. |
| C8 | RPC verbs | `Method` (`rpc/methods.rs:22-31`) gains **no** variant. No `list_todos`, no `create_todo`. Component kind travels inside the opaque ICS payload (`rpc/ics.rs:28-30`). | INVIOLABLE | `rpc::tests::method_set_is_exactly_the_six_known_verbs` (assert the serialized names) + `provider::handler::tests::an_unknown_command_is_a_parse_error` — pin the `"Failed to parse request"` shape at `handler.rs:75-78` so anyone adding a verb sees the break. |
| C9 | RPC request/response fields | Additive `Option` fields only; no `deny_unknown_fields` anywhere. | INVIOLABLE | `rpc::tests::request_structs_ignore_unknown_fields` — deserialize each of the six request types from JSON carrying a fabricated extra key; assert `Ok`. Plus a repo-level `grep` assertion: `rpc::tests::deny_unknown_fields_appears_nowhere` reading the crate source. |
| C10 | Wire decode of an unknown component | Decoding an ICS payload whose component is not understood must be a **hard error**, never a silent drop or an empty vec. A silent drop feeds C7's delete path. | INVIOLABLE | `rpc::ics::tests::decoding_an_unknown_component_errors_rather_than_dropping` — feed a `VJOURNAL`-only payload; assert `Err`, and assert `list_events` returning it fails the whole call rather than yielding a short `Vec`. |
| C11 | `caldir-core` Rust API | Semver-breaking changes (retyping `Event::start`, changing `Vec<CalendarEvent>` returns, removing a `lib.rs:15-26` export) are **allowed** but require: a 0.x **minor** bump per `.agents/skills/bump/SKILL.md` §4, matching bumps of all six dependants, and their `version = "…"` pins updated. | SOFT | `caldir-cli` and all five providers compiling against the new core (CI already does this, `-D warnings`) is the assertion. Add `ci`-level check that every `caldir-core = { … version = "X" }` pin equals `caldir-core/Cargo.toml`'s version. |
| C12 | `--json` agenda schema | Documented public (`caldir-cli/src/output/agenda_json.rs:1`). No existing key removed, renamed, or retyped. `start: String` stays non-optional, which means **an item with no DTSTART must not enter `AgendaView`** unless a new, additive shape is designed for it. | INVIOLABLE | `output::agenda_json::tests::schema_keys_are_stable` — serialize a fixture entry and assert the exact key set and each value's JSON type; the test must fail on any removal or retype. |
| C13 | CLI text output & exit codes | Existing text for existing situations is unchanged. `doctor` keeps reporting a syntactically broken VEVENT as `DoctorWarning::UnreadableEvents` (`doctor.rs:55-57`) — the S5 relaxation must not silently swallow genuinely malformed files. | INVIOLABLE | Keep `commands::doctor::tests::treats_unreadable_events_as_warnings` (already exists, `doctor.rs:73-91`) green, and add `..::a_malformed_vevent_is_still_a_warning_after_component_skipping`. |

**Spec amendment (explicit, per BRIEF):** `specs/caldir.md:211-223` "Fields We Skip"
must be amended if `PRIORITY` is carried for VTODO, and `specs/caldir.md:229-243`
"Filename Convention" must gain the undated-task rule. Both are additive edits; no
existing sentence about VEVENT filenames may change (C1).

## The two-direction wire verdict

**New core + old provider — SAFE, loud.** The old binary parses `Method` fine (no new verb,
C8) and receives an ICS string. Push a VTODO to a provider that cannot represent one and
`rpc::ics::deserialize` → `Event::from_single_ics_str` → `UnexpectedEventCount { expected: 1,
found: 0 }` (`event.rs:111-121`), returned as `{"status":"error", …}` for that one change —
a per-change failure recorded by `connection.rs:84,114`, no data loss. Additive
`CalendarConfig` fields are ignored (C9). Mitigation to make it quiet rather than merely
safe: an additive component-set field on `CalendarConfig` (S6/Q9) so core never attempts it.

**Old core + new provider — SAFE ONLY IF C10 HOLDS.** A VTODO-aware provider returning a
VTODO from `list_events` hands old core a payload its `Vec<Event>` deserializer rejects; one
bad element fails the whole vector, so the call errors and the sync aborts before any apply.
That is the correct, fail-safe outcome. The catastrophic variant is a design that silently
drops the unknown component: the remote list arrives short. Its mirror image is the real
hazard — item present remotely, absent locally because the local file was skipped — which is
C7. **Both directions hinge on one principle: unknown must be loud or quarantined, never
forgotten.**

## Q14 — does the poison-pill fix ship separately, before the VTODO work?

**For shipping separately.** It is orthogonal to every VTODO modelling decision; it
touches three call sites (`calendar.rs:114`, `calendar/event.rs:40-50`,
`agenda.rs:35`); it fixes the user's actual symptom — one VTODO anywhere blanks the
entire agenda across *all* calendars (reproduction.md §4.3) — today rather than at the
end of an eight-stage feature; it de-risks every later stage by removing the failure
mode that would otherwise mask VTODO bugs; and it is independently reviewable upstream,
which matters for a lean project taking an external PR.

**Against.** It changes existing error behaviour, which is itself a compatibility
surface (C13 exists precisely because of it): a corrupt VEVENT that today fails loudly
would begin to be skipped, and a user's real event could vanish from `caldir status`
with a warning nobody reads. Worse, and this is the argument the "low-risk" framing
misses: an S5-only core paired with a *later* VTODO-aware provider binary — a
combination that separate shipping actively creates, because providers update
independently — reaches `calendar_diff.rs:110` with the task present remotely, absent
locally, and its base known, and emits an **outgoing Delete** against the server. The
risk-free-looking fix is the one that can delete a user's tasks. It also delivers no
feature: the user's tasks still do not sync.

**Recommendation: YES, ship it separately and first — but the PR must include C7.**
The delete-suppression guard ("a calendar with any unreadable file emits no deletes
this sync") is what turns S5 from a latent data-loss vector into a genuine de-risking
step, and it costs one condition plus one test. Scope of that PR: graceful per-file
degradation, the C7 guard, the C13 doctor assertions, and the reproduction.md GREEN
controls. Explicitly **out** of that PR: any change to `Event`, `Method`, the wire, the
`--json` schema, or `base_slug`. Land it, cut a patch release, then start VTODO
modelling on top.

## Consequences

Easy: adding VTODO fields, config keys and `--json` keys; shipping S5 first; running
mixed core/provider versions. Hard: any new RPC verb (permanently closed, C8); making
`Event::start` optional without a coordinated minor bump of seven crates (C11); putting
undated items into agenda `--json` (C12). Must be tested: all thirteen assertions above,
across the three-timezone CI matrix, against the 686-test baseline.

## Backward-compatibility impact

Local files: none (C1/C2). `.caldir/` state: forward-readable by design; the downgrade
direction is safe because both state readers already skip what they cannot parse (C5) and
the event reader fails closed (C6). RPC wire: unchanged verb set, additive fields only,
both mixed-version directions verified above. Installed provider binaries: keep working
unmodified — they never see a VTODO if the component-set flag is adopted, and error
cleanly per-item if it is not.

## Rejected because

- **Option A (hard freeze):** forbids the poison-pill fix and the arity relaxation, so
  it blocks the mission while protecting a surface (error strings for already-broken
  files) that nobody depends on.
- **New RPC verb:** `Method` has no `#[serde(other)]` (`rpc/methods.rs:22-31`); every
  installed provider binary would hard-fail mid-upgrade.
- **Silent-drop decoding for unknown components:** cheap to write, and it converts a
  loud mixed-version failure into a silent one that C7's delete path can act on.

## Open sub-questions

1. **Is `caldir-core` actually consumed out-of-tree?** It is published (0.14.1) and
   presented as a library, but I found no external consumer. UNVERIFIED. Cheapest
   experiment: `curl https://crates.io/api/v1/crates/caldir-core/reverse_dependencies`
   and check download counts. If zero, C11 relaxes further and an `Option<EventTime>`
   start becomes cheap.
2. **Does C7 belong in `Calendar::events()` or in `CalendarDiff::compute`?** The guard
   needs a "this listing is incomplete" signal to cross a seam that currently carries
   only `Vec<CalendarEvent>`. Cheapest experiment: change the return to a small
   `CalendarListing { events, unreadable: Vec<PathBuf> }` and run `cargo check -p
   caldir-cli` to count call-site churn (5 core + 2 CLI sites per system-map S1).
3. **Does `caldir update` (`caldir-cli/src/commands/update.rs`) upgrade core and
   providers atomically?** If yes, the mixed-version window is narrow and the
   component-set flag is optional polish; if no, it is required. UNVERIFIED — read
   `update.rs` and `install.sh` end to end.
4. **Should the S5 PR also fix the mixed VEVENT+VTODO silent-drop** (reproduction.md
   §7: the VTODO is dropped and a rewrite loses it on disk)? It is a data-loss bug
   today, but its fix depends on the Q13 arity decision, which is not S5's to make.
