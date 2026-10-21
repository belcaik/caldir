# Provider author guide

For an out-of-tree provider author (Proton is the motivating case, §17) implementing task
capability declaration against `caldir-core`. You should not need to open
`caldir-provider-caldav` or `caldir-provider-google` to follow this — that is this document's
acceptance test (D-5).

This covers capability declaration, error behaviour, the task write lifecycle (create/update/delete),
provider identity metadata, canonicalization rules, and a minimal reference implementation — the
full contract §17 requires. If you find a gap after reading this file, `capabilities.md`, and
[`docs/vtodo/spec.md`](../spec.md) in full, it is a bug in this document, not a deliberate omission.

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
2. *What happens to my tasks if I return `tasks.write=false`?* caldir never calls your
   `create_item`/`update_item`/`delete_item` (see "Create contract" etc. below) against this
   remote; your local task data is left alone.
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

## List/pull contract

`list_events` (wire verb `list_events`, type `rpc::ListEvents`) is how caldir asks for everything —
events and tasks alike — in a window. There is no separate `list_tasks` verb and there will not be
one: §5 requires the existing item-oriented protocol to be extended rather than duplicated per
component kind.

```rust
pub struct ListEvents {
    pub remote: RemoteConfigParams, // flattened; see "Provider identity metadata" below
    pub from: String,
    pub to: String,
}
```

You return `Result<Vec<CalendarItem>>`. `CalendarItem` is `Event(Event)` or `Todo(Todo)`
(`caldir_core::CalendarItem`, `caldir-core/src/item.rs`) — a task is a `VTODO` ICS document on the
wire exactly like an event is a `VEVENT` one; both travel as plain ICS strings inside the same
`Vec`. If you advertise `tasks.read=true`, put your `Todo`s in the same vector as your `Event`s;
caldir sorts identity out on its side by component kind (see "Provider identity metadata" below).

`from`/`to` are advisory bounds, not a hard contract you must filter to exactly. Returning items
slightly outside the window is harmless — caldir applies its own window logic — but returning
*fewer* tasks than actually exist inside the window is not: see "tasks.read is a promise" under
Error behaviour below for why an incomplete answer must be an error, never a short list.

## Create contract

`create_item` (wire verb `create_event`, wire payload key `"event"` — see C-16 above) receives
`rpc::CreateItem`:

```rust
pub struct CreateItem {
    pub remote: RemoteConfigParams,
    pub event: CalendarItem, // may be Event or Todo
}
```

You return `Result<CalendarItem>`. **The return value is not an acknowledgement — it is the item
that becomes the sync base for this cycle.** After a successful create, `push_outgoing_changes`
(`caldir-core/src/connection.rs`) writes whatever you returned back over the user's local file and
records it as the next sync base. Concretely:

- If your remote assigns its own ID, mutates a timestamp, reformats a description, or otherwise
  changes anything about the item on write, your response must reflect the *post-write* remote
  state — not an echo of `cmd.event` unchanged. Returning the request unmodified when the remote
  actually altered it means the next sync compares the local file against a base that doesn't match
  what the remote holds, and creates a duplicate.
- The returned item's identity (uid/recurrence-id/kind — see "Provider identity metadata") must
  match `cmd.event`'s, unless your remote is one of the rare providers that reassigns UIDs
  server-side; `push_outgoing_changes` looks the original item up by its *pre-write* identity to
  apply your response, so a returned item with the wrong identity is silently dropped rather than
  causing an obvious error.
- See "Canonicalization rules" below for exactly which fields you're allowed to leave untouched
  versus which you must overwrite from the remote's own response.

If you can't create the item — wrong capability, missing scope, remote rejected the payload —
return a classified error (see "Error behaviour" below) rather than a best-effort partial write.

## Update contract

`update_item` (wire verb `update_event`) receives `rpc::UpdateItem`, the same two-field shape as
`CreateItem`. Two rules specific to update:

- **Resolve the target by stored provider identity, never by summary/title or any other mutable
  field.** The provider's own id — round-tripped through an `X-`-prefixed property, see "Provider
  identity metadata" — is the only stable handle you have across edits; a task's summary can and
  will change out from under you between syncs.
- Return the canonicalized item, exactly as for create: whatever the remote's write actually
  produced, not `cmd.event` echoed back. The same second-sync-duplicate/silent-drop hazard applies.

## Delete contract

`delete_item` (wire verb `delete_event`) receives `rpc::DeleteItem` (same shape again) and returns
`Result<()>` — there is no item to canonicalize on a delete.

Resolve the target the same way as update: by stored provider identity. **An already-absent remote
item is a success, not an error.** If the remote object named by `cmd.event`'s provider id is
already gone — deleted out-of-band, or a retried delete after a dropped response — return `Ok(())`.
Treating "not found" as a failure here would leave a task local caldir believes it deleted stuck in
a retry loop against a remote that has nothing left to delete.

## Provider identity metadata

