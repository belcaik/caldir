# GRILL-FINDINGS: adversarial review of the nine VTODO ADRs

Reviewer role: skeptical architecture reviewer, whole-set reader. Nine ADRs were written
in parallel by agents that never saw each other. This document reports; it changes no
source and no ADR.

**New primary evidence gathered during this review** (none of the nine ADRs had it):

- A live **Radicale** instance was found running (`caldir-radicale-probe`, port 15232) and
  was used to settle several items every ADR marked UNVERIFIED. That container was torn down
  mid-review by the orchestrator's parallel probe, whose results
  (`docs/vtodo/context/radicale-probe.md`, Radicale 3.7.6, 8 fixtures) are the authority
  wherever we overlap. **My independent measurements agree with theirs on every point except
  one, where mine was wrong and is retracted below (see the C-3 postscript and F-1).**
  Where we agree, treat it as independent corroboration by two separately-authored probes.
- Every `icalendar` probe was re-checked for version pinning; one was wrong (see C-3) and
  was re-run pinned.

---

## Adjudication C-1 — `EventInstanceId` vs `ComponentKind`

**Ruling: ADR-02 wins the identity question. ADR-01 decision #3 and #7 must be struck.
But ADR-02's *stated rationale* is wrong, and its binding rule #4 is unimplementable.**

### Does ADR-02's evidence hold? Re-verified against real source.

I re-traced the diff mis-keying claim myself, as instructed, in
`caldir-core/src/diff/calendar_diff.rs`:

- `:20-23` `local_event_ids: HashSet<_>` from `e.event().event_instance_id()`
- `:25-28` `let remote_by_id: HashMap<_, &RemoteEvent> = remote_events.iter().map(|e| (e.event().event_instance_id(), e)).collect();`
- `:33` pass 1 iterates the **local** `Vec`; `:36` `if let Some(remote_event) = remote_by_id.get(&id)`;
  `:38` `if event == remote_event.event() { continue; }`; `:63-71` otherwise pushes
  `EventChange::Update`.
- `:93` pass 2 iterates the **remote** `Vec`; `:97` `if local_event_ids.contains(&id) { continue; }`.
- `:84` / `:110` delete inference is exactly `sync_bases.get(&id).is_some()`.

**The trace holds.** With a component-agnostic key, a local VTODO and a remote VEVENT
sharing a UID hash to the same key, so `:36` matches, `:38` fails, and `:63` emits an
`Update` that replaces one component with the other. Hazard 3 (double `Delete` from one
`sync_bases` entry, because pass 1 runs once per local file and `:84` is true both times)
also holds. Hazard 5 holds: `caldir-cli/src/commands/doctor/warning/duplicate_file.rs:14-20`
buckets by `EventInstanceId` and warns on `paths.len() > 1`. Hazards 1 and 4 hold by
inspection of `state/sync_bases.rs:12,43,60` and `state/event_bases.rs:29`.

**ADR-01 #7 does not answer four of the five.** Its mitigation ("a base's ICS names its own
component, so a kind-mismatched base is treated as no base") touches hazard 4 only. It is
silent on the `known_event_ids` collapse, the `remote_by_id` collision, the double delete,
and the doctor warning. As written it is not a competing solution; it is an unaddressed gap.

### But ADR-02's motivating story is false, and I can now prove it.

ADR-02 justifies the change with "a Radicale collection may legitimately hold both
components [sharing a UID]" and binds rule #4: *"CalDAV `event_url` must ... keep `{uid}.ics`
for VEVENT; kind-distinct href for VTODO."* Both claims fail against a real Radicale
(collection `/testuser/grill/`, all output real):

```
PUT event {uid}.ics                    = 201
PUT todo  {uid}.ics (If-None-Match: *) = 412
PUT todo  {uid}.ics (unconditional)    = 204     <- the VEVENT is GONE
GET dual-9.ics -> BEGIN:VTODO ... UID:dual-9 ... END:VTODO   (event replaced)

PUT todo, same UID, DIFFERENT href (dual-9-todo.ics) = 409 Conflict
PUT one resource holding VEVENT+VTODO                = 400 Bad Request
```

So: (a) ADR-01's open sub-question 1 and ADR-02's open sub-question 1 are **answered — the
second PUT does overwrite**; (b) Radicale enforces **UID uniqueness per collection across
component kinds**, so ADR-02 rule #4's kind-distinct href gets a **409** — the rule is
unimplementable against the MVP's own target server; (c) the "legitimate server-controlled
mixed-UID collection" that ADR-02 rejects O1 over **cannot exist on Radicale**.

The orchestrator's independent probe (Radicale 3.7.6) reached the identical three results —
`409 <C:no-uid-conflict/>` across paths, `400 "Multiple component types in object"` within one
resource, silent `204` at the same path — from a separately-authored fixture set. Two
independent measurements agree; treat this as settled for Radicale.

