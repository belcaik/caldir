# caldir-core domain model: Calendar, Event, identity, time, recurrence

Scope: `caldir-core/src/event.rs`, `caldir-core/src/event/**`, `caldir-core/src/calendar.rs`,
`caldir-core/src/calendar/**`, `caldir-core/src/lib.rs`. Read-only survey, branch `vtodo-support`.

## Type inventory

Event domain:

- `Event` — the single component type; a parsed VEVENT. `caldir-core/src/event.rs:40`
- `EventInstanceId` — `(EventUid, Option<RecurrenceId>)` newtype; the universal identity. `caldir-core/src/event/instance_id.rs:13`
- `EventUid` — newtype over `String` (RFC 5545 UID). `caldir-core/src/event/instance_id/event_uid.rs:3`
- `RecurrenceId` — newtype over `EventTime`; identifies one instance of a series. `caldir-core/src/event/instance_id/recurrence_id.rs:7`
- `EventTime` — 4-variant time enum (date / UTC / floating / zoned). `caldir-core/src/event/time.rs:6`
- `NormalizedEventTime` — `pub(crate)` comparable form of `EventTime`. `caldir-core/src/event/time.rs:19`
- `Recurrence` — RRULE string + EXDATE/RDATE lists. `caldir-core/src/event/recurrence.rs:5`
- `expand_in_range(events, from, to) -> Vec<Event>` — occurrence expansion. `caldir-core/src/event/occurrences.rs:10`
- `Status` — Confirmed / Tentative / Cancelled, `Default = Confirmed`. `caldir-core/src/event/status.rs:5`
- `Availability` — Busy / Free (TRANSP), `Default = Busy`. `caldir-core/src/event/availability.rs:5`
- `Visibility` — Public / Private / Confidential (CLASS), no Default. `caldir-core/src/event/visibility.rs:5`
- `Attendee` — email + optional name/PARTSTAT. `caldir-core/src/event/attendee.rs:8`
- `ParticipationStatus` — PARTSTAT enum. `caldir-core/src/event/attendee/participation_status.rs:5`
- `Organizer` — email + optional name. `caldir-core/src/event/organizer.rs:5`
- `Reminder` — `minutes_before_start: i64` (VALARM). `caldir-core/src/event/reminder.rs:13`
- `Attachment` — ATTACH uri + params. `caldir-core/src/event/attachment.rs:4`
- `XProperty` — name/value/params for `X-*` props. `caldir-core/src/event/x_property.rs:4`
- `EventError` — parse/IO/identity errors. `caldir-core/src/event/error.rs:4`
- `tz_normalize::normalize` / `::from_iana` — Windows↔IANA tzid mapping. `caldir-core/src/event/tz_normalize.rs:85,150`

Calendar domain:

- `Calendar` — a directory of `.ics` files. `caldir-core/src/calendar.rs:40`
- `CalendarEvent` — an `Event` bound to a file path. `caldir-core/src/calendar/event.rs:12`
- `CalendarConfig` — `.caldir/config.toml`. `caldir-core/src/calendar/config.rs:10`
- `CalendarState` — wraps `SyncBases`. `caldir-core/src/calendar/state.rs:19`
- `SyncBases` (`pub(crate)`) — `HashMap<EventInstanceId, Option<Box<Event>>>`. `caldir-core/src/calendar/state/sync_bases.rs:12`
- `KnownEventIds` (`pub(crate)`) — `HashSet<EventInstanceId>`. `caldir-core/src/calendar/state/known_event_ids.rs:21`
- `EventBases` (`pub(crate)`) — `HashMap<EventInstanceId, Event>`, `.caldir/state/bases/`. `caldir-core/src/calendar/state/event_bases.rs:10`
- Errors: `CalendarError` `calendar/error.rs:8`, `CalendarEventError` `calendar/event/error.rs:6`,
  `CalendarStateError` `calendar/state/error.rs:2`, `CalendarConfigError` `calendar/config/error.rs:4`.

Re-exported from the crate root at `caldir-core/src/lib.rs:16-23` (`Calendar`, `CalendarConfig`,
`CalendarEvent`, and the whole event set).

## The `Event` struct

`caldir-core/src/event.rs:38-68`:

