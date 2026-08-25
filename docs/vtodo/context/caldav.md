# Context: `caldir-provider-caldav`

Scope: `caldir-provider-caldav/**` only. Facts as of branch `vtodo-support` (tree identical to `main` @ 2ed74fa for this crate). Read-only survey.

## Crate layout

```
src/main.rs                        # provider::Handler impl -> commands::*
src/commands/{connect,list_calendars,list_events,create_event,update_event,delete_event}.rs
src/remote_config.rs               # caldav_account + caldav_calendar_url
src/session/{store,types}.rs       # username/password + discovered URLs on disk
src/caldav/client.rs               # libdav client construction + URL/date helpers
src/caldav/ops.rs                  # parse_event, find_event_location, FindEventByUid REPORT
src/caldav/ops/{discover,list_calendars,list_events,create_event,update_event,delete_event}.rs
```

Two layers: `commands/*` resolve credentials from the session store then delegate to `caldav/ops/*`, which are pure functions of `(username, password, calendar_url, …)`.

### Libraries (versions from `Cargo.lock`)

| crate | version | role |
|---|---|---|
| `libdav` | 0.10.5 (`Cargo.lock:1493`) | CalDAV/WebDAV client, `DavRequest` trait, `FindCalendars`, `GetEtag`, `Delete`, `GetCalendarResources` |
| `hyper` | 1.8.1 (`Cargo.lock:1091`) | HTTP/1 client |
| `hyper-rustls` | 0.27.7 (`Cargo.lock:1128`) | TLS, native roots |
| `tower-http` | 0.6.8 (`Cargo.lock:3149`) | `AddAuthorization::basic`, `FollowRedirect` |
| `roxmltree` | 0.21.1 (`Cargo.lock:2304`) | multistatus XML parsing |

Client construction: `caldir-provider-caldav/src/caldav/client.rs:28-56` (basic auth at `:47`, redirect layer at `:51`).

## Requests per operation

### list — `calendar-query` REPORT

`caldir-provider-caldav/src/caldav/ops/list_events.rs:93-118`. Custom `DavRequest` (`GetCalendarResourcesInRange`), method `REPORT`, `Depth: 1`, `Content-Type: application/xml`, URI = calendar collection href. Body (`list_events.rs:95-110`):

```xml
<C:calendar-query xmlns="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
    <prop>
        <getetag/>
        <C:calendar-data/>
    </prop>
    <C:filter>
        <C:comp-filter name="VCALENDAR">
            <C:comp-filter name="VEVENT">
                <C:time-range start="{}" end="{}"/>
            </C:comp-filter>
        </C:comp-filter>
    </C:filter>
</C:calendar-query>
```

`start`/`end` come from `format_caldav_datetime` (`client.rs:93`), RFC3339 → `YYYYMMDDTHHMMSSZ`. Response parsed at `list_events.rs:135-183`: every `<response>` yields `href`, `getetag`, `calendar-data`; only entries with calendar-data are kept.

### create — PUT

`caldir-provider-caldav/src/caldav/ops/create_event.rs:24-48`. Body is `event.to_ics_string()` (`:24`), URI `{calendar_url}/{uid}.ics` (`:26`, via `client.rs:73-76`), raw `PUT` with `Content-Type: text/calendar` and `If-None-Match: *` (`:37-38`). Uses `caldav.request_raw` deliberately to avoid libdav emitting a duplicate `Content-Type` (comment `:29-32`). Non-2xx → bail (`:46`). Then re-reads via `GetCalendarResources::new(&calendar_href).with_hrefs([&href])` (`:53`) and re-parses.

### update — PUT with `If-Match`

`caldir-provider-caldav/src/caldav/ops/update_event.rs:25-48`. Locates the resource via `find_event_location` (`:29`), then raw `PUT` to `location.href` with `Content-Type: text/calendar` and `If-Match: {etag}` (`:37-38`). Read-back identical to create (`:52`).

### delete — DELETE

