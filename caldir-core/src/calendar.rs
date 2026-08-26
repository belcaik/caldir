mod config;
mod error;
mod event;
mod listing;
mod state;

use crate::event::{EventInstanceId, EventTime, EventUid, Recurrence, expand_in_range};
use crate::utils::slugify;
use crate::{CalendarItem, Event, RemoteConfig};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
pub use config::CalendarConfig;
pub use error::CalendarError;
pub use event::{CalendarEvent, CalendarEventError};
pub use listing::{CalendarListing, ExpandedItems, SkippedEvent};
pub use state::CalendarState;
pub(crate) use state::SyncBases;

const DOTDIR_NAME: &str = ".caldir";

// ~/caldir/my_calendar/.caldir/config.toml
const CONFIG_FILE_NAME: &str = "config.toml";

// ~/caldir/my_calendar/.caldir/state/known_event_ids
const STATE_DIR_NAME: &str = "state";

fn calendar_dotdir(calendar_path: &Path) -> PathBuf {
    calendar_path.join(DOTDIR_NAME)
}

fn calendar_config_path(calendar_path: &Path) -> PathBuf {
    calendar_dotdir(calendar_path).join(CONFIG_FILE_NAME)
}

fn calendar_state_dir(calendar_path: &Path) -> PathBuf {
    calendar_dotdir(calendar_path).join(STATE_DIR_NAME)
}

#[derive(Debug)]
pub struct Calendar {
    path: PathBuf,
    config: Option<CalendarConfig>,
    state: CalendarState,
}

impl Calendar {
    /// Create new calendar
    pub fn create(path: &Path, config: Option<CalendarConfig>) -> Result<Self, CalendarError> {
        // Error if path already exists:
        if path.exists() {
            return Err(CalendarError::AlreadyExists(path.to_path_buf()));
        }

        // create calendar directory and its .caldir/ subdirectory:
        std::fs::create_dir_all(calendar_dotdir(path))?;

        // Create calendar config file (if config is provided):
        if let Some(ref config) = config {
            let config_path = calendar_config_path(path);
            CalendarConfig::write(config, &config_path)?;
        }

        Ok(Self {
            path: path.to_path_buf(),
            config,
            state: CalendarState::new(),
        })
    }