```rust
#[derive(Debug, Clone, Eq, educe::Educe)]
#[educe(PartialEq)]
pub struct Event {
    pub uid: EventUid,
    pub summary: Option<String>,
    pub description: Option<String>,
    pub location: Option<String>,
    pub start: EventTime,
    pub end: Option<EventTime>,
    pub status: Status,
    pub availability: Availability,
    pub visibility: Option<Visibility>,
    pub recurrence: Option<Recurrence>,
    pub recurrence_id: Option<RecurrenceId>,
    pub organizer: Option<Organizer>,
    pub attendees: Vec<Attendee>,
    pub reminders: Vec<Reminder>,
    pub url: Option<String>,
    #[educe(PartialEq(method(attachments_eq)))]
    pub attachments: Vec<Attachment>,
    #[educe(PartialEq(method(x_properties_eq)))]
    pub x_properties: Vec<XProperty>,
    #[educe(PartialEq(ignore))]
    pub last_modified: Option<DateTime<Utc>>,
    #[educe(PartialEq(ignore))]
    pub sequence: i32,
}
```

Optional fields: `summary`, `description`, `location`, `end`, `visibility`, `recurrence`,
`recurrence_id`, `organizer`, `url`, `last_modified`. Required: `uid`, `start`, plus the
defaulted enums `status`/`availability` and the `Vec` fields (empty = absent).

Equality semantics (`event.rs:283-292`): `last_modified` and `sequence` are ignored;
`x_properties` compare order-independently by name+value; `attachments` compare
order-independently by `uri` only. This makes `==` a "content changed?" test for sync.

Notable inherent methods: `new` (:71), `from_ics_str` (:96), `to_ics_string` (:123),
`occurs_in_range` (:136), `has_occurrence_in_range` (:146 — expands RRULE),
`is_invite_for`/`attendee_status`/`find_attendee`/`set_attendee_status` (:156-198),
`with_new_uid` (:206), `x_property(name)` (:214), `event_instance_id()` (:252),
`with_x_properties_merged_from` (:257), `base_slug()` (`event/slugify.rs:13`),
`occurrence_at` (`event/occurrences.rs:105`, `pub(crate)`).

UID minting: `new_uid()` (`event.rs:276`) = `"{uuid_v4}@caldir"` (`ICS_UID_DOMAIN`, :36).
ICS output is written with `PRODID:CALDIR`, `VERSION:2.0` (:34-35) and VALARMs spliced in
by string replacement on `END:VEVENT` (`event.rs:242-250`).

## Identity

```rust
pub struct EventInstanceId((EventUid, Option<RecurrenceId>));  // instance_id.rs:13
pub struct EventUid(String);                                   // event_uid.rs:3
pub struct RecurrenceId(EventTime);                            // recurrence_id.rs:7
```

- `EventInstanceId::new(uid, recurrence_id)`, accessors `uid() -> &EventUid`,
  `recurrence_id() -> Option<&RecurrenceId>`. Derives `PartialEq, Eq, Hash` — it is the key
  type for every state map (`SyncBases`, `KnownEventIds`, `EventBases`).
- `EventUid` is opaque `String` with `new(impl Into<String>)` / `as_str()`; derives Eq+Hash
  (byte equality, case-sensitive).
- `RecurrenceId` derives only `Debug, Clone, Eq`; `PartialEq` and `Hash` are hand-written
  (`recurrence_id.rs:25-35`) over `EventTime::normalized()`, so two rids naming the same
  instant in different tzids/formats are equal and hash alike. Invariant: any `Hash`/`Eq`
  use must go through `normalized()`.
- String form (`instance_id.rs:29-115`) — `Display` + `From<&str>`/`From<String>`:
  - non-recurring: `{uid}`
  - instance: `{uid}__{rid}` with rid one of `YYYYMMDD`, `YYYYMMDDTHHMMSSZ`,
    `YYYYMMDDTHHMMSS`, `TZID={tzid}:YYYYMMDDTHHMMSS`.
  - Parsing is **infallible**: `rsplit_once("__")`, and if the suffix isn't a parseable
    recurrence id the whole string is the UID (UIDs may legitimately contain `__`).
- A recurring **master** is `recurrence.is_some()`; an **override** is `recurrence_id.is_some()`;
  a plain event is neither (`occurrences.rs:19-28`, `calendar.rs:143-150`).

## Time (`caldir-core/src/event/time.rs`)

