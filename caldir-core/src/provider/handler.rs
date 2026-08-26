//! Scaffolding for caldir providers
//!
//! Providers implement [`Handler`].
//! It takes care of reading JSON payloads from stdin
//! and writing JSON responses to stdout.

use async_trait::async_trait;
use serde::de::DeserializeOwned;
use std::error::Error as StdError;
use std::future::Future;
use std::io::{self, BufRead, Write};

use crate::provider::ProviderCapabilities;
use crate::rpc::{
    Capabilities, Connect, ConnectResponse, CreateItem, DeleteItem, ErrorCode, ListCalendars,
    ListEvents, Method, Request, Response, Rpc, UpdateItem, WireValue,
};
use crate::{CalendarConfig, CalendarItem};

use super::slug::provider_slug_from_filename;

/// Error returned by [`Handler`] methods.
pub type Error = Box<dyn StdError + Send + Sync>;

/// Result alias for [`Handler`] methods.
pub type Result<T> = std::result::Result<T, Error>;

/// Tags a [`Handler`]'s own failure with a §16 `ErrorCode` so
/// `process_request` can put it on the wire without inspecting the message
/// text. The `create_item`/`update_item`/`delete_item` defaults
/// below and the public [`classified_error`] both go through this.
#[derive(Debug)]
struct Classified {
    code: ErrorCode,
    message: String,
}

impl Classified {
    fn boxed(code: ErrorCode, message: impl Into<String>) -> Error {
        Box::new(Classified {
            code,
            message: message.into(),
        })
    }
}

/// Lets a `Handler` attach a §16 `ErrorCode` to an error it returns, so it
/// arrives at the consumer as the matching `ProviderError` variant instead of
/// the generic one. The `Handler` trait's `Result<T, Error>`
/// signatures are unchanged — this only widens what a provider can put
/// inside the `Err`. Additive: an error that never passes through here still
/// degrades to `ProviderError::Provider`, exactly as before.
pub fn classified_error(code: ErrorCode, msg: impl Into<String>) -> Error {
    Classified::boxed(code, msg)
}

impl std::fmt::Display for Classified {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl StdError for Classified {}

/// Implemented by each provider to serve the caldir RPC protocol.
#[async_trait]
pub trait Handler: Send + Sync {
    async fn connect(&self, cmd: Connect) -> Result<ConnectResponse>;

    async fn list_calendars(&self, _cmd: ListCalendars) -> Result<Vec<CalendarConfig>> {
        Err("list_calendars is not supported by this provider".into())
    }

    /// The one inbound channel for a task: item-typed so a provider can
    /// return a `VTODO` at all. The outbound methods below stay `Event`-typed —
    /// the push wire carries events only, so a task can never ride it.
    async fn list_events(&self, _cmd: ListEvents) -> Result<Vec<CalendarItem>> {
        Err("list_events is not supported by this provider".into())
    }

    /// D-3: item-typed so a provider's `match` can serve a `Todo` arm
    /// once it has one; the wire verb stays `create_event`.
    async fn create_item(&self, _cmd: CreateItem) -> Result<CalendarItem> {
        Err(Classified::boxed(
            ErrorCode::UnsupportedOperation,
            "This provider does not support creating events",
        ))
    }

    async fn update_item(&self, _cmd: UpdateItem) -> Result<CalendarItem> {
        Err(Classified::boxed(
            ErrorCode::UnsupportedOperation,
            "This provider does not support updating events",
        ))
    }

    async fn delete_item(&self, _cmd: DeleteItem) -> Result<()> {
        Err(Classified::boxed(
            ErrorCode::UnsupportedOperation,
            "This provider does not support deleting events",
        ))
    }

