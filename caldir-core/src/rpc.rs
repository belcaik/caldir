mod ics;
mod methods;
mod wire;

use serde::{Deserialize, Serialize};
use std::time::Duration;

pub use methods::{
    Capabilities, Connect, ConnectResponse, ConnectStepKind, CreateItem, CredentialField,
    CredentialsData, DeleteItem, FieldType, HostedOAuthData, ListCalendars, ListEvents, Method,
    OAuthData, SetupData, UpdateItem,
};

#[cfg(test)]
pub(crate) use wire::JsonWireValue;
pub(crate) use wire::{Wire, WireValue};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15);

// Handles serialization of command + deserialization of response
pub(crate) trait Rpc: Serialize {
    type Response: WireValue;
    const METHOD: Method;
    const TIMEOUT: Duration = DEFAULT_TIMEOUT;

    fn to_json(&self) -> Result<serde_json::Value, serde_json::Error>
    where
        Self: Sized,
    {
        serde_json::to_value(Request::from_rpc(self)?)
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Request {
    #[serde(rename = "command")]
    pub method: Method,
    #[serde(default)]
    pub params: serde_json::Value,
}

impl Request {
    pub(crate) fn from_rpc<C: Rpc>(cmd: &C) -> Result<Self, serde_json::Error> {
        Ok(Self {
            method: C::METHOD,
            params: serde_json::to_value(cmd)?,
        })
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Response<T> {
    Success {
        data: T,
    },
    Error {
        error: String,
        /// Additive (R55/C9): absent on every envelope written before this
        /// field existed, and on every error an older provider still emits.
        #[serde(default)]
        code: Option<ErrorCode>,
    },
}

/// A machine-matchable discriminator riding beside `Response::Error::error`.
/// `format_chain` flattens the source chain into `error` itself, so anything
/// a caller needs to match on travels here instead of in the message text.
///
/// `#[non_exhaustive]`: a seventh class later must not break a downstream
/// match (C-5). `#[serde(other)]` on `Unrecognized` is the wire-level twin of
/// that guarantee — a *newer* provider sending a class string this binary has
/// never heard of must still decode, not fail parsing (§17/§18, C-4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ErrorCode {
    /// The `command` on the envelope is not a verb this provider binary
    /// understands — an old binary facing a newer core, or a typo.
    UnsupportedMethod,

    /// §16: this provider does not support the write it was asked to do
    /// (e.g. task writes on an events-only provider).
    UnsupportedOperation,

    /// §16: the provider's credentials lack the scope the request needs
    /// (`GOOGLE-13`'s core half).
    MissingAuthScope,

    /// §16: the remote object the request named does not exist.
    RemoteNotFound,

    /// §16: a conflict or precondition failure (e.g. a stale ETag).
    Conflict,

    /// §16: the provider cannot represent what was asked of it, distinct
    /// from an outright unsupported operation (e.g. a field the remote API
    /// silently truncates).
    SemanticLimitation,

    /// §16: a network or API failure talking to the remote. Distinct from
    /// `ProviderTransportError`, which models failures in the local
    /// subprocess exchange, not the provider's own upstream call.
    Network,

    /// A class string this binary does not recognise — a provider newer
    /// than this core. Never produced locally; only ever the result of
    /// decoding a code this enum's variant list doesn't (yet) contain.
    #[serde(other)]
    Unrecognized,
}

impl<T: Serialize> Response<T> {
    /// Serialize a success response to a JSON string for stdout.
    pub fn success(data: T) -> String {
        serde_json::to_string(&Response::Success { data })
            .expect("Response::Success serialization is infallible for Serialize types")
    }
}

impl Response<()> {
    /// Serialize an error response to a JSON string for stdout.
    pub fn error(msg: &str, code: Option<ErrorCode>) -> String {
        serde_json::to_string(&Response::<()>::Error {
            error: msg.to_string(),
            code,
        })
        .expect("Response::Error serialization is infallible")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CalendarItem;
    use crate::test_utils::{VEVENT_CONTROL_ICS, VTODO_ICS};
    use serde_json::json;

    /// R55/C9 (INVIOLABLE). An older provider's error envelope carries no
    /// structured field at all; a new optional field must not make it
    /// unparseable, or every installed provider binary breaks the day core
    /// adds one.
    #[test]
    fn an_error_envelope_from_an_older_provider_still_decodes() {
        let decoded: Response<()> =
            serde_json::from_str(r#"{"status":"error","error":"boom"}"#).unwrap();

        assert!(matches!(decoded, Response::Error { error, .. } if error == "boom"));
    }

    /// P2-04/C-4: a *newer* provider may send a class this core has never
    /// heard of. `#[serde(other)]` on `ErrorCode::Unrecognized` must catch it
    /// so the envelope still decodes, mirroring `#[non_exhaustive]`'s
    /// source-level guarantee at the wire level.
    #[test]
    fn an_error_code_this_binary_does_not_recognise_still_decodes() {
        let decoded: Response<()> = serde_json::from_str(
            r#"{"status":"error","error":"boom","code":"a_class_from_the_future"}"#,
        )
        .expect("an unrecognised code must not fail the decode");

        assert!(matches!(
            decoded,
            Response::Error {
                error,
                code: Some(ErrorCode::Unrecognized)
            } if error == "boom"
        ));
    }

    /// `T-33` — R52/C8 (INVIOLABLE). `Method` is a closed serde enum with no
    /// `#[serde(other)]`, so a seventh verb makes every *installed* provider
    /// binary fail at request-parse mid-upgrade. There is no deprecation path.
    ///
    /// A tripwire, not a smoke test: it must fail the day anyone adds
    /// `create_todo`. Do not relax it to a `>=` check — the exhaustive `match`
    /// is what makes a new variant impossible to add without reading this.
    ///
    /// `capabilities` (`P1-02`/D-2) is the one deliberate exception to R52's
    /// "no new `Method` variant": `Method` is not `#[non_exhaustive]`
    /// (C-5), so this variant breaks any out-of-tree matcher, Proton
    /// included, the moment it ships. That is accepted, not an oversight —
    /// `P1-01`'s graceful-skew path (`ErrorCode::UnsupportedMethod`) is the
    /// migration story for every provider binary built before this line.
    #[test]
    fn method_set_is_exactly_the_seven_known_verbs() {
        let all = [
            Method::Connect,
            Method::ListCalendars,
            Method::ListEvents,
            Method::CreateEvent,
            Method::UpdateEvent,
            Method::DeleteEvent,
            Method::Capabilities,
        ];

        // Exhaustive on purpose: a new variant breaks compilation here first.
        for method in all {
            match method {
                Method::Connect
                | Method::ListCalendars
                | Method::ListEvents
                | Method::CreateEvent
                | Method::UpdateEvent
                | Method::DeleteEvent
                | Method::Capabilities => {}
            }
        }

        let names: Vec<String> = all
            .iter()
            .map(|method| {
                serde_json::to_value(method)
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect();

        assert_eq!(
            names,
            vec![
                "connect",
                "list_calendars",
                "list_events",
                "create_event",
                "update_event",
                "delete_event",
                "capabilities",
            ],
        );
    }

    /// `T-34` — R55/C9 (INVIOLABLE). A newer provider may send fields this
    /// core has never heard of; every request struct must still deserialize.
    #[test]
    fn request_structs_ignore_unknown_fields() {
        let extra = json!("a field from a newer caldir");

        let mut connect = json!({"options": {}, "data": {}});
        connect["caldir_from_the_future"] = extra.clone();
        serde_json::from_value::<Connect>(connect).expect("Connect");

        let mut list_calendars = json!({"account_identifier": "user@hmail.com"});
        list_calendars["caldir_from_the_future"] = extra.clone();
        serde_json::from_value::<ListCalendars>(list_calendars).expect("ListCalendars");

        let mut list_events =
            json!({"from": "2026-01-01T00:00:00+00:00", "to": "2026-12-31T23:59:59+00:00"});
        list_events["caldir_from_the_future"] = extra.clone();
        serde_json::from_value::<ListEvents>(list_events).expect("ListEvents");

        for name in ["CreateItem", "UpdateItem", "DeleteItem"] {
            let mut payload = json!({"event": VEVENT_CONTROL_ICS});
            payload["caldir_from_the_future"] = extra.clone();

            match name {
                "CreateItem" => {
                    serde_json::from_value::<CreateItem>(payload).expect(name);
                }
                "UpdateItem" => {
                    serde_json::from_value::<UpdateItem>(payload).expect(name);
                }
                _ => {
                    serde_json::from_value::<DeleteItem>(payload).expect(name);
                }
            }
        }
    }

    /// `T-34` — R55/C9 (INVIOLABLE), the source-level half. One "tightening"
    /// of one struct silently breaks forward compatibility with every newer
    /// provider, and `flatten` interacts badly with it besides.
    ///
    /// Two tricks let the scan cover `rpc.rs` — where `Request`/`Response`
    /// live — as well as `rpc/`, without the test tripping over its own
    /// source: the needle is assembled by `concat!`, and matches are
    /// word-bounded, so this test's *name* is not a hit while a real
    /// `#[serde(...)]` attribute is.
    #[test]
    fn deny_unknown_fields_appears_nowhere() {
        let needle = concat!("deny_unknown", "_fields");
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");

        let mut offenders = Vec::new();
        scan_rust_sources(&root.join("rpc.rs"), needle, &mut offenders);
        scan_rust_sources(&root.join("rpc"), needle, &mut offenders);

        assert!(
            offenders.is_empty(),
            "{needle} must appear nowhere under the RPC modules, found in {offenders:?}"
        );
    }

    fn scan_rust_sources(path: &std::path::Path, needle: &str, offenders: &mut Vec<String>) {
        if path.is_dir() {
            for entry in std::fs::read_dir(path).unwrap() {
                scan_rust_sources(&entry.unwrap().path(), needle, offenders);
            }
            return;
        }

        if !path.extension().is_some_and(|ext| ext == "rs") {
            return;
        }

        let source = std::fs::read_to_string(path).unwrap();

        if contains_word(&source, needle) {
            offenders.push(path.display().to_string());
        }
    }

    /// `needle` as a whole word. This test's own name is an identifier that
    /// merely *starts* with the needle, so it is not a hit; the needle inside
    /// a `#[serde(...)]` attribute — followed by `)` or `,` — is.
    fn contains_word(haystack: &str, needle: &str) -> bool {
        let is_ident = |c: char| c.is_alphanumeric() || c == '_';

        haystack.match_indices(needle).any(|(at, _)| {
            let before = haystack[..at].chars().next_back();
            let after = haystack[at + needle.len()..].chars().next();

            !before.is_some_and(is_ident) && !after.is_some_and(is_ident)
        })
    }

    /// `T-36`, inverted by this ticket: `CreateItem`/`UpdateItem`/`DeleteItem`
    /// now **carry** a task payload — R53's second sentence, which the
    /// event-only wire refused. All three structs must move together, or a
    /// half-migration would leave one arm of `P2-01`'s `apply_change` match
    /// unable to compile while two silently could.
    #[test]
    fn a_task_payload_decodes_into_every_outbound_request_struct() {
        let payload = json!({ "event": VTODO_ICS });
        let expected = CalendarItem::from_single_ics_str(VTODO_ICS).unwrap();

        let create = serde_json::from_value::<CreateItem>(payload.clone())
            .expect("a VTODO must decode into CreateItem");
        let update = serde_json::from_value::<UpdateItem>(payload.clone())
            .expect("a VTODO must decode into UpdateItem");
        let delete = serde_json::from_value::<DeleteItem>(payload)
            .expect("a VTODO must decode into DeleteItem");

        assert_eq!(create.event, expected);
        assert_eq!(update.event, expected);
        assert_eq!(delete.event, expected);
    }
}
