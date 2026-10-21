# ADR-05: Unknown-property preservation for safe VTODO round-trip

## Status
PROPOSED (Stage 3 candidate)

## Context

How much unknown-property preservation must `Todo` carry for
`local -> caldir -> CalDAV -> caldir -> local` to avoid *silent* data loss?

### What `Event` does today — traced, then measured

`event/from_icalendar.rs:10-93` reads a fixed property list into typed fields, then at
`:60-65` captures **only** names starting with `X-` into `x_properties` (name + value +
params). Nothing else is read. `event/to_icalendar.rs:4-97` rebuilds the component from
those fields alone. caldir never echoes source bytes — disk files, sync bases
(`calendar/state/event_bases.rs:34`) and the RPC wire (`rpc/ics.rs:28`) all re-serialize
from the domain type. An unmodelled property dies at parse; nothing can recover it.

**Experiment** (scratch crate, `caldir-core` path dep, `icalendar` pinned `=0.17.10` to
match `Cargo.lock:1194-1195`). Input VEVENT carried `UID DTSTAMP CREATED DTSTART DTEND
SUMMARY CATEGORIES GEO RELATED-TO PRIORITY RESOURCES COMMENT CONTACT COLOR
X-APPLE-TRAVEL-ADVISORY-BEHAVIOR X-VENDOR-ID;X-PARAM=keepme X-LIST:a,b,c`.
`Event::from_ics_str` -> `to_ics_string` actually produced:

```
BEGIN:VCALENDAR / VERSION:2.0 / PRODID:CALDIR / BEGIN:VEVENT
DTSTAMP:20260825T054558Z   <- synthesized fresh on EVERY serialize
DTEND:20260815T110000Z / DTSTART:20260815T100000Z / SUMMARY:Round-trip probe
UID:probe-1@example.com
X-APPLE-TRAVEL-ADVISORY-BEHAVIOR:AUTOMATIC
X-LIST:a,b,c                          <- verbatim, unescaped
X-VENDOR-ID;X-PARAM=keepme:vendor-42  <- params survive
END:VEVENT / END:VCALENDAR

DROPPED: ["CREATED","CATEGORIES","GEO","RELATED-TO","PRIORITY","RESOURCES",
          "COMMENT","CONTACT","COLOR"]
byte_stable_second_pass=true  structural_eq_after_roundtrip=true
```

So for `Event` today: **(b) parsed into typed fields** = the fixed list in
`from_icalendar.rs`; **(a) preserved verbatim** = `X-*` only; **(c) dropped** =
everything else, including every IANA property caldir does not model. Deliberate:
`specs/caldir.md:211-226` "Fields We Skip" already declares `CREATED, PRIORITY,
CATEGORIES, GEO, RESOURCES, CONTACT, COMMENT` out of scope. (It also lists `RDATE`,
which *is* implemented — `event/recurrence.rs:5-17`. Stale row.)

### A generic "preserve everything" bag is not free

Measured on 0.17.10, re-emitting parsed unknown properties through the crate's
`Property` model is itself lossy:

- `CATEGORIES:WORK,URGENT` -> `CATEGORIES:WORK\,URGENT` (`Text` per `value_types.rs:53`,
  escaped at `properties.rs:203`): two categories become one for any third party.
  One-time mutation, stable afterwards.
- Two `RELATED-TO` in, one out — absent from `MULTIS` (`parser/properties.rs:27-44`).
- `DTSTAMP` is regenerated every serialize (`components.rs:99-102`, visible above); a bag
  that captured and compared it would diff forever.

`X-*` is the one class the crate round-trips verbatim (unknown name -> `ValueType`
`None` -> never escaped, never unescaped) — which is why caldir's carrier is `X-`-shaped.
### The preserve-vs-compare tension, and how `Event` resolves it

`Event`'s `PartialEq` is educe-tuned (`event.rs:38`, `:283-292`) and `==` *is* the
"content changed?" test driving sync direction (`diff/calendar_diff.rs:58`).
`x_properties` are preserved **and compared**, with three softenings: volatile
properties are never captured at all (`DTSTAMP`/`CREATED` fall outside the `X-` filter);
`last_modified` and `sequence` are `#[educe(PartialEq(ignore))]`; `XProperty::eq` ignores
`params` (`x_property.rs:12-16`) and comparison is order-independent (`event.rs:283`).
The resolution is not "preserve without comparing" — it is *narrow the captured set to
stable things, then compare them*. Its write-side counterpart is
`with_x_properties_merged_from` (`event.rs:257`), called only on push (`remote.rs:51`,
where `from` is the **remote** event), so a local write cannot strip provider metadata
off the server.

The same hazard exists for todos, and is not hypothetical: Apple Reminders rides these
CalDAV ops with no mapping layer of its own (provider-matrix, iCloud) and stamps
`X-APPLE-SORT-ORDER` on VTODOs. `XProperty` is component-agnostic (domain-model
"Seams"), so the same function covers todos verbatim — but only if `Remote::apply_change`
is taught to call it on the todo update path.

## Options considered

**O1 — `Todo` matches `Event` exactly:** typed ADR-04 set + `X-*` verbatim, everything
else dropped; loss stated in the spec and pinned by a gating test. Blast radius:
`todo/from_icalendar.rs`, `todo/to_icalendar.rs`, one line in `remote.rs:41-60`,
`specs/caldir.md:211-226`, two tests. One fidelity model across both components; the
compare/churn question is already answered by existing, tested code. Cost: a
hand-authored `CATEGORIES` or `RELATED-TO` is destroyed on the first push, on disk *and*
on the server (CalDAV PUT replaces the whole resource, `ops/create_event.rs:34-39`).

