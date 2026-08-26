# Upstream maintainer review — `vtodo-support`

**Reviewer:** t4t5 (maintainer). **Base:** `main` (`2ed74fa`) → `HEAD` (`5a2f756`), 65 commits.
**Shape:** 90 code/test files, +15,325/−967. Plus `docs/vtodo/`, 81 files, +16,600.
**Suite:** `cargo test --workspace` green on my machine.

---

## Verdict: **REJECT as a single PR.**

Not a rejection of the feature. I want pull-only CalDAV tasks in caldir, and the core of
this work — `CalendarItem`, kind-carrying identity, item-typed sync seams, and the
byte-pinned proof that the on-disk state format is unchanged for events — is the best
thinking anyone has contributed to this codebase. I would merge that.

But I am not merging 32,000 lines in one go, and several of the eight objections below I
would not waive at any diff size. Split it as described at the bottom and most of it lands.

---

## Blocking objections

### B1. 686 references to a private planning vocabulary are compiled into the shipped crates

```
$ grep -rnoE "R[0-9]{1,3}\b"      --include=*.rs caldir-{core,cli}/src caldir-provider-caldav/src | wc -l   # 515
$ grep -rnoE "\bL[0-8]-[0-9]{2}\b"                                                                          # 48
$ grep -rnoE "\b[TQ]-[0-9]+\b"                                                                              # 82
$ grep -rnoE "ADR-[0-9]+"                                                                                   # 41
```

That is 686 occurrences on **524 lines across 45 source files**. The densest are
`diff/calendar_diff.rs` (64), `live/radicale.rs` (49), `caldav/ops.rs` (44), `todo.rs` (43),
`connection.rs` (38), `item.rs` (29).

**186 of these are in `///` doc comments in `caldir-core`**, which is published to
crates.io and rendered on docs.rs:

- `caldir-core/src/provider/handler.rs:35` — on the **public trait third-party providers
  implement**: *"item-typed so a provider can return a `VTODO` at all (R53). The outbound
  methods below stay `Event`-typed (R70)."* An external provider author cannot decode either.
- `caldir-core/src/rpc.rs:83` — `/// `T-33` — R52/C8 (INVIOLABLE). `Method` is a closed serde enum…`
- `caldir-core/src/remote/error.rs:8` — *"`L6-01` owns turning …"*: an unfinished-work
  pointer, in shipped rustdoc, aimed at a ticket that will not exist in the repo.
- `caldir-core/src/item.rs:144-152` — nine lines of rustdoc on `into_event` naming `ADR-07`,
  `R71`, `R19` and three test functions verbatim, ending *"Do not soften it without moving
  those."*
- `caldir-core/src/todo.rs:18-27` — the entire type-level doc on `pub struct Todo` is R7, R8,
  R25, R87, R88. Every field doc adds more.
- `caldir-core/src/rpc/ics.rs:6-24` — the module `//!` is nine lines of
  R54/R18/R102/R70/R71/R53/R19/R57 plus `L4-02`.
- `caldir-core/src/connection/error.rs:14,20` — public error variants documented as *"(R65)"*
  and *"(R62) — the error is the feature"*.

`R65`, `T-33`, `INVIOLABLE` are meaningless to every reader of this crate who is not the
author. They are also meaningless to *me* in six months. `caldir-provider-caldav/Cargo.toml`
does it too:

```toml
# HTTP double for the CalDAV request/response gates (ADJUDICATION: authorized
# on L5-01 M7 evidence). Dev-only; never ships in a binary.
wiremock = "0.6"
```

**Resolves:** every one of these rewritten to say the *reason*, not the *ticket*. The
reasons are almost always already in the sentence — delete the identifier and the comment
is fine. `(R65)` → nothing. `T-33 — R52/C8 (INVIOLABLE)` → `Adding a variant here breaks
every older provider.` This is a large but purely mechanical pass, and I will not merge
without it.

### B2. `docs/vtodo/` does not belong in this repo, and it is already stale

