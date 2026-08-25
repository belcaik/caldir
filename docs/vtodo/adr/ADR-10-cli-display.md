# ADR-10: CLI display of tasks

## Status
PROPOSED (Stage 3 candidate). Owns grill finding B-5. Assumes the poison-pill
graceful-degradation fix + ADR-09 C7 have landed. ADR-07 is CUT. Direction is settled by
the adjudication (B-5) — DUE anchors a task as DTSTART anchors an event, undated tasks get
their own section, C12 is amended. This ADR designs only the *how*.

## Context

- `AgendaView { entries: Vec<AgendaEntry>, range_start, range_end, time_format }`,
  `AgendaEntry { event: Event, calendar, rsvp }` — `output/agenda.rs:8-19`. One flat `Vec`,
  filtered by `is_visible` (`agenda.rs:37`; `output/event.rs:6-8` = `status != Cancelled`),
  fed by `Calendar::expanded_events_in_range` (`agenda.rs:36`, `caldir-core/src/calendar.rs:328-334`).
- Sort chain, duplicated at `agenda.rs:51-62` and `agenda_text.rs:24-34`:
  `local_date(start)` → `is_date()` reversed (all-day first) → `start.to_utc()`.
  `sort_by` is stable, so equal keys keep `read_dir` order — **already nondeterministic**.
- Text: bold day header (`agenda_text.rs:48`), then `format_event_line`
  (`output/event.rs:11-23`) = `"  {time:>7} {summary} {[cal].dimmed()}{status}"`.
- JSON: `AgendaJson` + flattened `EventJson` (`agenda_json.rs:30-79`), public and documented
  at `website/src/content/docs/commands.md:124-170`; `start: String` required
  (`agenda_json.rs:64`) — the C12 conflict. Feeders: `events.rs:28`, `today.rs:18`, `week.rs:17`.

