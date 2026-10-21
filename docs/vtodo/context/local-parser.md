# Local ICS parser path (facts, branch `vtodo-support`)

All paths relative to repo root. Line numbers as of this branch.

## 1. Call chain: directory -> Event

1. `Caldir::calendars()` — `caldir-core/src/caldir.rs:71-83`. `std::fs::read_dir(self.data_dir())` (`caldir.rs:74`); every non-hidden **directory** becomes `Calendar::load(&entry.path())` (`caldir.rs:77`).
2. `Calendar::load(path)` — `caldir-core/src/calendar.rs:71-87`. Reads `.caldir/config.toml` (`calendar.rs:77`) and `.caldir/state` (`calendar.rs:80`). No event files touched here.
3. `Calendar::events()` — `caldir-core/src/calendar.rs:106-119`:
   - `std::fs::read_dir(self.path())` — `calendar.rs:109`
   - filter: `entry.file_type()?.is_file() && path.extension().is_some_and(|ext| ext == "ics")` — `calendar.rs:113`
   - `events.push(CalendarEvent::load(path)?)` — `calendar.rs:114` (the `?` makes one bad file fail the whole listing).
4. `CalendarEvent::load(path)` — `caldir-core/src/calendar/event.rs:28-53`:
   - `path.is_file()` guard -> `CalendarEventError::NotFound` (`calendar/event.rs:31-33`)
   - `std::fs::read_to_string(&path)` — `calendar/event.rs:35`
   - `Event::from_ics_str(&contents)` — `calendar/event.rs:37`
   - single-event narrowing — `calendar/event.rs:40-50`
5. `Event::from_ics_str(contents)` — `caldir-core/src/event.rs:96-102`:
   - `contents.parse::<icalendar::Calendar>()` — `event.rs:97-99`, failure -> `EventError::InvalidIcs`
   - `icalendar.events().map(Event::try_from).collect()` — `event.rs:101`
6. `impl TryFrom<&icalendar::Event> for Event` — `caldir-core/src/event/from_icalendar.rs:7-10`; DTSTART at `from_icalendar.rs:11`, UID at `from_icalendar.rs:22`, struct built at `from_icalendar.rs:67-91`.
7. Collection: `Vec<CalendarEvent>` returned from `calendar.rs:118`; consumers clone `ce.event()` (`calendar/event.rs:84-86`).

Single-file variant (bases, RPC wire): `Event::load_single` — `event.rs:104-109` -> `Event::from_single_ics_str` — `event.rs:111-121`.

`icalendar` is pinned at `0.17.10` (`caldir-core/Cargo.toml:21`, `Cargo.lock:1194-1197`). `icalendar::Calendar::events()` yields only components parsed as `VEVENT`; the crate source is not vendored in this checkout.

## 2. "expected exactly one event ... found 0"

`caldir-core/src/calendar/event.rs:40-50`:

```rust
let event = match <[Result<Event, _>; 1]>::try_from(events) {
    Ok([result]) => {
        result.map_err(|err| CalendarEventError::InvalidEvent(path.clone(), err))?
    }
    Err(events) => {
        return Err(CalendarEventError::ExpectedSingleEvent {
            path,
            found: events.len(),
        });
    }
};
```

Message text: `caldir-core/src/calendar/event/error.rs:10-11`

```rust
#[error("expected exactly one event in {path}, found {found}")]
ExpectedSingleEvent { path: PathBuf, found: usize },
```

`found: 0` happens whenever `icalendar.events()` (`event.rs:101`) yields nothing — a syntactically valid VCALENDAR whose only components are non-VEVENT (VTODO, VJOURNAL, VFREEBUSY) or none at all. Confirmed by `caldir-cli/src/commands/doctor.rs:75-91`, which writes `BEGIN:VCALENDAR\nVERSION:2.0\nEND:VCALENDAR` and gets `DoctorWarning::UnreadableEvents`.

