use crate::RemoteConfigParams;
use crate::item::CalendarItem;
use crate::rpc::{Method, Rpc};
use serde::{Deserialize, Serialize};

/// The Rust name is item-typed (`UpdateItem`); the wire verb stays
/// `update_event` (`Method::UpdateEvent`, unrenamed) and the JSON key stays
/// `event`.
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct UpdateItem {
    #[serde(flatten)]
    pub remote: RemoteConfigParams,
    #[serde(with = "crate::rpc::ics::item")]
    pub event: CalendarItem,
}

impl Rpc for UpdateItem {
    const METHOD: Method = Method::UpdateEvent;
    type Response = CalendarItem;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Event, RemoteConfigParams, event::EventTime};

    #[test]
    fn update_event_serializes_json() {
        let mut params = RemoteConfigParams::new();
        params.insert(
            "hooli_account".to_string(),
            toml::Value::String("user@hmail.com".to_string()),
        );

        let event = Event::new(
            "Test",
            EventTime::Date(chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()),
        );

        let cmd = UpdateItem {
            remote: params,
            event: CalendarItem::Event(event.clone()),
        };

        let json = cmd.to_json().unwrap();

        assert_eq!(json["command"], "update_event");
        assert_eq!(json["params"]["hooli_account"], "user@hmail.com");

        let ics = json["params"]["event"]
            .as_str()
            .expect("event should be a string");

        assert!(ics.starts_with("BEGIN:VCALENDAR"));
        assert!(ics.contains(&format!("UID:{}", event.uid.as_str())));
    }

    /// R53/S4/CROSS-6: same shape as `CreateItem`'s sibling — all three
    /// structs must move together.
    #[test]
    fn a_task_payload_survives_the_update_request_round_trip() {
        let todo = CalendarItem::from_single_ics_str(crate::test_utils::VTODO_ICS).unwrap();

        let cmd = UpdateItem {
            remote: RemoteConfigParams::new(),
            event: todo.clone(),
        };

        let json = cmd.to_json().unwrap();
        let ics = json["params"]["event"]
            .as_str()
            .expect("event should be a plain string");

        assert!(ics.contains("BEGIN:VTODO"), "got {ics:?}");
        assert!(!ics.contains("BEGIN:VEVENT"), "got {ics:?}");

        let decoded: UpdateItem = serde_json::from_value(json["params"].clone()).unwrap();
        assert_eq!(decoded.event, todo);
    }
}
