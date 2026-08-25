# Empirical probe: Radicale 3.7.6

Settles six questions that ADR-02, -04, -05, -06, -07 and -08 currently mark UNVERIFIED.
Every response below is real captured output from a live server, not reasoning from the RFC.

- **Server:** Radicale **3.7.6** (`tomsquest/docker-radicale:latest`,
  digest `sha256:e2ef8624b2156ada47489223df12575406d625c96725f03cfc4dc890e3eda400`,
  image label `org.label-schema.version=3.7.6`, CPython 3.14.7 / WSGIServer 0.2)
- **Probe date:** 2026-08-25 (matters: every `time-range` below is "today ±365d" = `20250825T000000Z` .. `20270825T000000Z`)
- **Transport:** plain `curl`, HTTP, no TLS.

## Summary

| Q | Verdict | ADR | Changes that ADR's decision? |
|---|---|---|---|
| **A** — advertises `supported-calendar-component-set`? | **YES**, on every calendar collection, Depth 0 and Depth 1. Default = `VTODO,VEVENT,VJOURNAL`; VTODO-only calendar = `VTODO`. Absent (404) on the principal itself. **Not enforced on PUT.** | ADR-07 | No — strengthens it. The signal exists and is usable for discovery, but it is advisory only, so it cannot be the enforcement point. |
| **B** — implements RFC 4791 §9.9 VTODO table? | **YES, faithfully, all six rows.** An undated task with an old `COMPLETED` **IS omitted** by a time-range query. | ADR-08 | **No — it CONFIRMS the decision.** The data-loss finding is now reproduced against a real server. Decisions 1+2 must ship together. |
| **C** — byte-for-byte echo of a PUT? | **NO.** Radicale re-serializes: properties sorted alphabetically within each component, lines re-folded at 75 octets, `VTIMEZONE` injected for any `TZID` used. Stable fixed point after one pass. | ADR-06 | Yes (mechanically). A byte-diff acceptance gate cannot hold. Use structural `==`, or diff pass-2 against pass-1. **ETag is computed over the normalized form**, so it is stable across property reordering — that is the usable change signal. |
| **D** — injects a non-`X-` property caldir would destroy? | **NO.** Zero properties added inside the `VTODO`. No `SEQUENCE`, no `CREATED`, no `LAST-MODIFIED`, no rewritten `COMPLETED`/`DUE`. Only a sibling `VTIMEZONE` component at `VCALENDAR` level. | ADR-05 | No. The X-only preservation rule survives contact with Radicale. One new caveat: caldir must emit `VTIMEZONE` or accept that Radicale adds it (component, not property — outside ADR-05's scope). |
| **E** — VEVENT + VTODO sharing one UID in one collection? | **NO — Radicale hard-rejects it.** Different path, same UID → `409` + `<C:no-uid-conflict/>`. Same path → silent overwrite (`204`). Both components in one resource → `400 Bad Request`. Same UID in a *different* collection → fine (`201`). | ADR-02 | Yes, materially. The hazard ADR-02 guards against **cannot arise inside one Radicale collection at all**; it is a purely *local-directory* and *cross-collection* hazard. |
| **F** — rejects `STATUS:COMPLETED` without `PERCENT-COMPLETE:100`? | **NO.** `201`, stored verbatim. Even the contradictory `STATUS:COMPLETED` + `PERCENT-COMPLETE:0` is accepted verbatim. Both `ATTACH` lines and both `RELATED-TO` lines survive server-side with parameters intact. | ADR-04 | No. The server imposes no coupling, so ADR-04's "inconsistency, not loss" framing for `PERCENT-COMPLETE` holds. The server-side multiplicity survival means any `ATTACH`/`RELATED-TO` collapse is caldir's parser, never Radicale. |

---

## Reproduction

All scratch files live outside the repo. Working directory for the commands below is any empty dir.

### 1. Config

```sh
mkdir -p config data

# bcrypt hash for testuser:testpass, generated with the image's own venv python
docker run --rm --entrypoint /venv/bin/python3 tomsquest/docker-radicale:latest \
  -c "import bcrypt;print('testuser:'+bcrypt.hashpw(b'testpass',bcrypt.gensalt()).decode())" \
  > config/users

cat > config/config <<'EOF'
[server]
hosts = 0.0.0.0:5232

[auth]
type = htpasswd
htpasswd_filename = /config/users
htpasswd_encryption = bcrypt

[storage]
filesystem_folder = /data/collections

[rights]
type = owner_only
EOF
```

### 2. Run

```sh
docker run -d --name caldir-radicale-probe -p 15232:5232 \
  -v "$PWD/config:/config:ro" -v "$PWD/data:/data" \
  tomsquest/docker-radicale:latest
```

Base URL `http://127.0.0.1:15232`, auth `-u testuser:testpass`.

> Gotcha, cost me two 401s: **zsh does not word-split unquoted parameter expansion.**
> `A="-u testuser:testpass"; curl $A ...` sends the username as `" testuser"` and fails auth.
> Pass `-u testuser:testpass` literally, or use a shell array.

### 3. Collections

```sh
cat > mk-default.xml <<'EOF'
<?xml version="1.0" encoding="utf-8" ?>
<C:mkcalendar xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <D:set><D:prop><D:displayname>Default</D:displayname></D:prop></D:set>
</C:mkcalendar>
EOF

cat > mk-tasks.xml <<'EOF'
<?xml version="1.0" encoding="utf-8" ?>
<C:mkcalendar xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <D:set><D:prop>
    <D:displayname>TasksOnly</D:displayname>
    <C:supported-calendar-component-set><C:comp name="VTODO"/></C:supported-calendar-component-set>
  </D:prop></D:set>
</C:mkcalendar>
EOF

curl -u testuser:testpass -X MKCALENDAR -H 'Content-Type: application/xml' \
  --data-binary @mk-default.xml http://127.0.0.1:15232/testuser/default/     # 201
curl -u testuser:testpass -X MKCALENDAR -H 'Content-Type: application/xml' \
  --data-binary @mk-tasks.xml   http://127.0.0.1:15232/testuser/tasksonly/   # 201
```

### 4. Teardown

```sh
docker rm -f caldir-radicale-probe
```

---

## Q-A (ADR-07) — `supported-calendar-component-set`

### Request

```sh
cat > pf-comp.xml <<'EOF'
<?xml version="1.0" encoding="utf-8" ?>
<D:propfind xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <D:prop>
    <D:resourcetype/>
    <D:displayname/>
    <C:supported-calendar-component-set/>
  </D:prop>
</D:propfind>
EOF

for u in default tasksonly; do for d in 0 1; do
  curl -u testuser:testpass -X PROPFIND -H "Depth: $d" -H 'Content-Type: application/xml' \
    --data-binary @pf-comp.xml "http://127.0.0.1:15232/testuser/$u/"
done; done
```

### Response — default collection (identical at Depth 0 and Depth 1)

```xml
<?xml version='1.0' encoding='utf-8'?>
<multistatus xmlns="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav"><response><href>/testuser/default/</href><propstat><prop><resourcetype><C:calendar /><collection /></resourcetype><displayname>Default</displayname><C:supported-calendar-component-set><C:comp name="VTODO" /><C:comp name="VEVENT" /><C:comp name="VJOURNAL" /></C:supported-calendar-component-set></prop><status>HTTP/1.1 200 OK</status></propstat></response></multistatus>
```

`HTTP 207`. Note the order: **`VTODO` first**, then `VEVENT`, then `VJOURNAL`.

### Response — VTODO-only collection (identical at Depth 0 and Depth 1)

```xml
<?xml version='1.0' encoding='utf-8'?>
<multistatus xmlns="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav"><response><href>/testuser/tasksonly/</href><propstat><prop><resourcetype><C:calendar /><collection /></resourcetype><displayname>TasksOnly</displayname><C:supported-calendar-component-set><C:comp name="VTODO" /></C:supported-calendar-component-set></prop><status>HTTP/1.1 200 OK</status></propstat></response></multistatus>
```

The value set at MKCALENDAR time is stored and echoed back exactly.

### Response — principal / home-set, Depth 1 on `/testuser/`

```xml
<?xml version='1.0' encoding='utf-8'?>
<multistatus xmlns="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav"><response><href>/testuser/</href><propstat><prop><resourcetype><principal /><collection /></resourcetype></prop><status>HTTP/1.1 200 OK</status></propstat><propstat><prop><displayname /><C:supported-calendar-component-set /></prop><status>HTTP/1.1 404 Not Found</status></propstat></response><response><href>/testuser/default/</href><propstat><prop><resourcetype><C:calendar /><collection /></resourcetype><displayname>Default</displayname><C:supported-calendar-component-set><C:comp name="VTODO" /><C:comp name="VEVENT" /><C:comp name="VJOURNAL" /></C:supported-calendar-component-set></prop><status>HTTP/1.1 200 OK</status></propstat></response><response><href>/testuser/tasksonly/</href><propstat><prop><resourcetype><C:calendar /><collection /></resourcetype><displayname>TasksOnly</displayname><C:supported-calendar-component-set><C:comp name="VTODO" /></C:supported-calendar-component-set></prop><status>HTTP/1.1 200 OK</status></propstat></response></multistatus>
```

**One Depth-1 PROPFIND on the home-set returns the component set for every calendar.** No per-calendar
round trip is needed, which matters for ADR-07 force 8 (`list_calendars` cost).

The principal resource itself returns `404 Not Found` in its own `propstat` for
`supported-calendar-component-set` — as expected; it is a `<principal/><collection/>`, not a
`<C:calendar/>`. Radicale also does **not** expose `calendar-home-set`:

```xml
<response><href>/</href><propstat><prop><current-user-principal><href>/testuser/</href></current-user-principal></prop><status>HTTP/1.1 200 OK</status></propstat><propstat><prop><C:calendar-home-set /></prop><status>HTTP/1.1 404 Not Found</status></propstat></response>
```

Discovery path is `current-user-principal` → PROPFIND Depth 1 on it.

### Enforcement: NOT enforced on PUT

```sh
# fresh-UID VEVENT into the VTODO-only collection
curl -u testuser:testpass -X PUT -H 'Content-Type: text/calendar' \
  --data-binary @ev2.ics http://127.0.0.1:15232/testuser/tasksonly/ev2.ics
```

```
HTTP 201
```

**Verdict:** Radicale advertises the property reliably and per-collection, at both depths, and it is
readable in bulk from the home-set. It is **advisory only** — a VEVENT PUT into a `VTODO`-only
collection is accepted with `201`.

---

## Q-B (ADR-08) — the RFC 4791 §9.9 VTODO time-range table

**This is the question that matters.**

### Fixtures (all PUT into `/testuser/tasksonly/`, all returned `201`)

| file | dates carried |
|---|---|
| `t1.ics` | none at all (no DTSTART, DUE, COMPLETED, CREATED) |
| `t2.ics` | `STATUS:COMPLETED` + `COMPLETED:20200101T000000Z` only |
| `t3.ics` | `DUE:20260901T120000Z` (inside window) |
| `t4.ics` | `DUE:20300601T120000Z` (far outside window) |
| `t5.ics` | `CREATED:20200101T000000Z` only |
| `t6.ics` | `STATUS:COMPLETED` + `PERCENT-COMPLETE:100`, **no** `COMPLETED` timestamp |
| `t7.ics` | `COMPLETED:20260801T000000Z` (inside window) |
| `t8.ics` | `DTSTART:20260901T090000Z`, no DUE |

`DTSTAMP:20260825T000000Z` on all of them. Example:

```
BEGIN:VCALENDAR
VERSION:2.0
PRODID:-//caldir probe//EN
BEGIN:VTODO
UID:task2-completed-old
DTSTAMP:20260825T000000Z
SUMMARY:T2 undated completed 2020
STATUS:COMPLETED
COMPLETED:20200101T000000Z
END:VTODO
END:VCALENDAR
```

### The two REPORTs

```sh
cat > q-notr.xml <<'EOF'
<?xml version="1.0" encoding="utf-8" ?>
<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <D:prop><D:getetag/></D:prop>
  <C:filter><C:comp-filter name="VCALENDAR"><C:comp-filter name="VTODO"/></C:comp-filter></C:filter>
</C:calendar-query>
EOF

cat > q-tr.xml <<'EOF'
<?xml version="1.0" encoding="utf-8" ?>
<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <D:prop><D:getetag/></D:prop>
  <C:filter>
    <C:comp-filter name="VCALENDAR">
      <C:comp-filter name="VTODO">
        <C:time-range start="20250825T000000Z" end="20270825T000000Z"/>
      </C:comp-filter>
    </C:comp-filter>
  </C:filter>
</C:calendar-query>
EOF

curl -u testuser:testpass -X REPORT -H "Depth: 1" -H 'Content-Type: application/xml' \
  --data-binary @q-notr.xml http://127.0.0.1:15232/testuser/tasksonly/
curl -u testuser:testpass -X REPORT -H "Depth: 1" -H 'Content-Type: application/xml' \
  --data-binary @q-tr.xml   http://127.0.0.1:15232/testuser/tasksonly/
```

### Real results

**(b) WITHOUT `time-range` — `HTTP 207`, 8 hrefs:**

```
t1.ics  t2.ics  t3.ics  t4.ics  t5.ics  t6.ics  t7.ics  t8.ics
```

**(a) WITH `time-range` 2025-08-25 .. 2027-08-25 — `HTTP 207`, 6 hrefs:**

```
t1.ics  t3.ics  t5.ics  t6.ics  t7.ics  t8.ics
```

Raw XML for the first four-fixture run, verbatim:

```xml
<?xml version='1.0' encoding='utf-8'?>
<multistatus xmlns="DAV:"><response><href>/testuser/tasksonly/t1.ics</href><propstat><prop><getetag>"1de1f299e3e901f71b7a61f53654ef1da17632fe7ee2319b54db97e60e5ed613"</getetag></prop><status>HTTP/1.1 200 OK</status></propstat></response><response><href>/testuser/tasksonly/t3.ics</href><propstat><prop><getetag>"5dac4383a5bb2b7a8285d9f5344cb8edac5f27f75394df79b5ca5061ac946c60"</getetag></prop><status>HTTP/1.1 200 OK</status></propstat></response></multistatus>
```

(Only `t1` and `t3` — `t2` and `t4` absent.)

### Row-by-row against RFC 4791 §9.9

| fixture | RFC row | RFC condition | expected | **Radicale actual** | match? |
|---|---|---|---|---|---|
| t1 | none of the above | TRUE | in | **IN** | ✓ |
| t2 | COMPLETED only | `start <= COMPLETED AND end >= COMPLETED` → `2025 <= 2020` false | **out** | **OUT** | ✓ |
| t3 | DUE only | `start < DUE AND end >= DUE` | in | **IN** | ✓ |
| t4 | DUE only | `end >= DUE` → `2027 >= 2030` false | out | **OUT** | ✓ |
| t5 | CREATED only | `end > CREATED` → `2027 > 2020` true | in | **IN** | ✓ |
| t6 | none of the above (`STATUS`/`PERCENT-COMPLETE` are not date props) | TRUE | in | **IN** | ✓ |
| t7 | COMPLETED only | `start <= COMPLETED AND end >= COMPLETED` | in | **IN** | ✓ |
| t8 | DTSTART only | `start <= DTSTART AND end > DTSTART` | in | **IN** | ✓ |

All eight agree. `DTSTAMP` correctly does **not** participate.

### VERDICT — stated unambiguously

**Radicale 3.7.6 DOES implement the RFC 4791 §9.9 VTODO time-range table, correctly and completely.**

**Task 1 (fully undated) is RETURNED by the time-range query. Task 2 (undated, `COMPLETED:20200101T000000Z`) is OMITTED by the time-range query and returned only when the time-range is removed.**

**The ADR-08 data-loss finding is CONFIRMED against a real server, not merely against the RFC text.**
A naive `VEVENT`→`VTODO` swap that keeps `<C:time-range>` in
`caldir-provider-caldav/src/caldav/ops/list_events.rs:103-104` will, against this exact stock
Radicale, fail to return a previously-synced completed task; `calendar_diff.rs:78-86` will then see
an id present in `sync_bases` with no remote counterpart and, because the task is undated and
therefore in-window, emit an `incoming Delete` that removes the user's file. No server bug is
involved. **ADR-08 Decisions 1 and 2 are one decision and must ship in the same commit.**

The residual scope is narrower than "all completed tasks": the hazard fires only for tasks whose
*only* date property is a `COMPLETED` older than `start`. A task that also carries `CREATED`
(t5) is rescued by the `end > CREATED` row, and many producers do emit `CREATED`. That narrows
the blast radius; it does not remove it, because `CREATED` is optional and caldir does not model
it (ADR-05 decision item 4 explicitly drops it, so any task caldir has *written* will have lost
its `CREATED` — caldir's own writes manufacture the vulnerable shape).

---

## Q-C (ADR-06) — does Radicale echo a PUT byte-for-byte?

### Request

A rich VTODO with deliberately non-alphabetical property order, a long unfolded `DESCRIPTION`,
a `TZID`, duplicated `ATTACH`/`RELATED-TO`, an `X-` property and a `VALARM`:

```
BEGIN:VCALENDAR
VERSION:2.0
PRODID:-//caldir probe//EN
CALSCALE:GREGORIAN
BEGIN:VTODO
UID:rich-todo-001
DTSTAMP:20260825T101500Z
CREATED:20260820T080000Z
LAST-MODIFIED:20260824T090000Z
SUMMARY:Rich task with many properties
DESCRIPTION:Line one\nLine two with a very long tail that should exceed seventy-five oct
 ets so that folding behaviour can be observed on the round trip.
DTSTART;TZID=Europe/Paris:20260901T090000
DUE;TZID=Europe/Paris:20260905T170000
STATUS:NEEDS-ACTION
PERCENT-COMPLETE:42
PRIORITY:3
CLASS:PRIVATE
CATEGORIES:work,urgent
LOCATION:Somewhere
ORGANIZER;CN=Test User:mailto:testuser@example.com
ATTENDEE;CN=Bob;PARTSTAT=NEEDS-ACTION:mailto:bob@example.com
RELATED-TO;RELTYPE=PARENT:parent-uid-1
RELATED-TO;RELTYPE=CHILD:child-uid-1
ATTACH:https://example.com/a.pdf
ATTACH:https://example.com/b.pdf
GEO:48.8566;2.3522
URL:https://example.com/task
RRULE:FREQ=WEEKLY;COUNT=3
X-CALDIR-CUSTOM:hello
BEGIN:VALARM
ACTION:DISPLAY
TRIGGER:-PT15M
DESCRIPTION:Reminder
END:VALARM
END:VTODO
END:VCALENDAR
```

```sh
curl -u testuser:testpass -X PUT -H 'Content-Type: text/calendar; charset=utf-8' \
  --data-binary @rich.ics http://127.0.0.1:15232/testuser/default/rich.ics   # 201
curl -u testuser:testpass -o rich-get.ics -D rich-get.headers \
  http://127.0.0.1:15232/testuser/default/rich.ics
diff rich.ics rich-get.ics
```

### Response headers

```
HTTP/1.0 200 OK
Server: WSGIServer/0.2 CPython/3.14.7
Content-Type: text/calendar; charset=utf-8
Last-Modified: Tue, 25 Aug 2026 05:54:43 GMT
ETag: "5a0c6c784fa86b982fe62d85931a0351bbb403d00f739eec388a5c51aa9f4d5e"
Content-Length: 1398
```

PUT was 1052 bytes; GET is 1398 bytes. `md5sum`: `b29d8b8f…` in, `4dd0e548…` out.

### Exact bytes returned

```
BEGIN:VCALENDAR
VERSION:2.0
CALSCALE:GREGORIAN
PRODID:-//caldir probe//EN
BEGIN:VTIMEZONE
TZID:Europe/Paris
BEGIN:STANDARD
DTSTART:20001029T030000
RRULE:FREQ=YEARLY;BYDAY=-1SU;BYMONTH=10
TZNAME:CET
TZOFFSETFROM:+0200
TZOFFSETTO:+0100
END:STANDARD
BEGIN:DAYLIGHT
DTSTART:20000326T020000
RRULE:FREQ=YEARLY;BYDAY=-1SU;BYMONTH=3
TZNAME:CEST
TZOFFSETFROM:+0100
TZOFFSETTO:+0200
END:DAYLIGHT
END:VTIMEZONE
BEGIN:VTODO
ATTACH:https://example.com/a.pdf
ATTACH:https://example.com/b.pdf
ATTENDEE;CN=Bob;PARTSTAT=NEEDS-ACTION:mailto:bob@example.com
CATEGORIES:work,urgent
CLASS:PRIVATE
CREATED:20260820T080000Z
DESCRIPTION:Line one\nLine two with a very long tail that should exceed sev
 enty-five octets so that folding behaviour can be observed on the round tr
 ip.
DTSTAMP:20260825T101500Z
DTSTART;TZID=Europe/Paris:20260901T090000
DUE;TZID=Europe/Paris:20260905T170000
GEO:48.8566;2.3522
LAST-MODIFIED:20260824T090000Z
LOCATION:Somewhere
ORGANIZER;CN=Test User:mailto:testuser@example.com
PERCENT-COMPLETE:42
PRIORITY:3
RELATED-TO;RELTYPE=PARENT:parent-uid-1
RELATED-TO;RELTYPE=CHILD:child-uid-1
RRULE:FREQ=WEEKLY;COUNT=3
STATUS:NEEDS-ACTION
SUMMARY:Rich task with many properties
UID:rich-todo-001
URL:https://example.com/task
BEGIN:VALARM
ACTION:DISPLAY
DESCRIPTION:Reminder
TRIGGER:-PT15M
END:VALARM
X-CALDIR-CUSTOM:hello
END:VTODO
END:VCALENDAR
```

(CRLF line endings throughout, confirmed with `cat -A`.)

### What changed

| transformation | detail |
|---|---|
| **Property reordering** | Alphabetical by name **within each component**, `VTODO` and `VALARM` alike. `X-CALDIR-CUSTOM` sorts after `URL`, i.e. after the named props but *before* the closing `END`; the nested `VALARM` component floats between them. At `VCALENDAR` level `VERSION` stays first, then `CALSCALE`, then `PRODID`. |
| **Line re-folding** | Radicale unfolds and re-folds at 75 octets. The input's hand-folded `DESCRIPTION` (89-octet first line) came back folded into three lines at different offsets. |
| **`VTIMEZONE` injection** | A full `VTIMEZONE` for `Europe/Paris` was synthesized and inserted because `DTSTART`/`DUE` used `TZID=Europe/Paris`. Sole source of the 346-byte growth. |
| **Escaping** | `DESCRIPTION` `\n` preserved literally. `GEO:48.8566;2.3522` unchanged. Parameters (`TZID`, `CN`, `PARTSTAT`, `RELTYPE`, `FMTTYPE`) preserved verbatim. |
| **Values** | **No timestamp was reformatted.** `COMPLETED`, `DUE`, `DTSTART`, `CREATED`, `LAST-MODIFIED`, `DTSTAMP` all byte-identical. |
| **Nothing dropped** | `comm` over the property-name sets shows an empty PUT-minus-GET difference. |

Even a **minimal** UTC-only VTODO is reordered:

```
$ diff min.ics min-get.ics
5d4
< UID:min-1
7d5
< SUMMARY:Minimal
8a7,8
> SUMMARY:Minimal
> UID:min-1
```

### Fixed point

```sh
curl -u testuser:testpass -X PUT --data-binary @rich-get.ics .../rich.ics   # 204
curl -u testuser:testpass -o rich-get2.ics .../rich.ics
cmp rich-get.ics rich-get2.ics
```

```
FIXED POINT: 2nd round trip byte-identical
```

### ETag is over the normalized form — the usable signal

```
PUT original-order bytes   -> ETag "2b546834cdac228a3f32ca9d0562f8059910f3e56df45a1ab95ee1ed9b577535"
PUT normalized-order bytes -> ETag "2b546834cdac228a3f32ca9d0562f8059910f3e56df45a1ab95ee1ed9b577535"
```

Identical. Property order does **not** perturb the ETag. But `DTSTAMP` does:

```
PUT with DTSTAMP:20260825T101500Z -> ETag "2b546834cdac228a…"
PUT with DTSTAMP:20260825T235959Z -> ETag "73d25676e9022c3a…"
```

On-disk storage matches the GET body exactly
(`/data/collections/collection-root/testuser/default/min.ics`) — Radicale normalizes at write time,
not at read time.

**Verdict:** **NO byte-for-byte echo.** A "sync twice, second diff empty" gate defined as a
*byte* diff between what caldir PUT and what Radicale GETs back **cannot hold**. Three
consequences for ADR-06:

1. The gate must compare **structurally** (parsed domain `==`), or compare **GET-pass-2 against
   GET-pass-1** — which does hold, since Radicale's normalization is idempotent after one pass.
2. The ETag is a sound change signal for everything except `DTSTAMP`. Because caldir deliberately
   re-stamps `DTSTAMP` on every serialize (ADR-06's own "not-a-field beats ignored-field"), **every
   caldir push mutates the ETag even when nothing changed.** ETag can prove "remote changed"; it
   can never prove "caldir's push was a no-op".
3. Radicale adds `VTIMEZONE` whenever caldir emits a `TZID`. If caldir's serializer does not emit
   `VTIMEZONE`, the remote body will structurally differ from the pushed body on the very first
   sync — harmless for property-level equality, fatal for any whole-file comparison.

---

## Q-D (ADR-05) — injected non-`X-` properties

Derived from the Q-C diff, by set difference over property names:

```
props present in GET but not in PUT: TZID TZNAME TZOFFSETFROM TZOFFSETTO
props present in PUT but not in GET: (none)
```

All four are **inside the injected `VTIMEZONE` component**, not on the `VTODO`.

Specifically **absent** from the returned `VTODO`, having not been sent:

- no `SEQUENCE`
- no `CREATED` (the one present was ours)
- no `LAST-MODIFIED` rewrite (ours came back byte-identical, and see F1 below: a VTODO with no
  `LAST-MODIFIED` at all gets none added)
- no `COMPLETED` / `DUE` / `DTSTART` reformatting
- no `PRODID`/`X-` stamping of its own

**Verdict:** **Radicale injects ZERO non-`X-` properties onto a VTODO.** ADR-05's X-only
preservation rule loses nothing to this server. The single new fact is the sibling `VTIMEZONE`
*component* — outside ADR-05's property scope, but worth naming in the ADR because a naive
"re-serialize the whole VCALENDAR from the domain type" push will drop it and Radicale will
silently re-add it, producing a body that is never byte-stable across the wire.

---

## Q-E (ADR-02) — VEVENT and VTODO sharing one UID

`UID:shared-uid-42`, collection `/testuser/default/`.

```sh
# 1. VEVENT to {uid}.ics
curl -u testuser:testpass -X PUT --data-binary @ev.ics .../default/shared-uid-42.ics
# 2. VTODO, same UID, SAME path
curl -u testuser:testpass -X PUT --data-binary @td.ics .../default/shared-uid-42.ics
# 3. VTODO, same UID, DIFFERENT path
curl -u testuser:testpass -X PUT --data-binary @td.ics .../default/shared-uid-42-todo.ics
```

### Results

**1. PUT VEVENT → `HTTP 201`.**

**2. PUT VTODO to the same path → `HTTP 204`, silent overwrite.** GET now returns only the task:

```
BEGIN:VCALENDAR
VERSION:2.0
PRODID:-//caldir probe//EN
BEGIN:VTODO
DTSTAMP:20260825T101500Z
DUE:20260902T120000Z
SUMMARY:The task
UID:shared-uid-42
END:VTODO
END:VCALENDAR
```

The VEVENT is gone. No warning, no 409, no precondition.

**3. PUT VTODO with the same UID to a different path → `HTTP 409`:**

```xml
<?xml version='1.0' encoding='utf-8'?>
<error xmlns="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav"><C:no-uid-conflict /></error>
```

**Radicale enforces UID uniqueness per collection, component-type-agnostically.**

**4. Both components in one resource → `HTTP 400 Bad Request`** (tried with the same UID and with
two different UIDs; both rejected). Server log:

```
[WARNING] Bad PUT request on '/testuser/default/both.ics' (prepare): Multiple component types in object: 'VEVENT', 'VTODO'
[WARNING] Bad PUT request on '/testuser/default/multi.ics' (prepare): Multiple component types in object: 'VEVENT', 'VTODO'
```

**5. Same UID in a *different* collection → `HTTP 201`.** No cross-collection constraint.

**6. REPORT after the dust settles** — `shared-uid-42.ics` holds a VTODO, so a `VTODO` comp-filter
returns it and a bare `VCALENDAR` filter returns it; there is never a state in which one UID yields
two items in one collection.

**Verdict:** **A VEVENT and a VTODO sharing a UID CANNOT coexist in one Radicale collection.**
Enforced two ways: `409 <C:no-uid-conflict/>` across paths, and `400` within one resource.

This **materially narrows ADR-02**. The u_crawler shape it cites
(`deadlines/assignment-123.ics` VTODO + `windows/assignment-123.ics` VEVENT) is exactly the
*cross-collection* case Radicale permits — so ADR-02's premise "a Radicale collection may
legitimately hold both components [with the same UID]" is **false for Radicale**. The hazards
ADR-02 traces (state-file collision, diff mis-keying) are still REAL, but their reachable trigger is:

- a **local** directory holding both files (`calendar.rs:113` matches `*.ics` by extension — nothing
  stops a user), and
- **two caldir calendars** mapped to two Radicale collections whose `known_event_ids` are per-calendar
  anyway.

The "one remote collection hands core a VEVENT and a VTODO with one UID in a single fetch" path
does not exist on Radicale. ADR-02 should say so and re-derive its severity from the local case
alone. Two corollaries worth landing:

- Caldir must handle `409 <C:no-uid-conflict/>` on push. A user who renames a local `.ics` file
  (changing the slug, not the UID) makes caldir attempt exactly the rejected pattern.
- Caldir must never PUT a multi-component `VCALENDAR`. ADR-03's Option A (one supported component
  per file, loud error otherwise) is independently required by the server: Radicale answers `400`.

---

## Q-F (ADR-04) — STATUS coupling and multi-valued property survival

### F1 — `STATUS:COMPLETED`, no `PERCENT-COMPLETE`, no `COMPLETED`

```
BEGIN:VCALENDAR
VERSION:2.0
PRODID:-//caldir probe//EN
BEGIN:VTODO
UID:f1-status-completed
DTSTAMP:20260825T101500Z
SUMMARY:F1
STATUS:COMPLETED
END:VTODO
END:VCALENDAR
```

**PUT → `HTTP 201`.** GET:

```
BEGIN:VCALENDAR
VERSION:2.0
PRODID:-//caldir probe//EN
BEGIN:VTODO
DTSTAMP:20260825T101500Z
STATUS:COMPLETED
SUMMARY:F1
UID:f1-status-completed
END:VTODO
END:VCALENDAR
```

Accepted, stored verbatim, **no `PERCENT-COMPLETE` added, no `COMPLETED` added**.

### F1b — contradictory `STATUS:COMPLETED` + `PERCENT-COMPLETE:0`

**PUT → `HTTP 201`.** GET:

```
BEGIN:VTODO
DTSTAMP:20260825T101500Z
PERCENT-COMPLETE:0
STATUS:COMPLETED
SUMMARY:F1b
UID:f1b
END:VTODO
```

Stored verbatim. Radicale enforces no semantic coupling whatsoever.

### F2 — two `ATTACH`, two `RELATED-TO`

PUT:

```
ATTACH:https://example.com/one.pdf
ATTACH;FMTTYPE=text/plain:https://example.com/two.txt
RELATED-TO;RELTYPE=PARENT:parent-1
RELATED-TO;RELTYPE=SIBLING:sibling-1
```

**PUT → `HTTP 201`.** GET:

```
BEGIN:VTODO
ATTACH:https://example.com/one.pdf
ATTACH;FMTTYPE=text/plain:https://example.com/two.txt
DTSTAMP:20260825T101500Z
RELATED-TO;RELTYPE=PARENT:parent-1
RELATED-TO;RELTYPE=SIBLING:sibling-1
SUMMARY:F2
UID:f2-multi
END:VTODO
```

**Both `ATTACH` values and both `RELATED-TO` values survive, in input order, with their parameters
(`FMTTYPE`, `RELTYPE`) intact.** The Q-C rich fixture independently confirms this
(`RELTYPE=PARENT` + `RELTYPE=CHILD` both returned).

**Verdict:** **NO**, Radicale does not reject `STATUS:COMPLETED` without `PERCENT-COMPLETE:100`, and
it does not synthesize either property. Multi-valued `ATTACH`/`RELATED-TO` are fully preserved
server-side. Therefore:

- ADR-04's `PERCENT-COMPLETE:OUT` reasoning is safe from the server's side — the "inconsistency,
  not loss" framing holds; the server will happily store whatever caldir sends, including a
  contradiction, and will never repair it.
- Any observed `RELATED-TO` collapse-to-last (BRIEF:62, ADR-03) is **entirely icalendar-0.17.10 /
  caldir's parser**, with zero server contribution. That isolates the bug and makes it fixable
  locally.
- ADR-04's conditional ("if A05 lands X-`*`-only, `ATTACH`/`ATTENDEE`/`CATEGORIES` flip OUT→IN") is
  a live concern: the server preserves them, so anything caldir drops is caldir destroying data the
  server was holding correctly.

---

## Teardown performed

```sh
docker rm -f caldir-radicale-probe
```
