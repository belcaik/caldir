# ADR-07: How provider capabilities express VTODO support

## Status
PROPOSED (Stage 3 candidate)

## Context

Forces, with evidence:

1. **No negotiation exists.** The only capability signal is `read_only: Option<bool>` on
   `CalendarConfig` (`calendar/config.rs:13`), read by `Connection::read_only()`
   (`connection.rs:30-34`) and enforced by `diff.discard_outgoing()` (`connection.rs:57-59`,
   `diff/calendar_diff.rs:136-138`). It gates *direction*; nothing gates *component kind*.
2. **A new RPC verb is forbidden.** `Method` is a closed serde enum, no `#[serde(other)]`
   (`rpc/methods.rs:22-31`); unknown commands die at `from_str::<Request>` (`handler.rs:75-78`).
3. **Optional fields are wire-safe.** `deny_unknown_fields` appears nowhere; every
   `CalendarConfig` field is already `Option` (`config.rs:9-17`).
4. **No shipped binary can carry a VTODO at all.** The wire codec is
   `Event::from_single_ics_str` (`rpc/ics.rs:32-35`) over `icalendar.events()`
   (`event.rs:96-102,111-121`): a VTODO decodes as `found: 0`. True of every provider, caldav
   included — its comp-filter is hard-coded `VEVENT` (`ops/list_events.rs:103`, `ops.rs:107`).
5. **Failing late is unactionable.** Ungated, pushing a task to Google prints this verbatim red
   line and then *aborts the rest of that calendar's push* (the `?` in `push_outgoing_changes`,
   `connection.rs:231`):
   `Remote error: Provider error: Provider returned error: Error handling request: expected 1 event(s) in ICS, found 0`
   (chain: `connection/error.rs:7` -> `remote/error.rs:5` -> `provider/error.rs:26` ->
   `handler.rs:82` -> `event/error.rs:11`).
6. **Google/Outlook cannot simply implement tasks:** different APIs, new OAuth scopes, re-consent
   for every existing user (provider-matrix; `google/connect.rs:23-26`, `outlook/connect.rs:25`).
   Their *behaviour* is what this ADR fixes, not their implementation.
7. **`supported-calendar-component-set` is real and unread** (RFC 4791 §5.2.3), and is CalDAV-only
   — webcal, Google and Outlook have no equivalent.
8. **`CalendarConfig` is persisted:** `list_calendars`/`connect` output is written verbatim to
   `.caldir/config.toml` (`config.rs:33-39`, from `connect.rs:188`), so what core reads at sync
   time comes from *disk*. A per-sync probe is not free (one subprocess per RPC call,
   `subprocess_transport.rs:26-77`) and webcal implements no `list_calendars`.

## Options considered

**(a) New optional field on `CalendarConfig`.** Blast radius: `calendar/config.rs`,
`connection.rs`, `diff/calendar_diff.rs`, `cli/output/diff.rs`, one line per provider crate.
Benefit: same shape, lifecycle and enforcement point as `read_only` — the two become orthogonal
axes (direction × kind) of one capability model, and the whole question collapses into a single
predicate. Wire-safe per (3). Cost: the persisted value can go stale when a provider binary is
upgraded without re-running `connect` — exactly the staleness `read_only` already has.

**(b) Reuse `read_only`.** Marks a whole Google calendar read-only because it cannot take tasks:
conflates two independent axes and silently stops syncing *events*. Non-starter.

**(c) No declaration — attempt and fail.** Zero new surface, and honest that only the provider
truly knows. Cost: the message in Context 5 — names neither tasks, nor the limitation, nor a
fix; poisons the rest of that calendar's push; recurs every sync.

**(d) `connect`-time capability map via the free-form `options`/`data` maps**
(`rpc/methods/connect.rs:13-18`). Wire-safe, no struct change. But per-*account*, not
per-*calendar* (CalDAV VTODO support is per-collection), and nothing persists it, so
`Connection::diff` could not read it without re-running connect. Rejected on lifecycle, not wire
safety.

