# First-class VTODO (task) support

**What you get:** tasks pulled from a CalDAV server now appear in your agenda alongside your
events, plus an `--undated` flag on `events`, `today` and `week`.

That is the whole user-facing feature. caldir **cannot push tasks** — a local task change is
withheld and reported, never sent. `caldir new` still creates only events. Google, Outlook and
webcal cannot carry a task at all. iCloud is **untested**.

---

## ⚠ Read this first: the maintainer asked for this to be split

The upstream maintainer review returned **REJECT as a single PR**. Not a rejection of the
feature — *"I want pull-only CalDAV tasks in caldir, and the core of this work … is the best
thinking anyone has contributed to this codebase"* — a rejection of the **shape**: 69 commits
and ~17,700 lines in one review, with at least six separable changes bundled in.

The recommended split, in landing order:

| # | PR | Content | Why separate |
|---|---|---|---|
| 1 | **Graceful degradation on unreadable `.ics`** | `Calendar::events()` → `CalendarListing`, `calendar/listing.rs`, the `Deletes::Suppressed` gate, `doctor` skipped-file warnings, the agenda `incomplete` footer | A pure resilience fix for VEVENT users — today one bad file blanks a calendar. The most valuable thing in the branch and it has nothing to do with tasks. Already written up separately in `docs/vtodo/PR1-DESCRIPTION.md`. |
| 2 | **`push` exit code and stderr** | `commands/push.rs`, `tests/push_exit_status.rs` | `push` exits 0 on failure today and prints the error to stdout. That is a bug affecting every scripted user; it deserves its own changelog line. |
| 3 | **CalDAV: strict resource decoding** | `ops.rs` `parse_event` → `parse_item`, `ops/list_events.rs` `decode_resources` | The riskiest hunk in the branch. A malformed VEVENT is silently skipped today; after this it aborts the listing — and an aborted listing is one belt away from a mass delete. Own review, own release note. |
| 4 | **CalDAV: 409 `no-uid-conflict` fallback** | `ops/create_event.rs`, the `wiremock` dev-dep, `test_dav.rs` | A create silently becoming an update is a remote-write behaviour change for VEVENTs. Argue it on its own merits. |
| 5 | **Public error API** | `lib.rs` re-exports, `#[non_exhaustive]`, `tests/public_error_api.rs` | API hygiene, breaking, and it needs the wildcard-vs-panic decision made deliberately rather than as a side effect. |
| 6 | **Pure moves and doc fixes** | `event/equality.rs`, `event/slugify.rs` extraction, `AGENTS.md` | Zero behaviour. Makes PR 7 ~200 lines smaller. |
| 7 | **VTODO support** | `item.rs`, `todo/**`, `ComponentKind`, the item-typed sync seams, the state-format sigil, agenda/JSON/diff task rendering, `--undated`, the Radicale live tier, `specs/caldir.md`, the website pages | The feature. |

Two conditions across all seven, both from the review:

- **Strip the private planning vocabulary from shipped code first.** Production code is now
  clean — zero `R##`/`L#-##`/`T-##`/`ADR-##` references outside `#[cfg(test)]` modules,
  verified by grep. **483 occurrences remain in test doc-comments**, and the reviewer's
  condition was "no PR of yours merges with `R70` or `L5-01` in a comment", full stop. Not done.
- **`docs/vtodo/` does not belong in the repo.** ~18,300 lines of one contributor's process,
  with four wrong line citations found in the first ten minutes of checking. The reasoning
  belongs in PR descriptions; what should stay is the `specs/caldir.md` VTODO section, the
  website pages, and a short user-facing limitations page distilled from the backlog.

This description covers the whole branch, so that a reviewer choosing to take the split has the
full picture in one place.

---

## Problem

caldir could not represent a `VTODO`. A task file in a calendar directory was not merely
unsupported — it was **actively destructive**:

- `Event::from_ics_str` used `icalendar.events()`, which yields nothing for a VTODO-only
  document, so a task file loaded as `expected exactly one event, found 0`.
- One unreadable file made `Calendar::events()` return `Err`, which blanked the **entire**
  agenda across **all** calendars. A hand-authored `.ics` with a typo — the exact workflow
  caldir's plaintext model invites — took down the whole view.
- Worse, a remote listing that came back short read as "everything else was deleted". A CalDAV
  collection holding both events and tasks was a partial-listing hazard.

## Use case

Radicale, Nextcloud and other generic CalDAV servers keep tasks and events in the same
collection. Today a caldir user pointed at such a collection cannot see their tasks and risks
their files being treated as deletes. This PR makes tasks first-class *on disk and on the way
in*: they parse, they get a stable filename, they carry identity across sync state, and they
render in the agenda.

