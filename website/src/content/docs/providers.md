---
title: Providers
description: Provider plugin architecture for Google, iCloud, Outlook, CalDAV, and Webcal
order: 4
---

# Providers

caldir uses a plugin architecture for calendar providers.

## Default providers

These providers come pre-installed with caldir-cli:

| Provider | Binary | Auth method |
|---|---|---|
| Google Calendar | `caldir-provider-google` | OAuth (hosted/self-hosted) |
| Outlook | `caldir-provider-outlook` | OAuth (hosted/self-hosted) |
| iCloud | `caldir-provider-icloud` | App-specific password |
| Generic CalDAV | `caldir-provider-caldav` | Username + password |
| Webcal (ICS feeds) | `caldir-provider-webcal` | None (public URLs) |

See [community providers](/ecosystem#community-providers) to connect caldir to other services like [Proton Calendar](https://proton.me/calendar) and [Tuta Calendar](https://tuta.com/calendar).

Any `caldir-provider-*` binary discoverable through your `$PATH` can be used. You can even [build your own](#plugin-architecture)!

## Google Calendar

### Hosted auth (easy)

```bash
caldir connect google
```

In this mode, OAuth is handled by the caldir.org server for simplicity. Your tokens are never stored or logged on the server (see [source code](https://github.com/t4t5/caldir/blob/main/website/functions/auth/google/start.ts) and [privacy policy](/privacy) for details).

### Self-hosted auth (harder)

If you prefer to use your own Google Cloud credentials instead of going through caldir.org, you can run:

```bash
caldir connect google --hosted=false
```

This will prompt you to create OAuth credentials in Google Cloud Console and set up the right
permissions.

### Google Calendar does not carry tasks

Google keeps tasks in a separate product (Google Tasks) with its own API. A Google Calendar remote is never reinterpreted as a task list, so `caldir push` and `caldir sync` hold a task change back and print a note saying it wasn't pushed. The events in that calendar push as usual, nothing is deleted, and no file is modified — the task simply stays local.

### Google Task lists sync separately

Running `caldir connect google` also discovers your Google Task lists, as a separate remote kind from your calendars — a task list is never conflated with a Calendar remote. Each task list supports full read and write.

Google Tasks is a lossy remote: several ICS fields caldir models are kept locally but never reach Google (`PRIORITY`, `URL`, `DTSTART`, `PERCENT-COMPLETE`, attendees, alarms, attachments, and unknown `X-` properties), a recurring task (`RRULE`) is refused outright rather than silently dropped, and a task's `DUE` datetime comes back as a date. The full field-by-field table is in [`docs/vtodo/provider-sync/providers/google-tasks.md`](https://github.com/t4t5/caldir/blob/main/docs/vtodo/provider-sync/providers/google-tasks.md).

If your account's session predates the Tasks scope, discovery still returns your calendars and prints a warning naming the account and the fix, e.g. `Google account you@gmail.com is not authorized for Google Tasks. Run caldir connect google to reauthorize and grant access.`

## iCloud

```bash
caldir connect icloud
```

This will prompt you to enter your Apple ID and an [app-specific password](https://support.apple.com/en-us/102654) (*not* your main Apple ID password).

### Tasks are pull-only, and untested

iCloud runs the same CalDAV code as the generic provider, so pulling Reminders may work — but caldir has never been tested against a real iCloud account for this, so it isn't a supported feature. Pushing tasks is refused outright on iCloud, unlike the generic CalDAV provider below.

## Outlook

### Hosted auth (easy)

```bash
caldir connect outlook
```

OAuth is handled via caldir.org, similar to the Google provider, with no setup needed.

### Self-hosted auth (harder)

If you prefer to use your own Azure AD app credentials:

```bash
caldir connect outlook --hosted=false
```

This will prompt you to register an app in the Azure portal and provide a client ID and secret.

### Tasks are not supported

Microsoft keeps tasks in a separate product with its own API, so caldir does not sync them here.

If a task (`VTODO`) file ends up inside an Outlook-backed calendar directory, `caldir push` and `caldir sync` hold it back and print a note saying the task was not pushed. The events in that calendar push as usual, nothing is deleted, and no file is modified — the task simply stays local.

## Other CalDAV server

Use this to connect to any other CalDAV-compatible server (Nextcloud, Radicale, Baikal...)

```bash
caldir connect caldav
```

You'll be prompted for a server URL, username, and password.

### Tasks are fully supported

Generic CalDAV is the one provider where tasks (`VTODO`) sync in both directions: discovery, create, update, and delete, verified against a real [Radicale](https://radicale.org) server. Support is advertised per remote and fails closed — a collection that can't serve tasks reports so, and caldir leaves its tasks alone rather than guessing.

## Webcal (public ICS feeds)

Subscribe to any public ICS calendar feed (`webcal://` or `https://` URLs).

```bash
caldir connect webcal
```

Webcal subscriptions are **read-only**: you can pull events, but `caldir push` won't modify the remote feed. No credentials are stored — the feed URL itself is the only configuration.

Tasks are not supported here: a `VTODO` in a feed is ignored, and only events are pulled.

Common uses: public holiday calendars, sports schedules, shared team calendars published as ICS feeds.

Example feed: [Public US holidays](https://calendar.google.com/calendar/ical/en.usa%23holiday%40group.v.calendar.google.com/public/basic.ics)

## Plugin architecture

Providers are discovered by looking for executables named `caldir-provider-{name}` in your PATH. This enables:

- **Permissionless ecosystem** — anyone can create a new provider
- **Language-agnostic** — providers can be written in any language
- **Independent versioning** — providers update separately from core
- **Smaller core binary** — provider-specific dependencies stay in provider crates

### Provider protocol

Providers communicate via JSON over stdin/stdout. The protocol is simple: the CLI sends `{command, params}` and the provider responds with a JSON result.

Commands:
- `connect` — authenticate with the provider (multi-step state machine)
- `list_calendars` — list all calendars for an account
- `list_events` — list events in a calendar within a time range
- `create_event` — create a new event or task
- `update_event` — update an existing event or task
- `delete_event` — delete an event or task
- `capabilities` — report what a remote supports (events, and whether it can read/write tasks)

Each provider manages its own state (credentials, tokens) in `~/.config/caldir/providers/{name}/`.
