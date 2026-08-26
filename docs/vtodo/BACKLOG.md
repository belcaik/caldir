# VTODO support — exploration backlog

Destination: first-class VTODO in caldir, round-tripping local ICS <-> domain <-> provider RPC <-> generic CalDAV (Radicale). No VEVENT conversion. No VEVENT regressions.

## Stage 1 — knowledge base (read-only exploration)
- [ ] domain-model      -> context/domain-model.md
- [ ] parser            -> context/local-parser.md
- [ ] sync-engine       -> context/sync-engine.md
- [ ] provider-contract -> context/provider-contract.md
- [ ] caldav            -> context/caldav.md
- [ ] providers         -> context/provider-matrix.md
- [ ] dependency        -> context/icalendar-capabilities.md
- [ ] tests             -> context/testing.md
- [ ] SYNTHESIS         -> context/system-map.md

---

# Part II — What was deliberately NOT built (consolidated record)

Written by **L8-06**. Source: `docs/vtodo/adr/ADJUDICATION.md` (binding), `docs/vtodo/spec.md`
§16 + Appendix, and direct verification against the tree at **902 passed / 0 failed** (plus 6
live Radicale gates, off by default).

**Record only. Nothing here is a task to do now.** Every entry carries its trigger condition:
the thing that would make it worth picking up.

Three categories, kept distinct:

- **§A Non-goals** — deliberately out of scope, with the evidence-based reason.
- **§B Deferred** — wanted, not done, with what it would take.
- **§C Known limitations** — true of the shipped feature; a user can hit them.

Plus **§D** the open questions Q-1..Q-11 with their resolutions, **§E** the probe-hygiene project
rule (R108), and **§F** an audit trail of items the adjudication recorded as open that were
**verified CLOSED** while writing this.

Counts: **10 non-goals, 20 deferred items, 13 known limitations.**

---

## §A — Non-goals (10)

Deliberately out of scope. Each states the evidence, not a preference.

### A-1. `R86` — Google Tasks / Microsoft To Do
A different API surface entirely: only `/events` collections exist in either provider today
(`caldir-provider-google/src/google_event/from_google.rs`,
`caldir-provider-outlook/src/graph_api/types/mod.rs`). Both would need **new OAuth scopes** —
Outlook's consent is `Calendars.ReadWrite User.Read offline_access`, Google's is
`calendar.calendarlist.readonly` + `calendar.events`; neither covers a tasks API. That means
**re-consent for every existing user**, which is why this is a non-goal rather than a deferral.
`UNVERIFIED:` the exact vendor scope names. *Trigger:* a maintainer accepting the re-consent
cost. *Settled by:* vendor docs, not code.

### A-2. `R87` — task hierarchies / `RELATED-TO` / subtasks
The `icalendar` parser collapses them: `MULTIS` lists the ABNF placeholder `"RELATED"`, not
`"RELATED-TO"`, so two lines in yield one out with `PARENT` silently gone (verified).
Re-encoding as `X-CALDIR-RELATED-TO` inherits the identical defect — repeated same-name `X-*`
collapse the same way. Radicale holds both values with `RELTYPE` params intact (probe Q-F2), so
**the loss is 100% caldir's parser**, not the server's. `Todo` has no `related_to` field,
`to_icalendar.rs` never emits one, `R15_DROPPED_PROPERTIES` includes it, and `T-17` asserts the
drop. *Trigger:* a hand-rolled emitter/parser for repeated properties (see also A-3 — same
mechanism, same fix). *See also* B-18: spec §6's row still contradicts this.

### A-3. `R88` — `CATEGORIES`
Modelling it through the crate **corrupts** it: `WORK,URGENT` becomes `WORK\,URGENT` (verified
against pinned `=0.17.10`), turning two categories into one for every third party. Flipping it IN
via the typed path is *worse* than dropping it. Plus `specs/caldir.md` skips it for events, and a
todo-only exception inside a table named "Fields We Skip" is incoherent. *Trigger:* a hand-rolled
emitter. *Would take:* a follow-up ADR covering **both** kinds together, not a todo-only patch.

### A-4. `R89` — recurring-task expansion
`RRULE` / `EXDATE` / `RDATE` are **modelled and preserved** (R13) but never expanded: the crate's
`recurrence` feature is off, and expansion needs a `DTSTART` a task may not have. Master only.
Working rule, revisited only on a user report; a recurring undated task is in-window
(R44 row 4 = TRUE).

### A-5. `R90` — `caldir new --due`
No ADR owns it; tasks arrive by pull or by hand-authored `.ics`. `caldir new` continues to create
events only. This is **guarded, not merely prose** — `caldir-cli/src/commands/new.rs:480`
`new_has_no_due_flag` locks it.

### A-6. `R91` — the capability mechanism (ADR-07), in whole
**Cut by user decision.** No config field, no builder, no provider-crate edits, no third diff
bucket, no renderer or pluralization, no `is_empty()` fix. It bought a nicer Google/Outlook error
while every other decision places those providers outside the MVP — **and** its own default
("absent = does not support VTODO") meant every existing caldav and iCloud user would get zero
task support after upgrading until re-running `connect`, silently. Worse than the message it
prevented. Do not re-open piecemeal; see C-3 for the accepted consequence.

