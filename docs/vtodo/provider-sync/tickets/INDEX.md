# Provider write sync — rulings, wave plan, ticket definitions

**Scope:** this file is the binding record and the DAG. Ticket **bodies** live beside it as
`P<wave>-<nn>.md`. A row in §4 is a definition, not a body.

**Binding inputs:** `docs/vtodo/provider-sync/spec.md` (§§1–27; acceptance IDs `CALDAV-1..8`,
`GOOGLE-1..13`, `CROSS-1..6`) and §3 of this file, which **overrides the spec wherever they
conflict**.

**Prerequisite documentation, authoritative and not duplicated here:** `docs/vtodo/spec.md`
(R1–R108, the shipped VTODO core), `docs/vtodo/adr/ADJUDICATION.md`,
`docs/vtodo/SESSION-LOG.md` §8 (known gaps). Link back to those; never restate them.

**Traceability handle:** cite the spec's acceptance IDs. This effort coins **no** new requirement
numbering — R1–R108 belong to the shipped core and are not extended here.

**Sizing contract:** every ticket is scoped to ≤ ~4 closely-related files and 2–4 modules of
required understanding. Where a ticket exceeds it, its row says so.

---

## 1. Approved decisions

Ratified by the maintainer. Binding on every ticket. **Do not reopen one without evidence from
implementation that directly invalidates it.**

**D-1 — the upstream PR split is accepted.** Provider-sync work develops against `vtodo-support`
HEAD, but no ticket may assume the VTODO core ships as one PR. Every ticket declares which **core
capability** it depends on, in the terms of §2's spine, never "whatever is on the branch".
Rebasing onto the eventual core PR sequence is expected and must stay mechanical. Implementation
does not wait for the split to physically happen. Artificial coupling to unrelated VTODO-core work
is a defect.

**D-2 — a dedicated `capabilities` RPC method.** Capabilities are a runtime property of the
installed provider binary, so they are fetched at execution time and never persisted:
`ConnectResponse` is setup-time and would go stale on provider upgrade. The shape:

```
ProviderCapabilities {
    events: { read: bool, write: bool },
    tasks:  { read: bool, write: bool },
}
```

**Per-remote, not per-binary.** The `capabilities` request flattens `RemoteConfigParams` the way
`ListEvents` does. One provider binary serves remotes with different answers — a Google Calendar
and a Google Tasks list are the same binary — so a binary-wide reply can only ever return the
conservative floor. Providers that do not vary by remote ignore the params.

**Fail closed.** When capabilities cannot be obtained — error, timeout, unknown method, absent
field — caldir does not push tasks, does not read absence as support, preserves local data, and
surfaces an actionable provider/protocol error. Because the new `Method` variant is
protocol-affecting, compatibility is Wave 1 work.

**D-3 — retype the existing write RPCs to `CalendarItem`.** Generalize the write boundary the way
the read boundary was already generalized: `create_event(Event)` → `create_item(CalendarItem)`,
likewise update, and delete resolves on component-aware item identity. Preserve wire compatibility
where `CalendarItem` continues to serialize through the existing ICS codec. Component branching
lives **inside providers** (`match item { Event(..) => …, Todo(..) => … }`); provider-name
branching never enters core. Parallel `create_task`/`update_task`/`delete_task` methods are
rejected unless implementation proves an unavoidable provider-level reason.

**D-4 — explicit lossy Google mapping with canonical remote state.** Google Tasks is lossy by
nature; do not chase losslessness and do not encode unsupported semantics into invented fields.
MVP faithful set: `SUMMARY↔title`, `DESCRIPTION↔notes`, `DUE↔due`, completed state. The Google
task id is provider metadata; the RFC `UID` stays caldir's logical identity. **DUE:** a datetime
`DUE` pushed to Google converts to the corresponding local calendar date under the timezone policy
already available in context; the canonical remote value is date-only; after push the sync base
holds the canonical representation so the truncation never becomes permanent churn. Unsupported
fields are retained locally where the architecture permits, never given invented equivalents,
never reported as round-tripped, and always classified in a doc and a test. **The u_crawler
fixture sets the mapping priority — CalDAV never waits on a Google field question.**

**D-5 — documentation lives in `docs/vtodo/provider-sync/`**, in this layout, with no new
top-level taxonomy:

```
docs/vtodo/provider-sync/
├── spec.md
├── capabilities.md
├── provider-author-guide.md
├── verification.md
├── providers/{caldav.md,google-tasks.md}
└── tickets/{INDEX.md,P*.md}
```

`provider-author-guide.md` MUST be usable by the separately maintained Proton provider **without
reading `caldir-provider-caldav` or `caldir-provider-google` source.** That is its acceptance test.

