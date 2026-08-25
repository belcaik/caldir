mod ics;
mod methods;
mod wire;

use serde::{Deserialize, Serialize};
use std::time::Duration;

pub use methods::{
    Connect, ConnectResponse, ConnectStepKind, CreateEvent, CredentialField, CredentialsData,
    DeleteEvent, FieldType, HostedOAuthData, ListCalendars, ListEvents, Method, OAuthData,
    SetupData, UpdateEvent,
};

pub(crate) use ics::Ics;
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
    Success { data: T },
    Error { error: String },
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
    pub fn error(msg: &str) -> String {
        serde_json::to_string(&Response::<()>::Error {
            error: msg.to_string(),
        })
        .expect("Response::Error serialization is infallible")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{VEVENT_CONTROL_ICS, VTODO_ICS};
    use serde_json::json;

    /// `T-33` — R52/C8 (INVIOLABLE). `Method` is a closed serde enum with no
    /// `#[serde(other)]`, so a seventh verb makes every *installed* provider
    /// binary fail at request-parse mid-upgrade. There is no deprecation path.
    ///
    /// A tripwire, not a smoke test: it must fail the day anyone adds
    /// `create_todo`. Do not relax it to a `>=` check — the exhaustive `match`
    /// is what makes a new variant impossible to add without reading this.
    #[test]
    fn method_set_is_exactly_the_six_known_verbs() {
        let all = [
            Method::Connect,
            Method::ListCalendars,
            Method::ListEvents,
            Method::CreateEvent,
            Method::UpdateEvent,
            Method::DeleteEvent,
        ];

        // Exhaustive on purpose: a new variant breaks compilation here first.
        for method in all {
            match method {
                Method::Connect
                | Method::ListCalendars
                | Method::ListEvents
                | Method::CreateEvent
                | Method::UpdateEvent
                | Method::DeleteEvent => {}
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

        for name in ["CreateEvent", "UpdateEvent", "DeleteEvent"] {
            let mut payload = json!({"event": VEVENT_CONTROL_ICS});
            payload["caldir_from_the_future"] = extra.clone();

            match name {
                "CreateEvent" => {
                    serde_json::from_value::<CreateEvent>(payload).expect(name);
                }
                "UpdateEvent" => {
                    serde_json::from_value::<UpdateEvent>(payload).expect(name);
                }
                _ => {
                    serde_json::from_value::<DeleteEvent>(payload).expect(name);
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

    /// The honest counterpart to the ticket's `T-36`, which asked that
    /// `CreateEvent`/`UpdateEvent`/`DeleteEvent` **carry** a task payload.
    /// That criterion is refused: R70 keeps the outbound field `Event`, so a
    /// task cannot ride it by construction. What is testable — and what R71
    /// actually requires — is that the attempt aborts **loudly**, with the
    /// text `L6-01` builds its user-facing story on. A silent success here
    /// would push a task to a remote as if it were a meeting.
    #[test]
    fn a_task_payload_is_rejected_by_every_outbound_request_struct() {
        let payload = json!({ "event": VTODO_ICS });
        const EXPECTED: &str = "expected 1 event(s) in ICS, found 0";

        let create = serde_json::from_value::<CreateEvent>(payload.clone())
            .expect_err("a VTODO must not decode into CreateEvent");
        let update = serde_json::from_value::<UpdateEvent>(payload.clone())
            .expect_err("a VTODO must not decode into UpdateEvent");
        let delete = serde_json::from_value::<DeleteEvent>(payload)
            .expect_err("a VTODO must not decode into DeleteEvent");

        assert_eq!(create.to_string(), EXPECTED);
        assert_eq!(update.to_string(), EXPECTED);
        assert_eq!(delete.to_string(), EXPECTED);
    }
}
