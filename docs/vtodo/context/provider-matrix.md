# Provider matrix: VEVENT assumptions and remote task APIs

Scope: `caldir-provider-{google,outlook,icloud,webcal}` on branch `vtodo-support`.
`caldir-provider-caldav` is deliberately not analyzed here (owned separately); it is
referenced only where iCloud consumes it as a library.

Shared baseline: every provider implements `caldir_core::provider::Handler`
(`caldir-core/src/provider/handler.rs:27`), whose whole vocabulary is
`list_events` / `create_event` / `update_event` / `delete_event` over
`caldir_core::Event`. There is no task-shaped RPC method
(`caldir-core/src/provider/handler.rs:28-48`), so *no* provider can carry a VTODO
today regardless of what its remote offers. Unimplemented methods default to an
error string ("This provider does not support …").

---

## Google (`caldir-provider-google`)

**Transport.** Google Calendar REST v3 via the `google-calendar` crate 0.10
(`caldir-provider-google/Cargo.toml:24`), OAuth2 with scopes
`calendar.calendarlist.readonly` + `calendar.events`
(`caldir-provider-google/src/commands/connect.rs:23-26`).

**Fetch → Event.** `client.events().list_all(...)`
(`caldir-provider-google/src/commands/list_events.rs:30-47`) → `process_google_events`
(`.../list_events.rs:65`) → `Event::from_google`
(`caldir-provider-google/src/google_event/from_google.rs:15-16`). Outbound mapping is
`Event::to_google` (`caldir-provider-google/src/google_event/to_google.rs:12-13`).
Provider id round-trips as `X-GOOGLE-EVENT-ID`
(`caldir-provider-google/src/constants.rs:2`).

**Hard VEVENT assumptions.**
1. The only collection touched is `events()` — list/insert/update/delete
   (`list_events.rs:30`, `create_event.rs:85`, `update_event.rs` via
   `events().update`, `delete_event.rs:33-34`). The Calendar API has no task
   resource at all.
2. `from_google` hard-fails when `start` or `end` is missing
   (`from_google.rs:17-22`, "Event has no start time"). A task-shaped item with
   only a due date could not be mapped.
3. Cancelled-instance tombstones are synthesized into instance-override *events*
   (`list_events.rs:100-141`) — recurrence handling is entirely VEVENT-framed.
4. `Availability` ← Google `transparency` opaque/transparent
   (`from_google.rs:50-54`, `to_google.rs:27-30`); `Status` ← confirmed /
   tentative / cancelled (`from_google.rs:24-28`) — the VEVENT status set, not
   VTODO's NEEDS-ACTION / COMPLETED / IN-PROCESS.
5. Reminder semantics assume "minutes before start" (`to_google.rs:40-48`).

**Task API on the remote.** Nothing in this crate references one. Google Tasks is
a separate product/API surface from Calendar v3 (distinct endpoint and OAuth
scope); the code here neither requests nor knows about it. *Verify against Google
vendor docs before relying on specifics* (endpoint shape, scope name, whether
Tasks exposes RFC 5545 semantics at all) — nothing in-repo attests to it.

**Write capability.** Full push: create/update/delete are all implemented
(`caldir-provider-google/src/main.rs:32-42`). Read-only is per calendar, derived
from `accessRole` at discovery time — `!matches!(cal.access_role, "writer"|"owner")`
(`caldir-provider-google/src/commands/list_calendars.rs:39-44`) — and enforced in
core, not in the provider: `Connection::diff` calls `diff.discard_outgoing()`
when `read_only()` (`caldir-core/src/connection.rs:30-35, 57-59`).

---

## Outlook (`caldir-provider-outlook`)

**Transport.** Microsoft Graph v1.0 REST, hand-rolled client over `reqwest`;
base URL `https://graph.microsoft.com/v1.0`
(`caldir-provider-outlook/src/graph_api/client.rs:7`), verbs at
`client.rs:22/45/62/79`. OAuth scopes `Calendars.ReadWrite User.Read offline_access`
(`caldir-provider-outlook/src/commands/connect.rs:25`).

**Fetch → Event.** `GET /me/calendars/{id}/events?$select=…`
(`caldir-provider-outlook/src/commands/list_events.rs:39-42`), plus one
`GET /me/events/{master}/instances` per series master to recover `type=exception`
overrides (`.../list_events.rs:116-148`). Mapping is
`from_outlook(GraphEvent, account) -> Event`
(`caldir-provider-outlook/src/outlook_event/from_outlook.rs:18`); outbound
`to_outlook` (`caldir-provider-outlook/src/outlook_event/to_outlook.rs`).
Wire type `GraphEvent` at `caldir-provider-outlook/src/graph_api/types/mod.rs:42`.

