# QA report — QA-CORE / QA-MIXED (independent, adversarial)

**Scope:** parse / serialize / identity / equality / filenames for `VTODO`, and calendars holding
both kinds. Driven by **executing the real binary** and the real `caldir-core` codec, not by
reading tests.

**Method.**
1. `cargo build -p caldir-cli` at `2ed74fa`+branch (`caldir-core 0.15.0`, `caldir-cli 0.13.0`).
2. A scratch round-trip harness linking `caldir-core` by path with **`icalendar = "=0.17.10"`**
   pinned (R108) — every codec claim below was re-measured on the pinned version after an
   unpinned first pass resolved 0.17.13. Results were identical on both.
3. A scratch provider binary `caldir-provider-qamock` on `PATH`, backed by a directory of `.ics`
   files, so `pull` / `push` / `sync` / `status` / `discard` run end to end with real RPC.
4. A **baseline binary built from the merge-base `2ed74fa` (`caldir-core 0.14.1`)** to measure the
   VEVENT-regression rows (C1/C2/C4/C13) and to separate pre-existing behaviour from new defects.

No repo source file was modified. The only repo write is this document.

---

## 1. Findings table

| # | Sev | What | Reproduction | Documented? |
|---|---|---|---|---|
| F-1 | **MAJOR** | **Dated tasks escape the query window.** `Calendar::expanded_events_in_range(from, to)` filters events through `expand_in_range` but appends **every** task unconditionally (`caldir-core/src/calendar.rs:390` — `items.extend(tasks);`). `from`/`to` are never applied to tasks. `caldir events --from/--to --json` therefore emits tasks whose `DUE` is outside the requested range. | §3.1 | **NO.** Contradicts ADR-10 D5's own defence: *"dated completed tasks are bounded by the window."* Q-1 covers only `--undated`, not the date range. |
| F-2 | **MAJOR** | **Unparseable task property values are silently discarded.** `DUE`, `DTSTART`, `COMPLETED`, `PRIORITY`, `PERCENT-COMPLETE`, `SEQUENCE` and a malformed `VALARM` all vanish with no error. A bad `DUE` silently converts a **dated** task into an **undated** one — and an undated task is *unconditionally in-window* (R44 row 4), so the item's sync semantics change too. caldir's next write to that file deletes those properties from disk and, on a writable remote, from the server. The VEVENT control errors loudly for the equivalent case (`DTSTART:NOTADATE` → `event is missing a start time (DTSTART)`). | §3.2 | **NO.** Not in BACKLOG §C; not covered by R15's guarantee (which is about *unmodelled* properties, not unparseable values of modelled ones). |
| F-3 | **MAJOR** (mechanism pre-existing, consequence new) | **One unpushable task permanently blocks every other outgoing change in that calendar — and the CLI reports the blocked changes as if they succeeded.** With one locally-edited task pending, three new local events were rendered green (`+ Multi 1/2/3`) and **none reached the remote**. Exit code `0`. | §3.3 | **PARTLY.** C-1 documents the abort; C-3 documents "aborts the rest of that calendar's push" but titles it *Google/Outlook* only — the mechanism is in core (`CalendarItem::into_event`) and hits CalDAV/Radicale identically. The **optimistic green rendering** and the **exit code 0** are undocumented. |
| F-4 | MINOR | **`PRIORITY` above 255 or negative is silently dropped.** `PRIORITY:256` and `PRIORITY:-1` are legal RFC 5545 `INTEGER`s; both parse to `None` and disappear from the serialized output. `PERCENT-COMPLETE:250` is *kept*, so the two `Option<u8>` fields behave inconsistently at the edge. | §3.4 | **NO.** Contradicts spec §6 / R13: *"Out-of-range MUST round-trip verbatim."* T-15 only exercises `42`. |
| F-5 | MINOR | **An unknown `STATUS` value silently collapses to the RFC default.** `STATUS:X-WAITING` parses as `NeedsAction` and no `STATUS` line is emitted on write, so the server ends up with `NEEDS-ACTION`. | §3.5 | **PARTLY.** R14 permits "dropped with the loss pinned by a test", and `T-14` pins it. But it is **absent from BACKLOG §C** and from R15's user guarantee, so a user reading the limitation list will not find it. |
| F-6 | MINOR | **R48 delete suppression is completely silent.** With an unreadable file in the calendar, a pull that would have deleted a task prints `No changes to pull` and says nothing about the withheld delete. The sibling suppression (R49, task-less remote) prints a loud, excellent warning. | §3.6 | **NO.** |
| F-7 | MINOR (pre-existing) | **`push` / `sync` / `pull` / `events` / `doctor` exit `0` on error.** A remote error, a local calendar error, and a broken `.ics` all exit 0. Reproduced identically on the **baseline** binary, so it is not a VTODO regression — but F-3 makes it far more consequential, because a task turns a transient failure into a permanent one that no scripted sync will ever notice. | §3.7 | **NO.** |
| F-8 | MINOR (pre-existing) | **Stack overflow → `SIGABRT` + core dump on a deeply nested `.ics`.** ~1000 nested `BEGIN:VTODO` (or `BEGIN:VEVENT`) in one file kills the whole process; `caldir events`, `caldir doctor` and therefore `pull`/`sync` all abort. One such file anywhere under `calendar_dir` takes the binary down. Identical with VEVENT, so pre-existing `icalendar` parser recursion — but VTODO widens the input surface (a CalDAV response body). | §3.8 | **NO.** |
| F-9 | NIT | `known_event_ids` can contain a **literally duplicated line** when a VEVENT UID starts with `VTODO:` — two distinct identities serialize to one string. Deletes still resolved correctly in the end-to-end test. | §3.9 | C-11 documents the *semantics* of the collision; the duplicate line is not mentioned, and C4 says "one id per line". |
| F-10 | NIT (pre-existing) | `known_event_ids` is written **without a trailing newline**, so any append-style writer merges the appended id into the last one. | §3.9 | NO. |
| F-11 | NIT | Task-only output uses event vocabulary: pulling 6 tasks renders `+ (6 new events)`. | §3.10 | A-9 records the type-name residue; the user-facing string is arguably covered by A-6's "no renderer or pluralization". |
| F-12 | NIT | A `VTODO` with no `UID` errors with **`event is missing a UID`**. | §3.11 | NO. |
| F-13 | NIT (doc) | C-9's wording — *"The five additive keys are appended at the end, so the pre-existing key order is preserved"* — is wrong about the emitted bytes. `AgendaJson` uses `#[serde(flatten)]`, so serde_json serializes through a `BTreeMap` and **all 31 keys come out alphabetically sorted**; the new keys are interleaved. The guarantee C-9 actually makes (the `del(...)` predicate) **does** hold — verified against the real baseline binary in three timezones. | §4.1 | Wording only. |
| F-14 | NIT (pre-existing) | A UTF-8 BOM makes an `.ics` unparseable. Identical for VEVENT on the baseline binary. | §3.12 | NO. |
| F-15 | NIT (pre-existing) | A non-UTF-8 file produces `calendar event error: io error: stream did not contain valid UTF-8` with **no filename**, so the user cannot find it. | §3.12 | NO. |