**D-6 — a bounded dependency spike precedes Google implementation.** No implementation agent picks
a Google Tasks crate. The spike inspects what `caldir-provider-google` already depends on,
determines whether the existing authenticated HTTP/auth stack can call the Tasks API directly, and
prefers that reuse; only otherwise does it evaluate maintained crates, documenting maintenance
status, auth integration, API coverage, dependency weight, MSRV implications, and whether generated
types leak across boundaries. **Whatever is chosen stays private to `caldir-provider-google`.** No
Google SDK type enters `caldir-core`, the shared RPC, or the capability contract.

---

## 2. The capability spine

Every ticket's `Needs:` line names a rung, never a branch state. This is what survives the D-1
rebase.

```
S1  graceful protocol skew      an unknown method degrades to a typed error
S2  capability negotiation      ProviderCapabilities available at execution time, fail-closed
S3  capability-aware diff       the outgoing set respects tasks.write
S4  item-typed write boundary   create/update/delete carry CalendarItem
S5  core task dispatch          a task change reaches a provider
S6  provider task writes        a named provider performs them
```

---

## 3. Standing corrections

Verified against the tree; ratified by the maintainer. These override the spec.

**Numbering:** `C-n` here are provider-sync corrections. They are unrelated to the shipped core's
`C1`–`C13` constraints in `docs/vtodo/spec.md`. When a body means the core's, it writes `R2/C13`.

**C-1 — §19's Stage A–M is a serial narrative, not the dependency graph.** Superseded by §4's
waves. §27's *release* order survives: it is a shipping constraint, not a build constraint.
**CalDAV is the release gate** — Wave 4 may be developed concurrently with Wave 3 but is not
shippable before Wave 3 is proven.

**C-2 — sync-base state needs no new format.** `SyncBases` is already
`HashMap<EventInstanceId, Option<Box<CalendarItem>>>` (`caldir-core/src/calendar/state/sync_bases.rs`),
and `EventInstanceId` already carries `ComponentKind`. §6/§7 are satisfied by the existing map.
**Do not design a task-side state format.** The canonicalized item the provider returns is the
base, exactly as for events.

**C-3 — §16 cannot be implemented where it is written.** `withhold_task_pushes` runs inside
`CalendarDiff::compute_with` (`caldir-core/src/diff/calendar_diff.rs`), which never sees a
`Remote`. `Connection::diff` does. Thread a capability flag into `compute_with` the way
`Deletes::Allowed` already is — same enum shape, same call site. `CalendarDiff` MUST NOT inspect a
`Remote` or a provider name. Preserve `discard_outgoing()`'s zeroing of `withheld_task_pushes`: a
read-only calendar pushes nothing at all, so naming tasks there reports the wrong restriction.

**C-4 — §17's old-binary guarantee is false today.** An unknown `command` fails `Method`'s derived
`Deserialize` inside `process_request` (`caldir-core/src/provider/handler.rs`), producing
`Failed to parse request: …` — the malformed-JSON path §17 forbids. **Wave 1 work. Not final QA.**

**C-5 — `Method` is not `#[non_exhaustive]`.** Adding a variant breaks out-of-tree matchers,
Proton included. Protocol versioning binds the moment the first variant lands.

**C-6 — §11's remote metadata needs no core change.** `RemoteConfigParams` is a free-form
`BTreeMap<String, toml::Value>` flattened into `[remote]`. `google_resource_kind` /
`google_resource_id` are provider-private keys. A typed core field for them would violate P3.

**C-7 — `suppressed_task_deletes` is revisited when capabilities land.** The belt exists because a
zero-task remote response cannot be told from a provider that cannot answer a VTODO query.
`tasks.read=true` tells them apart. A legacy suppression MUST NOT stay active against a provider
that explicitly advertises safe task support.

**C-7a — `tasks.read=true` is a promise the provider must be able to keep. Precondition on `P3-01`/`P4-05`.**
Established by `P2-06` (`40f93d9`), which lifted the `suppressed_task_deletes` belt for providers
advertising `tasks.read`. `P2-06` closed the *local* incomplete-enumeration hazard: a local listing
known to be incomplete yields `Deletes::Suppressed`, which blocks the delete independently of the
belt. It could **not** close the *remote* half, because core has no detection signal for it —
`Connection::diff` trusts `remote_events` wholesale once capabilities resolve.

So: **a provider that advertises `tasks.read=true` and then returns a listing that silently omits
tasks turns every local VTODO into an incoming delete.** That is C-14's measured scenario — a
`supported-calendar-component-set` hint omitting `VTODO`, a REPORT that fails while still returning
`200` — arriving through the capability channel instead.

This is inert today: no shipped provider advertises `tasks.read=true`, so the belt is on
everywhere. It becomes live the moment `P3-01` lands.

