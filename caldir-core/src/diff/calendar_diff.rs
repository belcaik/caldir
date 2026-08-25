use std::collections::{HashMap, HashSet};

use super::event_change::EventChange;
use crate::calendar::{CalendarListing, SyncBases};
use crate::event::Status;
use crate::{CalendarEvent, DateRange, RemoteEvent};

pub struct CalendarDiff {
    outgoing: Vec<EventChange>,
    incoming: Vec<EventChange>,
}

impl CalendarDiff {
    /// Diff a local listing against the remote. R48: a listing that skipped an
    /// unreadable file is known-incomplete, so no local absence proves a delete
    /// and every delete for this calendar is suppressed, both directions.
    pub(crate) fn compute_from_listing(
        local_listing: CalendarListing,
        remote_events: Vec<RemoteEvent>,
        sync_bases: &SyncBases,
        range: &DateRange,
    ) -> Self {
        let deletes = if local_listing.is_incomplete() {
            Deletes::Suppressed
        } else {
            Deletes::Allowed
        };

        Self::compute_with(
            local_listing.into_events(),
            remote_events,
            sync_bases,
            range,
            deletes,
        )
    }

    fn compute_with(
        local_events: Vec<CalendarEvent>,
        remote_events: Vec<RemoteEvent>,
        sync_bases: &SyncBases,
        range: &DateRange,
        deletes: Deletes,
    ) -> Self {
        let local_event_ids: HashSet<_> = local_events
            .iter()
            // VEVENT-only: this set is compared against remote *event* ids, and
            // pass 2 below only ever looks up an event identity.
            .filter_map(|e| Some(e.event()?.event_instance_id()))
            .collect();

        let remote_by_id: HashMap<_, &RemoteEvent> = remote_events
            .iter()
            .map(|e| (e.event().event_instance_id(), e))
            .collect();

        let mut outgoing = Vec::new();
        let mut incoming = Vec::new();

        for local_event in &local_events {
            // Narrowing, not a filter: a task simply produces no change here.
            // It is never treated as absent-and-therefore-deleted, because the
            // delete branches below are inside this same `Some` arm.
            let Some(event) = local_event.event() else {
                continue;
            };
            let id = event.event_instance_id();

            // In both local and remote: skip if equal, otherwise update
            if let Some(remote_event) = remote_by_id.get(&id) {
                if event == remote_event.event() {
                    continue;
                }

                // Both cancelled: treat as in-sync regardless of field drift.
                // Cancelled events are historical; we don't churn syncing them.
                if event.status == Status::Cancelled
                    && remote_event.event().status == Status::Cancelled
                {
                    continue;
                }

                // Never push an unspecified (None) visibility — inherit the remote's.
                let mut to_push = event.clone();
                if to_push.visibility.is_none() {
                    to_push.visibility = remote_event.event().visibility;
                }

                let base = sync_bases.get(&id).and_then(Option::as_deref);
                let direction = if &to_push == remote_event.event() {
                    Some(UpdateDirection::Pull)
                } else {
                    update_direction(local_event, event, remote_event, base)
                };

                match direction {
                    Some(UpdateDirection::Push) => outgoing.push(EventChange::Update {
                        from: remote_event.event().clone(),
                        to: to_push,
                    }),
                    Some(UpdateDirection::Pull) => incoming.push(EventChange::Update {
                        from: event.clone(),
                        to: remote_event.event().clone(),
                    }),
                    None => {}
                }
                continue;
            }

            // Out-of-window events aren't in the remote response, so we
            // can't tell if they're deleted or just out of range. Skip.
            if let (Some(from), Some(to)) = (range.from, range.to)
                && !event.has_occurrence_in_range(from, to)
            {
                continue;
            }

            if sync_bases.get(&id).is_some() {
                // R48: suppressed when the local listing is incomplete.
                if deletes == Deletes::Allowed {
                    incoming.push(EventChange::Delete(event.clone()));
                }
            } else {
                outgoing.push(EventChange::Create(event.clone()));
            }
        }

        for remote_event in &remote_events {
            let id = remote_event.event().event_instance_id();

            // Already in local and remote, skip
            if local_event_ids.contains(&id) {
                continue;
            }

            // Missing locally + cancelled on remote: treat as already in sync.
            // A missing file is semantically equivalent to STATUS:CANCELLED —
            // both mean "not active". Avoids resurrecting tombstones on pull
            // and avoids spurious push-deletes for events Google has already
            // cancelled.
            if remote_event.event().status == Status::Cancelled {
                continue;
            }

            if sync_bases.get(&id).is_some() {
                // Remote event was in local, gone now. Delete remotely —
                // unless R48 suppressed deletes: the local file may only be
                // unreadable, not gone.
                if deletes == Deletes::Allowed {
                    outgoing.push(EventChange::Delete(remote_event.event().clone()));
                }
            } else {
                // Not in local, create it!
                incoming.push(EventChange::Create(remote_event.event().clone()));
            }
        }

        CalendarDiff { outgoing, incoming }
    }