    /// D-2/§18: the default every unmodified provider gets for free —
    /// events read+write true, tasks read+write false — regardless of the
    /// flattened remote params, so an unmodified provider's answer never
    /// depends on config it has never read. A provider opts into a
    /// per-remote answer (or task support) by overriding this.
    async fn capabilities(&self, _cmd: Capabilities) -> Result<ProviderCapabilities> {
        Ok(ProviderCapabilities::events_only())
    }
}

/// Run a provider as a subprocess speaking the caldir RPC protocol over
/// stdin/stdout. Blocks until stdin closes.
pub async fn run_provider<H: Handler>(handler: H) {
    let input = io::stdin().lock();
    let mut output = io::stdout();

    for line in input.lines() {
        let Ok(line) = line else { break };

        if line.trim().is_empty() {
            continue;
        }

        let response = process_request(&handler, &line).await;

        if writeln!(output, "{}", response).is_err() || output.flush().is_err() {
            break;
        }
    }
}

/// The subset of an envelope that decodes even when `command` is a verb this
/// binary has never heard of. `Method`'s `Deserialize` is intentionally
/// strict (R52: no `#[serde(other)]`, no `#[non_exhaustive]`), so the
/// tolerance for an unrecognised verb lives here, one step before `Method`
/// ever sees the string.
#[derive(serde::Deserialize)]
struct RawRequest {
    command: String,
    #[serde(default)]
    params: serde_json::Value,
}

/// Process a single JSON-encoded request line and return the JSON-encoded
/// response. Exposed for unit tests — most providers only need [`run_provider`].
pub async fn process_request<H: Handler>(handler: &H, line: &str) -> String {
    let raw: RawRequest = match serde_json::from_str(line) {
        Ok(r) => r,
        Err(e) => return Response::error(&format!("Failed to parse request: {e}"), None),
    };

    let method: Method =
        match serde_json::from_value(serde_json::Value::String(raw.command.clone())) {
            Ok(method) => method,
            Err(_) => {
                return Response::error(
                    &format!(
                        "{} does not support the \"{}\" command; the provider may need updating",
                        provider_name(),
                        raw.command,
                    ),
                    Some(ErrorCode::UnsupportedMethod),
                );
            }
        };

    let request = Request {
        method,
        params: raw.params,
    };

    match dispatch(handler, request).await {
        Ok(data) => Response::success(data),
        Err(e) => {
            let code = e.downcast_ref::<Classified>().map(|c| c.code);
            Response::error(
                &format!("Error handling request: {}", format_chain(&*e)),
                code,
            )
        }
    }
}

/// The provider's own name, recovered from its binary filename — no new
/// argument needed (`provider_slug_from_filename`, `slug.rs`). Falls back to
/// a generic label when the filename can't be read (e.g. in unit tests).
fn provider_name() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.file_name().map(|name| name.to_os_string()))
        .and_then(|name| name.to_str().map(str::to_string))
        .and_then(|name| provider_slug_from_filename(&name))
        .map(|slug| slug.to_string())
        .unwrap_or_else(|| "this provider".to_string())
}

