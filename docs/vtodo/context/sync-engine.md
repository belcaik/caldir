# Sync engine: status / diff / pull / push / sync

Branch `vtodo-support`. All paths relative to repo root. Facts only.

## 1. Entry points

Every CLI command that touches a remote follows the same shape: resolve a
`DateRange`, get `Connection`s, call `Connection::diff(&range)`, then apply.

| Command | File | Applies |
| --- | --- | --- |
| `status` | `caldir-cli/src/commands/status.rs:51` | nothing (renders only) |
| `pull` | `caldir-cli/src/commands/pull.rs:54` | `apply_incoming_diff` (`pull.rs:69`) |
| `push` | `caldir-cli/src/commands/push.rs:65` | `apply_outgoing_diff` (`push.rs:84`) |
| `sync` | `caldir-cli/src/commands/sync.rs:80` | incoming then outgoing (`sync.rs:95`, `sync.rs:104`) |
| `discard` | `caldir-cli/src/commands/discard.rs:36` | `discard_outgoing_diff` |

`push`/`sync`/`discard` gate on `allow_mass_delete` (`caldir-cli/src/utils/guards.rs:10`),
which counts `EventChange::Delete` in `outgoing()`.

## 2. Types

- `CalendarDiff { outgoing: Vec<EventChange>, incoming: Vec<EventChange> }` —
  `caldir-core/src/diff/calendar_diff.rs:8`. Accessors `incoming()`/`outgoing()`/
  `is_empty()`/`discard_outgoing()` at `:122`–`:138`.
- `EventChange::{Create(Event), Update{from,to}, Delete(Event)}` —
  `caldir-core/src/diff/event_change.rs:5`.
- `CalendarEvent { event: Event, path: PathBuf }` (local, file-backed) —
  `caldir-core/src/calendar/event.rs:12`.
- `RemoteEvent(Event)` — `caldir-core/src/remote/event.rs:5`;
  `modified_at()` returns `event.last_modified` (`:12`).
- `SyncBases(HashMap<EventInstanceId, Option<Box<Event>>>)` —
  `caldir-core/src/calendar/state/sync_bases.rs:12`. `Some(base)` = full base
  event on disk; `None` = legacy known-id entry only.
- `CalendarState { sync_bases }` — `caldir-core/src/calendar/state.rs:19`.

## 3. Keying / identity

Both sides are keyed by `EventInstanceId = (EventUid, Option<RecurrenceId>)`
(`caldir-core/src/event/instance_id.rs:13`), produced by
`Event::event_instance_id()` (`caldir-core/src/event.rs:252`).

- local set: `HashSet` of ids, `calendar_diff.rs:20`
- remote map: `HashMap<EventInstanceId, &RemoteEvent>`, `calendar_diff.rs:25`
- pull/push apply maps: `connection.rs:66`, `connection.rs:96`

String form of the id (used in `known_event_ids`) is `{uid}` or
`{uid}__{recurrence_id}`, recurrence id formatted from an `EventTime`
(`instance_id.rs:44`, `:81`); parsing back is infallible (`:60`).

Local files are loaded by scanning `*.ics` in the calendar dir
(`caldir-core/src/calendar.rs:106`); each file must contain exactly one event
(`calendar/event.rs:40`).

## 4. Diff algorithm — `CalendarDiff::compute` (`calendar_diff.rs:14`)

Pass 1, over local events (`:33`):
1. If the id exists remotely and `event == remote_event.event()` → skip (`:38`).
2. If both sides are `Status::Cancelled` → skip regardless of drift (`:45`).
3. `to_push` = local clone; a `None` visibility inherits the remote's (`:52`).
4. If `to_push == remote` after that, direction is forced to `Pull` (`:58`),
   otherwise `update_direction(local, remote, base)` (`:61`).
5. `Push` → `outgoing.push(Update{from: remote, to: to_push})` (`:65`);
   `Pull` → `incoming.push(Update{from: local, to: remote})` (`:69`).
6. Not in remote and out of window → skip (`:80`, see §6).
7. Not in remote, in window: base known → `incoming Delete` (pull deletes the
   local file); base unknown → `outgoing Create` (`:86`–`:90`).

Pass 2, over remote events (`:93`):
- present locally → skip (`:97`);
- remote `Status::Cancelled` and missing locally → skip, "missing file ==
  CANCELLED" (`:106`);
- base known → `outgoing Delete` (user deleted the file) (`:110`);
- otherwise → `incoming Create` (`:115`).

`Connection::diff` (`caldir-core/src/connection.rs:37`) loads both sides,
backfills bases (§5), computes the diff, and calls `discard_outgoing()` for
read-only calendars (`:57`).

## 5. Direction decision

`update_direction` (`calendar_diff.rs:153`):

```rust
if let Some(base) = base {
    let local_has_changes = local.event() != base;
    let remote_has_changes = remote.event() != base;
    match (local_has_changes, remote_has_changes) {
        (false, false) => return None,
        (true, false) => return Some(UpdateDirection::Push),
        (false, true) => return Some(UpdateDirection::Pull),
        (true, true) => {}
    }
}
if local_is_newer(local, remote) { Some(Push) } else { Some(Pull) }
```

