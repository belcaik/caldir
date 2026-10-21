# QA — Stage 7, provider strategy

Independent verification. Nothing here was built by the author of this document.
Every claim below is backed by captured output from the real binaries
(`target/debug/caldir`, `target/debug/caldir-provider-caldav`,
`target/debug/caldir-provider-webcal`), a real Radicale 3.7.6 container, or a
cited `file:line`. No source file was modified.

Method: a scratch caldir (`XDG_CONFIG_HOME` + `calendar_dir` redirected, `env -i`)
driven by the shipped CLI against (a) a stub provider binary discovered on `PATH`
that speaks the real JSON-over-stdio protocol and logs every request, and
(b) a real Radicale I started myself on `127.0.0.1:5299`, seeded over `curl`,
with the real `caldir-provider-caldav` binary and a hand-written session file.

---

## 1. Stance table

| provider | claimed | verified | evidence |
|---|---|---|---|
| **caldav** | tasks PULL; push aborts by design (R70) | **TRUE, both halves** | Pull: real Radicale, `caldir pull` created `_task__2026-09-05T1700__dated-task.ics`, `_task__recurring__water-the-plants.ics`, `_task__undated-task.ics` alongside the VEVENT — §5. Push: `caldir-core/src/remote.rs:47,53,59` calls `CalendarItem::into_event()?` before every RPC; `caldir-core/src/item.rs:155` returns `EventError::UnexpectedEventCount{expected:1,found:0}` for `Todo`. Wire is structurally event-only: `pub event: Event` in all three of `caldir-core/src/rpc/methods/{create,update,delete}_event.rs:10` |
| **icloud** | shares the caldav code path; UNMEASURED | **TRUE — genuinely shares it, no mapping layer** | All five ops call straight into `caldir_provider_caldav::caldav::ops`: `list_events.rs:19` `ops::fetch_events`, `create_event.rs:19`, `update_event.rs:19`, `delete_event.rs:18`, `list_calendars.rs:19`, `connect.rs:33`. Grep for `ops::` in `caldir-provider-icloud/src` returns exactly those six lines and nothing else. `Cargo.toml` depends on `caldir-provider-caldav`. No iCloud-side ICS/property transform exists |
| **google** | UNSUPPORTED — different API, new OAuth scopes | **TRUE, and stronger than claimed** | `caldir-provider-google/src/commands/list_events.rs:15` returns `Result<Vec<Event>>` — a task cannot be represented at all, let alone dropped. Endpoints are `…/calendar/v3/calendars/{}/events…` only. Scopes at `commands/connect.rs:24-25` are `calendar.calendarlist.readonly` + `calendar.events`; neither reaches the Tasks API |
| **outlook** | UNSUPPORTED — same | **TRUE, and stronger than claimed** | `caldir-provider-outlook/src/commands/list_events.rs:17` returns `Result<Vec<Event>>`. Every Graph path is `/me/calendars/{}/events` or `/me/events/{}` (`commands/*.rs`). `SCOPES` at `commands/connect.rs:25` is exactly `"Calendars.ReadWrite User.Read offline_access"` |
| **webcal** | read-only by construction | **TRUE** | `caldir-provider-webcal/src/main.rs` implements only `connect` and `list_events` on `provider::Handler`; there is no create/update/delete arm to call. `commands/connect.rs:162` pins `read_only() == Some(true)`. Live check §6: a feed carrying a VEVENT **and** a VTODO pulled the event and silently dropped the task |

---

## 2. Push-abort behaviour, driven through the real code path

### 2.1 It aborts loudly, and loses nothing *of the item that failed*

Real `caldir push`, stub provider on `PATH`, one VEVENT and one VTODO in a writable calendar:

```
📅 work
   + QA control event   Tue Sep 1, 09:00
   + Hand in the assignment   Sat Sep 5, 17:00
   Remote error: expected 1 event(s) in ICS, found 0
```

Provider request log (every byte the provider received):

