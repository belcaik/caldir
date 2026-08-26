# Capability contract

The task-capability half of the [provider-sync spec](spec.md) (§4, §17), as shipped by `P1-01` and
`P1-02`. For the VTODO domain model itself (what a task *is*), see
[`docs/vtodo/spec.md`](../spec.md) — this document does not restate it.

The write lifecycle (create/update/delete contracts, provider identity metadata,
canonicalization rules) is **`P6-01`, not yet written**. See "Deferred to P6-01" below.

## 1. The shape

```rust
pub struct Capability {
    pub read: bool,
    pub write: bool,
}

pub struct ProviderCapabilities {
    pub events: Capability,
    pub tasks: Capability,
}
```

Both types live at `caldir_core::provider::{Capability, ProviderCapabilities}`, are public,
`Copy`, `Default`, and constructible by struct literal outside the crate — locked by
`a_downstream_crate_can_name_and_construct_provider_capabilities`. Neither is
`#[non_exhaustive]`: a caller can build a `ProviderCapabilities` directly without a builder or a
wildcard arm.

Every field is `#[serde(default)]`. An absent JSON key decodes as `false`, never `true` — locked by
`an_absent_capability_field_reads_as_false`, which checks both a missing `tasks` key and a
present-but-empty `tasks: {}`. This is the one bug in this contract that would delete a user's data
downstream: absence read as support.

## 2. The `capabilities` RPC verb

`Method::Capabilities` is the seventh (and, as of this writing, newest) verb. The request,
`rpc::Capabilities`, flattens `RemoteConfigParams` exactly the way `ListEvents` does:

```rust
pub struct Capabilities {
    #[serde(flatten)]
    pub remote: RemoteConfigParams,
}
```

caldir calls it through `Remote::capabilities() -> (ProviderCapabilities, Option<RemoteError>)`.
This call is **fetched at execution time and never persisted** — it is not a field of
`ConnectResponse`, and it is not a key in `.caldir/config.toml`. A provider binary upgrade is
visible on the very next call; there is nothing to migrate or invalidate.

## 3. Capabilities are per-remote, not per-binary

**This is the single most likely thing to get wrong.** The request carries the remote's config
params so that one provider binary can answer differently for different remotes of the same kind —
for example, a Google Calendar remote and a Google Tasks list remote are served by the *same*
binary, and only the Tasks-list remote may answer `tasks.write=true`.

A provider that ignores `cmd.remote` and always returns the same `ProviderCapabilities` is legal —
it just means that provider's answer is a binary-wide constant, and per D-2 a binary-wide constant
can only ever be the conservative floor across every remote it serves. A provider that wants a
richer answer must branch on its own remote-config keys (private to that provider — see
[§4](spec.md), `RemoteConfigParams` is a free-form map) inside its `capabilities` implementation.

Locked by two tests in `caldir-core/src/provider/handler.rs`:
- `one_handler_answers_two_remotes_differently` — the same `Handler` returns `tasks.write=false`
  for one remote and `tasks.write=true` for another, from the same process, based only on the
  flattened params.
- `the_default_answers_events_only_whatever_the_remote_says` — the unmodified default is *not*
  itself per-remote; per-remote variance is something a provider opts into by overriding
  `capabilities`, never something that falls out of ignoring the params.

## 4. The default: events-only

`Handler::capabilities` has a default implementation:

```rust
async fn capabilities(&self, _cmd: Capabilities) -> Result<ProviderCapabilities> {
    Ok(ProviderCapabilities::events_only())
}
```

`events_only()` returns `events: { read: true, write: true }, tasks: { read: false, write: false
}`, regardless of the remote params passed in. Locked by
`an_unmodified_provider_reports_events_only_capabilities` and
`the_default_answers_events_only_whatever_the_remote_says`.

**Every provider written before this ticket keeps compiling and keeps working, unmodified.** No
existing provider crate needs to implement `capabilities` at all to remain correct.

**Why `events.write` defaults to `true`, not because caldir gates event writes on it.** A webcal
provider cannot write events, and its own `Handler::create_item` default already returns an error
independently of this field (§18 forbids changing that shipped behaviour); the wire verb it fails
against is still `create_event` (C-16 — see the provider-author guide). `events.write: true` in
the default is not a promise that the provider can write — it is the pre-existing, unrestricted
baseline for events, which this ticket does not touch. Fail-closed is a **tasks-only** contract.
Do not design around `events.write` gating anything; nothing reads it that way today.

A provider opts into task support — or a per-remote answer of any kind — only by overriding
`capabilities`. `a_provider_may_advertise_task_writes` is the control: a `Handler` that overrides
`capabilities` to return `tasks: { read: true, write: true }` is honored as-is.

## 5. `tasks.write=true` is a promise about all three verbs

A provider MUST NOT return `tasks.write=true` unless create, update **and** delete are implemented
coherently for tasks (§4). There is no field to declare partial write support (e.g. create-only);
`tasks.write` is one bit covering all three. The write lifecycle itself — what "implemented
coherently" requires of each of the three RPCs — is `P6-01`, not yet written.

## 6. Fail-closed: every way the answer can fail to arrive

`Remote::capabilities()` never propagates an error to its caller. Every failure mode below reads as
identical to an explicit `tasks: { read: false, write: false }` (i.e. `ProviderCapabilities::default()`),
with events also read as unsupported — the fail-closed floor makes no claim about events either. The
failure reason is preserved alongside the value as `Option<RemoteError>` so the caller can surface
an actionable message; it is never silently swallowed.