**Crashes / hangs / data loss:** one crash (**F-8**, pre-existing, `SIGABRT` + core dump).
No hangs. Data loss: **F-2** (unparseable modelled values), **F-4** (out-of-`u8` `PRIORITY`),
**F-5** (unknown `STATUS`), and **F-3** (outgoing event changes reported as applied but never
sent — lost until the next run, and permanently blocked while the task remains).

---

## 2. Single most serious finding

> **`Calendar::expanded_events_in_range` does not apply its own `from`/`to` window to tasks.**
> Every task in the calendar is returned for every range query. The public `--json` agenda
> therefore emits tasks that fall entirely outside `--from`/`--to`, directly contradicting
> ADR-10 D5's stated defence that *"dated completed tasks are bounded by the window"* — which is
> the argument the ADR uses to justify showing completed tasks by default. The text renderer
> masks it only because it groups by day and never prints a day outside the range.

`Todo::occurs_in_range` — the correct predicate — already exists and is used by the diff. It is
simply never called on this path.

---

## 3. Every case run, with real input and real output

### 3.1 F-1 — dated tasks escape the window (clean reproduction)

Calendar `c` holds one event on Sep 1 and one task due Sep 1. Query Oct 1–2:

```
$ caldir events --from 2026-10-01 --to 2026-10-02
No events found
$ caldir events --from 2026-10-01 --to 2026-10-02 --json
[{"all_day":false,...,"due":"2026-09-01T17:00:00Z",...,"kind":"todo",...,
  "summary":"Task due Sep 1",...}]
```