### A-7. `R92` — `supported-calendar-component-set`. **RECORD ONLY. DO NOT IMPLEMENT.**
**It is ADVISORY ONLY and can NEVER be an enforcement point.** A VEVENT PUT into a VTODO-only
collection returned **`201`** (probe Q-A). Any future work MUST treat the declared value as a hint
that can simply be wrong: enforcement built on it would reject valid operations and permit invalid
ones.

The mechanism, measured, so it is not re-measured later:
- It **is** advertised by Radicale on every calendar collection, at **Depth 0 and Depth 1**.
- Default value `VTODO,VEVENT,VJOURNAL`; a VTODO-only collection returns `VTODO`.
- **404 on the principal.**
- **Bulk-readable via ONE Depth-1 PROPFIND on the home-set** — per-calendar round trips are
  unnecessary.

The trap is precisely that this reads like a design ("one PROPFIND gets everything!"). The
mechanism works; the *semantics* do not. **Non-goal:** adding a
`supported-calendar-component-set` PROPFIND to the CalDAV provider "just to read it".

### A-8. `R93` — VJOURNAL
Out of scope, but the design MUST NOT preclude it: adding it later should be one enum variant, one
struct, one match arm — no second redesign. (This is why B-4's `#[non_exhaustive]` work matters:
`ComponentKind` and `CalendarItem` are the enums VJOURNAL grows.)

### A-9. `R94` — renaming the `Event*`-prefixed shared types
Cosmetic. ~20 call sites of churn for zero behavioural benefit. **Deferred unless a maintainer
asks.** Note the naming residue this leaves: `EventInstanceId`, `EventChange`, `CalendarEvent`
and `Calendar::events()` all now carry tasks too.

### A-10. `CLASS` is not a modelled field on `Todo` (adjudication ruling, raised by L1-01)
Spec §6 says CLASS "MAY be inherited, MUST NOT be added deliberately" but never says it is a
*field*. **RULED: it is not.** The codec MUST NOT invent one. Consequence is a known limitation,
not a hidden one — see C-4.

---

## §B — Deferred (20)

Wanted, not done. Each says what it would take.

### B-1. Task field rows in the diff renderer, beyond `summary` / `due` / `status`
**Ruled:** L7-07's narrower set IS the MVP — `status` already carries the completion signal a user
acts on (`needs_action` → `completed`), and `completed` is only its timestamp. **Deferred to this
named follow-up, not silently dropped:** rows for `percent_complete`, `priority`, `completed`,
`description`, `location`, `url`, `reminders`, `attendees`, `attachments`, `x_properties`.
*Consequence:* a task update touching ONLY an unrowed field renders a header line with no rows —
**under-described, not invisible, and not data loss** (C-7). *Would take:* one row per field in
`caldir-cli/src/output/diff.rs`, each with a byte-exact lock.

### B-2. `priority` and `created` have no `--json` surface
Both are destructured as `_` in the task arm (`caldir-cli/src/output/agenda_json.rs:221`, `:231`),
each with a comment saying why. ADR-10 D6 says PRIORITY "rides whatever surface ADR-04's property
set gives it, or gains a key under the same rule" — it currently has none. `created` is unmodelled
on `Event`, so a key would be non-uniform across kinds. Both were explicit ticket non-goals.
**They are owed, not silently dropped.** *Would take:* two additive keys under the same
append-at-the-end rule that C12's DEL predicate depends on (C-9).

### B-3. `--hide-completed`, and any config key for it
ADR-10 D5: explicitly deferred, not MVP. The MVP's own fixture is `STATUS:COMPLETED` (u_crawler),
so hiding completed tasks by default would hide the demo case. *Trigger:* a user with a long
completed-task tail.

### B-4. The 0.15.0 breaking-window list — **VERIFIED NOT DONE, and currently unowned**
Four items must ride the breaking release the branch is **already** taking (0.14.1 → 0.15.0),
because adding each is *itself* breaking and would otherwise cost the same again later:
1. `#[non_exhaustive]` on `CalendarError` (gained `IncompleteListing` in L0-05 — already breaking
   for a downstream matching exhaustively without a wildcard),
2. `#[non_exhaustive]` on `ComponentKind` (grows with VJournal under R93),
3. `#[non_exhaustive]` on `CalendarItem` (same position; L2-01 left it bare because its AC said
   "exactly two variants" — right for the ticket, wrong for the release),
4. `RemoteError::UnsupportedItem` joining that list.

**STRUCK from this list:** `WireValue::from_wire` returning `Result`. L4-02 made it *unnecessary*,
not deferred — the inbound direction gets its own `ItemIcs` newtype whose `Deserialize` carries the
fallible decode, so `from_wire` only ever unwraps an already-validated value. The invariant is
documented on the trait.

**VERIFIED at 902/0:** `grep -rn "non_exhaustive" caldir-core/src/` finds only a comment in
`item.rs:243`. **None of the four has landed.** The adjudication assigns them to "L8-05 (release
prep)", but L8-05 is *the acceptance gate* and L8-04 is *the semver bump* — **neither ticket's
body mentions `non_exhaustive`.** *Trigger: before 0.15.0 ships. After it ships, the window
closes and the cost is paid twice.*

