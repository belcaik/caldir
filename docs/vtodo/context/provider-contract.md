# Provider contract: RPC + data types (as of branch `vtodo-support`)

Facts only. All paths relative to repo root. No skill at `~/.claude/skills/provider-rpc/SKILL.md` (absent).

## 1. RPC methods

Wire envelope: request `{"command": <method>, "params": <object>}`, response
`{"status":"success","data":…}` or `{"status":"error","error":"…"}`
(`caldir-core/src/rpc.rs:35-57`).

| Method (wire name) | Request type | Response type (Rust) | Wire type of response | Defined at |
|---|---|---|---|---|
| `connect` | `Connect` | `ConnectResponse` | JSON object (`JsonWireValue`) | `caldir-core/src/rpc/methods/connect.rs:13-23` |
| `list_calendars` | `ListCalendars` | `Vec<CalendarConfig>` | JSON array of objects | `caldir-core/src/rpc/methods/list_calendars.rs:6-13` |
| `list_events` | `ListEvents` | `Vec<Event>` | JSON array of **ICS strings** | `caldir-core/src/rpc/methods/list_events.rs:6-16` |
| `create_event` | `CreateEvent` | `Event` | **ICS string** | `caldir-core/src/rpc/methods/create_event.rs:6-16` |
| `update_event` | `UpdateEvent` | `Event` | **ICS string** | `caldir-core/src/rpc/methods/update_event.rs:6-16` |
| `delete_event` | `DeleteEvent` | `()` | `null` | `caldir-core/src/rpc/methods/delete_event.rs:6-16` |

The closed method set is `enum Method` (`caldir-core/src/rpc/methods.rs:22-31`), `#[serde(rename_all = "snake_case")]`.
Dispatch table: `caldir-core/src/provider/handler.rs:98-109`.
Default per-call timeout 15s, overridable per `Rpc` impl (`caldir-core/src/rpc.rs:19,25`).

Request payload shapes carrying events (all three flatten remote params next to `event`):

```rust
// caldir-core/src/rpc/methods/create_event.rs:5-11 (update_event.rs / delete_event.rs identical)
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct CreateEvent {
    #[serde(flatten)]
    pub remote: RemoteConfigParams,
    #[serde(with = "crate::rpc::ics")]
    pub event: Event,
}
```

```rust
// caldir-core/src/rpc/methods/list_events.rs:5-11
pub struct ListEvents {
    #[serde(flatten)]
    pub remote: RemoteConfigParams,
    pub from: String,   // RFC 3339
    pub to: String,
}
```

`RemoteConfigParams` is an untyped `BTreeMap<String, toml::Value>` (`caldir-core/src/remote/config/params.rs:5-6`) — arbitrary provider-specific keys pass through unchecked.

Core-side callers: `caldir-core/src/remote.rs:24-91` (list/create/update/delete), `caldir-core/src/provider/account.rs:18-24` (list_calendars), `caldir-core/src/provider.rs:60-68` (connect). Generic call path: `caldir-core/src/provider.rs:70-88`.

## 2. `ProviderHandler` trait

The trait is named `Handler` (exported as `caldir_core::provider::Handler`), `caldir-core/src/provider/handler.rs:26-49`:

```rust
/// Implemented by each provider to serve the caldir RPC protocol.
#[async_trait]
pub trait Handler: Send + Sync {
    async fn connect(&self, cmd: Connect) -> Result<ConnectResponse>;

    async fn list_calendars(&self, _cmd: ListCalendars) -> Result<Vec<CalendarConfig>> {
        Err("list_calendars is not supported by this provider".into())
    }

    async fn list_events(&self, _cmd: ListEvents) -> Result<Vec<Event>> {
        Err("list_events is not supported by this provider".into())
    }

    async fn create_event(&self, _cmd: CreateEvent) -> Result<Event> {
        Err("This provider does not support creating events".into())
    }

    async fn update_event(&self, _cmd: UpdateEvent) -> Result<Event> {
        Err("This provider does not support updating events".into())
    }

    async fn delete_event(&self, _cmd: DeleteEvent) -> Result<()> {
        Err("This provider does not support deleting events".into())
    }
}
```

`Error = Box<dyn StdError + Send + Sync>`, `Result<T> = std::result::Result<T, Error>` (`handler.rs:20-23`). Only `connect` is required; every other method defaults to a runtime error string. Entry point `run_provider` reads newline-delimited JSON from stdin and writes one JSON line per request (`handler.rs:53-70`); `process_request` is the per-line unit (`handler.rs:74-84`). Error source chains are flattened into the `error` string (`handler.rs:87-96`).

Transport: one subprocess spawn *per request*, stdin closed after write, whole stdout read as the response (`caldir-core/src/provider/transport/subprocess_transport.rs:26-77`). Trait: `caldir-core/src/provider/transport.rs:14-20` — "transports take JSON strings in and return JSON strings out". Discovery is by filename prefix `caldir-provider-` on `PATH` (`caldir-core/src/provider/registry.rs:22-25,69-84`).