    pub fn incoming(&self) -> &[EventChange] {
        &self.incoming
    }

    pub fn outgoing(&self) -> &[EventChange] {
        &self.outgoing
    }

    pub fn is_empty(&self) -> bool {
        self.outgoing.is_empty() && self.incoming.is_empty()
    }

    /// Drop outgoing changes. Used for read-only calendars where outgoing
    /// could never be applied — surfacing them as pending pushes is misleading.
    pub fn discard_outgoing(&mut self) {
        self.outgoing.clear();
    }
}

#[cfg(test)]
impl CalendarDiff {
    /// Diff a complete list of local events: absence locally is trustworthy
    /// evidence of a delete. Production always goes through
    /// [`CalendarDiff::compute_from_listing`], which carries the R48 signal.
    pub(crate) fn compute(
        local_events: Vec<CalendarEvent>,
        remote_events: Vec<RemoteEvent>,
        sync_bases: &SyncBases,
        range: &DateRange,
    ) -> Self {
        Self::compute_with(
            local_events,
            remote_events,
            sync_bases,
            range,
            Deletes::Allowed,
        )
    }

    pub(crate) fn from_changes(outgoing: Vec<EventChange>, incoming: Vec<EventChange>) -> Self {
        Self { outgoing, incoming }
    }
}

/// Whether absence from the local listing may be inferred as a delete (R48).
/// Defaults to `Allowed` everywhere the listing is known complete.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Deletes {
    Allowed,
    Suppressed,
}

enum UpdateDirection {
    Push,
    Pull,
}

fn update_direction(
    local: &CalendarEvent,
    local_event: &crate::Event,
    remote: &RemoteEvent,
    base: Option<&crate::Event>,
) -> Option<UpdateDirection> {
    // If base exists, use that for determining direction:
    if let Some(base) = base {
        let local_has_changes = local_event != base;
        let remote_has_changes = remote.event() != base;

        match (local_has_changes, remote_has_changes) {
            (false, false) => return None,
            (true, false) => return Some(UpdateDirection::Push),
            (false, true) => return Some(UpdateDirection::Pull),
            (true, true) => {}
        }
    }

    // Both sides changed, or no base exists for this legacy sync state (TODO: resolve conflicts?),
    // use mtime / LAST-MODIFIED to determine direction. If both are equal, default to pull.
    if local_is_newer(local, local_event, remote) {
        Some(UpdateDirection::Push)
    } else {
        Some(UpdateDirection::Pull)
    }
}

