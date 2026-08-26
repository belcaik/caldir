---
title: Commands
description: CLI command reference
order: 3
---

# Commands

## `caldir connect`

Connect to a [calendar provider](/providers) and fetch its calendars.

```bash
# Google Calendar (hosted OAuth via caldir.org)
caldir connect google
```

You can connect multiple accounts (e.g. personal and work) by running the command multiple times.

## `caldir status`

Show pending changes per calendar, similar to `git status`.

```bash
caldir status

# Show detailed diff
caldir status --verbose

# Status for a specific calendar
caldir status --calendar work
```

## `caldir pull`

Download remote changes to your local caldir directory.

```bash
# Pull events within ±1 year of today
caldir pull

# Pull all events since start
caldir pull --from start

# Pull only a specific calendar
caldir pull --calendar work
```

## `caldir push`

Upload local changes to the remote.

```bash
caldir push

# Push only a specific calendar
caldir push --calendar work
```

Note: if you delete a local `.ics` file and run `push`, the event is also deleted from the remote.


## `caldir sync`

Pull/push in a single command.
```bash
caldir sync
```

## `caldir new`

Create a new event in your local directory.

```bash
# Interactive mode (for humans)
caldir new

# Non-interactive mode (for agents):

# Timed event (defaults to 1 hour)
caldir new "Meeting with Alice" --start 2025-03-20T15:00

# With explicit duration
caldir new "Team standup" --start 2025-03-20T09:00 --duration 30m

# With a location
caldir new "Lunch" --start 2025-03-20T12:00 --location "Café Central"

# With a reminder
caldir new "Sprint planning" --start 2025-03-22T10:00 --reminder 10m

# In a specific calendar
caldir new "Sprint planning" --start 2025-03-22T10:00 --calendar work
```

- If neither `--end` nor `--duration` is specified, new events default to being 1 hour long.
- If `default_reminders` is set in your [global config](/configuration), those reminders are added to new events automatically.
- `caldir new` creates events only. There is no flag for creating a task — tasks arrive by pulling them, or by writing the `.ics` file yourself.

## `caldir events`

View upcoming events. Events that are invites show a colored status indicator: (pending), (accepted), (declined), or (tentative).

```bash
caldir events              # Next 3 days
caldir today               # Today's events
caldir week                # This week (through Sunday)
caldir events --from 2025-03-01 --to 2025-03-31  # Custom range

# Events from one calendar
caldir events --calendar work

# Also list undated tasks, instead of only counting them
caldir events --undated
```

Tasks (`VTODO`) are listed alongside events, anchored on their `DUE` date — falling back to `DTSTART` — with a status box in front of the title:

```
Fri Aug 14
    09:00 Standup [work]
      due [ ] File taxes [personal]
    17:00 [~] Ship crawler patch [work]

+ 2 undated tasks (caldir events --undated)
```

- `[ ]` not started, `[~]` in progress, `[x]` completed. Completed tasks are still shown, dimmed; overdue ones are red; cancelled tasks are hidden.
- A date-only `DUE` prints `due` in the time column rather than a clock time — a task is due *by* that day, it doesn't occupy it.
- A task with neither `DUE` nor `DTSTART` belongs to no day, so it is counted in a dimmed footer instead of being listed. `--undated` (on `events`, `today` and `week`) lists them in a trailing `No date` section instead.