**Binding on `P3-01` and `P4-05`:** a provider may advertise `tasks.read=true` only where it can
*honestly answer* a task query, and it MUST report a failed or degraded task enumeration as an
error rather than as an empty list. An empty list is an assertion that the remote holds no tasks.
Returning one after a failed VTODO REPORT is a data-loss bug, not a graceful degradation — and it
is the specific bug C-14 forbids gating the REPORT on a hint in order to avoid.

**C-8 — no silent deletion path is acceptable, in either direction.** A provider without task-write
support preserves local VTODOs. A capability that is absent, unknown, or errored keeps every belt on.

**C-9 — §8.5's same-UID event/task pair already ships**, live-gated in
`caldir-provider-caldav/src/live/radicale.rs`. So does the bare-`VTODO` `calendar-query` (no
`<C:time-range>`, RFC 4791 §9.9). Their tickets are regression locks, not new work.

**C-10 — `CALDAV-2` is not automatable.** "Thunderbird recognizes it as a task" is a human gate.
§24 permits it: a documented manual command with an expected result, never a `#[test]`.

**C-11 — every `file.rs:NN` in `docs/vtodo/` is a navigation hint, not a citation.** The prior
effort filed stale line numbers as a systematic defect and the upstream reviewer found four of four
spot-checked citations wrong. **Grep the symbol, never the number.**

**C-12 — §7's "remove sync base" on local deletion is wrong and must not be implemented.**
`SyncBases` (`caldir-core/src/calendar/state/sync_bases.rs`) is insert-only: it exposes `get`,
`insert_known_event_id`, `insert_event_base` and `record`, and **no removal path of any kind**.
That is deliberate — identities are retained after deletes so caldir keeps distinguishing a user
delete from a never-seen remote item, which is the project's stated sync rule. Implementing §7
literally would resurrect deleted tasks on the next pull. The base is retained; the item is gone.

**C-13 — a task update must merge provider-managed `X-` properties.**
`Event::with_x_properties_merged_from` (`caldir-core/src/event.rs`) is Event-only, while `Todo`
carries the same `x_properties: Vec<XProperty>` field. A dispatch implementation that mirrors the
event path without extending the merge silently strips provider-managed `X-` metadata — a Google
task id, a CalDAV-side marker — on every task update, and the next sync cannot find the remote
resource. The merge covers both kinds.

**C-14 — `supported-calendar-component-set` is advisory and never gates a request.**
`docs/vtodo/spec.md` R92 and `BACKLOG.md` A-7 rule it **RECORD ONLY, DO NOT IMPLEMENT**, measured:
a VEVENT PUT into a VTODO-only collection returned `201`, so the advertisement can simply be wrong.
Skipping the VTODO REPORT because a hint omitted `VTODO` would hand core an events-only listing,
which it reads as the complete remote state — **every task becomes an incoming delete.** The
property is therefore consulted only *after* the server itself rejected the VTODO REPORT, decides
fatal-vs-degrade only, fires zero extra requests on the happy path, and degrades loudly and only on
explicit evidence. `libdav` already ships `GetSupportedComponents` — do not hand-roll it.

**C-15 — the production fixtures are far poorer than the shipped test fixtures.**
Measured across the 57 `deadlines/*.ics` in `docs/vtodo/caldir-vtodo-fixtures.tar.gz`: every file
carries `UID`, `DTSTAMP`, `DUE`, `PRIORITY:1`, `SUMMARY` and `URL`; 32 add `STATUS:COMPLETED`; and
**none** carries `DESCRIPTION`, `COMPLETED`, `PERCENT-COMPLETE`, `DTSTART`, `RRULE` or any `X-`
property. The completed ones are exactly R68's Radicale case — `STATUS:COMPLETED` with neither
`COMPLETED` nor `PERCENT-COMPLETE`. Two further facts: the `deadlines/` task and its `windows/`
event do **not** share a UID (`u_crawler-todo-N@…` vs `u_crawler-window-N@…`), so §8.5's same-UID
pair is synthetic-only; and 40 of 57 summaries carry non-ASCII UTF-8 while no shipped fixture does.
The shipped `VTODO_ICS` const is *richer* than production — it has `DESCRIPTION` and `COMPLETED`.
Per D-4 the production fixture sets the mapping priority, so every classification table is built
against this census first and marks the rest speculative.

**C-16 — rename the Rust API, keep the wire verb.**
D-3's substance is the payload type, and it ships in full. Its naming clause splits: the `Handler`
methods and RPC structs become item-typed (`create_item`, `update_item`, `delete_item`), which
breaks an out-of-tree provider at **compile time** with a clear error — the "smallest reasonable
migration burden" §18 asks for. The wire verbs (`create_event`, `update_event`, `delete_event`) and
the JSON payload key stay, because renaming those breaks an installed provider **silently at
runtime**, which is the failure D-2's fail-closed rule and §17's versioning exist to prevent.