`caldir-provider-caldav/src/caldav/ops/delete_event.rs:33-35`: `libdav::dav::Delete::new(&href).force()` (`.force()` = no `If-Match`). Location resolution failure is treated as already-deleted success (`:25-31`); 404/NOT_FOUND in the error string is also success (`:41`).

### Location lookup — second `calendar-query` REPORT

`caldir-provider-caldav/src/caldav/ops.rs:50-70`: fast path `GetEtag` on `{calendar_url}/{uid}.ics`; on failure, `FindEventByUid` REPORT (`ops.rs:99-124`), `Depth: 1`, body (`ops.rs:100-116`):

```xml
<C:calendar-query xmlns="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
    <prop>
        <getetag/>
    </prop>
    <C:filter>
        <C:comp-filter name="VCALENDAR">
            <C:comp-filter name="VEVENT">
                <C:prop-filter name="UID">
                    <C:text-match collation="i;octet">{}</C:text-match>
                </C:prop-filter>
            </C:comp-filter>
        </C:comp-filter>
    </C:filter>
</C:calendar-query>
```

### discovery / list_calendars

`ops/discover.rs:238-272`: `find_current_user_principal` + `FindCalendarHomeSet` (both libdav PROPFINDs). `ops/list_calendars.rs:32-99`: libdav `FindCalendars` on the home set, then per-calendar `GetProperty` for `DISPLAY_NAME` (`:57`) and `CALENDAR_COLOUR` (`:73`), plus a custom `PROPFIND` for `DAV:current-user-privilege-set` (`:130-149`) mapped to `read_only` via `is_writable_privilege_set` (`:105-109`).

## Does the REPORT filter hard-code VEVENT?

Yes, twice, and nowhere else:

- `caldir-provider-caldav/src/caldav/ops/list_events.rs:103` — `<C:comp-filter name="VEVENT">` in the time-range query.
- `caldir-provider-caldav/src/caldav/ops.rs:107` — `<C:comp-filter name="VEVENT">` in the UID lookup query.

A repo-wide grep for `VEVENT`/`VTODO` inside the crate returns only those two lines plus a doc comment (`ops.rs:30`) and a test fixture (`ops.rs:172`). There is no `VTODO` string anywhere in the crate.

## `supported-calendar-component-set`

**Not read anywhere.** Grep for `supported-calendar-component-set`, `SUPPORTED_CALENDAR`, and `component_set` across `caldir-provider-caldav/**` returns zero hits. Discovery (`ops/discover.rs`) and calendar enumeration (`ops/list_calendars.rs`) request only `displayname`, `calendar-color`, and `current-user-privilege-set`; `RawCalendar` (`list_calendars.rs:13-26`) has no component-set field, and `CalendarConfig` is built from name/color/read_only only (`commands/list_calendars.rs:32-42`). Whether libdav's `FindCalendars` internally filters collections by component set is a libdav-internal detail, out of this crate's scope.

## Semantic layer vs transport

The provider is **semantic on the wire boundary, transport in the middle**:

- Inbound ICS is parsed into a core `Event`: `ops.rs:32-37` `parse_event` calls `caldir_core::Event::from_ics_str` and takes the first `Ok` entry, silently dropping anything that fails to parse. Used by `fetch_events` (`list_events.rs:39`), `create_event` (`:60`), `update_event` (`:59`).
- Outbound is `Event::to_ics_string()` (`create_event.rs:24`, `update_event.rs:25`). The provider never touches ICS text itself — no line manipulation, no property rewriting, no timezone conversion in this crate.
- The RPC surface is `Event`-typed end to end (`src/main.rs:9-41`); there is no raw-ICS pass-through path.

So the provider carries no calendar semantics of its own beyond the two hard-coded `comp-filter` names; all component understanding lives in `caldir-core`'s `Event` type (`caldir-core/src/event.rs:96`, `:123`).

## ETag / href handling