/// Preserves context from providers' `anyhow::Context`
fn format_chain(err: &(dyn StdError + 'static)) -> String {
    let mut out = err.to_string();
    let mut source = err.source();
    while let Some(e) = source {
        out.push_str(": ");
        out.push_str(&e.to_string());
        source = e.source();
    }
    out
}

async fn dispatch<H: Handler>(handler: &H, request: Request) -> Result<serde_json::Value> {
    let Request { method, params } = request;

    match method {
        Method::Connect => call(params, |c| handler.connect(c)).await,
        Method::ListCalendars => call(params, |c| handler.list_calendars(c)).await,
        Method::ListEvents => call(params, |c| handler.list_events(c)).await,
        Method::CreateEvent => call(params, |c| handler.create_item(c)).await,
        Method::UpdateEvent => call(params, |c| handler.update_item(c)).await,
        Method::DeleteEvent => call(params, |c| handler.delete_item(c)).await,
        Method::Capabilities => call(params, |c| handler.capabilities(c)).await,
    }
}

async fn call<C, F, Fut>(params: serde_json::Value, handler: F) -> Result<serde_json::Value>
where
    C: DeserializeOwned + Rpc,
    F: FnOnce(C) -> Fut,
    Fut: Future<Output = Result<C::Response>>,
{
    let cmd: C = serde_json::from_value(params)?;
    let response = handler(cmd).await?;
    Ok(serde_json::to_value(response.into_wire())?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Event, EventTime, RemoteConfigParams};
    use chrono::NaiveDate;

    struct StubHandler;

    #[async_trait]
    impl Handler for StubHandler {
        async fn connect(&self, _cmd: Connect) -> Result<ConnectResponse> {
            Ok(ConnectResponse::Done {
                account_identifier: Some("me@example.com".to_string()),
                calendars: None,
            })
        }
    }

    struct EventHandler;

    #[async_trait]
    impl Handler for EventHandler {
        async fn connect(&self, _cmd: Connect) -> Result<ConnectResponse> {
            unreachable!()
        }

        async fn create_item(&self, cmd: CreateItem) -> Result<CalendarItem> {
            Ok(cmd.event)
        }
    }

    #[tokio::test]
    async fn dispatches_connect_and_returns_success_envelope() {
        let response = process_request(
            &StubHandler,
            r#"{"command":"connect","params":{"options":{},"data":{}}}"#,
        )
        .await;

        let parsed: serde_json::Value = serde_json::from_str(&response).unwrap();
        assert_eq!(parsed["status"], "success");
        assert_eq!(parsed["data"]["account_identifier"], "me@example.com");
    }

    #[tokio::test]
    async fn event_codec_is_confined_to_dispatch_boundary() {
        let event = Event::new(
            "Test",
            EventTime::Date(NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()),
        );
        let request = CreateItem {
            remote: RemoteConfigParams::new(),
            event: CalendarItem::Event(event.clone()),
        }
        .to_json()
        .unwrap();

        let response = process_request(&EventHandler, &request.to_string()).await;
        let parsed: serde_json::Value = serde_json::from_str(&response).unwrap();
        let wire_event =
            CalendarItem::from_single_ics_str(parsed["data"].as_str().unwrap()).unwrap();

        assert_eq!(wire_event, CalendarItem::Event(event));
    }

    /// W2-C3, the whole ticket's acceptance test: a `Handler` attaches a §16
    /// class with the public `classified_error` constructor, and the bytes
    /// `process_request` puts on the wire for it are fed through the real
    /// consumer-side decode path (`Provider::call`) — not inspected as JSON
    /// here. The class must arrive as the matching `ProviderError` variant,
    /// not fall back to the generic one `classified_error` didn't exist to
    /// prevent.
    #[tokio::test]
    async fn a_handler_attached_error_class_survives_the_wire_round_trip() {
        struct RefusingHandler;

        #[async_trait]
        impl Handler for RefusingHandler {
            async fn connect(&self, _cmd: Connect) -> Result<ConnectResponse> {
                unreachable!()
            }

            async fn create_item(&self, _cmd: CreateItem) -> Result<CalendarItem> {
                Err(classified_error(
                    ErrorCode::MissingAuthScope,
                    "reauth required",
                ))
            }
        }

        fn create_item_request() -> CreateItem {
            CreateItem {
                remote: RemoteConfigParams::new(),
                event: CalendarItem::Event(Event::new(
                    "Standup",
                    EventTime::Date(NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()),
                )),
            }
        }

        let request = create_item_request().to_json().unwrap();
        let wire_response = process_request(&RefusingHandler, &request.to_string()).await;

        let mock = std::sync::Arc::new(
            crate::provider::transport::mock_transport::MockTransport::with_response(wire_response),
        );
        let provider = crate::provider::Provider::with_transport(
            crate::provider::ProviderSlug::from("test"),
            mock as std::sync::Arc<dyn crate::provider::transport::ProviderTransport>,
        );

        let err = provider.call(create_item_request()).await.unwrap_err();

        assert!(
            matches!(
                &err,
                crate::provider::ProviderError::MissingAuthScope(msg)
                    if msg.contains("reauth required")
            ),
            "got {err:?}"
        );
    }

    struct ListingHandler(Vec<CalendarItem>);

    #[async_trait]
    impl Handler for ListingHandler {
        async fn connect(&self, _cmd: Connect) -> Result<ConnectResponse> {
            unreachable!()
        }

        async fn list_events(&self, _cmd: ListEvents) -> Result<Vec<CalendarItem>> {
            Ok(self.0.clone())
        }
    }

    const LIST_EVENTS_REQUEST: &str = r#"{"command":"list_events","params":{"from":"2026-01-01T00:00:00+00:00","to":"2026-12-31T23:59:59+00:00"}}"#;

    /// R1/R2 (INVIOLABLE) — the migration guarantee. An item-typed response
    /// must leave the bytes an **event** listing puts on the wire exactly as
    /// they were, or every installed provider's pull changes shape. Asserted
    /// against `Event::to_ics_string` — the bytes' own source of truth — not
    /// against the wire codec, which is the thing under test.
    #[tokio::test]
    async fn an_event_listing_is_byte_identical_to_the_event_only_encoding() {
        let event = Event::new(
            "Test",
            EventTime::Date(NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()),
        );
        let handler = ListingHandler(vec![CalendarItem::Event(event.clone())]);

        let response = process_request(&handler, LIST_EVENTS_REQUEST).await;
        let parsed: serde_json::Value = serde_json::from_str(&response).unwrap();

        assert_eq!(parsed["data"], serde_json::json!([event.to_ics_string()]));
    }

    /// R53 — the inbound half of the same seam: a provider can hand a task
    /// back, and it arrives as a bare ICS string like every other element.
    #[tokio::test]
    async fn a_task_listing_reaches_the_wire_as_a_vtodo_string() {
        let todo = CalendarItem::from_single_ics_str(crate::test_utils::VTODO_ICS).unwrap();
        let handler = ListingHandler(vec![todo]);

        let response = process_request(&handler, LIST_EVENTS_REQUEST).await;
        let parsed: serde_json::Value = serde_json::from_str(&response).unwrap();

        let ics = parsed["data"][0]
            .as_str()
            .expect("each element is a plain ICS string");

        assert!(ics.contains("BEGIN:VTODO\r\n"), "got {ics:?}");
        assert!(!ics.contains("BEGIN:VEVENT"), "got {ics:?}");
    }

    /// The compensating lock for a change touching 27 `cmd.event` sites
    /// across 10 files and 4 crates: both directions of a `create_event`
    /// exchange must put the exact bytes `Event::to_ics_string` puts on the
    /// wire — asserted against the bytes' own source of truth, not against
    /// the codec under test, so no installed provider ever sees the shape
    /// of an event change.
    #[tokio::test]
    async fn an_event_create_is_byte_identical_to_the_event_only_encoding() {
        let event = Event::new(
            "Test",
            EventTime::Date(NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()),
        );

        let request = CreateItem {
            remote: RemoteConfigParams::new(),
            event: CalendarItem::Event(event.clone()),
        }
        .to_json()
        .unwrap();

        assert_eq!(request["params"]["event"], event.to_ics_string());

        let response = process_request(&EventHandler, &request.to_string()).await;
        let parsed: serde_json::Value = serde_json::from_str(&response).unwrap();

        assert_eq!(parsed["data"], event.to_ics_string());
    }

    /// `CROSS-6`: the tempting implementation of an unsupported `Todo` arm in
    /// a VEVENT-bound provider is to map it onto an event. This asserts the
    /// refusal is the shipped behaviour instead — an error envelope, and no
    /// `VEVENT` anywhere in the exchange.
    #[tokio::test]
    async fn a_provider_that_cannot_write_a_task_errors_without_converting_it() {
        struct EventOnlyHandler;

        #[async_trait]
        impl Handler for EventOnlyHandler {
            async fn connect(&self, _cmd: Connect) -> Result<ConnectResponse> {
                unreachable!()
            }

            async fn create_item(&self, cmd: CreateItem) -> Result<CalendarItem> {
                match cmd.event {
                    CalendarItem::Event(event) => Ok(CalendarItem::Event(event)),
                    CalendarItem::Todo(_) => {
                        Err("this provider does not support creating tasks".into())
                    }
                }
            }
        }

        let todo = CalendarItem::from_single_ics_str(crate::test_utils::VTODO_ICS).unwrap();
        let request = CreateItem {
            remote: RemoteConfigParams::new(),
            event: todo,
        }
        .to_json()
        .unwrap();

        let response = process_request(&EventOnlyHandler, &request.to_string()).await;
        let parsed: serde_json::Value = serde_json::from_str(&response).unwrap();

        assert_eq!(parsed["status"], "error");
        assert!(
            !response.contains("VEVENT"),
            "a refused task must never be converted to an event, got {response}"
        );
    }

    #[tokio::test]
    async fn unimplemented_method_returns_error_envelope() {
        let response = process_request(
            &StubHandler,
            r#"{"command":"list_calendars","params":{"account_identifier":"me@example.com"}}"#,
        )
        .await;

        let parsed: serde_json::Value = serde_json::from_str(&response).unwrap();
        assert_eq!(parsed["status"], "error");
        assert!(
            parsed["error"]
                .as_str()
                .unwrap()
                .contains("list_calendars is not supported"),
            "got: {}",
            parsed["error"]
        );
    }

    #[tokio::test]
    async fn error_response_includes_source_chain() {
        #[derive(Debug)]
        struct Outer;
        impl std::fmt::Display for Outer {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "outer")
            }
        }
        impl StdError for Outer {
            fn source(&self) -> Option<&(dyn StdError + 'static)> {
                Some(&Inner)
            }
        }

        #[derive(Debug)]
        struct Inner;
        impl std::fmt::Display for Inner {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "inner")
            }
        }
        impl StdError for Inner {}

        struct ChainHandler;
        #[async_trait]
        impl Handler for ChainHandler {
            async fn connect(&self, _cmd: Connect) -> Result<ConnectResponse> {
                Err(Box::new(Outer))
            }
        }

        let response = process_request(
            &ChainHandler,
            r#"{"command":"connect","params":{"options":{},"data":{}}}"#,
        )
        .await;

        let parsed: serde_json::Value = serde_json::from_str(&response).unwrap();
        assert_eq!(parsed["status"], "error");
        assert_eq!(parsed["error"], "Error handling request: outer: inner");
    }

    #[tokio::test]
    async fn an_unknown_command_is_reported_as_a_version_error() {
        let response =
            process_request(&StubHandler, r#"{"command":"list_todos","params":{}}"#).await;

        let parsed: serde_json::Value = serde_json::from_str(&response).unwrap();
        assert_eq!(parsed["status"], "error");
        let error = parsed["error"].as_str().unwrap();
        assert!(error.contains("list_todos"), "got: {error}");
        assert!(!error.contains("Failed to parse request"), "got: {error}");
    }

    #[tokio::test]
    async fn a_typo_in_a_known_command_is_still_loud() {
        struct PanicsIfCreateEventRuns;

        #[async_trait]
        impl Handler for PanicsIfCreateEventRuns {
            async fn connect(&self, _cmd: Connect) -> Result<ConnectResponse> {
                unreachable!()
            }

            async fn create_item(&self, _cmd: CreateItem) -> Result<CalendarItem> {
                panic!("create_item must not run for a typo'd command");
            }
        }

        let response = process_request(
            &PanicsIfCreateEventRuns,
            r#"{"command":"create_evnet","params":{}}"#,
        )
        .await;

        let parsed: serde_json::Value = serde_json::from_str(&response).unwrap();
        assert_eq!(parsed["status"], "error");
        let error = parsed["error"].as_str().unwrap();
        assert!(error.contains("create_evnet"), "got: {error}");
    }

    /// P2-04's headline proof: the CLI must be able to tell "unsupported"
    /// apart from any other failure by reading a field, never by inspecting
    /// `parsed["error"]`'s prose. `StubHandler` never overrides `create_item`,
    /// so the trait default — tagged `ErrorCode::UnsupportedOperation` — is
    /// what answers here.
    #[tokio::test]
    async fn an_error_envelope_carries_a_machine_readable_class() {
        let event = Event::new(
            "Test",
            EventTime::Date(NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()),
        );
        let request = CreateItem {
            remote: RemoteConfigParams::new(),
            event: CalendarItem::Event(event),
        }
        .to_json()
        .unwrap();

        let response = process_request(&StubHandler, &request.to_string()).await;
        let parsed: serde_json::Value = serde_json::from_str(&response).unwrap();

        assert_eq!(parsed["status"], "error");
        assert_eq!(
            parsed["code"], "unsupported_operation",
            "the class must be readable without inspecting the message: {parsed}"
        );
    }

    /// §18/C-4: every provider binary shipped before this ticket emits an
    /// error envelope with no `code` field at all. It must decode into the
    /// unclassified/generic class with its message intact, never fail to
    /// parse — the regression `an_error_envelope_from_an_older_provider_still_decodes`
    /// (`rpc.rs`) pins at the wire level; this pins the same guarantee in the
    /// vocabulary `process_request`'s caller actually sees it in.
    #[test]
    fn an_untyped_error_from_an_old_provider_reads_as_the_generic_class() {
        let json = r#"{"status":"error","error":"boom"}"#;

        let decoded: Response<()> =
            serde_json::from_str(json).expect("an old provider's envelope must still decode");

        match decoded {
            Response::Error { error, code } => {
                assert_eq!(error, "boom");
                assert_eq!(code, None, "an absent code must read as the generic class");
            }
            Response::Success { .. } => panic!("expected an error envelope"),
        }
    }

    #[tokio::test]
    async fn malformed_json_returns_parse_error() {
        let response = process_request(&StubHandler, "not json").await;

        let parsed: serde_json::Value = serde_json::from_str(&response).unwrap();
        assert_eq!(parsed["status"], "error");
        assert!(
            parsed["error"]
                .as_str()
                .unwrap()
                .contains("Failed to parse request"),
        );
    }

    /// D-2's whole compatibility claim is that no provider crate needs
    /// editing. If the default leaned the other way, every legacy provider
    /// would be asked to push tasks it silently drops — and no in-tree test
    /// would notice, because every in-tree provider gets its own override
    /// eventually.
    #[tokio::test]
    async fn an_unmodified_provider_reports_events_only_capabilities() {
        let response =
            process_request(&StubHandler, r#"{"command":"capabilities","params":{}}"#).await;

        let parsed: serde_json::Value = serde_json::from_str(&response).unwrap();
        assert_eq!(parsed["status"], "success");
        assert_eq!(parsed["data"]["events"]["read"], true);
        assert_eq!(parsed["data"]["events"]["write"], true);
        assert_eq!(parsed["data"]["tasks"]["read"], false);
        assert_eq!(parsed["data"]["tasks"]["write"], false);
    }

    struct TaskWritingHandler;

    #[async_trait]
    impl Handler for TaskWritingHandler {
        async fn connect(&self, _cmd: Connect) -> Result<ConnectResponse> {
            unreachable!()
        }

        async fn capabilities(&self, _cmd: Capabilities) -> Result<ProviderCapabilities> {
            Ok(ProviderCapabilities {
                events: crate::provider::Capability {
                    read: true,
                    write: true,
                },
                tasks: crate::provider::Capability {
                    read: true,
                    write: true,
                },
            })
        }
    }

    /// The control for the default-events-only tests above: without it,
    /// hard-coding events-only in `dispatch` would still pass every other
    /// assertion in this ticket.
    #[tokio::test]
    async fn a_provider_may_advertise_task_writes() {
        let response = process_request(
            &TaskWritingHandler,
            r#"{"command":"capabilities","params":{}}"#,
        )
        .await;

        let parsed: serde_json::Value = serde_json::from_str(&response).unwrap();
        assert_eq!(parsed["data"]["tasks"]["write"], true);
    }

    struct PerRemoteHandler;

    #[async_trait]
    impl Handler for PerRemoteHandler {
        async fn connect(&self, _cmd: Connect) -> Result<ConnectResponse> {
            unreachable!()
        }

        async fn capabilities(&self, cmd: Capabilities) -> Result<ProviderCapabilities> {
            let is_task_list = cmd
                .remote
                .get("google_resource_kind")
                .and_then(|v| v.as_str())
                == Some("tasks");

            Ok(ProviderCapabilities {
                events: crate::provider::Capability {
                    read: true,
                    write: true,
                },
                tasks: crate::provider::Capability {
                    read: is_task_list,
                    write: is_task_list,
                },
            })
        }
    }

    /// This is the whole reason the request carries the remote at all. Every
    /// other test in this ticket passes just as well against a binary-wide
    /// constant — and a binary-wide constant can only ever return the
    /// conservative floor, which makes `P4-05`'s Google task-list
    /// declaration unreachable.
    #[tokio::test]
    async fn one_handler_answers_two_remotes_differently() {
        let calendar_response = process_request(
            &PerRemoteHandler,
            r#"{"command":"capabilities","params":{"google_resource_kind":"calendar"}}"#,
        )
        .await;
        let calendar: serde_json::Value = serde_json::from_str(&calendar_response).unwrap();

        let tasks_response = process_request(
            &PerRemoteHandler,
            r#"{"command":"capabilities","params":{"google_resource_kind":"tasks"}}"#,
        )
        .await;
        let tasks: serde_json::Value = serde_json::from_str(&tasks_response).unwrap();

        assert_eq!(calendar["data"]["tasks"]["write"], false);
        assert_eq!(tasks["data"]["tasks"]["write"], true);
        assert_ne!(calendar["data"]["tasks"], tasks["data"]["tasks"]);
    }

    /// The guardrail for the test above: per-remote variance must be
    /// something a provider *opts into*. A default that varied with params
    /// would make every unmodified provider's answer depend on config it
    /// never reads.
    #[tokio::test]
    async fn the_default_answers_events_only_whatever_the_remote_says() {
        let calendar_response = process_request(
            &StubHandler,
            r#"{"command":"capabilities","params":{"google_resource_kind":"calendar"}}"#,
        )
        .await;
        let calendar: serde_json::Value = serde_json::from_str(&calendar_response).unwrap();

        let tasks_response = process_request(
            &StubHandler,
            r#"{"command":"capabilities","params":{"google_resource_kind":"tasks"}}"#,
        )
        .await;
        let tasks: serde_json::Value = serde_json::from_str(&tasks_response).unwrap();

        assert_eq!(calendar["data"], tasks["data"]);
        assert_eq!(calendar["data"]["tasks"]["write"], false);
    }
}
