# QA-UCRAWLER + QA-REGRESSION

Independent QA pass on branch `vtodo-support` @ `1c573b4`, against `main` @ `2ed74fa`.
Author did not build the feature. No source file was modified; the working tree was clean
before and after (`git status --short` empty).

Binaries driven:
- **branch** — `caldir-cli 0.13.0`, `target/debug/caldir` (workspace build)
- **main** — `caldir-cli 0.12.1`, built in a throwaway `git worktree` (removed afterwards)

Isolation: every run used a private `XDG_CONFIG_HOME` sandbox under the scratchpad, so no
run touched the user's real `~/.config/caldir` or calendar directory.

---

## 1. The fixture, and the premise

**The premise is still UNVERIFIED in the strict sense: no captured `u_crawler` run exists on
disk.** `find /home/belcaik/Dev/u_crawler -iname '*.ics'` returns zero `.ics` files; the only
matches are markdown spike notes.

However this pass did substantially better than "representative". `u_crawler`'s **ICS
generator source** is on disk and was read directly — `/home/belcaik/Dev/u_crawler/src/calendar.rs`,
functions `render_vtodo` (:286-320) and `render_vevent` (:322-360), plus `deadline_uid` (:116),
`window_uid` (:132), `deadline_filename` (:121), `window_filename` (:141). The fixture below is
**reconstructed line-for-line from that generator**, and cross-checked against `u_crawler`'s own
test assertions (`src/calendar.rs:996,997,1279,1288,1583`) which pin the exact UID and `DUE`
forms. So it is *generator-faithful*, not guessed — but it is still **not a captured run**.

### Correction to the brief's premise

The brief asked for a VTODO and VEVENT **"sharing the UID stem `assignment-123`"**. Real
`u_crawler` does **not** do that. It deliberately gives them *different* UIDs:

```
VTODO   UID:u_crawler-todo-123@u-crawler.local
VEVENT  UID:u_crawler-window-123@u-crawler.local
```

