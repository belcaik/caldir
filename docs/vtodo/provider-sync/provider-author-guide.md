# Provider author guide

For an out-of-tree provider author (Proton is the motivating case, §17) implementing task
capability declaration against `caldir-core`. You should not need to open
`caldir-provider-caldav` or `caldir-provider-google` to follow this — that is this document's
acceptance test (D-5).

This covers **capability declaration and error behaviour end to end.** It does not cover the task
write lifecycle (create/update/delete RPCs), provider identity metadata, canonicalization rules, or
a reference implementation — those sections are named below as **`P6-01`, not yet written**, so a
gap you find here is either that (deliberate) or a bug in this document (not deliberate).

For the VTODO domain model (what a task is, its fields, its identity) see
[`docs/vtodo/spec.md`](../spec.md). For the sync contract this guide implements against, see
[`spec.md`](spec.md) and its companion, [`capabilities.md`](capabilities.md), which this guide
assumes and does not repeat.

## What you implement

You implement `caldir_core::provider::Handler` (`caldir-core/src/provider/handler.rs`) and pass
your implementation to `run_provider`, which owns the stdin/stdout JSON-RPC loop for you. You never
hand-roll wire parsing.

```rust
#[async_trait::async_trait]
pub trait Handler: Send + Sync {
    async fn connect(&self, cmd: Connect) -> Result<ConnectResponse>;
    async fn list_calendars(&self, cmd: ListCalendars) -> Result<Vec<CalendarConfig>> { .. }
    async fn list_events(&self, cmd: ListEvents) -> Result<Vec<CalendarItem>> { .. }
    async fn create_item(&self, cmd: CreateItem) -> Result<CalendarItem> { .. }
    async fn update_item(&self, cmd: UpdateItem) -> Result<CalendarItem> { .. }
    async fn delete_item(&self, cmd: DeleteItem) -> Result<()> { .. }
    async fn capabilities(&self, cmd: Capabilities) -> Result<ProviderCapabilities> { .. }
}
```

Only `connect` is required; every other method has a default. This guide is about `capabilities`
and about what happens on both sides when a method is missing or fails.

**C-16: the Rust API is item-typed; the wire is not.** `create_item`/`update_item`/`delete_item`
and their `CreateItem`/`UpdateItem`/`DeleteItem`/`CalendarItem` types are Rust-side names only —
they exist so a provider's `match` can serve a task once it has one. On the wire, nothing changed:
the RPC verbs are still `create_event`, `update_event`, `delete_event`, and the JSON payload key is
still `"event"`. Do not go looking for a renamed verb or key; there isn't one.

## Declaring task support

Override `capabilities`:

```rust
async fn capabilities(&self, cmd: Capabilities) -> Result<ProviderCapabilities> {
    Ok(ProviderCapabilities {
        events: Capability { read: true, write: true },
        tasks: Capability { read: true, write: true },
    })
}
```

