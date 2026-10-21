# VTODO reproduction (Stage 2, branch `vtodo-support`)

Status: RED tests committed to the working tree. **Zero production lines changed** — every
edit is inside a `#[cfg(test)] mod tests` block or in `caldir-core/src/test_utils.rs`
(itself `#[cfg(test)]`-gated at `caldir-core/src/lib.rs:11-12`).

## 1. Fixture

`caldir-core/src/test_utils.rs` — `VTODO_ICS` (CRLF line endings, RFC 5545 valid),
representative of the `deadlines/assignment-123.ics` half of `u_crawler` output:

```
BEGIN:VCALENDAR
VERSION:2.0
PRODID:-//u_crawler//EN
CALSCALE:GREGORIAN
BEGIN:VTODO
UID:assignment-123@u-crawler
DTSTAMP:20260801T090000Z
DUE:20260815T235900Z
SUMMARY:Assignment 3 - Recursive descent parser
DESCRIPTION:Submit the parser and a short report via the course portal.
URL:https://example.edu/courses/cs101/assignments/123
PRIORITY:1
STATUS:COMPLETED
COMPLETED:20260814T120000Z
END:VTODO
END:VCALENDAR
```

Control fixture `VEVENT_CONTROL_ICS` is the `windows/assignment-123.ics` half
(same course item as a VEVENT). `EMPTY_VCALENDAR_ICS` is a component-less VCALENDAR.

## 2. Exact call chain (verified, file:line as of this branch)

1. `Caldir::calendars()` — `caldir-core/src/caldir.rs:71-83`. Every non-hidden **directory**
   under `data_dir` becomes a `Calendar` (`caldir.rs:77`). No event file is read here.
2. `Calendar::events()` — `caldir-core/src/calendar.rs:105-119`; `read_dir` at `calendar.rs:109`,
   `.ics` filter at `calendar.rs:113`, and **`events.push(CalendarEvent::load(path)?)` at
   `calendar.rs:114`** — the `?` makes one bad file fail the entire listing.
3. `CalendarEvent::load(path)` — `caldir-core/src/calendar/event.rs:28-53`;
   `Event::from_ics_str` at `calendar/event.rs:37`, arity narrowing at `calendar/event.rs:40-50`,
   `CalendarEventError::ExpectedSingleEvent` raised at `calendar/event.rs:45-48`
   (message: `calendar/event/error.rs:10-11`).
4. `Event::from_ics_str` — `caldir-core/src/event.rs:96-102`:
   - `contents.parse::<icalendar::Calendar>()` (`event.rs:97-99`) — **succeeds** for a VTODO.
   - `icalendar.events().map(Event::try_from).collect()` (`event.rs:101`) — **yields nothing**.
5. Parallel single-string path: `Event::from_single_ics_str` — `event.rs:111-121`, raising
   `EventError::UnexpectedEventCount { expected: 1, found: 0 }` at `event.rs:116-119`
   (message: `event/error.rs:11-12`). Used by RPC wire decode (`rpc/ics.rs:34`) and
   sync bases (`calendar/state/event_bases.rs:34,60`).

## 3. Verified mechanism (confirmed, not assumed)

The suspected root cause is **confirmed**. Direct probe of the `icalendar` 0.17.10 API on the
fixture:

```
PROBE vtodo components=1 events=0 todos=1
PROBE vtodo from_ics_str len=Ok(0)
PROBE vtodo from_single=Some("expected 1 event(s) in ICS, found 0")
```

So: the ICS text parses cleanly — `icalendar::Calendar` holds **1 component**, and
`Calendar::todos()` sees it. The loss happens purely at `caldir-core/src/event.rs:101`,
where `icalendar::Calendar::events()` filters to `VEVENT` only, producing an empty `Vec`.
The arity check downstream then reports "found 0". It is **not** a parse failure, not a
`Event::try_from` conversion failure, and not an IO failure. `event.rs:101` is the only
`.events()` call site in the workspace; everything funnels through `Event::from_ics_str`.

## 4. Verified blast radius

Three distinct levels, all verified by test:

1. **The file itself** — `CalendarEvent::load` returns
   `ExpectedSingleEvent { path, found: 0 }`.
2. **The whole enclosing calendar** — because of the `?` at `caldir-core/src/calendar.rs:114`,
   a single VTODO file makes `Calendar::events()` return `Err`, so **its VEVENT siblings in the
   same directory become invisible too**. One unreadable file takes down the entire calendar
   listing. Verified by
   `calendar::tests::events_lists_a_vtodo_file_alongside_its_vevent_sibling`.