Two sibling ADRs bind this one:
- ADR-01 (ACCEPTED): `CalendarItem { Event(Event), Todo(Todo) }`; `Todo` has
  `start: Option<EventTime>`, `due`, `completed`, `percent_complete`, `priority`,
  `status: TodoStatus`, and **no `end`, no `availability`** (ADR-01 #3; C-2 FORBIDS DTEND).
  So `AgendaEntry.event: Event` becomes `AgendaEntry.item: CalendarItem`.
- ADR-08 D1: an undated task is **unconditionally in-window**, so it *will* come out of the
  range query for `today`/`week`. Suppression is the CLI's job, not core's.

## Options considered

**(A) Interleave tasks with events at their anchor time, inside the existing day groups.**
Needs exactly one new accessor — `anchor(&CalendarItem) -> Option<&EventTime>` — after which
the whole sort/group/day-label machinery works unchanged. Small interface, all behaviour reused.

**(B) A "Tasks" sub-group under each day header.** Honest about kind, but `agenda_text.rs:43-62`
gains a second grouping level and `display_days` (`:72-90`) must learn which group it emits
into. It buys nothing the `[ ]` marker does not, and breaks the agenda's one job — *what is
next, in order*: a 09:00 task filed under "Tasks:" below the 14:00 meeting is a worse answer
to "what's today" than the same task at 09:00.

**(C) Separate `caldir tasks` command, agenda stays event-only.** Overruled by B-5. Recorded
so it is not re-proposed: the right *later* command, not a substitute.

## Decision

### D1 — Anchor and placement (option A)

One function in `output/agenda.rs`:

```
fn anchor(item: &CalendarItem) -> Option<&EventTime>
  Event(e) => Some(&e.start)
  Todo(t)  => t.due.as_ref().or(t.start.as_ref())
```

DUE wins over a task's DTSTART. `None` ⇒ **undated** (D3). Every `entry.event.start` in
`agenda.rs` / `agenda_text.rs` / `agenda_json.rs` becomes `anchor(&entry.item)`; for an
`Event` the value is byte-identical to today, so **event output is unchanged by
construction** (C13). Tasks are placed in the day group of `local_date(anchor)`, interleaved.
`display_days` (`agenda_text.rs:72-90`) repeats only `(Date, Some(Date))` pairs; a `Todo` has
no `end`, so it always takes the `vec![local_date(..)]` fallback. No change there.

### D2 — Total order

Extend the chain, in both copies, with **one** new tiebreak:
`local_date(anchor)` → `anchor.is_date()` reversed → `anchor.to_utc()` →
**kind ordinal, `Event(0) < Todo(1)`**.

Event and task at the same instant: event first. All-day event, date-valued DUE task, and a
09:00 event on one day: all-day event, task, 09:00. Rule 4 can never reorder two events, so
it cannot perturb existing output.

*Deliberately not added:* a final `instance_id` tiebreak. It would make event-vs-event ties
deterministic — a real improvement — but ties resolve by `read_dir` order today, so imposing
an order **would change existing text for existing situations** and breach C13. Own ticket.
Ordering is total across kinds; event-vs-event ties stay exactly as (non)deterministic as now.

### D3 — Undated tasks

An undated task is not "today" and not "this week". It is never placed in a day group and
never given an invented date (mirrors ADR-08 D1's "no synthesized dates").

- **Default** (`events`/`today`/`week`, any `--from/--to`): undated tasks are *counted, not
  listed*. Text output ends with one dimmed footer when any exist:
  `+ 3 undated tasks (caldir events --undated)`. Visible, zero noise, no silent drop.
- **`--undated`** (new flag, all three commands): renders them in a single trailing section
  after the last dated day, headed `No date`, same `.bold()` as a day label.
- **`--json` always includes them** (`start: null`, `due: null`). JSON has no sections to
  belong to and scripts must see the whole set; the flag is a text-view concern.

### D4 — Visual distinction (existing vocabulary only)

`format_event_line` stays; add a sibling `format_task_line` of the same shape — 2-space
indent, 7-wide right-aligned time column, dimmed `[cal]`:
`"  {time:>7} {box} {summary} {[cal].dimmed()}"`.

- `{box}`: `[ ]` NEEDS-ACTION, `[~]` IN-PROCESS, `[x]` COMPLETED. ASCII, same alphabet as
  `diff.rs`'s `+ ~ -`. No new emoji, no new indent level, no new colour role.
- `{time}`: clock time when DUE carries one; the literal `due` right-padded to 7 (matching
  `format_time_only`'s `{:>7}`, `output/time.rs:70`) when DUE is a DATE — `all-day` is false
  for a task; 7 spaces when undated.
- Colour, existing roles only: COMPLETED ⇒ summary `.dimmed()` (role shared with `[cal]` and
  `"No events found"`); DUE past and not COMPLETED ⇒ summary `.red()` (role shared with
  `diff.rs` delete / declined). Nothing else is coloured.

**Before** (event-only calendar — unchanged after this ADR):

```
Today
  all-day Sprint demo [work]
    09:00 Standup [work]
    14:00 Design review [work] (pending)

Tomorrow
  all-day Conference [work]
```

**After** (same calendar, tasks now present):

```
Today
  all-day Sprint demo [work]
      due [ ] Ship crawler patch [work]
    09:00 Standup [work]
    14:00 Design review [work] (pending)
    16:30 [~] Draft Q3 report [work]

Tomorrow
  all-day Conference [work]
      due [x] Rotate API keys [work]

+ 2 undated tasks (caldir events --undated)
```

**`caldir events --undated`** appends:

```
No date
          [x] Crawl u_crawler archive [research]
          [ ] Someday: rewrite installer [personal]
```

### D5 — Status; completed tasks are shown by default

`is_visible` extends to `CalendarItem`: `Event ⇒ status != Cancelled` (unchanged,
`output/event.rs:7`), `Todo ⇒ status != TodoStatus::Cancelled`. CANCELLED tasks are hidden
exactly as CANCELLED events are; the other three render per D4.

**COMPLETED tasks appear by default.** (1) The MVP fixture is `STATUS:COMPLETED`
(u_crawler) — hide-by-default hides the demo case, i.e. re-commits the B-5 failure this ADR
exists to fix. (2) A completed task with a DUE date is history for that day, like a past
event, which caldir already shows for any `--from` range. (3) Hiding by default is a *silent
drop* — the class B-3 and C10 rule against everywhere else. (4) The dimmed `[x]` already
carries the signal at a glance.
The honest counter — ADR-08's completed **undated** archive of thousands swamping the view —
is closed by D3: undated items are counted, never listed, unless asked for; dated completed
tasks are bounded by the window. Deferred, not MVP: `--hide-completed` / a config key.

### D6 — The `--json` schema change

C12 cannot hold literally: a DUE-only or undated task cannot supply a non-optional
`start: String`, and synthesizing one is the mission-forbidden fake event. Minimal change =
one rule + five additive keys.

**The rule:** `start` and `availability` widen from `T` to `T | null`, and **`null` may
appear on those two keys only when `kind == "todo"`.** Every `kind == "event"` entry stays
byte-identical.

**Additive keys** (on every entry, so object shape stays uniform):

| key | type | event | todo |
|---|---|---|---|
| `kind` | `"event" \| "todo"` | `"event"` | `"todo"` |
| `due` | `string \| null` | `null` | DUE, same encoding as `start` |
| `completed` | `string \| null` | `null` | COMPLETED, RFC3339 UTC |
| `percent_complete` | `number \| null` | `null` | 0–100 |
| `task_status` | `"needs_action"\|"in_process"\|"completed"\|"cancelled"\|null` | `null` | TodoStatus |

On a todo entry: `end` is always `null` (no end; C-2 FORBIDS DTEND); `all_day` =
`anchor.is_date()`, `false` when undated; `tzid` from the anchor. PRIORITY rides whatever
surface ADR-04's property set gives it, or gains a key under the same rule.

**`status` is NOT retyped.** It stays the non-optional `Status` enum meaning "is this item
live" — exactly how `is_visible` uses it (`output/event.rs:7`). TodoStatus maps on as
NeedsAction/InProcess/Completed ⇒ `"confirmed"`, Cancelled ⇒ `"cancelled"`; the true value
lives in `task_status`. Widening `status`'s value set instead breaks every consumer matching
on it exhaustively — strictly worse than one new key.

**Compatibility verdict: not breaking for any document a consumer could previously have
received; breaking for a consumer that assumes `.start` is non-null across all entries of a
directory that now contains tasks.** No key removed or renamed; `null` appears only on a
value shape that did not exist before this release. Migration is one predicate:
`select(.kind == "event")` reproduces the old stream exactly. Note also that a todo's
`instance_id` carries the `VTODO:` sigil (B-4) while `uid` stays bare; event `instance_id`s
are unchanged.

C12's test `schema_keys_are_stable` is amended to assert **two** fixtures: an event entry
(exact key set incl. the five new keys; `start` typed string, non-null) and a todo entry
(same key set; `start`/`availability` nullable). `commands.md:124-170` gains a second object.

### D7 — Affected commands

| command | evidence | change | MVP? |
|---|---|---|---|
| `events`/`today`/`week` | `events.rs:28`, `today.rs:18`, `week.rs:17` | thread new `undated: bool` into `AgendaView::collect`; nothing else | MVP |
| agenda renderer | `agenda{,_text,_json}.rs` | D1–D6 | MVP |
| `status`/`pull`/`push`/`sync`/`discard` | `output/diff.rs:35-60`, `:221-267` | `EventChange` carries `CalendarItem` (ADR-01 #1). `diff.rs:45` `format_datetime(&event.start,…)` is a **compile error** for a todo ⇒ use `anchor`, print `(no date)` on `None`. `render_field_diffs` must gain `due` + `task status` rows, else a task-only edit renders an **empty** diff | MVP (compile-forced; two rows are the minimum) |
| `doctor` | `doctor.rs:55`, `doctor/warning/duplicate_file.rs:17` | `calendar.events()` yields items; counts are component-agnostic. Duplicate detection keys on `event_instance_id()`, kind-aware for free via C-1. **C13 holds: a malformed VEVENT still reports `UnreadableEvents`** | MVP (compile-level only, no text change) |
| `rsvp` | `rsvp.rs:46-56`, `:91-102`, `:123` | **RSVP on a task is meaningless.** Single-file mode: fail with `"<path> is a task, not an event; tasks cannot be RSVP'd"`, non-zero exit, **no file write**. List mode: narrow via `let Some(event) = ce.event() else { continue }` so tasks never enter the invite list | MVP |
| `invites` | `invites.rs:38-57` | same narrowing. A task may carry ATTENDEE (C-2) but has no invite semantics | MVP |
| `new` | `commands/new.rs` | creates VEVENT only; no ADR owns `caldir new --due`. Tasks arrive by pull or hand-authored `.ics` | deferred |
| `calendars` | `calendars.rs:7-17`, `commands.md:241-242` | config only, never enumerates events | none |
| `connect`/`config`/`update` | — | no item enumeration | none |
| mass-delete guard | `utils/guards.rs:10` | counts outgoing deletes; component-agnostic | none |

## Consequences

Easy: VJOURNAL later reuses `anchor` + one glyph; one accessor localises every "where does
this item sit in time" question in the CLI. Hard: `render_field_diffs` grows a per-kind arm;
the two sort chains must stay in step (pre-existing duplication, unchanged here).
Must be tested: (i) an event-only fixture renders byte-identically before/after — the C13
lock; (ii) event / date-DUE task / timed task / all-day event on one day give the D2 order in
all three CI timezones; (iii) an undated task is absent from `today`'s body, present in the
footer count, present in `--json`; (iv) `schema_keys_are_stable` over both fixtures; (v) a
COMPLETED task with a past DUE renders dimmed `[x]` and is not hidden; (vi) `rsvp` on a
VTODO file exits non-zero and leaves the file unmodified.

## Backward-compatibility impact

- **Local files / `.caldir` state / RPC wire / installed provider binaries:** untouched.
  This ADR is display-only.
- **Text output:** unchanged for event-only calendars (D1 anchor identity + D2 rule 4).
- **`--json`:** widened per D6 — five additive keys plus `null` on `start`/`availability`
  for `kind:"todo"` entries only.

## Rejected because

- Per-day "Tasks" sub-group: second grouping level, and it destroys chronological order.
- Separate `caldir tasks` command: overruled by the B-5 user decision.
- Synthesizing a `start` for a DUE-only task: the mission's forbidden fake event.
- Hiding COMPLETED tasks by default: hides the MVP's own fixture; a silent drop.
- Widening the `status` enum's value set: breaks exhaustive consumers; C12 forbids it.
- `instance_id` final sort tiebreak: correct, but changes existing event output ⇒ C13.

## Open sub-questions

1. **Should `--json` honour `--undated`?** I ruled no (D3) so scripts always see the full
   set, at the cost of text and JSON disagreeing on cardinality. No experiment needed — a
   one-line policy flip if the maintainer prefers symmetry. This is the only sub-question
   I could not settle on evidence rather than taste.
2. `task_status` vs `todo_status` — cosmetic; settle at spec stage against `specs/caldir.md`'s
   new §To-dos terminology.
3. **UNVERIFIED:** whether the u_crawler fixture is undated or DUE-bearing — the adjudication
   records its layout as unconfirmed on disk. If DUE-bearing, D3's footer never fires for the
   demo and D5 alone carries it.