If you do nothing at all, the default answers `events: { read: true, write: true }, tasks: { read:
false, write: false }` — see [`capabilities.md` §4](capabilities.md#4-the-default-events-only) for
why `events.write` being `true` there is not a promise your provider can actually write events; it
is an untouched pre-existing baseline, not something you opted into.

**`cmd.remote` carries the remote's config** (a flattened `RemoteConfigParams` map — the same shape
`ListEvents` receives). Use it if one binary of yours serves more than one kind of remote with
different task support — for example if, like Google, a "calendar" remote and a "task list" remote
are the same binary. See [`capabilities.md` §3](capabilities.md#3-capabilities-are-per-remote-not-per-binary)
before you write this method: **this is the single most likely thing to get wrong.** If your
provider only ever serves one kind of remote, ignore `cmd.remote` and return a constant.

**The four checklist questions (this guide's own acceptance test):**

1. *What does `capabilities` return if I implement nothing?* `events: { read: true, write: true
   }`, `tasks: { read: false, write: false }` — locked by
   `an_unmodified_provider_reports_events_only_capabilities`.
2. *What happens to my tasks if I return `tasks.write=false`?* caldir never calls your (not-yet-
   specified — `P6-01`) task-write RPCs against this remote; your local task data is left alone.
3. *What if I return `tasks.write=true` but only implemented create?* Don't. `tasks.write=true` is
   one bit that promises create, update **and** delete are all coherent (§4 of the sync spec); there
   is no partial-support field.
4. *What does a caldir newer than my binary do if it asks something I've never heard of?* It gets a
   typed error naming your provider and the unrecognized command — never malformed JSON, never a
   panic. See "Old binaries and protocol skew" below.

## Fail-closed: you cannot accidentally opt into task writes

Every way your `capabilities` answer can fail to reach caldir — you return an `Err`, your process
dies or times out, your binary predates the `capabilities` verb, your reply doesn't parse, or your
reply simply omits the `tasks` key — is read as `tasks: { read: false, write: false }`, identical to
an explicit `false`. caldir preserves an error reason for the user (an `Option<RemoteError>`
alongside the returned capabilities) but never reads your silence, your crash, or your absent field
as consent to push tasks. Full enumeration and the test locking each row:
[`capabilities.md` §6](capabilities.md#6-fail-closed-every-way-the-answer-can-fail-to-arrive).

The only way your provider gets task writes is to return `tasks.write=true` on purpose, from a
working `capabilities` handler.

## Old binaries and protocol skew

If you build against an older `caldir-core` than the one calling you, or a newer caldir asks your
binary a verb you've never heard of, you do **not** need to do anything special — `run_provider`
already handles it. The wire envelope's `command` is decoded as a plain string before it is ever
matched against `Method`, so an unrecognized verb produces a typed, self-describing error
(`"{provider} does not support the \"{command}\" command; the provider may need updating"`,
tagged `ErrorCode::UnsupportedMethod` on the wire) rather than a parse failure or a panic. This is
locked by `an_unknown_command_is_reported_as_a_version_error` and (for a typo'd *known* verb, which
must still fail loudly rather than silently dispatch) `a_typo_in_a_known_command_is_still_loud`.

On the caller side (not your code, but useful to know what caldir sees): this reply becomes
`ProviderError::UnsupportedMethod`, distinct from a generic provider error, locked by
`an_unsupported_method_reply_becomes_a_typed_provider_error`. You do not construct or match this
type yourself — it is the CLI's job to match on `ProviderError` (see "Types you can actually name"
below for what is nameable and by whom), never your `Handler` implementation's — but it is why
upgrading caldir against your unmodified binary degrades gracefully instead of breaking.

`Method` itself is a closed, non-`#[non_exhaustive]` enum. If you match on it directly (most
providers dispatch through `Handler` and never do), add a wildcard arm defensively; a future verb
added to `caldir-core` is source-incompatible with an exhaustive match, by design (see
[`capabilities.md` §7](capabilities.md#7-protocol-versioning-and-old-binary-behaviour)).

## The six failure classes (§16)

§16 requires the CLI be able to distinguish six kinds of provider failure without parsing your
error message. `rpc::ErrorCode` (in `caldir-core/src/rpc.rs`) is the wire-level discriminator that
carries the class alongside the message:

- `unsupported_operation` — you understood the request but do not support this kind of write (e.g.
  task writes on an events-only provider). `create_item`/`update_item`/`delete_item`'s default
  bodies already carry this class; you get it for free by not overriding them.
- `missing_auth_scope` — your stored credentials lack the scope the request needs.
- `remote_not_found` — the remote object named in the request does not exist.
- `conflict` — a conflict or precondition failure (e.g. a stale ETag).
- `semantic_limitation` — you cannot represent what was asked (distinct from an outright refusal).
- `network` — your own call to the remote API failed for network reasons. Distinct from a failure
  in the exchange between caldir and your binary, which caldir already detects on its own.

Attach any of the six classes to an error you return from a `Handler` method with
`caldir_core::provider::classified_error`:

```rust
use caldir_core::provider::classified_error;
use caldir_core::rpc::ErrorCode;

async fn create_item(&self, cmd: CreateItem) -> caldir_core::provider::Result<CalendarItem> {
    if self.token_lacks_calendar_scope() {
        return Err(classified_error(
            ErrorCode::MissingAuthScope,
            "stored token lacks the calendar.events scope; reauth required",
        ));
    }
    // ...
}
```

The class rides the same `Err(Error)` your `Handler` method already returns — no trait signature
changes, so this is a drop-in addition to error paths you already have. Return a plain
string/`anyhow`/etc. error instead and it lands as the unclassified/generic class, exactly like
before `classified_error` existed: attaching a class is opt-in, not required. This is what the
`create_item`/`update_item`/`delete_item` trait defaults use internally to tag their own
`unsupported_operation` refusal — `classified_error` is that same mechanism, made public.

`ErrorCode` is `#[non_exhaustive]` and unknown values decode as a generic "unrecognized" bucket
rather than failing to parse — so a class added to a future `caldir-core` never breaks an older
core reading your binary's replies, and a class your binary doesn't yet emit is not a compatibility
requirement. Locked by `provider::error::tests::every_error_class_round_trips_over_the_wire`.

The `unsupported_method` class from the section above is a different thing: it is caldir's own
signal that your binary was asked a verb it has never heard of, not one of the six §16 classes, and
you never produce it yourself.

## Types you can actually name

Everything above only works if you can name and construct the types involved from outside
`caldir-core`. Checked against `caldir-core/src/lib.rs`'s and `caldir-core/src/provider.rs`'s
`pub use` lists, and locked by `a_downstream_crate_can_name_and_construct_provider_capabilities`:

- `caldir_core::provider::{Capability, ProviderCapabilities}` — public, `Copy`, constructible by
  struct literal, not `#[non_exhaustive]`.
- `caldir_core::RemoteError` — public, but **`#[non_exhaustive]`**: any `match` you write against
  it needs a wildcard `_` arm, or it fails to compile the moment a new variant is added upstream.
  This is a compile error you will hit the first time, not a runtime surprise.
- `caldir_core::ConnectionError` — same: public, `#[non_exhaustive]`, wildcard arm required.
- `caldir_core::provider::ProviderError` — public, `#[non_exhaustive]`, wildcard arm required
  (`W2-C1`). This is the type the CLI matches to get the six §16 classes above out of
  `RemoteError::Provider` without parsing a message.
- `caldir_core::provider::ProviderTransportError` — public, `#[non_exhaustive]`, wildcard arm
  required. Nested inside `ProviderError::Transport`; you need it only to pick the `network` class
  (`ProviderTransportError::Network`) out from an unrelated local subprocess failure.

None of this is something your `Handler` implementation constructs or matches — these are the
CLI's types for consuming failures your provider produces, not yours for producing them. To
*attach* a class to your own error, use `caldir_core::provider::classified_error`; see "The six
failure classes" above.

## Where you write state

If your provider needs on-disk state — OAuth tokens, an app password, a sync cursor — write it
under the path in the `CALDIR_PROVIDER_STORAGE_DIR` environment variable, falling back to
`~/.config/caldir/providers/{your-provider-name}/` if that variable is unset. Use
`caldir_core::provider::ProviderStorage::for_provider(name)` to resolve this path rather than
hard-coding it. **Never write inside a calendar directory** — that tree belongs to the user's ICS
files and caldir's own `.caldir/` sync state, not to your provider's private bookkeeping.

## Not yet written (`P6-01`)

This guide and `capabilities.md` are continued by `P6-01`, which will add, in these same two files:

- The create, update and delete contracts for tasks (what RPCs you implement, what shape they take,
  what "coherent" means for `tasks.write=true` beyond the one-bit promise above).
- Provider identity metadata.
- Canonicalization rules.
- A minimal reference provider you can read end to end.

If you need any of those today, they do not exist yet — this is not a documentation gap to work
around by reading `caldir-provider-caldav` or `caldir-provider-google` (doing so would defeat the
reason this guide exists; see D-5 in the provider-sync ticket index if you want the rationale).

Provider-specific behaviour for the in-tree providers (Google Tasks mapping, CalDAV specifics) is
documented separately under `docs/vtodo/provider-sync/providers/`, and is not part of this
contract — it should never be a prerequisite for implementing your own.
