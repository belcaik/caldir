mod config;
mod error;
mod event;

use crate::diff::EventChange;
use crate::provider::{ProviderCapabilities, ProviderError};
use crate::{CalendarItem, DateRange, Event, Provider, rpc};

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

    /// D-2, fail closed: capabilities are fetched fresh on every call
    /// (never persisted, `ConnectResponse` is unchanged) and every way the
    /// answer can fail to arrive — transport error, timeout, unknown
    /// method, unparseable payload — reads as task-less rather than
    /// propagating an error, so a caller can always gate on the returned
    /// value. The reason is preserved alongside it for the CLI to surface
    /// (C-8); `P1-03` is the consumer, `CalendarDiff` never sees a `Remote`.
    pub async fn capabilities(&self) -> (ProviderCapabilities, Option<RemoteError>) {
        match self
            .provider
            .call(rpc::Capabilities {
                remote: self.params.clone(),
            })
            .await
        {
            Ok(capabilities) => (capabilities, None),
            Err(err) => (ProviderCapabilities::default(), Some(err.into())),
        }
    }

    pub async fn apply_change(
        &self,
        change: &EventChange,
    ) -> Result<Option<RemoteEvent>, RemoteError> {
        match change {
            EventChange::Create(item) => {
                let remote_event = self.create_event(item.clone().into_event()?).await?;
                Ok(Some(remote_event))
            }
            // C-13: the merge is item-typed and runs *before* the
            // event-only narrowing (P1-04's non-goal keeps dispatch here
            // unchanged — `P2-01` owns sending a task for real), so a task
            // update never silently drops provider-managed `X-` properties
            // even though the narrow right after it still aborts.
            EventChange::Update { from, to } => {
                let merged = to
                    .clone()
                    .with_x_properties_merged_from(&from.clone())
                    .into_event()?;
                let remote_event = self.update_event(merged).await?;
                Ok(Some(remote_event))
            }
            EventChange::Delete(item) => {
                self.delete_event(item.clone().into_event()?).await?;
                Ok(None)
            }
        }
    }

    async fn create_event(&self, event: Event) -> Result<RemoteEvent, RemoteError> {
        let item = self
            .provider
            .call(rpc::CreateItem {
                remote: self.params.clone(),
                event: CalendarItem::Event(event),
            })
            .await?;

        Ok(RemoteEvent::new(item.into_event()?))
    }

    async fn delete_event(&self, event: Event) -> Result<(), RemoteError> {
        self.provider
            .call(rpc::DeleteItem {
                remote: self.params.clone(),
                event: CalendarItem::Event(event),
            })
            .await?;

        Ok(())
    }

    async fn update_event(&self, event: Event) -> Result<RemoteEvent, RemoteError> {
        let item = self
            .provider
            .call(rpc::UpdateItem {
                remote: self.params.clone(),
                event: CalendarItem::Event(event),
            })
            .await?;

        Ok(RemoteEvent::new(item.into_event()?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ProviderSlug;
    use crate::provider::ProviderCapabilities;
    use crate::provider::transport::ProviderTransportError;
    use crate::provider::transport::mock_transport::MockTransport;
    use crate::test_utils::{test_event, test_remote};
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

    /// R70/R71: the RPC payload is still an `Event`, so a task-shaped change
    /// cannot reach a provider. It aborts with the arity error the wire
    /// already defines — never a synthesized event, which would push a task
    /// as if it were a meeting. `L6-01` owns the user-facing story.
    #[tokio::test]
    async fn a_task_change_aborts_instead_of_reaching_the_provider() {
        let (mock, remote) = test_remote();
        let todo = crate::todo::Todo::new("Hand in the assignment");

        let err = remote
            .apply_change(&EventChange::Create(todo.into()))
            .await
            .expect_err("a task must never be pushed over the event-only wire");

        assert_eq!(err.to_string(), "expected 1 event(s) in ICS, found 0");
        assert!(!mock.received_a_request(), "nothing may reach the provider");
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

    /// R70/R71, the delete arm at the `Remote` seam. Companion to
    /// `a_task_change_aborts_instead_of_reaching_the_provider`, which covers
    /// `Create`.
    #[tokio::test]
    async fn a_task_delete_aborts_instead_of_reaching_the_provider() {
        let (mock, remote) = test_remote();
        let todo = crate::todo::Todo::new("Hand in the assignment");

        let err = remote
            .apply_change(&EventChange::Delete(todo.into()))
            .await
            .expect_err("a task delete must never be pushed over the event-only wire");

        assert_eq!(err.to_string(), "expected 1 event(s) in ICS, found 0");
        assert!(!mock.received_a_request(), "nothing may reach the provider");
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
}