## 3. Wire format for events: ICS strings, not structured JSON

Events cross the wire as a **single opaque ICS string** (a full `VCALENDAR` document containing exactly one component). There is no structured-JSON event representation on the wire.

```rust
// caldir-core/src/rpc/ics.rs:7-35
pub struct Ics(pub Event);

pub(crate) fn serialize<S: Serializer>(event: &Event, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&event.to_ics_string())
}

pub(crate) fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Event, D::Error> {
    let ics = String::deserialize(deserializer)?;
    Event::from_single_ics_str(&ics).map_err(serde::de::Error::custom)
}
```

Requests opt in per field via `#[serde(with = "crate::rpc::ics")]`; responses go through the `WireValue` indirection (`caldir-core/src/rpc/wire.rs:6-56`):

```rust
pub(crate) trait WireValue: Sized {
    type Wire: Serialize + DeserializeOwned;
    fn into_wire(self) -> Self::Wire;
    fn from_wire(wire: Self::Wire) -> Self;
}

impl WireValue for Event { type Wire = Ics; … }
impl<T: WireValue> WireValue for Vec<T> { type Wire = Vec<T::Wire>; … }

/// Response types that pass through as plain JSON; ICS-encoded types impl [`WireValue`] directly.
pub(crate) trait JsonWireValue: Serialize + DeserializeOwned {}
impl JsonWireValue for () {}
impl JsonWireValue for CalendarConfig {}
impl JsonWireValue for ConnectResponse {}
```

Encoding is confined to the dispatch boundary: `handler.rs:111-120` calls `response.into_wire()`, `provider.rs:81-86` calls `WireValue::from_wire`. Providers therefore hand/receive a typed `Event`, never a string.

The produced document is always `BEGIN:VCALENDAR` / `VERSION:2.0` / `PRODID:CALDIR` wrapping one `icalendar::Event` (`caldir-core/src/event.rs:34-35,123-133`), plus VALARM blocks spliced textually into the VEVENT (`event.rs:133`, `event.rs:249`).

`CalendarConfig` is the only structured type crossing the wire (`caldir-core/src/calendar/config.rs:9-17`):

```rust
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CalendarConfig {
    name: Option<String>,
    color: Option<String>,
    read_only: Option<bool>,
    #[serde(rename = "remote")]
    remote_config: Option<RemoteConfig>,
}
```

## 4. Capability / feature negotiation

**There is no negotiation mechanism.** Nothing in `rpc/**` or `provider/**` asks a provider what it supports. Capability is expressed in exactly two implicit ways:

1. **Per-call failure.** An unimplemented method returns `{"status":"error","error":"This provider does not support …"}` from the trait's default body (`caldir-core/src/provider/handler.rs:30-48`). Core discovers unsupported operations only by attempting them.
2. **`read_only` on the returned `CalendarConfig`.** A per-calendar boolean, set by the provider in `list_calendars`/`connect` (`caldir-core/src/calendar/config.rs:13`, accessor `:76-78`), consumed by `Connection::read_only()` (`caldir-core/src/connection.rs:30-34`) which discards all outgoing changes (`connection.rs:57-59`). This gates write direction only; it says nothing about which component kinds or fields a provider handles.

`ConnectStepKind` (`caldir-core/src/rpc/methods/connect/response.rs:6-15`) enumerates auth-flow shapes (`o_auth_redirect`, `hosted_o_auth`, `credentials`, `needs_setup`) — an auth capability signal, not a data-model one.

## 5. Versioning and forward/backward compatibility

**The protocol carries no version number.** No field, header, handshake, or env var names a protocol version anywhere in `rpc/**` or `provider/**` (grep for `protocol_version` / `deny_unknown_fields`: zero hits repo-wide). The only negotiated environment value is `CALDIR_PROVIDER_STORAGE_DIR` (`caldir-core/src/provider/storage.rs:24`).

Compatibility is governed by these serde attributes:

- `#[serde(deny_unknown_fields)]` appears **nowhere** in the repo. Every request/response struct therefore silently ignores unknown JSON fields → new optional fields are additive-safe in both directions.
- `Request` tolerates a missing params object: `#[serde(rename = "command")] pub method: Method` + `#[serde(default)] pub params: serde_json::Value` (`caldir-core/src/rpc.rs:35-41`).
- `Response<T>` is internally tagged: `#[serde(tag = "status", rename_all = "snake_case")]` with variants `Success { data }` / `Error { error }` (`caldir-core/src/rpc.rs:52-57`).
- `ConnectResponse` is internally tagged on the same key `status` with `#[serde(flatten)] data: serde_json::Value` for step payloads, and both `Done` fields are `#[serde(default, skip_serializing_if = "Option::is_none")]` (`connect/response.rs:17-41`).
- `Connect` request: both `options` and `data` are `#[serde(default)]` free-form `serde_json::Map` (`connect.rs:13-18`).
- `CredentialField.required` is `#[serde(default)]`, `help` is `skip_serializing_if = "Option::is_none"` (`connect/request.rs:9-18`).
- `RemoteConfigParams` is `#[serde(flatten)]`ed into event/list requests as an open map (`create_event.rs:7-8`, `params.rs:5-6`), so new remote keys pass through untouched.

