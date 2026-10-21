# ADR-02: Identity across VEVENT and VTODO

## Status

PROPOSED (Stage 3 candidate). Settles DECISION-02 / DECISION-08 and system-map Q10.

## Context

Identity is one key type, component-agnostic:
- `EventInstanceId((EventUid, Option<RecurrenceId>))`, `#[derive(PartialEq, Eq, Hash)]` —
  `event/instance_id.rs:12-13`; built only by `Event::event_instance_id()` (`event.rs:252-254`).
  `RecurrenceId` hand-implements `PartialEq`/`Hash` over `EventTime::normalized()`
  (`event/instance_id/recurrence_id.rs:25-35`).
- String form `{uid}` / `{uid}__{rid}`; `From<&str>` is **infallible** by design because UIDs may
  contain `__` (`instance_id.rs:38-40`, `:60-73`). That string is the whole content of a
  `known_event_ids` line (`state/known_event_ids.rs:23-27`, `:45`; written from `self.0.keys()` at
  `state/sync_bases.rs:60`) and the sole input to the base filename hash (`state/event_bases.rs:29`,
  `:78-80`; re-hash check `:64-67`).
- Delete inference keys purely on `sync_bases.get(&id).is_some()` — `diff/calendar_diff.rs:86`
  (incoming) and `:110` (outgoing).
- **Non-test constructors number exactly two**: `event.rs:253` and `known_event_ids.rs:45` (verified
  by grep; all other hits are `#[cfg(test)]`). The key type is cheap to change.

RFC 5545 scopes UID uniqueness per component type in practice, and u_crawler does exactly that:
`deadlines/assignment-123.ics` (VTODO) and `windows/assignment-123.ics` (VEVENT) share a UID.
Today those are separate calendars (`caldir.rs:71-83`; reproduction.md §4) so nothing collides. But
a Radicale collection may legitimately hold both components, and `Calendar::events()` matches
`*.ics` by extension only (`calendar.rs:113`), so a mixed local directory is equally reachable.

### The hazards, traced

Reachability caveat: all of these are unreachable **today** only because
`Event::from_ics_str` drops VTODO at `caldir-core/src/event.rs:101`. They become live on the first
commit that lets a VTODO produce an `EventInstanceId`. Verdict: **REAL, latent — not theoretical.**

1. **State-file collision — REAL.** `SyncBases` is `HashMap<EventInstanceId, _>`
   (`sync_bases.rs:12`) and `insert_event_base` is a plain `insert` (`:43`), so todo and event
   overwrite each other's base. `known_event_ids` is a `HashSet` (`known_event_ids.rs:21`) written
   from `self.0.keys()` (`sync_bases.rs:60`) — the pair collapses to one line.
2. **Diff mis-keying — REAL.** `remote_by_id` is a `HashMap` built by `.collect()`
   (`calendar_diff.rs:25-28`): two same-UID remote items silently become one, last wins. Pass 1
   iterates the local `Vec` (`:33`), so a local VTODO looks up (`:38`) and compares against a remote
   VEVENT (`:39`) and emits an `Update` that **overwrites one component with the other** — worse
   than a delete. Pass 2 iterates the remote `Vec` (`:93`) but skips on
   `local_event_ids.contains(&id)` (`:97`), so a remote VTODO is invisible while a same-UID VEVENT
   exists locally.
3. **Wrong-component deletion — REAL.** Both files local, neither remote, base known for the shared
   id: pass 1 runs once per local file and `sync_bases.get(&id).is_some()` is true both times
   (`:86`), so **two** Deletes fire though only one identity was ever tracked.
4. **Base-file overwrite — REAL.** Not a sha256 *hash* collision, an *input* collision:
   `hash_filename` takes the id string (`event_bases.rs:29`), identical for both, so both map to
   `bases/<same>.ics`. The skip-if-unchanged guard (`:34`) sees a mismatch and rewrites — the two
   ping-pong the file every sync, feeding `update_direction` (`calendar_diff.rs:153-167`) a base
   belonging to the wrong component.
5. **(Found while tracing) `caldir doctor` false positive — REAL.** `duplicate_file_warnings`
   buckets by `EventInstanceId` and warns on any bucket >1
   (`caldir-cli/src/commands/doctor/warning/duplicate_file.rs:13-19`) — telling the user to delete
   one of two legitimate files.

Separately, CalDAV derives the href from the bare UID —
`event_url(calendar_url, event.uid.as_str())` → `{base}/{uid}.ics`
(`caldir-provider-caldav/src/caldav/client.rs:73-76`, called at `ops/create_event.rs:26`) — so the
pair PUTs to one server path (A24). **UNVERIFIED against live Radicale** (no instance available);
the code path itself is unambiguous.

## Options considered

**O1 — Stay component-agnostic; forbid mixed calendars.** Zero identity change, but needs a
component-set invariant, a validation pass and a user-facing error across `calendar.rs`,
`connection.rs`, `calendar/config.rs` and the CLI — and it makes a legitimate, server-controlled
Radicale collection shape unsupported, which the user cannot remedy locally.

**O2 — Two id types (`EventInstanceId` + `TodoInstanceId`).** In its favour: a wrong-kind comparison
becomes a compile error, not a runtime near-miss. Cost: every state container (`SyncBases`,
`KnownEventIds`, `EventBases`), both diff maps and the state files must be duplicated or made
generic — all of `calendar/state/**` and `diff/**` — and two `known_event_ids` files is a migration.