- ETags are read in three places: the REPORT parses `getetag` into `CalendarResource._etag` but **discards it** — the field is underscore-prefixed and unused (`list_events.rs:78`); `find_event_location` keeps the etag from `GetEtag` or the UID REPORT (`ops.rs:40-43`, `:58-63`, `:152-157`); `update_event` sends it as `If-Match` (`update_event.rs:38`). Create uses `If-None-Match: *` instead (`create_event.rs:38`). Delete sends neither (`.force()`, `delete_event.rs:34`).
- The href is **derived from UID by default**: `event_url` = `{calendar_url}/{uid}.ics` (`client.rs:73-76`), used by create unconditionally (`create_event.rs:26`) and as the fast path for update/delete (`ops.rs:57`). The UID REPORT fallback exists precisely for servers that assign their own filenames (`ops.rs:47-49`), so update/delete tolerate non-UID hrefs while create does not.
- `url_to_href` (`client.rs:81-87`) reduces absolute URLs to their path for libdav.

## What must change for VTODO (observations only)

1. The time-range REPORT hard-codes `<C:comp-filter name="VEVENT">` at `src/caldav/ops/list_events.rs:103`; a VTODO on the server is never returned by `fetch_events`.
2. The UID-lookup REPORT hard-codes the same filter at `src/caldav/ops.rs:107`, so `find_event_location`'s fallback cannot locate a VTODO resource whose href is not `{uid}.ics` — affecting both update and delete.
3. `<C:time-range>` is applied unconditionally inside the comp-filter (`list_events.rs:104`); RFC 4791 time-range semantics for VTODO differ from VEVENT (DUE/DTSTART/DURATION/COMPLETED), and VTODOs with no date properties do not match a time-range at all.
4. Inbound parsing goes through `caldir_core::Event::from_ics_str` (`ops.rs:33`), and a component the core parser does not yield is silently dropped by `find_map(Result::ok)` (`ops.rs:34-36`) — no error surfaces.
5. Outbound serialization is `Event::to_ics_string()` (`create_event.rs:24`, `update_event.rs:25`); the provider has no way to emit a component core does not model.
6. The RPC handler signatures are `Event`-typed (`src/main.rs:26-40`) and command modules pass `cmd.event` through unchanged (`commands/create_event.rs:98`, `commands/update_event.rs:118`, `commands/delete_event.rs:141`).
7. `supported-calendar-component-set` is never requested during discovery (`ops/discover.rs`, `ops/list_calendars.rs`), and `RawCalendar` (`list_calendars.rs:13-26`) has no field for it — there is currently no way to know whether a discovered collection accepts VTODO.
8. `create_event` derives the resource href from the UID alone (`create_event.rs:26` → `client.rs:73-76`), so an event and a todo sharing a UID would collide on the same `.ics` path.
9. The doc comment at `ops.rs:30` documents the single-VEVENT-per-resource assumption; `parse_event` returns only the first parsed component (`ops.rs:32-37`).
10. Tests encode VEVENT-only fixtures (`ops.rs:172`) — no VTODO coverage exists in the crate.

## Verdict

**TRANSPORT-ONLY potential — currently UNSUPPORTED.** The HTTP layer is genuinely component-agnostic: create/update are plain `PUT`s of `text/calendar` bytes (`create_event.rs:34-39`, `update_event.rs:34-39`), delete is a plain `DELETE` on an href (`delete_event.rs:34`), and none of them inspect the payload. If the ICS text handed to them contained a VTODO, the server would receive it unchanged. What blocks VTODO today is entirely at two named places: the two `<C:comp-filter name="VEVENT">` literals (`ops/list_events.rs:103`, `ops.rs:107`), which make the component invisible to discovery-by-query, and the `Event`-typed RPC/serialization boundary (`ops.rs:32-37`, `main.rs:26-40`), where a non-VEVENT component would be silently dropped by `find_map(Result::ok)` rather than transported. The provider itself holds no other calendar semantics, so its VTODO story is decided almost entirely in `caldir-core`, plus those two filter strings.