**C-17 — "preserved only locally" is false unless the provider carries the field forward.**
`push_outgoing_changes` (`caldir-core/src/connection.rs`) calls `cal_event.update(returned_item)`:
core **overwrites the local `.ics` with the item the provider returned**. So a field a lossy
provider drops from its canonical response is destroyed on disk, not preserved. Every local-only
row in a classification table is a lie unless the provider copies those fields onto the response
item it hands back. This is the crux of `P5-06`, and it is why D-4's "retain them locally where the
architecture permits" needs provider-side work rather than an absence of work.

**C-18 — the production `DUE` values land on the previous local day.**
**Amended P1-05 (measured):** 56 of 57, not all 57. The distribution is 44 × `T035959Z`,
12 × `T025959Z`, and **one outlier at `T025900Z`** (23:59:00 local, not 23:59:59). The
conclusion is unchanged — the outlier lands on the previous local day exactly like the rest — but
no ticket may assert `DUE` ends in `5959Z`. The original text follows.
All 57 production VTODOs carry a UTC-instant `DUE` at `T035959Z` or `T025959Z` — both present
because `America/Santiago` changes offset in early September 2026 — and each is 23:59:59 on the
*previous* local day. `EventTime::to_utc` is `to_local_tz(&chrono::Local).with_timezone(&Utc)`, so
a date conversion that reaches for it moves the entire u_crawler deadline set **one day late**.
D-4's rule resolves it: the calendar date comes from the `DUE`'s own zone when it has one and the
system zone otherwise, and the offset comes from the zone database, never a constant. Assert both
fixture values by name and take the zone as a parameter.

---

## 4. Waves and ticket definitions

A wave is a set of tickets with no edges between them: dispatch it at once, one fresh bounded
context per ticket. A wave closes when every ticket is green, committed by explicit path, and
`just check` passes on the merged result.

**Concurrency rule:** one cargo invocation at a time — the pinned toolchain races rustup under
parallel cargo. Agents edit; a serializing runner checks.

**Commit discipline:** commit after every ticket, staging by explicit path. `git add -A` mid-wave
sweeps another agent's in-flight work into the wrong commit.

### Wave 1 — shared protocol and capability foundation

Spine is serial; the three parallel tickets are genuinely disjoint.

```
P1-01 ──> P1-02 ──> P1-03
P1-04  ── item-typed write boundary   (rpc/methods/* + 5 provider crates)
P1-05  ── production fixtures          (no code contact)
P1-06  ── capability contract doc      (no code contact)
```

**P1-01 — graceful protocol skew** · Provides `S1` · Needs: — ·
Responsibility: close C-4. An unrecognized `command` reaching a provider produces a typed
version/capability error naming the provider and the method. Lands **before** the first new variant
exists, so the guarantee is in place when `P1-02` creates the skew it protects against.
Files: `caldir-core/src/provider/handler.rs`, `caldir-core/src/rpc.rs`,
`caldir-core/src/provider/error.rs`. Spec: §17, §18. Proves: `CROSS-4`.
Risk: `Method`'s `Deserialize` must tolerate an unknown string without losing the strictness that
makes a genuine typo loud.

**P1-02 — provider capabilities RPC** · Provides `S2` · Needs `S1` ·
Responsibility: D-2 end to end — the `ProviderCapabilities` type, the `capabilities` method, and a
`Handler` default returning events-only so every unmodified provider keeps working untouched. The
fail-closed reading of every failure mode is this ticket's core assertion, not a later refinement.
Files: new `caldir-core/src/provider/capabilities.rs`, new
`caldir-core/src/rpc/methods/capabilities.rs`, `rpc/methods.rs`, `rpc.rs`, `provider/handler.rs`,
`provider.rs`. Spec: §4, §17, §18. Proves: `CROSS-3`, `CROSS-4`.
Risk: **the `Method` variant lands here — C-5 applies.** The versioning story ships with the
variant. `ProviderCapabilities` is public API: Proton consumes it.

**P1-03 — capability-aware diff** · Provides `S3` · Needs `S2` ·
Responsibility: task changes leave the outgoing set exactly when the provider does not advertise
`tasks.write`. Per C-3, thread the flag into `compute_with` alongside `Deletes`.
Files: `caldir-core/src/diff/calendar_diff.rs`, `caldir-core/src/connection.rs`. Spec: §7, §16,
§18. Proves: `CROSS-3`, `CROSS-5`.
Risk: `from_changes` (the `#[cfg(test)]` constructor) applies the same withholding on purpose —
keep both paths in step or a test diff holds a shape production cannot produce.