Control, same query, same tree: `grep -c '"kind": "event"'` → **0**. Events are filtered; tasks
are not. A second reproduction with a wider corpus:

```
### TEXT --from 2026-09-05 --to 2026-09-06
No events found
+ 2 undated tasks (caldir events --undated)
### JSON --from 2026-09-05 --to 2026-09-06
todo | Assignment 123 deadline |start None |due 2026-09-01T17:00:00Z
todo | Assignment 123 deadline |start None |due 2026-09-01T17:00:00Z
todo | Weekly chore          |start None |due 2026-09-03T17:00:00Z
todo | Someday maybe         |start None |due None
todo | Someday maybe         |start None |due None
```

Source, `caldir-core/src/calendar.rs:385-392`:

```rust
let mut items: Vec<CalendarItem> = expand_in_range(events, from, to)
    .into_iter()
    .map(CalendarItem::Event)
    .collect();

items.extend(tasks);
```

The in-crate tests around this method assert the two *correct* cases (an undated task survives —
R46; a recurring task yields only its master) but never a dated task outside the window, so the
suite is green.

### 3.2 F-2 — unparseable modelled values vanish silently

Input (pinned `=0.17.10`):

```
BEGIN:VCALENDAR
VERSION:2.0
BEGIN:VTODO
UID:t
DUE:NOTADATE
DTSTART:ALSOBAD
COMPLETED:BAD
PRIORITY:abc
PERCENT-COMPLETE:xyz
SEQUENCE:nope
SUMMARY:baddate
BEGIN:VALARM
ACTION:DISPLAY
TRIGGER:GARBAGE
END:VALARM
END:VTODO
END:VCALENDAR
```

Parsed:

```
COMPONENTS: 1
[0] id=EventInstanceId { kind: Todo, uid: EventUid("t"), recurrence_id: None }
  start=None due=None
  status=NeedsAction completed=None pct=None prio=None
  rec=None rid=None rem=[]
  created=None lm=None seq=0
```

Serialized — nine properties in, two out:

```
BEGIN:VCALENDAR
VERSION:2.0
PRODID:CALDIR
BEGIN:VTODO
DTSTAMP:20260826T004540Z
SUMMARY:baddate
UID:t
END:VTODO
END:VCALENDAR
```

`SLUG: _task__baddate` — the dated task has become an undated one, filename included.

**VEVENT control** (`DTSTART:NOTADATE`):

```
[0] ITEM-ERR: event is missing a start time (DTSTART)
```

So the loud path exists; tasks do not use it. (Honest caveat: for *optional* event properties the
same silent drop is pre-existing — `DTEND:GARBAGE` on a VEVENT also parses clean. What is new is
that `DUE` is the task's **anchor**, so its loss changes windowing and filename, not just a
display field.)

Through the CLI, the same corpus lists as ordinary undated tasks with no warning:

```
No date
          [ ] baddate [junk]
          [ ] bigdate [junk]
          [ ] alarm [junk]
```

### 3.3 F-3 — one task blocks the calendar's whole push, rendered as success

`mixed` calendar, remote is `caldir-provider-qamock`. One task edited locally, then three new
local events added:

```
$ caldir push
📅 mixed
   ~ EDITED deadline Tue Sep 1, 13:00
     summary: Assignment 123 deadline → EDITED deadline
   + Multi 3 Sun Sep 13, 06:00
   + Multi 1 Fri Sep 11, 06:00
   + Multi 2 Sat Sep 12, 06:00
   Remote error: expected 1 event(s) in ICS, found 0
exit=0
$ ls remote/
ev.ics  localnew@qa.ics  new.ics  td2.ics  td.ics      # no multi1/2/3
```