Parallel path for single-string parses: `EventError::UnexpectedEventCount { expected, found }` — raised at `caldir-core/src/event.rs:116-119`, message at `caldir-core/src/event/error.rs:11-12`.

## 3. Every VEVENT-only filter site

`icalendar::Calendar::events()` (the actual VEVENT filter):
- `caldir-core/src/event.rs:101` — the single call site in the workspace. Every other path funnels through `Event::from_ics_str`.

Callers of `Event::from_ics_str` (all therefore VEVENT-only):
- `caldir-core/src/event.rs:112` (`from_single_ics_str`)
- `caldir-core/src/calendar/event.rs:37` (`CalendarEvent::load`)
- `caldir-provider-webcal/src/commands/list_events.rs:16` (malformed events skipped, `list_events.rs:19-25`)
- ~~`caldir-provider-caldav/src/caldav/ops.rs` (`parse_event`, takes first `Ok`)~~ — **FIXED (L5-01).** The symbol is now `parse_item`, it delegates to `CalendarItem::from_single_ics_str`, and it is loud rather than silently first-wins. Hazard A10 is closed.
- `caldir-cli/src/output/agenda_json.rs:471` (test)

Callers of `Event::from_single_ics_str` / `load_single`:
- `caldir-core/src/rpc/ics.rs:34` — RPC wire decode of provider events
- `caldir-core/src/calendar/state/event_bases.rs:34` and `:60` — sync-base `.ics` files
- `caldir-core/src/event.rs:237` (`parse_single_ics`, test-only)

Callers of `Calendar::events()` (all consume only VEVENT-derived `CalendarEvent`s):
- `caldir-core/src/calendar.rs:133` (`event_by_instance_id`), `:145` (`master_event_for`), `:189` (`split_recurring_series_at`), `:255` (`delete_recurring_instance`), `:333` (`expanded_events_in_range`)
- `caldir-core/src/connection.rs:38` (`diff`), `:68` (`apply_incoming_diff`), `:98` (`apply_outgoing_diff`), `:125` (`discard_outgoing_diff`)
- `caldir-cli/src/commands/doctor.rs:55`
- `caldir-cli/src/commands/rsvp.rs:91`
- tests: `caldir-core/src/calendar.rs:467,468,482,552`; `caldir-core/src/connection.rs:530,564,657`; `caldir-cli/src/commands/doctor/warning/duplicate_file.rs:68,84`

Single-event lookups keyed to `.ics` names:
- `Calendar::event(slug)` — `caldir-core/src/calendar.rs:122-126` (`format!("{}.ics", event_slug)`, `calendar.rs:123`)
- `caldir-cli/src/commands/rsvp.rs:46` — `CalendarEvent::load(&path)`

Note: `.events()` in `caldir-provider-google/src/commands/*.rs` (`create_event.rs:56,85,108`, `delete_event.rs:33`, `list_events.rs:30`) is the Google API hub builder, unrelated to icalendar.

## 4. Serialization back to disk

`Event::to_ics_string()` — `caldir-core/src/event.rs:123-134`:
- `let ical_event: icalendar::Event = self.into();` — `event.rs:124`, impl at `caldir-core/src/event/to_icalendar.rs:4-98` (`icalendar::Event::new()` at `to_icalendar.rs:6`, `event.done()` at `to_icalendar.rs:97`)
- wrapper: `icalendar::Calendar::empty().append_property(VERSION).append_property(PRODID).push(ical_event).done().to_string()` — `event.rs:126-131`
- constants: `ICS_PRODID = "CALDIR"`, `ICS_VERSION = "2.0"`, `ICS_UID_DOMAIN = "caldir"` — `event.rs:34-36`; asserted by `event.rs:412-422`
- VALARMs are spliced textually, not pushed as sub-components: `splice_valarms_into_vevent` — `event.rs:242-250`, `ics.replacen("END:VEVENT\r\n", …, 1)` at `event.rs:249` (reason comment `event.rs:240-241`)
- CRLF: line endings come from `icalendar`'s `to_string`; the splice literal `"END:VEVENT\r\n"` (`event.rs:249`) and the round-trip tests that `.replace('\n', "\r\n")` (`event.rs:436`, `event.rs:486`) rely on CRLF output
- Round-trip defaults omitted so bytes match input: STATUS `to_icalendar.rs:18-23`, TRANSP `:25-30`, SEQUENCE `:63-70`; CLASS always written when set `:32-35`
- DTSTAMP is regenerated on every serialization (`event.rs:424-442` asserts it changes)

