mod caldir;
mod calendar;
mod connection;
mod diff;
mod event;
mod item;
pub mod provider;
mod remote;
pub mod rpc;
mod todo;
mod utils;

#[cfg(test)]
mod test_utils;

// Public API:
pub use caldir::{Caldir, CaldirConfig, CaldirError, TimeFormat};
pub use calendar::{
    Calendar, CalendarConfig, CalendarError, CalendarEvent, CalendarEventError, CalendarListing,
    ExpandedItems, SkippedEvent,
};
pub use connection::{Connection, ConnectionError};
pub use diff::{CalendarDiff, EventChange};
pub use event::{
    Attachment, Attendee, Availability, ComponentKind, Event, EventError, EventInstanceId,
    EventTime, EventUid, Organizer, ParticipationStatus, Recurrence, RecurrenceId, Reminder,
    Status, Visibility, XProperty, expand_in_range, tz_normalize,
};
pub use item::CalendarItem;
pub use provider::{Provider, ProviderRegistry, ProviderSlug};
pub use remote::{Remote, RemoteConfig, RemoteConfigParams, RemoteError, RemoteEvent};
pub use todo::{Todo, TodoStatus};
pub use utils::{DateBounds, DateRange};