### Ruling — severity re-derived from the local case alone

I was asked not to defer to ADR-02 now that its headline premise is gone, so I re-derived
this from scratch. With the remote trigger removed, the only reachable triggers are:

- a **local mixed directory** — nothing stops it: `Calendar::events()` globs `*.ics` by
  extension (`calendar.rs:113`) and ADR-03 Decision 1 deliberately makes a directory able to
  hold both kinds. reproduction.md:14-33 confirms the u_crawler pair really does share
  `assignment-123@u-crawler`; today they live in two directories, and a user consolidating
  them is one `mv` away.
- **two caldir calendars**, whose `known_event_ids` are per-calendar and therefore safe.

So the honest severity is: **user-triggered, not server-triggered — but silent and
destructive when triggered.** Hazard 2's outcome is an `EventChange::Update` that replaces
one component's content with the other's, on a file the user authored. Hazard 5 (`doctor`
telling the user to delete one of two legitimate files) needs no UID coincidence to be
confusing, only the mixed directory.

**Verdict: `ComponentKind` is still justified, and I could not construct a cheaper fix.**
The cost is one enum field plus one prefix rule; the alternative (ADR-02's O1, forbid mixed
calendars) requires a validation pass across `calendar.rs`, `connection.rs`, `config.rs` and
the CLI — strictly more code, to *remove* a capability ADR-03 deliberately grants. I rule
for ADR-02 on the surviving evidence, not on its retracted premise.

Required changes:

1. **ADR-01**: delete decision #3's "`EventInstanceId` remains the single, unchanged key
   type" and delete decision #7 entirely. Replace with a pointer to ADR-02.
2. **ADR-02**: strike binding rule #4. Replace with the verified fact: a CalDAV collection
   cannot hold a same-UID pair (Radicale 409/204), so caldir must **surface a push error**
   for a local same-UID pair rather than invent an href scheme. Rewrite the Context
   paragraph that claims mixed-UID collections are legitimate; the justification is the
   local directory, full stop.
3. **ADR-02** must also close its own open sub-question 2 (prefix spelling) before any
   state file is written. See BLOCKER B-4 below — the prefix collides with ADR-09 C4.

---

## Adjudication C-2 — the broken ADR-04 -> ADR-05 dependency

**Ruling: the condition fails, and the damage is wider than ADR-04's three named rows.
`ATTACH`, `ATTENDEE`, `ORGANIZER`, `LOCATION` and `RRULE/EXDATE/RDATE` flip IN.
`CATEGORIES` stays OUT. This is not a close call — the fix is nearly free.**

### Is ADR-05's mitigating claim true?

ADR-05's implied comfort is that an unmodelled property survives on the server until the
first genuine push of that item. **I traced it and it is true.** After a pull, the local
file, the sync base (`state/event_bases.rs:34`, re-serialized from the domain type) and the
re-parsed remote item all lack the property equally, so `calendar_diff.rs:38`
`event == remote_event.event()` short-circuits and nothing is pushed. The property sits on
the server indefinitely.

**But the moment anything about that task genuinely changes, it is destroyed.** CalDAV
update is a whole-resource PUT (`caldir-provider-caldav/src/caldav/ops/update_event.rs:33-38`,
body = `event.to_ics_string()`), so the server's copy is replaced by caldir's lossy
re-serialization. For `ATTACH` this is exactly RFC 8607's delete semantics.

### Blast radius, quantified against a real Radicale

Radicale is a **faithful store** — it preserves every one of these verbatim. Real PUT/GET
round-trip of one VTODO (only difference: Radicale sorts properties alphabetically, which
is harmless to caldir):

```
ATTACH:https://x/y.pdf
ATTACH:https://x/z.pdf                       <- both survive
ATTENDEE;PARTSTAT=NEEDS-ACTION:mailto:a@b.c  <- params survive
CATEGORIES:WORK,URGENT                       <- UNESCAPED comma survives
RELATED-TO;RELTYPE=PARENT:parent-1           <- survives
X-APPLE-SORT-ORDER:12                        <- survives
STATUS:COMPLETED without PERCENT-COMPLETE    <- accepted, HTTP 201
```

The orchestrator's probe sharpens this further (Q-F2): Radicale preserves **both** `ATTACH`
values *and* **both** `RELATED-TO` values with `FMTTYPE`/`RELTYPE` params intact. So every
collapse-to-last is 100% `icalendar` 0.17.10 / caldir's parser, with zero server
contribution — meaning **anything caldir drops is caldir destroying data the server was
holding correctly.** No shared-limitation excuse is available.

So every property caldir drops is a property the server was faithfully holding. The loss is
real, silent, and irreversible for: an Apple Reminders task assigned to someone
(`ATTENDEE`), a task with attachments (`ATTACH`), and — the row nobody flagged — **every
recurring task**, because ADR-04's `RRULE/EXDATE/RDATE` row literally says *"A05 preserves
them ... No loss."* That sentence is false; a recurring task's RRULE is destroyed on the
first edit.

