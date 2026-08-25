//! ICS string codec for events on the RPC wire.
//! Fields use `#[serde(with = "crate::rpc::ics")]`; response values wrap in [`Ics`].

use crate::Event;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ics(pub Event);

impl Ics {
    pub fn into_inner(self) -> Event {
        self.0
    }
}

impl Serialize for Ics {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serialize(&self.0, serializer)
    }
}

impl<'de> Deserialize<'de> for Ics {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserialize(deserializer).map(Self)
    }
}

pub(crate) fn serialize<S: Serializer>(event: &Event, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&event.to_ics_string())
}

/// The wire payload stays an `Event`: retyping it to `CalendarItem` would put
/// ~25 `cmd.event` sites across five provider crates in this feature's blast
/// radius (R70). Parsing now goes through the item loader, and a task is
/// narrowed away here with the arity error the wire already reports (R71).
pub(crate) fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Event, D::Error> {
    let ics = String::deserialize(deserializer)?;
    // TODO(L6-01): pushing a task to a provider aborts here.
    Event::narrowed_from_single_ics_str(&ics).map_err(serde::de::Error::custom)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EventTime;
    use chrono::NaiveDate;

    #[test]
    fn event_round_trips_through_ics_wire_format() {
        let event = Event::new(
            "Test",
            EventTime::Date(NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()),
        );

        let json = serde_json::to_string(&Ics(event.clone())).unwrap();
        let decoded: Ics = serde_json::from_str(&json).unwrap();

        assert_eq!(decoded.into_inner(), event);
    }

    /// R70/R71: the wire payload is still an `Event`, so a task cannot ride
    /// it. The abort is the arity error the wire already defined — locked
    /// verbatim, because `L6-01` builds the user-facing story on this text
    /// and a silent success here would push a task as if it were an event.
    #[test]
    fn a_task_cannot_ride_the_event_only_wire() {
        use crate::test_utils::VTODO_ICS;

        let json = serde_json::to_string(VTODO_ICS).unwrap();
        let err = serde_json::from_str::<Ics>(&json)
            .expect_err("a VTODO must never decode into the event-only wire payload");

        assert_eq!(err.to_string(), "expected 1 event(s) in ICS, found 0");
    }
}