## Before / after

| Situation | Before | After |
|---|---|---|
| A `.ics` holding a `VTODO` | `expected exactly one event …, found 0` | Loads as a task; renders in the agenda as `[ ] Water the plants` |
| A CalDAV collection with events **and** tasks | Only the events pulled; the tasks were invisible | Both pull; two REPORTs per sync, the VTODO one with no time-range per RFC 4791 §9.9 |
| A task with `DUE` but no `DTSTART` | n/a | Anchored on `DUE`; a date-only `DUE` prints `due` in the time column |
| A task with neither | n/a | Counted in a dimmed footer; `--undated` lists them in a trailing `No date` section |
| A local task change in a synced calendar | n/a | **Withheld** from the push, with `⚠ 2 task changes not pushed: tasks are pull-only`. The calendar's events keep syncing. |
| A remote listing that reports zero tasks | Would have deleted every local task file | Delete withheld, with a warning — indistinguishable from a provider that cannot list them |
| One unreadable `.ics` in a calendar | Whole agenda empty, exit 1, no explanation | Agenda renders; `⚠ N unreadable files in <calendar> (caldir doctor)`; `--json` names it on stderr |
| A legal `.ics` with a `VEVENT` beside an `X-` or `VJOURNAL` component | Sibling ignored | Sibling ignored **and reported by `doctor`** — same visibility, more information |

## Architecture

- **`CalendarItem`** — a two-variant enum, `Event` | `Todo`, and the crate's single parse entry
  point. One ordered pass over the document's components; `VTIMEZONE` allow-listed out of arity
  counting; unsupported siblings recorded and surfaced through `doctor`; a document whose
  components are *all* unsupported is a hard error naming them, because yielding nothing is what
  feeds the delete path.

  **Why an enum and not a widened `Event` or a trait.** `SyncBases` is **one** map per calendar
  directory that has to hold both kinds, and `CalendarDiff` uses structural `==` as its
  "has the content changed?" test. `Box<dyn Component>` loses the derived `PartialEq` that *is*
  the sync-direction test; a type parameter would have had to thread through `CalendarEvent`,
  `RemoteEvent`, `EventChange`, `CalendarDiff` and `SyncBases` and still meet an enum at the
  directory boundary. Storage is the constraint, not migration cost.

- **`Todo`** — a sibling struct, never a variant of `Event`. No `end`/`duration` field, because
  RFC 5545 §3.6.2 forbids `DTEND` on a VTODO and the field not existing is what makes emitting
  one impossible. `PartialEq` ignores `created`/`last_modified`/`sequence` and is locked by an
  exhaustive destructure with no `..`, so adding a field is a compile error until someone
  decides which side of equality it belongs on — `==` **is** the sync-direction predicate.

- **Kind-carrying identity** — `EventInstanceId { kind, uid, recurrence_id }`. Tasks take a
  `VTODO:` sigil in the string form; the event form is byte-identical to before, which is what
  keeps `known_event_ids` and `bases/<sha256>.ics` compatible in both directions.

- **One item-typed diff engine**, with two belts: an incomplete local listing emits zero
  deletes, and a remote response carrying no tasks at all never deletes task files.

- **A split RPC seam.** `Handler::list_events` returns `Vec<CalendarItem>`;
  `create/update/delete_event` stay `Event`-typed. That asymmetry is the defining cut of this
  PR and the reason it is pull-only. Retyping the outbound side would touch ~25 `cmd.event`
  sites across the provider crates.

- **`withhold_task_pushes`** — drops every non-`VEVENT` change from the outgoing set and returns
  the count, keyed on `ComponentKind` rather than the Rust type so a future component is
  withheld rather than smuggled onto an event-only wire. This replaced an earlier design that
  *refused the whole calendar's push*, which wedged the feature's own primary path; the
  maintainer review is what changed it.

- **Graceful degradation** — `Calendar::events()` returns a `CalendarListing { events, skipped }`
  instead of all-or-nothing. This is PR 1 in the split above.

## Provider implications

| Provider | Task pull | Task push | Notes |
|---|---|---|---|
| **caldav** | **Yes** | No | The only provider that pulls tasks. Verified against a real Radicale 3.7.6 container — six live gates, executed, not just read. |
| **icloud** | **Unknown — UNTESTED** | No | Shares the CalDAV code path verbatim (all five ops call straight into `caldir_provider_caldav::caldav::ops`; there is no iCloud-side ICS layer or query builder), so tasks *should* light up with zero icloud-crate edits. **That is a structural argument, not a measurement.** Four load-bearing unknowns remain: does iCloud 409 a same-UID pair, does it implement RFC 4791 §9.9, does it inject non-`X-` properties, and are Reminders collections returned by discovery at all. **Do not claim iCloud task support.** |
| **google** | No | No | Structurally VEVENT-bound: `list_events` returns `Result<Vec<Event>>`, endpoints are `/events` only, scopes do not reach the Tasks API. A task in a Google-backed calendar is withheld with a notice; the calendar's events keep syncing. |
| **outlook** | No | No | Same. |
| **webcal** | No (dropped) | N/A — read-only | A `VTODO` in a feed is dropped without a word, deliberately out of scope. The resulting "zero tasks" is covered by the delete-suppression belt, so it can never remove local task files. |