Three-way base comparison first; the mtime fallback only runs when both sides
changed or no base exists. `local_is_newer` (`calendar_diff.rs:180`):

```rust
fn local_is_newer(local: &CalendarEvent, remote: &RemoteEvent) -> bool {
    if remote.modified_at().is_none() && local.event().sequence != remote.event().sequence {
        return local.event().sequence > remote.event().sequence;
    }
    match (local.modified_at(), remote.modified_at()) {
        (Some(l), Some(r)) => l > r,
        (Some(_), None) => true,
        (None, Some(_)) => false,
        (None, None) => true,
    }
}
```

- local side = **file mtime** (`CalendarEvent::modified_at`,
  `calendar/event.rs:97`, from `fs::metadata`).
- remote side = **`LAST-MODIFIED`** (`RemoteEvent::modified_at`,
  `remote/event.rs:12` → `Event::last_modified`).
- On every local write the file mtime is pinned to the event's `LAST-MODIFIED`
  (`sync_file_mtime`, `calendar/event.rs:153`, called from `create` `:23` and
  `update` `:62`), so the two clocks are comparable.
- Ties (`l == r`) fall through to Pull.

## 6. Equality / change detection

Field-by-field structural equality on `Event`, no hash, no timestamp.
`#[derive(Debug, Clone, Eq, educe::Educe)] #[educe(PartialEq)]`
(`caldir-core/src/event.rs:38`). Two fields are excluded from equality:

```rust
#[educe(PartialEq(ignore))]
pub last_modified: Option<DateTime<Utc>>,
#[educe(PartialEq(ignore))]
pub sequence: i32,
```
(`event.rs:63`–`:67`)

Two fields use custom order-insensitive comparators (`event.rs:283`, `:290`):

```rust
fn x_properties_eq(a: &[XProperty], b: &[XProperty]) -> bool {
    a.len() == b.len() && a.iter().all(|x| b.contains(x))
}
fn attachments_eq(a: &[Attachment], b: &[Attachment]) -> bool {
    a.len() == b.len() && a.iter().all(|x| b.iter().any(|y| y.uri == x.uri))
}
```

Every other field (`uid, summary, description, location, start, end, status,
availability, visibility, recurrence, recurrence_id, organizer, attendees,
reminders, url`) participates directly (`event.rs:40`–`:61`).

Base files are also compared by parsed equality, not bytes, when deciding
whether to rewrite (`state/event_bases.rs:34`).

## 7. Sync state: `known_event_ids` and `bases`

Directory: `<calendar>/.caldir/state/`.

- `known_event_ids` (legacy, still written) — plaintext, one
  `EventInstanceId` per line, sorted, written atomically via tempfile+rename
  (`state/known_event_ids.rs:18`, `:23`, `:60`–`:84`). Load ignores blank lines
  (`:41`). Purpose documented at `:1`–`:13`: distinguish "local deleted, delete
  remote" from "new remote, create local".
- `bases/` — one `<sha256(instance_id)>.ics` per event
  (`state/event_bases.rs:8`, `:29`, `:78`); filename must re-hash to the parsed
  event's id or the file is ignored (`:67`); an unparseable base is skipped and
  degrades to the known-id entry (`:60`).
- `SyncBases::load_from_state_dir` unions both, bases winning
  (`state/sync_bases.rs:28`, `:83`–`:97`).
- `record` inserts each supplied event as its base, rewrites the full
  `known_event_ids` set from `self.0.keys()`, and writes only the changed base
  files (`state/sync_bases.rs:48`–`:66`).

**Entries are appended when:**
- pull: on incoming `Create` and `Update` (`connection.rs:206`, `:212`) —
  incoming `Delete` pushes nothing, so the id survives in `known_event_ids`
  (only base files are never removed either; nothing deletes them).
- push: on the provider-returned event for each applied change
  (`connection.rs:248`), which also rewrites the local file if the provider
  changed the UID (`:236`–`:246`).
- `diff` itself, via `bases_to_backfill` (`connection.rs:162`): pairs that are
  byte-identical on both sides but whose base is missing/stale get the remote
  recorded as base; pairs with no sync state at all are skipped (`:180`).
- Both apply paths record even on partial failure (`connection.rs:84`, `:114`).

**Deletes are inferred** purely from `sync_bases.get(&id).is_some()`:
local-missing + known → outgoing Delete (`calendar_diff.rs:110`);
remote-missing + known → incoming Delete (`calendar_diff.rs:86`).

## 8. Sync window

`DateRange { from: Option<DateTime<Utc>>, to: Option<DateTime<Utc>> }`
(`caldir-core/src/utils/date_range.rs:11`). Default `±365` days around local
today, snapped to local start/end of day and converted to UTC
(`date_range.rs:7`, `:18`, `:22`). CLI mirrors the constants in
`caldir-cli/src/utils/sync_range.rs:6`, `:9`; `--from start` means unbounded
(`sync_range.rs:26`). Unbounded ends serialize as sentinels
`1970-01-01` / `2100-01-01` on the wire (`date_range.rs:5`, `:36`), and the
range is sent to the provider as RFC3339 `from`/`to` in `ListEvents`
(`caldir-core/src/remote.rs:138`).