    /// Load existing calendar
    pub fn load(path: &Path) -> Result<Self, CalendarError> {
        if !path.is_dir() {
            return Err(CalendarError::NotFound(path.to_path_buf()));
        }

        let config_path = calendar_config_path(path);
        let config = CalendarConfig::load_optional(&config_path)?;

        let state_dir = calendar_state_dir(path);
        let state = CalendarState::load(&state_dir)?;

        Ok(Self {
            path: path.to_path_buf(),
            config,
            state,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn config_path(&self) -> PathBuf {
        calendar_config_path(self.path())
    }

    pub fn state(&self) -> &CalendarState {
        &self.state
    }

    pub fn slug(&self) -> Option<&str> {
        self.path().file_name().and_then(|s| s.to_str())
    }

    /// Load all events in calendar.
    ///
    /// A file that fails to load is recorded as skipped rather than aborting
    /// the listing: one bad `.ics` must never hide its healthy siblings. Only
    /// directory-level failures are returned as `Err`.
    pub fn events(&self) -> Result<CalendarListing, CalendarError> {
        let mut listing = CalendarListing::default();

        for entry in std::fs::read_dir(self.path())? {
            let entry = entry?;
            let path = entry.path();

            if entry.file_type()?.is_file() && path.extension().is_some_and(|ext| ext == "ics") {
                match CalendarEvent::load(path.clone()) {
                    Ok(event) => listing.push_event(event),
                    Err(err) => listing.push_skipped(path, err.into()),
                }
            }
        }

        Ok(listing)
    }

    /// Load specific event in calendar
    pub fn event(&self, event_slug: &str) -> Result<CalendarEvent, CalendarError> {
        let event_path = self.path().join(format!("{}.ics", event_slug));
        let calendar_event = CalendarEvent::load(event_path)?;
        Ok(calendar_event)
    }

    pub fn event_by_instance_id(
        &self,
        id: &EventInstanceId,
    ) -> Result<Option<CalendarEvent>, CalendarError> {
        let found = self
            .events()?
            .into_events()
            .into_iter()
            .find(|ce| ce.item().instance_id() == *id);

        Ok(found)
    }

    /// Find the master event of a recurring series given its uid.
    /// i.e. a master event is one whose `recurrence` field is set.
    /// Instance overrides (with `recurrence_id` set) are not considered masters.
    pub fn master_event_for(&self, uid: &str) -> Result<Option<Event>, CalendarError> {
        let master = self
            .events()?
            .into_events()
            .into_iter()
            .find(|ce| {
                ce.event()
                    .is_some_and(|event| event.uid.as_str() == uid && event.recurrence.is_some())
            })
            .and_then(|ce| ce.event().cloned());
        Ok(master)
    }

    pub fn is_read_only(&self) -> bool {
        self.config
            .as_ref()
            .and_then(|c| c.read_only())
            .unwrap_or(false)
    }

    pub fn name(&self) -> Option<&str> {
        self.config.as_ref().and_then(|c| c.name())
    }

    pub fn color(&self) -> Option<&str> {
        self.config.as_ref().and_then(|c| c.color())
    }

    pub fn read_only_setting(&self) -> Option<bool> {
        self.config.as_ref().and_then(|c| c.read_only())
    }

    /// Split a recurring series at `split_start`:
    ///
    /// Original master:
    /// 1. RRULE is truncated to end strictly before `split_start`
    /// 2. Any EXDATEs at or after `split_start` are dropped
    /// 3. Any override files at or after `split_start` are deleted
    ///
    /// New master:
    /// 1. Starts at `split_start`
    /// 2. Inherit all other metadata (summary, description, location etc) from original
    /// 3. Gets a fresh UID and a reset SEQUENCE
    pub fn split_recurring_series_at(
        &self,
        master_uid: &EventUid,
        split_start: EventTime,
        split_end: EventTime,
        new_recurrence: Option<Recurrence>,
    ) -> Result<Event, CalendarError> {
        let mut all_events = self.events()?.into_events();

        let master_idx = all_events
            .iter()
            .position(|ce| {
                ce.event()
                    .is_some_and(|event| event.uid == *master_uid && event.recurrence.is_some())
            })
            .ok_or_else(|| CalendarError::MasterNotFound(master_uid.as_str().to_string()))?;

        let mut master_ce = all_events.swap_remove(master_idx);
        // Unreachable: `master_idx` was found through `as_event` above.
        let master_event = master_ce
            .event()
            .ok_or_else(|| CalendarError::MasterNotFound(master_uid.as_str().to_string()))?
            .clone();
        let master_recurrence = master_event
            .recurrence
            .as_ref()
            .ok_or_else(|| CalendarError::NotRecurring(master_uid.as_str().to_string()))?;

        let truncated_recurrence =
            master_recurrence.truncate_before(&master_event.start, &split_start);

        let truncated_master = Event {
            recurrence: Some(truncated_recurrence),
            last_modified: Some(Utc::now()),
            sequence: master_event.sequence + 1,
            ..master_event.clone()
        };
        master_ce.update(truncated_master)?;

        let new_master = Event {
            start: split_start.clone(),
            end: Some(split_end),
            recurrence: new_recurrence,
            recurrence_id: None,
            last_modified: Some(Utc::now()),
            sequence: 0,
            ..master_event.with_new_uid()
        };
        // `create_event` stores the item unchanged, so the written event is
        // `new_master` itself.
        self.create_event(new_master.clone())?;

        let split_start_utc = split_start.to_utc();
        for ce in all_events.into_iter() {
            let Some(event) = ce.event() else {
                continue;
            };
            if event.uid != *master_uid {
                continue;
            }
            let Some(rid) = event.recurrence_id.as_ref() else {
                continue;
            };
            let after_split = rid.as_event_time().to_utc() >= split_start_utc;
            if after_split {
                ce.delete()?;
            }
        }

        Ok(new_master)
    }

    /// Exclude a single occurrence from a recurring series
    /// i.e. "delete this instance only"
    pub fn delete_recurring_instance(&self, id: &EventInstanceId) -> Result<(), CalendarError> {
        let uid = id.uid();

        let Some(recurrence_id) = id.recurrence_id() else {
            return Err(CalendarError::NotRecurring(uid.as_str().to_string()));
        };

        let mut master = None;
        let mut override_event = None;

        // Find master event (+ potential override event)
        for ce in self.events()?.into_events() {
            let Some(event) = ce.event() else {
                continue;
            };
            if event.uid != *uid {
                continue;
            }

            let is_override_for_occurrence = event.event_instance_id() == *id;
            let is_master = event.recurrence.is_some();

            if is_master {
                master = Some(ce);
            } else if is_override_for_occurrence {
                override_event = Some(ce);
            }
        }

        if master.is_none() && override_event.is_none() {
            return Err(CalendarError::MasterNotFound(uid.as_str().to_string()));
        }

        // If override exists, delete it:
        if let Some(ce) = override_event {
            ce.delete()?;
        }

        // If a master exists, record the exclusion as an EXDATE.
        if let Some(mut ce) = master {
            ce.add_exdate(recurrence_id.as_event_time().clone())?;
        }

        Ok(())
    }

    /// Override a single occurrence of a recurring series
    pub fn update_recurring_instance(
        &self,
        id: &EventInstanceId,
        apply: impl FnOnce(&mut Event),
    ) -> Result<CalendarEvent, CalendarError> {
        let Some(recurrence_id) = id.recurrence_id() else {
            return Err(CalendarError::NotRecurring(id.uid().as_str().to_string()));
        };

        let existing = self.event_by_instance_id(id)?;

        match existing {
            // An override file already exists -> edit it in place
            Some(mut ce) => {
                // Unreachable for an event id: an event identity only ever
                // matches an event.
                // TODO: tasks need their own instance-override path.
                let Some(mut event) = ce.event().cloned() else {
                    return Err(CalendarError::MasterNotFound(id.uid().as_str().to_string()));
                };
                apply(&mut event);

                event.last_modified = Some(Utc::now());
                event.sequence += 1;
                ce.update(event)?;

                Ok(ce)
            }
            // No override exists yet -> synthesize one from the master
            None => {
                let master = self
                    .master_event_for(id.uid().as_str())?
                    .ok_or_else(|| CalendarError::MasterNotFound(id.uid().as_str().to_string()))?;

                let mut event = master.occurrence_at(recurrence_id.as_event_time().clone());

                event.sequence = 0;
                event.last_modified = Some(Utc::now());
                apply(&mut event);

                self.create_event(event)
            }
        }
    }

    /// List every item to show for `[from, to)`: events expanded into their
    /// occurrences, tasks as themselves.
    ///
    /// Tasks are **modelled, never expanded** — a recurring task yields
    /// its master and nothing else. They are still *windowed*, by
    /// `Todo::occurs_in_range`: the window is the query, and the
    /// `--json` renderer has no second filter behind it, so a task placed
    /// outside `[from, to)` would otherwise escape the range the caller asked
    /// for. An **undated** task is unconditionally in-window there — no date
    /// is ever invented to make the predicate work.
    pub fn expanded_events_in_range(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<Vec<CalendarItem>, CalendarError> {
        Ok(self.expanded_items_in_range(from, to)?.items)
    }

    /// `expanded_events_in_range`, plus the `.ics` files it could not read.
    ///
    /// A listing is never silently short, and a view that drops the
    /// skips is exactly that. The caller that shows the items to a user is the
    /// caller that must show the skips, so the two arrive together.
    pub fn expanded_items_in_range(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<ExpandedItems, CalendarError> {
        let (calendar_events, skipped) = self.events()?.into_parts();

        let mut events = Vec::new();
        let mut tasks = Vec::new();

        for calendar_event in calendar_events {
            match calendar_event.into_item() {
                CalendarItem::Event(event) => events.push(event),
                item => tasks.push(item),
            }
        }

        let mut items: Vec<CalendarItem> = expand_in_range(events, from, to)
            .into_iter()
            .map(CalendarItem::Event)
            .collect();

        items.extend(tasks.into_iter().filter(|item| match item {
            CalendarItem::Todo(todo) => todo.occurs_in_range(from, to),
            // Unreachable: `events` took every `Event` above. Kept in-window
            // rather than dropped, so a future item kind is never lost here.
            CalendarItem::Event(_) => true,
        }));

        Ok(ExpandedItems { items, skipped })
    }

    /// Create a new item in the calendar, of whichever supported kind.
    pub fn create_event(
        &self,
        item: impl Into<CalendarItem>,
    ) -> Result<CalendarEvent, CalendarError> {
        let calendar_event = CalendarEvent::create(self, item)?;
        Ok(calendar_event)
    }

    /// Delete event from calendar
    pub fn delete_event(&self, event_slug: &str) -> Result<(), CalendarError> {
        let event = self.event(event_slug)?;
        event.delete()?;
        Ok(())
    }

    pub fn remote_config(&self) -> Option<&RemoteConfig> {
        self.config.as_ref().and_then(|c| c.remote_config())
    }

    pub fn has_remote(&self) -> bool {
        self.remote_config().is_some()
    }

    pub fn config(&self) -> Option<&CalendarConfig> {
        self.config.as_ref()
    }

    pub fn remote_email(&self) -> Option<&str> {
        self.remote_config()
            .and_then(|remote_config| remote_config.account_identifier())
            .filter(|id| id.contains('@'))
    }

    pub(crate) fn record_sync_bases<I>(&mut self, items: I) -> Result<(), CalendarError>
    where
        I: IntoIterator,
        I::Item: Into<CalendarItem>,
    {
        self.state
            .record_sync_bases(items, &calendar_state_dir(&self.path))?;
        Ok(())
    }

    pub fn base_slug_for(name: Option<&str>) -> String {
        name.map(slugify).unwrap_or_else(|| "calendar".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{
        test_caldir, test_calendar, test_calendar_config, test_calendar_path, test_event,
    };

    #[test]
    fn create_creates_caldir_subdirectory() {
        let (_, path) = test_calendar_path();

        let calendar = Calendar::create(&path, None).unwrap();

        assert!(calendar.path().join(".caldir").is_dir());
    }

    #[test]
    fn create_without_config_does_not_create_config_file() {
        let (_, path) = test_calendar_path();

        let calendar = Calendar::create(&path, None).unwrap();

        assert!(!calendar.config_path().is_file());
    }

    #[test]
    fn create_with_config_writes_config_file() {
        let (_, path) = test_calendar_path();
        let config = test_calendar_config();

        let calendar = Calendar::create(&path, Some(config.clone())).unwrap();

        let expected_config_path = &path.join(".caldir").join("config.toml");

        // Config file is located in the right place:
        assert!(calendar.config_path().is_file());
        assert_eq!(&calendar.config_path(), expected_config_path);

        // with the right content:
        let loaded_config = CalendarConfig::load(&calendar.config_path()).unwrap();
        assert_eq!(loaded_config, config);
    }

    #[test]
    fn load_returns_existing_calendar() {
        let (_, path) = test_calendar_path();
        let result = Calendar::create(&path, None);

        assert!(result.is_ok());
        assert_eq!(result.unwrap().path(), path);
    }

    #[test]
    fn load_errors_when_directory_missing() {
        let (tmp, _caldir) = test_caldir();

        let result = Calendar::load(&tmp.path().join("missing"));

        assert!(matches!(result, Err(CalendarError::NotFound(_))));
    }

    #[test]
    fn load_errors_when_not_directory() {
        let (tmp, _caldir) = test_caldir();

        let file_path = tmp.path().join("not_a_directory");
        std::fs::write(&file_path, "I am a file, not a directory").unwrap();

        let result = Calendar::load(&file_path);

        assert!(matches!(result, Err(CalendarError::NotFound(p)) if p == file_path));
    }

    #[test]
    fn events_only_returns_events_from_current_calendar() {
        let (_tmp, caldir) = test_caldir();

        // 2 calendars in the same caldir data directory:
        let work = caldir.create_calendar("work", None).unwrap();
        let personal = caldir.create_calendar("personal", None).unwrap();

        work.create_event(test_event()).unwrap();
        work.create_event(test_event()).unwrap();
        personal.create_event(test_event()).unwrap();

        assert_eq!(work.events().unwrap().len(), 2);
        assert_eq!(personal.events().unwrap().len(), 1);
    }

    #[test]
    fn events_ignores_non_ics_files() {
        let (_tmp, calendar) = test_calendar();

        calendar.create_event(test_event()).unwrap();

        // Drop in stray files that other tools (e.g. vdirsyncer) might leave around.
        std::fs::write(calendar.path().join("color"), "#ff0000").unwrap();
        std::fs::write(calendar.path().join("displayname"), "Work").unwrap();
        std::fs::write(calendar.path().join("README.md"), "notes").unwrap();

        let events = calendar.events().unwrap();

        assert_eq!(events.len(), 1);
    }

    #[test]
    fn event_returns_event_by_slug() {
        let (_tmp, calendar) = test_calendar();
        let created = calendar.create_event(test_event()).unwrap();

        let found = calendar.event("2026-01-01T1200__test-event").unwrap();

        assert_eq!(found.path(), created.path());
    }

    #[test]
    fn event_errors_when_file_missing() {
        let (_tmp, calendar) = test_calendar();

        let result = calendar.event("does-not-exist");

        assert!(matches!(
            result,
            Err(CalendarError::Event(CalendarEventError::NotFound(_)))
        ));
    }

    #[test]
    fn delete_event_removes_file() {
        let (_tmp, calendar) = test_calendar();
        let cal_event = calendar.create_event(test_event()).unwrap();
        let path = cal_event.path().to_path_buf();
        assert!(path.is_file());

        calendar
            .delete_event("2026-01-01T1200__test-event")
            .unwrap();

        assert!(!path.exists());
    }

    use crate::event::{ComponentKind, RecurrenceId};
    use chrono::TimeZone;

    fn make_master(uid: &str, start: DateTime<Utc>, rrule: &str) -> Event {
        let mut event = Event::new("Daily standup", EventTime::DateTimeUtc(start));
        event.uid = crate::event::EventUid::new(uid);
        event.description = Some("Notes".to_string());
        event.location = Some("Office".to_string());
        event.end = Some(EventTime::DateTimeUtc(start + chrono::Duration::hours(1)));
        event.recurrence = Some(Recurrence::new(rrule));
        event
    }

    fn make_override(uid: &str, instance: DateTime<Utc>, summary: &str) -> Event {
        let mut event = Event::new(summary, EventTime::DateTimeUtc(instance));
        event.uid = crate::event::EventUid::new(uid);
        event.recurrence_id = Some(RecurrenceId::from_event_time(EventTime::DateTimeUtc(
            instance,
        )));
        event
    }

    fn loaded_master(cal: &Calendar, uid: &str) -> Event {
        cal.master_event_for(uid)
            .unwrap()
            .unwrap_or_else(|| panic!("master {uid} should exist"))
    }

    fn loaded_overrides(cal: &Calendar, uid: &str) -> Vec<Event> {
        cal.events()
            .unwrap()
            .into_events()
            .into_iter()
            .map(|ce| ce.event().unwrap().clone())
            .filter(|e| e.uid.as_str() == uid && e.recurrence_id.is_some())
            .collect()
    }

    fn t(year: i32, month: u32, day: u32, hour: u32, min: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(year, month, day, hour, min, 0)
            .unwrap()
    }

    /// The events of an expansion, for the event-only assertions below.
    /// Panics rather than filtering, so a stray item can never quietly shorten
    /// the list an assertion is checking.
    fn expanded_events(items: Vec<CalendarItem>) -> Vec<Event> {
        items
            .into_iter()
            .map(|item| match item {
                CalendarItem::Event(event) => event,
                other => panic!("expected an event, got {other:?}"),
            })
            .collect()
    }

    fn instance_id(uid: &str, occurrence: EventTime) -> EventInstanceId {
        EventInstanceId::new(
            EventUid::new(uid),
            Some(RecurrenceId::from_event_time(occurrence)),
        )
    }

    #[test]
    fn master_event_for_returns_master_when_present() {
        let (_tmp, calendar) = test_calendar();
        calendar
            .create_event(make_master(
                "series@caldir",
                t(2026, 4, 1, 10, 0),
                "FREQ=DAILY",
            ))
            .unwrap();

        let master = calendar.master_event_for("series@caldir").unwrap();

        assert!(master.is_some());
        assert_eq!(master.unwrap().uid.as_str(), "series@caldir");
    }

    #[test]
    fn master_event_for_returns_none_when_missing() {
        let (_tmp, calendar) = test_calendar();

        let master = calendar.master_event_for("nonexistent").unwrap();

        assert!(master.is_none());
    }

    #[test]
    fn master_event_for_ignores_non_recurring_events_with_same_uid() {
        let (_tmp, calendar) = test_calendar();

        let mut event = test_event();
        event.uid = crate::event::EventUid::new("series@caldir");
        calendar.create_event(event).unwrap();

        // Same uid, but no recurrence → not a master.
        let master = calendar.master_event_for("series@caldir").unwrap();

        assert!(master.is_none());
    }

    #[test]
    fn event_by_instance_id_finds_non_recurring_event() {
        let (_tmp, calendar) = test_calendar();
        let cal_event = calendar.create_event(test_event()).unwrap();
        let id = cal_event.event().unwrap().event_instance_id();

        let found = calendar.event_by_instance_id(&id).unwrap();

        assert!(found.is_some());
        assert_eq!(found.unwrap().path(), cal_event.path());
    }

    #[test]
    fn event_by_instance_id_finds_recurring_instance_override() {
        let (_tmp, calendar) = test_calendar();
        calendar
            .create_event(make_master(
                "series@caldir",
                t(2026, 4, 1, 10, 0),
                "FREQ=DAILY",
            ))
            .unwrap();
        let override_event =
            make_override("series@caldir", t(2026, 4, 3, 10, 0), "Special standup");
        let override_id = override_event.event_instance_id();
        calendar.create_event(override_event).unwrap();

        let found = calendar.event_by_instance_id(&override_id).unwrap();

        assert!(found.is_some());
        assert_eq!(
            found.unwrap().event().unwrap().summary.as_deref(),
            Some("Special standup")
        );
    }

    #[test]
    fn event_by_instance_id_returns_none_when_missing() {
        let (_tmp, calendar) = test_calendar();
        let id = test_event().event_instance_id();

        let found = calendar.event_by_instance_id(&id).unwrap();

        assert!(found.is_none());
    }

    #[test]
    fn is_read_only_false_when_no_config() {
        let (_tmp, calendar) = test_calendar();

        assert!(!calendar.is_read_only());
    }

    #[test]
    fn is_read_only_false_when_config_has_no_read_only_field() {
        let (_tmp, caldir) = test_caldir();
        let config = CalendarConfig::new(Some("Test".to_string()), None, None, None);
        let calendar = caldir.create_calendar("test", Some(config)).unwrap();

        assert!(!calendar.is_read_only());
    }

    #[test]
    fn is_read_only_true_when_config_says_true() {
        let (_tmp, caldir) = test_caldir();
        let config = CalendarConfig::new(Some("Test".to_string()), None, Some(true), None);
        let calendar = caldir.create_calendar("test", Some(config)).unwrap();

        assert!(calendar.is_read_only());
    }

    #[test]
    fn is_read_only_false_when_config_says_false() {
        let (_tmp, caldir) = test_caldir();
        let config = CalendarConfig::new(Some("Test".to_string()), None, Some(false), None);
        let calendar = caldir.create_calendar("test", Some(config)).unwrap();

        assert!(!calendar.is_read_only());
    }

    #[test]
    fn split_truncates_master_rrule_before_split() {
        let (_tmp, cal) = test_calendar();
        let uid = "master@test";
        cal.create_event(make_master(uid, t(2026, 4, 1, 10, 0), "FREQ=DAILY"))
            .unwrap();

        let split_start = EventTime::DateTimeUtc(t(2026, 4, 5, 10, 0));
        let split_end = EventTime::DateTimeUtc(t(2026, 4, 5, 11, 0));
        cal.split_recurring_series_at(&EventUid::new(uid), split_start, split_end, None)
            .unwrap();

        let master = loaded_master(&cal, uid);
        let rrule = &master.recurrence.as_ref().unwrap().rrule;
        // UNTIL is one second before split_start, in UTC form.
        assert_eq!(rrule, "FREQ=DAILY;UNTIL=20260405T095959Z");
    }

    #[test]
    fn split_creates_new_master_with_fresh_uid_and_inherited_metadata() {
        let (_tmp, cal) = test_calendar();
        let uid = "master@test";
        cal.create_event(make_master(uid, t(2026, 4, 1, 10, 0), "FREQ=DAILY"))
            .unwrap();

        let new_recurrence = Some(Recurrence::new("FREQ=WEEKLY"));
        let new_master = cal
            .split_recurring_series_at(
                &EventUid::new(uid),
                EventTime::DateTimeUtc(t(2026, 4, 5, 10, 0)),
                EventTime::DateTimeUtc(t(2026, 4, 5, 11, 30)),
                new_recurrence,
            )
            .unwrap();

        // Fresh UID, not the master's.
        assert_ne!(new_master.uid.as_str(), uid);
        // Inherits metadata.
        assert_eq!(new_master.summary.as_deref(), Some("Daily standup"));
        assert_eq!(new_master.description.as_deref(), Some("Notes"));
        assert_eq!(new_master.location.as_deref(), Some("Office"));
        // Uses the new start/end and recurrence.
        assert_eq!(
            new_master.start,
            EventTime::DateTimeUtc(t(2026, 4, 5, 10, 0))
        );
        assert_eq!(
            new_master.end,
            Some(EventTime::DateTimeUtc(t(2026, 4, 5, 11, 30)))
        );
        assert_eq!(new_master.recurrence.as_ref().unwrap().rrule, "FREQ=WEEKLY");
        assert!(new_master.recurrence_id.is_none());
    }

    #[test]
    fn split_bumps_master_sequence() {
        let (_tmp, cal) = test_calendar();
        let uid = "master@test";
        let mut master = make_master(uid, t(2026, 4, 1, 10, 0), "FREQ=DAILY");
        master.sequence = 3;
        cal.create_event(master).unwrap();

        cal.split_recurring_series_at(
            &EventUid::new(uid),
            EventTime::DateTimeUtc(t(2026, 4, 5, 10, 0)),
            EventTime::DateTimeUtc(t(2026, 4, 5, 11, 0)),
            None,
        )
        .unwrap();

        assert_eq!(loaded_master(&cal, uid).sequence, 4);
    }

    #[test]
    fn split_drops_overrides_at_or_after_split_keeps_earlier_ones() {
        let (_tmp, cal) = test_calendar();
        let uid = "master@test";
        cal.create_event(make_master(uid, t(2026, 4, 1, 10, 0), "FREQ=DAILY"))
            .unwrap();

        cal.create_event(make_override(uid, t(2026, 4, 3, 10, 0), "before"))
            .unwrap();
        cal.create_event(make_override(uid, t(2026, 4, 5, 10, 0), "at-split"))
            .unwrap();
        cal.create_event(make_override(uid, t(2026, 4, 7, 10, 0), "after"))
            .unwrap();

        cal.split_recurring_series_at(
            &EventUid::new(uid),
            EventTime::DateTimeUtc(t(2026, 4, 5, 10, 0)),
            EventTime::DateTimeUtc(t(2026, 4, 5, 11, 0)),
            None,
        )
        .unwrap();

        let overrides = loaded_overrides(&cal, uid);
        assert_eq!(
            overrides.len(),
            1,
            "only the pre-split override should remain"
        );
        assert_eq!(overrides[0].summary.as_deref(), Some("before"));
    }

    #[test]
    fn split_drops_exdates_at_or_after_split() {
        let (_tmp, cal) = test_calendar();
        let uid = "master@test";
        let mut master = make_master(uid, t(2026, 4, 1, 10, 0), "FREQ=DAILY");
        let kept = EventTime::DateTimeUtc(t(2026, 4, 3, 10, 0));
        let dropped_at = EventTime::DateTimeUtc(t(2026, 4, 5, 10, 0));
        let dropped_after = EventTime::DateTimeUtc(t(2026, 4, 6, 10, 0));
        master.recurrence = Some(Recurrence {
            rrule: "FREQ=DAILY".into(),
            exdates: vec![kept.clone(), dropped_at, dropped_after],
            rdates: vec![],
        });
        cal.create_event(master).unwrap();

        cal.split_recurring_series_at(
            &EventUid::new(uid),
            EventTime::DateTimeUtc(t(2026, 4, 5, 10, 0)),
            EventTime::DateTimeUtc(t(2026, 4, 5, 11, 0)),
            None,
        )
        .unwrap();

        assert_eq!(
            loaded_master(&cal, uid).recurrence.unwrap().exdates,
            vec![kept]
        );
    }

    #[test]
    fn split_with_no_new_recurrence_creates_single_event() {
        let (_tmp, cal) = test_calendar();
        let uid = "master@test";
        cal.create_event(make_master(uid, t(2026, 4, 1, 10, 0), "FREQ=DAILY"))
            .unwrap();

        let new_master = cal
            .split_recurring_series_at(
                &EventUid::new(uid),
                EventTime::DateTimeUtc(t(2026, 4, 5, 10, 0)),
                EventTime::DateTimeUtc(t(2026, 4, 5, 11, 0)),
                None,
            )
            .unwrap();

        assert!(new_master.recurrence.is_none());
    }

    #[test]
    fn split_errors_when_master_not_found() {
        let (_tmp, cal) = test_calendar();

        let err = cal
            .split_recurring_series_at(
                &EventUid::new("nonexistent"),
                EventTime::DateTimeUtc(t(2026, 4, 5, 10, 0)),
                EventTime::DateTimeUtc(t(2026, 4, 5, 11, 0)),
                None,
            )
            .unwrap_err();

        assert!(matches!(err, CalendarError::MasterNotFound(_)));
    }

    #[test]
    fn split_errors_when_event_is_not_recurring() {
        let (_tmp, cal) = test_calendar();
        // Non-recurring event with the target uid. Since master_event_for filters
        // by recurrence.is_some(), this looks like the master "isn't there" —
        // surface as MasterNotFound, which is the right user-facing message.
        let mut single = test_event();
        single.uid = crate::event::EventUid::new("solo@test");
        cal.create_event(single).unwrap();

        let err = cal
            .split_recurring_series_at(
                &EventUid::new("solo@test"),
                EventTime::DateTimeUtc(t(2026, 4, 5, 10, 0)),
                EventTime::DateTimeUtc(t(2026, 4, 5, 11, 0)),
                None,
            )
            .unwrap_err();

        assert!(matches!(err, CalendarError::MasterNotFound(_)));
    }

    #[test]
    fn delete_recurring_instance_adds_exdate_to_master() {
        let (_tmp, cal) = test_calendar();
        let uid = "series@caldir";
        cal.create_event(make_master(uid, t(2026, 4, 1, 10, 0), "FREQ=DAILY"))
            .unwrap();

        let occurrence = EventTime::DateTimeUtc(t(2026, 4, 3, 10, 0));
        cal.delete_recurring_instance(&instance_id(uid, occurrence.clone()))
            .unwrap();

        assert_eq!(
            loaded_master(&cal, uid).recurrence.unwrap().exdates,
            vec![occurrence]
        );
    }

    #[test]
    fn delete_recurring_instance_removes_it_from_expansion() {
        let (_tmp, cal) = test_calendar();
        let uid = "series@caldir";
        cal.create_event(make_master(uid, t(2026, 4, 1, 10, 0), "FREQ=DAILY"))
            .unwrap();

        let occurrence = EventTime::DateTimeUtc(t(2026, 4, 3, 10, 0));
        cal.delete_recurring_instance(&instance_id(uid, occurrence.clone()))
            .unwrap();

        let starts: Vec<_> = expanded_events(
            cal.expanded_events_in_range(t(2026, 4, 1, 0, 0), t(2026, 4, 5, 0, 0))
                .unwrap(),
        )
        .into_iter()
        .map(|e| e.start.to_utc())
        .collect();
        assert_eq!(
            starts,
            vec![
                t(2026, 4, 1, 10, 0),
                t(2026, 4, 2, 10, 0),
                t(2026, 4, 4, 10, 0)
            ]
        );
    }

    #[test]
    fn delete_recurring_instance_deletes_materialized_override() {
        let (_tmp, cal) = test_calendar();
        let uid = "series@caldir";
        cal.create_event(make_master(uid, t(2026, 4, 1, 10, 0), "FREQ=DAILY"))
            .unwrap();
        // A previously moved/edited instance for 04-03 sits on disk as an override.
        cal.create_event(make_override(uid, t(2026, 4, 3, 10, 0), "moved standup"))
            .unwrap();

        cal.delete_recurring_instance(&instance_id(
            uid,
            EventTime::DateTimeUtc(t(2026, 4, 3, 10, 0)),
        ))
        .unwrap();

        // Override file is gone, and the base occurrence is excluded from the master.
        assert!(loaded_overrides(&cal, uid).is_empty());
        assert_eq!(
            loaded_master(&cal, uid).recurrence.unwrap().exdates,
            vec![EventTime::DateTimeUtc(t(2026, 4, 3, 10, 0))]
        );
    }

    #[test]
    fn delete_recurring_instance_bumps_sequence() {
        let (_tmp, cal) = test_calendar();
        let uid = "series@caldir";
        let mut master = make_master(uid, t(2026, 4, 1, 10, 0), "FREQ=DAILY");
        master.sequence = 3;
        cal.create_event(master).unwrap();

        cal.delete_recurring_instance(&instance_id(
            uid,
            EventTime::DateTimeUtc(t(2026, 4, 3, 10, 0)),
        ))
        .unwrap();

        assert_eq!(loaded_master(&cal, uid).sequence, 4);
    }

    #[test]
    fn delete_recurring_instance_is_idempotent() {
        let (_tmp, cal) = test_calendar();
        let uid = "series@caldir";
        cal.create_event(make_master(uid, t(2026, 4, 1, 10, 0), "FREQ=DAILY"))
            .unwrap();

        let occurrence = EventTime::DateTimeUtc(t(2026, 4, 3, 10, 0));
        cal.delete_recurring_instance(&instance_id(uid, occurrence.clone()))
            .unwrap();
        cal.delete_recurring_instance(&instance_id(uid, occurrence.clone()))
            .unwrap();

        let master = loaded_master(&cal, uid);
        // No duplicate EXDATE, and the second call didn't bump SEQUENCE again.
        assert_eq!(master.recurrence.unwrap().exdates, vec![occurrence]);
        assert_eq!(master.sequence, 1);
    }

    #[test]
    fn delete_recurring_instance_succeeds_for_orphan_override_without_master() {
        let (_tmp, cal) = test_calendar();
        let uid = "series@caldir";
        // Override with no master in the calendar.
        cal.create_event(make_override(uid, t(2026, 4, 3, 10, 0), "orphan"))
            .unwrap();

        cal.delete_recurring_instance(&instance_id(
            uid,
            EventTime::DateTimeUtc(t(2026, 4, 3, 10, 0)),
        ))
        .unwrap();

        assert!(loaded_overrides(&cal, uid).is_empty());
    }

    #[test]
    fn delete_recurring_instance_errors_when_uid_missing() {
        let (_tmp, cal) = test_calendar();

        let err = cal
            .delete_recurring_instance(&instance_id(
                "nonexistent",
                EventTime::DateTimeUtc(t(2026, 4, 3, 10, 0)),
            ))
            .unwrap_err();

        assert!(matches!(err, CalendarError::MasterNotFound(_)));
    }

    #[test]
    fn update_recurring_instance_creates_override_from_master() {
        let (_tmp, cal) = test_calendar();
        let uid = "series@caldir";
        cal.create_event(make_master(uid, t(2026, 4, 1, 10, 0), "FREQ=DAILY"))
            .unwrap();

        let occurrence = EventTime::DateTimeUtc(t(2026, 4, 3, 10, 0));
        cal.update_recurring_instance(&instance_id(uid, occurrence.clone()), |event| {
            event.summary = Some("Moved standup".to_string());
            event.start = EventTime::DateTimeUtc(t(2026, 4, 3, 14, 0));
            event.end = Some(EventTime::DateTimeUtc(t(2026, 4, 3, 15, 0)));
        })
        .unwrap();

        let overrides = loaded_overrides(&cal, uid);
        assert_eq!(overrides.len(), 1);
        let override_event = &overrides[0];
        // Carries the occurrence's recurrence id, not its own RRULE.
        assert!(override_event.recurrence.is_none());
        assert_eq!(
            override_event
                .recurrence_id
                .as_ref()
                .unwrap()
                .as_event_time(),
            &occurrence
        );
        // Edited fields applied.
        assert_eq!(override_event.summary.as_deref(), Some("Moved standup"));
        assert_eq!(
            override_event.start,
            EventTime::DateTimeUtc(t(2026, 4, 3, 14, 0))
        );
        // Metadata inherited from the master.
        assert_eq!(override_event.location.as_deref(), Some("Office"));
        // Master is untouched.
        assert!(
            loaded_master(&cal, uid)
                .recurrence
                .unwrap()
                .exdates
                .is_empty()
        );
    }

    #[test]
    fn update_recurring_instance_overrides_only_that_occurrence_in_expansion() {
        let (_tmp, cal) = test_calendar();
        let uid = "series@caldir";
        cal.create_event(make_master(uid, t(2026, 4, 1, 10, 0), "FREQ=DAILY"))
            .unwrap();

        cal.update_recurring_instance(
            &instance_id(uid, EventTime::DateTimeUtc(t(2026, 4, 3, 10, 0))),
            |event| event.summary = Some("Special".to_string()),
        )
        .unwrap();

        let summaries: Vec<_> = expanded_events(
            cal.expanded_events_in_range(t(2026, 4, 1, 0, 0), t(2026, 4, 5, 0, 0))
                .unwrap(),
        )
        .into_iter()
        .map(|e| e.summary.unwrap_or_default())
        .collect();
        assert_eq!(
            summaries,
            vec!["Daily standup", "Daily standup", "Special", "Daily standup"]
        );
    }

    #[test]
    fn update_recurring_instance_edits_existing_override_in_place() {
        let (_tmp, cal) = test_calendar();
        let uid = "series@caldir";
        cal.create_event(make_master(uid, t(2026, 4, 1, 10, 0), "FREQ=DAILY"))
            .unwrap();
        cal.create_event(make_override(uid, t(2026, 4, 3, 10, 0), "first edit"))
            .unwrap();

        cal.update_recurring_instance(
            &instance_id(uid, EventTime::DateTimeUtc(t(2026, 4, 3, 10, 0))),
            |event| event.summary = Some("second edit".to_string()),
        )
        .unwrap();

        // Still a single override (updated, not duplicated).
        let overrides = loaded_overrides(&cal, uid);
        assert_eq!(overrides.len(), 1);
        assert_eq!(overrides[0].summary.as_deref(), Some("second edit"));
    }

    #[test]
    fn update_recurring_instance_bumps_existing_override_sequence() {
        let (_tmp, cal) = test_calendar();
        let uid = "series@caldir";
        cal.create_event(make_master(uid, t(2026, 4, 1, 10, 0), "FREQ=DAILY"))
            .unwrap();
        let mut override_event = make_override(uid, t(2026, 4, 3, 10, 0), "edit");
        override_event.sequence = 2;
        cal.create_event(override_event).unwrap();

        cal.update_recurring_instance(
            &instance_id(uid, EventTime::DateTimeUtc(t(2026, 4, 3, 10, 0))),
            |event| event.summary = Some("re-edit".to_string()),
        )
        .unwrap();

        assert_eq!(loaded_overrides(&cal, uid)[0].sequence, 3);
    }

    #[test]
    fn update_recurring_instance_errors_when_not_recurring() {
        let (_tmp, cal) = test_calendar();

        let err = cal
            .update_recurring_instance(
                &EventInstanceId::new(EventUid::new("solo@test"), None),
                |_| {},
            )
            .unwrap_err();

        assert!(matches!(err, CalendarError::NotRecurring(_)));
    }

    #[test]
    fn update_recurring_instance_errors_when_master_missing() {
        let (_tmp, cal) = test_calendar();

        let err = cal
            .update_recurring_instance(
                &instance_id("nonexistent", EventTime::DateTimeUtc(t(2026, 4, 3, 10, 0))),
                |_| {},
            )
            .unwrap_err();

        assert!(matches!(err, CalendarError::MasterNotFound(_)));
    }

    // --- VTODO support (RED regression tests) ---

    /// Blast radius within one calendar: a single VTODO file must not take the
    /// sibling VEVENT files down with it (`?` at caldir-core/src/calendar.rs:114).
    #[test]
    fn events_lists_a_vtodo_file_alongside_its_vevent_sibling() {
        use crate::test_utils::{VEVENT_CONTROL_ICS, VTODO_ICS, write_ics};

        let (_tmp, calendar) = test_calendar();
        write_ics(calendar.path(), "window-123.ics", VEVENT_CONTROL_ICS);
        write_ics(calendar.path(), "assignment-123.ics", VTODO_ICS);

        let events = match calendar.events() {
            Ok(listing) => listing.into_events(),
            Err(err) => panic!(
                "one VTODO file aborted the whole calendar listing, including the \
                 VEVENT sibling: {err:?}"
            ),
        };

        let mut uids: Vec<&str> = events.iter().map(|ce| ce.item().uid().as_str()).collect();
        uids.sort_unstable();

        assert_eq!(
            uids,
            vec!["assignment-123@u-crawler", "window-123@u-crawler"]
        );
    }

    /// Graceful per-file degradation: an unloadable `.ics` file is skipped and
    /// reported, never fatal, and never silently dropped from the listing.
    #[test]
    fn an_unreadable_file_is_skipped_and_reported_not_fatal() {
        use crate::test_utils::{VEVENT_CONTROL_ICS, write_ics};

        let (_tmp, calendar) = test_calendar();
        write_ics(calendar.path(), "window-123.ics", VEVENT_CONTROL_ICS);
        let broken = write_ics(calendar.path(), "broken.ics", "this is not ICS at all\r\n");

        let listing = calendar
            .events()
            .expect("one unreadable file must not abort the whole calendar listing");

        let uids: Vec<&str> = listing
            .events()
            .iter()
            .map(|ce| ce.event().unwrap().uid.as_str())
            .collect();
        assert_eq!(uids, vec!["window-123@u-crawler"]);

        assert!(listing.is_incomplete());
        assert_eq!(listing.skipped().len(), 1);
        assert_eq!(listing.skipped()[0].path(), broken);
    }

    /// Control: a calendar holding only VEVENT files still lists them.
    #[test]
    fn events_lists_the_vevent_control_file() {
        use crate::test_utils::{VEVENT_CONTROL_ICS, write_ics};

        let (_tmp, calendar) = test_calendar();
        write_ics(calendar.path(), "window-123.ics", VEVENT_CONTROL_ICS);

        let events = calendar.events().unwrap().into_events();

        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].event().unwrap().uid.as_str(),
            "window-123@u-crawler"
        );
    }

    /// A task file is addressable by slug and by identity, like any other
    /// item. The kind is part of the key, so an event id with the same UID
    /// must not resolve to the task (R3, R4).
    #[test]
    fn event_and_event_by_instance_id_resolve_a_task_file() {
        use crate::test_utils::{VTODO_ICS, write_ics};

        let (_tmp, calendar) = test_calendar();
        write_ics(calendar.path(), "assignment-123.ics", VTODO_ICS);

        let by_slug = calendar.event("assignment-123").unwrap();
        assert!(matches!(by_slug.item(), CalendarItem::Todo(_)));

        let uid = EventUid::new("assignment-123@u-crawler");
        let found = calendar
            .event_by_instance_id(&EventInstanceId::todo(uid.clone(), None))
            .unwrap()
            .expect("the task is addressable by its own instance id");
        assert_eq!(found.item().uid().as_str(), "assignment-123@u-crawler");

        assert!(
            calendar
                .event_by_instance_id(&EventInstanceId::new(uid, None))
                .unwrap()
                .is_none(),
            "an event identity must not resolve to a same-UID task"
        );
    }

    /// The `RECURRENCE-ID` half of a task's identity is part of the key too: a
    /// task override and its master share a UID and must stay distinguishable.
    #[test]
    fn a_task_override_resolves_separately_from_its_master() {
        use crate::test_utils::write_ics;

        let master = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VTODO\r\nUID:weekly@u-crawler\r\nDUE:20260401T235900Z\r\nRRULE:FREQ=WEEKLY\r\n\
SUMMARY:Weekly reading\r\nEND:VTODO\r\nEND:VCALENDAR\r\n";
        let override_task = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VTODO\r\nUID:weekly@u-crawler\r\nRECURRENCE-ID:20260408T235900Z\r\n\
DUE:20260409T235900Z\r\nSUMMARY:Weekly reading (moved)\r\nEND:VTODO\r\nEND:VCALENDAR\r\n";

        let (_tmp, calendar) = test_calendar();
        write_ics(calendar.path(), "weekly.ics", master);
        write_ics(calendar.path(), "weekly-override.ics", override_task);

        let uid = EventUid::new("weekly@u-crawler");
        let rid = RecurrenceId::from_event_time(EventTime::DateTimeUtc(t(2026, 4, 8, 23, 59)));

        let found = calendar
            .event_by_instance_id(&EventInstanceId::todo(uid.clone(), Some(rid)))
            .unwrap()
            .expect("the task override is addressable by its own recurrence id");
        assert_eq!(found.item().summary(), Some("Weekly reading (moved)"));

        let master = calendar
            .event_by_instance_id(&EventInstanceId::todo(uid, None))
            .unwrap()
            .expect("the master is addressable by the un-instanced id");
        assert_eq!(master.item().summary(), Some("Weekly reading"));
    }

    /// R4/R100: a VEVENT and a VTODO may legitimately share a UID inside one
    /// directory. Supported, non-error local state — two items, nothing
    /// skipped.
    #[test]
    fn a_same_uid_event_and_task_pair_lists_two_items() {
        use crate::test_utils::{VTODO_ICS, write_ics};

        let same_uid_event = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VEVENT\r\nUID:assignment-123@u-crawler\r\nDTSTART:20260815T100000Z\r\n\
SUMMARY:Assignment 3 - submission window\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

        let (_tmp, calendar) = test_calendar();
        write_ics(calendar.path(), "assignment-123.ics", VTODO_ICS);
        write_ics(calendar.path(), "window-123.ics", same_uid_event);

        let listing = calendar.events().unwrap();

        assert_eq!(listing.len(), 2);
        assert!(!listing.is_incomplete());

        let mut kinds: Vec<ComponentKind> = listing
            .events()
            .iter()
            .map(|ce| ce.item().instance_id().kind())
            .collect();
        kinds.sort_by_key(|kind| format!("{kind:?}"));
        assert_eq!(kinds, vec![ComponentKind::Event, ComponentKind::Todo]);
    }

    /// R89: a task's RRULE is modelled, never expanded. Range expansion yields
    /// the master and nothing else — contrast the recurring *event* above,
    /// which yields four occurrences over the same window.
    #[test]
    fn a_recurring_task_yields_only_its_master_from_range_expansion() {
        use crate::test_utils::write_ics;

        let recurring_task = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VTODO\r\nUID:weekly@u-crawler\r\nDUE:20260401T235900Z\r\n\
RRULE:FREQ=DAILY\r\nSUMMARY:Weekly reading\r\nEND:VTODO\r\nEND:VCALENDAR\r\n";

        let (_tmp, cal) = test_calendar();
        write_ics(cal.path(), "weekly.ics", recurring_task);

        let items = cal
            .expanded_events_in_range(t(2026, 4, 1, 0, 0), t(2026, 4, 5, 0, 0))
            .unwrap();

        assert_eq!(items.len(), 1);
        match &items[0] {
            CalendarItem::Todo(todo) => {
                assert_eq!(todo.uid.as_str(), "weekly@u-crawler");
                assert!(todo.recurrence.is_some(), "the RRULE must be preserved");
                assert!(
                    todo.recurrence_id.is_none(),
                    "the master is not an instance"
                );
            }
            other => panic!("expected the task master, got {other:?}"),
        }
    }

    /// R46 (INVIOLABLE): an undated task is never given a fabricated date to
    /// make it fit the range, and it is never dropped for lacking one.
    #[test]
    fn an_undated_task_survives_range_expansion_with_no_date_invented() {
        use crate::test_utils::write_ics;

        let undated_task = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VTODO\r\nUID:someday@u-crawler\r\nSUMMARY:Read the RFC\r\n\
END:VTODO\r\nEND:VCALENDAR\r\n";

        let (_tmp, cal) = test_calendar();
        write_ics(cal.path(), "someday.ics", undated_task);

        let items = cal
            .expanded_events_in_range(t(2026, 4, 1, 0, 0), t(2026, 4, 5, 0, 0))
            .unwrap();

        assert_eq!(items.len(), 1);
        match &items[0] {
            CalendarItem::Todo(todo) => {
                assert_eq!(todo.summary.as_deref(), Some("Read the RFC"));
                assert!(todo.start.is_none(), "DTSTART must not be synthesized");
                assert!(todo.due.is_none(), "DUE must not be synthesized");
            }
            other => panic!("expected the undated task, got {other:?}"),
        }
    }

    /// F-1 (ADR-10 D5). The window is the query, and `--json` has no second
    /// filter behind it, so a task whose `DUE` lies outside `[from, to)` must
    /// not be listed. The undated task in the same calendar is the control:
    /// R46 makes it unconditionally in-window, and applying a window to tasks
    /// must never be implemented by inventing a date for it.
    #[test]
    fn a_dated_task_outside_the_window_is_not_listed_but_an_undated_one_is() {
        use crate::test_utils::{VTODO_ICS, write_ics};

        let undated_task = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VTODO\r\nUID:someday@u-crawler\r\nSUMMARY:Read the RFC\r\n\
END:VTODO\r\nEND:VCALENDAR\r\n";

        let (_tmp, cal) = test_calendar();
        // DUE 2026-08-15, five months past the window below.
        write_ics(cal.path(), "assignment-123.ics", VTODO_ICS);
        write_ics(cal.path(), "someday.ics", undated_task);

        let items = cal
            .expanded_events_in_range(t(2026, 4, 1, 0, 0), t(2026, 4, 5, 0, 0))
            .unwrap();

        let summaries: Vec<&str> = items.iter().filter_map(|i| i.summary()).collect();
        assert_eq!(
            summaries,
            vec!["Read the RFC"],
            "the August task escaped an April window"
        );
    }

    /// Control (R1/R2): the expansion of an event-only calendar is unchanged —
    /// a recurring master still yields one occurrence per day in the window,
    /// and a task in the same calendar does not disturb them.
    ///
    /// The task's `DUE` sits inside the queried window on purpose: since F-1 a
    /// task outside it is not listed at all, which is a different property
    /// (pinned above) and would hide the one this test is for.
    #[test]
    fn task_presence_does_not_change_event_expansion() {
        use crate::test_utils::{VTODO_ICS, write_ics};

        let (_tmp, cal) = test_calendar();
        cal.create_event(make_master(
            "series@caldir",
            t(2026, 4, 1, 10, 0),
            "FREQ=DAILY",
        ))
        .unwrap();
        let in_window_task = VTODO_ICS.replace("DUE:20260815T235900Z", "DUE:20260402T235900Z");
        write_ics(cal.path(), "assignment-123.ics", &in_window_task);

        let items = cal
            .expanded_events_in_range(t(2026, 4, 1, 0, 0), t(2026, 4, 4, 0, 0))
            .unwrap();

        let starts: Vec<DateTime<Utc>> = items
            .iter()
            .filter_map(|item| match item {
                CalendarItem::Event(event) => Some(event.start.to_utc()),
                _ => None,
            })
            .collect();

        assert_eq!(
            starts,
            vec![
                t(2026, 4, 1, 10, 0),
                t(2026, 4, 2, 10, 0),
                t(2026, 4, 3, 10, 0)
            ]
        );
        assert_eq!(items.len(), 4, "the task is listed, never expanded");
        assert!(
            matches!(items[3], CalendarItem::Todo(_)),
            "tasks are appended after the expanded events, so a consumer that \
             narrows to events never truncates the list it is walking"
        );
    }
}