The provider's own RPC log for that run confirms nothing was attempted — the only `CREATE_EVENT`
in the whole session is the earlier one that happened to be ordered before the task:

```
CREATE_EVENT uid=localnew@qa
LIST_EVENTS from=2025-08-25T04:00:00+00:00 to=2027-08-26T03:59:59+00:00
  -> 5 items
                      <- the three-event push: one LIST_EVENTS, zero CREATE_EVENT
```

Three creates printed in green, zero applied, exit 0. The task change is applied first, so it
aborts everything queued behind it. The state is *not* corrupted — a later `pull` still works and
local files are untouched — but the calendar can never push again while the task is unsynced.

`sync` behaves the same, with the pull half still completing:

```
$ caldir sync
📅 mixed
   Local changes (to push):
   ~ EDITED deadline ...
   Remote changes (to pull):
   + New remote event Thu Sep 10, 06:00
   Remote error: expected 1 event(s) in ICS, found 0
Pulled: 1 created, 0 updated, 0 deleted
exit=0
```

The byte-exact error string `expected 1 event(s) in ICS, found 0` (C-1 / R71) is confirmed.

### 3.4 F-4 — `PRIORITY` edge values

| Input | `priority` | Emitted |
|---|---|---|
| `PRIORITY:0` | `Some(0)` | `PRIORITY:0` |
| `PRIORITY:9` | `Some(9)` | `PRIORITY:9` |
| `PRIORITY:42` | `Some(42)` | `PRIORITY:42` |
| `PRIORITY:99` | `Some(99)` | `PRIORITY:99` |
| `PRIORITY:255` | `Some(255)` | `PRIORITY:255` |
| **`PRIORITY:256`** | **`None`** | **(absent)** |
| **`PRIORITY:-1`** | **`None`** | **(absent)** |
| `PERCENT-COMPLETE:0` | `Some(0)` | `PERCENT-COMPLETE:0` |
| `PERCENT-COMPLETE:100` | `Some(100)` | `PERCENT-COMPLETE:100` |
| `PERCENT-COMPLETE:250` | `Some(250)` | `PERCENT-COMPLETE:250` |

Real output for `PRIORITY:256`:

```
  status=NeedsAction completed=None pct=None prio=None
--- SERIALIZED ---
BEGIN:VCALENDAR
VERSION:2.0
PRODID:CALDIR
BEGIN:VTODO
DTSTAMP:20260826T003402Z
SUMMARY:P256
UID:t1@qa
END:VTODO
END:VCALENDAR
```

### 3.5 F-5 — `STATUS` values

| Input | `status` | Emitted |
|---|---|---|
| `STATUS:NEEDS-ACTION` / absent | `NeedsAction` | (omitted, correct — RFC default) |
| `STATUS:IN-PROCESS` | `InProcess` | `STATUS:IN-PROCESS` |
| `STATUS:COMPLETED` | `Completed` | `STATUS:COMPLETED` |
| `STATUS:CANCELLED` | `Cancelled` | `STATUS:CANCELLED` |
| **`STATUS:X-WAITING`** | **`NeedsAction`** | **(absent)** |
| `STATUS:completed` (lower) | `NeedsAction` | (absent) |

The lowercase case is **not** a VTODO regression: `event::Status::from_ics_str` is equally
case-sensitive (`caldir-core/src/event/status.rs:35`), so tasks match the pre-existing event rule.

### 3.6 F-6 — silent vs loud delete suppression

Loud (R49, remote reported zero tasks):

```
📅 c1
   ⚠ task deletion withheld: the remote reported no tasks at all, which is
     indistinguishable from a provider that cannot list them. No task file was removed.
```

Silent (R48, one unreadable local file present, one task genuinely deleted on the remote):

```
📅 c1
   No changes to pull
exit=0
```

Removing the unreadable file and re-pulling then produced `- Same title` / `1 deleted`, proving
the delete really had been withheld. Correct behaviour, zero user signal.

### 3.7 F-7 — exit codes

```
$ caldir push                      # remote error (task)              → exit 0
$ caldir push                      # remote error (provider failure)  → exit 0   [baseline too]
$ caldir doctor                    # broken .ics                      → exit 0   [baseline too]
$ caldir events                    # broken .ics                      → exit 0   [baseline too]
```

