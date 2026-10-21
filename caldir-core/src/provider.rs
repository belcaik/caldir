mod account;
mod capabilities;
mod error;
mod handler;
mod registry;
mod slug;
mod storage;
pub(crate) mod transport;

#[cfg(test)]
pub(crate) mod mock_provider;

use crate::rpc;
use account::ProviderAccount;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use transport::{ProviderTransport, SubprocessTransport};

pub use capabilities::{Capability, ProviderCapabilities};
pub use error::ProviderError;
pub use handler::{Error, Handler, Result, classified_error, process_request, run_provider};
pub use registry::ProviderRegistry;
pub use slug::{ProviderSlug, provider_slug_from_filename};
pub use storage::{ProviderStorage, StorageError};
pub use transport::ProviderTransportError;

#[derive(Debug, Clone)]
pub struct Provider {
    slug: ProviderSlug,
    transport: Arc<dyn ProviderTransport>,
}

impl Provider {
    pub(crate) fn from_binary_path(
        binary_path: PathBuf,
    ) -> std::result::Result<Self, ProviderError> {
        if !is_executable(&binary_path) {
            return Err(ProviderError::NotExecutable(binary_path));
        }

        let slug = binary_path
            .file_name()
            .and_then(|filename| filename.to_str())
            .and_then(provider_slug_from_filename)
            .ok_or_else(|| ProviderError::InvalidProviderFilename(binary_path.clone()))?;

        let transport = SubprocessTransport::new(binary_path);

        Ok(Provider {
            slug,
            transport: Arc::new(transport),
        })
    }

    pub fn slug(&self) -> &ProviderSlug {
        &self.slug
    }

    pub fn provider_account(&self, identifier: String) -> ProviderAccount {
        ProviderAccount::new(self.clone(), identifier)
    }

    pub async fn connect(
        &self,
        options: serde_json::Map<String, serde_json::Value>,
        data: serde_json::Map<String, serde_json::Value>,
    ) -> std::result::Result<rpc::ConnectResponse, ProviderError> {
        let result = self.call(rpc::Connect { options, data }).await?;

        Ok(result)
    }

    pub(crate) async fn call<C: rpc::Rpc>(
        &self,
        call: C,
    ) -> std::result::Result<C::Response, ProviderError> {
        let request_value = call.to_json().map_err(ProviderError::Serialize)?;
        let request_json =
            serde_json::to_string(&request_value).map_err(ProviderError::Serialize)?;

        // Make call:
        let response_json = self.transport.exchange(&request_json, C::TIMEOUT).await?;

        let response: rpc::Response<rpc::Wire<C::Response>> =
            serde_json::from_str(&response_json).map_err(ProviderError::Deserialize)?;

        match response {
            rpc::Response::Success { data } => Ok(rpc::WireValue::from_wire(data)),
            rpc::Response::Error { error, code } => Err(error::classify(error, code)),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_transport(
        slug: ProviderSlug,
        transport: Arc<dyn ProviderTransport>,
    ) -> Self {
        Provider { slug, transport }
    }

    #[cfg(test)]
    pub(crate) fn transport(&self) -> &dyn ProviderTransport {
        &*self.transport
    }
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    path.is_file()
        && path
            .metadata()
            .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
}

#[cfg(windows)]
fn is_executable(path: &Path) -> bool {
    path.is_file()
        && path
            .extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| format!(".{extension}"))
            .is_some_and(|extension| extension.eq_ignore_ascii_case(std::env::consts::EXE_SUFFIX))
}

#[cfg(not(any(unix, windows)))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rpc::Rpc;
    use crate::test_utils::{legacy_provider_binary, test_binary};
    use serde::{Deserialize, Serialize};
    use std::time::Duration;
    use transport::ProviderTransportError;
    use transport::mock_transport::MockTransport;

    #[derive(Serialize)]
    struct EchoCommand {
        value: String,
    }

    #[derive(Serialize, Deserialize, Debug, PartialEq)]
    struct EchoResponse {
        value: String,
    }

    impl Rpc for EchoCommand {
        type Response = EchoResponse;
        const METHOD: rpc::Method = rpc::Method::ListEvents;
        const TIMEOUT: Duration = Duration::from_secs(7);
    }