16,600 lines: 49 ticket files, 10 ADRs, an `ADJUDICATION.md`, a `GRILL-FINDINGS.md`,
`ORCHESTRATOR-NOTES.md`, three QA reports, a `PR1-DESCRIPTION.md`. This is one
contributor's process, not a project artifact. I have to either maintain it or watch it rot,
and it has *already rotted, before merge*:

| `BACKLOG.md` claim | What is actually there |
|---|---|
| C-3: "The abort happens at `connection.rs:232` with `expected 1 event(s) in ICS, found 0`" | `connection.rs:232` is `Ok(())` / a closing brace. The abort is `refuse_unpushable` at `connection.rs:285-310`, with an entirely different message. |
| C-1: "asserted at `connection.rs:764`" | `connection.rs:764` is an unrelated `CalendarDiff::from_changes` in a timeout test. |
| B-2: "`agenda_json.rs:221`, `:231`" for `priority`/`created` | `:221` is `organizer`, `:231` is `sequence`. |
| C-1: "25 `cmd.event` sites across five provider crates" | The PR description says 27 sites across four crates. The corpus disagrees with itself. |

Four wrong line citations found in the first ten minutes of checking. That is the whole
argument against merging line-number-pinned prose. It will be 100% wrong within two
releases and nobody will notice, because nothing verifies it.

Worse, B1 makes the corpus **load-bearing**: 686 code comments are pointers into it. So I
cannot drop it without orphaning them, and I cannot keep it without owning it. That
coupling is the actual defect.

**Resolves:** fix B1 first so the code stands alone, then keep out of the repo everything
that is not a durable statement about caldir. What I *will* take, as normal repo content:
the `specs/caldir.md` VTODO section (already in the diff, and good), the website pages, and
a short `docs/tasks-limitations.md` distilled from §C — the ~10 limitations a *user* can
hit, written without ticket IDs and without line numbers. That is maybe 150 lines. The
other 16,450 belong in the PR description or a gist.

### B3. The website documents an error string the branch's own tests assert users will not see

`website/src/content/docs/overview.md:52`:

> `caldir push` and `caldir sync` stop that one calendar with an error
> (`expected 1 event(s) in ICS, found 0`).

`caldir-core/src/connection.rs:911` and `:1323`:

```rust
assert!(
    !message.contains("expected 1 event(s) in ICS"),
```

The final commit (F-3) replaced that path with `ConnectionError::UnpushableTask`, and the
website was not updated. `website/src/content/docs/providers.md` repeats the same claim for
Google and Outlook. This is the first string a confused user will paste into a search box,
and it is wrong on the marketing site.

That it slipped is the symptom I actually care about: docs written against a design that
kept moving, with nothing tying them to the code.

**Resolves:** correct all three sites, and add a test that greps the website for the error
strings it quotes — or stop quoting error strings in the website.

### B4. Seven user-visible defects were found by external QA *after* the suite was green, and shipped in one unreviewed commit

`5a2f756`, the final commit, its own message:

> All were invisible to a 906-test green suite because each sat in a gap between what a
> test asserted and what a user would do.
>
> F-1: `expanded_events_in_range` never applied its window to tasks, so `--json` leaked
> tasks outside `--from/--to`.
> F-2: unparseable task properties were silently discarded…
> F-3: a task in the outgoing set silently abandoned every other pending change, in inode
> order, with exit 0 and the error on stdout.
> F-4: `PRIORITY:256/-1` silently dropped… Widened to `Option<i32>`.

F-1 is a wrong-output bug on the flagship command. F-3 is silent data-abandonment with a
zero exit code. F-4 is a public type change (`Option<u8>` → `Option<i32>`) made in the last
commit of a 65-commit branch.

The suite is enormous — `calendar_diff.rs` is 233 production lines under 1,554 test lines,
`connection.rs` is 347 under 1,370 — and it caught none of these, because the tests are
pinned to acceptance criteria (`/// AC 7:`, `M6`, `M12`, "byte-exact lock", 44 occurrences
of `pinned`/`INVIOLABLE`) rather than to what a user does. A suite that big which misses
that class is a maintenance liability twice over: it did not protect me, and every future
refactor has to drag 3,000 lines of byte-locked assertions along.