3. **Every other calendar, at the CLI level** — `AgendaView::collect`
   (`caldir-cli/src/output/agenda.rs:35`) iterates the resolved calendars and calls
   `calendar.expanded_events_in_range(from, to)?`. That `?` aborts the whole aggregation, so
   `caldir events` / `today` / `week` return an error and show **no events at all**, including
   the events of unrelated, perfectly healthy calendars. Verified by
   `caldir::tests::aggregating_all_calendars_survives_a_vtodo_calendar`.

### Separate calendars: `windows/` vs `deadlines/`

**Verified: they are two separate calendars.** `Caldir::calendars()` (`caldir.rs:71-83`) turns
every non-hidden subdirectory of `calendar_dir` into its own `Calendar`, and `Calendar::load`
touches no event file. So:

- At the **core** level the two are independent: with a VTODO in `deadlines/`, calling
  `windows.events()` directly still succeeds
  (`caldir::tests::vevent_calendar_lists_independently_of_a_sibling_vtodo_calendar`, green today).
- At the **CLI** level they are *not* independent: any command that aggregates across calendars
  short-circuits on the first failing calendar, so the VTODO in `deadlines/` **does** hide the
  VEVENTs in `windows/`.

Severity: a single VTODO file anywhere under the caldir blanks out the user's entire agenda,
not just one file and not just one calendar.

## 5. Tests added

All inline, following the repo convention (no `tests/` dirs, no `.ics` fixture files).

RED (acceptance; each asserts on the specific error variant / mechanism, not merely "an error"):

| Test | File |
| --- | --- |
| `event::tests::from_ics_str_parses_a_vtodo_document` | `caldir-core/src/event.rs` |
| `event::tests::from_single_ics_str_parses_a_vtodo_document` | `caldir-core/src/event.rs` |
| `calendar::event::tests::load_reads_a_vtodo_file` | `caldir-core/src/calendar/event.rs` |
| `calendar::tests::events_lists_a_vtodo_file_alongside_its_vevent_sibling` | `caldir-core/src/calendar.rs` |
| `caldir::tests::aggregating_all_calendars_survives_a_vtodo_calendar` | `caldir-core/src/caldir.rs` |

GREEN (controls — guard against a "fix" that breaks VEVENTs, and pin component-less behaviour):

| Test | File |
| --- | --- |
| `event::tests::from_single_ics_str_parses_the_vevent_control_document` | `caldir-core/src/event.rs` |
| `event::tests::from_ics_str_yields_nothing_for_a_component_less_document` | `caldir-core/src/event.rs` |
| `calendar::event::tests::load_reads_the_vevent_control_file` | `caldir-core/src/calendar/event.rs` |
| `calendar::tests::events_lists_the_vevent_control_file` | `caldir-core/src/calendar.rs` |
| `caldir::tests::vevent_calendar_lists_independently_of_a_sibling_vtodo_calendar` | `caldir-core/src/caldir.rs` |

### Commands

Just the five RED tests:

```bash
cargo test -p caldir-core --lib -- --exact \
  event::tests::from_ics_str_parses_a_vtodo_document \
  event::tests::from_single_ics_str_parses_a_vtodo_document \
  calendar::event::tests::load_reads_a_vtodo_file \
  calendar::tests::events_lists_a_vtodo_file_alongside_its_vevent_sibling \
  caldir::tests::aggregating_all_calendars_survives_a_vtodo_calendar
```

Everything VTODO-related (RED + controls):

```bash
cargo test -p caldir-core --lib -- vtodo vevent component_less
```

Full suite: `just test` (== `cargo test`); for complete counts use
`cargo test --workspace --no-fail-fast`.

## 6. Captured RED output