**P1-04 — item-typed write boundary** · Provides `S4` · Needs: — (rebase onto the spine) ·
Responsibility: D-3. `CreateEvent`/`UpdateEvent`/`DeleteEvent` carry a `CalendarItem` and return
one; delete resolves on component-aware identity. Providers gain the `match item` shape and return
the unsupported error for the arm they cannot serve.
Files: `caldir-core/src/rpc/methods/{create,update,delete}_event.rs`,
`caldir-core/src/provider/handler.rs`, `caldir-core/src/remote.rs`, plus create/update/delete
commands in the **four** write-capable `caldir-provider-*` crates. `caldir-provider-webcal`
implements only `connect` and `list_events` and must compile untouched — assert that with
`git diff --stat`. Spec: §5, §18. Proves: `CROSS-1`, `CROSS-6`.
Risk: **exceeds the sizing contract** — measured at 27 `cmd.event` sites across 10 files and 4 crates. Mechanical but
wide. The compensating lock: outbound wire bytes for an event are byte-identical before and after,
asserted against `Event::to_ics_string`, the shape `handler.rs` already uses inbound. Green lock,
safe breadth.

**P1-05 — production fixtures, extracted and redacted** · Provides fixtures · Needs: — ·
Responsibility: lift a representative set out of `docs/vtodo/caldir-vtodo-fixtures.tar.gz` into
committed fixtures — one incomplete assignment, one completed, one with `URL`, one with `PRIORITY`,
one timestamped `DUE`, and a matching `VEVENT` window — with personal and environment-specific data
removed. Files: `caldir-core/src/test_utils.rs` and/or a fixtures directory. Spec: §20, §14.
Risk: redaction must not alter the property shapes the mapping tickets exercise. Every later
mapping decision is made against **these** files, never hypotheticals.

**P1-06 — capability contract documentation** · Needs `S2` ·
Responsibility: the Wave-1-decidable half of the Proton-facing contract — capability declaration,
the fail-closed rules, protocol versioning, and old-binary behaviour. The write lifecycle is
`P6-01`, which continues the same files.
Files: `docs/vtodo/provider-sync/capabilities.md`, `docs/vtodo/provider-sync/provider-author-guide.md`.
Spec: §17, §26. Proves: `CROSS-4`.
Risk: link back to `docs/vtodo/spec.md` for the VTODO domain model; restating it forks the source
of truth.

### Wave 2 — core sync write dispatch

**P2-01 — core task write dispatch** · Provides `S5` · Needs `S3`, `S4` ·
Responsibility: `Remote::apply_change` dispatches task create, update and delete instead of
aborting. Its two current aborts are named locks
(`a_task_change_aborts_instead_of_reaching_the_provider`,
`a_task_delete_aborts_instead_of_reaching_the_provider`) — they invert here, and the inversion is
the proof. Files: `caldir-core/src/remote.rs`, `caldir-core/src/connection.rs`. Spec: §5, §7.
Proves: `CROSS-1`.
Risk: create/update/delete are **one ticket, three acceptance criteria**, because all three route
through the single `apply_change` match. Splitting them puts three agents in one function and buys
no reduction in scope. The two inverted locks are assertions of the old world — rewrite them
deliberately, with the diff reviewed, never weakened into vacuity.

**P2-02 — sync-base round trip for tasks** · Needs `S5` ·
Responsibility: the canonical item a provider returns from a write becomes the sync base in the
same cycle, so a second sync reports no changes. Per C-2 this uses the existing map — the ticket
adds no state format. Files: `caldir-core/src/connection.rs`, `caldir-core/src/remote.rs`.
Spec: §6, §7, §15. Proves: `CALDAV-7`, `GOOGLE-10`.
Risk: the failure mode is a duplicate on the second sync. Assert the sequence, not the function.

**P2-03 — fail-closed unsupported behaviour** · Needs `S2`, `S3` ·
Responsibility: C-8 made executable. A provider without `tasks.write` never has a task change
attempted against it, never loses a local VTODO, and a capability that is absent, unknown or
errored behaves identically to an explicit `false`.
Files: `caldir-core/src/connection.rs`, `caldir-core/src/diff/calendar_diff.rs` tests.
Spec: §16, §18. Proves: `CROSS-5`.
Risk: the interesting cases are the failure modes, not the honest `false`. Test the timeout and the
error reply, not only a provider that answers politely.

**P2-04 — typed provider error taxonomy** · Needs `S1`, `S2` ·
Responsibility: the six §16 classes — unsupported task writes, missing auth scope, remote not
found, conflict/precondition, semantic limitation, network/API — distinguishable by the CLI without
string matching. Files: `caldir-core/src/provider/error.rs`, `caldir-core/src/provider/handler.rs`,
`caldir-core/src/connection/error.rs`. Spec: §16, §17. Proves: `GOOGLE-13`, `CROSS-4`.
Risk: the wire carries a flat `error: String` today and `format_chain` flattens the source chain
into it. Adding structure is a wire change — `P1-01`/`P1-02`'s versioning covers it, so sequence
accordingly.

