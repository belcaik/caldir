# Google Tasks: dependency spike

## Decision

`caldir-provider-google` talks to the Google Tasks API by hand-rolling a thin client over its
existing `reqwest` dependency, reusing `Session::access_token()` / `SessionStore::load_valid` for
auth exactly as `SessionStore::refresh_hosted` already does for the caldir.org refresh endpoint —
no new dependency is added.

## The Tasks surface caldir needs

Reference: [Google Tasks API — Tasks resource](https://developers.google.com/tasks/reference/rest/v1/tasks)
and [TaskLists resource](https://developers.google.com/tasks/reference/rest/v1/tasklists).

Endpoints Wave 4/5 need:
- `GET https://tasks.googleapis.com/tasks/v1/users/@me/lists` — task-list enumeration
  (`tasklists.list`).
- `GET https://tasks.googleapis.com/tasks/v1/lists/{tasklist}/tasks` — task listing for one list
  (`tasks.list`).
- `POST https://tasks.googleapis.com/tasks/v1/lists/{tasklist}/tasks` — create (`tasks.insert`).
- `PATCH https://tasks.googleapis.com/tasks/v1/lists/{tasklist}/tasks/{task}` — update
  (`tasks.patch`; `tasks.update` is the PUT sibling, `patch` is the natural fit for partial field
  writes).
- `DELETE https://tasks.googleapis.com/tasks/v1/lists/{tasklist}/tasks/{task}` — delete
  (`tasks.delete`).

`Task` resource fields (all `string` unless noted): `id`, `title`, `notes`, `status`, `due`,
`completed`, `updated`, `deleted` (`boolean`), `hidden` (`boolean`).

`TaskList` resource fields: `id` (string), `title` (string), `updated` (string, RFC 3339
timestamp).

OAuth scope strings, verbatim:
- `https://www.googleapis.com/auth/tasks` — create, edit, organize, delete tasks.
- `https://www.googleapis.com/auth/tasks.readonly` — view tasks only.

`due` field time component: per the Tasks resource reference, **"the due date only records date
information; the time portion of the timestamp is discarded when setting the due date"** and it is
not possible to read or write the time a task is due via the API — the wire type is an RFC 3339
timestamp string but only its date component is meaningful or preserved. This is the API behavior
§13's canonicalization policy (owned by `P5-05`) is built on; this document only records the fact.

## Candidates

### Candidate 0 — hand-rolled client over the existing `reqwest` dependency (baseline)

- **maintenance status** — n/a; this is code caldir would write and own, not a third-party
  release cycle to track.
- **auth integration** — direct. `Session::access_token()` (`caldir-provider-google/src/session/types.rs`)
  already yields a bearer token string; a Tasks call is `reqwest::Client::new().get(url).bearer_auth(token)`,
  the same shape `SessionStore::refresh_hosted` (`caldir-provider-google/src/session/store.rs`)
  already uses for a JSON POST against `HOSTED_REFRESH_URL`. `SessionStore::load_valid` remains the
  single place that guarantees a live token before any call — a hand-rolled client takes a token in
  and never manages refresh itself, so it can't fork the session state.
- **API coverage** — full and exact: every endpoint in §2 above is a plain HTTP verb + JSON body,
  matched one-to-one against hand-written `serde` structs for `Task`/`TaskList`.
- **dependency weight** — zero new crates. `reqwest` is already a direct dependency with
  `default-features = false, features = ["json", "rustls-tls"]` (`caldir-provider-google/Cargo.toml`);
  `serde`/`serde_json` are already direct dependencies too.
- **MSRV implications** — none; no new crate, so no new MSRV floor against the pinned `1.97.0`
  toolchain (`rust-toolchain.toml`).
- **boundary leakage** — none possible. The types are hand-written structs private to whatever
  module holds them; nothing forces them across a signature boundary any more than
  `google_calendar::types::Event` already is (see Containment below for the existing precedent).

### Candidate 1 — `google-tasks1` (google-apis-rs generated client)

- **maintenance status** — actively published (latest seen: `7.0.0+20251216`), tracks the current
  Tasks API surface as a generated client typically does. Not a staleness problem on its face.
- **auth integration** — poor fit. It is built against `yup-oauth2`'s `Authenticator` abstraction,
  which owns its own token acquisition/refresh/cache, not a bare bearer-token constructor. Handing
  it a raw access token means fighting its intended entry point; making it do real refresh means
  duplicating `SessionStore::refresh_hosted` / `refresh_local` inside a second, competing token
  store — exactly what D-6's notes warn against ("a candidate that wants to own refresh is a
  candidate that forks the session state").
- **API coverage** — full (it's generated from Google's discovery doc, so it covers list/insert/
  patch/update/delete and every resource field), but that completeness is moot given the auth
  mismatch above — partial usability, not partial API coverage.
- **dependency weight** — brings `hyper`, `hyper-rustls`, `hyper-util`, and `yup-oauth2` as a
  parallel HTTP/auth stack sitting beside `reqwest`'s `rustls-tls`. This is a second HTTP client
  configuration to build, feature-gate, and keep in sync with `reqwest`'s TLS choice, for zero
  functional gain over Candidate 0.
- **MSRV implications** — not stated by the crate; unverified against `1.97.0`, and irrelevant
  given the auth rejection below.
- **boundary leakage** — its generated `Task`/`TaskList` types would need to stay inside the
  provider the same way `google_calendar::types::Event` does today (see Containment), so leakage
  risk is equivalent to Candidate 0's, not worse — this axis does not distinguish the candidates.
- **Verdict**: rejected on auth integration and dependency weight; it would fork the session
  refresh path caldir already has "already solved and must not be forked" (ticket notes).

### Candidate 2 — a hypothetical Tasks equivalent of `google-calendar` (oxide-computer's
third-party-api-clients family)

- No such sibling crate exists in that family. `google-calendar-0.10.0`'s own `Cargo.toml.orig`
  (`repository = "https://github.com/oxidecomputer/third-party-api-clients/tree/main/google/calendar"`)
  shows it is generated per-API from that monorepo, and there is no published `google-tasks`
  counterpart on crates.io tracking the same generator. This candidate does not exist to evaluate;
  recorded here only because the ticket explicitly warns not to assume a sibling crate is
  equivalent — checked, and there isn't one.

## Recommendation

Candidate 0 stands: hand-roll the Tasks client over `reqwest`. It beats Candidate 1 on the two axes
that matter most for this codebase — auth integration and dependency weight — by a wide enough
margin that API coverage and maintenance status (where Candidate 1 looks fine) don't move the
needle. Day one for the next ticket (`P4-02`/`P4-04`): add the two OAuth scope strings from §2 to
`constants.rs`, and write a small module (e.g. `google_tasks/client.rs`) that takes a bearer token
string and exposes `list_task_lists`, `list_tasks`, `insert_task`, `patch_task`, `delete_task`,
each a `reqwest` call plus a hand-written `serde` struct — no `Cargo.toml` edit needed for the
client itself. (A crate is still needed later only if `P4-04`'s read path wants richer JSON
ergonomics than `serde_json::Value` plumbing; that's the one narrow case the ticket already
carves out as `P4-04`'s call, not this spike's.)

## Containment

The Tasks client (whatever module it lands in, e.g. `caldir-provider-google/src/google_tasks/`)
owns every Tasks-specific type: request/response structs, the `Task`/`TaskList` shapes, any
hand-rolled error type. The only types allowed to cross out of `caldir-provider-google` into
`caldir-core` or the shared RPC are `caldir_core::CalendarItem` / `Todo` / `CalendarConfig` and the
`rpc` request types — the same rule the existing Calendar path already follows: every
`provider::Handler` method in `caldir-provider-google/src/main.rs` (`create_event`, `update_event`,
`delete_event`, `list_events`, `list_calendars`) returns a core type, never `google_calendar::types::*`,
mapped at the boundary by `FromGoogle for Event` (`caldir-provider-google/src/google_event/from_google.rs`).
A hand-rolled Tasks type is held to the identical discipline: mapped to `Todo`/`CalendarItem` before
it ever reaches a `Handler` method's return type.

Grep that proves containment holds:

```sh
grep -rn 'google_tasks::\|GoogleTask' caldir-core/ caldir-provider-google/src/main.rs
```

Any hit inside `caldir-core/` is a regression by definition (D-6, spec §5, §9, `CROSS-2`). A hit in
`main.rs` is acceptable only if it appears strictly as a private `use`/module reference internal to
a `commands::*::handle` call whose *return type* is still a core type — the `Handler` trait method
signatures themselves must never name a Tasks type.

## Unsupported field policy

(Extended in place by `P5-06` — do not fill in here.)

## Session migration

`P4-02` adds the Tasks scope (`https://www.googleapis.com/auth/tasks`) to the consent request
alongside the two existing Calendar scopes. A session created before this change has no record of
Tasks access.

**What an existing user must do:** run `caldir connect google` again for the affected account. This
re-runs the OAuth consent flow and re-grants the Calendar scopes the session already had plus the
new Tasks scope; it does not lose or reset Calendar sync state.

**What they see if they do nothing:** Calendar sync (events) keeps working exactly as before —
nothing about the existing session is invalidated. The first attempt to read or write Tasks against
that session fails a scope guard (`Session::require_tasks_scope`, `caldir-provider-google/src/session/types.rs`)
before any Google API call is made, with a message naming the account and the exact fix:

> Google account `<email>` is not authorized for Google Tasks. Run `caldir connect google` to
> reauthorize and grant access.

It never surfaces as a bare Google 403 — the guard runs before the request, and an empty
`granted_scopes` (the shape of every pre-existing session file) is treated as "not granted", never
"unknown, assume yes".