### Cost of flipping IN: near zero, because the code already exists

`caldir-core/src/event/to_icalendar.rs` already emits all of them, tested:
`:76-78` `for attendee in &value.attendees { event.append_multi_property(...) }`;
`:87-89` the same for `attachments`; `:72-74` `organizer`; `:56` `location`;
`:37-39` `recurrence.apply_to(&mut event)`. Every one of these takes an `EventLike`, which
`icalendar::Todo` implements (`components.rs:664-665`). **ADR-01 decision step 3 already
puts all five on `Todo`** as part of its "15 genuinely shared fields". ADR-04 is the only
document saying OUT, and it says so on a premise that does not hold.

I verified `ATTACH` x2 survives icalendar 0.17.10's own parse+emit:

```
BEGIN:VTODO ... ATTACH:https://x/y.pdf / ATTACH:https://x/z.pdf / CATEGORIES:WORK\,URGENT
```

### Why `CATEGORIES` stays OUT

Three independent reasons, and the orchestrator was right not to want a reflex flip:

1. **Modelling it through the crate corrupts it.** Verified on pinned `=0.17.10`:
   `CATEGORIES:WORK,URGENT` re-emits as `CATEGORIES:WORK\,URGENT`. Two categories become
   one for every third party. Flipping it IN via the typed path is *worse* than dropping it.
   A correct implementation needs a hand-rolled emitter — real cost, unlike the other five.
2. **`specs/caldir.md` skips CATEGORIES for events.** A todo-only divergence in a table
   titled "Fields We Skip" is incoherent: the same user, same tool, same table, opposite
   answer by component. Either both or neither.
3. Radicale holds it faithfully, so the "survives until first genuine push" mitigation gives
   real breathing room to defer it to a follow-up that fixes it for `Event` too.

### Required changes

| ADR-04 row | Was | Becomes | Why |
|---|---|---|---|
| `ATTACH` | OUT | **IN** | Existing codec; RFC 8607 delete-on-omit; Radicale holds it |
| `ATTENDEE` | OUT | **IN** | Existing codec; unassigns people server-side |
| `ORGANIZER` | OUT | **IN** | Existing codec; free once ATTENDEE is in |
| `LOCATION` | OUT | **IN** | Existing codec; ADR-01 already has it |
| `RRULE`/`EXDATE`/`RDATE` | OUT ("no loss") | **IN (modelled, not expanded)** | The "no loss" claim is false; existing `Recurrence` type |
| `PERCENT-COMPLETE` | OUT | **IN** | ADR-01 #3 and ADR-06's equality table both already model it |
| `CREATED` | OUT (“informational”) | **IN** (`PartialEq(ignore)`) | Not fidelity — **data safety**. It is the property that rescues an undated completed task from RFC 4791 §9.9 omission. See M-6. |
| `CATEGORIES` | OUT | **stays OUT** | Crate corrupts it; spec coherence; follow-up ADR for both components |

ADR-04 must additionally **delete every "A05-safe" / "A05 preserves it" / "No loss"
justification from its OUT column** and replace them with ADR-05's actual one-sentence
guarantee. ADR-05's guarantee sentence is correct as written and needs no change.

---

## Adjudication C-3 — probe provenance

**Ruling: one of four load-bearing probes measured the wrong crate. Its conclusion
nevertheless survives re-running pinned. Documentation must be corrected; the decision
stands.**

Pinning audit of every scratchpad probe (real `Cargo.lock` contents):

| Probe | Used by | Declared | **Actually resolved** | Verdict |
|---|---|---|---|---|
| `vtodo-probe` | `context/icalendar-capabilities.md` | `= 0.17.10` (path dep) | **0.17.10** | OK |
| `rt` | ADR-05 round-trip | `= 0.17.10` | **0.17.10** | OK |
| `exp` | ADR-01 `Option<EventTime>` check | workspace copy | **0.17.10** | OK |
| `valarm-probe` | **ADR-04 Q5, VALARM IN** | `icalendar = "0.17.10"` (caret) | **0.17.13** | **WRONG CRATE** |

ADR-04:85 states *"icalendar 0.17.10, default features (same as caldir)"*. That statement is
false: `<scratchpad>/valarm-probe/Cargo.lock` contains `name = "icalendar" / version = "0.17.13"`.
The caret requirement resolved forward — the same trap ADR-05 documented and ADR-04 walked
into.

**Re-run, pinned to `=0.17.10` via the extracted source** (`valarm-probe-1710`, real output):