    impl rpc::JsonWireValue for EchoResponse {}

    fn provider_with_transport(transport: Arc<dyn ProviderTransport>) -> Provider {
        Provider::with_transport(ProviderSlug::from("test"), transport)
    }

    #[test]
    fn from_binary_path_succeeds_for_valid_provider_binary() {
        let (_tmp, bin) = test_binary("caldir-provider-hooli");

        let provider = Provider::from_binary_path(bin.clone()).unwrap();

        assert_eq!(provider.slug.as_str(), "hooli");
    }

    #[test]
    fn from_binary_path_errors_when_file_does_not_exist() {
        let tmp = tempfile::TempDir::new().unwrap();
        let bin = tmp.path().join("caldir-provider-nonexistant");

        let result = Provider::from_binary_path(bin.clone());

        assert!(matches!(result, Err(ProviderError::NotExecutable(p)) if p == bin));
    }

    #[cfg(unix)]
    #[test]
    fn from_binary_path_errors_when_file_not_executable() {
        let tmp = tempfile::TempDir::new().unwrap();
        let bin = tmp.path().join("caldir-provider-hooli");
        std::fs::write(&bin, b"").unwrap();

        let result = Provider::from_binary_path(bin.clone());

        assert!(matches!(result, Err(ProviderError::NotExecutable(p)) if p == bin));
    }

    #[test]
    fn from_binary_path_errors_when_filename_lacks_prefix() {
        let (_tmp, bin) = test_binary("hooli");

        let result = Provider::from_binary_path(bin.clone());

        assert!(matches!(result, Err(ProviderError::InvalidProviderFilename(p)) if p == bin));
    }

    #[test]
    fn from_binary_path_errors_when_slug_is_empty() {
        let (_tmp, bin) = test_binary("caldir-provider");

        let result = Provider::from_binary_path(bin.clone());

        assert!(matches!(result, Err(ProviderError::InvalidProviderFilename(p)) if p == bin));
    }

    #[tokio::test]
    async fn call_sends_typed_request_and_returns_typed_response() {
        let mock = Arc::new(MockTransport::with_response(
            r#"{"status":"success","data":{"value":"echoed"}}"#,
        ));
        let provider = provider_with_transport(mock.clone());

        let response = provider
            .call(EchoCommand {
                value: "hello".into(),
            })
            .await
            .unwrap();

        assert_eq!(
            response,
            EchoResponse {
                value: "echoed".into()
            }
        );

        let captured: serde_json::Value =
            serde_json::from_str(&mock.captured_request().unwrap()).unwrap();
        assert_eq!(captured["command"], "list_events");
        assert_eq!(captured["params"]["value"], "hello");
    }

    #[tokio::test]
    async fn call_uses_per_command_timeout() {
        let mock = Arc::new(MockTransport::with_response(
            r#"{"status":"success","data":{"value":"x"}}"#,
        ));
        let provider = provider_with_transport(mock.clone());

        provider
            .call(EchoCommand { value: "x".into() })
            .await
            .unwrap();

        assert_eq!(mock.captured_timeout(), Some(Duration::from_secs(7)));
    }

    #[tokio::test]
    async fn call_returns_provider_error_on_error_response() {
        let mock = Arc::new(MockTransport::with_response(
            r#"{"status":"error","error":"oh no"}"#,
        ));
        let provider = provider_with_transport(mock);

        let err = provider
            .call(EchoCommand { value: "x".into() })
            .await
            .unwrap_err();

        assert!(matches!(err, ProviderError::Provider(msg) if msg == "oh no"));
    }

    /// `P1-02`'s fail-closed branch and `P2-04`'s taxonomy both need to tell
    /// "too old to answer" from "answered and failed" without substring
    /// matching on the message (`CROSS-4`, §16).
    #[tokio::test]
    async fn an_unsupported_method_reply_becomes_a_typed_provider_error() {
        let mock = Arc::new(MockTransport::with_response(
            r#"{"status":"error","error":"unknown command: list_todos","code":"unsupported_method"}"#,
        ));
        let provider = provider_with_transport(mock);

        let err = provider
            .call(EchoCommand { value: "x".into() })
            .await
            .unwrap_err();

        assert!(
            matches!(err, ProviderError::UnsupportedMethod(ref msg) if msg.contains("list_todos")),
            "got: {err:?}"
        );
    }