Baseline (`0.14.1`) reproduction of the event-only case:

```
📅 evonly
   + Fail me Tue Sep 15, 06:00
   Remote error: Provider error: ... simulated server failure
exit=0
```

### 3.8 F-8 — stack overflow

```
$ caldir doctor
thread 'main' (3636386) has overflowed its stack
fatal runtime error: stack overflow, aborting
timeout: the monitored command dumped core
```

Bisected by nesting depth, one component kind per row:

| n | `BEGIN:VTODO` ×n | `BEGIN:VEVENT` ×n |
|---|---|---|
| 100 | exit 0 | exit 0 |
| 500 | exit 0 | exit 0 |
| 1000 | **exit 134** | **exit 134** |
| 5000 | **exit 134** | **exit 134** |

Identical for both kinds ⇒ pre-existing `icalendar` parser recursion, not a VTODO regression.

### 3.9 F-9 / F-10 — `known_event_ids`

After a pull of a `UID:VTODO:sneaky@qa` **VEVENT**, a `UID:sneaky@qa` **VTODO**, and a
`UID:other@qa` VTODO into one calendar:

```
VTODO:other@qa
VTODO:sneaky@qa
VTODO:sneaky@qa            <- two distinct identities, one string, written twice
```

`doctor` stayed quiet, and both deletes still resolved against the right files:

```
$ rm remote/real.ics && caldir pull   →  - Real task    (1 deleted, sigil event kept)
$ rm remote/sig.ics  && caldir pull   →  - Sigil event  (1 deleted)
```

