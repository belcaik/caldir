use crate::rpc::{Method, Rpc};
use crate::{CalendarItem, RemoteConfigParams};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct ListEvents {
    #[serde(flatten)]
    pub remote: RemoteConfigParams,
    pub from: String,
    pub to: String,
}

impl Rpc for ListEvents {
    const METHOD: Method = Method::ListEvents;
    type Response = Vec<CalendarItem>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CalendarItem;
    use crate::provider::Provider;
    use crate::provider::ProviderError;
    use crate::provider::transport::mock_transport::MockTransport;
    use crate::test_utils::{VEVENT_CONTROL_ICS, VTODO_ICS};

    #[test]
    fn list_events_serializes_json() {
        let mut params = RemoteConfigParams::new();
        params.insert(
            "hooli_account".to_string(),
            toml::Value::String("user@hmail.com".to_string()),
        );

        let cmd = ListEvents {
            remote: params,
            from: "2026-01-01T00:00:00+00:00".into(),
            to: "2026-12-31T23:59:59+00:00".into(),
        };

        let json = cmd.to_json().unwrap();

        assert_eq!(json["command"], "list_events");
        assert_eq!(json["params"]["hooli_account"], "user@hmail.com");
        assert_eq!(json["params"]["from"], "2026-01-01T00:00:00+00:00");
        assert_eq!(json["params"]["to"], "2026-12-31T23:59:59+00:00");
    }

    /// The inbound seam this whole ticket exists for: `list_events` is the
    /// **only** channel by which a provider hands a task back to core. With an
    /// event-typed response no provider could ever list one — the CalDAV work
    /// in `L5` would have nowhere to return its decoded items.
    ///
    /// Driven from a raw wire envelope rather than `MockProvider::reply`, so
    /// the expected value comes from the fixture and not from the encoder
    /// under test.
    #[tokio::test]
    async fn a_listed_task_reaches_core_as_a_task() {
        let items = call_list_events(&[VTODO_ICS]).await.unwrap();

        let [CalendarItem::Todo(todo)] = items.as_slice() else {
            panic!("expected exactly one VTODO, got {items:?}");
        };

        assert_eq!(todo.uid.as_str(), "assignment-123@u-crawler");
        assert_eq!(
            todo.summary.as_deref(),
            Some("Assignment 3 - Recursive descent parser")
        );
    }

    /// R1/R2 control: the event half of the same response is unchanged. A
    /// listing that gained tasks and lost events would still pass the test
    /// above.
    #[tokio::test]
    async fn a_listed_event_still_reaches_core_as_an_event() {
        let items = call_list_events(&[VEVENT_CONTROL_ICS]).await.unwrap();

        let [CalendarItem::Event(event)] = items.as_slice() else {
            panic!("expected exactly one VEVENT, got {items:?}");
        };

        assert_eq!(event.uid.as_str(), "window-123@u-crawler");
    }

    /// R17 through the wire: both kinds in one response, in document order,
    /// so the item-typed decode is not quietly partitioning by kind.
    #[tokio::test]
    async fn a_mixed_listing_keeps_both_kinds_in_order() {
        let items = call_list_events(&[VEVENT_CONTROL_ICS, VTODO_ICS])
            .await
            .unwrap();

        let uids: Vec<&str> = items.iter().map(|item| item.uid().as_str()).collect();

        assert_eq!(
            uids,
            vec!["window-123@u-crawler", "assignment-123@u-crawler"]
        );
        assert!(matches!(items[0], CalendarItem::Event(_)), "got {items:?}");
        assert!(matches!(items[1], CalendarItem::Todo(_)), "got {items:?}");
    }

    /// R19/R57 (INVIOLABLE): one bad element fails the **whole** vector. That
    /// is the fail-safe, not a bug — a short remote list is what feeds the
    /// delete path, so a skipped element would delete the user's data.
    #[tokio::test]
    async fn one_undecodable_element_fails_the_whole_listing() {
        const VJOURNAL_ICS: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VJOURNAL\r\nUID:journal@u-crawler\r\nDTSTAMP:20260801T090000Z\r\n\
END:VJOURNAL\r\nEND:VCALENDAR\r\n";

        let err = call_list_events(&[VTODO_ICS, VJOURNAL_ICS])
            .await
            .expect_err("an undecodable element must fail the whole listing");

        let ProviderError::Deserialize(err) = err else {
            panic!("expected a deserialize failure, got {err:?}");
        };
        assert!(
            err.to_string().contains("VJOURNAL"),
            "the offending component must be named, got {err}"
        );
    }

    /// Drive `Provider::call` against a canned `list_events` success envelope
    /// whose `data` is the array of ICS strings the wire actually carries.
    async fn call_list_events(
        documents: &[&str],
    ) -> std::result::Result<Vec<CalendarItem>, ProviderError> {
        let envelope = serde_json::json!({
            "status": "success",
            "data": documents,
        });
        let transport = std::sync::Arc::new(MockTransport::with_response(envelope.to_string()));
        let provider = Provider::with_transport("hooli".into(), transport);

        provider
            .call(ListEvents {
                remote: RemoteConfigParams::new(),
                from: "2026-01-01T00:00:00+00:00".into(),
                to: "2026-12-31T23:59:59+00:00".into(),
            })
            .await
    }
}