    /// `W2-C2`: `error::classify` is `Provider::call`'s only production
    /// caller and had zero test callers — a mutation scrambling four of the
    /// six §16 match arms left 650/650 tests passing. Drives every wire code
    /// through the real envelope-to-typed-error path (not `classify` in
    /// isolation), one assertion per row so a partial match can't pass.
    #[tokio::test]
    async fn each_wire_error_code_maps_to_its_specific_provider_error_variant() {
        struct Case {
            code: Option<rpc::ErrorCode>,
            check: fn(&ProviderError) -> bool,
            name: &'static str,
        }

        let cases = [
            Case {
                code: Some(rpc::ErrorCode::UnsupportedMethod),
                check: |e| matches!(e, ProviderError::UnsupportedMethod(msg) if msg == "boom"),
                name: "UnsupportedMethod",
            },
            Case {
                code: Some(rpc::ErrorCode::UnsupportedOperation),
                check: |e| matches!(e, ProviderError::UnsupportedOperation(msg) if msg == "boom"),
                name: "UnsupportedOperation",
            },
            Case {
                code: Some(rpc::ErrorCode::MissingAuthScope),
                check: |e| matches!(e, ProviderError::MissingAuthScope(msg) if msg == "boom"),
                name: "MissingAuthScope",
            },
            Case {
                code: Some(rpc::ErrorCode::RemoteNotFound),
                check: |e| matches!(e, ProviderError::RemoteNotFound(msg) if msg == "boom"),
                name: "RemoteNotFound",
            },
            Case {
                code: Some(rpc::ErrorCode::Conflict),
                check: |e| matches!(e, ProviderError::Conflict(msg) if msg == "boom"),
                name: "Conflict",
            },
            Case {
                code: Some(rpc::ErrorCode::SemanticLimitation),
                check: |e| matches!(e, ProviderError::SemanticLimitation(msg) if msg == "boom"),
                name: "SemanticLimitation",
            },
            Case {
                code: Some(rpc::ErrorCode::Network),
                // Pinned specifically (§16's asymmetric row): `Network`
                // folds into the transport error family, not a second
                // network-flavoured `ProviderError` variant of its own.
                check: |e| {
                    matches!(
                        e,
                        ProviderError::Transport(ProviderTransportError::Network(msg))
                            if msg == "boom"
                    )
                },
                name: "Network",
            },
            Case {
                code: Some(rpc::ErrorCode::Unrecognized),
                check: |e| matches!(e, ProviderError::Provider(msg) if msg == "boom"),
                name: "Unrecognized",
            },
            Case {
                code: None,
                check: |e| matches!(e, ProviderError::Provider(msg) if msg == "boom"),
                name: "None",
            },
        ];

        for case in cases {
            let response_json = rpc::Response::<()>::error("boom", case.code);
            let mock = Arc::new(MockTransport::with_response(&response_json));
            let provider = provider_with_transport(mock);

            let err = provider
                .call(EchoCommand { value: "x".into() })
                .await
                .unwrap_err();

            assert!(
                (case.check)(&err),
                "code {:?} (case {}) classified as {err:?}",
                case.code,
                case.name
            );
        }
    }

    #[tokio::test]
    async fn call_returns_deserialize_error_on_garbage_response() {
        let mock = Arc::new(MockTransport::with_response("not json at all"));
        let provider = provider_with_transport(mock);

        let err = provider
            .call(EchoCommand { value: "x".into() })
            .await
            .unwrap_err();

        assert!(matches!(err, ProviderError::Deserialize(_)));
    }

    #[tokio::test]
    async fn call_propagates_transport_error() {
        let mock = Arc::new(MockTransport::with_error(ProviderTransportError::Timeout(
            Duration::from_secs(1),
        )));
        let provider = provider_with_transport(mock);

        let err = provider
            .call(EchoCommand { value: "x".into() })
            .await
            .unwrap_err();

        assert!(matches!(
            err,
            ProviderError::Transport(ProviderTransportError::Timeout(_))
        ));
    }

