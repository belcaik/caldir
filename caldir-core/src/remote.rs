mod config;
mod error;
mod event;

use crate::diff::EventChange;
use crate::provider::ProviderError;
use crate::{DateRange, Event, Provider, rpc};

pub use config::{RemoteConfig, RemoteConfigParams};
pub(crate) use error::RemoteError;
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

    pub async fn apply_change(
        &self,
        change: &EventChange,
    ) -> Result<Option<RemoteEvent>, RemoteError> {
        match change {
            EventChange::Create(item) => {
                let remote_event = self.create_event(item.clone().into_event()?).await?;
                Ok(Some(remote_event))
            }
            EventChange::Update { from, to } => {
                let merged = to
                    .clone()
                    .into_event()?
                    .with_x_properties_merged_from(&from.clone().into_event()?);
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
        let event = self
            .provider
            .call(rpc::CreateEvent {
                remote: self.params.clone(),
                event,
            })
            .await?;

        Ok(RemoteEvent::new(event))
    }

    async fn delete_event(&self, event: Event) -> Result<(), RemoteError> {
        self.provider
            .call(rpc::DeleteEvent {
                remote: self.params.clone(),
                event,
            })
            .await?;

        Ok(())
    }

    async fn update_event(&self, event: Event) -> Result<RemoteEvent, RemoteError> {
        let event = self
            .provider
            .call(rpc::UpdateEvent {
                remote: self.params.clone(),
                event,
            })
            .await?;

        Ok(RemoteEvent::new(event))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{test_event, test_remote};
    use pretty_assertions::assert_eq;

    #[tokio::test]
    async fn apply_change_sends_create_event_for_outgoing_create() {
        let (mock, remote) = test_remote();
        let event = test_event();
        mock.reply::<rpc::CreateEvent>(event.clone());

        remote
            .apply_change(&EventChange::Create(event.clone().into()))
            .await
            .unwrap();

        assert_eq!(mock.captured_request::<rpc::CreateEvent>().event, event);
    }

    #[tokio::test]
    async fn apply_change_sends_update_event_for_outgoing_update() {
        let (mock, remote) = test_remote();
        let from = test_event();
        let mut to = from.clone();
        to.summary = Some("Updated".into());
        mock.reply::<rpc::UpdateEvent>(to.clone());

        remote
            .apply_change(&EventChange::Update {
                from: from.into(),
                to: to.clone().into(),
            })
            .await
            .unwrap();

        assert_eq!(mock.captured_request::<rpc::UpdateEvent>().event, to);
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

        mock.reply::<rpc::UpdateEvent>(to.clone());

        remote
            .apply_change(&EventChange::Update {
                from: from.into(),
                to: to.into(),
            })
            .await
            .unwrap();

        let captured = mock.captured_request::<rpc::UpdateEvent>().event;
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
        mock.reply::<rpc::DeleteEvent>(());

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
        mock.reply::<rpc::DeleteEvent>(());

        remote
            .apply_change(&EventChange::Delete(event.clone().into()))
            .await
            .unwrap();

        assert_eq!(mock.captured_request::<rpc::DeleteEvent>().event, event);
    }
}