Write path — `caldir-core/src/calendar/event.rs`:
- `CalendarEvent::create` — `:18-26`: `event.base_slug()` (`:19`), `event.to_ics_string()` (`:20`), `write_best_event_file` (`:22`), `sync_file_mtime` (`:23`)
- `CalendarEvent::update` — `:55-78`: writes new path then removes old (`:69`)
- `write_best_event_file` — `:165-201`: filename `format!("{base_slug}.ics")` (`:175`) or `format!("{base_slug}-{suffix}.ics")` (`:177`); `create_new(true)` collision loop (`:186-198`)
- `sync_file_mtime` — `:153-163`: file mtime pinned to `LAST-MODIFIED` via `filetime` (`:160-161`); no-op when `last_modified` is `None` (`:157-159`)
- `CalendarEvent::delete` — `:80-82`

Slug rules — `caldir-core/src/event/slugify.rs`:
- `base_slug()` — `:13-19`: recurring masters -> `_recurring__{summary_slug}` (`:15`); otherwise `{time_slug}__{summary_slug}` (`:18`)
- `summary_slug()` — `:21-43`: non-alphanumeric/whitespace chars stripped (`:26-29`), then `crate::utils::slugify` (`:33`), empty -> `"untitled"` (`:4`, `:36`)
- `time_slug()` — `:48-60`: `EventTime::Date` -> `%Y-%m-%d` (`:51`); otherwise local-tz `%Y-%m-%dT%H%M` (`:54-58`)

Sync-base files use a different naming scheme: SHA-256 of the `EventInstanceId` string + `.ics` — `caldir-core/src/calendar/state/event_bases.rs:29-30`, `:78-80`, validated by `is_hashed_filename` (`:82-89`).

RPC wire: events are serialized as ICS strings — `caldir-core/src/rpc/ics.rs:28-30`, decoded at `:32-35`.

## 5. Error types

`CalendarEventError` — `caldir-core/src/calendar/event/error.rs:5-24`:
- `InvalidEvent(PathBuf, EventError)` (`:7-8`)
- `ExpectedSingleEvent { path, found }` (`:10-11`)
- `Io(#[from] std::io::Error)` (`:13-14`)
- `NotFound(PathBuf)` (`:16-17`)
- `Event(#[from] EventError)` (`:19-20`)
- `NotRecurring(String)` (`:22-23`)

`EventError` — `caldir-core/src/event/error.rs:3-22`:
- `Io(PathBuf, std::io::Error)` (`:5-6`)
- `InvalidIcs(String, String)` (`:8-9`)
- `UnexpectedEventCount { expected, found }` (`:11-12`)
- `MissingStart` (`:14-15`)
- `MissingUid` (`:17-18`)
- `AttendeeNotFound { email }` (`:20-21`)

`CalendarError` — `caldir-core/src/calendar/error.rs`; `Calendar::events()` returns it, wrapping `CalendarEventError`. `DoctorWarning::UnreadableEvents(String)` — `caldir-cli/src/commands/doctor/warning.rs:13`, produced at `caldir-cli/src/commands/doctor.rs:57`.

## 6. VEVENT-only invariants (testable claims)