I am not asking for more tests. I am saying the QA pass found things the process was
structurally blind to, and the fixes for them went in without going back through the same
review the other 64 commits got. **Resolves:** F-1 through F-7 land as their own reviewed
commits with the rest of the branch behind them, not as a trailer.

### B5. `#[non_exhaustive]` and `unreachable!()` in the same feature, on the same enum

Four public enums gained `#[non_exhaustive]` (`CalendarError`, `ComponentKind`,
`CalendarItem`, `RemoteError`), justified as making a future `VJOURNAL` additive.

**It buys nothing.** `caldir-cli` and every provider depend on `caldir-core` **by path, in
this workspace** (`caldir-provider-caldav/Cargo.toml:22`). There is no third party that
matches on these enums today — `BACKLOG.md` Q-5, "Is `caldir-core` actually consumed
out-of-tree?", is still marked **OPEN, `UNVERIFIED`**. So the attribute protects a consumer
nobody has confirmed exists, and in exchange it deletes the compiler's exhaustiveness check
at every in-tree site, which is where the real consumers are. The branch's own comment at
`caldir-cli/src/output/agenda_json.rs:169` says the quiet part: *"caldir-cli depends on
caldir-core by path, so this cannot occur without a new variant being added in this same
tree."* Correct — which is an argument for letting the compiler find those sites, not for
hiding them.

The result is that the same enum is matched **exhaustively 12 times inside `caldir-core`
and with a wildcard 12 times outside it**, and the split falls exactly on the
`#[non_exhaustive]` boundary. Those 12 outside arms behave three different ways:

**Four panic in production code paths:**

- `caldir-cli/src/commands/doctor/warning/duplicate_file.rs:14` — `_ => unreachable!("caldir_core::ComponentKind grew a variant; …")`
- `caldir-cli/src/output/agenda_text.rs:139` — `_ => unreachable!("caldir_core::CalendarItem grew a variant; …")`
- `caldir-cli/src/output/agenda_json.rs:172` — `_ => unreachable!("… give it a `kind` discriminator and a field set here (ADR-10 D6)")`
- `caldir-provider-caldav/src/caldav/ops.rs:135` — `_ => unreachable!("… give it its iCalendar component name here")`

**Eight degrade silently, with no compiler help and wrong output:**

`agenda.rs:102` `_ => None` (unknown kind gets no anchor) · `agenda.rs:114` `_ => 2` ·
`agenda_text.rs:216` `_ => 2` · `agenda_json.rs:79` `_ => false` (`recurring` reported
false) · `agenda_json.rs:91` `_ => &UNTITLED` · `output/diff.rs:72` `_ => false` ·
`output/event.rs:26` `_ => true`.

`agenda_json.rs:96` even declares `static UNTITLED: Option<String> = None;` — a `static`
invented purely so an arm that "cannot be reached" has something to borrow.

**And one is inside `caldir-core` itself, where `#[non_exhaustive]` does not even apply:**

```rust
// caldir-core/src/calendar.rs:395-397
match calendar_event.into_item() {
    CalendarItem::Event(event) => events.push(event),
    item => tasks.push(item),
}
```

A future `VJOURNAL` is silently filed as a task, in the crate that defines the enum, twelve
lines above `calendar.rs:406-411` which matches both variants explicitly and correctly. A
binding wildcard defeats exhaustiveness even in-crate; this one is a straightforward bug
waiting for a variant.

So on the day someone adds `VJOURNAL`: the compiler points at zero of the thirteen sites,
the CLI panics in four places, prints quietly wrong output in seven, and core misfiles the
component as a task. `BACKLOG.md` B-13 already concedes that the pre-existing
`connection.rs:325` `unreachable!("apply_change returns None for Delete")` "holds **by luck
of ordering, not by assertion**".