**(e) Discover from the server (`supported-calendar-component-set`).** Genuinely more correct
*for CalDAV*: the server is the authority and per-collection is the right granularity (iCloud
Reminders lists are exactly this). But not a *core* mechanism — core must keep zero CalDAV
knowledge, and Google/Outlook/webcal have no such property. Discovery and declaration are not
rivals; they sit at different layers, and the Decision uses both.

## Decision

**Add one optional field to `CalendarConfig`; absent = "VEVENT only"; enforce in
`Connection::diff` next to `read_only`; report as a per-calendar warning, never an error.**

1. **Wire shape.** In `caldir-core/src/calendar/config.rs`, **after `read_only`, before
   `remote`** (TOML requires scalars before tables):

   ```rust
   #[serde(default, skip_serializing_if = "Option::is_none")]
   supported_components: Option<Vec<String>>,
   ```

   JSON (`list_calendars` / `connect` response):
   `{"name":"Tasks","color":null,"read_only":false,"supported_components":["VEVENT","VTODO"],"remote":{…}}`
   TOML (`.caldir/config.toml`): `supported_components = ["VEVENT", "VTODO"]`

   `Vec<String>`, not a typed enum: a closed enum would hard-fail on a future `"VJOURNAL"` both
   on the wire and when loading existing config files. RFC 5545 names, matched
   case-insensitively **in exactly one place** — the interface is the predicate, not the vector:

   ```rust
   pub fn supports_todos(&self) -> bool {
       self.supported_components.as_ref()
           .is_some_and(|c| c.iter().any(|s| s.eq_ignore_ascii_case("VTODO")))
   }
   ```

   **Do not add a 5th positional parameter to `CalendarConfig::new`** — it is public and called
   by every provider crate, in-tree and out. Add `with_supported_components(mut self, …) -> Self`
   instead, so every existing provider keeps compiling untouched.

2. **Old-provider default: absent = does not support VTODO** (`is_some_and` makes `None` false).
   See Backward-compatibility for why this costs nothing.

3. **Enforcement: core-side, before dispatch, in `Connection::diff`.** Partition local events
   *before* `CalendarDiff::compute` sees them — not after — so an unsupported task can never be
   inferred as an `incoming Delete` (`calendar_diff.rs:86`) and destroy the user's file. Skipped
   items land in a third bucket on `CalendarDiff` beside `outgoing`/`incoming`. Providers are
   therefore never handed a Todo they did not declare support for: **no provider needs a typed
   error and no provider crate changes behaviour.** Mirrors `read_only` exactly —
   `if !self.supports_todos() { diff.skip_todos(); }` one line from
   `if self.read_only() { diff.discard_outgoing(); }` (`connection.rs:57-59`).

4. **User-facing text.** Yellow `!`, three-space indent per `output/diff.rs:81`; once per
   calendar per `push`/`pull`/`status`; exit code unchanged (0). Verbatim:

   ```
      ! Skipped 1 task: the "google" provider does not support tasks (VTODO).
        buy-milk.ics
        Tasks sync only with CalDAV-backed calendars (caldav, icloud).
        Move it there, or delete the file to silence this warning.
   ```

   Plural: `Skipped {n} tasks: …`, up to 3 filenames, then `… and {n} more`, then
   `Move them there, or delete the files to silence this warning.`
   `render_bidirectional`'s `if diff.is_empty() { "No changes" }` early return
   (`output/diff.rs:150-152`) **must** account for the skipped bucket, or a calendar holding
   only tasks prints "No changes" and nothing else.

5. **Who declares what.** caldav: derive from `supported-calendar-component-set` in
   `list_calendars_raw` — option (e) adopted *inside the caldav provider*, as the source of the
   declared value. iCloud: pass caldav's value through (`icloud/list_calendars.rs:44`). google,
   outlook: `["VEVENT"]`, explicit, matching the default. webcal: `["VEVENT","VTODO"]` —
   component-agnostic transport, already hard-coded read-only (`webcal/connect.rs:82-87`), so no
   push hazard and feed tasks become readable. The flags compose: `read_only` = direction,
   `supported_components` = kind.

