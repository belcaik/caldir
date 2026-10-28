use crate::{event::EventInstanceId, item::CalendarItem};
use std::collections::HashMap;
use std::path::Path;

use super::error::CalendarStateError;
use super::event_bases::{EVENT_BASES_DIR_NAME, EventBases};
use super::known_event_ids::{KNOWN_IDS_FILE_NAME, KnownEventIds};

// If a base file exists -> <EventInstanceId, Some<CalendarItem>>
// If no base file, but a known event ID exists -> <EventInstanceId, None>
//
// The key carries the component kind, so an event and a task sharing a UID get
// two distinct entries backed by two distinct base files.
#[derive(Debug)]
pub(crate) struct SyncBases(HashMap<EventInstanceId, Option<Box<CalendarItem>>>);

impl SyncBases {
    pub(crate) fn new() -> Self {
        Self(HashMap::new())
    }

    /// Presence, kind-agnostic: this is what delete detection reads, so it
    /// must answer for a task exactly as it does for an event.
    pub(crate) fn get(&self, id: &EventInstanceId) -> Option<&Option<Box<CalendarItem>>> {
        self.0.get(id)
    }

    #[cfg(test)]
    pub(crate) fn iter(
        &self,
    ) -> impl Iterator<Item = (&EventInstanceId, &Option<Box<CalendarItem>>)> {
        self.0.iter()
    }

    pub(crate) fn load_from_state_dir(state_dir: &Path) -> Result<Self, CalendarStateError> {
        let known_event_ids = Self::load_known_event_ids(state_dir)?;
        let event_bases = Self::load_event_bases(state_dir)?;

        let sync_bases = Self::from_event_bases_and_known_ids(event_bases, known_event_ids);

        Ok(sync_bases)
    }

    // Legacy method:
    pub(crate) fn insert_known_event_id(&mut self, id: EventInstanceId) {
        self.0.entry(id).or_insert(None);
    }

    pub(crate) fn insert_event_base(&mut self, id: EventInstanceId, item: impl Into<CalendarItem>) {
        self.0.insert(id, Some(Box::new(item.into())));
    }

    /// Records new bases and persists them. Only the given items' base files
    /// are written, so a sync touches O(changes) files, not O(history).
    pub(crate) fn record<I>(&mut self, items: I, state_dir: &Path) -> Result<(), CalendarStateError>
    where
        I: IntoIterator,
        I::Item: Into<CalendarItem>,
    {
        let items: Vec<CalendarItem> = items.into_iter().map(Into::into).collect();

        for item in &items {
            self.insert_event_base(item.instance_id(), item.clone());
        }

        // Keep writing the legacy format for clients using an older caldir-core.
        KnownEventIds::write_from(self.0.keys(), &state_dir.join(KNOWN_IDS_FILE_NAME))?;

        // New format with item bases:
        EventBases::write_from(items.iter(), &state_dir.join(EVENT_BASES_DIR_NAME))?;

        Ok(())
    }

    pub(crate) fn forget<I>(&mut self, ids: I, state_dir: &Path) -> Result<(), CalendarStateError>
    where
        I: IntoIterator<Item = EventInstanceId>,
    {
        let ids: Vec<_> = ids.into_iter().collect();
        if ids.is_empty() {
            return Ok(());
        }

        for id in &ids {
            EventBases::remove(id, &state_dir.join(EVENT_BASES_DIR_NAME))?;
        }

        KnownEventIds::write_from(
            self.0.keys().filter(|id| !ids.contains(id)),
            &state_dir.join(KNOWN_IDS_FILE_NAME),
        )?;

        for id in ids {
            self.0.remove(&id);
        }

        Ok(())
    }

    // Legacy file:
    fn load_known_event_ids(state_dir: &Path) -> Result<KnownEventIds, CalendarStateError> {
        let known_ids_path = state_dir.join(KNOWN_IDS_FILE_NAME);
        KnownEventIds::load(&known_ids_path)
    }

    // New file format with event bases:
    fn load_event_bases(state_dir: &Path) -> Result<EventBases, CalendarStateError> {
        let event_bases_dir = state_dir.join(EVENT_BASES_DIR_NAME);
        let event_bases = EventBases::load(&event_bases_dir)?;
        Ok(event_bases)
    }

