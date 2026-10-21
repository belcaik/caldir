# VTODO Support — Session Log

Context-friendly record of the whole effort. Read this first when resuming;
everything else in `docs/vtodo/` is detail behind it.

Branch: `vtodo-support` (70 commits off `main` @ `2ed74fa`)
Snapshot tag: `wip-snapshot` (pre-restructure tree)
Sibling branch: `pr1-graceful-degradation` (2 commits, 695/0, ships independently)

## 1. The problem

Real integration: `Canvas -> u_crawler -> ICS filesystem -> caldir -> Radicale -> CalDAV clients`.

u_crawler emits per course:

```
course/
  deadlines/assignment-123.ics   # VTODO
  windows/assignment-123.ics     # VEVENT
```

VEVENT worked. VTODO failed with `expected exactly one event in <file>, found 0`.
Verified cause: the domain model was `Event`-only, parsing read `icalendar.events()`
alone, and one `.ics` file was assumed to hold exactly one VEVENT.

Intent: real `VTODO local <-> domain <-> provider protocol <-> CalDAV <-> Radicale`,
preserving all VEVENT behaviour. **Not** transforming tasks into fake events.

## 2. Final state

```
TESTS:   968 passed / 0 failing   (UTC, Europe/Stockholm, America/New_York)
LIVE:    6/6 Radicale 3.7.6 gates (CALDIR_LIVE_RADICALE=1)
FMT:     clean
CLIPPY:  0 issues (-D warnings)
RUSTDOC: 0 internal refs in rendered output
```

Acceptance against the spec (`spec.md`, R1-R108 + C1-C13):

| | R1-R108 | C1-C13 | Total |
|---|---|---|---|
| PASS | 97 | 10 | **107** |
| PARTIAL | 9 | 3 | **12** |
| REFUSED | 2 | 0 | **2** |
| **FAIL** | 0 | 0 | **0** |

Reproducer, old binary vs new, same fixture:

```
main:    Error: expected exactly one event in .../deadlines/assignment-123.ics, found 0   EXIT=1
branch:  20:00 Assignment 3 - Recursive descent parser [windows]
         19:59 [x] Assignment 3 - Recursive descent parser [deadlines]                    EXIT=0
```

## 3. What shipped

- `CalendarItem` enum (`caldir-core/src/item.rs`) — the central abstraction.
  `parse_document` makes **one ordered pass** over `icalendar.components`; it never
  calls `events()` then `todos()`. `IGNORED_COMPONENTS = ["VTIMEZONE"]`.
- `Todo` domain type (`caldir-core/src/todo.rs` + `todo/{status,from_icalendar,to_icalendar,slugify}.rs`),
  `TodoStatus` = NeedsAction / InProcess / Completed / Cancelled.
- `EventInstanceId` gained `ComponentKind { Event, Todo }`, `#[default] Event`.
  `Display` is **byte-identical** for events; todos carry a `VTODO:` sigil.
- `CalendarListing { events, skipped }` + `is_incomplete()` (`calendar/listing.rs`).
- CLI: tasks in the agenda anchored on `DUE`, `[ ]`/`[~]`/`[x]` states, undated
  footer + `--undated`, five additive `--json` keys, task-aware diffs.
- `IncompleteCause { UnreadableFiles(n), Unlistable }` in the agenda output.
- CalDAV: VTODO queries issued **without** a time-range (RFC 4791 §9.9).

Observed CLI output:

```
Tomorrow
      due [x] Assignment 3 - Recursive descent parser [work]
    09:00 Standup [work]

+ 1 undated task (caldir events --undated)
```

## 4. Tasks are pull-only — and why

`withhold_task_pushes` (`caldir-core/src/diff/calendar_diff.rs`) removes task changes
from `outgoing` at diff construction, in **both** `compute_with` and the `#[cfg(test)]`
`from_changes`. The CLI then prints:

```
   ⚠ 1 task change not pushed: tasks are pull-only — caldir cannot push tasks to a remote yet.
```

This replaced an earlier whole-push refusal, which had a wedge: ticking one task off
stopped a user's **meetings** from syncing. Withholding keeps events flowing.

`CalendarItem::into_event` stays fallible on purpose — pushing a task as an event is
silent corruption, dropping it is silent data loss. It is a backstop, not a user path.

## 5. Version bumps

`caldir-core` 0.14.1→**0.15.0**, cli 0.12.1→0.13.0, caldav 0.2.1→0.3.0,
google 0.8.1→0.9.0, icloud 0.3.1→0.4.0, outlook 0.5.0→0.6.0, webcal 0.2.2→0.3.0.

A **seventh, undocumented pin** exists: `caldir-provider-icloud/Cargo.toml:21` pins
`caldir-provider-caldav`. This cross-provider pin class is missing from the `bump`
skill's table.

## 6. Method

Ten stages, executed under a strict context-rot policy: no subagent ever owned a task
broader than a narrow question or bounded change; anything spanning more than ~2-4
modules was split again; every agent wrote to disk or returned a compact report.

1. Knowledge base — 8 parallel explorers + synthesis (`context/`)
2. RED tests locking the bug (`reproduction.md`)
3. Domain design — 10 ADRs + skeptical grilling (`adr/`)
4. Spec — 912 lines, 108 requirements, 13 constraints, 10 scenarios, 59-row matrix
5. Ticket DAG — 49 tickets (`tickets/INDEX.md`)
6. Implementation factory — TDD RED→GREEN→REFACTOR→VERIFY per ticket
7. Provider strategy
8. End-to-end QA (`qa-*.md`)
9. Adversarial review by 5 fresh reviewers (`review-*.md`)
10. Ship report (`verification.md`, `PR-DESCRIPTION.md`)