## Consequences

Easy: VJOURNAL later needs no new mechanism; `caldir calendars` can grow a "tasks" column;
a provider gains task support by shipping a binary + the user re-running `connect`.
Hard: the value is a snapshot — upgrading a provider binary without re-running `connect`
leaves a stale `.caldir/config.toml` and tasks stay skipped (`caldir doctor` could compare
stored config against a live `list_calendars`). Nothing refreshes it during `pull`/`push`,
by design (Context 8).

Must be tested: (i) a config with no `supported_components` loads and `supports_todos()` is
false; (ii) `to_toml` with `None` is byte-identical to today (guards
`writes_full_config_with_remote_to_expected_toml`); (iii) a local task in a non-VTODO calendar
with populated `sync_bases` produces **no** `incoming Delete`; (iv) events in that calendar
still push (no regression); (v) `["vtodo"]` lowercase accepted; (vi)
`["VEVENT","VTODO","VJOURNAL"]` deserializes rather than erroring.

## Backward-compatibility impact

- **Local files on disk:** unchanged. The gate only decides whether a task is *offered* to a
  provider; no `.ics` is written, moved or deleted by it.
- **`.caldir` state:** `config.toml` gains one optional scalar; existing files parse unchanged
  (`#[serde(default)]`), and `None` serializes byte-identically (`skip_serializing_if`).
  `known_event_ids` / `sync_bases` untouched.
- **RPC wire:** additive only. No new `Method` variant, no changed request shape. New core +
  old provider: field absent, `supports_todos()` false, tasks skipped with the warning above.
  Old core + new provider: unknown field silently ignored (no `deny_unknown_fields`).
- **Installed provider binaries:** none break, and **none regress** — no shipped binary can
  carry a VTODO today (`event.rs:96-102` drops it before the arity check at `:111-121`), so
  "absent means no VTODO" is not a pessimistic default, it is a *factually accurate*
  description of every binary in existence. Honest migration consequence: the caldav provider
  must set the field in the same PR that removes its hard-coded
  `<C:comp-filter name="VEVENT">` (`ops/list_events.rs:103`, `ops.rs:107`) — one PR, not
  lockstep across releases, because there is no working task sync to keep working. New core
  against an *old* caldav binary gets the warning, not corruption.
- **Source compatibility:** preserved for out-of-tree providers *provided*
  `CalendarConfig::new` keeps its 4-arg signature (Decision 1).
- **Docs:** `website/src/content/docs/providers.md` needs a tasks column; `specs/caldir.md`
  needs no amendment for this ADR (no ICS-format change here).

## Rejected because

- (b) conflates direction with component kind and would stop event sync too.
- (c) fails per-item with `expected 1 event(s) in ICS, found 0` and aborts the rest of the push.
- (d) is per-account and never persisted, so `Connection::diff` cannot read it.
- (e) *as a core mechanism*: CalDAV-only property; adopted instead inside the caldav provider.

## Open sub-questions

1. **Does Radicale advertise `supported-calendar-component-set` on every collection?**
   UNVERIFIED. If a server omits it, the caldav provider needs a fallback. *Experiment:*
   `PROPFIND Depth:1` against a local Radicale and grep the response. Proposed fallback if
   absent: `["VEVENT","VTODO"]` — the failure mode is then a server 4xx, not data loss.
2. **Should `caldir doctor` warn on a stale `supported_components`?** Deferrable; a product
   decision, no experiment.
3. **Does the warning belong on `pull` at all?** A provider that cannot carry tasks never
   returns one, so the pull-side warning only fires for *local* tasks; push/status only may be
   better. The skipped bucket makes either choice a one-line renderer change.
