---
title: Overview
description: Your calendar as a directory of plain text files
order: 0
---

# Your calendar as a directory

Caldir is a tool for storing your calendar as a directory of ICS files:

```
~/caldir/
├── google/
│   ├── 2026-06-25T0900__dentist.ics
│   ├── 2026-06-28__johns-birthday.ics
│   └── ...
└── outlook/
    ├── 2026-06-20T1500__client-call.ics
    ├── 2026-06-26T1400__sprint-planning.ics
    └── ...
```

It syncs with cloud providers (Google Calendar, iCloud, Outlook, CalDAV...) using git-like pull/push actions.

## Why?

Calendars today are often hidden behind APIs and proprietary sync layers, limiting what you can
do.

By turning them into plaintext files, you can use tools like `grep` to search them, or set up advanced workflows using scripts and agents.

It also makes it really easy to migrate your data from one provider to another.

## Tasks

A caldir directory can also hold tasks (`VTODO`), not just events. A task is a regular `.ics` file, named with a `_task__` prefix:

```
~/caldir/
└── work/
    ├── 2026-06-20T1500__client-call.ics
    ├── _task__2026-06-30__file-taxes.ics
    └── _task__someday-maybe.ics
```

Tasks show up next to your events in `caldir events`, `caldir today` and `caldir week`, and in their [`--json` output](/commands#caldir-events).

### Tasks sync one way only

caldir can **pull** tasks from a CalDAV server, but it cannot **push** them.

A task you create or edit locally stays local. If a task file sits in a calendar that has a remote, `caldir push` and `caldir sync` hold that change back and tell you so:

```
   ⚠ 1 task change not pushed: tasks are pull-only — caldir cannot push tasks to a remote yet.
```

The rest of that calendar pushes as usual, nothing is deleted, and no file is modified. The task keeps living in your directory, and the note comes back on every run for as long as it differs from what the server has.

Where tasks stand per provider:

| Provider | Tasks |
|---|---|
| Generic CalDAV | Pulled. Verified against a real Radicale server |
| iCloud | Untested — it runs the same CalDAV code, but caldir makes no promise here |
| Google Calendar | Not supported — Google Tasks is a separate product with its own API |
| Outlook | Not supported — Microsoft To Do is a separate product with its own API |
| Webcal | Not supported — feeds are read-only, and a task in a feed is ignored |

Two more things worth knowing:

- caldir never creates tasks for you: `caldir new` makes events only. Tasks arrive by pulling them, or by writing the `.ics` file yourself.
- Tasks keep the same properties events do, plus `DUE`, `COMPLETED`, `PERCENT-COMPLETE`, `PRIORITY` and the task `STATUS`. Anything caldir doesn't model — `CATEGORIES` and `RELATED-TO` among them — is dropped the first time caldir writes that file, on disk and on the server alike, exactly as it already is for events. The full list is in the [ICS format spec](https://github.com/t4t5/caldir/blob/main/specs/caldir.md).