The RFC `UID` (plus recurrence id, plus `ComponentKind` — see the note below) is caldir's logical
identity and you never replace it, reuse it for your own resource id, or treat it as optional.
`CalendarItem::instance_id()` (`caldir-core/src/item.rs`) returns an `EventInstanceId`
(`caldir-core/src/event/instance_id.rs`) built from exactly those three parts — **identity has three
parts, not two.** `EventInstanceId::todo(uid, recurrence_id)` sets `ComponentKind::Todo`; an event
and a task that happen to share a `UID` are still distinct items, because the kind is part of the
identity, not an afterthought. A guide (or a provider) that treats identity as `(uid,
recurrence_id)` alone will collide an event and a task that share a UID.

Your own remote-assigned id (a CalDAV href, a Google Task id, whatever your API calls it) does not
go in the `UID`. It round-trips through an `X-`-prefixed custom property on the `Todo`/`Event`'s
`x_properties: Vec<XProperty>` (`caldir_core::XProperty`, `caldir-core/src/event/x_property.rs`),
under a name you own — the shipped convention is `X-{PROVIDER}-{KIND}-ID` (`X-GOOGLE-EVENT-ID`,
`X-GOOGLE-TASK-ID`), but the naming is yours to choose; nothing else depends on the string. This is
how you find the right remote object on `update_item`/`delete_item`, and it is why the create
contract above requires your returned item to carry the property: **if it's missing from the item
you return from `create_item`, it never reaches the sync base, and the next sync — having no
provider id to resolve against — creates a duplicate remote object instead of updating the one that
already exists.**

Remote-scoped settings that aren't part of the task itself (an account id, a task-list id, a
provider-private feature flag) belong in `RemoteConfigParams` — the free-form, flattened map every
RPC's `remote` field carries (`caldir_core::RemoteConfigParams`, `caldir-core/src/remote/config.rs`,
backed by a `BTreeMap<String, toml::Value>`). It is written into the calendar's `[remote]` TOML
table, and caldir neither validates nor interprets its contents — every key is yours.

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

### `tasks.read=true` is a promise, not a hope

Advertising `tasks.read=true` from `capabilities` (see "Declaring task support" above) is a
statement that you can honestly answer a task query for that remote. Only advertise it where that's
true — per remote, not per binary, exactly as §4 requires for `tasks.write`.

**A failed or degraded task enumeration from `list_events` must be a returned error, never an empty
or partial list.** An empty `Vec<CalendarItem>` is not "no tasks found this time" — it is read as an
assertion that the remote holds no tasks at all. Because deletes are detected by absence against
the stored sync base (see [`docs/vtodo/spec.md`](../spec.md) for the sync model), a provider that
advertises `tasks.read=true` and then returns a short list because of a transient failure causes
caldir to delete every local task that has a sync base — a class of bug that reads as "sync worked,
data vanished," not as a visible error. If you cannot complete the enumeration, return
`Err(classified_error(ErrorCode::Network, ..))` (or whichever class fits) from `list_events`
instead of returning what you managed to gather.

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

## Canonicalization rules

This is the general rule the create and update contracts above both point at, stated once: **where
your remote's representation of a task is narrower than caldir's local one — it drops a field,
rewrites a value, assigns its own timestamp — your `create_item`/`update_item` response must return
the canonical remote form of everything the remote actually changed, so the stored sync base matches
what the remote holds and the next sync is quiet.** A base that doesn't match the remote produces
spurious diffs or duplicate writes on the very next cycle; that's the whole reason the return type
is an item and not `()`.

The safe pattern is: **start from the local item you were asked to write, and overwrite only the
fields your remote's response actually canonicalizes** — never build the response from the remote's
reply alone. Fields the remote doesn't touch, doesn't understand, or doesn't echo back must survive
untouched from the local input; a response assembled purely from what the remote sent back silently
destroys every field the remote is simply silent about, because whatever you return is written over
the user's file verbatim.

This is not a hypothetical failure mode. A shipped provider once put a task's `STATUS` in the
"overwrite from remote" list because the remote's create/update response always included a status
value. The bug: that remote has no equivalent of an "in progress" status, so its API silently
downgrades anything it doesn't recognize to its own default before ever echoing a value back. The
response's status field was never new information — it was the remote parroting its own downgrade —
but naively trusting it meant every task saved locally as in-progress reverted to not-started on
every single push, because the provider dutifully copied that downgraded value onto the returned
item's status field instead of keeping the status the user had actually set locally. The fix was to
special-case exactly the values the remote cannot represent: keep the local value for those, and
only take the remote's status for the values it genuinely round-trips. Generalized: **before you put
a field on your "always overwrite from remote" list, ask whether the remote can actually represent
every value that field can hold — if it can't, the remote's echo for the values it can't represent
is not information, it's just its own lossy default reflected back at you.**

## A minimal reference provider

