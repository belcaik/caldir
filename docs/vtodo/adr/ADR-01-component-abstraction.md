# ADR-01: The core component abstraction for VTODO

## Status

PROPOSED (Stage 3 candidate). Settles DECISION-01, system-map **Q1** and **Q2**.

## Context

- One dispatch point drops every VTODO: `Ok(icalendar.events().map(Event::try_from).collect())`
  — `caldir-core/src/event.rs:101`. On the fixture: `components=1 events=0 todos=1`
  (`docs/vtodo/reproduction.md` §3). Parsing is fine; the filter is the bug.
- `icalendar::Calendar.components: Vec<CalendarComponent>` is a **public field**
  (`icalendar-0.17.10/src/calendar.rs:71`), so one ordered pass can dispatch
  `Event | Todo | _`. `CalendarComponent` is `#[non_exhaustive]` (needs a `_` arm).
- `icalendar::Todo` has the *same* trait stack as `Event`: `component_impl!{Todo,"VTODO"}` +
  `event_impl!{Todo}` (`components.rs:664-665`). `Component` supplies uid/summary/
  description/class/url/sequence/last_modified/priority/get_start/get_end/get_recurrence_id
  (`components.rs:73-375`), `EventLike` adds location/starts/ends/rdate/exdate (`:377-573`)
  — so the shared codec half can be one generic fn pair over `T: EventLike`, not a copy-paste.
- `Event` is 19 fields with educe-tuned `PartialEq` (`event.rs:38,56-67`): ignores
  `last_modified`+`sequence`, x-props order-independent by name+value, attachments by `uri`.
  `==` **is** the "content changed?" test driving sync direction (`calendar_diff.rs:37,155-166`).
  Its two comparison helpers are module-private free fns (`event.rs:282,289`).
- All state is keyed by `EventInstanceId` (`instance_id.rs:13`), component-agnostic today,
  and its `{uid}__{rid}` string form is **on-disk contract** (`known_event_ids`,
  `bases/<sha256(id)>.ics`). `SyncBases` is `HashMap<EventInstanceId, Option<Box<Event>>>`
  (`sync_bases.rs:12`) — **one** map per calendar dir, which must hold both kinds together.
- Wire already carries opaque ICS (`rpc/ics.rs`); `Method` is a closed serde enum
  (`rpc/methods.rs:22-31`), so no new verb is permissible.

### Experiment run (Q2)

Scratchpad copy, single edit `start: EventTime` -> `Option<EventTime>`,
`cargo check --workspace --all-targets`:

```
15 event/occurrences.rs   8 event.rs   6 calendar.rs   4 event/from_icalendar.rs
 3 event/to_icalendar.rs  3 event/slugify.rs   2 diff/calendar_diff.rs  = 41 errors / 7 files
```

Compilation aborts at `caldir-core`, so `caldir-cli` and all five providers were never
reached — the true number is higher (`.start` appears in 26 files workspace-wide, incl.
outlook/google mappers irrelevant to tasks). Those 7 files hold **220 of the 686 tests**,
all VEVENT tests.

## Options considered

**(A) `enum CalendarItem { Event(Event), Todo(Todo) }` + separate `Todo`.**
`Event` untouched. `Todo` reuses `EventUid`/`EventTime`/`RecurrenceId`/`Recurrence`/
`XProperty`/`Attachment`/`Attendee`/`Organizer`/`Visibility`/`Reminder` verbatim (verified
component-agnostic — domain-model §"Seams"). `CalendarEvent`/`RemoteEvent`/`EventChange`
become item-typed; `CalendarDiff` stays **one** engine behind a ~6-method enum interface
(`event_instance_id`, `is_cancelled`, `has_occurrence_in_range`, `sequence`,
`visibility`/`set_visibility`, `to_ics_string`). Blast radius **~24 files**: the 70
`.event()` call sites sit in just 6 (`calendar_diff.rs` 20, `calendar.rs` 18,
`calendar/event.rs` 16, `connection.rs` 11, `cli/commands/rsvp.rs` 4,
`cli/…/duplicate_file.rs` 1), plus `event.rs:96-121`, `diff/event_change.rs`, `remote.rs`,
`remote/event.rs`, `rpc/{ics,wire}.rs`, `state/{sync_bases,event_bases}.rs`, `lib.rs`, CLI
render (`output/{agenda,agenda_text,agenda_json,diff,event}.rs`), the two CalDAV
comp-filters (`ops/list_events.rs:103`, `ops.rs:107`), and new `item.rs` + `todo/**`.
Cost: widest diff, plus genuine field duplication. Benefit: "todo rendered as a fake event"
is unrepresentable — the mission constraint, enforced by the compiler.