```
{"command":"list_events",...}
{"command":"create_event","params":{"event":"BEGIN:VCALENDAR...BEGIN:VEVENT...UID:qa-event-1@caldir...","hooli_calendar_id":"primary"}}
```

No `create_event` was ever issued for the task. `md5sum` of the task file before
and after the failed push: `816be0d78884a3c13466134ef2ea38e2` both times — byte
identical. `.caldir/state/known_event_ids` contains `qa-event-1@caldir` only;
`.caldir/state/bases/` holds one file, the event's. **No partial write, no file
mutated, no state recorded for the task.** Confirmed.

This is provider-independent: the abort is in `caldir-core`, before any transport
is chosen, so it is byte-identical on caldav, icloud, google and outlook. Webcal
never reaches it because `read_only = true` discards the outgoing set first.

### 2.2 …but it silently abandons everything queued behind it

`push_outgoing_changes` (`caldir-core/src/connection.rs:272`) is a plain
`for change in diff.outgoing()` with `?` — the first task in the set ends the
loop. The local half of that set is built by iterating
`std::fs::read_dir` (`caldir-core/src/calendar.rs:114`), **unsorted**, so which of
the user's changes survive is decided by filesystem ordering.

Measured against real Radicale. Six new local events plus one pending task edit.
`read_dir` order as the code sees it:

```
['new6.ics', 'new3.ics', '_task__recurring__water-the-plants.ics',
 '2026-09-01T0900__real-event-edited-locally.ics', 'new1.ics', 'new5.ics',
 '_task__undated-task.ics', '_task__2026-09-05T1700__dated-task.ics',
 'new4.ics', 'new2.ics']
```

`caldir push` printed:

```
📅 qa
   + (6 new events)
   ~ (1 changed event)
   Remote error: expected 1 event(s) in ICS, found 0
```

What actually reached the server (`PROPFIND Depth: 1`):

```
/testuser/qa/new1%40qa.ics
/testuser/qa/new3%40qa.ics
/testuser/qa/new5%40qa.ics
/testuser/qa/new6%40qa.ics
```

**`new2` and `new4` were never pushed and the user was never told which two.**
They sat after `_task__2026-09-05T1700__dated-task.ics` in directory order.

### 2.3 A remote delete queued behind a task never happens

Stub provider, stateful. Event synced, then deleted locally (the user wants it
gone from the server), with one task present:

```
📅 work
   + Hand in the assignment       Sat Sep 5, 17:00
   - QA control event             Tue Sep 1, 09:00
   Remote error: expected 1 event(s) in ICS, found 0
```

RPC log for that push: `list_events` only. Remote store afterwards still holds
`qa-event-1@caldir`. The event the user deleted stays on the server, on this run
and on every future run, and the CLI reports it as a pending delete each time.

### 2.4 Exit code and stream

```
$ caldir push  # with a task pending
...
   Remote error: expected 1 event(s) in ICS, found 0
EXIT=0
```

`caldir-cli/src/commands/push.rs:86` prints the error with `println!` (stdout,
not stderr) and `run` returns `Ok(())`. **A wedged calendar exits 0 with the
error on stdout** — no script or cron job can detect it.

Worse, `caldir-cli/src/commands/push.rs:85` only pushes the diff onto `applied`
on `Ok(())`, so the trailing `Pushed: N created, …` summary is skipped entirely
on a failed calendar. In §2.2 the run created **four** events on the server and
printed **no** `Pushed:` line at all. The user is told nothing was pushed while
four things were.

### 2.5 Pull is unaffected

Verified: with the push wedged, `caldir sync` still applied the incoming half
(`Pulled: 1 created, 0 updated, 0 deleted`). Only the outgoing direction is dead.

---

## 3. Over-claims found