**Hard VEVENT assumptions.**
1. Every path is `/me/events` or `/me/calendars/{id}/events`:
   list `list_events.rs:40`, instances `list_events.rs:127`, create
   `create_event.rs:74`, patch `update_event.rs:41/66`, action
   `update_event.rs:74`, delete `delete_event.rs:30`. Graph's `/me/todo/*` is
   never touched.
2. `GraphEvent` is a calendar-event schema: `start`/`end` as `DateTimeTimeZone`,
   `isAllDay`, `showAs`, `isCancelled`, `originalStart`, `type`
   (`graph_api/types/mod.rs:52-108`). No `dueDateTime`, `status`, `completedDateTime`,
   or `percentComplete` fields exist.
3. Recurrence identity is Graph's `seriesMaster` / `occurrence` / `exception`
   trichotomy (`list_events.rs:29-34, 71-72`, `from_outlook.rs:636-643`).
4. `isAllDay` drives all time parsing (`from_outlook.rs:206-239`).
5. Reminders are the single scalar `reminderMinutesBeforeStart`
   (`graph_api/types/mod.rs:76`), i.e. relative to a start time.

**Task API on the remote.** Not referenced anywhere in the crate. Microsoft Graph
does expose a separate To Do surface (`/me/todo/lists`, `todoTask`) with its own
scope family, distinct from `Calendars.ReadWrite`; that is not evidenced by this
code — *verify against Microsoft Graph docs* before designing on it, and note the
current OAuth consent would not cover it.

**Write capability.** Full push (`caldir-provider-outlook/src/main.rs:33-43`).
Read-only per calendar from Graph's `canEdit`: `let read_only = !cal.can_edit`
(`caldir-provider-outlook/src/commands/list_calendars.rs:38-44`, field at
`graph_api/types/mod.rs:36`); enforced in core at `caldir-core/src/connection.rs:57-59`.

---

## iCloud (`caldir-provider-icloud`)

**Transport.** CalDAV against `https://caldav.icloud.com`
(`caldir-provider-icloud/src/constants.rs:2`), with Apple-ID + app-password
credentials. **It re-uses `caldir-provider-caldav` as a library** — dependency at
`caldir-provider-icloud/Cargo.toml:19-20`, and every command delegates to
`caldir_provider_caldav::caldav::ops`:
- `ops::discover_endpoints` — `commands/connect.rs:33`
- `ops::list_calendars_raw` (+ `RawCalendar`) — `commands/list_calendars.rs:18-19`
- `ops::fetch_events` — `commands/list_events.rs:19-25`
- `ops::create_event` — `commands/create_event.rs:19`
- `ops::update_event` — `commands/update_event.rs:19`
- `ops::delete_event` — `commands/delete_event.rs:18`

So iCloud owns only credentials, session storage, remote-config shape, and colour
normalisation; all protocol behaviour (REPORT bodies, comp-filters, ICS parsing)
lives in the caldav crate and is out of scope here.

**Fetch → Event.** `ops::fetch_events(user, pass, calendar_url, from, to)` returns
`Vec<Event>` directly (`commands/list_events.rs:19-26`) — there is no
iCloud-specific mapping layer, no `from_icloud`/`to_icloud` module.

**Hard VEVENT assumptions.** None are authored in this crate; they are inherited
wholesale from `caldir-provider-caldav::caldav::ops` (the only iCloud-side
assumption is structural: the ops signatures speak `Event` in and `Event` out,
`create_event.rs:19`, `update_event.rs:19`). Whether the CalDAV REPORT filters on
`VEVENT` is a caldav-crate question.

**Task API on the remote.** iCloud Reminders are stored on the same CalDAV
account as VTODO components in their own collections — a CalDAV
`supported-calendar-component-set` matter, not a separate REST API. Nothing in
this crate filters or exposes them; `list_calendars` passes through whatever
`ops::list_calendars_raw` returns (`commands/list_calendars.rs:18-24`). *Verify
Apple's component-set behaviour against the caldav crate's discovery code and
Apple docs* rather than assuming from here.

**Write capability.** Full push (`caldir-provider-icloud/src/main.rs:30-40`).
Read-only is passed straight through from the CalDAV privilege set:
`CalendarConfig::new(Some(cal.name), color, cal.read_only, …)`
(`commands/list_calendars.rs:44`); enforced in core at
`caldir-core/src/connection.rs:57-59`.