```rust
pub enum EventTime {
    Date(NaiveDate),                                            // all-day (VALUE=DATE)
    DateTimeUtc(DateTime<Utc>),                                 // ...Z
    DateTimeFloating(NaiveDateTime),                            // no zone
    DateTimeZoned { datetime: NaiveDateTime, tzid: String },     // TZID=IANA
}
```

- Derives `PartialEq, Eq, Hash` structurally (raw form). Semantic comparison goes through
  `pub(crate) fn normalized() -> NormalizedEventTime` (:51) which collapses to
  `Date | Instant(DateTime<Utc>) | Floating(NaiveDateTime)`; an unresolvable tzid degrades
  to `Floating` with a warning.
- `is_date()` (:47) is the all-day test. `to_local_tz<Tz>(&self, tz)` (:26) renders in a
  target zone — `Date` becomes midnight, floating is interpreted as local. `to_utc()` (:42)
  is `to_local_tz(&chrono::Local)` — i.e. floating/unknown-zone times resolve against the
  **system** zone.
- DST handling: `resolve_local` (:79) takes the earliest candidate; on a spring-forward gap
  it bumps one hour forward.
- ICS boundary: `From<&EventTime>/From<EventTime> for DatePerhapsTime` (:90,:105) and
  `From<DatePerhapsTime> for EventTime` (:111). Inbound TZIDs go through
  `tz_normalize::classify` → `Tzid::Iana` (kept as zoned), `Tzid::FixedOffset(secs)`
  (converted to `DateTimeUtc`), or `Tzid::Unknown` (kept verbatim + warning).
- `tz_normalize` (`event/tz_normalize.rs`) is the Windows↔IANA map: `normalize(String)` (:85)
  inbound, `from_iana(&str)` (:150) for the outbound Microsoft edge.
- Filenames use local time: `Event::time_slug` formats `%Y-%m-%d` for `Date`, otherwise
  `%Y-%m-%dT%H%M` in `chrono::Local` (`event/slugify.rs:47-59`).

## Recurrence

```rust
pub struct Recurrence {          // recurrence.rs:5
    pub rrule: String,           // raw RRULE value, deliberately untyped
    pub exdates: Vec<EventTime>,
    pub rdates: Vec<EventTime>,
}
```

- `rrule` stays a string for robustness (odd provider RRULEs must not fail a load) and to
  avoid re-serialization drift causing spurious diffs (:8-17). `rrule::RRule` is parsed
  ad hoc at call sites.
- `PartialEq` is hand-written (:32-38): EXDATE/RDATE compare as sets, order-independent,
  sorted by `to_utc()`.
- RDATE PERIOD values are not modelled and are silently dropped (:23-26).
- `Recurrence::new(rrule)` (:47), `from_ical_event` (:55, `pub(crate)`),
  `truncate_before(dtstart, before)` (:82 — strips UNTIL/COUNT, injects a fresh UNTIL,
  drops EXDATEs at/after the cut; the primitive behind "this and future"),
  `apply_to(&mut icalendar::Event)` (:101).
- Expansion (`event/occurrences.rs`): `expand_in_range` (:10) buckets input into singles,
  masters, and overrides keyed `HashMap<String /*uid*/, HashMap<RecurrenceId, Event>>`,
  then `expand_master` (:56) builds an `RRuleSet` string from `master.start` + recurrence,
  caps at `.all(366)` occurrences, and per occurrence either substitutes the override
  (skipping `Status::Cancelled`) or synthesizes an instance. Synthesized instances clear
  `recurrence`, set `recurrence_id`, and shift `end` by the master's duration
  (`synthesize_instance` :116). Orphan overrides (no master present) are emitted as-is.
  Unparseable RRULE falls back to the master alone.
- Slugs: recurring masters get a `_recurring__` filename prefix instead of a date
  (`event/slugify.rs:14-19`).

## X-properties

```rust
pub struct XProperty { pub name: String, pub value: String, pub params: Vec<(String, String)> }
```

- Inbound: every property whose key starts with `X-` is captured verbatim, params included
  (`event/from_icalendar.rs:60-65`). Outbound: each is re-emitted as a `Property`
  (`event/to_icalendar.rs:93-95`, `x_property.rs:28-36`).
- `PartialEq` ignores `params` (`x_property.rs:12-16`) — name+value only, for backwards
  compatibility. Event-level comparison is additionally order-independent (`event.rs:283`).