**O-1 (material). "Nothing is deleted and no file is modified" is true but
answers the wrong question.** `website/src/content/docs/overview.md:52`,
`providers.md:52`, `providers.md:90`. What the user actually loses is every
*other* pending push in that calendar — creates, updates and deletes alike —
silently, with no list of what was skipped, and with which ones survive decided by
`read_dir` ordering (§2.2, §2.3). The sentence reads as reassurance and is the
only thing the docs say about the blast radius. It should say: *that calendar's
remaining pushes are abandoned; which ones is not deterministic.*

**O-2 (material). "your other calendars sync as usual" is stated; "the rest of
this calendar does not" is not.** Same three lines. The first half is true —
`push.rs:22` loops calendars independently and I confirmed a failure is per
calendar. The omission of the second half is the over-claim.

**O-3. "that calendar keeps failing until the task is moved somewhere else."**
`overview.md:52`. Moving the file is not the only exit and not the one a user
will reach for. `caldir discard --force` also clears it — by **deleting the
task**. Captured: a hand-authored `_task__hand-written.ics` and a locally
completed task, both in a CalDAV calendar:

```
📅 qa
   Local changes (to discard):
   + Precious hand written task   (no date)
   ~ Dated task                   Sat Sep 5, 17:00
     status: needs_action → completed
Discarded 4 changes: 3 created, 1 updated, 0 deleted
```

The file is gone and the completion is reverted. Consistent with event semantics,
but for a task there is *no* push path, so `discard` is the only built-in way to
clear the wedge and it is destructive. The docs never mention it.

**O-4. `providers.md:104` — "This is the one provider where tasks (`VTODO`)
sync."** "Sync" is the wrong verb for a one-directional pull, and the page
corrects itself only in the next sentence. The heading-level claim over-states.

**O-5 (under-claim, not over). `overview.md:50` "caldir can **pull** tasks from a
CalDAV server, but it cannot **push** them" plus "A task you create or edit
locally stays local".** "Stays local" is too gentle. A local task edit can never
win, so any later remote change to that task destroys it. Captured: local task
marked `STATUS:COMPLETED` (unpushable), server later touched the same task with a
newer `LAST-MODIFIED`; `caldir pull` overwrote it —

```
   ~ Hand in the assignment (renamed on server)
     summary: Hand in the assignment → Hand in the assignment (renamed on server)
     status:  completed → needs_action
Pulled: 0 created, 1 updated, 0 deleted
```

The user's completion is gone. Shown in the diff render, so not silent, but
unrecoverable and inevitable rather than a tiebreak. For tasks, the sync is not
last-write-wins; local always loses eventually.

**O-6 (omission). The R49 suppression belt is nowhere in the website docs.**
Live-verified: after deleting every VTODO from the Radicale collection,
`caldir pull` printed

```
📅 qa
   ⚠ task deletion withheld: the remote reported no tasks at all, which is
     indistinguishable from a provider that cannot list them. No task file was removed.
```

and kept all three local task files. Correct and deliberate
(`caldir-core/src/diff/calendar_diff.rs`, `caldir-cli/src/output/diff.rs:237`),
but a user who genuinely clears their task list on the server will keep the files
forever and see a warning they have no documentation for.

**Not over-claimed — checked and true:** the Google/Outlook "separate product,
separate API" wording (§1); "no file is modified" for the *failing item* (§2.1);
"feeds are read-only" and "a `VTODO` in a feed is ignored" (§6); the error string
`expected 1 event(s) in ICS, found 0` quoted verbatim in `overview.md:52` matches
the shipped text exactly; "verified against a real Radicale server"
(`providers.md:104`) — the tier genuinely runs, §7.

---

## 4. iCloud: UNMEASURED → MEASURED

Verified by grep and by reading, not by trusting the claim. `caldir-provider-icloud`
has no ICS layer, no property mapper and no query builder of its own: every
command is a credential lookup followed by a call into
`caldir_provider_caldav::caldav::ops`. The VTODO `calendar-query` iCloud would
send is byte-identical to the generic one —
`<C:comp-filter name="VTODO"/>` with no `<C:time-range>`
(`caldir-provider-caldav/src/caldav/ops/list_events.rs:79,111,125`).