Out-of-window protection is exactly one check, in pass 1 only:

```rust
if let (Some(from), Some(to)) = (range.from, range.to)
    && !event.has_occurrence_in_range(from, to)
{
    continue;
}
```
(`calendar_diff.rs:80`)

`has_occurrence_in_range` (`event.rs:146`) expands the RRULE for recurring
events, else delegates to `occurs_in_range` (`event.rs:136`):

```rust
let event_start = self.start.to_utc();
let event_end = self.end.as_ref().unwrap_or(&self.start).to_utc();
event_start < to && event_end > from
```

## 9. Where the engine assumes VEVENT semantics

Numbered, testable claims. A "task" here means: no `DTSTART`, has `DUE`,
possibly `COMPLETED`, possibly `STATUS:NEEDS-ACTION`/`IN-PROCESS`/`COMPLETED`.

1. **Parsing rejects a component with no start.** `Event::try_from(&icalendar::Event)`
   returns `EventError::MissingStart` when `get_start()` is `None`
   (`caldir-core/src/event/from_icalendar.rs:11`, `event/error.rs:15`). A VTODO
   with only `DUE` can never become an `Event`.
2. **Only VEVENT components are read at all.** `Event::from_ics_str` iterates
   `icalendar.events()` (`event.rs:101`); a `VTODO` in a file yields zero
   events, and `CalendarEvent::load` then fails with
   `ExpectedSingleEvent { found: 0 }` (`calendar/event.rs:40`–`:49`).
3. **`Event.start: EventTime` is non-optional**, and there is no `due` or
   `completed` field (`event.rs:45`–`:67`). Equality therefore cannot compare
   `DUE`/`COMPLETED`/`PERCENT-COMPLETE` at all.
4. **Writing always emits `VEVENT` with `DTSTART`**: `icalendar::Event::new()`
   plus `event.starts(...)` unconditionally (`event/to_icalendar.rs:6`–`:8`),
   and `to_ics_string` splices `VALARM`s into `END:VEVENT`
   (`event.rs:133`, `:249`).
5. **Window filtering is keyed on start/end.** `occurs_in_range` uses
   `self.start` and `self.end.unwrap_or(self.start)` (`event.rs:136`–`:141`).
   A task's due date is invisible to it; a start-less task would have no
   defensible value for `event_start`.
6. **The out-of-window skip (`calendar_diff.rs:80`) is the sole protection for
   local-only items**, and it calls `has_occurrence_in_range`. Any item whose
   time semantics aren't start/end would be classified by that call — a wrong
   `false` skips it forever, a wrong `true` makes it eligible for
   `incoming Delete` at `calendar_diff.rs:86`.
7. **The remote fetch window is start/end-shaped**: `list_events` sends only
   `from`/`to` (`remote.rs:138`–`:150`) and the provider is expected to filter
   by occurrence. There is no separate due-date window.
8. **Filenames encode the start time.** `base_slug` = `{time_slug}__{summary}`
   with `time_slug` derived from `self.start` (`event/slugify.rs:13`, `:48`).
   Used on every create and update (`calendar/event.rs:19`, `:56`) and by the
   base-file hash indirectly via the instance id.
9. **"Not active" is modelled only as `Status::Cancelled`.** The enum has
   exactly `Confirmed | Tentative | Cancelled` (`event/status.rs:5`), so
   `NEEDS-ACTION`/`IN-PROCESS`/`COMPLETED` parse to nothing. The two skip rules
   at `calendar_diff.rs:45` and `calendar_diff.rs:106` — "both cancelled" and
   "missing locally + cancelled remotely" — key on that enum; a completed task
   is not cancelled, so a locally-deleted completed task would still produce an
   outgoing Delete, and a remote completed task missing locally would be
   re-created on pull.
10. **Direction's fallback needs `LAST-MODIFIED` and file mtime** and nothing
    else (`calendar_diff.rs:180`, `calendar/event.rs:97`); `COMPLETED` is never
    consulted as a recency signal.
11. **`mtime` pinning depends on `last_modified` being present**
    (`calendar/event.rs:153`–`:159`); with `None` the file keeps its write time
    and `local_is_newer` returns `true` on the `(Some, None)` arm
    (`calendar_diff.rs:187`), biasing toward Push.
12. **Recurrence expansion is event-only.** `has_occurrence_in_range` calls
    `occurrences::expand_master` (`event.rs:148`), which itself calls
    `occurs_in_range` (`event/occurrences.rs:33`, `:45`, `:68`) — a recurring
    VTODO would flow through the same start/end logic.
13. **The RPC codec round-trips through `to_ics_string`/`from_single_ics_str`**
    (`caldir-core/src/rpc/ics.rs:28`, `:32`), so every provider call inherits
    claims 1, 2 and 4 for both directions.
