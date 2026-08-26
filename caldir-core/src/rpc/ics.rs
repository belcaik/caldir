//! ICS string codec for calendar items on the RPC wire.
//!
//! [`item`] is the codec proper. It encodes and decodes a [`CalendarItem`]
//! and applies the **same** arity and unsupported-component rules as the
//! disk loader, `VTIMEZONE` exemption included. `#[serde(with =
//! "crate::rpc::ics::item")]` reaches it directly on every RPC struct field
//! now that the outbound write RPCs (`CreateItem`/`UpdateItem`/`DeleteItem`)
//! are item-typed (the write boundary carries `CalendarItem`, not `Event`) —
//! the same field, one layer, no narrowing.
//!
//! [`ItemIcs`] is the wire representation `CalendarItem` uses as an RPC
//! *response* type (`WireValue`), so `list_events` and the write RPCs can
//! hand a task back at all.
//!
//! `WireValue::from_wire` is infallible, so `ItemIcs` may not narrow inside
//! it — a narrowing there has no error channel and could only panic or drop.
//! Every fallible step lives in `Deserialize`, where serde's error channel
//! is, and it holds an already-decoded value afterwards.
//!
//! Decode failure is loud, and that loudness is the whole reason an old core
//! paired with a new provider is safe: an unsupported component fails the
//! item deserialization, one bad element fails the whole vector on a
//! listing, the call errors, and the sync aborts **before any apply**.

use crate::item::CalendarItem;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The **inbound** wire payload: one supported item of whichever kind.
///
/// Carries a decoded [`CalendarItem`], never a narrowing. That is what makes
/// `WireValue::from_wire`'s infallible signature safe here: by the time a
/// value exists, [`item::deserialize`] has already reported any failure
/// through serde.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ItemIcs(pub(crate) CalendarItem);

impl ItemIcs {
    pub(crate) fn into_inner(self) -> CalendarItem {
        self.0
    }
}

impl Serialize for ItemIcs {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        item::serialize(&self.0, serializer)
    }
}

impl<'de> Deserialize<'de> for ItemIcs {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        item::deserialize(deserializer).map(Self)
    }
}

/// The item-typed ICS codec — one supported component per document, of
/// whichever kind.
///
/// Shaped as a module so a field can reach it as
/// `#[serde(with = "crate::rpc::ics::item")]` the moment the method structs
/// are item-typed. Nothing here knows about `Event` specifically;
/// the narrowing lives one layer up.
pub(crate) mod item {
    use super::CalendarItem;
    use serde::{Deserialize, Deserializer, Serializer};