**(B) Widen `Event` with a `kind` discriminant + optional `start`/`due`/`completed`.**
Cheapest to write; most call sites compile unchanged. Rejected on the measurement above: 41
errors in 7 files holding 220 VEVENT tests before the checker even reaches the CLI, and
every error sits inside logic sync depends on (`occurs_in_range` `event.rs:136`, `time_slug`
`slugify.rs:47`, `expand_master` `occurrences.rs:110-130`). It also makes
`Event{kind:Event, due:Some(..)}` and `Todo{end:Some(..)}` representable — the RFC-invalid
shape `icalendar` already leaks via `EventLike::ends()` on `Todo` (C4), except now caldir
owns it.

**(C) A `Component` trait implemented by both, generic diff.**
Blocked by storage: `SyncBases` is one heterogeneous map (`sync_bases.rs:12`), so a trait
needs `Box<dyn Component>`, losing derived `Clone` and — fatally — `PartialEq`, the exact
semantics this ADR must preserve (A18). Making
`CalendarEvent`/`RemoteEvent`/`EventChange`/`CalendarDiff`/`SyncBases` generic over `T`
instead threads a type parameter through the whole sync stack and *still* needs an enum at
the disk boundary, where one directory yields a mixed `Vec`. A trait is worth having
*inside* (A); it cannot be the dispatch type.

**(D) `Todo` struct, no shared supertype, parallel pipeline.**
Lowest VEVENT regression risk — existing paths literally untouched. But it duplicates
`CalendarDiff::compute` (117 lines of subtle skip rules and three-way direction logic,
`calendar_diff.rs:14-131`), `Connection::apply_{incoming,outgoing}_diff`, and the wire
plumbing, and the copies must stay in step forever (R7). It also does **not** dodge the
shared-storage problem: `SyncBases` is still one map, so either it grows a second parallel
map (and `bases/<sha256(id)>.ics` gains a collision the layout can't express) or the enum
reappears at the state layer anyway. The duplication here is the sync engine, not a struct.

## Decision

**Adopt (A).** Implementer instructions:

1. Add `pub enum CalendarItem { Event(Event), Todo(Todo) }` in `caldir-core/src/item.rs`,
   re-exported from `lib.rs`. It is the type stored, diffed, keyed, and serialized.
   `CalendarEvent.event: Event` becomes `CalendarItem`; `RemoteEvent` and `EventChange`
   carry `CalendarItem`.
2. **`Event` is not modified.** `start` stays `EventTime`, non-optional. Its educe
   `PartialEq`, `occurs_in_range`, `has_occurrence_in_range`, `base_slug`, `expand_in_range`
   and both codec impls keep their current signatures and semantics. **Q2 is answered no.**
3. Add `pub struct Todo` (`caldir-core/src/todo.rs`) with its **own** time fields:
   `start: Option<EventTime>`, `due: Option<EventTime>`, `completed: Option<DateTime<Utc>>`,
   `percent_complete: Option<u8>`, `priority: Option<u8>`, `status: TodoStatus`. No `end`,
   no `availability` (TRANSP is VEVENT-only, RFC 5545 §3.8.2.7).
4. Before writing `Todo`'s `PartialEq`, move `x_properties_eq`/`attachments_eq`
   (`event.rs:282,289`) to a `pub(crate)` shared module and point both structs' educe attrs
   at the same fns — making equality drift (R7) structurally impossible for those fields.
   `last_modified`/`sequence` must be `#[educe(PartialEq(ignore))]` on `Todo` too.
5. Replace `event.rs:101` with one ordered pass over `icalendar.components`, matching
   `CalendarComponent::Event | ::Todo | _ => skip`, returning `Vec<Result<CalendarItem,..>>`.
   One pass, not `events()` then `todos()`, so document order survives.
6. Factor the shared codec half into `read_shared<C: EventLike>` / `write_shared<C: EventLike>`
   so `Todo`'s codec is ~30 lines of VTODO-specific mapping, not a 110-line copy. **Never
   call `ends()`/`all_day()` on a `Todo`** (C4 — the library writes an RFC-invalid `DTEND`).
7. **`EventInstanceId` remains the single key type, unchanged, with no component axis.** Its
   string form is on-disk contract; a component axis would invalidate every user's
   `known_event_ids` and `bases/`. Cross-component UID collision is handled by
   self-description: a base's ICS names its own component, so a kind-mismatched base is
   treated as **no base** rather than compared.
8. VJOURNAL later = one variant + one struct + one match arm. No second redesign.
9. `specs/caldir.md` amendment (**required**): scope §"Fields We Skip" so `PRIORITY` is
   supported **for VTODO only**; `CATEGORIES`/`RESOURCES`/`COMMENT`/`CONTACT` stay skipped
   for both. Add a §"To-dos" for `DUE`/`COMPLETED`/`PERCENT-COMPLETE`/`STATUS`.

### Duplication vs. god-type — which cost I'd rather pay

Of `Event`'s 19 fields, `Todo` genuinely shares 15 (`uid`, `summary`, `description`,
`location`, `visibility`, `recurrence`, `recurrence_id`, `organizer`, `attendees`,
`reminders`, `url`, `attachments`, `x_properties`, `last_modified`, `sequence` — all valid
VTODO properties, RFC 5545 §3.6.2). Only `end` and `availability` are excluded; only
`start`'s optionality differs. So (A) duplicates ~15 field *declarations* and two educe
lines. That is the honest price, and I would rather pay it: the duplication is declarative
and diff-visible — drift shows up as a missing line in a struct, caught by a mirrored test.
The god-type's cost is invisible and behavioural: every `Option<EventTime>` unwrap in
`occurrences.rs`/`slugify.rs`/`occurs_in_range` becomes a decision about a component the
author wasn't thinking about, inside the 220 tests that pin VEVENT behaviour today.
Duplicated *code* (vs. field lines) is bounded to ~60 lines by step 6, and the sync engine —
where duplication would actually hurt (option D) — is not duplicated.

