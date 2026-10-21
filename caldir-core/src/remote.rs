mod config;
mod error;
mod event;

use crate::diff::EventChange;
use crate::provider::{ProviderCapabilities, ProviderError};
use crate::{CalendarItem, DateRange, Provider, rpc};

pub use config::{RemoteConfig, RemoteConfigParams};
pub use error::RemoteError;
pub use event::RemoteEvent;

/// provider with config should resolve to a unique remote
pub struct Remote {
    provider: Provider,
    params: RemoteConfigParams,
}

impl Remote {
    pub fn new(provider: Provider, params: RemoteConfigParams) -> Self {
        Self { provider, params }
    }

    pub async fn list_events(&self, range: &DateRange) -> Result<Vec<RemoteEvent>, RemoteError> {
        let (from, to) = range.to_rfc3339();
        let events = self
            .provider
            .call(rpc::ListEvents {
                remote: self.params.clone(),
                from,
                to,
            })
            .await?
            .into_iter()
            .map(RemoteEvent::new)
            .collect();

        Ok(events)
    }

    /// Fail closed: capabilities are per-remote, fetched fresh on every call
    /// (never persisted, `ConnectResponse` is unchanged) and every way the
    /// answer can fail to arrive — transport error, timeout, unknown
    /// method, unparseable payload — reads as task-less rather than
    /// propagating an error, so a caller can always gate on the returned
    /// value. The reason is preserved alongside it for the CLI to surface;
    /// `CalendarDiff` never sees a `Remote`.
    pub async fn capabilities(&self) -> (ProviderCapabilities, Option<RemoteError>) {
        match self
            .provider
            .call(rpc::Capabilities {
                remote: self.params.clone(),
            })
            .await
        {
            Ok(capabilities) => (capabilities, None),
            // A coded (typed) error already names its class; left as-is.
            // An uncoded one — every provider built before the `code` field
            // existed — carries only its own parse-failure text, which
            // never names the provider or the verb it choked on. Naming
            // both here (not in `classify`) keeps every typed class, and
            // every other call site, untouched.
            Err(ProviderError::Provider(message)) => (
                ProviderCapabilities::default(),
                Some(
                    ProviderError::Provider(format!(
                        "provider `{}` gave an unrecognized reply to `capabilities`: {message}",
                        self.provider.slug()
                    ))
                    .into(),
                ),
            ),
            Err(err) => (ProviderCapabilities::default(), Some(err.into())),
        }
    }

    pub async fn apply_change(
        &self,
        change: &EventChange,
    ) -> Result<Option<RemoteEvent>, RemoteError> {
        match change {
            EventChange::Create(item) => {
                let remote_event = self.create_item(item.clone()).await?;
                Ok(Some(remote_event))
            }
            // The merge is item-typed and dispatches on whichever kind
            // backs the change, so a task update carries its own
            // provider-managed `X-` properties across just as an event does.
            EventChange::Update { from, to } => {
                let merged = to.clone().with_x_properties_merged_from(from);
                let remote_event = self.update_item(merged).await?;
                Ok(Some(remote_event))
            }
            EventChange::Delete(item) => {
                self.delete_item(item.clone()).await?;
                Ok(None)
            }
        }
    }

    async fn create_item(&self, item: CalendarItem) -> Result<RemoteEvent, RemoteError> {
        let item = self
            .provider
            .call(rpc::CreateItem {
                remote: self.params.clone(),
                event: item,
            })
            .await?;

        Ok(RemoteEvent::new(item))
    }

    async fn delete_item(&self, item: CalendarItem) -> Result<(), RemoteError> {
        self.provider
            .call(rpc::DeleteItem {
                remote: self.params.clone(),
                event: item,
            })
            .await?;

        Ok(())
    }