caldir pulls tasks but cannot push them — see [Tasks](/#tasks).

<details>
<summary>Optional JSON output</summary>

Useful when building scripts, status bar widgets, etc.

```bash
caldir events --json
```

Returns:

```json
[
  {
    "instance_id": "b2f9@caldir__TZID=Europe/Stockholm:20260814T160000",
    "uid": "b2f9@caldir",
    "calendar": "personal",
    "title": "Friday retro",
    "summary": "Friday retro",
    "all_day": false,
    "start": "2026-08-14T16:00:00+02:00",
    "end": "2026-08-14T16:30:00+02:00",
    "tzid": "Europe/Stockholm",
    "location": "Conference room",
    "description": "Weekly retrospective",
    "status": "confirmed",
    "availability": "busy",
    "visibility": "private",
    "recurrence": null,
    "recurrence_id": "2026-08-14T16:00:00+02:00",
    "organizer": {
      "email": "host@example.com",
      "name": "Host Person"
    },
    "attendees": [
      {
        "email": "me@example.com",
        "name": "Me",
        "status": "accepted"
      }
    ],
    "reminders": [
      { "minutes_before_start": 10 }
    ],
    "url": "https://meet.google.com/abc-defg-hij",
    "attachments": [
      {
        "uri": "https://example.com/agenda.html",
        "params": [
          { "name": "FMTTYPE", "value": "text/html" }
        ]
      }
    ],
    "x_properties": [
      {
        "name": "X-GOOGLE-CONFERENCE",
        "value": "https://meet.google.com/abc-defg-hij",
        "params": []
      }
    ],
    "last_modified": "2026-08-13T10:11:12Z",
    "sequence": 2,
    "rsvp": "accepted",
    "recurring": true,
    "kind": "event",
    "due": null,
    "completed": null,
    "percent_complete": null,
    "task_status": null
  }
]
```

A task entry has the **same keys** — only the values differ:

```json
[
  {
    "instance_id": "VTODO:assignment-123@u-crawler",
    "uid": "assignment-123@u-crawler",
    "calendar": "personal",
    "title": "Ship crawler patch",
    "summary": "Ship crawler patch",
    "all_day": true,
    "start": null,
    "end": null,
    "tzid": null,
    "location": null,
    "description": null,
    "status": "confirmed",
    "availability": null,
    "visibility": null,
    "recurrence": null,
    "recurrence_id": null,
    "organizer": null,
    "attendees": [],
    "reminders": [],
    "url": null,
    "attachments": [],
    "x_properties": [],
    "last_modified": null,
    "sequence": 0,
    "rsvp": null,
    "recurring": false,
    "kind": "todo",
    "due": "2026-08-15",
    "completed": "2026-08-15T09:30:00Z",
    "percent_complete": 100,
    "task_status": "completed"
  }
]
```

- `start`/`end` are RFC 3339 timestamps with UTC offset. All-day events use date-only strings, where `end` follows the ICS exclusive-end convention (a one-day event on the 14th has `end` on the 15th).
- `rsvp` is only set when the event is an invite addressed to the calendar's account.
- `kind` is `"event"` or `"todo"`. It, along with `due`, `completed`, `percent_complete` and `task_status`, is present on **every** entry — the four task keys are `null` on an event.
- These five keys were *added*: no existing key was removed, renamed or retyped, and the pre-existing keys keep their order. If your script only wants the stream it got before tasks existed, one predicate gives it back: `select(.kind == "event")`. Dropping the five new keys from that selection (`del(.kind, .due, .completed, .percent_complete, .task_status)`) reproduces the old output byte for byte.
- `status` still uses the event value set (`confirmed`, `tentative`, `cancelled`). The real state of a task lives in `task_status`: `needs_action`, `in_process`, `completed` or `cancelled`.
- On a task, `due` is the anchor, `end` is always `null` (a `VTODO` has no end), and `start` and `availability` are `null` unless the task carries a `DTSTART`. `null` on `start`/`availability` never happens for `kind == "event"`.
- `completed` is encoded like every other timestamp: a UTC `COMPLETED` serializes as RFC 3339 UTC, and a floating one renders in the local zone rather than being given an invented offset.
- `--json` always includes undated tasks, while the text view only counts them unless you pass `--undated`. The two views deliberately disagree on how many items you see; the JSON is the complete set.
- A cancelled task is hidden from both views.

</details>

## `caldir invites`

List pending invites across all calendars (next 30 days). Shows organizer, file path, and current status for each invite.

```bash
caldir invites

# Include already-responded invites (not just pending)
caldir invites --all

# Filter to one calendar
caldir invites --calendar work
```

Tasks never appear in the invite list, even with `--all`.

## `caldir rsvp`

Respond to pending calendar invites. Updates the local ICS file (run `caldir push` afterward to sync your response).

```bash
# Interactive mode (for humans)
caldir rsvp

# Non-interactive mode (for agents)
caldir rsvp ~/caldir/work/2025-03-20T1500__standup.ics accept
caldir rsvp ~/caldir/work/2025-03-20T1500__standup.ics decline
caldir rsvp ~/caldir/work/2025-03-20T1500__standup.ics maybe

```

Tasks can't be RSVP'd: pointing `caldir rsvp` at a task file fails with an error and leaves the file untouched.

## `caldir discard`

Discard unpushed local changes, reverting to the remote state.

```bash
caldir discard

# Discard changes in one calendar
caldir discard --calendar work

# Skip confirmation prompt
caldir discard --force
```

## `caldir calendars`

List every calendar in the local caldir together with its stored name, color,
read-only setting, and provider.

```bash
caldir calendars

# As JSON
caldir calendars --json
```

This command reads local calendar configuration only. It does not contact
providers or inspect events.

## `caldir config`

Show configuration paths and calendar info.

```bash
caldir config

# As JSON
caldir config --json
```

## `caldir update`

Update caldir and all installed providers to the latest version.

```bash
caldir update
```