### B-5. Test constructors for downstream crates — **ONE ticket, five instances**
Root cause is single: `Caldir::new` is `#[cfg(test)] pub(crate)` (`caldir-core/src/caldir.rs:20`)
and `CalendarDiff::compute` / `from_changes` sit under `#[cfg(test)]`
(`caldir-core/src/diff/calendar_diff.rs:234-253`), so **no downstream crate can construct
either**. The five recorded instances — `output/diff.rs`, `output/agenda.rs`, `commands/rsvp.rs`,
plus the `CalendarDiff` gap itself, plus L7-07's uncatchable M14/M15 — are one defect.
*Consequence:* it blocks **every** future CLI-level lock over a whole diff or a whole `Caldir`,
not just the ones found so far. *Would take:* one 0.15.0-window ticket exposing test constructors
for `Caldir` + `CalendarDiff`. Rides B-4's window.

### B-6. Public-but-unnameable error types (reported **twice**)
**VERIFIED still open** — `caldir-core/src/lib.rs` re-exports neither:
- `CalendarError` is the error type of the public `Calendar::events() -> Result<CalendarListing,
  CalendarError>` and of `SkippedEvent::error()`. Works today via `Display` through method
  resolution, but a consumer **cannot match on it**.
- `CalendarEventError` is `pub(crate) use`d in `calendar.rs`, yet it is the error type of the
  public `CalendarItem::load` and `CalendarEvent::load`.

*Would take:* two `pub use` lines. **Resolve BOTH together, in the 0.15.0 window** — adding them
alongside B-4 keeps the enums' `#[non_exhaustive]` decision and their public visibility in one
diff.

### B-7. `duplicate_file_sets` should bucket into a `BTreeMap`
L2-08's M6 is only ~50% catchable because the **mutant is nondeterministic**: without the trailing
`sets.sort()` the order comes from randomized `HashMap` iteration and lands correct about half the
time. The honest fix is a `BTreeMap` so ordering is intrinsic and the sort disappears — but
`EventInstanceId` derives only `Debug, Clone, PartialEq, Eq, Hash`
(`caldir-core/src/event/instance_id.rs:22`), so this needs `Ord` on a **public type**. Rides B-4's
breaking window.

### B-8. `CalendarError` needs a "not an event" variant
Two unreachable narrowings in `calendar.rs` fall back to `MasterNotFound`, which is semantically
wrong if it ever fires. Now that tasks have real paths, this wants a proper variant. Rides B-4's
window (adding a variant is breaking unless B-4 lands first).

### B-9. `Connection` has no "fresh process" seam
`T-49` and `T-52` both reach into the **private** `connection.local` to reload the calendar,
because a second `caldir pull` is a fresh process that reads bases from the state dir rather than
an in-memory map. Two sites now touch a private field. *Would take:* a small
`Connection::reload_local()`. **VERIFIED:** no such method exists.

### B-10. No recovery path for `412`
A lost race on a conditional PUT surfaces as a raw error with no guidance. Explicitly a non-goal
in L5-03. *Would take:* a decision on retry-vs-report and one wiremock test; the seam already
exists (`caldav/ops/test_dav.rs`).

### B-11. `update_event` has no direct test
**VERIFIED:** `caldir-provider-caldav/src/caldav/ops/update_event.rs` has no `mod tests`. It is
exercised only transitively via `create_event`'s 409 fallback, so M10/M11 fail *by proxy* rather
than for the right reason. *Would take:* one dedicated round-trip test — cheap now that the
wiremock seam exists.

### B-12. Provider `Handler` adapters are untested — 5 tests, a MASS-DELETE path
L4-02's M12 slipped: replacing a provider's `list_events` body with `Ok(Vec::new())` passes that
crate's suite. **VERIFIED:** `grep -rn process_request caldir-provider-*/src/` finds nothing — **no
provider crate has any test over its `Handler` impl's adapter layer**, and the pre-change one-line
body was equally untested. A dropped listing is a mass delete. *Would take:* one `process_request`
round-trip test per provider (5 tests).

### B-13. A pure task-only outgoing `Delete` has no lock
`[Delete(todo)]` aborts inside `into_event()` **before** `apply_change` can return, so the
`unreachable!("apply_change returns None for Delete")` invariant holds **by luck of ordering, not
by assertion**. Cheap to add. L6-01 territory.

