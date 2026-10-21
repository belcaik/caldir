use crate::item::CalendarItem;

/// One pending change, carrying whichever component kind it is about.
///
/// Retyped from `Event` to [`CalendarItem`] so a task can be diffed at all.
/// The wire payload deliberately stays `Event` — see `rpc::ics` — so a
/// task-shaped change aborts at the provider boundary rather than widening the
/// provider blast radius.
#[derive(Debug, PartialEq, Eq)]
#[allow(clippy::large_enum_variant)]
pub enum EventChange {
    Create(CalendarItem),
    Update {
        from: CalendarItem,
        to: CalendarItem,
    },
    Delete(CalendarItem),
}

impl EventChange {
    /// The item this change *writes*: an update's `to`, never its `from`.
    pub(crate) fn written_item(&self) -> &CalendarItem {
        match self {
            EventChange::Create(item) | EventChange::Delete(item) => item,
            EventChange::Update { to, .. } => to,
        }
    }
}