```
=== PATH A (crate serializes the VALARM child) ===
BEGIN:VALARM / DTSTAMP:20260825T055341Z / ACTION:DISPLAY / DESCRIPTION:Reminder
TRIGGER;RELATED=START:-PT30M / UID:6a74eb0f-13c6-4a04-bf83-9da7f0236b23 / END:VALARM
A stable across two serializes? false
PATH A: VALARM UIDs -> Some("6a74eb0f-...") vs Some("13e86ad3-...")

=== PATH B (hand-rolled splice on "END:VTODO\r\n") ===
extracted reminder minutes = Some(30)
BEGIN:VALARM / ACTION:DISPLAY / DESCRIPTION:Reminder / TRIGGER;RELATED=START:-PT30M / END:VALARM
B stable across two serializes? true
gen1 == gen2 bytes? true      gen2 == gen3 bytes? true
reminder survives 3 generations? Some(30) Some(30) Some(30)
bytes equal WITHOUT dtstamp? true
```

**Path B byte-stability holds on the locked crate.** VALARM stays IN; the mechanism (splice,
never `EventLike::alarm`) is confirmed. Required change: ADR-04 must correct its provenance
line and cite the pinned re-run, and the repo should adopt a rule that every probe crate
pins `=` the locked version.

Two further facts I confirmed on pinned `=0.17.10` while I was there:

- **ADR-04's `DTEND` FORBIDDEN row is real.** `Todo::ends(...)` emits
  `BEGIN:VTODO / DTSTAMP:... / DTEND:20260103T100000Z / UID:t2 / END:VTODO`. Keep the test.
- **ADR-03's open sub-question 1 is ANSWERED.** A document with `VEVENT`, `X-FOO`,
  `VJOURNAL`, `VTODO` yields `components.len() == 4` and dispatches as
  `Event / Other(X-FOO) / Other(VJOURNAL) / Todo`, with `component_kind()` naming each. So
  `NoSupportedComponent { found: String }` is implementable exactly as ADR-03 specifies.

### Postscript: a probe of my own that was wrong, and how it was caught

I take my own medicine. Mid-review I reported that ADR-08's §9.9 hazard did not reproduce on
Radicale, because my time-ranged VTODO query returned an `undated-completed-old` fixture the
RFC says should be omitted. **That was my bug, not Radicale's.** I built the fixture with
`printf '...%s...' "STATUS:COMPLETED\\r\\nCOMPLETED:20200101T000000Z"`; bash `printf` does not
expand escapes inside a `%s` argument, so the literal characters `\r\n` were embedded and the
task was stored with **no `COMPLETED` property at all** — a fully undated task, which §9.9
correctly returns. Verified after the fact with `cat -A` against the stored resource:

```
STATUS:COMPLETED\r\nCOMPLETED:20200101T000000Z^M$      <- one malformed line; no COMPLETED
```

The orchestrator's probe used eight properly-formed fixtures and matched all eight §9.9 rows.
**ADR-08's finding is CONFIRMED; my contrary finding is retracted** (F-1 in the table).

The methodological point is the same one C-3 makes about ADR-04, and it is worth adopting as a
project rule: *a probe that returns the answer you expected is not evidence until you have
inspected the input it actually sent.* ADR-04 did not check which crate it resolved; I did not
check which bytes I PUT. Both errors were invisible in the output.

---

## Findings