**P2-05 — provider-specific unsupported messaging** · Needs `S3`, `P2-04` ·
Responsibility: retire `tasks are pull-only — caldir cannot push tasks to a remote yet` for a
message naming the provider, e.g. `Provider "webcal" does not support task writes.`
Files: `caldir-cli/src/output/diff.rs`, `website/src/content/docs/overview.md`. Spec: §16.
Proves: `CROSS-5`.
Risk: four assertions in `output/diff.rs` quote the old string verbatim, and the website quotes it
too. Copy and behaviour change in one commit.

**P2-06 — suppressed-task-delete cleanup** · Needs `S2` ·
Responsibility: C-7. Honour real remote task deletions for providers advertising `tasks.read`;
keep the belt everywhere else. Files: `caldir-core/src/diff/calendar_diff.rs`,
`caldir-cli/src/output/diff.rs`. Spec: §15, §18. Proves: `GOOGLE-3`, `CROSS-5`.
Risk: **the highest-consequence ticket in the effort** — it converts a refusal-to-delete into a
delete. A capability misread here deletes a user's tasks. C-8 governs: absent, unknown or errored
keeps the belt on.

### Wave 3 — CalDAV *(the release gate)*

**P3-01 — task collection discovery and connect** · Provides `S6`(caldav) · Needs `S2` ·
Responsibility: declare `tasks.{read,write}=true`; surface task-capable collections in `connect`;
close the R2/C13 gap where the second REPORT's failure breaks a working event-only sync. Per C-14
the component-set property decides **fatal-vs-degrade after a rejection**, never whether to send
the REPORT.
Files: `caldir-provider-caldav/src/commands/{capabilities,list_calendars,list_events}.rs`,
`src/caldav/ops/list_calendars.rs`. Spec: §4, §8.5, §11 by analogy, §18. Proves: `CROSS-3`,
`CROSS-4`.
Risk: this is the ticket that must not regress event-only sync against a server with no VTODO
support. That regression has no test today; add it here.

**P3-02 — VTODO create** · Needs `S5`, `P3-01` ·
Responsibility: upload a local `VTODO` as `VTODO` calendar data at a UID-derived href; never
rewrite it as `VEVENT`. Files: `caldir-provider-caldav/src/caldav/ops/create_event.rs`,
`src/commands/create_event.rs`. Spec: §8.1, §8.4. Proves: `CALDAV-1`, `CROSS-6`.
Risk: the reachable `409 <C:no-uid-conflict/>` trigger is *a resource at another href already
holding that UID* — a server-assigned filename, not a local rename, which yields the same
UID-derived href. **Assert the outcome — one resource, correct body, no duplicate — never the
status code.**

**P3-03 — VTODO update** · Needs `P3-02` ·
Responsibility: update the existing remote resource, reusing the ETag/precondition handling the
event path already uses. A change to `SUMMARY`, `DUE`, `STATUS` or `PRIORITY` yields one updated
remote `VTODO`. Files: `caldir-provider-caldav/src/caldav/ops/update_event.rs`,
`src/commands/update_event.rs`. Spec: §8.2, §8.4. Proves: `CALDAV-3`, `CALDAV-4`, `CALDAV-5`.
Risk: creating a second remote task instead of updating the first is the failure this prevents —
assert resource count, not only content.

**P3-04 — VTODO delete** · Needs `P3-02` ·
Responsibility: delete the corresponding resource when sync direction authorizes it.
Files: `caldir-provider-caldav/src/caldav/ops/delete_event.rs`, `src/commands/delete_event.rs`.
Spec: §8.3. Proves: `CALDAV-6`.
Risk: the authorization test is core's, not the provider's — the provider deletes what it is told
to delete.

**P3-05 — mixed-component behaviour** · Needs `P3-02..04` ·
Responsibility: pin, do not build (C-9). A collection holding both `VEVENT` and `VTODO` enumerates
both; an event and a task sharing a UID stay distinct through create, update and delete.
Files: `caldir-provider-caldav/src/caldav/ops/list_events.rs` tests, `src/live/radicale.rs`.
Spec: §8.5, §8.6. Proves: `CALDAV-8`.
Risk: low. The value is that writes are the new way to break shipped behaviour.

**P3-06 — round-trip fidelity** · Needs `P3-02..04`, `P1-05` ·
Responsibility: prove CalDAV is the highest-fidelity task provider — every property the core task
codec supports survives push/pull, with provider-managed and unknown `X-` properties following the
preservation rules the core spec defines. Files: `caldir-provider-caldav/src/caldav/ops/` tests.
Spec: §8.4. Proves: `CALDAV-7`.
Risk: "no semantic loss" needs an enumerated property list to be checkable. Derive it from the core
spec's IN set; a bare "round-trips" assertion proves nothing.