So the code-sharing claim is true, and *only* the code-sharing claim is true.
Moving to MEASURED needs an Apple ID, an app-specific password and a Reminders
list, then re-running the six live gates against it. Concretely, the four
unknowns that Radicale cannot answer:

1. **Does iCloud honour a bare `VTODO` comp-filter with no time-range?** Radicale
   does. If iCloud instead returns nothing, or requires a time-range, every
   iCloud task read is empty — which R49's belt turns into a permanent
   "task deletion withheld" warning rather than data loss, but also into zero
   task support that looks like a bug.
2. **Is an undated / completed task returned at all?** Gate
   `an_undated_completed_task_survives_a_pull` measures this on Radicale only.
3. **Same-UID event/task pair** — does iCloud answer `409 <C:no-uid-conflict/>`,
   or silently `204`-overwrite one href with the other? `same_uid_pair`
   (`connection.rs`) refuses before any RPC, so caldir is safe either way, but
   the gate's premise is unverified there.
4. **Does iCloud inject non-`X-` properties onto a task?** Apple is known to add
   `X-APPLE-*`; anything non-`X-` would be dropped on caldir's first write.
   Gate `the_server_injects_no_non_x_properties_onto_a_task` is Radicale-only.

Mechanically this is cheap: point a new `live::icloud` module at the same six
assertions with the URL and credentials from a session file, gated on a second
env var. The blocker is an account, not code.

---

## 5. Live CalDAV pull against real Radicale (my own container, extra probes)

Beyond the shipped suite. I started my own Radicale 3.7.6, seeded it over `curl`,
and drove the real CLI + real `caldir-provider-caldav`.

Seeded (note the last one):

```
MKCALENDAR 201
ev1.ics 201     VEVENT
t1.ics 201      VTODO, dated
t2.ics 201      VTODO, undated
trec.ics 201    VTODO with RRULE:FREQ=WEEKLY
j1.ics 201      VJOURNAL
mixed.ics 400   one resource holding a VEVENT and a VTODO — Radicale refuses it
```

`mixed.ics 400` independently confirms the premise behind
`caldir-provider-caldav/src/caldav/ops.rs:66` — a server will not hold a
multi-component resource, so caldir never has to parse one.

`caldir pull`:

```
📅 qa
   + Real event          Tue Sep 1, 09:00
   + Dated task          Sat Sep 5, 17:00
   + Water the plants    Sat Aug 1, 09:00 🔁
   + Undated task        (no date)
Pulled: 4 created, 0 updated, 0 deleted
```

**Two things the shipped suite does not probe, both clean:**

- **A `VJOURNAL` sharing the collection is harmless.** The two comp-filters
  exclude it, so R57's "one undecodable element fails the whole listing"
  fail-safe is never armed by an ordinary journal entry. Worth knowing: a
  Radicale collection advertises `VTODO,VEVENT,VJOURNAL` by default, so this is
  the common case, not an edge case.
- **A recurring task master round-trips** into
  `_task__recurring__water-the-plants.ics` and renders with 🔁 — matching
  `specs/caldir.md:362` and backlog A-4 (master only, never expanded).

---

## 6. Webcal, live

Local ICS feed served over HTTP containing one VEVENT and one VTODO:

```
📅 feed
   + Feed event   Tue Sep 1, 09:00
Pulled: 1 created, 0 updated, 0 deleted
```

The task is dropped with no message on stdout or stderr. Docs claim
("a `VTODO` in a feed is ignored, and only events are pulled",
`providers.md:118`) is accurate. The in-code comment at
`caldir-provider-webcal/src/commands/list_events.rs:20-26` calls this "a silent
VTODO drop" that "the sync engine reads as a delete (R49)" — verified harmless in
practice, because a webcal calendar is `read_only` and a feed task can never
acquire a sync base, so the delete path is unreachable.

---

## 7. The live Radicale tier genuinely ran

