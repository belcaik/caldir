use crate::RemoteConfigParams;
use crate::item::CalendarItem;
use crate::rpc::{Method, Rpc};
use serde::{Deserialize, Serialize};

/// C-16: the Rust name is item-typed (`CreateItem`); the wire verb stays
/// `create_event` (`Method::CreateEvent`, unrenamed) and the JSON key stays
/// `event` — only the payload type widens, per D-3's substance/C-16's split.
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct CreateItem {
    #[serde(flatten)]
    pub remote: RemoteConfigParams,
    #[serde(with = "crate::rpc::ics::item")]
    pub event: CalendarItem,
}

impl Rpc for CreateItem {
    const METHOD: Method = Method::CreateEvent;
    type Response = CalendarItem;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Event, RemoteConfigParams, event::EventTime};

    #[test]
    fn create_event_serializes_json() {
        let mut params = RemoteConfigParams::new();
        params.insert(
            "hooli_account".to_string(),
            toml::Value::String("user@hmail.com".to_string()),
        );

        let event = Event::new(
            "Test",
            EventTime::Date(chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()),
        );

        let cmd = CreateItem {
            remote: params,
            event: CalendarItem::Event(event.clone()),
        };

        let json = cmd.to_json().unwrap();

        assert_eq!(json["command"], "create_event");
        assert_eq!(json["params"]["hooli_account"], "user@hmail.com");

        let ics = json["params"]["event"]
            .as_str()
            .expect("event should be a string");

        assert!(ics.starts_with("BEGIN:VCALENDAR"));
        assert!(ics.contains(&format!("UID:{}", event.uid.as_str())));
    }

    /// R53/S4/CROSS-6: `CreateItem` must actually carry a task — not just
    /// compile against `CalendarItem` while quietly re-encoding it as an
    /// event. `P2-01` needs somewhere to send a task at all.
    #[test]
    fn a_task_payload_survives_the_create_request_round_trip() {
        let todo = CalendarItem::from_single_ics_str(crate::test_utils::VTODO_ICS).unwrap();

        let cmd = CreateItem {
            remote: RemoteConfigParams::new(),
            event: todo.clone(),
        };

        let json = cmd.to_json().unwrap();
        let ics = json["params"]["event"]
            .as_str()
            .expect("event should be a plain string");

        assert!(ics.contains("BEGIN:VTODO"), "got {ics:?}");
        assert!(!ics.contains("BEGIN:VEVENT"), "got {ics:?}");

        let decoded: CreateItem = serde_json::from_value(json["params"].clone()).unwrap();
        assert_eq!(decoded.event, todo);
    }
}