1. Only `VEVENT` components ever enter the domain: `icalendar.events()` is the sole component source — `caldir-core/src/event.rs:101`. A VCALENDAR containing only VTODOs parses OK and yields an empty `Vec`.
2. A calendar directory file is considered an event solely by the `.ics` extension, regardless of the component inside — `caldir-core/src/calendar.rs:113`.
3. Every `.ics` file in a calendar directory must contain exactly one VEVENT; zero or 2+ yields `ExpectedSingleEvent` — `caldir-core/src/calendar/event.rs:40-50`.
4. One unloadable file fails the entire `Calendar::events()` listing (`?` propagation) — `caldir-core/src/calendar.rs:114`; observed as a whole-calendar `UnreadableEvents` warning in `caldir-cli/src/commands/doctor.rs:55-58`.
5. The only ICS -> domain conversion impl is `TryFrom<&icalendar::Event>` — `caldir-core/src/event/from_icalendar.rs:7-10`. No VTODO/VJOURNAL conversion exists.
6. A domain `Event` requires a DTSTART: `value.get_start().ok_or(EventError::MissingStart)?` — `caldir-core/src/event/from_icalendar.rs:11`; the field is non-optional `start: EventTime` — `caldir-core/src/event.rs:45`.
7. A domain `Event` requires a UID: `value.get_uid().ok_or(EventError::MissingUid)?` — `caldir-core/src/event/from_icalendar.rs:22`.
8. `end` is the only VEVENT terminal field; there is no DUE/COMPLETED/PERCENT-COMPLETE/priority field on `Event` — struct at `caldir-core/src/event.rs:40-68`.
9. `Status` is the VEVENT status enum (CONFIRMED default) — `caldir-core/src/event/status.rs`, defaulted at `caldir-core/src/event/from_icalendar.rs:34-37`; VTODO statuses (NEEDS-ACTION/COMPLETED) have no representation.
10. Serialization always emits a VEVENT: `From<&Event> for icalendar::Event` — `caldir-core/src/event/to_icalendar.rs:4-98`, pushed into the VCALENDAR at `caldir-core/src/event.rs:129`.
11. The VCALENDAR wrapper always carries exactly `VERSION:2.0` and `PRODID:CALDIR` — `caldir-core/src/event.rs:126-128`, constants `event.rs:34-35`, asserted `event.rs:412-422`.
12. Reminder serialization is keyed to the literal string `"END:VEVENT\r\n"`; no VEVENT marker means no VALARM is emitted — `caldir-core/src/event.rs:249`.
13. Event identity is `(uid, recurrence_id)` — `Event::event_instance_id` at `caldir-core/src/event.rs:252-254`; used for the sync-base filename hash at `caldir-core/src/calendar/state/event_bases.rs:29`.
14. Filenames are derived from VEVENT DTSTART + SUMMARY (or `_recurring__` for RRULE masters) — `caldir-core/src/event/slugify.rs:13-19`, `:48-60`; there is no naming rule for a component without a start time.
15. File mtime is pinned to VEVENT `LAST-MODIFIED`, which is the sync direction signal — `caldir-core/src/calendar/event.rs:153-163`.
16. Sync-base `.ics` files under `.caldir/state/bases` are also parsed as exactly one VEVENT; a non-VEVENT base is silently skipped — `caldir-core/src/calendar/state/event_bases.rs:34`, `:60-62`.
17. The provider RPC wire type carries a single VEVENT per event — `caldir-core/src/rpc/ics.rs:29` (encode) and `:34` (decode via `from_single_ics_str`).
18. CalDAV resource parsing takes the first parsable VEVENT and drops everything else — `caldir-provider-caldav/src/caldav/ops.rs:32-37`; the server-side filter is `<C:comp-filter name="VEVENT">` — `caldir-provider-caldav/src/caldav/ops/list_events.rs:103`, `caldir-provider-caldav/src/caldav/ops.rs:107`.
19. Webcal feed parsing keeps only VEVENTs and logs+skips per-event parse errors — `caldir-provider-webcal/src/commands/list_events.rs:16-32`.
20. Recurrence expansion and range filtering operate on VEVENT semantics (DTSTART/DTEND overlap) — `caldir-core/src/event.rs:136-153`, `caldir-core/src/calendar.rs:328-335`.