| id | sev | ADR(s) | Problem | Required change |
|---|---|---|---|---|
| **B-1** | BLOCKER | 01, 02 | Direct contradiction on the key type (C-1). The set is unimplementable as written. | Strike ADR-01 #3/#7; strike ADR-02 rule #4 and its mixed-UID-collection rationale. |
| **B-2** | BLOCKER | 04, 05 | ADR-04's OUT column rests on a passthrough that does not exist (C-2). Silent, irreversible server-side destruction of `ATTACH`, `ATTENDEE`, and **every recurring task's RRULE** on first edit. | Flip ATTACH/ATTENDEE/ORGANIZER/LOCATION/RRULE/EXDATE/RDATE/PERCENT-COMPLETE to IN; keep CATEGORIES OUT; delete all "A05-safe / No loss" justifications. |
| **B-3** | BLOCKER | 01, 03, 09 | ADR-01 step 5 says the component dispatch is `Event \| Todo \| _ => skip`. ADR-03 rule 2 says an unsupported component "is an error, never a silent drop". ADR-09 **C10 (INVIOLABLE)** says an unknown component "must be a hard error, never a silent drop or an empty vec", because a silent drop feeds C7's delete path. ADR-01 breaches an INVIOLABLE row. | Rewrite ADR-01 step 5: the `_` arm must **collect the component kind and error**, per ADR-03's `NoSupportedComponent { found }` (proven implementable — see C-3). |
| **B-4** | BLOCKER | 02, 09 | ADR-02's `VTODO:` id prefix breaches ADR-09 **C4 (INVIOLABLE)**, whose test is spelled *"assert the file body matches `^[^\n]+$` **with no new sigil**"*. The two documents cannot both ship. | Adjudicated in ADR-02's favour: without a per-kind string, `known_event_ids` collapses the pair (hazard 1) and there is no fix. **ADR-09 C4 must be amended** to permit a kind prefix, its assertion rewritten to "existing event-form lines are byte-identical", and C4 downgraded from INVIOLABLE to INVIOLABLE-for-event-form. ADR-02 must fix the prefix spelling in the same edit. |
| **B-5** | BLOCKER | 01, 03, 04, 08, 09 | **No ADR owns CLI rendering, and C12 forbids the MVP's own fixture.** ADR-09 **C12 (INVIOLABLE)**: `start: String` stays non-optional, so *"an item with no DTSTART must not enter `AgendaView`"*. `caldir-cli/src/output/agenda_json.rs:30-40` builds `AgendaJson` from `EventJson<'a>`. The reproduction fixture (reproduction.md:14-28) is **DUE-only, no DTSTART**, and ADR-08 goes further and makes fully undated tasks first-class. As specified, the MVP syncs tasks that cannot be displayed. ADR-01 punts this to a "CLI-facing ADR" that does not exist. | Add a tenth ADR (or extend ADR-03) that decides the agenda/`--json` shape for tasks, additively per C12. This is a gap in the *set*, not a defect in any one ADR. |
| **M-1** | MAJOR | 09, 07 | ADR-09's "two-direction wire verdict" says new-core+old-provider is *"a per-change failure recorded by `connection.rs:84,114`, no data loss"*. Wrong: `push_outgoing_changes` at `connection.rs:231` is `remote.apply_change(change).await?` — the `?` **aborts the rest of that calendar's push**. ADR-07 Context 5 states this correctly. ADR-09 understates its own risk. | Correct ADR-09's verdict paragraph to match `connection.rs:231`. |
| **M-2** | MAJOR | 01, 09 | The RPC method structs carry a **typed `Event`**, not opaque text: `rpc/methods/delete_event.rs:6-10` is `pub event: Event` with `#[serde(with = "crate::rpc::ics")]`; `ListEvents`'s response is `Vec<Event>`. ADR-01's blast radius names `rpc/{ics,wire}.rs` but **not** `rpc/methods/*.rs`. These fields must become `CalendarItem` or a todo cannot be created, updated or deleted at all. | ADR-01 must add `rpc/methods/{create,update,delete}_event.rs` and `list_events.rs` to its blast radius and state that the typed field changes while the JSON stays a string (C8/C9 unaffected). |
| **M-3** | MAJOR | 02 | ADR-02 rule #4 changes only `create_event`. But `update_event.rs:29` and `delete_event.rs:24` resolve the href via `find_event_location` (`caldav/ops.rs:50-70`), which tries `{uid}.ics` **first** and falls back to a REPORT whose filter is hard-coded `<C:comp-filter name="VEVENT">` (`ops.rs:107`). A todo update/delete therefore targets a VEVENT resource, or fails to find its own. Also: rule #4 leaks core's identity model into a provider. | With rule #4 struck (C-1), ADR-02 must instead require the caldav provider to component-parameterise `find_event_location`'s REPORT, and ADR-08 Decision 2's "`ops.rs:107` needs only the component-name treatment" must say the *fast path* needs it too. |
| **M-4** | MAJOR | 01, 04, 06 | Three ADRs specify three different `Todo` field sets. ADR-01 #3 has `percent_complete`; ADR-04 says OUT. ADR-06's equality table has `percent_complete` (participates) and `attachments`; ADR-04 says both OUT. ADR-01 shares `location`/`organizer`/`attendees`/`attachments`/`recurrence`; ADR-04 says all OUT. | Resolved by C-2: adopt ADR-01's field set. ADR-04's table shrinks to genuinely todo-specific decisions (DUE, COMPLETED, STATUS, PRIORITY, PERCENT-COMPLETE, DTEND-forbidden). |
| **M-5** | MAJOR | 04 | Provenance of the VALARM probe is false (C-3). Every "0.17.10" claim in ADR-04 is unsourced until re-checked. | Correct ADR-04:85 and cite the pinned re-run above. Adopt a repo rule: probe crates pin `=<locked version>`. |
| **F-1** | *(RETRACTED — see postscript to C-3)* | 08 | I initially reported that ADR-08's time-range hazard did not reproduce on Radicale. **That finding was based on a malformed fixture of my own and is withdrawn.** ADR-08's claim is CONFIRMED. | None. ADR-08 Decisions 1+2 stand, jointly and in one commit. |
| **M-6** | MAJOR | 05, 08, 04 | **ADR-05 x ADR-08 interaction: caldir's own writes manufacture the vulnerable shape.** The §9.9 row that omits a task is `COMPLETED`-only; a task carrying `CREATED` is rescued by `end > CREATED`. ADR-05 decision item 4 says *"Do not capture `DTSTAMP`, `CREATED`, or any other non-`X-` property"*, and ADR-04 marks `CREATED` OUT as "informational". So every completed, undated task caldir has ever *written* has lost its `CREATED` and is now in the one shape a compliant server will omit. No ADR considered `CREATED` on data-safety grounds. | Model `CREATED` on `Todo` as `Option<DateTime<Utc>>`, `#[educe(PartialEq(ignore))]` (stable, so no churn; ignored so a producer that omits it cannot cause drift). ADR-05 needs an explicit carve-out for it. If it is *not* modelled, ADR-08 must instead land a named test at `ops/list_events.rs:103` asserting the VTODO REPORT body contains no `time-range`, with a comment naming this interaction — the invariant must be enforced by something, not by memory. |
| **M-7** | MAJOR | 03, 05, 09 | **Radicale returns a sibling `VTIMEZONE` for any `TZID`**, so a real CalDAV task payload is a **multi-component** `VCALENDAR` on the wire, not just on disk. ADR-03 rule 2 exempts `VTIMEZONE` from counting but states it for the *local file model*; rule 3 puts the same arity pair on `EventError`, which is what `rpc/ics.rs:34` uses. If the wire-side arity counts `VTIMEZONE`, **every timezone-bearing task from Radicale fails to decode**. Separately, caldir's serializer emits no `VTIMEZONE`, so the first GET after a push structurally differs from what was PUT. | ADR-03 must state the `VTIMEZONE` exemption explicitly for `EventError` / the wire, not only for the disk model, with a test feeding a `VTIMEZONE`+`VTODO` payload through `rpc::ics::deserialize`. ADR-05 or ADR-04 must decide whether caldir emits `VTIMEZONE` or tolerates the server adding it. |
| **M-8** | MAJOR | 02, 09 | **`409 <C:no-uid-conflict/>` on push is unowned.** A user renaming a local `.ics` changes the slug but not the UID; `create_event.rs:26-38` then PUTs to a new `{uid}.ics` href with `If-None-Match: *` while the old resource still holds that UID — exactly the pattern Radicale rejects with 409. No ADR mentions it. | Assign it. Cheapest correct behaviour: on 409, fall back to `find_event_location` + conditional PUT (i.e. treat it as an update), which is what the rename actually is. |
| **M-9** | MAJOR | 04 | Radicale preserves **both** `RELATED-TO` values with their `RELTYPE` params (probe Q-F2). So ADR-04 Q6's collapse-to-last is 100% caldir's parser destroying data the server held correctly — not a shared limitation. ADR-04 files the mitigation as *"Cheap mitigation I recommend but do not design here"*. Given the server is provably faithful, an undesigned recommendation is not enough. | Promote it to a binding decision in ADR-04: count `RELATED-TO` lines in the raw text pre-parse and refuse to rewrite that file (doctor warning), or model it. Do not leave it as a suggestion. |
| **m-1** | MINOR | 03 | Open sub-question 1 is now answered (see C-3). | Replace it with the verified result. |
| **m-2** | MINOR | 02 | Open sub-question 3: should `ComponentKind` be `#[non_exhaustive]`? | No. It forces `_` arms across core for a two-variant enum in one crate that owns all matches. ADR-02's leaning is right; record the answer. |
| **m-3** | MINOR | 07 | Open sub-question 1 answered twice over: Radicale **does** advertise `supported-calendar-component-set` per collection (my run: `default` -> `VTODO, VEVENT, VJOURNAL`; `tasksonly` -> `VTODO`), and the orchestrator's probe adds that it is bulk-readable in one Depth-1 PROPFIND on the home-set — which removes the per-calendar cost objection — but is **advisory only**: a VEVENT PUT into a VTODO-only collection returned 201. | Delete the fallback question. Vindicates `Vec<String>` over a closed enum — a real server really does return `VJOURNAL`. The advisory-ness does not weaken ADR-07's design (the gate is client-side in `Connection::diff`), but it does mean the declared value can be wrong, so it must never be treated as a guarantee. |
| **m-4** | MINOR | 03 | ADR-03 option B's motivation ("matches what CalDAV servers may legitimately PUT") is falsified: Radicale rejects a VEVENT+VTODO resource with **400 Bad Request**. | Record it; it strengthens Decision A. |
| **m-5** | MINOR | 06, 04 | Open sub-question 1 answered: Radicale preserves all values verbatim but **re-serializes** — alphabetical property order, re-folding at 75 octets, `VTIMEZONE` injection — and is a fixed point after one pass. No injected SEQUENCE/CREATED/LAST-MODIFIED, no reformatted timestamps. Also `STATUS:COMPLETED` without `PERCENT-COMPLETE` is accepted (201) and never repaired, settling ADR-04's open question and validating its "inconsistency, not loss" framing. | Record; drop both open questions. **Add one constraint:** ADR-06 scenario 8 and any other end-to-end acceptance gate must compare **parsed domain `==`** or GET-pass-2 against GET-pass-1 — never raw bytes against what was PUT. ADR-04's must-test (1) is local-only and is unaffected. |