### B-14. `Calendar::master_event_for` — deliberate remaining absence-inference hole
**VERIFIED still present** (`caldir-core/src/calendar.rs:153-164`): it calls `self.events()?` then
`into_events()`, so an **unreadable master file is dropped** and the method returns `Ok(None)`.
Same shape as the three L0-05 fixed. Left because it **never writes**, and its one internal caller
(`update_recurring_instance`'s `None` branch) is now only reachable on a complete listing. But a
GUI calling it directly still cannot distinguish "no master" from "unreadable master".
**Not a data-loss path.** *Trigger:* making the whole absence-query family coherent.

### B-15. `delete_recurring_instance` master/override precedence (pre-existing, not introduced here)
A degenerate file that is BOTH a master (`recurrence.is_some()`) and the target override hits both
branches. Current code prefers master (EXDATE); **nothing locks that choice** and no fixture has
such a file. L2-03 preserved the order exactly and did NOT repair it. *Would take:* either a
degenerate fixture, or an explicit ruling that the shape is illegal.

### B-16. `R58`'s new failure surface — fail-loud vs tolerate. **Undecided.**
`fetch_events` now issues a **second REPORT per sync** against every CalDAV/iCloud calendar, and a
failure on it **propagates**. Radicale accepts a bare VTODO query (probe Q-B), but a server that
**rejects** VTODO queries would turn a previously-working VEVENT sync into an error. *Would take:*
a decision (tolerate-and-warn is the conservative option) plus one wiremock test for the rejecting
server. **VERIFIED: still undecided** — the second REPORT propagates today.

### B-17. `docs/vtodo/context/testing.md` additions — real doc debt
**VERIFIED absent** (`grep -n "reply_raw_data\|test_dav\|percent-encod"` → nothing). Missing:
- `mock_provider::reply_raw_data` — the crate's **only** raw-wire escape hatch, i.e. the only way
  to stub a wire shape no typed response can express (a provider *newer* than this core).
- `caldir-provider-caldav/src/caldav/ops/test_dav.rs` — the crate's HTTP-double seam.
- **The raw-ICS rule** (below — the single most transferable finding of the effort).
- `MockTransport`'s FIFO and last-request traps.
- Radicale percent-encodes `@` in hrefs.

**The raw-ICS rule, stated once here so it survives even if testing.md never gets it:**
`MockProvider::reply` encodes the expected value using `to_ics_string` — **the encoder under
test** — so a lossy serialize cancels on both sides and the assertion goes blind. **Measured:**
under a `reply`-based idempotence gate, **6 of 12** mutations that the raw-fixture version catches
SURVIVED (dropped `COMPLETED`, stripped `X-*`, flattened `TZID`, coerced floating `COMPLETED`,
synthesized `DTSTART`, `DESCRIPTION` drift). **Every future round-trip / idempotence / fidelity
assertion must drive the remote from RAW ICS via `reply_raw_data`, never from domain values via
`reply`.** The remote must be an independent store — which is what a remote *is*.

### B-18. `docs/vtodo/spec.md` §6's `RELATED-TO` row is **wrong** — open doc debt
**VERIFIED still wrong, in two places:**
- `spec.md:198` — *"A single value survives unmodelled."*
- `spec.md:686` (R87) — the same clause.

Both **contradict R15, T-17 and the shipped codec**: `Todo` has no `related_to` field,
`to_icalendar.rs` never emits one, `R15_DROPPED_PROPERTIES` includes RELATED-TO, and T-17 asserts
the drop. The phrasing came from ADR-05's justification, which **C-2 VOIDED** when it widened the
property set. *Would take:* correct it, or explicitly scope it to *"survives ON THE SERVER until
caldir's first write"* — which is the true statement. Assigned to L8-02/L8-04; **not done at
902/0**, so it is recorded here rather than lost.

### B-19. A crate-version-pinned guard for `icalendar::TodoStatus::Custom`
**Accepted non-lock, not a testing gap.** L1-03's M8 (swapping the raw `STATUS` read for the
crate's `get_status()`) slips and is **unobservable at that seam**:
`icalendar::TodoStatus::from_str` (`properties.rs:303-311`) is a byte-for-byte match of caldir's
`TodoStatus::from_ics_str` — same four literals, same `_ => None` — and both call sites
`.unwrap_or_default()`. Since R14 rules the outcome is DROP, the two are behaviourally identical
on `0.17.10`. Reading raw is **future-proofing**: the crate's enum is not caldir's, and its
commented-out `//Custom(&str)` shows upstream intends to widen it. **Do NOT invent an assertion
that reaches into implementation detail to "catch" M8.** *Trigger:* the day upstream adds
`Custom` — a version-pinned guard test is the right shape then.

### B-20. `EventError::UnsupportedComponent`'s name is unpinned
R76 forbids duplicating the "none" variant on `EventError`, so R19's carrier there is a distinct
`UnsupportedComponent(String)` (`caldir-core/src/event/error.rs:18`). **Neither the spec nor any
ADR pins that name, and the wire error text is USER-VISIBLE.** It shipped unpinned. *Would take:*
a one-line spec row plus a byte-exact lock, the way the two arity literals got one.

---

## §C — Known limitations (13)

True of the shipped feature. A user can hit every one of these.

### C-1. **caldir PULLS tasks from CalDAV but CANNOT PUSH them.** (`R69` / `R70`)
The outbound RPC field stays `Event` (`rpc/methods/create_event.rs:10`) — 25 `cmd.event` sites
across five provider crates depend on it, and retyping it would take the provider blast radius
from 6 errors to ~25 sites. So a task push **aborts in `into_event()` before any RPC is
serialized**, with the byte-exact `expected 1 event(s) in ICS, found 0`
(`EventError::UnexpectedEventCount`; asserted at `connection.rs:764`). **This must be stated
plainly in the ship report and the PR description.** It is the single largest limitation of the
feature.

*Downstream consequences worth naming:* a local task file **rename** never reaches a server (a
rename reaches a server only via a push); the test that "proves" href stability for tasks does so
**by construction** (`event_url` takes the UID and nothing else), not by measurement.

### C-2. **iCloud (Apple Reminders) is UNMEASURED.** (`R69` / `Q-10`)
iCloud rides the **exact same** `caldav::ops` code path — all four ops are pure pass-throughs with
no mapping layer, and both hard-coded `<C:comp-filter name="VEVENT">` literals live in the caldav
crate — so L3-02/L5-02 light up iCloud tasks with **zero icloud-crate edits**. But **every
statement in spec §12 is unverified there.** Three load-bearing unknowns:
1. does it **409** a same-UID pair in one collection? (load-bearing for R62/R65)
2. does it implement **RFC 4791 §9.9** the way Radicale does? (R58)
3. does it inject **non-`X-` properties** onto a task? (R67)

Plus: **are Reminders collections returned by discovery at all?**
***Settled by:* running the same six probes against a real iCloud account. Nothing else will do
it.** **Do not claim iCloud task support anywhere.**

### C-3. Pushing a task to Google/Outlook aborts the rest of that calendar's push
Accepted consequence of the ADR-07 cut (A-6). The abort happens at `connection.rs:232` with
`expected 1 event(s) in ICS, found 0`. **Ugly, NOT data loss** — ADR-08 D3 and ADR-09 C7 cover the
destructive variants. There is no capability pre-check by design, and per A-7 there could not be a
reliable one.

### C-4. `CLASS` on a VTODO is **dropped on caldir's first write**
Per A-10 it is not a modelled field, and ADR-05's unmodelled-property passthrough is **`X-*` ONLY**
(measured). An inherited `CLASS` therefore does not survive.

### C-5. caldir's first genuine edit **destroys correct server data** for every dropped property
This is the general shape behind C-4, A-2 and A-3, and it is worth stating once: CalDAV update is
a **whole-resource PUT**. After a *pull*, local/base/remote all lack a dropped property equally, so
nothing pushes — but the **first genuine edit** re-PUTs the whole resource without it. Radicale is
a faithful store (it holds both `ATTACH`, both `RELATED-TO` with params, `ATTENDEE`, `CATEGORIES`
unescaped), so **everything caldir drops is caldir destroying correct server data**, not the server
losing it. C-2 widened the modelled set specifically to shrink this surface (`ATTACH`, `ATTENDEE`,
`ORGANIZER`, `LOCATION`, `RRULE`/`EXDATE`/`RDATE`, `PERCENT-COMPLETE`, `CREATED` all flipped IN).
What still falls in this hole: `CATEGORIES`, `RELATED-TO`, `GEO`, `RESOURCES`, `COMMENT`,
`CONTACT`, `CLASS`, and any other IANA or non-`X-` property.

### C-6. Recurring tasks are **master only** — never expanded (A-4)

### C-7. A task update touching only an unrowed field renders a header line with no rows
Under-described, not invisible, and not data loss. See B-1.

### C-8. `VALARM` in a VTODO round-trips only through caldir's **hand-rolled** emitter (`R24`)
The crate's typed path regenerates a **random VALARM UID** on every generation (Path A); the
hand-rolled block spliced on the way out (Path B) is byte-stable across three generations, verified
against pinned `=0.17.10`. Anything that reroutes VALARM through the typed path reintroduces
per-write churn.

### C-9. `--json`: "byte-identical" is arithmetically impossible; the DEL predicate is the honest guarantee
Adding five keys to every entry necessarily changes every line's bytes. What **is** true, and is
proven against bytes captured from the pre-change tree in all three timezones (1653 bytes each):

```
select(.kind == "event") | del(.kind,.due,.completed,.percent_complete,.task_status)
```

is **byte-for-byte identical** to the old stream — 26 keys, same order. The five additive keys are
**appended at the end**, so the pre-existing key order is preserved. Spec §15's C12 row and the
website docs must state **this predicate**, never the word "byte-identical".

### C-10. `completed`'s encoding diverges from ADR-10 D6's wording
D6 tabulates `completed` as "RFC3339 UTC". `Todo::completed` is an `EventTime` because a
**floating** `COMPLETED` is legal (R22), so it is encoded through the same `serialize_time` rule as
every other timestamp: a UTC value serializes exactly as the ADR says; a **floating** one renders
in the **local zone** rather than being coerced — coercing would invent an offset. The code is
right; the ADR's wording is the thing that is narrow. Docs must state the actual rule.

### C-11. A VEVENT whose UID literally begins with `VTODO:` round-trips as a task
`known_event_ids` needs a per-kind sigil, or it collapses the `(uid, recurrence_id)` pair across
kinds. **Residual accepted:** no encoding removes this ambiguity, because UID is TEXT. It is the
same class the codebase already accepts upstream for `__` (`instance_id.rs:38-40`). Backward
compatibility holds in both directions: existing unprefixed lines still parse as `kind: Event`, and
an **older** caldir treats a `VTODO:`-prefixed line as an inert unknown id that cannot trigger a
delete.

### C-12. caldir emits **no `VTIMEZONE`**; the pushed body and the stored body differ structurally
See D-3 (Q-3). Harmless under property-level equality — **fatal for any whole-document
comparison.** Related and equally load-bearing: Radicale does **not** echo PUTs byte-for-byte
(properties sorted alphabetically within each component, lines re-folded at 75 octets, VTIMEZONE
synthesized for any TZID), though it **is** idempotent (2nd round trip byte-identical). Therefore
**"sync twice, second diff empty" MUST be parsed-domain `==`, or GET-pass-2 vs GET-pass-1 — never
raw bytes against what was PUT.**

### C-13. `ETag` can prove "remote changed"; it can **never** prove "caldir's push was a no-op"
ETag is invariant to property reordering, **but `DTSTAMP` changes it**, so every caldir push
mutates the ETag.

---

## §D — Open questions Q-1 .. Q-11

Each carries its original *Settled by:* clause — **that clause is the value**. Resolved questions
are recorded **as resolved, with who settled them and how**; none is deleted.

### Q-1 — Should `--json` honour a `--undated` text flag, or always emit the full set? — **OPEN**
ADR-10 ruled "always the full set" so scripts see everything, at the cost of text and JSON
disagreeing on cardinality, and recorded this as the one sub-question it could not settle on
evidence rather than taste. ***Settled by:* a maintainer preference; a one-line policy flip.**

### Q-2 — `task_status` vs `todo_status` as the `--json` key — **CLOSED**
*Original settled-by:* writing `specs/caldir.md`'s §To-dos terminology first (R16), which did not
exist at the time. **Resolved by L8-01**, verified at 902/0: `specs/caldir.md:183` now carries a
`## VTODO (Task)` section, and the shipped key is **`task_status`**
(`caldir-cli/src/output/agenda_json.rs:138`). It was also independently pinned twice — L1-01
inferred the serde labels `needs_action` / `in_process` / `completed` / `cancelled` by analogy with
`Status`, and ADR-10 specifies exactly those four values for `task_status`. They agree. The
renderer must not re-derive them, and must not retype the existing `status` key (C12).

### Q-3 — Does caldir emit `VTIMEZONE`, or tolerate the server adding one? — **CLOSED by measurement**
*Original settled-by:* one decode test, one zoned round trip, and a decision on the emit side.
**Answered by L5-05's live Radicale tier:** **caldir emits a zoned `DUE`/`DTSTART` as a bare `TZID`
parameter with NO `VTIMEZONE` component, and TOLERATES the server adding one.** Measured live —
Radicale accepts a VTODO with `DUE;TZID=Europe/Stockholm` and no VTIMEZONE (`201`), returns it with
a full **synthesized** VTIMEZONE sibling, and caldir parses that two-component document back to the
same zoned DUE, because **VTIMEZONE is exempt from the single-component arity rule** (R18/R102).
See C-12 for the consequence.

### Q-4 — Should `percent_complete: None` and `Some(0)` compare equal? — **CLOSED**
*Original settled-by:* a round-trip probe against a second server. **Answered by L4-04:**
`None` vs `Some(0)` is **NOT a churn source today.** `PERCENT-COMPLETE:0` parses to `Some(0)`,
re-serializes to `PERCENT-COMPLETE:0`, parses back to `Some(0)` — **stable**. `None != Some(0)`
under `Todo`'s `PartialEq`, but **nothing in the pipeline converts one to the other**, so the
distinction never produces a diff. T-49's fixture carries `PERCENT-COMPLETE:0` **permanently**, so
an asymmetric future collapse is caught.

### Q-5 — Is `caldir-core` actually consumed out-of-tree? — **OPEN, `UNVERIFIED`**
Published (0.14.1) and presented as a library, but no external consumer was found. If none exists,
C11 relaxes further. ***Settled by:* crates.io reverse-dependencies and download counts.**
*(L8-06 did not check: it requires network access this ticket does not take.)* This question gates
how much B-4 / B-5 / B-6 actually matter — if nothing consumes core, the breaking window is cheap.

### Q-6 — Where does R48's "listing is incomplete" signal live? — **CLOSED**
*Original settled-by:* changing the return to a small listing struct and counting call-site churn
(~5 core + 2 CLI sites). **Resolved by L0-05, exactly as predicted:** the signal lives in
**`Calendar::events()`'s return type** — `pub fn events(&self) -> Result<CalendarListing,
CalendarError>` (`caldir-core/src/calendar.rs:112`), with `CalendarListing` and `SkippedEvent` in
`caldir-core/src/calendar/listing.rs`, both re-exported from `lib.rs:18`. A listing is never
silently short.

### Q-7 — Does `caldir update` upgrade core and providers atomically? — **ANSWERED by L8-06 (reading)**
*Original settled-by:* ten minutes of reading — which this ticket did.
**Answer:** `caldir-cli/src/commands/update.rs` downloads **one release tarball for one version**,
discovers the binaries inside it, and updates **only those already installed locally** (so users
keep whichever providers they chose), in a single sequential `remove_file` + `copy` loop.

So it is **atomic in intent** — one version, one tarball, one invocation — but **NOT
transactional**: a failure part-way through the loop (permission denied, disk full) leaves a
**mixed-version install**, caldir updated and some providers not. Two further gaps: a provider
installed by other means is never touched, and a caldir installed via cargo or a package manager
never goes through this path at all. **Therefore the mixed-version window is narrow but real, and
R49 (tolerate unknown ids, never delete on them) stays load-bearing.** *Would take, to close
fully:* copy-to-temp-then-rename-all, or a rollback on partial failure.

### Q-8 — Are sibling comp-filters under `VCALENDAR` OR'd in Radicale? — **OPEN, and now moot**
*Original settled-by:* one probe. **Still unprobed** — but the shipped code records the reason it
does not matter: **RFC 4791 §9.7.1 says a comp-filter matches when its children *all* match**, so
`VEVENT` and `VTODO` side by side under one `VCALENDAR` filter read as **AND** and would match
nothing (`caldav/ops/list_events.rs:60-67`). Even if a given server ORs siblings, relying on it
would be non-conformant. **caldir issues two REPORTs and never relies on it (R59).** *Trigger:*
only a measured performance problem from the second REPORT would justify revisiting.

### Q-9 — Does the real u_crawler fixture carry `LAST-MODIFIED` and/or `DTSTART`? — **OPEN, `UNVERIFIED`**
`LAST-MODIFIED` decides whether R41's Push-biased fallback is on the hot path or never reached;
`DTSTART` decides whether the demo case is anchored or undated (and so whether R84's undated footer
ever fires for the demo). ***Settled by:* the Stage 8 fixture swap.**
**Verified state at 902/0:** the fixture in `docs/vtodo/reproduction.md:14-33` is explicitly
*"representative of"* u_crawler output, not captured from it — it carries `DUE` and **no**
`LAST-MODIFIED` and **no** `DTSTART`. `find /home/belcaik/Dev/u_crawler -name '*.ics'` returns
**nothing**, so the related premise the adjudication filed as *"Unverified premise, still open"* —
u_crawler's `windows/` / `deadlines/` layout, described by the user but never found on disk — is
**also still unverified**. Treated as accurate for design purposes. *Would take:* one real
`.ics` file from a u_crawler run.

### Q-10 — Does iCloud (Apple Reminders) behave like Radicale? — **OPEN.** See C-2.
***Settled by:* running the same six probes against a real iCloud account before Stage 8.**

### Q-11 — Rename the pre-existing error variants, or keep the old names for continuity? — **OPEN**
Pre-1.0 says rename; a maintainer preference overrides. ***Settled by:* asking the maintainer.**
Related but distinct from A-9 (`R94`, the `Event*` type rename) and B-20 (an *unpinned* name, which
is a different problem from a *disputed* one).

---

## §E — Project rule carried forward: probe hygiene (`R108`)

**Any future probe crate MUST pin `icalendar = "=0.17.10"`, MUST print the resolved version, and
MUST echo the exact input bytes it sent.**

Two probes in this effort produced a **wrong answer from an unexamined input**:

1. **`valarm-probe` resolved `icalendar` 0.17.13, not 0.17.10.** ADR-04's cited version was FALSE —
   a caret requirement drifted, which is the exact trap ADR-05 had already found. Re-run **pinned**
   `=0.17.10`, the conclusion survived and the evidence was replaced: Path B byte-stable across
   three generations, Path A regenerates a random VALARM UID.
2. **The grill's own bash `printf` fixture embedded a literal `\r\n`** and measured an ICS carrying
   **no `COMPLETED` property at all**. Retracted as F-1.

> *A probe that returns the answer you expected is not evidence until you have inspected the input
> it actually sent.*

**Corollary rules of the same family, recorded here because they are the same failure:**
- Drive a remote from **raw ICS**, never from domain values re-encoded by the code under test
  (B-17).
- **Cite the symbol, never the line number.** Spec and ADR line references in this doc set drift
  1–2 lines systematically (e.g. `calendar_diff.rs:78-90` for an actual `:76-90`;
  `connection.rs:231` for an actual `:232`).
- **Name which literal.** The two arity errors are **distinct locks on distinct paths**, and were
  conflated once already:
  - `expected 1 event(s) in ICS, found 0` = `EventError::UnexpectedEventCount` = the **outbound**
    task-push abort;
  - `expected 1 supported component(s) in ICS, found {n}` = `EventError::UnexpectedComponentCount`
    = the **inbound / local decode** path.

---

## §F — Recorded as open, VERIFIED CLOSED at 902/0

An audit trail, so a reader does not chase items that were already fixed by a later ticket. Each
was checked against the tree, not against a report.

| Item, as the adjudication recorded it | Verified state |
|---|---|
| ⚠ *"HIGHEST PRIORITY — the belt is still silent"*: `suppressed_task_deletes()` is public but nothing reads it; R49's "and warn" only half discharged | **CLOSED.** Consumed by `caldir-cli/src/output/diff.rs:174,190,200`, per calendar, on incoming surfaces, evaluated **before** the `is_empty()` early return. PR-1's MAJOR-2 is answered. |
| *"MUST-FIX before ship — the two-REPORT merge (L5-01 M7)"*: nothing locks the merge; M7 passed the entire 812-test suite | **CLOSED.** The M7 lock is at `caldir-provider-caldav/src/caldav/ops/list_events.rs:448`, on the `wiremock` + `test_dav.rs` seam. |
| *"OPEN — `ProviderError::Provider` is untested"* | **CLOSED.** `caldir-core/src/provider.rs:263-266` `call_returns_provider_error_on_error_response`. |
| L1-07 carry-forward: undated-task slug collisions on `_task__{summary}.ics` are unproven | **CLOSED.** `caldir-core/src/calendar/event.rs:861` `two_undated_tasks_sharing_a_summary_get_distinct_files` — asserts `_task__someday-maybe.ics` and `_task__someday-maybe-2.ics`. |
| L2-08 / L7-05: doctor's *"same event saved as multiple files"* is inaccurate for a task pair | **CLOSED.** Two byte-exact locks: `duplicate_file.rs:253` keeps the event sentence untouched, `:286` gives tasks *"same task saved as multiple files"*. C13 unbreached. |
| Carried-forward: `CLAUDE.md`'s sync description is stale (says mtime) | **CLOSED.** `CLAUDE.md:36` now describes the three-way comparison against a stored base, with mtime / `LAST-MODIFIED` / `SEQUENCE` named as the fallback. |
| Carried-forward: `CLAUDE.md:52` names `ProviderHandler` at a path that does not exist | **CLOSED.** `CLAUDE.md:54` now names `Handler` at `caldir-core/src/provider/handler.rs`. |
| Carried-forward: `caldir-provider-caldav/CLAUDE.md` gives the wrong path for shared ops | **CLOSED.** It now says `src/caldav/ops.rs` (plus `src/caldav/ops/`). |
| L8 doc debt: `provider-contract.md` (15, 65, 173), `system-map.md:58`, `provider-matrix.md:128` describe the inbound response as `Vec<Event>` | **CLOSED.** All now say `Vec<CalendarItem>`, each annotating the L4-02 retype. |
| L8 doc debt: `tickets/L5-02.md:64`, `system-map.md:55,105`, `local-parser.md:66` cite `parse_event` / hazard A10 as a **live** defect | **CLOSED.** All now cite `parse_item` and mark A10 **FIXED (L5-01)**. |
| Carried-forward: `specs/caldir.md` lists `RDATE` as skipped though it is implemented | **CLOSED.** `specs/caldir.md:251` documents `RRULE` / `EXDATE` / `RDATE` together as implemented. |
| Q-6 (listing-incomplete signal) | **CLOSED** — see D-6. |
| Q-2 (`task_status` key name) | **CLOSED** — see D-2. |
| Q-3 (VTIMEZONE), Q-4 (`percent_complete`) | **CLOSED by measurement** — see D-3, D-4. |
| *"NEW for the 0.15.0 window: `WireValue::from_wire` should return `Result`"* | **STRUCK, not deferred** — L4-02's `ItemIcs` newtype made it unnecessary. The window is four items, not five. See B-4. |
| L5-05 M4b: `find_event_location`'s fallback masks a broken `event_url`; a dedicated unit lock would be stronger | **PARTIALLY CLOSED.** `caldav/client.rs:172` `event_url_joins_calendar_and_uid` and `:179` `event_url_handles_trailing_slash` do assert the href is purely UID-derived; a clock-derived href fails them. T-58 adds the outcome assertion. No further work identified. |

**Still open and correctly so** (do not mistake these for closed): B-4, B-5, B-6, B-9, B-11, B-12,
B-16, B-17, B-18 were each re-checked against the tree at 902/0 and are **not** done.

---

## §G — Disclosures the PR description owes (not deferrals; stated here so they are not forgotten)

1. **New dev-dependencies in `caldir-provider-caldav`**, both dev-only, neither ships in a binary:
   `wiremock = "0.6"` and `serial_test = "3"`. Justification for wiremock is **L5-01's mutation
   M7** — consuming only the first of the two REPORTs passed the **entire 812-test suite**, i.e.
   silent task loss with zero coverage. Constraint attached to the ruling: **caldav crate only**;
   do not add it to core or any other provider without a fresh ruling; prefer a pure function over
   a double where one suffices. The live tier additionally requires **`curl` on PATH**.
2. **An observable request change on the fast path:** `find_event_location`'s fast path was a
   `GetEtag` PROPFIND on the resource; it is now a `calendar-multiget` REPORT on the collection.
   Outcome-identical for events and locked as such, **but it is a different request on the wire.**
3. **R70 was breached, boundedly, and it is accepted:** L4-02 touched **5 provider files** (one
   adapter each in `caldir-provider-{caldav,google,icloud,outlook,webcal}/src/main.rs`). This is
   the **minimum possible** — `Handler::list_events` is a trait method and the only inbound
   channel, so any return-type change forces every implementor. **No provider logic changed**, and
   the 25 `cmd.event` sites R70 primarily protects remain untouched. A zero-breach alternative (a
   defaulted `list_items` delegating to `list_events`) was considered and **rejected**: two
   overlapping methods where overriding the wrong one silently drops data is a worse trap than five
   identical adapter lines. **State this honestly rather than claiming zero provider edits.**
4. **Tasks pull but do not push** (C-1), and **iCloud is unmeasured** (C-2). Say both plainly.
