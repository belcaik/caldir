use crate::item::CalendarItem;

/// One pending change, carrying whichever component kind it is about.
///
/// Retyped from `Event` to [`CalendarItem`] so a task can be diffed at all
/// (R36). The wire payload deliberately stays `Event` — see `rpc::ics` — so a
/// task-shaped change aborts at the provider boundary rather than widening the
/// provider blast radius (R70, R71).
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