---

## Sustained — attacked, could not break

- **ADR-01's `CalendarItem` enum.** I tried hard to kill it. The deletion test says it earns
  its keep: `SyncBases` is one heterogeneous `HashMap` (`state/sync_bases.rs:12`) and the
  disk layer yields a mixed `Vec` from one directory, so *something* must be a sum type at
  that seam. `dyn` loses the educe `PartialEq` that literally decides sync direction.
  Justified. Note the interface stays small (~6 methods), which is the right shape.
- **ADR-01's rejection of widening `Event`.** The *measurement* is weak — it counted 41
  first-wave compile errors without fixing any, and compilation aborted before the CLI, so
  the number is neither a floor nor a ceiling. But the decision is right on a stronger
  argument ADR-01 also makes: widening makes `Event{kind:Event, due:Some(..)}` representable,
  which breaches the mission constraint. Keep the decision, drop the 41-errors framing.
- **ADR-04's `TodoStatus` as a separate enum.** Merging into `Status` would add impossible
  variants at every VEVENT match site and change the public `--json` enum (C12). Correct.
- **ADR-04's "both skip-rules key on `Cancelled` only".** The reasoning is decisive and I
  confirmed the fixture is `STATUS:COMPLETED` (reproduction.md:23): sharing the rule at
  `calendar_diff.rs:106` would make the MVP's own fixture never pull. Right.