**Five provider `main.rs` adapter sites were edited** — caldav, google, icloud, outlook and
webcal (`git diff --stat main...HEAD -- 'caldir-provider-*/src/main.rs'` → 5 files, +19/−13),
plus `caldir-provider-webcal/src/commands/list_events.rs`. This PR does **not** claim zero
provider edits. `Handler::list_events` is a trait method and the only inbound channel, so any
return-type change forces every implementor; a zero-edit alternative (a defaulted `list_items`
delegating to `list_events`) was rejected because two overlapping methods where overriding the
wrong one silently drops data is a worse trap than five identical adapter lines. No provider
*logic* changed.

**These adapters have no test of their own in any provider crate** — `grep -rn process_request
caldir-provider-*/src/` returns nothing — and a dropped listing is a mass delete. Five
`process_request` round-trip tests are the recommended fix and are not in this PR.

## Tests

```
for Z in UTC Europe/Stockholm America/New_York; do TZ=$Z cargo test --workspace; done
→ 968 passed, 0 failed, 0 ignored — identical in all three zones

cargo fmt --all --check                                → exit 0
cargo clippy --workspace --all-targets -- -D warnings  → exit 0

CALDIR_LIVE_RADICALE=1 cargo test -p caldir-provider-caldav --lib -- \
  live::radicale --nocapture --test-threads=1
→ 6 passed, 0 failed (real Radicale 3.7.6 container, 13.5s)
```

Per crate: caldir-cli 180 · caldir-core 605 · caldir-provider-caldav 57 · google 49 ·
outlook 49 · icloud 14 · webcal 14. Baseline on `main` was 686.

**New dev-dependencies**, both in `caldir-provider-caldav` only, both dev-only, neither ships in
a binary:

- **`wiremock = "0.6"`** — an HTTP double for the CalDAV request/response gates, which assert on
  the exact bytes sent and so need a real server socket. Justified by a mutation: making
  `fetch_events` consume only the **first** of its two REPORTs passed the entire 812-test suite
  at the time — silent task loss with zero coverage.
- **`serial_test = "3"`** — serialises the live Radicale tier so its containers never race for a
  port. Already a `caldir-core` dependency.

`Cargo.lock` gains seven crates total (`wiremock`, `assert-json-diff`, `h2`, `deadpool`,
`deadpool-runtime`, `num_cpus`, `hermit-abi`), none in a shipped binary.

**The live tier additionally needs `docker` and `curl` on `PATH`** plus
`CALDIR_LIVE_RADICALE=1`. It is gated at *runtime*, not with `#[ignore]`, so
`cargo test --workspace` stays green on a machine with neither. Without the gate, five of the
six gates assert nothing (the sixth runs its request half unconditionally). The container is
referenced by tag, not digest — a known gap.

**What the suite does not cover**, stated because a green number is what fooled this project
once already: the five provider adapters; the staleness half of `bases_to_backfill` (two
surviving mutations); a CalDAV server that *rejects* the new VTODO REPORT; and iCloud, at any
tier.

## Compatibility

`caldir-core` **0.14.1 → 0.15.0 — breaking, with six dependants** (`caldir-cli` and all five
provider crates), each repinned to `version = "0.15.0"`.

| Item | `main` | this PR |
|---|---|---|
| `Calendar::events` | `Result<Vec<CalendarEvent>, _>` | `Result<CalendarListing, _>` |
| `CalendarEvent::event` | `&Event` | `Option<&Event>` |
| `RemoteEvent` | `RemoteEvent(Event)` | `RemoteEvent(CalendarItem)` |
| `EventChange::{Create,Update,Delete}` | carry `Event` | carry `CalendarItem` |
| `EventInstanceId` | tuple struct | named struct + `kind` |
| `CalendarDiff::compute` | — | `compute_from_listing` |
| `Event::from_ics_str` / `from_single_ics_str` / `load_single` | present | removed |
| `Handler::list_events` | `Vec<Event>` | `Vec<CalendarItem>` |

Additive: `CalendarItem`, `Todo`, `TodoStatus`, `ComponentKind`, `CalendarListing`,
`SkippedEvent`, `ExpandedItems`, and re-exported `CalendarError`, `CalendarEventError`,
`EventError`, `RemoteError`. Nothing was removed from `lib.rs`.