    // Use event bases if they exist,
    // Fallback to known event IDs if no event base exists:
    fn from_event_bases_and_known_ids(
        event_bases: EventBases,
        known_event_ids: KnownEventIds,
    ) -> Self {
        let mut sync_bases = Self::new();

        for id in known_event_ids.iter() {
            sync_bases.insert_known_event_id(id.clone());
        }

        for (id, item) in event_bases.into_iter() {
            sync_bases.insert_event_base(id, item);
        }

        sync_bases
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::test_event;
    use pretty_assertions::assert_eq;

    #[test]
    fn corrupt_base_degrades_to_known_id_entry() {
        let state_dir = tempfile::TempDir::new().unwrap();
        let event = test_event();
        let id = event.event_instance_id();

        let mut sync_bases = SyncBases::new();
        sync_bases.record([event], state_dir.path()).unwrap();

        let bases_dir = state_dir.path().join(EVENT_BASES_DIR_NAME);
        for entry in std::fs::read_dir(&bases_dir).unwrap() {
            std::fs::write(entry.unwrap().path(), "not an ics file").unwrap();
        }

        let loaded = SyncBases::load_from_state_dir(state_dir.path()).unwrap();

        // Base content is lost, but the known-id entry keeps deletion memory.
        assert_eq!(loaded.get(&id), Some(&None));
    }

    /// AC2 at the map level, and the degradation the L1-06/L3-01 split
    /// exposes: a task with no base file of its own is *absent* from the
    /// store — it never resolves to the event's base. "No base" costs a
    /// three-way compare; a wrong base would corrupt one.
    #[test]
    fn a_task_sharing_a_uid_gets_its_own_entry_and_never_the_events_base() {
        let state_dir = tempfile::TempDir::new().unwrap();

        let mut event = test_event();
        event.uid = crate::event::EventUid::new("shared@u-crawler".to_string());
        let event_id = event.event_instance_id();
        let task_id = crate::EventInstanceId::todo(event.uid.clone(), None);

        let mut sync_bases = SyncBases::new();
        sync_bases
            .record([event.clone()], state_dir.path())
            .unwrap();

        // Before the task has ever synced: no base, and no wrong base.
        let loaded = SyncBases::load_from_state_dir(state_dir.path()).unwrap();
        assert_eq!(
            loaded.get(&event_id),
            Some(&Some(Box::new(CalendarItem::from(event.clone()))))
        );
        assert_eq!(loaded.get(&task_id), None);

        // Once the task has a base of its own, both entries coexist.
        let mut todo = crate::todo::Todo::new("Shared");
        todo.uid = event.uid.clone();
        let task = CalendarItem::from(todo);
        EventBases::write_from([&task], &state_dir.path().join(EVENT_BASES_DIR_NAME)).unwrap();

        let loaded = SyncBases::load_from_state_dir(state_dir.path()).unwrap();
        assert_eq!(
            loaded.get(&event_id),
            Some(&Some(Box::new(CalendarItem::from(event))))
        );
        assert_eq!(loaded.get(&task_id), Some(&Some(Box::new(task))));
    }

    #[test]
    fn record_leaves_other_base_files_untouched() {
        let state_dir = tempfile::TempDir::new().unwrap();
        let event_a = test_event();
        let mut event_b = test_event();
        event_b.uid = crate::event::EventUid::new("other@example.com".to_string());

        let mut sync_bases = SyncBases::new();
        sync_bases.record([event_a], state_dir.path()).unwrap();

        // Garble A's base on disk; recording B must not rewrite it.
        let bases_dir = state_dir.path().join(EVENT_BASES_DIR_NAME);
        let a_path = std::fs::read_dir(&bases_dir)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        std::fs::write(&a_path, "garbled").unwrap();

        sync_bases
            .record([event_b.clone()], state_dir.path())
            .unwrap();

        assert_eq!(std::fs::read_to_string(&a_path).unwrap(), "garbled");

        let loaded = SyncBases::load_from_state_dir(state_dir.path()).unwrap();
        assert_eq!(
            loaded.get(&event_b.event_instance_id()),
            Some(&Some(Box::new(CalendarItem::from(event_b))))
        );
    }

    #[test]
    fn forget_removes_the_known_id_and_event_base_after_reload() {
        let state_dir = tempfile::TempDir::new().unwrap();
        let event = test_event();
        let id = event.event_instance_id();

        let mut sync_bases = SyncBases::new();
        sync_bases.record([event], state_dir.path()).unwrap();

        sync_bases.forget([id.clone()], state_dir.path()).unwrap();

        let loaded = SyncBases::load_from_state_dir(state_dir.path()).unwrap();
        assert_eq!(loaded.get(&id), None);
        assert!(
            std::fs::read_dir(state_dir.path().join(EVENT_BASES_DIR_NAME))
                .unwrap()
                .next()
                .is_none()
        );
        assert!(
            std::fs::read_to_string(state_dir.path().join(KNOWN_IDS_FILE_NAME))
                .unwrap()
                .is_empty()
        );
    }
}
