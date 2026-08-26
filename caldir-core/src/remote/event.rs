use chrono::{DateTime, Utc};

use crate::item::CalendarItem;

/// One item as the remote reports it.
///
/// Carries a [`CalendarItem`], not an `Event`: the diff has to be able to see
/// a remote task at all, or absence of one is indistinguishable from a delete.
/// The RPC wire underneath still hands back events only — see `rpc::ics` — so
/// widening here costs no provider change.
#[derive(Debug)]
pub struct RemoteEvent(CalendarItem);

impl RemoteEvent {
    pub fn new(item: impl Into<CalendarItem>) -> Self {
        Self(item.into())
    }

    pub fn modified_at(&self) -> Option<DateTime<Utc>> {
        self.0.last_modified()
    }

    pub fn item(&self) -> &CalendarItem {
        &self.0
    }
}