**P3-07 — Radicale E2E and u_crawler QA** · Needs `P3-02..04`, `P1-05` ·
Responsibility: the §8.6 matrix against a real Radicale — create, pull, update, delete, plus
`create→pull→no changes`, `update→pull→no changes`, `delete→pull→absent`, a completed VTODO, an
undated VTODO, and the `P1-05` fixture.
Files: `caldir-provider-caldav/src/live/radicale.rs`. Spec: §8.6, §21, §25.
Proves: `CALDAV-1`, `CALDAV-7`, and clause 2 of the shipped core's R95, which SESSION-LOG §8
records as currently unreachable.
Risk: gates run behind `CALDIR_LIVE_RADICALE=1`, off by default. The Radicale image is pinned by
tag, not digest — pin the digest here.

### Wave 4 — Google plumbing *(concurrent with Wave 3; not shippable before it)*

**P4-01 — dependency spike** · Needs: — ·
Responsibility: D-6, as a research deliverable, not code. Files:
`docs/vtodo/provider-sync/providers/google-tasks.md`. Risk: it must reach a recommendation, not a
survey. The default answer to beat is "reuse the existing authenticated HTTP stack".

**P4-02 — OAuth scopes and session migration** · Needs `P4-01`, `P2-04` ·
Responsibility: request task access alongside existing Calendar scopes; a session predating the
scope fails with an actionable reauthorization instruction, never a bare Google 403.
Files: `caldir-provider-google/src/commands/connect.rs` (which holds `SCOPES` and the
`SetupData::instructions` copy), `src/constants.rs`, `src/session/`. **Not** `app_config.rs`:
`AppConfig` holds only `client_id`/`client_secret`. Spec: §10. Proves: `GOOGLE-13`.
Risk: no refresh or access token may be written inside a calendar directory — storage stays under
`CALDIR_PROVIDER_STORAGE_DIR`. Document the migration path for existing sessions.

**P4-03 — task-list discovery and resource kind** · Needs `P4-02` ·
Responsibility: `list_calendars` returns Calendar collections *and* Tasks lists, tagged by kind in
free-form remote params per C-6; the CLI distinguishes them plainly enough that a task directory
cannot be connected to an event-only Calendar by accident.
Files: `caldir-provider-google/src/commands/list_calendars.rs`, `src/remote_config.rs`, the CLI
connect output. Spec: §11. Proves: `GOOGLE-1`, `GOOGLE-2`.
Risk: **never infer the kind from the ID string.** An existing Google Calendar remote config stays
valid and is never reinterpreted as a task list.

**P4-04 — Google Task → VTODO mapping** · Needs `P4-03` ·
Responsibility: a Google Task becomes a `VTODO` over D-4's faithful set. The Google task id is
provider metadata in an `X-GOOGLE-TASK-ID` property, mirroring `X-GOOGLE-EVENT-ID` — never the
`UID`. Files: new `caldir-provider-google/src/google_task/{mod,from_google}.rs`,
`src/commands/list_events.rs`. Spec: §9, §12, §6. Proves: `GOOGLE-3`, `GOOGLE-5`, `GOOGLE-6`,
`GOOGLE-7`.
Risk: a `VTODO` must never travel the Calendar API path and a Google Task must never surface as a
`VEVENT`. The two-API split stays private to the provider.

**P4-05 — Google capabilities declaration** · Provides `S6`(google, read) · Needs `P4-04` ·
Responsibility: advertise `tasks.read` for a task-list remote and no task support for a Calendar
remote. `tasks.write` stays `false` until Wave 5 closes.
Files: `caldir-provider-google/src/commands/capabilities.rs`. Spec: §4. Proves: `CROSS-3`.
Risk: per D-2, `tasks.write=true` is legal only once create, update and delete are all coherent.

### Wave 5 — Google writes and canonicalization

**P5-01 — create** · Needs `P4-04`, `S5` · Files: `src/google_task/to_google.rs`,
`src/commands/create_event.rs` · Spec: §12, §15 · Proves: `GOOGLE-4` · Risk: the create response
carries the Google-generated id; it must reach the sync base in the same cycle or the next sync
duplicates.

**P5-02 — update** · Needs `P5-01` · Files: `src/commands/update_event.rs` · Spec: §15 · Proves:
`GOOGLE-9` · Risk: resolve the target by stored task id, never by title.

**P5-03 — delete** · Needs `P5-01` · Files: `src/commands/delete_event.rs` · Spec: §15 · Proves:
`GOOGLE-9` · Risk: a remote task already gone is a success — the `P2-04` not-found class. **The base is
retained, not removed** (C-12); the earlier wording of this row was wrong.

**P5-04 — provider identity lifecycle** · Needs `P5-01..03` · Files: Google provider tests · Spec:
§6, §7 · Proves: `GOOGLE-9` · Risk: the whole ticket is the no-duplicate property —
`create → sync → sync` yields exactly one remote task, and the id survives update and delete.