`adr/ADJUDICATION.md` (~48KB) is the effort's operational memory: every binding ruling
and ~12 recorded corrections of the lead's own errors.

Verification discipline: mutation testing (~500 mutations applied). **27 locks were
found unable to fail and repaired.** 7 QA defects surfaced *after* 906 green tests.

## 7. Lead-side errors worth remembering

The recurring failure mode was the orchestrator asserting, not the agents implementing.

- **Shipping blocker, self-inflicted.** ADR-03's "unknown component is a hard error" was
  ratified as INVIOLABLE and generalised to *all* components without checking `main`.
  A legal `.ics` with a VEVENT beside a VFREEBUSY / VJOURNAL / top-level VALARM /
  `X-WR-CUSTOM` **lost its event entirely**, exit 0, no warning. Marking it INVIOLABLE
  stopped anyone weakening it *and* stopped anyone questioning it. Fixed via
  `IGNORED_COMPONENTS` / `load_with_ignored`.
- **Backwards reasoning on undated tasks.** "Always in-window is protective" was wrong:
  `in_range == false` means `continue` (no delete, no create); `in_range == true`
  **authorizes** deletion. ADR-08 corrected it; the correction was propagated downstream.
- **RELATED-TO "preserved verbatim"** — false. `Todo` has no `related_to` field and
  `R15_DROPPED_PROPERTIES` includes it. An agent refused the ticket rather than write it.
- **409 trigger = local rename** — false. The href is UID-derived (`client.rs:73-76`),
  so a rename yields the same href. The reachable trigger is a server-assigned filename.
  This had already been propagated into three ticket briefs.
- **Two arity literals conflated**: `expected 1 event(s) in ICS, found 0` (outbound) vs
  `expected 1 supported component(s) in ICS, found {n}` (inbound) — distinct locks,
  distinct paths.
- **A ruling assigned to a ticket by name without checking its scope** (`#[non_exhaustive]`
  to "L8-05", which was the acceptance gate). It floated unattached to executable work
  until an audit caught it.
- Blast radius, `as_event()` naming (renamed `event()`), "25 `cmd.event` sites"
  (actually 27 across 16 files), and ADR-01's "41 errors" justification (it measured
  migration friction, not architectural necessity — the real reason is that `SyncBases`
  is one map holding both kinds, and `Box<dyn Component>` loses the derived `PartialEq`
  that *is* the sync-direction test).

Operational: `git add -A` run twice while agents were mid-edit swept in-flight work into
wrong commits. Rule adopted — **commit after every ticket, stage by explicit path.**
Running L1 implementation concurrently with the PR-1 review contaminated the reviewed
tree; fixed by isolated-worktree extraction.

Environment: `just` unavailable (cargo equivalents used); toolchain 1.97.0 races rustup
under parallel cargo (serialized); a stale `target/package/caldir-core-0.15.0/` tree
shadows core sources in naive greps (`--exclude-dir=target` required).

## 8. Known gaps

- **R95 (A-1, INVIOLABLE) clause 2 — "caldir pushes it to a CalDAV server" — is
  unreachable.** Half the mandatory scenario is demonstrated live; the other half is
  deliberately not built.
- **R2/C13**: the second CalDAV REPORT propagates failure, so a server rejecting a bare
  `VTODO` comp-filter breaks a previously-working **event-only** sync. No test covers it.
- **iCloud UNMEASURED** — shares the CalDAV path, never tested against a real account.
- F-8: pre-existing stack overflow on ~1000 nested `BEGIN:` — reproduces identically with
  VEVENT on the baseline binary, so not a regression.
- 3 stale sites in `caldir-provider-caldav/src/live/radicale.rs` (97, 730, 1103) — test
  doc comments still quoting the removed abort.
- 483 ticket references remain in **test** doc comments. They never reach a shipped
  binary or docs.rs, but the maintainer's B1 condition forbids them anywhere.
- R107: Radicale image pinned by tag, not digest.

## 9. Open decisions for the maintainer

1. **The upstream review returned REJECT as a single PR** — not of the feature, of the
   shape. 32,000 lines for a read-only task viewer on one of five providers. It recommends
   a **7-PR split** led by graceful degradation, which already exists as
   `pr1-graceful-degradation` (695/0) and fixes a real bug independent of tasks.
   Under the stated Stage-10 gate ("no unresolved blocking findings"), this means the work
   is **not** to be declared ready without a decision here.
2. **Whether `docs/vtodo/` belongs in the repo at all.** ~30 planning documents; the
   reviewer spot-checked four line citations in `BACKLOG.md` and **all four were wrong**.

## 10. Review verdicts

| Review | Verdict |
|---|---|
| Architecture | sound as built — 4 majors, all fixed |
| Spec | 92/108, all 8 refusals sound |
| Test quality | "would catch a regression in every mechanism the effort was built around" (after HIGH-1 closed) |
| Backwards compat | found the shipping blocker — fixed |
| Upstream maintainer | **REJECT as a single PR**, 8 blocking objections, 7-PR split recommended |