| Failure mode | What happened | Result |
|---|---|---|
| Provider error | The provider's `capabilities` handler returned `Err` | task-less, reason preserved |
| Transport error | The subprocess pipe broke, I/O failed | task-less, reason preserved |
| Timeout | The provider did not reply in time | task-less, reason preserved |
| Unknown/unsupported method | An old binary predates the `capabilities` verb (see §7) | task-less, reason preserved |
| Unparseable payload | The reply is not a well-formed `ProviderCapabilities` (e.g. `"data":"not an object"`) | task-less, reason preserved |
| Absent field | The JSON reply omits `tasks`, or has `tasks: {}` | task-less (§1) |

All six rows are one enumeration, not a summary — this is the whole point of documenting them as a
table rather than a sentence. In every row: caldir does not push tasks, does not read the absence as
support, preserves local task data untouched, and has a reason string available to show the user.

Locked by `a_provider_that_cannot_answer_capabilities_is_read_as_task_less`, which drives all four
of transport error, timeout, unsupported-method reply, and unparseable payload through
`Remote::capabilities()` and asserts every case returns `ProviderCapabilities::default()` with
`reason.is_some()`. The absent-field row is locked separately by
`an_absent_capability_field_reads_as_false` (§1).

**A provider author cannot accidentally opt into task writes.** There is no code path — error,
timeout, missing field, or old binary — that caldir will read as `tasks.write=true`. The only way to
advertise task writes is to return `tasks.write=true` explicitly, deliberately, from a working
`capabilities` implementation.

## 7. Protocol versioning and old-binary behaviour

`Method` is a closed, non-`#[non_exhaustive]` seven-verb enum (Connect, ListCalendars, ListEvents,
CreateEvent, UpdateEvent, DeleteEvent, Capabilities). Locked at exactly seven by
`method_set_is_exactly_the_seven_known_verbs` — a tripwire meant to fail the day an eighth verb is
added without a matching update here.

**Historical rule, and its one ratified exception.** The VTODO core's rule (`docs/vtodo/spec.md`
R52) was that `Method` gains no variant, ever. `Capabilities` is the one variant this rule has
allowed past it, ratified for this ticket alone — see INDEX §3 C-5 for the record. `Method` is
still not `#[non_exhaustive]`, so every out-of-tree matcher (Proton included) must add a match arm
for `Capabilities` the next time it rebuilds against a `caldir-core` that has it; there is no
`#[serde(other)]` catch-all to fall back on, by design (R52/C-5 for the six pre-existing verbs; see
`method_set_is_exactly_the_seven_known_verbs`'s own comment for why relaxing this to a `>=` check
would defeat the tripwire).

**What an old provider binary does when asked `capabilities` (or any verb newer than its build).**
It does not see malformed JSON and it does not panic. `process_request` decodes the wire envelope's
`command` field as a bare string first, and only afterwards resolves that string against `Method`.
When resolution fails, the response is a typed error naming both the provider and the unrecognized
verb, e.g. `"{provider} does not support the \"{command}\" command; the provider may need
updating"`, carrying `Response::Error.code = Some(ErrorCode::UnsupportedMethod)`. Genuine malformed
JSON (a broken envelope, not just an unknown command) still goes through the pre-existing
"Failed to parse request: …" path — the distinction is decoding the envelope with `command` typed as
`String` rather than `Method`, not a new parser.

On the caller side, `Provider::call` turns an `UnsupportedMethod`-coded error reply into
`ProviderError::UnsupportedMethod(String)`, distinct from the general
`ProviderError::Provider(String)` — so a caller can match on protocol skew instead of
substring-matching an error message. `ProviderError` itself is not publicly nameable outside
`caldir-core` (see §8); this distinction reaches a downstream crate only through
`RemoteError::Provider`, and `Remote::capabilities()` already folds it into the fail-closed floor
(§6) before a caller ever sees it.

The `Response::Error.code` field is additive and `Option`-typed with `#[serde(default)]`, so an
error envelope from a provider built before this field existed still decodes — locked by
`an_error_envelope_from_an_older_provider_still_decodes`.

Locked by, in order of the claims above:
- `method_set_is_exactly_the_seven_known_verbs` — the closed-enum count.
- `an_unknown_command_is_reported_as_a_version_error` — an unrecognized verb produces a typed
  error naming the command, and explicitly not the "Failed to parse request" text.
- `a_typo_in_a_known_command_is_still_loud` — a typo'd known verb (`create_evnet`) is caught here
  too, not dispatched to the nearest handler by accident.
- `an_unsupported_method_reply_becomes_a_typed_provider_error` — the caller side,
  `Provider::call` mapping the coded reply to `ProviderError::UnsupportedMethod`.
- `an_error_envelope_from_an_older_provider_still_decodes` — the additive-field compatibility
  claim itself.

## 8. Naming things: what a downstream crate can actually match on

- `caldir_core::provider::{Capability, ProviderCapabilities}` — public, constructible, not
  `#[non_exhaustive]`.
- `caldir_core::RemoteError` — public, `#[non_exhaustive]`, so every downstream `match` needs a
  wildcard arm. `RemoteError::Provider(ProviderError)` is the variant carrying provider failures,
  but `ProviderError` itself is `pub(crate)` — a downstream crate cannot name it or match into it,
  only `Display` it via `RemoteError`'s own message.
- `caldir_core::ConnectionError` — public, `#[non_exhaustive]`, same wildcard-arm requirement.
- `Response::Error`'s `code: Option<ErrorCode>` and `rpc::ErrorCode` are provider-side wire types
  used inside a `Handler` implementation (see the provider-author guide); a consumer of `Remote`
  never sees `ErrorCode` directly, only the folded `RemoteError`/fail-closed result.

## Deferred to P6-01

Continuing these same two files, not yet written:
- Create, update and delete contracts for tasks.
- Provider identity metadata.
- Canonicalization rules.
- The minimal reference provider.

See [`spec.md` §17](spec.md) for the full list this ticket and `P6-01` jointly satisfy.