```
test event::tests::from_single_ics_str_parses_a_vtodo_document ... FAILED
test event::tests::from_ics_str_parses_a_vtodo_document ... FAILED
test calendar::event::tests::load_reads_a_vtodo_file ... FAILED
test calendar::tests::events_lists_a_vtodo_file_alongside_its_vevent_sibling ... FAILED
test caldir::tests::aggregating_all_calendars_survives_a_vtodo_calendar ... FAILED

failures:

---- event::tests::from_single_ics_str_parses_a_vtodo_document stdout ----
thread 'event::tests::from_single_ics_str_parses_a_vtodo_document' panicked at caldir-core/src/event.rs:1155:74:
VTODO rejected by arity check at caldir-core/src/event.rs:116 (expected 1, found 0)

---- event::tests::from_ics_str_parses_a_vtodo_document stdout ----
thread 'event::tests::from_ics_str_parses_a_vtodo_document' panicked at caldir-core/src/event.rs:1128:9:
assertion failed: `(left == right)`: VTODO document yielded 0 entries; icalendar::Calendar::events() (caldir-core/src/event.rs:101) filters to VEVENT only

Diff < left / right > :
<0
>1

---- calendar::event::tests::load_reads_a_vtodo_file stdout ----
thread 'calendar::event::tests::load_reads_a_vtodo_file' panicked at caldir-core/src/calendar/event.rs:550:75:
VTODO file rejected at caldir-core/src/calendar/event.rs:45 with found=0: icalendar::Calendar::events() yields no VEVENT for a VTODO document

---- calendar::tests::events_lists_a_vtodo_file_alongside_its_vevent_sibling stdout ----
thread 'calendar::tests::events_lists_a_vtodo_file_alongside_its_vevent_sibling' panicked at caldir-core/src/calendar.rs:1170:25:
one VTODO file aborted the whole calendar listing, including the VEVENT sibling: Event(ExpectedSingleEvent { path: "/tmp/.tmpBVkVHB/test-caldir/test-calendar/assignment-123.ics", found: 0 })

---- caldir::tests::aggregating_all_calendars_survives_a_vtodo_calendar stdout ----
thread 'caldir::tests::aggregating_all_calendars_survives_a_vtodo_calendar' panicked at caldir-core/src/caldir.rs:442:17:
aggregating calendar Some("deadlines") failed, so the whole agenda across every calendar fails: Event(ExpectedSingleEvent { path: "/tmp/.tmphGYNlg/test-caldir/deadlines/assignment-123.ics", found: 0 })

failures:
    caldir::tests::aggregating_all_calendars_survives_a_vtodo_calendar
    calendar::event::tests::load_reads_a_vtodo_file
    calendar::tests::events_lists_a_vtodo_file_alongside_its_vevent_sibling
    event::tests::from_ics_str_parses_a_vtodo_document
    event::tests::from_single_ics_str_parses_a_vtodo_document

test result: FAILED. 0 passed; 5 failed; 0 ignored; 0 measured; 467 filtered out; finished in 0.00s
```

## 7. Probes: mixed and component-less documents

**Both VEVENT and VTODO in one file** — observed:

```
PROBE mixed components=2 events=1 todos=1
PROBE mixed from_ics_str len=1 first_ok=true
PROBE mixed from_single=Ok(Some("Window"))
```

No error. `Event::from_ics_str` returns exactly the VEVENT and the VTODO is **silently
dropped**; `from_single_ics_str` and `CalendarEvent::load` both succeed and the file looks
healthy while half its content has disappeared. This is silent data loss, and it is worse than
the loud failure: a later `caldir push`/rewrite of that file would drop the VTODO on disk.
No acceptance test is committed for this case — the desired post-fix semantics (two entities
from one file vs. the single-event-per-file invariant) is a Stage 3 design decision.

**Zero components** (`BEGIN:VCALENDAR / VERSION / PRODID / END:VCALENDAR`) — observed:

```
PROBE empty components=0 events=0
PROBE empty from_ics_str len=Ok(0)
```

`from_single_ics_str` gives `UnexpectedEventCount { expected: 1, found: 0 }` and
`CalendarEvent::load` gives `ExpectedSingleEvent { found: 0 }` — the same error the VTODO case
produces, which is why the reported error message is indistinguishable between "unsupported
component" and "genuinely empty file". This behaviour is correct for a truly empty document and
is pinned green by `event::tests::from_ics_str_yields_nothing_for_a_component_less_document`.
`caldir-cli/src/commands/doctor.rs:75-91` already relies on it
(`DoctorWarning::UnreadableEvents`).

## 8. Baseline

| Run | Passed | Failed |
| --- | --- | --- |
| `cargo test --workspace --no-fail-fast` before (stashed) | 686 | 0 |
| `cargo test --workspace --no-fail-fast` after | 691 | 5 |

691 = 686 pre-existing + 5 new green controls. The 5 failures are exactly the new RED
acceptance tests. `cargo clippy --workspace --all-targets` is clean.