**What does not change for an existing, task-free user** — verified by building both binaries
and diffing outputs, not by reading tests: event filenames; `.ics` file bytes after a full sync
(`diff -r -I '^DTSTAMP'` identical); `known_event_ids`; `bases/<sha256>.ics` names and contents;
the RPC request stream; all seven text commands' stdout, stderr and exit codes; `--json` for
`config`/`calendars`/`invites`. The old↔new state interop matrix is byte-identical in every
direction. An **older** caldir meeting a task file fails loudly and safely — exit 1, zero RPC
calls, nothing deleted, `known_event_ids` untouched.

**The `--json` agenda migration is one predicate**, verified byte-for-byte in three timezones:

```
select(.kind == "event") | del(.kind, .due, .completed, .percent_complete, .task_status)
```

reproduces the old stream exactly. (Not "byte-identical" — adding five keys to every entry
necessarily changes every line's bytes. The predicate is the honest guarantee.)

**What does change on event-only paths:**

1. `events`/`today`/`week` with an unreadable `.ics`: exit 1 and an empty agenda → exit 0, a
   partial agenda and a `⚠` notice. Deliberate (that is PR 1), but it is a change.
2. `push` exit code 0 → 1 on failure, and the error moved stdout → stderr. `sync` and `discard`
   got the same treatment.
3. CalDAV and iCloud now issue a **second REPORT per `list_events`** — an extra round-trip per
   calendar per sync, and a new failure mode against any server that rejects a bare
   `<C:comp-filter name="VTODO"/>`. **No test covers a server that rejects it, and the question
   of whether to tolerate or fail is still open.**

**Pre-existing bugs fixed along the way** — why they are in a VTODO PR:

- **One unreadable `.ics` blanked the whole agenda across all calendars.** Found while making a
  task file loadable; a task file *was* an unreadable file to old caldir, so this had to be
  fixed before tasks could exist. It is the branch's most valuable change and is PR 1 in the
  split.
- **`caldir push` exited 0 on failure and printed the error to stdout.** Found by QA while
  measuring the task-push path; a scripted `set -e` sync never noticed a failed push.
  (`sync` and `discard` carried the identical shape and were fixed with it.)
- **CalDAV silently skipped malformed resources** (`find_map(Result::ok)`), which combines with
  "a short listing is a delete" into a mass-delete path. Had to change for tasks to decode
  safely; it is a VEVENT correctness fix too.
- **A CalDAV `create` returning 409 `no-uid-conflict` had no recovery path.** Surfaced by task
  round-tripping against Radicale; it affects events identically.
- **Four public error enums were unnameable or not `#[non_exhaustive]`.** Found while adding
  variants; adding a variant to a nameable public enum without the attribute is breaking every
  time, and this is the breaking window.

## Limitations

Stated plainly. These are the shipped behaviour, not bugs to be filed.

- **caldir pulls tasks from CalDAV and cannot push them.** A local task change — a completion,
  an edit, a delete, a file rename — is withheld from the push and reported per calendar. It
  never reaches a server. Because it can never win, a later remote change to the same task
  **overwrites it**: for tasks, sync is not last-write-wins, and local eventually loses. The
  only built-in way to clear a stuck local task change is `caldir discard`, which is
  destructive.
- **iCloud is untested.** See the provider table.
- **`caldir new` creates only events.** There is no `--due`.
- **Recurring tasks are master-only** — modelled and round-tripped, never expanded.
- **Properties caldir does not model are dropped on write**: `RELATED-TO` (so no subtasks or
  hierarchies), `CATEGORIES`, `CLASS`, `GEO`, `RESOURCES`, `COMMENT`, `CONTACT`. Only `X-*`
  properties pass through untouched. Because caldir cannot push a task, this is currently
  unreachable — it becomes reachable the day task push lands.
- **An unknown `STATUS` value collapses to `NEEDS-ACTION`** on write.
- **A remote that genuinely clears every task** cannot be distinguished from a provider that
  cannot list them, so the local files are kept and a warning prints — permanently, until
  something reports a task again.
- **A `VEVENT` whose UID literally begins with `VTODO:`** round-trips as a task. No encoding
  removes this, because UID is TEXT; it is the same class the codebase already accepts for `__`.
- **caldir emits no `VTIMEZONE`.** Servers that synthesize one (Radicale does) will store a
  structurally different document than caldir PUT. Harmless under property-level equality, fatal
  for any whole-document byte comparison.

Full detail, including the twelve requirements that are only partially met and the twelve
recommended follow-ups, is in `docs/vtodo/verification.md`.
