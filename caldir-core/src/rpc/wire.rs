use crate::CalendarConfig;
use crate::item::CalendarItem;
use crate::provider::ProviderCapabilities;
use crate::rpc::ConnectResponse;
use crate::rpc::ics::ItemIcs;
use serde::Serialize;
use serde::de::DeserializeOwned;

/// Bridges a domain type to the JSON its RPC payload is made of.
///
/// [`from_wire`](WireValue::from_wire) is **infallible**, so `Wire` must
/// decode straight into the domain value — a narrowing here would have no
/// error channel and could only panic or silently drop.
/// Every fallible step belongs in `Wire`'s `Deserialize`.
pub(crate) trait WireValue: Sized {
    type Wire: Serialize + DeserializeOwned;

    fn into_wire(self) -> Self::Wire;
    fn from_wire(wire: Self::Wire) -> Self;
}

/// The inbound item payload (`list_events`'s response, and now the write
/// RPCs' response too — D-3/C-16). Infallible in both
/// directions because [`ItemIcs`] already holds a decoded item.
impl WireValue for CalendarItem {
    type Wire = ItemIcs;

    fn into_wire(self) -> Self::Wire {
        ItemIcs(self)
    }

    fn from_wire(wire: Self::Wire) -> Self {
        wire.into_inner()
    }
}

/// One bad element fails the whole vector — the fail-safe, not a bug: a short
/// remote listing is what feeds the delete path.
impl<T: WireValue> WireValue for Vec<T> {
    type Wire = Vec<T::Wire>;

    fn into_wire(self) -> Self::Wire {
        self.into_iter().map(WireValue::into_wire).collect()
    }

    fn from_wire(wire: Self::Wire) -> Self {
        wire.into_iter().map(T::from_wire).collect()
    }
}

/// Response types that pass through as plain JSON; ICS-encoded types impl [`WireValue`] directly.
pub(crate) trait JsonWireValue: Serialize + DeserializeOwned {}

impl JsonWireValue for () {}
impl JsonWireValue for CalendarConfig {}
impl JsonWireValue for ConnectResponse {}
impl JsonWireValue for ProviderCapabilities {}

impl<T: JsonWireValue> WireValue for T {
    type Wire = Self;

    fn into_wire(self) -> Self::Wire {
        self
    }

    fn from_wire(wire: Self::Wire) -> Self {
        wire
    }
}

pub(crate) type Wire<T> = <T as WireValue>::Wire;