A complete `Handler` implementation for a hypothetical provider named `reference`, backed by an
in-memory store, advertising `tasks.{read,write}=true` and nothing else. It compiles as written
against the public API named throughout this guide, given a `Cargo.toml` with `caldir-core`,
`async-trait`, `tokio` (with the `rt-multi-thread` and `macros` features, for `#[tokio::main]`), and
`uuid` (with the `v4` feature) as dependencies.

```rust
use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use caldir_core::provider::{
    self, Capability, ProviderCapabilities, classified_error, run_provider,
};
use caldir_core::rpc::{
    Capabilities, Connect, ConnectResponse, CreateItem, DeleteItem, ErrorCode, ListEvents,
    UpdateItem,
};
use caldir_core::{CalendarItem, Todo, XProperty};

/// The name your compiled binary must carry: caldir discovers providers as
/// `caldir-provider-{name}` on `PATH` (`PROVIDER_BINARY_PREFIX`,
/// `provider_slug_from_filename`, `caldir-core/src/provider/slug.rs`). This
/// binary must therefore be named `caldir-provider-reference`.
const PROVIDER_ID_PROPERTY: &str = "X-REFERENCE-TASK-ID";

#[derive(Default)]
struct ReferenceProvider {
    // Keyed by the provider-assigned id stored in `PROVIDER_ID_PROPERTY`.
    tasks: Mutex<HashMap<String, Todo>>,
}

impl ReferenceProvider {
    fn provider_id(todo: &Todo) -> Option<String> {
        todo.x_properties
            .iter()
            .find(|p| p.name == PROVIDER_ID_PROPERTY)
            .map(|p| p.value.clone())
    }

    fn as_todo(event: CalendarItem) -> provider::Result<Todo> {
        match event {
            CalendarItem::Todo(todo) => Ok(todo),
            CalendarItem::Event(_) => Err(classified_error(
                ErrorCode::UnsupportedOperation,
                "this reference provider only serves tasks",
            )),
        }
    }
}

#[async_trait]
impl provider::Handler for ReferenceProvider {
    async fn connect(&self, _cmd: Connect) -> provider::Result<ConnectResponse> {
        Ok(ConnectResponse::Done {
            account_identifier: Some("reference-account".to_string()),
            calendars: None,
        })
    }

    async fn list_events(&self, _cmd: ListEvents) -> provider::Result<Vec<CalendarItem>> {
        let tasks = self.tasks.lock().unwrap();
        Ok(tasks.values().cloned().map(CalendarItem::Todo).collect())
    }

    async fn create_item(&self, cmd: CreateItem) -> provider::Result<CalendarItem> {
        let mut todo = Self::as_todo(cmd.event)?;
        let id = uuid::Uuid::new_v4().to_string();
        todo.x_properties
            .retain(|p| p.name != PROVIDER_ID_PROPERTY);
        todo.x_properties
            .push(XProperty::new(PROVIDER_ID_PROPERTY, &id));

        self.tasks.lock().unwrap().insert(id, todo.clone());
        Ok(CalendarItem::Todo(todo))
    }

    async fn update_item(&self, cmd: UpdateItem) -> provider::Result<CalendarItem> {
        let todo = Self::as_todo(cmd.event)?;
        let id = Self::provider_id(&todo).ok_or_else(|| {
            classified_error(ErrorCode::RemoteNotFound, "task carries no provider id")
        })?;

        let mut tasks = self.tasks.lock().unwrap();
        if !tasks.contains_key(&id) {
            return Err(classified_error(
                ErrorCode::RemoteNotFound,
                format!("no task with id {id}"),
            ));
        }
        tasks.insert(id, todo.clone());
        Ok(CalendarItem::Todo(todo))
    }

    async fn delete_item(&self, cmd: DeleteItem) -> provider::Result<()> {
        let todo = Self::as_todo(cmd.event)?;
        if let Some(id) = Self::provider_id(&todo) {
            // Already absent is success, not an error — see "Delete contract" above.
            self.tasks.lock().unwrap().remove(&id);
        }
        Ok(())
    }

    async fn capabilities(&self, _cmd: Capabilities) -> provider::Result<ProviderCapabilities> {
        Ok(ProviderCapabilities {
            events: Capability { read: false, write: false },
            tasks: Capability { read: true, write: true },
        })
    }
}

#[tokio::main]
async fn main() {
    run_provider(ReferenceProvider::default()).await;
}
```

This omits `list_calendars` (its default already returns a clean "not supported" refusal, which is
correct for a provider with exactly one implicit task list) and any actual network/disk persistence
— add both once you have a real remote and a real multi-calendar account model to serve.

## Further reading

Provider-specific behaviour for the two in-tree providers — Google Tasks' field mapping and
lossiness, CalDAV's property-fidelity table — is documented separately under
`docs/vtodo/provider-sync/providers/` (`caldav.md`, `google-tasks.md`). It is worked example, not
prerequisite: nothing in this guide or in `capabilities.md` requires reading either that
documentation or the `caldir-provider-caldav`/`caldir-provider-google` source to implement your own
provider's task support.