**Resolves:** drop `#[non_exhaustive]` from `CalendarItem` and `ComponentKind`. That single
change deletes four `unreachable!()`s, turns eight silent fallthroughs into compile errors
at exactly the moment they matter, and costs nothing until Q-5 is answered yes. Fix
`calendar.rs:395` regardless. If you want to keep the attribute, then every wildcard becomes
a `Result` — never a panic, never a silent default.

### B6. The PR retypes the one layer that has no tests, and a failure there is a mass delete

`BACKLOG.md` B-12, verified by me:

> `grep -rn process_request caldir-provider-*/src/` finds nothing — **no provider crate has
> any test over its `Handler` impl's adapter layer**… **A dropped listing is a mass delete.**

This PR changes `Handler::list_events` from `Vec<Event>` to `Vec<CalendarItem>` and edits
that adapter in all five provider `main.rs` files — three of them gaining a real body
(`events.into_iter().map(CalendarItem::from).collect()`) where they previously had a
one-line passthrough. It is a small, mechanical change into an entirely untested layer whose
failure mode is caldir deleting the user's calendar, and the branch's own author flagged it
and deferred it.

Two `process_request` round-trip tests were added — but in `caldir-core`
(`provider/handler.rs:206,224`), against a local `ListingHandler` test double, not against
any real provider's adapter. That covers the wire codec, which was not the risk.

**Resolves:** one `process_request` round-trip test per provider crate, five tests, asserting
a non-empty listing survives the adapter. `BACKLOG.md` B-12 costs it at exactly that. Given
what it guards, it is not optional in the PR that touches it.

### B7. At least six separable PRs are bundled in

See "Splitting" below. This alone is why the answer is no.

### B8. The push refusal wedges the one path this feature exists to serve

See the dedicated section below.

---

## The pull/push asymmetry — my honest read

**Pull-only is a legitimate MVP. This particular implementation of it is not.**

I have no objection to shipping without task push. Protecting 27 `cmd.event` sites across
four provider crates is the right call, and `Handler::list_events` retyped to
`Vec<CalendarItem>` while the outbound methods stay `Event` is a clean, defensible cut. The
refusal itself is well built: `refuse_unpushable` (`caldir-core/src/connection.rs:285-310`)
runs before the first RPC, keyed on kind rather than Rust type, so a future component is
stopped rather than smuggled; the error message
(`caldir-core/src/connection/error.rs:39-48`) is the best error text in this codebase. F-3
converting a mid-loop abort into a whole-push pre-flight was exactly right.

My objection is **where the refusal lands**, and it makes the feature worse than useless on
its own primary path:

1. CalDAV is the only provider that pulls tasks. So the target user connects Radicale or
   Nextcloud and pulls a calendar containing events *and* tasks into one directory.
2. They tick a task off — or delete it, or rename the file. It becomes an outgoing change.
3. `caldir push` and `caldir sync` now **refuse that entire calendar**. Not the task: the
   calendar. Their meetings stop syncing.
4. It never recovers on its own. The error tells them to move the task to a local-only
   calendar — i.e. to undo the pull that is the entire feature.