Hard-failure points for anything genuinely new:

- `Method` is a closed serde enum with no `#[serde(other)]` fallback (`methods.rs:22-31`). A **new method name sent to an old provider binary** fails at `serde_json::from_str::<Request>` and returns `"Failed to parse request: …"` (`handler.rs:75-78`). Old core receiving a new method name likewise cannot parse it.
- `CalendarConfig` field additions are safe (unknown fields ignored, all fields `Option`), so a new capability *flag* is addable there without breaking old binaries: old providers omit it, old core ignores it.
- A **new component type** cannot be expressed at all today: the ICS codec's decode path is `Event::from_single_ics_str`, which hard-requires exactly one parsed `Event` (`caldir-core/src/event.rs:111-121`, `EventError::UnexpectedEventCount { expected: 1, found }`), and `Event::from_ics_str` iterates `icalendar.events()` (`event.rs:96-102`) — non-VEVENT components are dropped before the count check, so a VTODO-only document decodes as `found: 0` and errors.
- `WireValue`/`JsonWireValue` are `pub(crate)` (`rpc.rs:14-17`), so out-of-tree providers cannot add wire types; they can only use the exported `Handler` surface (`provider.rs:19`).

## 6. VEVENT assumptions in the provider contract

1. **Every event-bearing RPC is typed `Event`**, a VEVENT-only Rust type — `CreateEvent.event`, `UpdateEvent.event`, `DeleteEvent.event`, `ListEvents::Response = Vec<Event>` (`rpc/methods/create_event.rs:10-15`, `update_event.rs:10-15`, `delete_event.rs:10-15`, `list_events.rs:15`). There is no component-kind discriminator on the wire.
2. **Method names are event-shaped and closed**: `list_events`, `create_event`, `update_event`, `delete_event` in `enum Method` (`rpc/methods.rs:24-30`); dispatch matches all six exhaustively (`provider/handler.rs:101-108`).
3. **Encode path emits a VEVENT**: `to_ics_string` builds `let ical_event: icalendar::Event = self.into();` and pushes it into an empty calendar (`caldir-core/src/event.rs:123-133`; `impl From<&Event> for icalendar::Event` at `event/to_icalendar.rs:4`).
4. **Decode path reads only VEVENTs**: `icalendar.events().map(Event::try_from)` — every non-VEVENT component in the document is silently discarded (`caldir-core/src/event.rs:96-102`).
5. **Exactly one component per wire payload**: `from_single_ics_str` errors with `UnexpectedEventCount { expected: 1, found }` for zero or many (`caldir-core/src/event.rs:111-121`), and this is the function `rpc::ics::deserialize` calls (`rpc/ics.rs:32-35`).
6. **VALARM handling is VEVENT-textual**: reminders are spliced by string-replacing `"END:VEVENT\r\n"` (`caldir-core/src/event.rs:249`, called from `to_ics_string` at `:133`).
7. **`Ics(pub Event)` is the sole wire wrapper** and `impl WireValue for Event { type Wire = Ics }` is the only ICS-carrying impl (`rpc/ics.rs:7-8`, `rpc/wire.rs:13-23`) — no parallel wrapper for another component kind exists.
8. **Sync/identity plumbing downstream of RPC is `Event`-typed**: `RemoteEvent(Event)` with `modified_at()` reading `Event::last_modified` (`caldir-core/src/remote/event.rs:5-19`), and `Remote::list_events` maps provider output straight into `RemoteEvent` (`caldir-core/src/remote.rs:24-38`).
9. **`EventChange` is the only change vocabulary** reaching a provider — `Create`/`Update`/`Delete` of an `Event`, with X-property merging on update (`caldir-core/src/remote.rs:40-58`).
10. **`Handler`'s default-method error strings name events**, e.g. `"This provider does not support creating events"` (`provider/handler.rs:34-48`) — a provider signals absence of event support, not of a component kind.
11. **Capability signalling has no component axis**: the only per-calendar flag is `read_only: Option<bool>` (`caldir-core/src/calendar/config.rs:13`), consumed as a write-direction gate (`caldir-core/src/connection.rs:30-34,57-59`).
12. **Window params are event-time-shaped**: `ListEvents { from, to }` RFC-3339 strings (`rpc/methods/list_events.rs:9-10`) fed from `DateRange::to_rfc3339` (`caldir-core/src/remote.rs:25-32`); overlap is computed from `Event::start`/`end` (`caldir-core/src/event.rs:136-141`).