    /// `P6-02`. Drives a real subprocess standing in for a binary whose
    /// `Method` enum predates `capabilities` entirely — not a mock that
    /// returns a canned envelope, which would only prove the decode.
    ///
    /// The stub answers a `capabilities` request the way a binary that
    /// predates `P1-01`'s two-stage envelope decode does: the whole request
    /// line fails to parse, verbatim, with no `code` field at all (`P1-01`'s
    /// `RawRequest` indirection, and the `code` field it needs to name a
    /// verb, do not exist yet at that vintage). `classify` has nothing to
    /// key on but the generic arm, so the caller gets `ProviderError::
    /// Provider` rather than the named, typed `UnsupportedMethod` a binary
    /// built *after* `P1-01` (but still before `P1-02`) would produce —
    /// `connection::tests::an_unknown_capabilities_method_withholds_every_task_push`
    /// already proves that better-case shape end to end. Either way the call
    /// fails closed: no panic, no malformed-JSON crash, no silent `Ok`.
    ///
    /// Discovered gap (reported, not patched here per this ticket's
    /// non-goals): for *this* vintage specifically, the surfaced message
    /// cannot name the provider slug the way §17's "clear version/capability
    /// error" aspires to, because the old binary itself never puts that
    /// information on the wire and nothing on `Provider::call`'s generic
    /// arm adds it back in.
    #[serial_test::serial]
    #[tokio::test]
    async fn a_pre_capability_binary_yields_a_version_error_for_the_capabilities_verb() {
        let (_tmp, bin) = legacy_provider_binary();
        let provider = Provider::from_binary_path(bin).unwrap();
        assert_eq!(provider.slug().as_str(), "legacy");

        let err = provider
            .call(rpc::Capabilities {
                remote: crate::RemoteConfigParams::new(),
            })
            .await
            .unwrap_err();

        let ProviderError::Provider(msg) = &err else {
            panic!("expected the generic Provider arm for a pre-RawRequest binary, got {err:?}");
        };
        assert!(
            msg.contains("capabilities"),
            "the offending verb must still be visible in the message: {msg}"
        );
        assert!(
            !msg.contains("legacy"),
            "a genuinely pre-P1-01 binary cannot know its own slug; if this \
             now passes, `Provider::call` grew slug-naming and this test \
             (and its doc comment) is stale: {msg}"
        );
    }

    /// `P6-02`. §18's migration-burden bound only holds if the six
    /// unmodified verbs are shown *unmodified*, driven against the very
    /// same real subprocess the test above proves degrades correctly on the
    /// seventh — "fail closed" must not be implementable by refusing the
    /// whole binary.
    #[serial_test::serial]
    #[tokio::test]
    async fn a_pre_capability_binary_still_serves_every_original_verb() {
        let (_tmp, bin) = legacy_provider_binary();
        let provider = Provider::from_binary_path(bin).unwrap();

        provider
            .call(rpc::Connect {
                options: Default::default(),
                data: Default::default(),
            })
            .await
            .expect("connect");

        provider
            .call(rpc::ListCalendars {
                account_identifier: "legacy@example.com".to_string(),
            })
            .await
            .expect("list_calendars");

        provider
            .call(rpc::ListEvents {
                remote: crate::RemoteConfigParams::new(),
                from: "2026-01-01T00:00:00+00:00".into(),
                to: "2026-12-31T23:59:59+00:00".into(),
            })
            .await
            .expect("list_events");

        provider
            .call(rpc::CreateItem {
                remote: crate::RemoteConfigParams::new(),
                event: crate::test_utils::test_event().into(),
            })
            .await
            .expect("create_event");

        provider
            .call(rpc::UpdateItem {
                remote: crate::RemoteConfigParams::new(),
                event: crate::test_utils::test_event().into(),
            })
            .await
            .expect("update_event");

        provider
            .call(rpc::DeleteItem {
                remote: crate::RemoteConfigParams::new(),
                event: crate::test_utils::test_event().into(),
            })
            .await
            .expect("delete_event");
    }
}