- **ADR-04's VALARM = IN, via the hand-rolled splice.** Survived re-running on the correct
  crate (C-3). The existing precedent is real: `event/to_icalendar.rs:80-83` comments that
  reminders bypass `event.alarm(...)` for exactly this reason.
- **ADR-05's O1 (match `Event` exactly).** Two fidelity models inside one codec would be
  worse than one honest, documented loss. The guarantee sentence is unusually good. Its only
  fault is that ADR-04 leaned on it for things it never claimed.
- **ADR-06's "DTSTAMP: not-a-field beats ignored-field".** Verified: `Event` has no
  `dtstamp`; `from_icalendar.rs:60-65` captures only `X-`. The reasoning that a field would
  ride into base files and the wire codec is correct.
- **ADR-06's Push-bias verdict.** Traced `local_is_newer` (`calendar_diff.rs:178-190`)
  myself; `(Some(_), None) => true` is real and is pre-existing VEVENT behaviour. Preferring
  wrong-Push over wrong-Pull is right for a plaintext-source-of-truth tool.
- **ADR-07's `Vec<String>` over a closed enum.** Vindicated by evidence ADR-07 did not have:
  Radicale really returns `VJOURNAL`. A closed enum would hard-fail on a live server today.
- **ADR-08 Decisions 1 and 2, as one decision.** I attacked this hardest of all and was
  wrong (C-3 postscript). It is now the best-evidenced decision in the set: eight fixtures,
  all eight §9.9 rows matched by a stock Radicale 3.7.6, with `t2` (undated,
  `COMPLETED:20200101T000000Z`) omitted under time-range and returned without it. The
  data-loss path is empirically real. **They must ship in the same commit**, and ADR-08 says so.
- **ADR-03's Option A (one supported component per file).** Independently required by the
  server, which nobody knew when ADR-03 was written: Radicale answers `400 "Multiple
  component types in object"` to a VEVENT+VTODO resource. Option B was arguing for a shape the
  target server refuses.
- **ADR-08's separate REPORT rather than sibling comp-filters.** §9.7.1's "all match"
  language is genuinely ambiguous and two REPORTs guarantee zero VEVENT regression for the
  cost of one function. Cheap insurance; keep.
- **ADR-08's "no synthesized dates" rule.** The trace is right: an epoch start evaluates
  false at `calendar_diff.rs:80` and would silently make undated tasks unpushable.
- **The ADR-07 / ADR-08 seam.** I looked for double-filtering and zero-filtering and found
  neither. ADR-07 partitions tasks out *before* `CalendarDiff::compute`, so an unsupported
  task is never keyed at all; ADR-08 Decision 3 then trivially sees no local tasks. In the
  dangerous direction — config says VTODO but an older binary returns none — ADR-07's gate
  passes and ADR-08's belt catches. They compose as complements, not as duplicates.
- **VJOURNAL extensibility.** `CalendarItem` + `ComponentKind` + one struct + one match arm.
  The enum does not lock anything in; ADR-01 #8 is correct.
- **ADR-09's C7** (any unreadable file suppresses all deletes for that calendar) is the
  sharpest idea in the whole set, and its argument that the "risk-free" poison-pill fix is
  the one that can delete a user's tasks is exactly right.
- **Leaky provider details in core.** I went looking and found only one: ADR-02 rule #4,
  which is struck anyway (M-3). ADR-07 Decision 5 correctly confines
  `supported-calendar-component-set` to inside the caldav provider. Core stays clean.

---

## Overbuilt — cut from the MVP

**Primary cut: ADR-07's capability mechanism, in whole.**