**O2 — a general `other_properties: Vec<RawProperty>` bag on `Todo` only.** Adds a type,
parse/serialize paths, an equality decision, and a deny-list for volatiles (`DTSTAMP`,
`LAST-MODIFIED`, `SEQUENCE`, `UID`, plus every ADR-04 property or they emit twice). Fair
to it: the only option that actually saves a user's `CATEGORIES`. But per the
measurements it is still lossy, so it buys a weaker guarantee than it advertises, and it
makes todos *more* faithful than events inside one codec — two fidelity models.

**O3 — raw-line preservation** (store unparsed source lines, splice on write, mirroring
`Reminder::ics_block` / `event.rs:249`). The only genuinely verbatim option; sidesteps
both the escaping bug and the `MULTIS` gap. Costs a second splice literal, line
unfolding, ordering rules, and an interaction with `to_ics_string`'s single-pass
`replacen` — and puts byte-level state on a structurally-compared type.

## Decision

**Adopt O1. `Todo` preserves exactly what `Event` preserves — no more, no less.**
An implementer must:

1. Capture `X-*` on `Todo` with the same filter as `from_icalendar.rs:60-65`, reusing
   `XProperty` unchanged (name, value, params).
2. Emit them outbound exactly as `to_icalendar.rs:93-95` does.
3. Mirror `Event`'s `PartialEq`: `#[educe(PartialEq(ignore))]` on `last_modified` and
   `sequence`, `#[educe(PartialEq(method(x_properties_eq)))]` on `x_properties`.
   `x_properties` are **preserved AND compared**. Do not add an `ignore` here — an
   ignored bag makes a remote-side metadata change invisible to `calendar_diff.rs:58`
   forever.
4. Do **not** capture `DTSTAMP`, `CREATED`, or any other non-`X-` property. That
   exclusion is what keeps the compared set stable and is why `Event` does not churn.
5. Call `with_x_properties_merged_from` on the todo push path in `Remote::apply_change`
   (`remote.rs:51`), passing the remote item as `from`.
6. Amend `specs/caldir.md:211-226`: state the guarantee below, scope it to VEVENT and
   VTODO, add `RELATED-TO` to the table, drop the stale `RDATE` row, and remove whatever
   ADR-04 models (e.g. `PRIORITY`) in the same PR.
7. Add two gating tests (risk R9): a todo round-trip asserting the dropped-name list,
   and a two-pass `parse -> serialize -> parse -> serialize` asserting structural `==`.

**The user guarantee, one sentence for the spec and the docs:**

> caldir round-trips a task's modelled properties plus every `X-` property verbatim,
> parameters included; any other property — `CATEGORIES`, `GEO`, `RELATED-TO`,
> `RESOURCES`, `COMMENT`, `CONTACT`, `CREATED`, and any other IANA or vendor property
> caldir does not model — is dropped the first time caldir writes that item, on disk and
> on the server alike, exactly as it already is for events.

## Consequences

Easy: `Todo` inherits a fidelity and equality model the existing 686 tests already
exercise for `Event`; no new churn surface; nothing new on the RPC wire.
Hard: a user who hand-adds `CATEGORIES` to a task loses it on the next push;
`RELATED-TO` hierarchies stay out of reach (already unreachable — system-map C3).
Must be tested: the dropped-name list (so a future change is a deliberate diff);
two-pass structural stability across the CI TZ matrix; `x_properties` order-independence
and params-ignored equality mirrored from `Event`; and that pushing a todo whose remote
copy carries `X-APPLE-SORT-ORDER` re-emits rather than strips it.

## Backward-compatibility impact

None, on all four axes. **Disk:** no new property written or required; existing `.ics`
files parse identically and already lost these properties. **`.caldir/` state:** bases
re-serialize from the domain type, so base and local file agree by construction — no
phantom first-sync push. **RPC wire:** `X-*` already travels inside the opaque ICS
payload (`rpc/ics.rs`); no field, no verb, no `Method` change (A20 respected).
**Installed provider binaries:** same property vocabulary as today.

## Rejected because

- **O2** — partial fidelity at the price of two divergent fidelity models in one codec,
  and it cannot deliver the guarantee it implies (`CATEGORIES` escaping, `RELATED-TO`
  collapse remain).
- **O3** — verbatim, but byte-level state on a structurally-compared type is a large,
  MVP-inappropriate change that re-opens the tension it was meant to close.

## Open sub-questions

1. **UNVERIFIED:** does Radicale (or iCloud Reminders) inject any *non-*`X-` property on
   a VTODO that caldir would then destroy on the next push? Cheapest experiment: PUT a
   minimal VTODO to Radicale, GET it back, diff the property-name sets.
2. **UNVERIFIED:** a follow-up bag (O2/O3) should apply to `Event` and `Todo` together,
   but must be its own ADR — changing `Event`'s captured set changes `==` and therefore
   sync direction for existing users. Cheapest experiment: prototype the raw-line splice
   on `Event` behind a test and see whether any existing equality test flips.
3. `PRIORITY`'s home is ADR-04's call; this ADR only requires that whatever ADR-04 models
   leaves the "Fields We Skip" table in the same PR.