**O3 — One key type with a component-kind axis, VEVENT-transparent string form.** Chosen.

## Decision

**`EventInstanceId` stays the one key type for both components and gains a component-kind axis. It
is neither split nor left component-agnostic.**

```rust
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum ComponentKind { #[default] Event, Todo }
// kind is NEW and participates in PartialEq + Hash:
pub struct EventInstanceId { kind: ComponentKind, uid: EventUid, recurrence_id: Option<RecurrenceId> }
```

Binding rules:

1. `EventInstanceId::new(uid, rid)` keeps its two-argument signature and means
   `kind: ComponentKind::Event`; add a second constructor for todos. Both existing non-test call
   sites then compile unchanged. `RecurrenceId` keeps its hand-written `PartialEq`/`Hash` over
   `normalized()` unchanged; `kind` is compared and hashed structurally alongside it.
2. **`Display` for `kind == Event` is byte-identical to today** (`{uid}` / `{uid}__{rid}`). Only
   `Todo` gains a prefix: `VTODO:{uid}` / `VTODO:{uid}__{rid}`. `From<&str>` stays **infallible**:
   strip a leading `VTODO:` if present (→ `Todo`), else `Event`, then apply the existing
   `rsplit_once("__")` rid logic to the remainder.
3. `EventBases::{load,write_from}` (`event_bases.rs:22-75`) keep the same on-disk layout and the
   same `hash_filename(id.to_string())` rule, but must become item-aware so a VTODO base parses and
   re-hashes against its own prefixed id at `:64-67`.
4. CalDAV `event_url` (`client.rs:73-76`) must stop deriving the href from the bare UID for todos.
   Keep `{uid}.ics` for VEVENT (no regression on existing resources); kind-distinct href for VTODO.
5. Therefore: one key type with a kind axis; a VEVENT and a VTODO **may** coexist in one calendar
   and **may** share a UID — a supported, non-error state; `.caldir/state/` needs **no migration**.

Not decided here: renaming the `Event*`-prefixed types (A26, COSMETIC) — follow-up ticket, not this PR.

## Consequences

Easy: mixed calendars and mixed CalDAV collections work; all five hazards close at once because they
funnel through the same key type and the same `Display` string. Hard: the string form now carries a
semantic prefix, so a VEVENT whose UID literally begins with `VTODO:` round-trips as a todo id — the
same class of ambiguity upstream already accepted for `__` (`instance_id.rs:38-40`), and unreachable
for caldir-minted UIDs (`{uuid_v4}@caldir`, `event.rs:36`).

Must be tested:
- `parse(format(id)) == id` for both kinds, all four rid sub-formats, a UID containing `__`, and the
  pathological `UID:VTODO:x` VEVENT (asserted so the loss is known).
- Event-form byte stability: pin `hash_filename` against the literal already asserted at
  `event_bases.rs:98-104`; assert an existing `known_event_ids` line is unchanged.
- `CalendarDiff::compute`, shared UID: local pair + empty remote + known base → exactly one Delete,
  right file (hazard 3); remote pair → both survive `remote_by_id` (hazard 2).
- `duplicate_file_warnings` on an event/todo UID pair: **no** `DuplicateFiles` warning (hazard 5).
- `SyncBases::record` on both: two map entries, two `known_event_ids` lines, two `bases/` files.

## Backward-compatibility impact

- **Local files on disk:** none — identity is derived, never stored in the `.ics`.
- **`.caldir/state/` — NO MIGRATION.** Existing `known_event_ids` lines lack the prefix and parse to
  `kind: Event`, which is what they always meant; existing `bases/*.ics` filenames hash an
  unprefixed string and keep their names. Downgrade-safe too: an older caldir-core reading
  `VTODO:assignment-123@u-crawler` treats the whole literal as a UID — no VEVENT ever produces that
  id, so it is an inert extra known-id that cannot trigger a delete (which needs the id present on
  exactly one side, `calendar_diff.rs:86`/`:110`) nor collide with a base filename.
- **RPC wire / installed provider binaries:** untouched. `EventInstanceId` is never serialized; the
  wire carries opaque ICS (`rpc/ics.rs:28-35`), no `Method` variant added, and href rule (4)
  changes only new todo resources.

## Rejected because

- **O1:** makes a legitimate, server-controlled Radicale collection shape unsupported.
- **O2:** duplicates or genericizes every state container, diff map and state file for a distinction
  one enum field expresses — and forces a state-format migration.
- **Renaming `Event*` in this PR:** ~20 call sites of cosmetic churn, zero behavioural benefit.

## Open sub-questions

1. **Does Radicale let a VEVENT and a VTODO share a UID in one collection, and does the second PUT
   to `{uid}.ics` overwrite the first?** (Q10 / A24.) Not runnable here — no Radicale instance.
   *Experiment:* PUT both to a local collection, `PROPFIND`, count resources. Affects only href rule
   (4); the identity decision holds either way.
2. **Exact spelling of the todo prefix.** `VTODO:` is proposed for legibility in `known_event_ids`.
   A character illegal in RFC 5545 UIDs would remove the residual ambiguity, but UID is `TEXT` and I
   could not establish one exists. A naming call — but make it before any state file is written in
   the wild.
3. **Should `ComponentKind` be `#[non_exhaustive]`** for a future VJOURNAL? Leaning no (YAGNI, and
   it forces `_` arms across core), but it is one line and hard to reverse.