---

## WebCal (`caldir-provider-webcal`)

**Transport.** Plain HTTP GET of an `.ics` feed; `webcal://` is rewritten to
`https://` at connect (`caldir-provider-webcal/src/commands/connect.rs:28`), fetch
in `caldir-provider-webcal/src/http.rs:15` (`fetch_feed`), which also captures the
HTTP `Last-Modified` header (`http.rs:49`).

**Fetch → Event.** `http::fetch_feed` then
`Event::from_ics_str(&feed.body)`
(`caldir-provider-webcal/src/commands/list_events.rs:14-32`), with the feed's
`Last-Modified` used as a fallback per event (`list_events.rs:26-31`) and an
in-process window filter (`list_events.rs:38-45`).

**Hard VEVENT assumptions.**
1. The parse boundary is core's `Event::from_ics_str`, which is
   `icalendar.events().map(Event::try_from)`
   (`caldir-core/src/event.rs:96-102`) — VJOURNAL/VTODO/VFREEBUSY components in a
   feed are silently dropped before the provider ever sees them. This is the
   single most load-bearing assumption for webcal and it lives in core, not here.
2. Feed validation only requires `BEGIN:VCALENDAR`
   (`commands/connect.rs:32-36`), so a VTODO-bearing feed connects fine and then
   yields zero items — a silent-empty failure mode.
3. Range filtering is `event.recurrence.is_some() || event.occurs_in_range(...)`
   (`list_events.rs:43`), i.e. DTSTART/DTEND-based occurrence logic.
4. Calendar metadata is scraped from VCALENDAR-level props only — `NAME` and
   `X-APPLE-CALENDAR-COLOR` (`commands/connect.rs:68-78`).

**Task API on the remote.** Not applicable — there is no API, only a static ICS
document. Whether tasks appear is purely a question of what components the
publisher emits; transport-wise VTODO is already arriving in the byte stream and
being discarded at `caldir-core/src/event.rs:102`.

**Write capability.** None. `WebcalProvider` implements only `connect` and
`list_events` (`caldir-provider-webcal/src/main.rs:16-25`), so create/update/delete
fall through to the `Handler` defaults that return "does not support" errors
(`caldir-core/src/provider/handler.rs:38-48`). Additionally, connect hardcodes the
calendar as read-only: `CalendarConfig::new(Some(name), color, Some(true), …)`
(`commands/connect.rs:82-87`), which core honours at
`caldir-core/src/connection.rs:57-59`.

---

## Summary

| Provider | Transport | Can push today | Plausible VTODO stance | Evidence |
|---|---|---|---|---|
| google | Google Calendar REST v3 (`google-calendar` 0.10) | Yes (create/update/delete) | UNSUPPORTED — Calendar v3 has no task resource; Tasks would be a different API + scope (verify vendor docs) | `caldir-provider-google/src/commands/list_events.rs:30`; `connect.rs:23-26`; `google_event/from_google.rs:17-22` |
| outlook | Microsoft Graph v1.0 REST `/me/…/events` | Yes (create/update/delete) | UNSUPPORTED — every path is `/events`; a To Do surface exists on Graph but is unreferenced and outside current scopes (verify vendor docs) | `graph_api/client.rs:7`; `commands/list_events.rs:40,127`; `commands/connect.rs:25`; `graph_api/types/mod.rs:42-108` |
| icloud | CalDAV to `caldav.icloud.com`, all ops delegated to `caldir-provider-caldav` | Yes (create/update/delete) | TRANSPORT-ONLY — CalDAV carries VTODO natively; every assumption lives in the caldav crate, not here | `constants.rs:2`; `Cargo.toml:19-20`; `commands/list_events.rs:19`; `commands/create_event.rs:19` |
| webcal | Plain HTTPS GET of an `.ics` document | No | TRANSPORT-ONLY (read side) — VTODO already arrives in the feed and is dropped by core's VEVENT-only parse | `src/http.rs:15`; `commands/list_events.rs:14-16`; `caldir-core/src/event.rs:96-102`; `src/main.rs:16-25` |

Cross-cutting: even a VTODO-capable transport is blocked by the RPC surface —
`Handler` speaks only `Event` (`caldir-core/src/provider/handler.rs:27-48`) and
`Method` has no task variants (`caldir-core/src/provider/handler.rs:101-108`).