The website states this plainly, to its credit ("that calendar keeps failing until the task
is moved somewhere else"), which tells me the author saw it and shipped it anyway.
`BACKLOG.md` C-3 calls it "Ugly, NOT data loss". I agree it is not data loss. I disagree
that ugly-but-not-data-loss is the bar, when the failure mode is *the default outcome of
using the feature as documented*.

There is a smaller and strictly better design available, and it is why I am pushing back
rather than just accepting the limitation:

> Tasks are not outgoing at all. Drop them from `CalendarDiff::outgoing` the way read-only
> calendars already drop everything, and print a visible per-calendar notice —
> `2 task(s) not pushed (tasks are pull-only)` — following the exact `suppressed_task_deletes`
> precedent this branch already built. Events keep syncing. Nothing wedges. The user's local
> task edits stay local, which is what "pull-only" means to a user.

That is less code than `refuse_unpushable`, needs no `SameUidPair` pre-flight for the task
case, and turns C-3 and C-5 from limitations into non-events. Then pull-only is a real MVP
and I would ship it without argument.

Two smaller things in the same area:

- `BACKLOG.md` C-5 — "caldir's first genuine edit **destroys correct server data**" for
  `CATEGORIES`, `RELATED-TO`, `GEO`, `RESOURCES`, `COMMENT`, `CONTACT`, `CLASS` — is filed
  under "**True of the shipped feature. A user can hit every one of these.**" It is not
  reachable, because task push is refused. Either the section heading is wrong or C-5 is.
  Under my suggested design it stays unreachable permanently, which is a point in its
  favour.
- `caldir new` is deliberately event-only (`caldir-cli/src/commands/new.rs:107`, R90). With
  push refused too, the total user-facing feature is: *tasks pulled from a CalDAV server are
  displayed in the agenda, plus one `--undated` flag on three commands.* That is a
  reasonable v1. It is not 32,000 lines' worth of reasonable v1, and the PR description
  should lead with that sentence rather than with the architecture.

---

## Non-blocking — I would ask for these

**N1. Naming — the type names are now actively false.** `BACKLOG.md` A-9 declares renaming
the `Event*` types a non-goal. Fair scope call for *this* PR; the consequence is that the
domain vocabulary is permanently wrong in a codebase whose `AGENTS.md` has a section called
"The data model in one paragraph":

- `CalendarEvent` (`caldir-core/src/calendar/event.rs:12`) now stores a `CalendarItem` and
  routinely holds a `Todo`. Its own `load` doc says *"Load whichever supported item the file
  holds"*. `CalendarFile` or `StoredItem` would be honest. This infects
  `CalendarListing::events()` / `into_events()` (`calendar/listing.rs:46,50`), which return
  task-carrying values, `SkippedEvent` (`:9`), which can be a skipped VTODO, and
  `push_event` (`:38`).
- `RemoteEvent(CalendarItem)` (`remote/event.rs:12`) — same.
- `Handler::list_events` returns `Vec<CalendarItem>`; `EventChange` carries items;
  `insert_event_base(id, item: impl Into<CalendarItem>)`
  (`calendar/state/sync_bases.rs:34`) renamed the parameter but not the method.
- `CalendarItem` vs `ComponentKind` are two names for one concept, in the same PR. RFC 5545
  already supplies the word, and this branch used it for the tag enum. Pick one pair:
  `Component`/`ComponentKind`, or `Item`/`ItemKind`.
- `Todo` in core, "task" in the CLI (`DuplicateTaskFiles`, `format_task_line`, the `_task__`
  slug prefix) — and in core's own doc comments. Two vocabularies, one concept.
- `CalendarItem::as_event` (`item.rs:108`) is `#[cfg(test)]`-only *and* misuses the `as_*`
  convention for a fallible narrowing; `CalendarEvent::event()` already does the job with a
  better name. `EventInstanceId::new` (`instance_id.rs:34`) silently means "Event kind" while
  its sibling `::todo` names its variant.
- `ExpandedItems` (`calendar.rs:388`) and `CalendarListing` are two types for the same
  `(items, skipped)` shape.

I will take these now; I want a follow-up issue for the rename in the same breaking window
as whatever comes next, and I want it *before* a third component kind lands, not after.

**N1b. `into_event` lies in the error type.** `CalendarItem::into_event`
(`caldir-core/src/item.rs:155`) returns `EventError::UnexpectedEventCount { expected: 1,
found: 0 }` to mean "this is a task", and its nine-line doc comment (`:144-152`) concedes
*"Ugly, and deliberately so"* while naming three test functions verbatim as the reason not
to change it. A dedicated `EventError::NotAnEvent` variant costs one line, removes the doc
comment, and would have prevented B3 — the website quotes that arity message *as the thing
users see when they push a task*.

**N2. Test seams shaping production code.** Three functions were extracted from `run()`
bodies with comments saying the reason is testability:

- `caldir-cli/src/commands/rsvp.rs:47` — *"Split out of `run_direct` so the narrowing, the
  write and the failure paths are reachable from a test without a `Caldir` (which has no
  constructor outside `caldir-core`)."*
- `caldir-cli/src/commands/new.rs:107` — `assemble_event`, extracted verbatim so
  `new_still_creates_only_events` can call it
- `caldir-cli/src/commands/invites.rs:81` — `collect_invites`, extracted verbatim, plus
  ~230 lines of test scaffolding

The root cause is named in `BACKLOG.md` B-5: `Caldir::new` is `#[cfg(test)] pub(crate)` and
`CalendarDiff::compute` is `#[cfg(test)]`, so downstream crates cannot construct either.
Fix the constructors — two attributes — rather than reshaping five call sites around the
gap. The branch is already taking a breaking release; this is the window.

Related, and worth a second look: `#[cfg(test)]` items now sit inside production types —
`CalendarItem::as_event` (`item.rs:105`), `Event::parse_single_ics` (`event.rs:211`,
panicking), `SyncBases::iter` (`sync_bases.rs:30`), and
`known_event_ids.rs:54`, which is `#[cfg(test)] **pub**` rather than `pub(crate)` and is
saved from the public API only by the module not being re-exported. Also: ten new public
exports in one release (`CalendarListing`, `ExpandedItems`, `SkippedEvent`, `ComponentKind`,
`CalendarItem`, `Todo`, `TodoStatus`, plus the three error re-exports). `ExpandedItems` has
one producer and `SkippedEvent::new` is already `pub(crate)` — do those need to be public at
all? Every one of them is mine to keep compiling forever.

**N3. Tautological tests.** `new_still_creates_only_events` asserts that a function whose
body is `Event::new(...)` returns an event.
`the_undated_flag_exists_on_all_three_agenda_commands_and_defaults_off` asserts clap
registered a `bool`. Both carry `/// AC 7:`-style docstrings. These test the ticket, not the
code, and they are the visible edge of B4.

**N4. `ensure_single_component` is unreachable by its own admission.**
`caldir-provider-caldav/src/caldav/ops.rs:53,75` guards both PUT paths, and its doc comment
says *"`to_ics_string` cannot emit that shape (R27), so **this never fires in production**"*.
Delete it, or ship it in the CalDAV hardening PR where it can be argued on its merits.

**N5. `duplicate_file_sets` ordering.** B-7 admits the sort makes the mutant only ~50%
catchable because `HashMap` iteration is randomized. `BTreeMap` needs `Ord` on
`EventInstanceId` — a public type — so it rides the breaking window or never happens.

**N6. `.gitignore` gained `.claude/worktrees/`** (`b89bf31`). Contributor tooling; belongs
in a personal global ignore.

---

## Things I checked and am happy with

**Dependencies: accepted.** `wiremock` and `serial_test`, both dev-only in
`caldir-provider-caldav`. `Cargo.lock` gains seven crates total (`wiremock`,
`assert-json-diff`, `h2`, `deadpool`, `deadpool-runtime`, `num_cpus`, `hermit-abi`), none
shipped in a binary. `serial_test` was already in the tree. Fine. Strip the ticket IDs from
the comments justifying them (B1).

**Live test tier: accepted, and well done.** `caldir-provider-caldav/src/live/radicale.rs`
is gated at *runtime* on `CALDIR_LIVE_RADICALE=1` rather than `#[ignore]`, so
`cargo test --workspace` stays green on a machine with no Docker — verified, my run passed.
`Drop` runs `docker rm -f` on success, failure and panic; container names are unique per
test; startup is health-polled, not slept. The two forbidden assertions (no raw-byte PUT/GET
comparison because Radicale re-sorts and re-folds; no ETag as proof of a no-op because
`DTSTAMP` re-stamps) are the kind of thing a contributor usually learns by shipping a flaky
test, and it is written down. I would not have asked for this tier, but I am glad to have it.

**The breaking release: justified, and I want more in it.** `caldir-core` 0.14.1 → 0.15.0
with six dependants is unavoidable —`CalendarEvent::event()` now returns `Option<&Event>`,
`Event::from_ics_str` is gone, `Handler::list_events` is retyped. The five provider `main.rs`
adapters are three-line mechanical changes. Since the window is open anyway, B-5 (test
constructors) and B-7 (`Ord` on `EventInstanceId`) should ride it rather than cost the same
again later. Note that `1c573b4` had to re-export `CalendarError`, `CalendarEventError` and
`RemoteError` because two of the four `#[non_exhaustive]` attributes were **inert on
unnameable types** — a good catch, and `caldir-core/tests/public_error_api.rs` proving it
from a genuine separate crate is the right test.

**`Todo` duplicating `Event`: I accept it, with eyes open.** Exactly 16 of `Event`'s 19
fields recur on `Todo` (`uid summary description location start status url recurrence
recurrence_id organizer attendees reminders attachments x_properties last_modified
sequence`); `status` differs in type (`Status` vs `TodoStatus`), so 15 are genuinely
identical. `Event`-only: `end availability visibility`. `Todo`-only: `due completed
percent_complete priority created`. There is no shared trait — the seam is match arms in
`CalendarItem`, plus a duplicated codec (`todo/from_icalendar.rs` 543 lines,
`todo/to_icalendar.rs` 316) mirroring the event one.

The duplication is not only in the fields. `to_ics_string` (`event.rs:101` vs
`todo.rs:150`) and `splice_valarms_into_vevent`/`_vtodo` (`event.rs:222` vs `todo.rs:161`)
are verbatim copies differing in two tokens and one string literal; ~14 of ~18 property-emit
blocks in the two `to_icalendar` modules are structurally identical, and six blocks in the
two `from_icalendar` modules are near-verbatim. `kind_ordinal` is hand-copied across crates
(`agenda.rs:109` and `agenda_text.rs:211`, the latter's comment reading "Mirrors
`agenda.rs`").

The sharpest instance, and the one I would fix before merge:

```rust
// caldir-core/src/todo/from_icalendar.rs:140-144
// event rule so cross-provider comparison stays stable; deliberately a copy
// rather than a shared helper, because R2 forbids touching the event codec.
fn non_empty(s: Option<&str>) -> Option<String> {
    s.filter(|v| !v.is_empty()).map(ToString::to_string)
}
```

Byte-identical to `event/from_icalendar.rs:107-109`, justified by citing a requirement
number. Adding `pub(crate) fn non_empty` to `crate::event` and importing it is not "touching
the event codec" in any sense I would recognise — this same branch does exactly that for
`summary_slug`, `time_slug`, `attachments_eq` and `x_properties_eq` (`event/equality.rs`,
imported at `todo.rs:11-12`). This is B1 doing real damage: a ticket ID standing in for an
argument, and winning.

They will drift, and I will be the one noticing. But the alternative — a generic component
with an optional-field bag — is worse in a domain where RFC 5545 *forbids* `DTEND` on a
VTODO, and the `Todo` doc comment saying so ("the field simply not existing is what makes
emitting one impossible") is the right instinct. `Recurrence::from_ical_event` and
`Reminder` being made generic over `Component`, and `summary_slug`/`time_slug` being lifted
out of `Event`, share the parts that are genuinely shareable. I would ask for one
integration test that fails when a field is added to `Event` and not considered for `Todo` —
even a compile-time exhaustive destructure with a comment — so the drift is at least loud.

**Also genuinely good, briefly:** the `AGENTS.md` sync-model correction (the documented
model said mtime; it is actually a three-way base comparison — that was wrong before this
PR and I did not know); the state-format compatibility work in
`known_event_ids.rs`/`event_bases.rs`, pinned with sha256 digests, and the C-11 analysis
showing an older caldir treats a `VTODO:`-prefixed line as an inert unknown id in both
directions; the `--json` additive-key discipline and the honest replacement of
"byte-identical" with a checkable `del(...)` predicate; and the website pages, which are
the clearest task documentation in the repo and would be excellent if B3 were fixed.

---

## What I would tell the author about splitting

You have done six PRs' worth of work and asked me to review one. Land them in this order;
each is independently valuable, independently revertible, and small enough that I can
actually read it.

| # | PR | Content | Why separate |
|---|---|---|---|
| 1 | **Graceful degradation on unreadable `.ics`** | `calendar.rs:111` (`events()` → `CalendarListing`), new `calendar/listing.rs`, the `Deletes::Suppressed` gate in `calendar_diff.rs`, the `doctor` skipped-file warnings, the agenda `incomplete` footer (`agenda_text.rs:20,44`) and the `AgendaView` → `Agenda` wrapper that exists to carry it | A pure resilience fix for VEVENT users. Today one bad file blanks a calendar. Ship this first — it is the most valuable thing in the branch and has nothing to do with tasks. Note it changes `CalendarDiff::compute` → `compute_from_listing`, so it wants the breaking window too. |
| 2 | **`push` exit code and stderr** | `commands/push.rs`, `tests/push_exit_status.rs` (252 lines) | `push` currently exits 0 on failure and prints the error to stdout. That is a bug affecting every scripted user; it deserves its own line in the changelog, not a footnote in a feature PR. |
| 3 | **CalDAV: strict resource decoding** | `ops.rs:43` (`parse_event`'s `find_map(Result::ok)` → `parse_item`), `ops/list_events.rs:50` | This is the riskiest hunk in the whole branch and it is buried. Today a malformed VEVENT is silently skipped; after this it aborts the listing — and an aborted listing is one belt away from a mass delete. It needs its own review and its own release note. |
| 4 | **CalDAV: 409 `no-uid-conflict` fallback** | `ops/create_event.rs:22,75`, the `wiremock` dev-dep, `test_dav.rs` | A create silently becoming an update against an href caldir did not choose is a remote-write behaviour change for VEVENTs. Argue it on its own merits. Drop `ensure_single_component` (N4) or defend it here. |
| 5 | **Public error API** | `lib.rs` re-exports, `#[non_exhaustive]` on the four enums, `tests/public_error_api.rs` | API hygiene, breaking, and — per B5 — needs the wildcard-vs-panic decision made deliberately rather than as a side effect. |
| 6 | **Pure moves and doc fixes** | `event/equality.rs`, `event/slugify.rs` extraction, `AGENTS.md`, `.gitignore` | Zero-behaviour. Trivially reviewable, and it makes PR 7 ~200 lines smaller. |
| 7 | **VTODO support** | `item.rs`, `todo/**`, `ComponentKind`, the item-typed sync seams, the state-format sigil, agenda/JSON/diff task rendering, `--undated`, the Radicale live tier, `specs/caldir.md`, the website pages | The feature. With 1–6 out of the way this is the coherent thing it should have been, and it is the one I want to spend my review budget on. |

Two conditions across all seven:

- **B1 first, everywhere.** No PR of yours merges with `R70` or `L5-01` in a comment. Do
  that pass before splitting; it will make the split easier to read too.
- **`docs/vtodo/` is not in any of them** (B2). Put the reasoning in the PR descriptions,
  where it will be read once at the moment it matters and then archived by GitHub. Distil
  §C into a short user-facing limitations page and keep that.

And for PR 7 specifically: bring the pull/push asymmetry back as "tasks are never outgoing,
with a visible notice" rather than "a task refuses the calendar's push". If you do that,
I do not have a design objection left.

---

*One last note, meant kindly.* The engineering judgement in this branch is better than
most of what I merge. What went wrong is process, not skill: 49 tickets with acceptance
criteria produced 920 tests that satisfy 49 tickets, an external QA pass then found seven
things a user would hit in an afternoon, and 16,600 lines of prose accumulated to hold the
whole thing together — prose that is now load-bearing for the code and already wrong in
four places. A smaller PR would have caught more of this earlier, from me, for free.
