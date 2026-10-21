//! One module per RPC action, each mapping to a [`Method`] variant.

mod capabilities;
mod connect;
mod create_event;
mod delete_event;
mod list_calendars;
mod list_events;
mod update_event;

use serde::{Deserialize, Serialize};

pub use capabilities::Capabilities;
pub use connect::{
    Connect, ConnectResponse, ConnectStepKind, CredentialField, CredentialsData, FieldType,
    HostedOAuthData, OAuthData, SetupData,
};
pub use create_event::CreateItem;
pub use delete_event::DeleteItem;
pub use list_calendars::ListCalendars;
pub use list_events::ListEvents;
pub use update_event::UpdateItem;

/// R52 keeps `Method` a closed six-verb enum, inviolably — but
/// `capabilities` is the one ratified exception: task
/// capability negotiation needs a seventh verb, `Method` is not
/// `#[non_exhaustive]`, and every out-of-tree matcher (Proton included)
/// breaks the moment it lands. The graceful-skew path
/// (`ErrorCode::UnsupportedMethod`) is the migration story for every
/// provider binary built before this line landed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Method {
    Connect,
    ListCalendars,
    ListEvents,
    CreateEvent,
    UpdateEvent,
    DeleteEvent,
    Capabilities,
}