The `todo-`/`window-` discriminators are load-bearing and documented as such in
`u_crawler/src/calendar.rs:107-115` — they exist precisely to avoid a CalDAV UID collision
(the source cites Radicale issue #101). What actually shares the stem is the **filename**:
both are `assignment-123.ics`, in sibling directories. The fixture uses the real distinct-UID
shape; the shared-UID case the brief assumed was tested separately in §6 and also passes.

### Fixture used

```
course/
  deadlines/assignment-123.ics     # VTODO, STATUS:COMPLETED
  windows/assignment-123.ics       # VEVENT
```

`deadlines/assignment-123.ics` (CRLF throughout, verified with `cat -A`):

```
BEGIN:VCALENDAR
VERSION:2.0
PRODID:-//u_crawler//calendar-sync//EN
BEGIN:VTODO
UID:u_crawler-todo-123@u-crawler.local
DTSTAMP:20260801T090000Z
DUE:20260826T235900Z
PRIORITY:1
STATUS:COMPLETED
SUMMARY:Assignment 3 - Recursive descent parser
URL:https://example.edu/courses/cs101/assignments/123
END:VTODO
END:VCALENDAR
```

`windows/assignment-123.ics` is the same assignment as a `VEVENT`
(`UID:u_crawler-window-123@…`, `DTSTART:20260820T000000Z`, `DTEND:20260826T235900Z`).

Note `render_vtodo` emits **no `COMPLETED` timestamp**, only `STATUS:COMPLETED`. The fixture
matches, which is why `completed` is `null` in the JSON below — that is correct, not a bug.

### One layout gotcha

`calendar_dir` must point at `course/`, so that `deadlines/` and `windows/` are the two
calendars. Pointing it at `course/`'s parent yields `No events found` with **exit 0 and no
diagnostic** — caldir does not recurse, and does not say so. Pre-existing behaviour, unrelated
to VTODO, but it will bite this exact workflow. Logged as F5.

---

## 2. Is the original bug gone? Yes. Captured proof.

Same fixture, same sandbox, both binaries, back to back.

**main (0.12.1) — the bug:**

```
$ caldir events
Error: calendar event error: expected exactly one event in .../course/deadlines/assignment-123.ics, found 0

Caused by:
    expected exactly one event in .../course/deadlines/assignment-123.ics, found 0
EXIT=1

$ caldir today
Error: calendar event error: expected exactly one event in .../course/deadlines/assignment-123.ics, found 0
EXIT=1
```

**branch (0.13.0) — fixed:**

```
$ caldir events
Wed Aug 19
    20:00 Assignment 3 - Recursive descent parser [windows]

Tomorrow
    19:59 [x] Assignment 3 - Recursive descent parser [deadlines]
EXIT=0
```

`caldir events 2>&1 | grep -i 'expected exactly one event'` → **no match**. The string appears
nowhere in branch output. (A near-twin string does appear on the *push* path — see F1.)

`today` and `week` both render, exit 0. `today` shows only the spanning window event; the task
is due tomorrow, so its absence from `today` is correct.

### `--json`

Both items present, correctly discriminated:

```json
{"kind":"event","calendar":"windows","instance_id":"u_crawler-window-123@u-crawler.local",
 "start":"2026-08-20T00:00:00Z","end":"2026-08-26T23:59:00Z","due":null,"task_status":null,...}
{"kind":"todo","calendar":"deadlines","instance_id":"VTODO:u_crawler-todo-123@u-crawler.local",
 "start":null,"end":null,"due":"2026-08-26T23:59:00Z","task_status":"completed",...}
```

---

## 3. Blast radius

This was the real symptom: on main, one VTODO blanked **the entire agenda across all
calendars**. Confirmed above — main exits 1 and renders nothing, so the perfectly valid
`windows/` VEVENT *in a different calendar* also disappears.

On the branch, with two deliberately bad files added to `deadlines/`
(`broken.ics` = raw garbage, `empty-vcal.ics` = component-less VCALENDAR):

```
$ caldir events
Wed Aug 19
    20:00 Assignment 3 - Recursive descent parser [windows]

Tomorrow
    19:59 [x] Assignment 3 - Recursive descent parser [deadlines]
EXIT=0
```

**Blast radius is fixed.** Bad files no longer take down their own calendar, let alone others.
Core models this explicitly: `CalendarListing` carries `events` *and* `skipped`, and
`caldir-core/src/calendar/listing.rs:27-29` documents "A listing is never silently short —
skipped files are always reported."

**But the agenda path never reports them.** Nothing on stdout, nothing on stderr, exit 0:

```
$ caldir events 2>&1 >/dev/null     # stderr only
[stderr ended]                       # <- empty
```

`grep -rn 'skipped\|is_incomplete' caldir-cli/src/` shows the only consumer of `skipped()` is
`commands/doctor.rs:57`. `doctor` does report both files clearly:

```
$ caldir doctor
📅 deadlines
   calendar event error: no VEVENT or VTODO in .../deadlines/empty-vcal.ics (found: nothing)
   calendar event error: invalid event in ICS file .../deadlines/broken.ics: failed to parse ICS ...
```

So the answer to "does a genuinely malformed file still warn rather than being silently
swallowed?" is: **only under `doctor`. Under `events`/`today`/`week` it is silently swallowed.**
Behaviour swung from too loud (main: hard-fail everything) to fully silent. Logged as F2.

---

## 4. Radicale round-trip

Ran. `tomsquest/docker-radicale:latest`, container `caldir-qa-radicale`, ephemeral port 15232,
plaintext htpasswd, `owner_only` rights. **Torn down at the end — `docker ps -a --filter
name=caldir` returns nothing.** (Note: the repo already ships a live-gate harness at
`caldir-provider-caldav/src/live/radicale.rs` behind `CALDIR_LIVE_RADICALE=1`; this pass drove
the CLI independently of it.)

### Pull — a task placed on the server by other means

`VTODO` PUT to the server with `curl` (deliberately not caldir's own client), then:

```
$ caldir pull
📅 qacal
   + Server-side task from Radicale  Tue Sep 1, 19:59

Pulled: 1 created, 0 updated, 0 deleted
EXIT=0
```

Landed on disk as `_task__2026-09-01T1959__serverside-task-from-radicale.ics`:

```
BEGIN:VTODO
DTSTAMP:20260826T003618Z
DUE:20260901T235900Z
PRIORITY:1
STATUS:COMPLETED
SUMMARY:Server-side task from Radicale
UID:u_crawler-todo-999@u-crawler.local
END:VTODO
```

`DUE`, `PRIORITY`, `STATUS:COMPLETED` and `UID` all survive. `--json` reports
`kind:"todo"`, `task_status:"completed"`. **Pull is correct.**

### Push — aborts, loses nothing, but not loudly

```
$ caldir push
📅 qacal
   + Locally created task to push  Sat Sep 5, 19:59
   Remote error: expected 1 event(s) in ICS, found 0
$ echo $?
0
```

Nothing lost: local files intact, task body byte-identical, server unchanged (`PROPFIND`
shows only the pre-existing `u_crawler-todo-999`). A follow-up `pull` says "No changes to
pull" and deletes nothing; three consecutive `sync` runs leave the task intact. **No data
loss, no state corruption.** That half of the requirement passes.

The "loudly" half does not — see F1. Three problems in that four-line transcript:
1. **`push` exits 0** despite the failure.
2. The task is printed with a green `+` as though it pushed, *then* an error follows.
3. The message is `expected 1 event(s) in ICS, found 0` — the sibling of the original bug's
   error (`EventError::UnexpectedEventCount`) — rather than "caldir cannot push tasks". A user
   who just upgraded to fix that error sees it again and reasonably concludes the fix failed.

### Mixed calendar — failure is contained

A calendar holding **both** an unpushable task and a pushable event:

```
$ caldir push
📅 mixed
   + Pushable event  Thu Sep 3, 06:00
   + Unpushable task Thu Sep 3, 19:59
   Remote error: expected 1 event(s) in ICS, found 0
$ curl PROPFIND .../mixed/   →   /testuser/mixed/mix-ev%40test.ics
```

The event **did** reach the server; the task did not. The task failure does not block its
event siblings. Good. The misleading `+` on the task line remains.

---

## 5. Regression results

### Per crate — all green

| crate | passed | failed |
|---|---|---|
| caldir-core (lib) | 581 | 0 |
| caldir-core (`tests/public_error_api.rs`) | 3 | 0 |
| caldir-cli | 139 | 0 |
| caldir-provider-caldav | 14 | 0 |
| caldir-provider-google | 49 | 0 |
| caldir-provider-icloud | 14 | 0 |
| caldir-provider-outlook | 49 | 0 |
| caldir-provider-webcal | 14 | 0 |
| **total** | **863** | **0** |

### Per timezone — one failure, in far-east zones

`TZ=<zone> cargo test --workspace --no-fail-fast`:

| TZ | passed | failed |
|---|---|---|
| UTC | 906 | 0 |
| America/New_York | 906 | 0 |
| Europe/Berlin | 906 | 0 |
| America/Santiago | 906 | 0 |
| Asia/Kolkata | 906 | 0 |
| Australia/Lord_Howe | 906 | 0 |
| **Pacific/Kiritimati** (UTC+14) | 905 | **1** |
| **Pacific/Chatham** (UTC+12:45) | 905 | **1** |

```
---- output::agenda::tests::assemble_sorts_and_carries_the_window_and_the_undated_flag ----
panicked at caldir-cli/src/output/agenda.rs:597:9:
assertion `left == right` failed
  left: 2026-08-15
 right: 2026-08-14
```

Exactly one test, same one in both zones; nothing else fails. The test is **new on this
branch** — `git show main:caldir-cli/src/output/agenda.rs | grep -c
assemble_sorts_and_carries_the_window_and_the_undated_flag` → `0`.

Cause: the test builds `from = 2026-08-14 12:00 UTC` and asserts `view.range_start ==
2026-08-14`, but `assemble` converts the window bound to the **local** date, which at UTC+14
is 2026-08-15. The production conversion is defensible; the *test* hardcodes a UTC-ish
expectation. So this reads as a **test defect, not a product defect** — but it is a new,
branch-introduced red build for any contributor or CI runner in UTC+13/+14. Logged as F3.

(The first sweep also showed only 138 tests running in those zones — cargo's fail-fast aborts
the remaining binaries. `--no-fail-fast` confirms the other 905 do pass.)

### Real event workflows — main vs branch, byte-compared

Identical event-only fixture (plain timed event, all-day event, `RRULE` recurring event,
`METHOD:REQUEST` invite) built twice, one sandbox per binary.

`events`, `today`, `week`, `invites`, `doctor`, `calendars`, `status` — full transcripts
diffed:

```
>>> IDENTICAL across all event workflows
```

**`--json` schema** — additive only:

```
removed: []
added:   ['completed', 'due', 'kind', 'percent_complete', 'task_status']
shared-key value comparison done      # no value differs on any shared key
```

Every pre-existing key survives with an unchanged value for every event. No consumer of the
old schema breaks.

**`caldir new`** — `--help` identical; creating the same event on both produced the same
filename (`2026-08-27T1500__design-review.ics`, i.e. **no `_task_` prefix leaked onto
events**) and byte-identical ICS modulo `DTSTAMP`/`UID`.

**`caldir rsvp`** — `--help` identical; same invocation gives the same error on both
(`No account email configured for this calendar`, exit 1) and leaves the invite ICS
byte-identical.

**Connected calendar, bidirectional sync via Radicale** — event-only calendar, one collection
per binary, running `status → pull → local add → status → push → status → ls`:

```
>>> CONNECTED EVENT SYNC IDENTICAL
```

Both pull the server event, both push the local event, both end at "No changes", both leave
the same two files on disk.

**Conclusion: no event regression found in any workflow driven.**

### Behaviour change worth calling out

Malformed file present, event-only calendar:

- **main:** `caldir events` → `Error: … failed to parse ICS …`, **exit 1**, nothing rendered
- **branch:** `caldir events` → full agenda rendered, **exit 0**, no mention of the bad file

Intentional and much better on balance, but it is a real change in contract for any script
that relied on a nonzero exit to detect corrupt data. See F2.

---

## 6. Extra cases checked

**Shared UID (the brief's assumed shape).** A VTODO and a VEVENT with the *identical*
`UID:assignment-123@u-crawler` in sibling calendars: both render, `doctor` is clean, and
`instance_id` keeps them apart via the `VTODO:` prefix —

```
event | assignment-123@u-crawler       | Window (event)
todo  | VTODO:assignment-123@u-crawler | Deadline (task)
```

Handled correctly even though real u_crawler never produces it.

**Undated tasks.** Hidden by default with a discoverability hint, shown under `--undated`:

```
$ caldir events
No events found
+ 1 undated task (caldir events --undated)

$ caldir events --undated
No date
          [ ] Someday task with no due date [mixed]
```

Good UX; no complaint.

---

## 7. Findings

| # | Severity | What | Reproduction | Documented? |
|---|---|---|---|---|
| **F1** | **High** | `caldir push` **exits 0** when a task push fails. The task is also printed with a green `+` as if it succeeded, and the error reads `expected 1 event(s) in ICS, found 0` — the twin of the original bug's message — instead of "caldir cannot push tasks". An automated Canvas→caldir→Radicale pipeline (the whole point of this effort) cannot detect the failure, and a human reads it as a regression. No data is lost. | Connect a calendar to CalDAV, drop a `VTODO` in it, `caldir push; echo $?` → prints `Remote error: expected 1 event(s) in ICS, found 0`, exits `0`. | Push-is-unsupported is documented as a design limit; the **silent exit 0** and the **misleading error text + green `+`** are not. |
| **F2** | **Medium** | `events`/`today`/`week` silently skip unreadable `.ics` files — nothing on stdout or stderr, exit 0. Core deliberately exposes `CalendarListing::skipped()`/`is_incomplete()` and documents "a listing is never silently short", but the only CLI consumer is `doctor`. main hard-failed (exit 1); the branch says nothing at all. | Put `garbage` in `cal/broken.ics`; `caldir events` renders the rest with exit 0 and no mention. `caldir doctor` does report it. | No. Contradicts `caldir-core/src/calendar/listing.rs:27-29`. |
| **F3** | **Medium** | New branch-only test `assemble_sorts_and_carries_the_window_and_the_undated_flag` fails in UTC+13/+14 zones — asserts a UTC date against a local-date conversion. Test defect, not product defect, but it is a red build. | `TZ=Pacific/Kiritimati cargo test -p caldir-cli assemble_sorts_and_carries` → `left: 2026-08-15, right: 2026-08-14` at `agenda.rs:597`. Same in `Pacific/Chatham`. | No. |
| **F4** | Low | `caldir doctor` exits 0 even when it prints errors, so it cannot gate a script. **Pre-existing — main behaves identically.** Not a regression; noted because F2 makes `doctor` the only place bad files surface, which raises the cost of its exit code being useless. | `caldir doctor; echo $?` with a malformed file → prints the error, exits 0. On both binaries. | No. |
| **F5** | Low | If `calendar_dir` is set one level above the calendars (e.g. at `course/`'s parent), caldir reports `No events found`, exit 0, no hint that the directory has non-calendar nesting. Pre-existing, but this is exactly the layout a u_crawler user is likely to misconfigure. | Point `calendar_dir` at a dir whose children are dirs-of-dirs; `caldir calendars` lists `course`, `caldir events` says `No events found`. | No. |
| **F6** | Info | `--json` is rejected by `doctor` (`Error: --json is not yet supported for this command`, exit 1), so the one command that surfaces skipped files has no machine-readable output. Compounds F1/F2 for automated pipelines. | `caldir --json doctor` → exit 1. | Message is explicit, so: yes, at the point of use. |
| **F7** | Info | On a completed task, `--json` reports `"task_status":"completed"` but `"status":"confirmed"`. Correct per the data model (`STATUS` in a VTODO *is* the task status; `status` is the event-status field carrying its default), but a consumer reading `status` sees a completed task described as "confirmed". | `caldir --json events` on the fixture in §1. | Not called out. |

---

## 8. Verdict

- The original `expected exactly one event … found 0` failure is **gone**, proven by running
  main and the branch against the same generator-faithful fixture.
- The blast radius — one bad file blanking the whole agenda across all calendars — is
  **fixed**.
- **No event regression** was found in any workflow driven: 863 tests green per crate, the
  JSON schema change is purely additive with no value drift, and event create / list / json /
  rsvp / invites / doctor / status / connected bidirectional sync are byte-identical to main.
- Two things stand between this and a clean bill of health: **F1** (task push fails silently
  with exit 0 and a misleading message) and **F2** (bad files are now invisible outside
  `doctor`). Both are reporting problems, not correctness problems — nothing loses data.