    /// Encode one item as a single-component `VCALENDAR` string, CRLF
    /// throughout. The JSON is a bare string — never an object, never a new
    /// field.
    pub(crate) fn serialize<S: Serializer>(
        item: &CalendarItem,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&item.to_ics_string())
    }

    /// Decode one item, routed through [`CalendarItem::from_single_ics_str`]
    /// rather than a second wire-local counting loop — the wire must apply the
    /// very same rules as the disk loader, and a wire-local loop is exactly
    /// where the `VTIMEZONE` exemption gets forgotten: every zoned task
    /// Radicale returns is a multi-component `VCALENDAR`.
    ///
    /// An unsupported component is an `Err` naming it, never a drop and never
    /// an empty result. `Error::custom` flattens the error to its
    /// `Display`, which is why the component name has to be *in* that text.
    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<CalendarItem, D::Error> {
        let ics = String::deserialize(deserializer)?;
        CalendarItem::from_single_ics_str(&ics).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Event;
    use crate::EventTime;
    use crate::test_utils::{EMPTY_VCALENDAR_ICS, VTODO_ICS};
    use chrono::NaiveDate;

    #[test]
    fn event_round_trips_through_ics_wire_format() {
        let event = Event::new(
            "Test",
            EventTime::Date(NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()),
        );
        let item = CalendarItem::Event(event.clone());

        let json = serde_json::to_value(WithItem(item)).unwrap();
        let decoded = decode_item(json.as_str().unwrap()).unwrap();

        assert_eq!(decoded, CalendarItem::Event(event));
    }

    /// The Radicale shape for any zoned task: a synthesized `VTIMEZONE`
    /// sibling, with its own nested `STANDARD`/`DAYLIGHT` sub-components,
    /// alongside the one `VTODO` (probe Q-C). Inline rather than a
    /// `test_utils` constant so the multi-component shape this test exists
    /// for cannot be edited out from under it.
    const RADICALE_ZONED_VTODO_ICS: &str = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Radicale//NONSGML Radicale Server//EN\r\n\
BEGIN:VTIMEZONE\r\n\
TZID:Europe/Stockholm\r\n\
BEGIN:STANDARD\r\n\
DTSTART:19701025T030000\r\n\
TZOFFSETFROM:+0200\r\n\
TZOFFSETTO:+0100\r\n\
TZNAME:CET\r\n\
END:STANDARD\r\n\
BEGIN:DAYLIGHT\r\n\
DTSTART:19700329T020000\r\n\
TZOFFSETFROM:+0100\r\n\
TZOFFSETTO:+0200\r\n\
TZNAME:CEST\r\n\
END:DAYLIGHT\r\n\
END:VTIMEZONE\r\n\
BEGIN:VTODO\r\n\
UID:zoned-task@u-crawler\r\n\
DTSTAMP:20260801T090000Z\r\n\
DUE;TZID=Europe/Stockholm:20260815T235900\r\n\
SUMMARY:Zoned deadline\r\n\
END:VTODO\r\n\
END:VCALENDAR\r\n";

    /// Decode a JSON string document through the item-typed wire decoder,
    /// exactly as `#[serde(with = "crate::rpc::ics::item")]` would.
    fn decode_item(ics: &str) -> Result<CalendarItem, serde_json::Error> {
        let json = serde_json::to_string(ics).unwrap();
        item::deserialize(&mut serde_json::Deserializer::from_str(&json))
    }

    /// `T-10` — R18/R102: a `VTIMEZONE` sibling is exempt from the arity
    /// count on the wire, not only on disk. Every zoned task Radicale returns
    /// is a multi-component `VCALENDAR`; an arity rule that counted the
    /// `VTIMEZONE` would fail 100% of them while every synthetic
    /// single-component fixture still passed.
    #[test]
    fn a_vtimezone_sibling_does_not_break_vtodo_decode() {
        let item = decode_item(RADICALE_ZONED_VTODO_ICS)
            .expect("a VTIMEZONE sibling must not be counted towards arity");

        let CalendarItem::Todo(todo) = item else {
            panic!("expected the VTODO, got {item:?}");
        };

        assert_eq!(todo.uid.as_str(), "zoned-task@u-crawler");
        assert_eq!(
            todo.due,
            Some(crate::EventTime::DateTimeZoned {
                datetime: NaiveDate::from_ymd_opt(2026, 8, 15)
                    .unwrap()
                    .and_hms_opt(23, 59, 0)
                    .unwrap(),
                tzid: "Europe/Stockholm".to_string(),
            })
        );
    }

    /// `T-35` — R19/C10/R57: a payload with **nothing** caldir supports is an
    /// error naming what it held, never an `Ok` that quietly decodes to
    /// nothing. A short remote list is what feeds the delete path.
    ///
    /// **Changed by the sibling-tolerance fix.** The fixture used to carry a
    /// `VTODO` beside the `VJOURNAL` and assert the whole payload failed —
    /// the same over-strict shape that lost a `VEVENT` next to a `VFREEBUSY`
    /// on disk. The property R19 actually protects is "never yields nothing",
    /// so the fixture drops the `VTODO` and the assertion is unchanged; the
    /// sibling case is locked by the test below.
    #[test]
    fn decoding_an_unknown_component_errors_rather_than_dropping() {
        let ics = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VJOURNAL\r\nUID:journal@u-crawler\r\nDTSTAMP:20260801T090000Z\r\nEND:VJOURNAL\r\n\
END:VCALENDAR\r\n";

        let err = decode_item(ics).expect_err("an unsupported component must not decode");

        assert!(
            err.to_string().contains("VJOURNAL"),
            "the offending component must survive `Error::custom`, got {err}"
        );
    }

    /// The wire seam gets the same rule as the disk loader, for the same
    /// reason: a `VJOURNAL` beside the item carries no identity the diff could
    /// read as a delete, and a server that returns one must not cost the user
    /// the item it came with.
    #[test]
    fn an_unsupported_sibling_does_not_cost_the_item_on_the_wire() {
        let ics = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VJOURNAL\r\nUID:journal@u-crawler\r\nDTSTAMP:20260801T090000Z\r\nEND:VJOURNAL\r\n\
BEGIN:VTODO\r\nUID:task@u-crawler\r\nEND:VTODO\r\nEND:VCALENDAR\r\n";

        let item = decode_item(ics).expect("the VTODO must still decode");

        assert!(
            matches!(item, CalendarItem::Todo(ref todo) if todo.uid.as_str() == "task@u-crawler")
        );
    }

    /// R75/R76 through the wire type: "none" and "too many" are distinct
    /// messages, so a debugging human can tell an empty document from a
    /// two-component one.
    #[test]
    fn zero_and_two_supported_components_are_distinct_wire_errors() {
        let two = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VEVENT\r\nUID:a@caldir\r\nDTSTART:20260301T100000Z\r\nEND:VEVENT\r\n\
BEGIN:VTODO\r\nUID:b@caldir\r\nEND:VTODO\r\nEND:VCALENDAR\r\n";

        let none_err = decode_item(EMPTY_VCALENDAR_ICS).unwrap_err().to_string();
        let two_err = decode_item(two).unwrap_err().to_string();

        assert_eq!(
            none_err,
            "expected 1 supported component(s) in ICS, found 0"
        );
        assert_eq!(two_err, "expected 1 supported component(s) in ICS, found 2");
        assert_ne!(none_err, two_err);
    }

    /// R27/R28: one supported component per document, CRLF throughout — for
    /// a task as much as for an event.
    #[test]
    fn encoding_a_task_emits_one_component_with_crlf() {
        let item = CalendarItem::from_single_ics_str(VTODO_ICS).unwrap();

        let json = serde_json::to_value(WithItem(item)).unwrap();
        let ics = json.as_str().expect("the wire payload is a plain string");

        assert_eq!(ics.matches("BEGIN:VTODO\r\n").count(), 1);
        assert_eq!(ics.matches("BEGIN:VEVENT").count(), 0);
        assert!(!ics.replace("\r\n", "").contains('\n'), "got {ics:?}");
    }

    /// R53/C8: the JSON is a bare string, not an object and not a new field —
    /// an item-typed codec must not become a schema change on the wire.
    #[test]
    fn the_wire_payload_is_still_a_plain_json_string() {
        let item = CalendarItem::from_single_ics_str(VTODO_ICS).unwrap();

        let json = serde_json::to_value(WithItem(item)).unwrap();

        assert!(json.is_string(), "got {json:?}");
    }

    /// Newtype whose `Serialize` routes through the item codec exactly as a
    /// `#[serde(with = "crate::rpc::ics::item")]` field would.
    struct WithItem(CalendarItem);

    impl Serialize for WithItem {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            item::serialize(&self.0, serializer)
        }
    }

    /// D-3/C-16, inverted: the `#[serde(with = "crate::rpc::ics::item")]`
    /// field is item-typed now, not `Event`. This fails to compile if the
    /// field narrows back — the same compile-level bite the old
    /// `the_serde_with_field_is_still_event_typed` used, in the new
    /// direction.
    #[test]
    fn the_outbound_field_is_item_typed() {
        let event = Event::new(
            "Test",
            EventTime::Date(NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()),
        );
        let cmd = crate::rpc::CreateItem {
            remote: crate::RemoteConfigParams::new(),
            event: CalendarItem::Event(event),
        };

        let _field: CalendarItem = cmd.event;
    }
}