## Consequences

Easy: VJOURNAL; per-component filenames and status rules; "no fake events" enforced by
`match`. One diff engine, one state layer, one wire codec.
Hard: 70 `.event()` call sites re-pointed. Containment: keep
`CalendarEvent::event() -> Option<&Event>` as a narrowing accessor so VEVENT-only callers
(`rsvp.rs`, `invites.rs`, `duplicate_file.rs`, google/outlook mappers) change by adding a
`let Some(e) = .. else { continue }` rather than being rewritten.

Must be tested: (i) the 686 baseline green with **zero** VEVENT test edits — any required
edit to an existing `Event` test is a design failure, not a fixup; (ii) mirrored equality
tests for `Todo` (same `last_modified`/different `sequence` -> equal; reordered x-props ->
equal; reordered attachments -> equal); (iii) a mixed VEVENT+VTODO file; (iv) a `Todo`
round-trip asserting no `DTEND` is emitted; (v) TZ matrix on every `DUE`/`COMPLETED` case.

## Backward-compatibility impact

- **Local files**: unchanged — `Event` serialization is byte-identical (untouched). VTODO
  files that previously poisoned a calendar listing now load.
- **`.caldir/` state**: same format, same keys. `bases/<sha256(id)>.ics` may now hold a
  `BEGIN:VTODO` body; an older caldir reading one gets `UnexpectedEventCount{found:0}` and
  already degrades to the legacy known-id path (`event_bases.rs:60-62`) — graceful.
- **RPC wire**: unchanged. No new `Method` verb (A20 respected); ICS payloads stay opaque
  strings, only the `BEGIN:` line differs.
- **Installed provider binaries**: unaffected — an old binary never receives a VTODO because
  it never returns one. Component negotiation is deferred (Q9: an additive `Option<…>` field
  on `CalendarConfig`, wire-safe — `deny_unknown_fields` appears nowhere).

## Rejected because

- **(B)**: 41 compile errors across 7 files holding 220 VEVENT tests before the CLI or any
  provider is reached — and it makes fake events representable.
- **(C)**: `SyncBases` needs one heterogeneous map, so `dyn` loses the educe `PartialEq` that
  drives sync direction; generics infect the whole stack and still need an enum on disk.
- **(D)**: duplicates `CalendarDiff::compute` and the connection apply loops — the sync
  engine, not a struct — and still collides in the single `bases/` layout.

## Open sub-questions

1. **UID collision between a VEVENT and a VTODO (A24/R4).** I chose "single key type +
   kind-mismatched base treated as absent", but CalDAV derives the href from the UID alone
   (`caldav/client.rs:73-76`), so the *server* may overwrite. Not settleable from source.
   **Cheapest experiment:** PUT a VEVENT and a VTODO with the same UID to Radicale, then
   list. If the second overwrites the first, the CalDAV ADR must component-qualify the href;
   this ADR is unaffected either way.
2. **Whether `Todo` keeps `start` at all.** RFC 5545 permits `DTSTART` in a VTODO and it is
   what makes `Reminder::minutes_before_start` and any time-range filter meaningful, but it
   re-opens "todo that looks like an event". I kept it `Option<EventTime>`. **Cheapest
   experiment:** none in code — check whether real Radicale/iCloud task payloads carry
   `DTSTART` (Stage 8 u_crawler bytes answer it).
3. **UNVERIFIED:** the CLI render surface for a `Todo` (`output/agenda*.rs`) — files counted,
   rendering not designed. Belongs to a CLI-facing ADR.