```
$ CALDIR_LIVE_RADICALE=1 cargo test -p caldir-provider-caldav --lib -- live::radicale
running 6 tests
test live::radicale::pushing_a_same_uid_pair_into_one_collection_surfaces_an_error ... ok
test live::radicale::the_server_injects_no_non_x_properties_onto_a_task ... ok
test live::radicale::get_pass_two_equals_get_pass_one ... ok
test live::radicale::a_u_crawler_task_round_trips_through_radicale_preserving_identity_and_semantics ... ok
test live::radicale::an_undated_completed_task_survives_a_pull ... ok
test live::radicale::renaming_a_local_file_updates_the_same_resource_and_never_a_second_one ... ok

test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 37 filtered out; finished in 13.32s
```

Without the gate, the same six report `ok` in **0.00s** with an explicit skip
line each — so a green CI run proves nothing and says so:

```
SKIP live::radicale::get_pass_two_equals_get_pass_one: set CALDIR_LIVE_RADICALE=1
     with a working `docker` and `curl` to run the live tier
test result: ok. 6 passed; ... finished in 0.00s
```

Not taken on trust. I polled `docker ps -a` throughout a gated run:

```
caldir-live-radicale-3601222-1787704630156751923 tomsquest/docker-radicale:latest 127.0.0.1:38893->5232/tcp
caldir-live-radicale-3601222-1787704632562766715 tomsquest/docker-radicale:latest 127.0.0.1:46445->5232/tcp
caldir-live-radicale-3601222-1787704634868897629 tomsquest/docker-radicale:latest 127.0.0.1:33103->5232/tcp
caldir-live-radicale-3601222-1787704636610742998 tomsquest/docker-radicale:latest 127.0.0.1:37923->5232/tcp
caldir-live-radicale-3601222-1787704638968164074 tomsquest/docker-radicale:latest 127.0.0.1:38671->5232/tcp
caldir-live-radicale-3601222-1787704641904083867 tomsquest/docker-radicale:latest 127.0.0.1:42551->5232/tcp
```

Six real containers, one per test, each on its own ephemeral loopback port.
Afterwards: `docker ps -a | grep caldir-live` → nothing. **Teardown is clean.**
My own probe container `caldir-qa-providers-radicale` was removed on exit; the
only Radicale container still running belongs to a concurrent QA agent and I left
it alone.

---

## 8. Anything that loses data

Ordered by severity.

1. **Pending pushes silently abandoned behind a task (§2.2, §2.3).** Not a
   corrupted file — an intent that never lands and is never reported. A local
   delete of an event never reaches the server; new events never reach the
   server; which ones is `read_dir` order. Exit code 0, error on stdout, and the
   `Pushed:` summary suppressed even when items *did* push. Everything is
   recoverable by removing the task and pushing again — but only by a user who
   knows to.
2. **Any local task edit is eventually destroyed by the remote (§O-5).** Local
   can never win, so the first remote touch of that task overwrites it. Shown in
   the diff render; not recoverable.
3. **`caldir discard` deletes tasks (§O-3).** Correct by event semantics,
   undocumented, and the most likely reflex for "clear this stuck calendar".
4. **A genuine remote deletion of every task is never applied (§O-6).** By
   design, warned about, not data loss — the price of the R49 belt.

## 9. Single most serious finding

**One task file in a remote-backed calendar silently and permanently kills that
calendar's entire outgoing sync, and the user is never told what was lost.**
Six new events, four pushed, two abandoned, no list, exit 0, and no `Pushed:`
line at all. The abort itself is correct and does exactly what the specs say; the
failure is that it is a `?` inside a bare `for` loop over an unsorted `read_dir`,
so the blast radius is the rest of the calendar, chosen by inode order — and the
docs describe the blast radius as "nothing is deleted and no file is modified".
On CalDAV this is not an edge case: marking a pulled task complete is the single
most likely thing a user will ever do to a task, and it wedges the calendar.