    async fn update_item(&self, item: CalendarItem) -> Result<RemoteEvent, RemoteError> {
        let item = self
            .provider
            .call(rpc::UpdateItem {
                remote: self.params.clone(),
                event: item,
            })
            .await?;

        Ok(RemoteEvent::new(item))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ProviderSlug;
    use crate::provider::ProviderCapabilities;
    use crate::provider::transport::ProviderTransportError;
    use crate::provider::transport::mock_transport::MockTransport;
    use crate::test_utils::{test_event, test_remote, test_remote_params};
    use pretty_assertions::assert_eq;
    use std::sync::Arc;
    use std::time::Duration;

    #[tokio::test]
    async fn apply_change_sends_create_event_for_outgoing_create() {
        let (mock, remote) = test_remote();
        let event = test_event();
        mock.reply::<rpc::CreateItem>(CalendarItem::Event(event.clone()));

        remote
            .apply_change(&EventChange::Create(event.clone().into()))
            .await
            .unwrap();

        assert_eq!(
            mock.captured_request::<rpc::CreateItem>().event,
            CalendarItem::Event(event)
        );
    }

    #[tokio::test]
    async fn apply_change_sends_update_event_for_outgoing_update() {
        let (mock, remote) = test_remote();
        let from = test_event();
        let mut to = from.clone();
        to.summary = Some("Updated".into());
        mock.reply::<rpc::UpdateItem>(CalendarItem::Event(to.clone()));

        remote
            .apply_change(&EventChange::Update {
                from: from.into(),
                to: to.clone().into(),
            })
            .await
            .unwrap();

        assert_eq!(
            mock.captured_request::<rpc::UpdateItem>().event,
            CalendarItem::Event(to)
        );
    }

    /// P2-01/S5, the update arm at the `Remote` seam. The only arm that
    /// transforms its input (the `X-` merge) before sending, so a task
    /// update needs its own dispatch proof: an untested arm here would send
    /// `from` unmodified and the user's edit would silently never land.
    #[tokio::test]
    async fn apply_change_sends_a_vtodo_for_an_outgoing_task_update() {
        let (mock, remote) = test_remote();
        let from = crate::todo::Todo::new("Hand in the assignment");
        let mut to = from.clone();
        to.summary = Some("Hand in the revised assignment".into());
        mock.reply::<rpc::UpdateItem>(CalendarItem::Todo(to.clone()));

        remote
            .apply_change(&EventChange::Update {
                from: from.into(),
                to: to.clone().into(),
            })
            .await
            .expect("a task update must reach the provider");

        let captured = mock.captured_request::<rpc::UpdateItem>().event;
        assert_eq!(captured, CalendarItem::Todo(to));

        let ics = captured.to_ics_string();
        assert!(ics.contains("BEGIN:VTODO"), "got {ics:?}");
        assert!(!ics.contains("BEGIN:VEVENT"), "got {ics:?}");
    }

    /// C-13/P2-01. Mirrors
    /// `apply_change_update_merges_x_property_params_from_remote` with a
    /// `Todo`: a provider-managed `X-` parameter present only on `from` must
    /// survive into the pushed `to`. The natural task implementation —
    /// "no merge for tasks" — compiles, passes every other test, and
    /// silently strips provider metadata off every task update; this is the
    /// test that catches it.
    #[tokio::test]
    async fn apply_change_update_merges_x_property_params_for_a_task() {
        let mut from = crate::todo::Todo::new("Hand in the assignment");
        from.x_properties = vec![crate::event::XProperty {
            name: "X-CALDAV-TASK-ID".to_string(),
            value: "abc123".to_string(),
            params: vec![("X-TITLE".to_string(), "Server label".to_string())],
        }];

        let mut to = from.clone();
        to.summary = Some("Updated".into());
        to.x_properties = vec![crate::event::XProperty::new("X-CALDAV-TASK-ID", "abc123")];

        let (mock, remote) = test_remote();
        mock.reply::<rpc::UpdateItem>(CalendarItem::Todo(to.clone()));

        remote
            .apply_change(&EventChange::Update {
                from: from.into(),
                to: to.into(),
            })
            .await
            .unwrap();

        let captured = mock.captured_request::<rpc::UpdateItem>().event;
        let CalendarItem::Todo(todo) = captured else {
            panic!("expected a Todo, got {captured:?}");
        };
        assert_eq!(todo.x_properties.len(), 1);
        assert_eq!(
            todo.x_properties[0].params,
            vec![("X-TITLE".to_string(), "Server label".to_string())]
        );
    }

    #[tokio::test]
    async fn apply_change_update_merges_x_property_params_from_remote() {
        // Local copy was written by an old parser that dropped params;
        // remote still carries them. Push must send the union so we don't
        // strip provider-managed metadata.
        let (mock, remote) = test_remote();

        let mut from = test_event();
        from.x_properties = vec![crate::event::XProperty {
            name: "X-APPLE-STRUCTURED-LOCATION".to_string(),
            value: "geo:51.47,-0.45".to_string(),
            params: vec![("X-TITLE".to_string(), "London Heathrow".to_string())],
        }];

        let mut to = from.clone();
        to.summary = Some("Updated".into());
        to.x_properties = vec![crate::event::XProperty::new(
            "X-APPLE-STRUCTURED-LOCATION",
            "geo:51.47,-0.45",
        )];

        mock.reply::<rpc::UpdateItem>(CalendarItem::Event(to.clone()));

        remote
            .apply_change(&EventChange::Update {
                from: from.into(),
                to: to.into(),
            })
            .await
            .unwrap();

        let captured = mock
            .captured_request::<rpc::UpdateItem>()
            .event
            .into_event()
            .unwrap();
        assert_eq!(captured.x_properties.len(), 1);
        assert_eq!(
            captured.x_properties[0].params,
            vec![("X-TITLE".to_string(), "London Heathrow".to_string())]
        );
    }

    /// P2-01/S5: the RPC payload is item-typed, so a task create actually
    /// reaches the provider now — the captured request carries the exact
    /// `Todo` it was handed, and the bytes on the wire are `VTODO`, never a
    /// synthesized `VEVENT` (`CROSS-6`). Formerly
    /// `a_task_change_aborts_instead_of_reaching_the_provider`, which
    /// asserted the opposite: that nothing reached the provider at all.
    #[tokio::test]
    async fn apply_change_sends_a_vtodo_for_an_outgoing_task_create() {
        let (mock, remote) = test_remote();
        let todo = crate::todo::Todo::new("Hand in the assignment");
        mock.reply::<rpc::CreateItem>(CalendarItem::Todo(todo.clone()));

        remote
            .apply_change(&EventChange::Create(todo.clone().into()))
            .await
            .expect("a task create must reach the provider");

        let captured = mock.captured_request::<rpc::CreateItem>().event;
        assert_eq!(captured, CalendarItem::Todo(todo));

        let ics = captured.to_ics_string();
        assert!(ics.contains("BEGIN:VTODO"), "got {ics:?}");
        assert!(!ics.contains("BEGIN:VEVENT"), "got {ics:?}");
    }

    /// The other half of `push_outgoing_changes`'s
    /// `unreachable!("apply_change returns None for Delete")`: a `Delete` that
    /// succeeds must yield no `RemoteEvent`. Its task sibling — a `Delete`
    /// that aborts before it can return at all — is pinned in
    /// `connection::tests::a_task_only_delete_aborts_before_reaching_the_provider`.
    #[tokio::test]
    async fn apply_change_yields_no_remote_event_for_a_delete() {
        let (mock, remote) = test_remote();
        mock.reply::<rpc::DeleteItem>(());

        let applied = remote
            .apply_change(&EventChange::Delete(test_event().into()))
            .await
            .unwrap();

        assert!(
            applied.is_none(),
            "a delete has no returned item; the push loop treats Some as impossible",
        );
    }

    /// P2-01/S5, the delete arm at the `Remote` seam. Companion to
    /// `apply_change_sends_a_vtodo_for_an_outgoing_task_create`. A task
    /// delete now reaches the provider carrying the task's own item, and
    /// still returns `Ok(None)` — the delete arm never yields a
    /// `RemoteEvent`, so `push_outgoing_changes`'s
    /// `unreachable!("apply_change returns None for Delete")` stays sound
    /// even now that a task delete is the newest shape that could reach it.
    /// Formerly `a_task_delete_aborts_instead_of_reaching_the_provider`,
    /// which asserted the opposite: that nothing reached the provider.
    #[tokio::test]
    async fn apply_change_sends_a_vtodo_for_an_outgoing_task_delete() {
        let (mock, remote) = test_remote();
        let todo = crate::todo::Todo::new("Hand in the assignment");
        mock.reply::<rpc::DeleteItem>(());

        let applied = remote
            .apply_change(&EventChange::Delete(todo.clone().into()))
            .await
            .expect("a task delete must reach the provider");

        assert_eq!(
            mock.captured_request::<rpc::DeleteItem>().event,
            CalendarItem::Todo(todo)
        );
        assert!(
            applied.is_none(),
            "a delete has no returned item; the push loop treats Some as impossible",
        );
    }

    #[tokio::test]
    async fn apply_change_sends_delete_event_for_outgoing_delete() {
        let (mock, remote) = test_remote();
        let event = test_event();
        mock.reply::<rpc::DeleteItem>(());

        remote
            .apply_change(&EventChange::Delete(event.clone().into()))
            .await
            .unwrap();

        assert_eq!(
            mock.captured_request::<rpc::DeleteItem>().event,
            CalendarItem::Event(event)
        );
    }

    /// C-13, the item-typed sibling of
    /// `apply_change_update_merges_x_property_params_from_remote`: the merge
    /// this ticket moves ahead of `into_event`'s narrowing must apply to a
    /// task too, or an item-typed update path compiles perfectly while
    /// silently dropping every provider-managed `X-` property on a task — a
    /// strip no round-trip-equality test would catch, because both sides
    /// would agree on the stripped value. The narrow after the merge still
    /// aborts (`P2-01` owns real task dispatch); what this pins is that the
    /// merge itself already ran, item-typed, before that abort.
    #[test]
    fn a_task_update_keeps_the_remote_x_properties() {
        let mut from = crate::todo::Todo::new("Hand in the assignment");
        from.x_properties = vec![crate::event::XProperty {
            name: "X-CALDAV-TASK-ID".to_string(),
            value: "abc123".to_string(),
            params: vec![("X-TITLE".to_string(), "Server label".to_string())],
        }];

        let mut to = from.clone();
        to.summary = Some("Updated".into());
        to.x_properties = vec![crate::event::XProperty::new("X-CALDAV-TASK-ID", "abc123")];

        let merged =
            CalendarItem::from(to).with_x_properties_merged_from(&CalendarItem::from(from));

        let CalendarItem::Todo(todo) = merged else {
            panic!("expected a Todo, got {merged:?}");
        };
        assert_eq!(todo.x_properties.len(), 1);
        assert_eq!(
            todo.x_properties[0].params,
            vec![("X-TITLE".to_string(), "Server label".to_string())]
        );
    }

    /// D-2's core assertion: every way the answer can fail to arrive reads
    /// as task-less, and the reason is preserved for the CLI to surface
    /// (C-8). A test of only the honest reply proves nothing — a provider
    /// that answers politely was never the hazard.
    #[tokio::test]
    async fn a_provider_that_cannot_answer_capabilities_is_read_as_task_less() {
        let cases: Vec<(&str, Arc<MockTransport>)> = vec![
            (
                "transport error",
                Arc::new(MockTransport::with_error(ProviderTransportError::Io(
                    std::io::Error::other("pipe closed"),
                ))),
            ),
            (
                "timeout",
                Arc::new(MockTransport::with_error(ProviderTransportError::Timeout(
                    Duration::from_secs(15),
                ))),
            ),
            (
                "unsupported method",
                Arc::new(MockTransport::with_response(
                    r#"{"status":"error","error":"unknown command: capabilities","code":"unsupported_method"}"#,
                )),
            ),
            (
                "unparseable payload",
                Arc::new(MockTransport::with_response(
                    r#"{"status":"success","data":"not an object"}"#,
                )),
            ),
        ];

        for (name, transport) in cases {
            let provider = Provider::with_transport(ProviderSlug::from("test"), transport);
            let remote = Remote::new(provider, RemoteConfigParams::new());

            let (capabilities, reason) = remote.capabilities().await;

            assert_eq!(
                capabilities,
                ProviderCapabilities::default(),
                "case {name}: must read as task-less (and claim nothing about events either)"
            );
            assert!(
                reason.is_some(),
                "case {name}: the reason must be preserved"
            );
        }
    }

    /// `W6-C2`. A genuinely pre-`P1-01` binary's own parse failure carries no
    /// `code`, so `classify` has nothing to key on but the generic arm and
    /// the text is passed through verbatim (`P6-02`, `provider::tests::
    /// a_pre_capability_binary_yields_a_version_error_for_the_capabilities_verb`
    /// proves that end to end against a real subprocess). `Remote::
    /// capabilities()` is the seam that knows both the provider it asked and
    /// that it asked for `capabilities`, so the reason it hands back should
    /// read as a version/capability problem, not a bare parse error, while
    /// still folding to the fail-closed default untouched (§17, D-2).
    #[tokio::test]
    async fn an_uncoded_capabilities_error_names_the_provider_and_method() {
        let transport = Arc::new(MockTransport::with_response(
            r#"{"status":"error","error":"Failed to parse request: unknown variant `capabilities`, expected one of `connect`, `list_calendars`, `list_events`, `create_event`, `update_event`, `delete_event`"}"#,
        ));
        let provider = Provider::with_transport(ProviderSlug::from("legacy"), transport);
        let remote = Remote::new(provider, RemoteConfigParams::new());

        let (capabilities, reason) = remote.capabilities().await;

        assert_eq!(
            capabilities,
            ProviderCapabilities::default(),
            "must still read as task-less"
        );
        let reason = reason.expect("the reason must be preserved").to_string();
        assert!(
            reason.contains("legacy"),
            "must name the provider: {reason}"
        );
        assert!(
            reason.contains("capabilities"),
            "must name the RPC method: {reason}"
        );
        assert!(
            reason.contains("unknown variant `capabilities`"),
            "the provider's own diagnostic text must survive verbatim inside the wrapper: {reason}"
        );
    }

    /// `P6-04`/`CROSS-1`+`CROSS-2` in one assertion: drives the identical
    /// task create through a `caldav`-slugged and a `google`-slugged
    /// `Provider` and asserts the two captured request bodies are
    /// byte-equal. One core `Task` type serves every provider (`CROSS-1`),
    /// and core's outbound bytes are provably independent of who is
    /// listening — the only way `apply_change`/`create_item` could differ
    /// per slug is a `match self.provider.slug()` this test would catch
    /// (`CROSS-2`).
    #[tokio::test]
    async fn a_task_write_produces_the_same_request_for_every_provider_slug() {
        let todo = crate::todo::Todo::new("Hand in the assignment");

        let caldav_mock = crate::provider::mock_provider::MockProvider::new("caldav");
        let caldav_remote = Remote::new(caldav_mock.provider(), test_remote_params());
        caldav_mock.reply::<rpc::CreateItem>(CalendarItem::Todo(todo.clone()));
        caldav_remote
            .apply_change(&EventChange::Create(todo.clone().into()))
            .await
            .expect("a task create must reach the caldav provider");

        let google_mock = crate::provider::mock_provider::MockProvider::new("google");
        let google_remote = Remote::new(google_mock.provider(), test_remote_params());
        google_mock.reply::<rpc::CreateItem>(CalendarItem::Todo(todo.clone()));
        google_remote
            .apply_change(&EventChange::Create(todo.clone().into()))
            .await
            .expect("a task create must reach the google provider");

        let caldav_request = caldav_mock.captured_request::<rpc::CreateItem>();
        let google_request = google_mock.captured_request::<rpc::CreateItem>();

        let caldav_body = serde_json::to_string(&caldav_request).unwrap();
        let google_body = serde_json::to_string(&google_request).unwrap();

        assert_eq!(
            caldav_body, google_body,
            "the outbound task-create body must not depend on the provider slug"
        );
    }
}