**P5-05 — due-date canonicalization** · Needs `P5-01`, `P4-04` · Files: `src/google_task/` ·
Spec: §13, §15 · Proves: `GOOGLE-8`, `GOOGLE-10` · Risk: **permanent sync churn is the specific
failure mode.** The proof is `create → sync → sync` reporting no changes on the second run, not a
unit test of the conversion function.

**P5-06 — unsupported-field policy** · Needs `P4-04`, `P1-05` ·
Responsibility: classify every VTODO property Google Tasks cannot carry — `PRIORITY`, `URL`,
`DTSTART`, `IN-PROCESS`, `CANCELLED`, `PERCENT-COMPLETE`, `RRULE`, `ATTENDEE`, `ORGANIZER`,
`ATTACH`, `VALARM`, arbitrary `X-` — as faithful, lossy, local-only, or rejected. One test per row;
warn when a user-authored semantic change cannot be represented; invent no equivalent value.
Files: `src/google_task/` tests, `docs/vtodo/provider-sync/providers/google-tasks.md`. Spec: §12,
§14. Proves: `GOOGLE-11`.
Risk: classify the properties the `P1-05` fixtures **actually contain** first. A table designed
from hypotheticals is the failure this ticket is written against.

**P5-07 — no-churn verification and `tasks.write`** · Needs `P5-01..06` · Files:
`src/commands/capabilities.rs`, Google provider tests · Spec: §4, §15 · Proves: `GOOGLE-4`,
`GOOGLE-10`, `CROSS-3` · Risk: this ticket closes Wave 5 by flipping `tasks.write` to `true`; it
does not open it.

### Wave 6 — external provider, docs, compatibility

**P6-01 — Proton-facing provider-author guide** · Needs Waves 1–5 · Files:
`docs/vtodo/provider-sync/provider-author-guide.md` (continuing `P1-06`) · Spec: §17, §26 ·
Proves: `CROSS-4` · Risk: **its acceptance test is that a reader implements task support without
opening the CalDAV or Google source.** Cover the write lifecycle, provider identity metadata, error
behaviour, canonicalization, and a minimal reference provider.

**P6-02 — old provider binary behaviour** · Needs `S1`, `S2` · Files: `caldir-core/src/rpc.rs`,
`caldir-core/src/provider/handler.rs` tests · Spec: §17, §18 · Proves: `CROSS-4` · Risk: the
executable proof is a **stub old binary** knowing only the pre-capability methods, driven end to
end, producing a clear version error and a fail-closed events-only sync.

**P6-03 — protocol compatibility verification** · Needs `P6-02` · Files: `caldir-core` tests ·
Spec: §17, §18 · Proves: `CROSS-4` · Risk: covers the D-1 rebase surface — the wire shapes that
must not drift when the core PR sequence reorders underneath.

**P6-04 — full workspace regression** · Needs Waves 1–5 · Files: `caldir-core` and CLI tests ·
Spec: §18, §23 · Proves: `CALDAV-8`, `GOOGLE-12`, `CROSS-1`, `CROSS-2`, `CROSS-5`, `CROSS-6` ·
Risk: `CROSS-2` is checkable mechanically — grep `caldir-core` for a provider-name match in any
task path and assert none. `CROSS-6` likewise: no code path converts a `VTODO` to a `VEVENT`.

**P6-05 — final provider-sync verification** · Needs `P3-07` · Files:
`docs/vtodo/provider-sync/verification.md`, `docs/vtodo/provider-sync/providers/caldav.md` · Spec:
§24, §25 · Proves: `CALDAV-2`, `CALDAV-7` · Risk: manual by nature (C-10). Record exact commands
and expected output for the pre-feature warning, the first run pushing N changes, the second
reporting none, and the task appearing in an independent CalDAV client.

**P6-06 — release mechanics** · Needs everything · Files: crate `Cargo.toml` versions,
`.github/workflows/release.yml`, `website/src/content/docs/{providers,quickstart,overview}.md` ·
Risk: `caldir-provider-icloud` pins `caldir-provider-caldav` — a cross-provider pin the `bump`
skill's table does not list. Bump it by hand. Also: `CLAUDE.md`'s provider checklist names
`website/src/content/docs/getting-started.md`, which does not exist — the page is `quickstart.md`.
Fixing that checklist line is part of this ticket.

---

## 5. Definition of done

Per §24: `cargo fmt --check`, `cargo clippy --workspace --all-targets` and
`cargo test --workspace` pass; the CalDAV mocked tests, the Radicale E2E gates, the Google unit and
mapping tests, and the `P1-05` fixture regression all pass; and every credential-requiring test
marked ignored carries its exact command and expected result in `P6-05`'s document.
