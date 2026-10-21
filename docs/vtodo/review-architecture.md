# VTODO support — Review 1 of 5: ARCHITECTURE

**Scope:** `git diff main..HEAD -- caldir-core caldir-cli caldir-provider-*`
**Question:** does the implementation match the accepted architecture, and is that
architecture sound in the code as built?
**Inputs:** ADR-01…ADR-10, `ADJUDICATION.md` (binding), `spec.md` §5/§6/§9/§10.
Every finding below was verified against the code, not against the ADRs' claims.

## Findings

| Sev | Finding | Where | Why it matters |
|---|---|---|---|
| MAJOR | A whole calendar that fails to list is **silently dropped from the agenda**. On `main` this propagated with `?` and errored loudly. It is now `let Ok(listing) = … else { continue };`, and the dropped calendar is **not** added to `incomplete` — the mechanism 15 lines above that exists precisely to say "this listing came back short". | `caldir-cli/src/output/agenda.rs:158` (vs `main`'s `agenda.rs:36`) | Same defect class QA fixed as F-6, reintroduced one level up and in the opposite direction. A permissions error on one calendar makes it vanish from `today`/`week`/`--json` with no word to the user. R77's own rationale ("a listing is never silently short") is quoted in the file at `:381` and then not applied here. |
| MAJOR | `SameUidPair`'s remedy is always wrong. `refuse_unpushable` checks `same_uid_pair` **first**; with only `Event` and `Todo`, a differing-kind pair necessarily contains a `Todo`, so `UnpushableTask` would also have fired. The user is therefore always shown *"Change one of the UIDs, or keep the pair in a local-only calendar"* — but changing the UID leaves the task unpushable under R70, so the advice cannot unblock anything. | `caldir-core/src/connection.rs:290-296`; message at `connection/error.rs:33-40` | Two guards, one reachable message, and it is the misleading one. The lock at `connection.rs:1391` only asserts the error *names the UID*, so nothing pins which of the two messages wins — a later reordering is invisible to the suite. |
| MAJOR | The domain migrated to `CalendarItem`; the **public API did not**. `CalendarEvent` stores a `CalendarItem`; `Calendar::create_event(item: impl Into<CalendarItem>)` creates tasks; `Calendar::expanded_events_in_range` returns `Vec<CalendarItem>`; `RemoteEvent(CalendarItem)`; `EventChange::Create(CalendarItem)`; `EventInstanceId`, `EventBases`, `known_event_ids`, `SkippedEvent` all key non-events. | `calendar/event.rs:12-14`; `calendar.rs:371-376,389-395`; `remote/event.rs:9`; `diff/event_change.rs:13-18`; `calendar/state/event_bases.rs:18` | This is the single thing that reads as an unfinished migration to a maintainer. Every name lies about what it holds, and the lie is in `lib.rs`'s exported surface. The 0.15.0 breaking window being used *right now* for `#[non_exhaustive]` is the only cheap moment to rename; after it ships the cost is a second breaking release. |
| MAJOR | The `#[non_exhaustive]` sweep (L8-05) covered `CalendarError`/`ComponentKind`/`CalendarItem`/`RemoteError` but **missed every enum this feature actually grew**: `EventError` (+3 variants), `CalendarEventError` (+3), `ConnectionError` (+2). None is `#[non_exhaustive]`. Separately, `EventError` remains **unnameable** from outside (`mod event;` is private; no `lib.rs` re-export) while being the payload of the public `RemoteError::UnsupportedItem` and `CalendarEventError::InvalidEvent`. | `event/error.rs:4`; `calendar/event/error.rs:6`; `connection/error.rs:7`; `lib.rs:22-30`; admitted in `tests/public_error_api.rs:60` ("both payload types are still crate-private") | ADJUDICATION's own rule: "adding a variant to a non-exhaustive-less public enum is a breaking change EVERY TIME… after 0.15.0 ships the window closes." A downstream cannot match `expected 1 event(s) in ICS, found 0` — the error R71 declares a *contract* — because it cannot name the type it lives in. |
| MINOR | **ADR-01 step 6 was never implemented.** There is no `read_shared<C: EventLike>` / `write_shared<C: EventLike>`. `todo/from_icalendar.rs` is 184 prod lines and `todo/to_icalendar.rs` 130, against the ADR's promised "~30 lines of VTODO-specific mapping, not a 110-line copy". Verbatim duplicates: the `X-` filter, `ATTACH`, `ATTENDEE`, `ORGANIZER`, `SEQUENCE`, `non_empty` (self-labelled "deliberately a copy"), `to_ics_string`, `splice_valarms_into_{vevent,vtodo}`. | `todo/from_icalendar.rs:52-58,66-82,108-113,139` vs `event/from_icalendar.rs:26-30,52-65,80-84,104-107`; `todo.rs:149-174` vs `event.rs:101-112` | The ADR priced the duplication at "~15 field declarations and two educe lines". The real bill is ~300 prod lines of behavioural code per kind, and it is a **per-kind constant** — VJOURNAL pays it again. Equality and slugs are correctly shared (below), so the drift the ADR most feared is closed; the overrun is entirely in the codec. |
| MINOR | `ItemJson::from` **panics** (`unreachable!`) on a future `CalendarItem` variant. Every other CLI wildcard degrades gracefully; this one aborts, on the `--json` render path. | `caldir-cli/src/output/agenda_json.rs:167-177` | Justified today by the path dependency, but it converts a compile error into a runtime crash the moment core and CLI versions are ever decoupled. `_ => ` a `kind: "unknown"` object with null fields would preserve C12's DEL predicate without the abort. |
| MINOR | Shipped silent VTODO drop carries a stale `TODO(L3-04)` — L3-04 landed and did not touch it. The real authority is spec R72 ("Making webcal task-aware is out of scope", covered by R49). | `caldir-provider-webcal/src/commands/list_events.rs:21-26` | Not a defect (R72 rules it out of scope; the R49 belt covers the delete path), but the annotation points at a closed ticket instead of the rule that sanctions it, so the next reader will re-litigate it. |
| MINOR | `same_uid_pair` is Speculative Generality today: no task can be pushed at all under R70, so the guard cannot fire on any path R70 does not already refuse. | `caldir-core/src/connection.rs:233-252` | Harmless as defence-in-depth, but combined with the ordering it produces the wrong message (MAJOR-2). |
| NIT | `let mut tasks = Vec::new();` receives **every non-`Event` item** via `item => tasks.push(item)`. The name is already an approximation and becomes a lie at the third kind. | `caldir-core/src/calendar.rs:392,396` | |
| NIT | `ComponentKind` derives `Default = Event`. A stray `ComponentKind::default()` silently means "this is an event". | `caldir-core/src/event/instance_id.rs:14-20` | |

## What I attacked and could not break

1. **The parse pass (ADR-01 step 5 vs B-3).** `item.rs:220-253` is one ordered pass over
   `icalendar.components`, and the adjudication's override held: there is **no `_ => skip`**.
   Unsupported components abort with the component's own name; VTIMEZONE is exempted by an
   explicit allowlist (`IGNORED_COMPONENTS`), not by a wildcard. `component_name`'s fallback
   is a naming default, not a drop. Document order is asserted, not assumed.
2. **`into_event` is not a drop.** `item.rs:155-163` returns the arity `Err`, and the abort is
   locked at three seams before any RPC is serialized (`remote.rs` create + delete,
   `connection.rs`). I looked specifically for a `_ => None` here; there is none.
3. **Identity.** C-1's ruling (`ComponentKind` in the key) is implemented and ADR-01 #3/#7
   correctly struck. Event lines in `known_event_ids` stay byte-identical; task lines take the
   `VTODO:` sigil; `bases/<sha256(id)>.ics` collision is closed structurally rather than by the
   ADR's weaker "kind-mismatched base treated as absent".
4. **One diff engine.** `calendar_diff.rs:41-190` is item-typed end to end with a single
   `compute_with`. Option D's duplicated sync engine was genuinely avoided. The R49 belt is
   derived **once** from the inputs (`:70-73`) rather than re-derived per branch, and
   `suppressed_task_deletes` is only set where a delete was actually withheld.
5. **The push pre-flight.** `refuse_unpushable` keys on `instance_id().kind() != ComponentKind::Event`
   — on the *kind*, not the Rust type — so a component added later is stopped rather than
   smuggled. It fixes F-3 properly: whole-push refusal before the first RPC, with
   `!mock.received_a_request()` and byte-for-byte local file survival asserted
   (`connection.rs:1380-1412`).
6. **Equality and slug drift (the ADR's own stated risk).** ADR-01 step 4 is done:
   `x_properties_eq`/`attachments_eq` live in `event/equality.rs:5,12` and both structs' educe
   attrs point at the same fns. `last_modified`/`sequence` are `ignore` on both.
   `summary_slug`/`time_slug` are shared (`event/slugify.rs:32,58`), so a task and an event with
   the same title slug identically. **No drift found in either.**
7. **Provider leakage into the domain model.** `Todo` carries no href, ETag, comp-filter or
   4791 concept — only doc-comment *citations* of RFC 4791 §9.9 justifying `occurs_in_range`,
   which is a legitimate correctness rationale, not a dependency. The one real leak is a
   *policy* leak in `Connection` (MAJOR-2), not a model leak.
8. **`Event` untouched (ADR-01 #2, R1/R2).** `start` is still non-optional; `occurs_in_range`,
   `has_occurrence_in_range`, `base_slug` and both codec impls keep their signatures. The only
   edits to `event.rs` are visibility changes and the removal of the now-superseded
   `from_ics_str`/`from_single_ics_str`/`load_single`. Q2 stayed answered "no".

## Did the enum earn ADR-01's measurement?

Yes, and for the reason the ADR gave rather than the one it argued. The decisive constraint is
not the 41-error experiment — it is `SyncBases`/`EventBases` being **one heterogeneous map per
calendar directory** (`event_bases.rs:18`) and `CalendarDiff::compute_with` needing structural
`==` as the "content changed?" test (`calendar_diff.rs:84,105`). A trait would have had to be
`dyn`, which loses the educe `PartialEq` that decides sync direction; a type parameter would
have had to thread through `CalendarEvent`/`RemoteEvent`/`EventChange`/`CalendarDiff`/`SyncBases`
and still meet an enum at the directory boundary. I could not construct a narrower seam that
survives both. The enum has not leaked anywhere a trait would have served better: production
matches on it in only ~10 places in core outside `item.rs`, and `item.rs` is exactly the
"~6-method enum interface" the ADR described (it grew to 11, all one-liners).

## VJOURNAL trace — ADR-01 #8 tested against the code

**The structural claim holds. The estimate does not.** Adding `VJournal` requires no change to
the diff engine's shape, the state layout, the wire protocol, the identity string format, or the
push pre-flight (which is already kind-keyed, not type-keyed). *No second redesign.* But
"one variant + one struct + one match arm" is off by an order of magnitude:

- `ComponentKind` variant + sigil in `Display` and `From<&str>` — `instance_id.rs:16,90-104`
- `CalendarItem` variant + **13 exhaustive match arms in `item.rs` alone** (`uid`, `instance_id`,
  `as_event`, `is_cancelled`, `sequence`, `into_event`, `summary`, `base_slug`, `to_ics_string`,
  `has_occurrence_in_range`, `last_modified`, `parse_document`, `component_name`) + a `From` impl
- a `todo.rs`-sized module: ~180 prod lines of parse + ~130 of serialize + slug + status.
  **This is the dominant cost and it recurs per kind**, precisely because ADR-01 step 6's shared
  codec was never built (MINOR-1). The ADR's "VJOURNAL = one struct" priced this at zero.
- 5 further core prod arms: `calendar.rs:396,407,410`, `calendar/event.rs:86`, `rpc/ics.rs`
- **caldir-cli is the weak point.** Because `CalendarItem` is `#[non_exhaustive]` and the CLI is a
  separate crate, **six wildcard arms compile silently** — `output/event.rs:26` (visible),
  `agenda.rs:102` (anchor→undated), `agenda.rs:114` and `agenda_text.rs:216` (sort ordinal),
  `agenda_json.rs:79,91` (recurring/title), `diff.rs:72` (recurrence badge). A VJOURNAL would
  render as an untitled undated line with no compile-time nudge anywhere. And
  `agenda_json.rs:174` **panics** instead.

Net: the architecture accommodates VJOURNAL; the CLI does not, and the `#[non_exhaustive]`
boundary converts what should be a compile error into either a silent under-render or a runtime
abort on `--json`.

## Verdict

**The architecture is sound as built, and the implementation matches the accepted design where
the adjudication overrode the ADR.** The three decisions that carry the feature — the
`CalendarItem` enum over a trait or a widened `Event`, the kind-carrying `EventInstanceId`
(C-1's override of ADR-01 #3/#7), and one item-typed diff engine — are all implemented as
ruled, and each is load-bearing for a reason I could verify in the code rather than in the ADR.
The parse pass has no skip arm, `into_event` errors instead of dropping, equality and slug
rules are structurally shared, and the push pre-flight refuses on the *kind* so a future
component cannot be smuggled onto the wire. The inbound/outbound asymmetry is the one part that
reads as an unfinished migration, and it is: `Handler::list_events` returns `Vec<CalendarItem>`
while `create_event`/`update_event`/`delete_event` take an `Event`, so CalDAV can pull tasks and
never push them. It is coherent as a *bounded* seam — documented at `rpc/ics.rs:1-26`, honest in
its R70 accounting, and the alternative (retyping the field) really would have cost 25 provider
sites — but it is a half-migrated trait, and the public API's naming (MAJOR-3) makes the whole
thing read more unfinished than it is. My four MAJORs are all cheap to close and three of them
must ride the 0.15.0 breaking window that is open right now: the missing `#[non_exhaustive]`
attributes and the unnameable `EventError`, and any renaming. The agenda's silent
whole-calendar drop is the only one that is a straight behaviour regression from `main` and
should be fixed regardless of the release window.