fn local_is_newer(local: &CalendarEvent, local_event: &crate::Event, remote: &RemoteEvent) -> bool {
    if remote.modified_at().is_none() && local_event.sequence != remote.event().sequence {
        return local_event.sequence > remote.event().sequence;
    }

    match (local.modified_at(), remote.modified_at()) {
        (Some(l), Some(r)) => l > r,
        (Some(_), None) => true,
        (None, Some(_)) => false,
        (None, None) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Event;
    use crate::calendar::{CalendarError, CalendarListing};
    use crate::event::{EventInstanceId, EventTime};
    use crate::test_utils::{test_calendar, test_calendar_event, test_event, test_event_time};
    use chrono::{TimeZone, Utc};
    use pretty_assertions::assert_eq;

    // C4 downgrade half (ADJUDICATION B-4): an OLDER caldir reading a
    // "VTODO:"-prefixed known-id line makes an id no VEVENT can ever produce.
    // It is therefore present on neither live side, and both delete passes
    // iterate the LIVE sides only — so it is inert, not a delete trigger.
    #[test]
    fn an_id_known_only_in_the_bases_triggers_no_delete() {
        let (_tmp, calendar_event) = test_calendar_event();
        let event = calendar_event.event().unwrap().clone();

        let mut sync_bases = SyncBases::new();
        // What an older caldir parses a task line into: one opaque UID.
        sync_bases.insert_known_event_id(EventInstanceId::new(
            crate::event::EventUid::new("VTODO:assignment-123@u-crawler".to_string()),
            None,
        ));

        let diff = CalendarDiff::compute(
            vec![calendar_event],
            vec![RemoteEvent::new(event)],
            &sync_bases,
            &DateRange::default(),
        );

        assert_eq!(diff.outgoing, vec![]);
        assert_eq!(diff.incoming, vec![]);
    }

    #[test]
    fn new_local_event_becomes_outgoing_create() {
        let (_tmp, calendar_event) = test_calendar_event();
        let new_event = calendar_event.event().unwrap().clone();

        let local_events = vec![calendar_event];
        let remote_events = vec![];
        let synced_ids = SyncBases::new();

        let diff = CalendarDiff::compute(
            local_events,
            remote_events,
            &synced_ids,
            &DateRange::default(),
        );

        assert_eq!(diff.outgoing, vec![EventChange::Create(new_event)]);
        assert_eq!(diff.incoming, vec![]);
    }

    #[test]
    fn new_remote_event_becomes_incoming_create() {
        let new_event = test_event();

        let local_events = vec![];
        let remote_events = vec![RemoteEvent::new(new_event.clone())];
        let synced_ids = SyncBases::new();

        let diff = CalendarDiff::compute(
            local_events,
            remote_events,
            &synced_ids,
            &DateRange::default(),
        );

        assert_eq!(diff.outgoing, vec![]);
        assert_eq!(diff.incoming, vec![EventChange::Create(new_event)]);
    }

    #[test]
    fn deleted_local_event_becomes_outgoing_delete() {
        let remote_event = test_event();

        let mut synced_ids = SyncBases::new();
        synced_ids.insert_known_event_id(remote_event.event_instance_id());

        let local_events = vec![];
        let remote_events = vec![RemoteEvent::new(remote_event.clone())];

        let diff = CalendarDiff::compute(
            local_events,
            remote_events,
            &synced_ids,
            &DateRange::default(),
        );

        assert_eq!(diff.outgoing, vec![EventChange::Delete(remote_event)]);
        assert_eq!(diff.incoming, vec![]);
    }

    #[test]
    fn invitee_seeing_cancellation_gets_status_flip_update() {
        // Active local + cancelled remote (same UID) → pull the status flip
        // so the user sees the cancellation in their calendar.
        let (_tmp, calendar) = test_calendar();
        let event = test_event();
        let calendar_event = calendar.create_event(event.clone()).unwrap();

        let mut cancelled = event.clone();
        cancelled.status = Status::Cancelled;
        // Cancellation happens on the provider side, so the remote LAST-MODIFIED
        // is fresher than the local file's mtime — pull direction. Far-future
        // timestamp keeps the test deterministic vs. the just-written tempfile.
        cancelled.last_modified = Some(Utc.with_ymd_and_hms(3000, 1, 1, 0, 0, 0).unwrap());

        let diff = CalendarDiff::compute(
            vec![calendar_event],
            vec![RemoteEvent::new(cancelled.clone())],
            &SyncBases::new(),
            &DateRange::default(),
        );

        assert_eq!(diff.outgoing, vec![]);
        assert_eq!(
            diff.incoming,
            vec![EventChange::Update {
                from: event,
                to: cancelled,
            }]
        );
    }

    #[test]
    fn missing_local_cancelled_remote_is_skipped() {
        // No local file + remote cancelled tombstone → already in sync.
        // Prevents spurious "outgoing delete" churn and stops cancelled
        // tombstones from being re-created on pull.
        let mut cancelled = test_event();
        cancelled.status = Status::Cancelled;

        let mut synced_ids = SyncBases::new();
        synced_ids.insert_known_event_id(cancelled.event_instance_id());

        let diff = CalendarDiff::compute(
            vec![],
            vec![RemoteEvent::new(cancelled)],
            &synced_ids,
            &DateRange::default(),
        );

        assert_eq!(diff.outgoing, vec![]);
        assert_eq!(diff.incoming, vec![]);
    }

    #[test]
    fn both_cancelled_with_field_drift_is_skipped() {
        // Same UID, both cancelled, but local has stripped data and remote
        // has full data. Don't surface this as churn — cancelled events are
        // historical record, the field-shape drift doesn't matter.
        let (_tmp, calendar) = test_calendar();
        let mut local = test_event();
        local.status = Status::Cancelled;
        let calendar_event = calendar.create_event(local.clone()).unwrap();

        let mut remote = local.clone();
        remote.summary = Some("Now with extra data".into());
        remote.location = Some("Somewhere".into());

        let diff = CalendarDiff::compute(
            vec![calendar_event],
            vec![RemoteEvent::new(remote)],
            &SyncBases::new(),
            &DateRange::default(),
        );

        assert_eq!(diff.outgoing, vec![]);
        assert_eq!(diff.incoming, vec![]);
    }

    #[test]
    fn expired_recurring_event_not_flagged_as_delete() {
        // Expired recurring event (UNTIL in the past) has no occurrences in
        // the sync window, but it's not deleted — don't flag it as one.
        let (_tmp, calendar) = test_calendar();

        let mut event = Event::new(
            "Old recurring",
            EventTime::Date(chrono::NaiveDate::from_ymd_opt(1968, 5, 8).unwrap()),
        );
        event.recurrence = Some(crate::event::Recurrence::new(
            "FREQ=YEARLY;UNTIL=20080507T120000Z",
        ));
        let calendar_event = calendar.create_event(event.clone()).unwrap();

        let mut synced_ids = SyncBases::new();
        synced_ids.insert_known_event_id(event.event_instance_id());

        let range = DateRange {
            from: Some(Utc.with_ymd_and_hms(2025, 5, 14, 0, 0, 0).unwrap()),
            to: Some(Utc.with_ymd_and_hms(2027, 5, 14, 0, 0, 0).unwrap()),
        };

        let diff = CalendarDiff::compute(vec![calendar_event], vec![], &synced_ids, &range);

        assert_eq!(diff.outgoing, vec![]);
        assert_eq!(diff.incoming, vec![]);
    }

    #[test]
    fn deleted_remote_event_becomes_incoming_delete() {
        let (_tmp, calendar_event) = test_calendar_event();
        let local_event = calendar_event.event().unwrap().clone();

        let mut synced_ids = SyncBases::new();
        synced_ids.insert_known_event_id(local_event.event_instance_id());

        let local_events = vec![calendar_event];
        let remote_events = vec![];

        let diff = CalendarDiff::compute(
            local_events,
            remote_events,
            &synced_ids,
            &DateRange::default(),
        );

        assert_eq!(diff.outgoing, vec![]);
        assert_eq!(diff.incoming, vec![EventChange::Delete(local_event)]);
    }

    #[test]
    fn event_base_marks_event_as_previously_synced() {
        let (_tmp, calendar_event) = test_calendar_event();
        let local_event = calendar_event.event().unwrap().clone();
        let id = local_event.event_instance_id();

        let mut sync_bases = SyncBases::new();
        sync_bases.insert_event_base(id, local_event.clone());

        let diff = CalendarDiff::compute(
            vec![calendar_event],
            vec![],
            &sync_bases,
            &DateRange::default(),
        );

        assert_eq!(diff.outgoing, vec![]);
        assert_eq!(diff.incoming, vec![EventChange::Delete(local_event)]);
    }

    #[test]
    fn updated_local_event_becomes_outgoing_update() {
        let (_tmp, calendar) = test_calendar();
        let remote_event = test_event();

        let mut local_event = remote_event.clone();
        local_event.summary = Some("Updated Test Event".to_string());
        let calendar_event = calendar.create_event(local_event.clone()).unwrap();

        let mut synced_ids = SyncBases::new();
        synced_ids.insert_known_event_id(remote_event.event_instance_id());

        let local_events = vec![calendar_event];
        let remote_events = vec![RemoteEvent::new(remote_event.clone())];

        let diff = CalendarDiff::compute(
            local_events,
            remote_events,
            &synced_ids,
            &DateRange::default(),
        );

        assert_eq!(
            diff.outgoing,
            vec![EventChange::Update {
                from: remote_event,
                to: local_event,
            }]
        );
        assert_eq!(diff.incoming, vec![]);
    }

    #[test]
    fn higher_remote_sequence_breaks_conflict_without_modified_time() {
        let (_tmp, calendar) = test_calendar();
        let mut base = test_event();
        base.sequence = 1;

        let mut local = base.clone();
        local.summary = Some("Edited locally".to_string());
        local.sequence = 2;
        let calendar_event = calendar.create_event(local.clone()).unwrap();

        let mut remote = base.clone();
        remote.summary = Some("Edited remotely".to_string());
        remote.sequence = 3;
        remote.last_modified = None;

        let mut sync_bases = SyncBases::new();
        sync_bases.insert_event_base(base.event_instance_id(), base);

        let diff = CalendarDiff::compute(
            vec![calendar_event],
            vec![RemoteEvent::new(remote.clone())],
            &sync_bases,
            &DateRange::default(),
        );

        assert_eq!(diff.outgoing, vec![]);
        assert_eq!(
            diff.incoming,
            vec![EventChange::Update {
                from: local,
                to: remote,
            }]
        );
    }

    #[test]
    fn remote_update_without_modified_time_is_pulled_when_local_matches_base() {
        let (_tmp, calendar) = test_calendar();
        let base = test_event();
        let calendar_event = calendar.create_event(base.clone()).unwrap();
        assert!(calendar_event.modified_at().is_some());

        let mut remote = base.clone();
        remote.summary = Some("Edited remotely".to_string());
        remote.last_modified = None;

        let mut sync_bases = SyncBases::new();
        sync_bases.insert_event_base(base.event_instance_id(), base.clone());

        let diff = CalendarDiff::compute(
            vec![calendar_event],
            vec![RemoteEvent::new(remote.clone())],
            &sync_bases,
            &DateRange::default(),
        );

        assert_eq!(diff.outgoing, vec![]);
        assert_eq!(
            diff.incoming,
            vec![EventChange::Update {
                from: base,
                to: remote,
            }]
        );
    }

    /// The base comparison must read the local event as stored, never the
    /// visibility-patched copy assembled for the push. Patching it first makes
    /// an untouched local file look edited, flipping an incoming update into an
    /// outgoing one that overwrites the remote's real change.
    #[test]
    fn base_comparison_ignores_the_visibility_patched_push_copy() {
        let (_tmp, calendar) = test_calendar();
        let base = test_event();
        assert_eq!(base.visibility, None);
        let calendar_event = calendar.create_event(base.clone()).unwrap();
        assert!(calendar_event.modified_at().is_some());

        let mut remote = base.clone();
        remote.summary = Some("Edited remotely".to_string());
        remote.visibility = Some(crate::event::Visibility::Private);
        remote.last_modified = None;

        let mut sync_bases = SyncBases::new();
        sync_bases.insert_event_base(base.event_instance_id(), base.clone());

        let diff = CalendarDiff::compute(
            vec![calendar_event],
            vec![RemoteEvent::new(remote.clone())],
            &sync_bases,
            &DateRange::default(),
        );

        assert_eq!(diff.outgoing, vec![]);
        assert_eq!(
            diff.incoming,
            vec![EventChange::Update {
                from: base,
                to: remote,
            }]
        );
    }

    #[test]
    fn remote_update_is_pulled_when_unchanged_local_file_has_newer_mtime() {
        let (_tmp, calendar) = test_calendar();
        let mut local = test_event();
        local.last_modified = Some(Utc.with_ymd_and_hms(2026, 3, 5, 8, 45, 16).unwrap());
        let calendar_event = calendar.create_event(local.clone()).unwrap();

        let mut base = local.clone();
        base.last_modified = None;

        let mut remote = base.clone();
        remote.summary = Some("Edited remotely".to_string());
        remote.last_modified = Some(Utc.with_ymd_and_hms(2025, 6, 9, 10, 42, 20).unwrap());

        assert!(calendar_event.modified_at() > remote.last_modified);

        let mut sync_bases = SyncBases::new();
        sync_bases.insert_event_base(base.event_instance_id(), base.clone());

        let diff = CalendarDiff::compute(
            vec![calendar_event],
            vec![RemoteEvent::new(remote.clone())],
            &sync_bases,
            &DateRange::default(),
        );

        assert_eq!(diff.outgoing, vec![]);
        assert_eq!(
            diff.incoming,
            vec![EventChange::Update {
                from: local,
                to: remote,
            }]
        );
    }

    #[test]
    fn local_update_is_pushed_when_unchanged_remote_has_newer_modified_time() {
        let (_tmp, calendar) = test_calendar();
        let base = test_event();

        let mut local = base.clone();
        local.summary = Some("Edited locally".to_string());
        local.last_modified = Some(Utc.with_ymd_and_hms(2025, 6, 9, 10, 42, 20).unwrap());
        let calendar_event = calendar.create_event(local.clone()).unwrap();

        let mut remote = base.clone();
        remote.last_modified = Some(Utc.with_ymd_and_hms(2026, 3, 5, 8, 45, 16).unwrap());
        assert!(calendar_event.modified_at() < remote.last_modified);

        let mut sync_bases = SyncBases::new();
        sync_bases.insert_event_base(base.event_instance_id(), base);

        let diff = CalendarDiff::compute(
            vec![calendar_event],
            vec![RemoteEvent::new(remote.clone())],
            &sync_bases,
            &DateRange::default(),
        );

        assert_eq!(diff.incoming, vec![]);
        assert_eq!(
            diff.outgoing,
            vec![EventChange::Update {
                from: remote,
                to: local,
            }]
        );
    }

    #[test]
    fn sync_metadata_only_differences_produce_no_diff() {
        // Differences confined to LAST-MODIFIED and SEQUENCE are sync noise —
        // they shouldn't surface as pending updates.
        let (_tmp, calendar) = test_calendar();
        let local_event = test_event();

        let mut remote_event = local_event.clone();
        remote_event.last_modified = Some(Utc::now() + chrono::Duration::days(1));
        remote_event.sequence += 1;

        let calendar_event = calendar.create_event(local_event.clone()).unwrap();

        let mut synced_ids = SyncBases::new();
        synced_ids.insert_known_event_id(local_event.event_instance_id());

        let diff = CalendarDiff::compute(
            vec![calendar_event],
            vec![RemoteEvent::new(remote_event)],
            &synced_ids,
            &DateRange::default(),
        );

        assert_eq!(diff.outgoing, vec![]);
        assert_eq!(diff.incoming, vec![]);
    }

    #[test]
    fn equal_timestamps_with_field_drift_resolve_to_pull() {
        // Regression: when local mtime equals remote LAST-MODIFIED but the
        // events still differ on some field, the difference reflects a
        // schema/parser change (the local file pre-dates our support for
        // some property), not a local edit. Direction must be pull so the
        // remote authoritative value refreshes the field we now understand.
        let (_tmp, calendar) = test_calendar();
        let when = Utc.with_ymd_and_hms(2026, 5, 15, 13, 12, 52).unwrap();

        let mut local_event = test_event();
        local_event.last_modified = Some(when);
        let calendar_event = calendar.create_event(local_event.clone()).unwrap();

        let mut remote_event = local_event.clone();
        remote_event.visibility = Some(crate::event::Visibility::Private);

        let mut synced_ids = SyncBases::new();
        synced_ids.insert_known_event_id(local_event.event_instance_id());

        let diff = CalendarDiff::compute(
            vec![calendar_event],
            vec![RemoteEvent::new(remote_event.clone())],
            &synced_ids,
            &DateRange::default(),
        );

        assert_eq!(diff.outgoing, vec![]);
        assert_eq!(
            diff.incoming,
            vec![EventChange::Update {
                from: local_event,
                to: remote_event,
            }]
        );
    }

    #[test]
    fn unspecified_local_visibility_resolves_to_pull_even_when_local_mtime_is_newer() {
        // Regression: a local file predating CLASS support has visibility: None.
        // Even when its mtime is newer than the remote LAST-MODIFIED (e.g. after
        // a bulk re-serialization bumped every file's mtime), an unspecified
        // visibility must not originate a push that overwrites the remote's real
        // value — it resolves to a pull instead.
        let (_tmp, calendar) = test_calendar();

        let mut local_event = test_event();
        local_event.last_modified = Some(Utc.with_ymd_and_hms(2026, 3, 5, 8, 45, 16).unwrap());
        assert_eq!(local_event.visibility, None);
        let calendar_event = calendar.create_event(local_event.clone()).unwrap();

        let mut remote_event = local_event.clone();
        remote_event.visibility = Some(crate::event::Visibility::Private);
        remote_event.last_modified = Some(Utc.with_ymd_and_hms(2025, 6, 9, 10, 42, 20).unwrap());

        let mut synced_ids = SyncBases::new();
        synced_ids.insert_known_event_id(local_event.event_instance_id());

        let diff = CalendarDiff::compute(
            vec![calendar_event],
            vec![RemoteEvent::new(remote_event.clone())],
            &synced_ids,
            &DateRange::default(),
        );

        assert_eq!(diff.outgoing, vec![]);
        assert_eq!(
            diff.incoming,
            vec![EventChange::Update {
                from: local_event,
                to: remote_event,
            }]
        );
    }

    #[test]
    fn outgoing_update_keeps_remote_visibility_when_local_is_unspecified() {
        // A genuine local edit (summary) still pushes, but it must not drag an
        // unspecified visibility along: the pushed event keeps the remote's
        // visibility rather than dropping it to the default.
        let (_tmp, calendar) = test_calendar();

        let mut local_event = test_event();
        local_event.summary = Some("Edited locally".to_string());
        local_event.last_modified = Some(Utc.with_ymd_and_hms(2026, 3, 5, 8, 45, 16).unwrap());
        let calendar_event = calendar.create_event(local_event.clone()).unwrap();

        let mut remote_event = local_event.clone();
        remote_event.summary = Some("Test Event".to_string());
        remote_event.visibility = Some(crate::event::Visibility::Private);
        remote_event.last_modified = Some(Utc.with_ymd_and_hms(2025, 6, 9, 10, 42, 20).unwrap());

        let mut synced_ids = SyncBases::new();
        synced_ids.insert_known_event_id(local_event.event_instance_id());

        let diff = CalendarDiff::compute(
            vec![calendar_event],
            vec![RemoteEvent::new(remote_event)],
            &synced_ids,
            &DateRange::default(),
        );

        assert_eq!(diff.incoming, vec![]);
        match diff.outgoing.as_slice() {
            [EventChange::Update { to, .. }] => {
                assert_eq!(to.summary.as_deref(), Some("Edited locally"));
                assert_eq!(to.visibility, Some(crate::event::Visibility::Private));
            }
            other => panic!("expected one outgoing Update, got {other:?}"),
        }
    }

    #[test]
    fn x_property_change_surfaces_as_diff() {
        let (_tmp, calendar) = test_calendar();
        let mut local_event = test_event();
        local_event
            .x_properties
            .push(crate::event::XProperty::new("X-GOOGLE-COLOR-ID", "5"));

        let mut remote_event = local_event.clone();
        remote_event.x_properties = vec![crate::event::XProperty::new("X-GOOGLE-COLOR-ID", "9")];
        remote_event.last_modified = Some(Utc::now() + chrono::Duration::days(1));

        let calendar_event = calendar.create_event(local_event.clone()).unwrap();

        let mut synced_ids = SyncBases::new();
        synced_ids.insert_known_event_id(local_event.event_instance_id());

        let diff = CalendarDiff::compute(
            vec![calendar_event],
            vec![RemoteEvent::new(remote_event.clone())],
            &synced_ids,
            &DateRange::default(),
        );

        assert_eq!(diff.outgoing, vec![]);
        assert_eq!(
            diff.incoming,
            vec![EventChange::Update {
                from: local_event,
                to: remote_event,
            }]
        );
    }

    #[test]
    fn x_properties_compare_equal_regardless_of_order() {
        // Local files come from BTreeMap-backed ICS parsing (alphabetical),
        // but providers like Google build x_properties in insertion order.
        // Same content in different order must compare equal.
        let mut event_a = test_event();
        event_a.x_properties = vec![
            crate::event::XProperty::new("X-GOOGLE-EVENT-ID", "abc"),
            crate::event::XProperty::new("X-GOOGLE-COLOR-ID", "5"),
        ];

        let mut event_b = event_a.clone();
        event_b.x_properties = vec![
            crate::event::XProperty::new("X-GOOGLE-COLOR-ID", "5"),
            crate::event::XProperty::new("X-GOOGLE-EVENT-ID", "abc"),
        ];

        assert_eq!(event_a, event_b);
    }

    #[test]
    fn updated_remote_event_becomes_incoming_update() {
        let (_tmp, calendar) = test_calendar();
        let local_event = test_event();

        let mut remote_event = local_event.clone();
        remote_event.summary = Some("Updated Test Event".to_string());
        remote_event.last_modified = Some(Utc::now() + chrono::Duration::days(1));

        let calendar_event = calendar.create_event(local_event.clone()).unwrap();

        let mut synced_ids = SyncBases::new();
        synced_ids.insert_known_event_id(local_event.event_instance_id());

        let local_events = vec![calendar_event];
        let remote_events = vec![RemoteEvent::new(remote_event.clone())];

        let diff = CalendarDiff::compute(
            local_events,
            remote_events,
            &synced_ids,
            &DateRange::default(),
        );

        assert_eq!(diff.outgoing, vec![]);
        assert_eq!(
            diff.incoming,
            vec![EventChange::Update {
                from: local_event,
                to: remote_event,
            }]
        );
    }

    #[test]
    fn local_event_outside_window_is_not_flagged_for_delete() {
        // Spec: events outside the queried window are left untouched even
        // when their IDs sit in synced_ids and the remote response is empty.
        let (_tmp, calendar) = test_calendar();

        let mut old_event = test_event();
        old_event.start =
            EventTime::DateTimeUtc(Utc.with_ymd_and_hms(2020, 1, 1, 9, 0, 0).unwrap());
        old_event.end = Some(EventTime::DateTimeUtc(
            Utc.with_ymd_and_hms(2020, 1, 1, 10, 0, 0).unwrap(),
        ));
        let calendar_event = calendar.create_event(old_event.clone()).unwrap();

        let mut synced_ids = SyncBases::new();
        synced_ids.insert_known_event_id(old_event.event_instance_id());

        let range = DateRange {
            from: Some(Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap()),
            to: Some(Utc.with_ymd_and_hms(2027, 1, 1, 0, 0, 0).unwrap()),
        };

        let diff = CalendarDiff::compute(vec![calendar_event], vec![], &synced_ids, &range);

        assert_eq!(diff.outgoing, vec![]);
        assert_eq!(diff.incoming, vec![]);
    }

    #[test]
    fn local_event_inside_window_is_flagged_for_delete_when_remote_drops_it() {
        let (_tmp, calendar) = test_calendar();

        let mut event = test_event();
        event.start = EventTime::DateTimeUtc(Utc.with_ymd_and_hms(2026, 6, 15, 9, 0, 0).unwrap());
        event.end = Some(EventTime::DateTimeUtc(
            Utc.with_ymd_and_hms(2026, 6, 15, 10, 0, 0).unwrap(),
        ));
        let calendar_event = calendar.create_event(event.clone()).unwrap();

        let mut synced_ids = SyncBases::new();
        synced_ids.insert_known_event_id(event.event_instance_id());

        let range = DateRange {
            from: Some(Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap()),
            to: Some(Utc.with_ymd_and_hms(2027, 1, 1, 0, 0, 0).unwrap()),
        };

        let diff = CalendarDiff::compute(vec![calendar_event], vec![], &synced_ids, &range);

        assert_eq!(diff.outgoing, vec![]);
        assert_eq!(diff.incoming, vec![EventChange::Delete(event)]);
    }

    /// T-29 (spec §18, R48/C7). A calendar with an unreadable `.ics` has a
    /// known-incomplete listing: no absence in it is evidence of a delete, so
    /// EVERY delete is suppressed, both directions. Creates still flow.
    #[test]
    fn unreadable_local_files_suppress_all_deletes() {
        let (_tmp, calendar) = test_calendar();

        // Local, base known, absent remotely → would be an incoming Delete.
        let local_deleted = test_event();
        let local_deleted_event = calendar.create_event(local_deleted.clone()).unwrap();

        // Local, no base, absent remotely → still an outgoing Create.
        let local_new = Event::new("Local new", test_event_time());
        let local_new_event = calendar.create_event(local_new.clone()).unwrap();

        // Remote, base known, absent locally → would be an outgoing Delete.
        let remote_deleted = Event::new("Remote deleted", test_event_time());

        // Remote, no base, absent locally → still an incoming Create.
        let remote_new = Event::new("Remote new", test_event_time());

        let mut sync_bases = SyncBases::new();
        sync_bases.insert_known_event_id(local_deleted.event_instance_id());
        sync_bases.insert_known_event_id(remote_deleted.event_instance_id());

        let mut listing = CalendarListing::default();
        listing.push_event(local_deleted_event);
        listing.push_event(local_new_event);
        listing.push_skipped(
            calendar.path().join("broken.ics"),
            CalendarError::MasterNotFound("broken".to_string()),
        );
        assert!(listing.is_incomplete());

        let diff = CalendarDiff::compute_from_listing(
            listing,
            vec![
                RemoteEvent::new(remote_deleted.clone()),
                RemoteEvent::new(remote_new.clone()),
            ],
            &sync_bases,
            &DateRange::default(),
        );

        let outgoing_deletes: Vec<_> = diff
            .outgoing()
            .iter()
            .filter(|c| matches!(c, EventChange::Delete(_)))
            .collect();
        let incoming_deletes: Vec<_> = diff
            .incoming()
            .iter()
            .filter(|c| matches!(c, EventChange::Delete(_)))
            .collect();

        assert_eq!(outgoing_deletes, Vec::<&EventChange>::new());
        assert_eq!(incoming_deletes, Vec::<&EventChange>::new());

        // Delete-scoped, not a blanket "do nothing".
        assert_eq!(diff.outgoing, vec![EventChange::Create(local_new)]);
        assert_eq!(diff.incoming, vec![EventChange::Create(remote_new)]);
    }

    /// The other polarity (R47): a complete listing deletes exactly as before.
    #[test]
    fn complete_listing_still_emits_deletes_in_both_directions() {
        let (_tmp, calendar) = test_calendar();

        let local_deleted = test_event();
        let local_deleted_event = calendar.create_event(local_deleted.clone()).unwrap();
        let remote_deleted = Event::new("Remote deleted", test_event_time());

        let mut sync_bases = SyncBases::new();
        sync_bases.insert_known_event_id(local_deleted.event_instance_id());
        sync_bases.insert_known_event_id(remote_deleted.event_instance_id());

        let mut listing = CalendarListing::default();
        listing.push_event(local_deleted_event);
        assert!(!listing.is_incomplete());

        let diff = CalendarDiff::compute_from_listing(
            listing,
            vec![RemoteEvent::new(remote_deleted.clone())],
            &sync_bases,
            &DateRange::default(),
        );

        assert_eq!(diff.outgoing, vec![EventChange::Delete(remote_deleted)]);
        assert_eq!(diff.incoming, vec![EventChange::Delete(local_deleted)]);
    }
}