It costs a `CalendarConfig` field, a builder method, edits to all five provider crates, a
third bucket on `CalendarDiff`, a new renderer branch with pluralization and
"… and {n} more", a fix to `render_bidirectional`'s early return, six tests, and two
unresolved sub-questions. What it buys is a **nicer error message for Google and Outlook —
providers ADR-04, ADR-07 Context 6 and ADR-08 all place outside the MVP.**

And its default actively harms the MVP's real users. ADR-07 Decision 2 is "absent = does
not support VTODO", and Context 8 establishes the value is read from `.caldir/config.toml`
on disk. So **every existing caldav and iCloud user gets zero task support after upgrading
until they re-run `connect`** — silently, with a warning that tells them to move or delete
their files. That is a worse outcome than the error message it was designed to prevent.

The orchestrator's probe removes one objection to ADR-07 that I never made — the whole
home-set's component sets are readable in one Depth-1 PROPFIND, so discovery is cheap. It
also adds one: the property is **advisory**, not enforced (a VEVENT PUT into a VTODO-only
collection returned 201), so the declared value can simply be wrong. Neither touches my
actual argument, which is about the default and the lifecycle, not the cost.

*Consequence of cutting it:* a task in a Google-backed calendar produces
`expected 1 event(s) in ICS, found 0` and aborts the rest of that calendar's push
(`connection.rs:231`), every sync. Ugly, recurring, but **not data loss** — ADR-08
Decision 3 already prevents the only destructive variant, and ADR-09 C7 covers the other.
If the ugliness is unacceptable, the *minimum* version is: keep the `Option<Vec<String>>`
field and the one-line gate in `Connection::diff`, and cut the third bucket, the four-line
warning renderer and the `is_empty()` special case down to a single stderr line. Do not
ship the full mechanism for the MVP.

Secondary cuts, smaller:

- **ADR-02's "not decided here" `Event*` renaming follow-up** — correctly deferred; keep it
  deferred and delete the paragraph so nobody re-opens it.
- **ADR-03's `NoSupportedComponent { found: String }` on `EventError`** (the wire/base twin,
  as opposed to the disk one). Base files are always written by caldir, and the wire case is
  already covered by C10's hard error. One variant, not two, on `EventError`. ADR-03's own
  "must move together" constraint is about the *arity rule*, not about mirroring every
  variant.
- **ADR-08's Decision 3 warning text** — ship the skip, defer the wording (ADR-08 already
  suggests this; make it explicit).

---

## Residual risk after all changes

1. **Only one CalDAV server has ever been tested, but it has now been tested twice.**
   Radicale 3.7.6 is well covered: two independently-authored probes agree on
   UID-uniqueness-across-components (409), multi-component rejection (400), byte-fidelity,
   component-set advertisement, and §9.9 conformance. **iCloud (Apple Reminders) is the gap.**
   It rides the same ops with no mapping layer and may answer every one of these differently —
   in particular whether it 409s a same-UID pair (load-bearing under C-1), whether it
   implements §9.9 (load-bearing under ADR-08), and whether it injects non-`X-` properties
   (load-bearing under ADR-05). *Settled by:* running the same six probes against a real
   iCloud account before Stage 8. Nothing else will do it.
2. **The fixture is still synthetic.** Whether real u_crawler VTODOs carry `LAST-MODIFIED`
   decides whether ADR-06's Push-bias fallback is on the hot path or never reached; whether
   they carry `DTSTART` decides ADR-01's open sub-question 2. *Settled by:* the Stage-8
   fixture swap — grep the real bytes.
3. **The CLI surface is undesigned (B-5).** Until it exists, nobody knows whether C12 can be
   satisfied additively or whether `AgendaEntry` must become item-typed — which would change
   ADR-01's blast radius. *Settled by:* writing the tenth ADR before implementation starts.
4. **No end-to-end run exists.** Every ADR reasons about `CalendarDiff::compute` statically.
   ADR-06 scenario 8 (sync twice, assert the second diff is empty) is the only thing that
   will actually prove the composition, and it has not been run. *Settled by:* building that
   test first, against the live Radicale that is already running.
5. **`VTIMEZONE` on the wire is untested end to end** (M-7). It is now known that Radicale
   returns a sibling `VTIMEZONE` for any `TZID`, but no test anywhere feeds such a payload
   through `rpc::ics::deserialize`, and caldir's serializer emits none — so the very first
   push/GET cycle for a zoned task is structurally asymmetric in a way no ADR modelled.
   *Settled by:* one decode test plus one round-trip against a zoned fixture. Cheap; do it early.
6. **`caldir update` atomicity** (ADR-09 open sub-question 3) is still unread, and it decides
   how wide the mixed-version window is — which is the entire justification for whatever
   survives of ADR-07. *Settled by:* reading `caldir-cli/src/commands/update.rs` and
   `install.sh`. Ten minutes; nobody has done it.