`cat -A` shows the file's final line carries no `$`, i.e. **no trailing newline**; appending a
line with `>>` produced `d@qaVTODO:ghost@qa` (my own test artefact, but the missing newline is
the binary's).

### 3.10 F-11 — vocabulary

```
$ caldir pull
📅 c1
   + (6 new events)
Pulled: 6 created, 0 updated, 0 deleted
```

All six were VTODOs.

### 3.11 F-12 — error text

```
$ echo 'BEGIN:VCALENDAR ... BEGIN:VTODO / DTSTAMP / SUMMARY:no uid / END:VTODO ...' | rtqa
[0] ITEM-ERR: event is missing a UID
SINGLE-ERR: event is missing a UID
```

### 3.12 F-14 / F-15 — BOM and non-UTF-8

```
PARSE-DOC-ERR: failed to parse ICS ﻿BEGIN:VCALENDAR   (VTODO, BOM)
```
Baseline binary, VEVENT with BOM: `calendar event error: invalid event in ICS file …: failed to
parse ICS ﻿BEGIN:VCALENDAR` — same failure, so pre-existing.

Non-UTF-8 file, current binary:

```
📅 junk
   calendar event error: io error: stream did not contain valid UTF-8
```

No path in the message.

---

## 4. What I verified as **working**

### 4.1 VEVENT non-regression, measured against the real pre-change binary

A baseline `caldir` was built from the merge-base `2ed74fa` (`caldir-core 0.14.1`,
`caldir-cli 0.12.1`) and run side by side with the branch binary over the same trees, in
`TZ=UTC`, `TZ=Europe/Stockholm` and `TZ=America/New_York`.

Corpus: a timed event with `DESCRIPTION`/`LOCATION`/`URL`/`ORGANIZER`/`ATTENDEE`/`ATTACH`/`X-FOO`
with a parameter/`STATUS:TENTATIVE`/`TRANSP`/`CLASS`/`SEQUENCE`/`LAST-MODIFIED`/`VALARM`; an
all-day event with an emoji + accented summary; a zoned recurring master; an untitled event.

| Row | Result (all three TZs) |
|---|---|
| **C1** filenames written by `pull` | **IDENTICAL** (`2026-09-01T0900__timed-event.ics`, `2026-09-02__all-day-cafe.ics`, `_recurring__recurring-standup.ics`, `2026-09-04T0900__untitled.ics`) |
| **C1/C2** file bytes (minus `DTSTAMP`) | **IDENTICAL** |
| **C4** `known_event_ids` | **IDENTICAL** |
| **C5** `state/bases/<sha256>.ics` filenames | **IDENTICAL** |
| **C13** `caldir events` text | **IDENTICAL** (`diff` empty) |
| **C12** `--json` DEL predicate | `select(.kind=="event") \| del(.kind,.due,.completed,.percent_complete,.task_status)` reproduces the baseline stream **exactly** (2809 → 3239 bytes raw; 2809 after the predicate) |

**C6 (downgrade).** The baseline binary, pointed at a tree containing a VTODO file:

```
$ caldir events   → Error: calendar event error: expected exactly one event in …_task__….ics, found 0
$ caldir pull     → Local calendar error: … found 0        files unchanged: YES
$ caldir doctor   → calendar event error: … found 0
```

Fails loudly, writes nothing, deletes nothing. **C4(b) downgrade:** a hand-inserted
`VTODO:ghost@qa` line was carried through untouched by the baseline binary with no error and no
delete — inert, exactly as C4 requires.

### 4.2 Round-trip fidelity (all measured on pinned `icalendar =0.17.10`)

40 fixtures. For every one that loads, `parse → serialize → parse` gave `ROUNDTRIP_EQ: true` and
generation-2 bytes identical to generation-1 (minus `DTSTAMP`). Specifically:

- **Floating `COMPLETED`** `COMPLETED:20240415T170000` → `DateTimeFloating(2024-04-15T17:00:00)`,
  re-emitted verbatim. (`get_completed()` correctly bypassed — R22.)
- **Zoned `COMPLETED`** `COMPLETED;TZID=America/New_York:…` → preserved with its TZID.
- **Windows TZID** `DUE;TZID=W. Europe Standard Time:20240415T170000` → normalized to
  `DUE;TZID=Europe/Berlin:20240415T170000` at the parse boundary (project rule).
- **`VALARM`** round-trips through the hand-rolled splice and is byte-stable across three
  generations (`TRIGGER:-PT15M` → `TRIGGER;RELATED=START:-PT15M`, then stable — R24 / C-8).
- **`X-*` with parameters** `X-FOO;BAR=baz;QUX=1:hello` → `params: [("BAR","baz"),("QUX","1")]`,
  re-emitted verbatim; re-ordering does not break `==`.
- **`DTEND` on a VTODO** is read into nothing and **never emitted** (R8/R26 confirmed on real
  bytes).
- **`RRULE` / `RECURRENCE-ID`** preserved; the master is never expanded; `RECURRENCE-ID` lands in
  `EventInstanceId { kind: Todo, uid, recurrence_id: Some(...) }`.
- **`CREATED` / `LAST-MODIFIED` / `SEQUENCE`** preserved and correctly ignored by `==`.
- **`ORGANIZER` / `ATTENDEE` / `ATTACH` / `LOCATION` / `DESCRIPTION` / `URL`** all preserved.
- **Documented drops behaved exactly as documented:** `CATEGORIES`, `RELATED-TO` (both lines),
  `GEO`, `CLASS` silently dropped on first write (R15, C-4, C-5, A-2, A-3); repeated `X-FOO`
  collapses to one.
- **Unicode** `Café naïve — 日本語 test ✅` round-trips; slug `_task__cafe-naive-ri-ben-yu-test`.
- **Emoji-only summary** `🎉🎊🥳` round-trips; slug falls back to `_task__untitled`.
- **300-char summary** round-trips; slug capped at 50 chars.

### 4.3 Malformed input and arity (R19 / R32 / R75)

| Input | Result |
|---|---|
| 0 components | `expected 1 supported component(s) in ICS, found 0` / on disk: `no supported component` |
| 2 VEVENTs | `expected 1 supported component(s) in ICS, found 2` |
| 2 VTODOs | `expected 1 supported component(s) in ICS, found 2` |
| 2 VTODOs sharing a UID | `… found 2` |
| **1 VEVENT + 1 VTODO** | `expected exactly one VEVENT or VTODO in …/mixed.ics, found 2; split it into one file per item` — **A-7 satisfied**, no silent drop |
| VTODO + VJOURNAL | `unsupported component VJOURNAL in ICS; caldir supports VEVENT and VTODO only` |
| VTODO + `X-FOO` component | `unsupported component X-FOO in ICS; caldir supports VEVENT and VTODO only` |
| **VTODO + VTIMEZONE** | **loads as one task**, TZID preserved — A-8 / R18 satisfied |
| truncated ICS | `failed to parse ICS …` (loud) |
| unterminated component | `failed to parse ICS …` (loud) |
| missing `UID` | `event is missing a UID` (loud; wording nit F-12) |
| **missing `DTSTAMP`** | **loads** (R25 — `DTSTAMP` is never a field) |
| LF-only line endings | loads; re-emitted CRLF (R28) |
| duplicate `SUMMARY` | last wins (`SUMMARY:second`), no error |
| empty `SUMMARY` | `summary: None`, slug `_task__untitled` |

### 4.4 Filenames (R34)

Written to disk by a real `pull` (local tz `America/New_York`, `DUE:…T170000Z` → `T1300`):

```
_task__2026-09-01T1300__assignment-123-deadline.ics   dated (DUE)
_task__recurring__weekly-chore.ics                    RRULE master
_task__undated-remote-task.ics                        undated
2026-09-01T0500__assignment-123-window.ics            the same-UID event, unprefixed
```

From the codec harness (`Todo::base_slug()` on parsed fixtures):

```
_task__2024-04-15__date-due            DUE;VALUE=DATE:20240415
_task__2024-04-15T0500__has-dtend      DTSTART fallback when DUE is absent
_task__recurring__water-the-plants     RRULE master, DUE present but not used
_task__someday-maybe                   undated
_task__2024-04-15T1300__override       RECURRENCE-ID present — not part of the stem,
                                       matching the event rule
```

Collision and safety behaviour, all produced by an actual pull:

```
_task__2026-09-01T1300__same-title.ics       two distinct UIDs, same DUE + summary
_task__2026-09-01T1300__same-title-2.ics     → writer disambiguates with -2
_task__evil-path.ics                         SUMMARY:"../../../evil path" — traversal neutralized
_task__untitled.ics / _task__untitled-2.ics  two emoji-only summaries
_task__xxxxxxxx…x.ics (50 chars)             400-char summary, capped
```

Second pull: `No changes to pull` — idempotent.

### 4.5 Identity, equality, sigil

- `(kind, uid, recurrence_id)` keeps a VEVENT and a VTODO with the **same UID** distinct:
  `EventInstanceId { kind: Event, uid: "assignment-123@u-crawler" }` vs
  `EventInstanceId { kind: Todo, uid: "assignment-123@u-crawler" }`.
- `--json` `instance_id` renders `assignment-123@u-crawler` for the event and
  `VTODO:assignment-123@u-crawler` for the task.
- A VEVENT whose UID literally begins with `VTODO:` round-trips as an **event** in the domain
  (`kind: Event`); only the `known_event_ids` string form collides (C-11, F-9).

### 4.6 QA-MIXED — a calendar holding both kinds

Directory holding a VEVENT and a VTODO with the same UID, plus an undated completed task:

```
$ caldir events --to 2026-09-30 --undated
Tomorrow
    08:00 Healthy event [deadlines]
Tue Sep 1
    05:00 Assignment 123 window [work]
    13:00 [ ] Assignment 123 deadline [deadlines]
    13:00 [ ] Assignment 123 deadline [work]
Thu Sep 3
    13:00 [~] Weekly chore [work]
No date
          [x] Someday maybe [work]
```

- Both kinds appear, event-before-task at the same instant (R84).
- `[ ]` / `[~]` / `[x]` markers for needs-action / in-process / completed.
- `+ N undated task(s) (caldir events --undated)` footer without the flag; `No date` section with
  it; `--json` always carries the full set (Q-1 ruling).
- **`doctor` stays quiet** on the shared-UID pair — A-6 satisfied. It *does* warn correctly when
  two files really do carry the same task identity:
  `⚠ same task saved as multiple files; delete all but one:`.
- **A VEVENT+VTODO file in one calendar does not blank the agenda** — R5 level 3 confirmed: the
  broken file's calendar kept listing its other items, and every other calendar was unaffected.
- Cancelled tasks are hidden from the agenda exactly as cancelled events are, dated and undated
  alike.
- `--json` for tasks: `start: null`, `availability: null`, `end: null`, `due` populated,
  `task_status` populated, `status` projected onto the event value set
  (`cancelled` → `cancelled`, the other three → `confirmed`, per ADR-10 D6). `recurring` means
  "is an expanded instance" and is `false` for a recurring **master** of either kind — consistent
  with events, not a bug. `priority` and `created` have no key (B-2, owed).

### 4.7 Sync state coherence across both kinds (end-to-end, real RPC)

Remote holding a VEVENT and a VTODO sharing `assignment-123@u-crawler`, plus an undated task:

```
$ caldir pull
   + Assignment 123 window   Tue Sep 1, 05:00
   + Assignment 123 deadline Tue Sep 1, 13:00
   + Undated remote task     (no date)
Pulled: 3 created, 0 updated, 0 deleted
```

- **3 distinct base files** under `state/bases/`, one holding a `VEVENT` document and two holding
  `VTODO` documents, each a single-component `VCALENDAR` (C5 shape preserved).
- **3 known ids**: `VTODO:assignment-123@u-crawler`, `VTODO:undated-9@qa`,
  `assignment-123@u-crawler` — the event form unprefixed and byte-identical to the old format
  (C4a).
- **Second pull → `No changes to pull`; `status` → `No changes`** (A-2 idempotence).
- **A-3:** an undated `STATUS:COMPLETED` task with `COMPLETED:20200101…` survived every pull and
  was never proposed for deletion.
- **A-5:** removing one of two remote tasks produced exactly one incoming delete against the right
  file (`- Task one`, `1 deleted`), the event and the other task untouched.
- **R49:** removing the *last* remote task produced the withheld-deletion warning and removed
  nothing.
- **R48/C7:** an unreadable file in the calendar suppressed a genuine task delete (silently — F-6).
- **R42/C-7:** a remote change to `PERCENT-COMPLETE` alone, and to `PRIORITY` alone, each rendered
  a header line with **no rows** (documented under-description, B-1); a `STATUS` change rendered
  `status: needs_action → completed`.
- **`caldir discard --force`** on a locally edited task restored it from the base and left
  `status` clean.
- **`caldir rsvp <task>.ics accept`** → `… is a task, not an event; tasks cannot be RSVP'd`,
  **exit 1**, file MD5 unchanged (R81/T-42 satisfied). `caldir invites` correctly ignores tasks
  even when the task carries an `ATTENDEE` matching the configured account, and `--json` reports
  `rsvp: null` for tasks.
- **`caldir new`** has no `--due` and creates events only (A-5 / R90).

### 4.8 Other things checked that behaved correctly

- Task push aborts **before any RPC is serialized** — the provider log recorded no
  `CREATE_EVENT`/`UPDATE_EVENT` for the task, only `LIST_EVENTS`.
- Local files and the remote were byte-identical before and after every failed push — no partial
  writes, no corruption.
- Unknown TZID emits a real warning to stderr rather than failing:
  `warning: unknown TZID 'Not/AZone'; treating it as floating local time`.
- A `VTODO` with a `UID` containing a NUL byte, a `UID` containing `__`, and a bare `BEGIN:VTODO`
  with no `VCALENDAR` wrapper all load without panicking.

---

## 5. Note on the `icalendar` pin (informational, not a finding)

`caldir-core/Cargo.toml` declares `icalendar = "0.17.10"` (caret). The workspace `Cargo.lock`
resolves `0.17.10`, so the shipped binary matches every probe. But an **out-of-tree consumer** of
`caldir-core` resolves the newest 0.17.x — my first scratch crate silently picked up **0.17.13**.
Every load-bearing codec decision in the spec (`ends()` leaking `DTEND`, `CATEGORIES` escaping,
`priority()` clamping, VALARM UID regeneration) was measured on `=0.17.10`. Re-running the full
fixture set on both versions produced identical results here, so nothing in this report depends on
it — but if the crate's behaviour is genuinely a design input, a `=` pin would make that explicit.