- `Event::x_property(name)` reads the first match (`event.rs:214`).
- `with_x_properties_merged_from(base)` (`event.rs:257`) unions provider-managed props and
  params from a base event into a local copy so a local write can't strip remote metadata.
- This is the storage for provider-specific IDs (per CLAUDE.md), keeping them out of the
  identity model.

## `Calendar`

```rust
pub struct Calendar { path: PathBuf, config: Option<CalendarConfig>, state: CalendarState }  // calendar.rs:40
pub struct CalendarEvent { event: Event, path: PathBuf }                                      // calendar/event.rs:12
```

- A calendar *is* a directory. `.caldir/` holds `config.toml` and `state/`
  (`calendar.rs:19-37`). `config` is `None` for a local-only calendar; `state` is always
  present (`CalendarState::new()` when empty).
- `CalendarConfig` (`calendar/config.rs:10`) holds `name`, `color`, `read_only`, and
  `remote_config: Option<RemoteConfig>` (serialized as `[remote]`).
- Event access: `events()` (:106) loads every `*.ics` file in the directory (one event per
  file — `CalendarEvent::load` errors on multi-event files); `event(slug)` (:122),
  `event_by_instance_id(&EventInstanceId)` (:128, linear scan),
  `master_event_for(uid)` (:143), `expanded_events_in_range` (:328) delegating to
  `expand_in_range`, `create_event`/`delete_event` (:338,:344).
- Recurrence-aware mutations live on `Calendar`: `split_recurring_series_at` (:182),
  `delete_recurring_instance` (:244, writes an EXDATE or deletes the override),
  `update_recurring_instance` (:287).
- `CalendarEvent` owns the file: `create` (:18) picks a filename from `event.base_slug()`
  and syncs the file mtime to `last_modified`; `update` (:55) may rename; `delete` (:80);
  `modified_at()` (:97) is the file mtime that the sync direction rule compares against
  remote `LAST-MODIFIED`; `add_exdate` (:120, `pub(crate)`).
- State keys everything by `EventInstanceId`: `SyncBases` maps id → `Option<Box<Event>>`
  (`None` = known id with no stored base), backed by the append-only `known_event_ids` file
  and the `bases/` directory.

## Seams for a sibling component type (VTODO)

Facts about what is VEVENT-bound today:

- Component-agnostic as written: `EventUid`, `RecurrenceId`, `EventInstanceId`, `EventTime`,
  `NormalizedEventTime`, `tz_normalize`, `Recurrence` (RRULE/EXDATE/RDATE are shared
  properties), `XProperty`, `Attachment`, `Attendee`/`ParticipationStatus`, `Organizer`,
  `Reminder` (though its field is literally `minutes_before_start`), `Visibility`,
  and the file/state machinery keyed by `EventInstanceId` (`SyncBases`, `KnownEventIds`,
  `EventBases`).
- VEVENT-specific by name only: the `Event*` prefixes on the identity and time types;
  `EventError::MissingStart` / `MissingUid`.
- VEVENT-specific by semantics: `Event.start: EventTime` is non-optional and
  `Event.end`/`Availability` (TRANSP) are VEVENT properties; VTODO uses DUE/COMPLETED/
  PERCENT-COMPLETE/PRIORITY and may have no DTSTART. `Status`'s three variants are the
  VEVENT set (VTODO adds NEEDS-ACTION/COMPLETED/IN-PROCESS).
- VEVENT-specific by parsing/serialization: `Event::from_ics_str` iterates
  `icalendar.events()` only (`event.rs:101`); `TryFrom<&icalendar::Event>`
  (`from_icalendar.rs:7`) and `From<&Event> for icalendar::Event` (`to_icalendar.rs:4`);
  `splice_valarms_into_vevent` string-replaces on the literal `"END:VEVENT\r\n"`
  (`event.rs:249`).
- Occurrence logic is VEVENT-shaped in one place: `master_duration`/`shift_end_time`
  derive an instance `end` from `start`+duration (`occurrences.rs:110-130`).
- Calendar-level assumptions: `Calendar::events()` matches any `*.ics` file and
  `CalendarEvent::load` requires exactly one VEVENT per file — a `.ics` containing only a
  VTODO yields zero events and fails with `ExpectedSingleEvent { found: 0 }` (`calendar/event.rs:37-50`).
- Slug/filename derivation (`event/slugify.rs`) is keyed on `start` and `recurrence`.
