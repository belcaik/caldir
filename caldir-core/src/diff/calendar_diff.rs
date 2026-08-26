use std::collections::{HashMap, HashSet};

use super::event_change::EventChange;
use crate::calendar::{CalendarListing, SyncBases};
use crate::event::ComponentKind;
use crate::item::CalendarItem;
use crate::{CalendarEvent, DateRange, RemoteEvent};

pub struct CalendarDiff {
    outgoing: Vec<EventChange>,
    incoming: Vec<EventChange>,
    /// Whether the version-skew belt actually suppressed a task delete on
    /// this run. See [`CalendarDiff::suppressed_task_deletes`].
    suppressed_task_deletes: bool,
    /// How many outgoing changes the event-only wire withheld on this run. See
    /// [`CalendarDiff::withheld_task_pushes`].
    withheld_task_pushes: usize,
}

impl CalendarDiff {
    /// Diff a local listing against the remote. A listing that skipped an
    /// unreadable file is known-incomplete, so no local absence proves a delete
    /// and every delete for this calendar is suppressed, both directions.
    pub(crate) fn compute_from_listing(
        local_listing: CalendarListing,
        remote_events: Vec<RemoteEvent>,
        sync_bases: &SyncBases,
        range: &DateRange,
        task_pushes: TaskPushes,
        task_reads: TaskReads,
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
            task_pushes,
            task_reads,
        )
    }

    fn compute_with(
        local_events: Vec<CalendarEvent>,
        remote_events: Vec<RemoteEvent>,
        sync_bases: &SyncBases,
        range: &DateRange,
        deletes: Deletes,
        task_pushes: TaskPushes,
        task_reads: TaskReads,
    ) -> Self {
        // Keyed by the kind-carrying identity: a VEVENT and a VTODO
        // sharing a UID are two entries on each side, so `.collect()` cannot
        // silently merge them and no lookup can match across kinds.
        let local_item_ids: HashSet<_> = local_events
            .iter()
            .map(|e| e.item().instance_id())
            .collect();

        let remote_by_id: HashMap<_, &RemoteEvent> = remote_events
            .iter()
            .map(|e| (e.item().instance_id(), e))
            .collect();

        // Version-skew belt: a pre-VTODO provider still filters on VEVENT and
        // reports no tasks at all. Their total absence is version skew, not a
        // delete. Scoped to *tasks*, so event deletes are untouched, and to the
        // *all-tasks-absent* case, so a task missing from a task-carrying
        // response is still a real delete.
        //
        // Decided once, here, from the inputs: re-deriving it per branch is
        // how the two suppression sites drift apart under a later edit.
        let remote_reports_tasks = remote_events
            .iter()
            .any(|e| matches!(e.item(), CalendarItem::Todo(_)));

        let mut outgoing = Vec::new();
        let mut incoming = Vec::new();
        let mut suppressed_task_deletes = false;

        for local_event in &local_events {
            let item = local_event.item();
            let id = item.instance_id();

            // In both local and remote: skip if equal, otherwise update
            if let Some(remote_event) = remote_by_id.get(&id) {
                if item == remote_event.item() {
                    continue;
                }

                // Both cancelled: treat as in-sync regardless of field drift.
                // Cancelled items are historical; we don't churn syncing them.
                // `CANCELLED` only, never `COMPLETED`.
                if item.is_cancelled() && remote_event.item().is_cancelled() {
                    continue;
                }

                // Never push an unspecified (None) visibility — inherit the
                // remote's. Event-only: `VTODO` has no `CLASS` field to patch.
                let mut to_push = item.clone();
                if let (CalendarItem::Event(push), CalendarItem::Event(remote)) =
                    (&mut to_push, remote_event.item())
                    && push.visibility.is_none()
                {
                    push.visibility = remote.visibility;
                }

                let base = sync_bases.get(&id).and_then(Option::as_deref);
                let direction = if &to_push == remote_event.item() {
                    Some(UpdateDirection::Pull)
                } else {
                    update_direction(local_event, item, remote_event, base)
                };

                match direction {
                    Some(UpdateDirection::Push) => outgoing.push(EventChange::Update {
                        from: remote_event.item().clone(),
                        to: to_push,
                    }),
                    Some(UpdateDirection::Pull) => incoming.push(EventChange::Update {
                        from: item.clone(),
                        to: remote_event.item().clone(),
                    }),
                    None => {}
                }
                continue;
            }

            // Out-of-window items aren't in the remote response, so we
            // can't tell if they're deleted or just out of range. Skip.
            //
            // Component-aware: events keep `has_occurrence_in_range`
            // untouched, tasks get RFC 4791 §9.9's "core's rule". Polarity:
            // `false` here means `continue` — neither the delete below
            // nor the create — and `true` is what authorizes the delete.
            if let (Some(from), Some(to)) = (range.from, range.to)
                && !item.has_occurrence_in_range(from, to)
            {
                continue;
            }

            if sync_bases.get(&id).is_some() {
                // Suppressed when the local listing is incomplete, and for
                // tasks when the remote reported none — unless the provider
                // explicitly advertises `tasks.read`, in which case a
                // zero-task response is a trustworthy answer, not version
                // skew. Only `TaskReads::Advertised` lifts this; every
                // other capability outcome collapses to `NotAdvertised`
                // upstream, never here.
                let suppressed_by_version_skew = matches!(item, CalendarItem::Todo(_))
                    && !remote_reports_tasks
                    && task_reads == TaskReads::NotAdvertised;

                // Recorded only where a delete was actually withheld, so the
                // signal means "the belt cost you something", not merely "the
                // response held no tasks" — which is the ordinary state of
                // every task-free calendar.
                suppressed_task_deletes |= suppressed_by_version_skew;

                if deletes == Deletes::Allowed && !suppressed_by_version_skew {
                    incoming.push(EventChange::Delete(item.clone()));
                }
            } else {
                outgoing.push(EventChange::Create(item.clone()));
            }
        }

        for remote_event in &remote_events {
            let item = remote_event.item();
            let id = item.instance_id();

            // Already in local and remote, skip
            if local_item_ids.contains(&id) {
                continue;
            }

            // Missing locally + cancelled on remote: treat as already in sync.
            // A missing file is semantically equivalent to STATUS:CANCELLED —
            // both mean "not active". Avoids resurrecting tombstones on pull
            // and avoids spurious push-deletes for events Google has already
            // cancelled. `CANCELLED` only, never `COMPLETED`: a remote
            // completed task must still be created locally.
            if item.is_cancelled() {
                continue;
            }

            if sync_bases.get(&id).is_some() {
                // Remote item was in local, gone now. Delete remotely —
                // unless an incomplete local listing suppressed deletes: the
                // local file may only be unreadable, not gone.
                if deletes == Deletes::Allowed {
                    outgoing.push(EventChange::Delete(item.clone()));
                }
            } else {
                // Not in local, create it!
                incoming.push(EventChange::Create(item.clone()));
            }
        }

        // The capability flag decides whether the event-only wire's
        // withholding runs at all. `CalendarDiff` never inspects a `Remote`
        // or a provider name to get here — `Connection::diff` resolved that
        // upstream and handed down only this enum, the way `Deletes` already
        // arrives decided.
        let withheld_task_pushes = match task_pushes {
            TaskPushes::Allowed => 0,
            TaskPushes::Withheld => withhold_task_pushes(&mut outgoing),
        };

        CalendarDiff {
            outgoing,
            incoming,
            suppressed_task_deletes,
            withheld_task_pushes,
        }
    }

    /// Whether the version-skew belt withheld at least one task delete.
    ///
    /// A remote response carrying **zero** tasks is indistinguishable from a
    /// pre-VTODO provider or a server that cannot answer a VTODO query, so
    /// core refuses to read it as "every task was deleted" and keeps the
    /// files. The price: a genuine remote deletion of *every* task goes
    /// unnoticed until something reports a task again.
    ///
    /// The suppression must not be silent — a self-perpetuating belt nobody
    /// is told about is the failure mode this accessor exists to prevent.
    /// Consumed by the CLI's diff renderer: `status`, `sync` and
    /// `pull` print a warning line per calendar whenever this is true, and the
    /// warning survives an otherwise-empty diff.
    pub fn suppressed_task_deletes(&self) -> bool {
        self.suppressed_task_deletes
    }

    /// How many outgoing changes were withheld because they are tasks.
    ///
    /// A remote that does not advertise `tasks.write` cannot take a task
    /// push, so it is dropped from the outgoing set the way a read-only
    /// calendar drops everything — never queued, never attempted — so a
    /// mixed calendar's *events* keep syncing exactly as they would without
    /// it. The limitation belongs to the provider, not to caldir.
    ///
    /// The withholding must not be silent: the CLI renders a warning line,
    /// naming the provider, per calendar on every surface that shows
    /// outgoing changes.
    pub fn withheld_task_pushes(&self) -> usize {
        self.withheld_task_pushes
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
        // A read-only calendar pushes nothing at all, so singling tasks out
        // there would report a restriction that is not the one in force.
        self.withheld_task_pushes = 0;
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
            TaskPushes::Withheld,
            TaskReads::NotAdvertised,
        )
    }

    /// Applies the same R70 withholding as the production path, so a test
    /// diff cannot hold an outgoing shape production can never produce.
    pub(crate) fn from_changes(mut outgoing: Vec<EventChange>, incoming: Vec<EventChange>) -> Self {
        let withheld_task_pushes = withhold_task_pushes(&mut outgoing);
        Self {
            outgoing,
            incoming,
            suppressed_task_deletes: false,
            withheld_task_pushes,
        }
    }
}

/// Drop every non-`VEVENT` change from the outgoing set, returning how many
/// were dropped.
///
/// Keyed on the kind, not on the Rust type: a component kind added later is
/// withheld here rather than smuggled onto an event-only wire.
fn withhold_task_pushes(outgoing: &mut Vec<EventChange>) -> usize {
    let before = outgoing.len();
    outgoing.retain(|change| change.written_item().instance_id().kind() == ComponentKind::Event);
    before - outgoing.len()
}

/// Whether absence from the local listing may be inferred as a delete.
/// Defaults to `Allowed` everywhere the listing is known complete.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Deletes {
    Allowed,
    Suppressed,
}

/// Whether the provider advertises `tasks.write`. Threaded into
/// `compute_with` the same way `Deletes` is: decided by the caller
/// (`Connection::diff`, from `Remote::capabilities()`) and handed down as an
/// opaque flag. `CalendarDiff` never sees a `Remote` or a provider name to
/// derive this itself.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum TaskPushes {
    Allowed,
    Withheld,
}

/// Whether the provider advertises `tasks.read`. Threaded into
/// `compute_with` the same way `TaskPushes` is: decided by the caller
/// (`Connection::diff`, from `Remote::capabilities()`) and handed down as an
/// opaque flag. `CalendarDiff` never sees a `Remote` or a provider name to
/// derive this itself.
///
/// Only an explicit, successfully-decoded `tasks.read = true` produces
/// `Advertised`. Absent, unknown-method, errored, timed-out, or explicit
/// `false` all collapse to `NotAdvertised` upstream — `Remote::capabilities()`
/// and `Connection::diff` own that collapse; `#[derive(Default)]` on
/// `Capability` must never be read as `Advertised` here or anywhere else.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum TaskReads {
    Advertised,
    NotAdvertised,
}

enum UpdateDirection {
    Push,
    Pull,
}

fn update_direction(
    local: &CalendarEvent,
    local_item: &CalendarItem,
    remote: &RemoteEvent,
    base: Option<&CalendarItem>,
) -> Option<UpdateDirection> {
    // If base exists, use that for determining direction:
    if let Some(base) = base {
        let local_has_changes = local_item != base;
        let remote_has_changes = remote.item() != base;

        match (local_has_changes, remote_has_changes) {
            (false, false) => return None,
            (true, false) => return Some(UpdateDirection::Push),
            (false, true) => return Some(UpdateDirection::Pull),
            (true, true) => {}
        }
    }

    // Both sides changed, or no base exists for this legacy sync state (TODO: resolve conflicts?),
    // use mtime / LAST-MODIFIED to determine direction. If both are equal, default to pull.
    if local_is_newer(local, local_item, remote) {
        Some(UpdateDirection::Push)
    } else {
        Some(UpdateDirection::Pull)
    }
}

fn local_is_newer(local: &CalendarEvent, local_item: &CalendarItem, remote: &RemoteEvent) -> bool {
    if remote.modified_at().is_none() && local_item.sequence() != remote.item().sequence() {
        return local_item.sequence() > remote.item().sequence();
    }

    match (local.modified_at(), remote.modified_at()) {
        (Some(l), Some(r)) => l > r,
        // Push-biased. A wrong Pull destroys the user's local edit with
        // no recovery, so this arm is not a tiebreak to tidy up.
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
    use crate::connection::Connection;
    use crate::event::{EventInstanceId, EventTime, Status};
    use crate::test_utils::{
        test_caldir, test_calendar, test_calendar_event, test_event, test_event_time,
        test_mock_provider, test_remote_config, test_remote_params,
    };
    use crate::todo::{Todo, TodoStatus};
    use crate::{CalendarConfig, rpc};
    use chrono::{DateTime, TimeZone, Utc};
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

        assert_eq!(diff.outgoing, vec![EventChange::Create(new_event.into())]);
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
        assert_eq!(diff.incoming, vec![EventChange::Create(new_event.into())]);
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

        assert_eq!(
            diff.outgoing,
            vec![EventChange::Delete(remote_event.into())]
        );
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
                from: event.into(),
                to: cancelled.into(),
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
        assert_eq!(diff.incoming, vec![EventChange::Delete(local_event.into())]);
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
        assert_eq!(diff.incoming, vec![EventChange::Delete(local_event.into())]);
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
                from: remote_event.into(),
                to: local_event.into(),
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
                from: local.into(),
                to: remote.into(),
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
                from: base.into(),
                to: remote.into(),
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
                from: base.into(),
                to: remote.into(),
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
                from: local.into(),
                to: remote.into(),
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
                from: remote.into(),
                to: local.into(),
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
                from: local_event.into(),
                to: remote_event.into(),
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
                from: local_event.into(),
                to: remote_event.into(),
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
            [
                EventChange::Update {
                    to: CalendarItem::Event(to),
                    ..
                },
            ] => {
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
                from: local_event.into(),
                to: remote_event.into(),
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
                from: local_event.into(),
                to: remote_event.into(),
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
        assert_eq!(diff.incoming, vec![EventChange::Delete(event.into())]);
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
            TaskPushes::Withheld,
            TaskReads::NotAdvertised,
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
        assert_eq!(diff.outgoing, vec![EventChange::Create(local_new.into())]);
        assert_eq!(diff.incoming, vec![EventChange::Create(remote_new.into())]);
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
            TaskPushes::Withheld,
            TaskReads::NotAdvertised,
        );

        assert_eq!(
            diff.outgoing,
            vec![EventChange::Delete(remote_deleted.into())]
        );
        assert_eq!(
            diff.incoming,
            vec![EventChange::Delete(local_deleted.into())]
        );
    }

    // ========================================================================
    // The RFC 4791 §9.9 window rule for tasks (R44–R47, R97)
    // ========================================================================

    /// Start of the bounded window every case below is measured against.
    fn window_from() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap()
    }

    /// End of that window.
    fn window_to() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 7, 1, 0, 0, 0).unwrap()
    }

    /// An absolute UTC `EventTime` `offset` seconds from `anchor`. UTC, not
    /// floating: the edges below assert exact equality with the bound, which
    /// only means something if the value does not move with `TZ`.
    fn offset_from(anchor: DateTime<Utc>, offset: i64) -> EventTime {
        EventTime::DateTimeUtc(anchor + chrono::Duration::seconds(offset))
    }

    fn task(start: Option<EventTime>, due: Option<EventTime>) -> CalendarItem {
        let mut todo = Todo::new("Window task");
        todo.start = start;
        todo.due = due;
        CalendarItem::Todo(todo)
    }

    /// T-24 (R44, R45, R46, R106). The four §9.9 "core's rule" rows, each at
    /// **both** edges of both bounds, plus the VEVENT control that must not
    /// move. TZ-marked: must pass under `TZ=UTC`, `TZ=Europe/Stockholm` and
    /// `TZ=America/New_York`.
    #[test]
    fn task_window_rule_covers_all_four_date_combinations_at_both_edges_and_events_are_unchanged() {
        let (from, to) = (window_from(), window_to());

        // (name, DTSTART, DUE, expected in-window)
        let cases: Vec<(&str, Option<EventTime>, Option<EventTime>, bool)> = vec![
            // Row N/Y — DUE only: `from < DUE AND to >= DUE`.
            (
                "DUE == from (from < DUE is strict)",
                None,
                Some(offset_from(from, 0)),
                false,
            ),
            (
                "DUE one second after from",
                None,
                Some(offset_from(from, 1)),
                true,
            ),
            (
                "DUE == to (to >= DUE is inclusive)",
                None,
                Some(offset_from(to, 0)),
                true,
            ),
            (
                "DUE one second after to",
                None,
                Some(offset_from(to, 1)),
                false,
            ),
            // Row Y/N — DTSTART only: `from <= DTSTART AND to > DTSTART`.
            (
                "DTSTART one second before from",
                Some(offset_from(from, -1)),
                None,
                false,
            ),
            (
                "DTSTART == from (from <= DTSTART is inclusive)",
                Some(offset_from(from, 0)),
                None,
                true,
            ),
            (
                "DTSTART one second before to",
                Some(offset_from(to, -1)),
                None,
                true,
            ),
            (
                "DTSTART == to (to > DTSTART is strict)",
                Some(offset_from(to, 0)),
                None,
                false,
            ),
            // Row Y/Y — both:
            // `(from < DUE OR from <= DTSTART) AND (to > DTSTART OR to >= DUE)`.
            (
                "both entirely before the window",
                Some(offset_from(from, -2)),
                Some(offset_from(from, -1)),
                false,
            ),
            (
                "DUE == from with DTSTART before it",
                Some(offset_from(from, -2)),
                Some(offset_from(from, 0)),
                false,
            ),
            (
                "DUE one second after from with DTSTART before it",
                Some(offset_from(from, -2)),
                Some(offset_from(from, 1)),
                true,
            ),
            (
                "DTSTART == from and DUE == from",
                Some(offset_from(from, 0)),
                Some(offset_from(from, 0)),
                true,
            ),
            (
                "DTSTART == to and DUE == to",
                Some(offset_from(to, 0)),
                Some(offset_from(to, 0)),
                true,
            ),
            (
                "DTSTART == to with DUE past it",
                Some(offset_from(to, 0)),
                Some(offset_from(to, 5)),
                false,
            ),
            (
                "DTSTART one second before to with DUE past it",
                Some(offset_from(to, -1)),
                Some(offset_from(to, 5)),
                true,
            ),
            (
                "both entirely after the window",
                Some(offset_from(to, 1)),
                Some(offset_from(to, 2)),
                false,
            ),
            // Row N/N — neither: TRUE, unconditionally.
            ("no dates at all", None, None, true),
        ];

        for (name, start, due, expected) in cases {
            assert_eq!(
                task(start, due).has_occurrence_in_range(from, to),
                expected,
                "RFC 4791 §9.9 core's rule (spec §10 R44), case: {name}"
            );
        }

        // R44 row 4 says TRUE *unconditionally*: the same undated task is in
        // window for a range that shares no instant with the one above.
        assert!(
            task(None, None).has_occurrence_in_range(
                Utc.with_ymd_and_hms(1990, 1, 1, 0, 0, 0).unwrap(),
                Utc.with_ymd_and_hms(1990, 2, 1, 0, 0, 0).unwrap(),
            ),
            "R44 row N/N is unconditional, not 'happens to overlap'"
        );

        // AC 2 / R44: COMPLETED and CREATED are never consulted. Both of these
        // would be OUT under a server implementing §9.9 in full — core is
        // deliberately more permissive, never less.
        let mut completed_only = Todo::new("Completed long ago");
        completed_only.completed = Some(EventTime::DateTimeUtc(
            Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap(),
        ));
        assert!(
            CalendarItem::Todo(completed_only).has_occurrence_in_range(from, to),
            "COMPLETED must not be consulted by core (R44)"
        );

        for created in [
            Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap(),
            // After `to`, so even §9.9's `end > CREATED` rescue row is false.
            Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap(),
        ] {
            let mut created_only = Todo::new("Created outside the window");
            created_only.created = Some(created);
            assert!(
                CalendarItem::Todo(created_only).has_occurrence_in_range(from, to),
                "CREATED must not be consulted by core (R44), CREATED={created}"
            );
        }

        // R45 control: the event path is untouched in both directions. Out of
        // window, a known-base event is skipped — no delete, no create.
        let (_tmp_a, calendar_a) = test_calendar();
        let (_tmp_b, calendar_b) = test_calendar();
        let event = test_event();
        let mut sync_bases = SyncBases::new();
        sync_bases.insert_event_base(event.event_instance_id(), event.clone());

        let out_of_window = CalendarDiff::compute(
            vec![CalendarEvent::create(&calendar_a, event.clone()).unwrap()],
            vec![],
            &sync_bases,
            &DateRange {
                from: Some(from),
                to: Some(to),
            },
        );

        assert_eq!(out_of_window.outgoing, vec![]);
        assert_eq!(out_of_window.incoming, vec![]);

        // R45 control, second half: the event arm still goes through
        // `has_occurrence_in_range`, RRULE expansion included. This master
        // starts before the window and has no occurrence *of its own* inside
        // it — only expanded ones — so swapping the arm to the unexpanded
        // `occurs_in_range` would wrongly skip it and lose the delete.
        let (_tmp_c, calendar_c) = test_calendar();
        let mut recurring = Event::new(
            "Ongoing weekly",
            EventTime::Date(chrono::NaiveDate::from_ymd_opt(2025, 1, 1).unwrap()),
        );
        recurring.recurrence = Some(crate::event::Recurrence::new("FREQ=WEEKLY"));
        let mut recurring_bases = SyncBases::new();
        recurring_bases.insert_event_base(recurring.event_instance_id(), recurring.clone());

        let expanded = CalendarDiff::compute(
            vec![CalendarEvent::create(&calendar_c, recurring.clone()).unwrap()],
            vec![],
            &recurring_bases,
            &DateRange {
                from: Some(from),
                to: Some(to),
            },
        );

        assert_eq!(expanded.outgoing, vec![]);
        assert_eq!(
            expanded.incoming,
            vec![EventChange::Delete(recurring.into())]
        );

        // In window, that same event still produces the incoming delete —
        // the polarity R47 warns about, pinned on the event side.
        let in_window = CalendarDiff::compute(
            vec![CalendarEvent::create(&calendar_b, event.clone()).unwrap()],
            vec![],
            &sync_bases,
            &DateRange {
                from: Some(Utc.with_ymd_and_hms(2025, 12, 1, 0, 0, 0).unwrap()),
                to: Some(Utc.with_ymd_and_hms(2026, 2, 1, 0, 0, 0).unwrap()),
            },
        );

        assert_eq!(in_window.outgoing, vec![]);
        assert_eq!(in_window.incoming, vec![EventChange::Delete(event.into())]);
    }

    /// T-25 (R97, A-3). The exact shape of the data-loss bug: a previously
    /// synced undated task, and a remote response that does not mention it.
    /// The file must stay. `has_occurrence_in_range` says TRUE for an undated
    /// task (R44 row 4), so nothing here is protected by the window check —
    /// this is the lock that says absence alone never removes a task.
    #[test]
    fn an_undated_task_with_a_known_base_and_an_empty_remote_is_not_deleted() {
        let (_tmp, calendar) = test_calendar();
        let todo = Todo::new("Undated, already synced");
        let calendar_event = CalendarEvent::create(&calendar, todo.clone()).unwrap();

        let mut sync_bases = SyncBases::new();
        let id = CalendarItem::from(todo).instance_id();
        sync_bases.insert_event_base(id, calendar_event.item().clone());

        let diff = CalendarDiff::compute(
            vec![calendar_event],
            vec![],
            &sync_bases,
            &DateRange {
                from: Some(window_from()),
                to: Some(window_to()),
            },
        );

        assert_eq!(diff.incoming, vec![], "R97: no incoming delete");
        assert_eq!(diff.outgoing, vec![], "R97: no outgoing delete either");
    }

    /// T-26 (R98, A-4). An undated task with no base is a *create*, never a
    /// skip: R44 row N/N makes the window predicate unconditionally true, so
    /// a bounded window must not swallow it.
    ///
    /// **Changed from asserting `outgoing == [Create(Todo)]`.** A task is
    /// never outgoing now (R70), so the create is withheld — but *withheld*
    /// and *skipped by the window* are different states, and the withheld
    /// count is what tells them apart. Assert the count, or R44's window
    /// predicate could silently invert for undated tasks again.
    #[test]
    fn an_undated_task_with_no_base_is_a_withheld_create_not_a_window_skip() {
        let (_tmp, calendar) = test_calendar();
        let todo = Todo::new("Undated, never synced");
        let calendar_event = CalendarEvent::create(&calendar, todo.clone()).unwrap();

        let diff = CalendarDiff::compute(
            vec![calendar_event],
            vec![],
            &SyncBases::new(),
            &DateRange {
                from: Some(window_from()),
                to: Some(window_to()),
            },
        );

        assert_eq!(
            diff.withheld_task_pushes, 1,
            "R98: an undated task with no base reaches the outgoing set (and is \
             then withheld by R70), rather than being skipped by the window"
        );
        assert_eq!(diff.outgoing, vec![]);
        assert_eq!(diff.incoming, vec![]);
        let _ = todo;
    }

    /// P1-03/S3. A provider that advertises `tasks.write` must actually let a
    /// task through: `compute_with`'s withholding is conditional on the
    /// capability flag, not unconditional (C-3).
    #[test]
    fn a_task_change_is_outgoing_when_task_writes_are_advertised() {
        let (_tmp, calendar) = test_calendar();
        let todo = Todo::new("Hand in the assignment");
        let calendar_event = CalendarEvent::create(&calendar, todo.clone()).unwrap();

        let diff = CalendarDiff::compute_with(
            vec![calendar_event],
            vec![],
            &SyncBases::new(),
            &DateRange::default(),
            Deletes::Allowed,
            TaskPushes::Allowed,
            TaskReads::NotAdvertised,
        );

        assert_eq!(
            diff.outgoing,
            vec![EventChange::Create(CalendarItem::Todo(todo))]
        );
        assert_eq!(diff.withheld_task_pushes, 0);
    }

    /// W1-C2 (P1-03/S3, acceptance criterion 4). `Create` above is not proof
    /// that `Update` also reaches the outgoing set — cover it directly.
    #[test]
    fn a_task_update_is_outgoing_when_task_writes_are_advertised() {
        let (_tmp, calendar) = test_calendar();
        let remote_todo = Todo::new("Hand in the assignment");

        let mut local_todo = remote_todo.clone();
        local_todo.summary = Some("Hand in the assignment (revised)".to_string());
        let calendar_event = CalendarEvent::create(&calendar, local_todo.clone()).unwrap();

        let mut sync_bases = SyncBases::new();
        sync_bases.insert_known_event_id(CalendarItem::from(remote_todo.clone()).instance_id());

        let diff = CalendarDiff::compute_with(
            vec![calendar_event],
            vec![RemoteEvent::new(remote_todo.clone())],
            &sync_bases,
            &DateRange::default(),
            Deletes::Allowed,
            TaskPushes::Allowed,
            TaskReads::NotAdvertised,
        );

        assert_eq!(
            diff.outgoing,
            vec![EventChange::Update {
                from: CalendarItem::Todo(remote_todo),
                to: CalendarItem::Todo(local_todo),
            }]
        );
        assert_eq!(diff.withheld_task_pushes, 0);
    }

    /// W1-C2 (P1-03/S3, acceptance criterion 4). Same gap, for `Delete`: a
    /// task known from a prior sync but gone locally must still reach the
    /// outgoing set when the provider advertises `tasks.write`.
    #[test]
    fn a_task_delete_is_outgoing_when_task_writes_are_advertised() {
        let todo = Todo::new("Hand in the assignment");

        let mut sync_bases = SyncBases::new();
        sync_bases.insert_event_base(CalendarItem::from(todo.clone()).instance_id(), todo.clone());

        let diff = CalendarDiff::compute_with(
            vec![],
            vec![RemoteEvent::new(todo.clone())],
            &sync_bases,
            &DateRange::default(),
            Deletes::Allowed,
            TaskPushes::Allowed,
            TaskReads::NotAdvertised,
        );

        assert_eq!(
            diff.outgoing,
            vec![EventChange::Delete(CalendarItem::Todo(todo))]
        );
        assert_eq!(diff.withheld_task_pushes, 0);
    }

    /// The honest `false` beside the honest `true` above: without the
    /// capability, the task is withheld and an event change in the same diff
    /// still goes out (R70 is scoped to tasks, never to the whole diff).
    #[test]
    fn a_task_change_is_withheld_when_task_writes_are_not_advertised() {
        let (_tmp, calendar) = test_calendar();
        let todo = Todo::new("Hand in the assignment");
        let event = test_event();
        let todo_event = CalendarEvent::create(&calendar, todo.clone()).unwrap();
        let event_event = calendar.create_event(event.clone()).unwrap();

        let diff = CalendarDiff::compute_with(
            vec![todo_event, event_event],
            vec![],
            &SyncBases::new(),
            &DateRange::default(),
            Deletes::Allowed,
            TaskPushes::Withheld,
            TaskReads::NotAdvertised,
        );

        assert_eq!(diff.outgoing, vec![EventChange::Create(event.into())]);
        assert_eq!(diff.withheld_task_pushes, 1);
    }

    /// R1/R2 control: an event's own outgoing status never depends on the
    /// task capability, in either direction.
    #[test]
    fn an_event_change_is_outgoing_regardless_of_task_capability() {
        for task_pushes in [TaskPushes::Allowed, TaskPushes::Withheld] {
            let (_tmp, calendar) = test_calendar();
            let event = test_event();
            let calendar_event = calendar.create_event(event.clone()).unwrap();

            let diff = CalendarDiff::compute_with(
                vec![calendar_event],
                vec![],
                &SyncBases::new(),
                &DateRange::default(),
                Deletes::Allowed,
                task_pushes,
                TaskReads::NotAdvertised,
            );

            assert_eq!(diff.outgoing, vec![EventChange::Create(event.into())]);
        }
    }

    /// C-3's last clause: a read-only calendar pushes nothing at all, so
    /// naming *tasks* there would report the wrong restriction even though
    /// the provider genuinely allows task writes.
    #[test]
    fn a_discarded_outgoing_set_reports_no_task_restriction_even_when_task_writes_are_allowed() {
        let (_tmp, calendar) = test_calendar();
        let todo = Todo::new("Hand in the assignment");
        let calendar_event = CalendarEvent::create(&calendar, todo).unwrap();

        let mut diff = CalendarDiff::compute_with(
            vec![calendar_event],
            vec![],
            &SyncBases::new(),
            &DateRange::default(),
            Deletes::Allowed,
            TaskPushes::Allowed,
            TaskReads::NotAdvertised,
        );
        assert_eq!(diff.withheld_task_pushes, 0);

        diff.discard_outgoing();

        assert!(diff.outgoing.is_empty());
        assert_eq!(diff.withheld_task_pushes(), 0);
    }

    /// P2-06/C-7, the ticket's deliverable. A provider that explicitly
    /// advertises `tasks.read` has told core it can honestly enumerate
    /// tasks, so a zero-task response is a genuine deletion, not version
    /// skew — the belt must not report a suppression it did not perform.
    #[test]
    fn a_task_reading_provider_reporting_no_tasks_deletes_the_local_task() {
        let (_tmp, calendar) = test_calendar();

        let task = Todo::new("Renew the parking permit");
        let local_task = CalendarEvent::create(&calendar, task.clone()).unwrap();
        let event = test_event();
        let local_event = calendar.create_event(event.clone()).unwrap();

        let mut sync_bases = SyncBases::new();
        sync_bases.insert_event_base(CalendarItem::from(task.clone()).instance_id(), task.clone());

        // The remote response carries only the (untouched) event — no tasks
        // at all — which is exactly the shape `tasks.read=true` makes
        // trustworthy.
        let diff = CalendarDiff::compute_with(
            vec![local_task, local_event],
            vec![RemoteEvent::new(event.clone())],
            &sync_bases,
            &DateRange::default(),
            Deletes::Allowed,
            TaskPushes::Withheld,
            TaskReads::Advertised,
        );

        assert_eq!(
            diff.incoming,
            vec![EventChange::Delete(CalendarItem::Todo(task))],
            "C-7: an advertised tasks.read makes a zero-task response an honest delete",
        );
        assert!(
            !diff.suppressed_task_deletes(),
            "the belt did not withhold anything here, so the signal must be false",
        );
    }

    /// P2-06/C-8, "two guards, not one". `tasks.read=true` lifts the
    /// version-skew veto; it must not be mistaken for lifting the
    /// `sync_bases.get(&id).is_some()` guard that actually authorises a
    /// delete. A task with no base is a never-synced item, not a delete,
    /// regardless of capability.
    #[test]
    fn a_task_reading_provider_still_does_not_delete_a_never_synced_task() {
        let (_tmp, calendar) = test_calendar();
        let task = Todo::new("Brand new, never synced");
        let local_task = CalendarEvent::create(&calendar, task.clone()).unwrap();

        let diff = CalendarDiff::compute_with(
            vec![local_task],
            vec![],
            &SyncBases::new(),
            &DateRange::default(),
            Deletes::Allowed,
            TaskPushes::Allowed,
            TaskReads::Advertised,
        );

        assert_eq!(
            diff.outgoing,
            vec![EventChange::Create(CalendarItem::Todo(task))],
            "a never-synced task must become an outgoing create, never a delete",
        );
        assert!(!diff.suppressed_task_deletes());
    }

    /// P2-06/C-8's incomplete-enumeration hazard, the local-listing half:
    /// `Deletes::Suppressed` already vetoes every delete when the local
    /// listing is known-incomplete, and that veto must survive a lifted
    /// task-read belt untouched. Lifting one guard must never be read as
    /// lifting both (see the never-synced-task test above for the other
    /// guard).
    #[test]
    fn a_task_reading_provider_does_not_delete_tasks_when_the_local_listing_is_incomplete() {
        let (_tmp, calendar) = test_calendar();

        let task = Todo::new("Renew the parking permit");
        let local_task_event = CalendarEvent::create(&calendar, task.clone()).unwrap();

        let mut sync_bases = SyncBases::new();
        sync_bases.insert_event_base(CalendarItem::from(task.clone()).instance_id(), task.clone());

        let mut listing = CalendarListing::default();
        listing.push_event(local_task_event);
        listing.push_skipped(
            calendar.path().join("broken.ics"),
            CalendarError::MasterNotFound("broken".to_string()),
        );
        assert!(listing.is_incomplete());

        // A remote response with no tasks at all, from a provider that
        // explicitly advertises `tasks.read` — the shape that, absent the
        // local-listing guard, `a_task_reading_provider_reporting_no_tasks_deletes_the_local_task`
        // proves core is now willing to read as a real delete.
        let diff = CalendarDiff::compute_from_listing(
            listing,
            vec![],
            &sync_bases,
            &DateRange::default(),
            TaskPushes::Withheld,
            TaskReads::Advertised,
        );

        assert!(
            !diff
                .incoming
                .iter()
                .chain(diff.outgoing.iter())
                .any(|c| matches!(c, EventChange::Delete(CalendarItem::Todo(_)))),
            "an unreadable local file must keep every task delete withheld even when \
tasks.read=true, got incoming {:?} outgoing {:?}",
            diff.incoming,
            diff.outgoing,
        );
    }

    fn task_reading_calendar_config() -> CalendarConfig {
        CalendarConfig::new(
            Some("P2-06 test".to_string()),
            None,
            Some(false),
            Some(test_remote_config("test-provider")),
        )
    }

    /// P2-06/C-8, belt-stays-on #1. A capabilities payload with no `tasks`
    /// object at all — the shape a partially-upgraded provider sends. Reading
    /// absence as support is the exact deletion C-8 forbids: the belt must
    /// stay on exactly as it would for an honest `tasks.read=false`.
    #[tokio::test]
    async fn an_absent_tasks_read_capability_keeps_the_belt_on() {
        let (_tmp, caldir) = test_caldir();
        let mut calendar = caldir
            .create_calendar("writable-cal", Some(task_reading_calendar_config()))
            .unwrap();

        let task = Todo::new("Renew the parking permit");
        let event = test_event();
        calendar.create_event(task.clone()).unwrap();
        calendar.create_event(event.clone()).unwrap();
        calendar
            .record_sync_bases(vec![
                CalendarItem::Todo(task.clone()),
                CalendarItem::Event(event.clone()),
            ])
            .unwrap();

        let mock = test_mock_provider();
        // Older-shape payload: an `events` object, no `tasks` object.
        mock.reply_raw_data(serde_json::json!({
            "events": { "read": true, "write": true },
        }));
        mock.reply::<rpc::ListEvents>(vec![CalendarItem::Event(event)]);
        let remote = crate::Remote::new(mock.provider(), test_remote_params());

        let mut connection = Connection::new(calendar, remote);
        let diff = connection.diff(&DateRange::default()).await.unwrap();

        assert!(
            !diff
                .incoming()
                .iter()
                .chain(diff.outgoing())
                .any(|c| matches!(c, EventChange::Delete(CalendarItem::Todo(_)))),
            "an absent tasks field must not be read as tasks.read=true, got incoming {:?} \
outgoing {:?}",
            diff.incoming(),
            diff.outgoing(),
        );
        assert!(
            diff.suppressed_task_deletes(),
            "the belt must report that it withheld a delete here",
        );
    }

    /// P2-06/C-8, belt-stays-on #2. A capabilities transport failure is the
    /// mode most likely to be handled with `unwrap_or_default()` — and
    /// `Default` on `Capability` is `false` for both `read` and `write`; only
    /// an explicit test says which reading actually shipped.
    #[tokio::test]
    async fn an_errored_capability_lookup_keeps_the_belt_on() {
        use crate::provider::transport::ProviderTransportError;

        let (_tmp, caldir) = test_caldir();
        let mut calendar = caldir
            .create_calendar("writable-cal", Some(task_reading_calendar_config()))
            .unwrap();

        let task = Todo::new("Renew the parking permit");
        calendar.create_event(task.clone()).unwrap();
        calendar
            .record_sync_bases(vec![CalendarItem::Todo(task.clone())])
            .unwrap();

        let mock = test_mock_provider();
        mock.reply_error(ProviderTransportError::Io(std::io::Error::other(
            "pipe closed",
        )));
        mock.reply::<rpc::ListEvents>(vec![]);
        let remote = crate::Remote::new(mock.provider(), test_remote_params());

        let mut connection = Connection::new(calendar, remote);
        let diff = connection.diff(&DateRange::default()).await.unwrap();

        assert!(
            !diff
                .incoming()
                .iter()
                .chain(diff.outgoing())
                .any(|c| matches!(c, EventChange::Delete(CalendarItem::Todo(_)))),
            "a capabilities transport error must keep the belt on, got incoming {:?} \
outgoing {:?}",
            diff.incoming(),
            diff.outgoing(),
        );
        assert!(diff.suppressed_task_deletes());
    }

    /// P2-06/C-8/C-4, belt-stays-on #3, the real old-binary path. Every
    /// provider binary installed today answers `capabilities` with
    /// `unsupported_method` — if that deleted, the first upgrade would delete
    /// every user's tasks against every un-upgraded provider.
    #[tokio::test]
    async fn an_unknown_capabilities_method_keeps_the_belt_on() {
        use crate::ProviderSlug;
        use crate::provider::Provider;
        use crate::provider::transport::mock_transport::MockTransport;
        use std::sync::Arc;

        let (_tmp, caldir) = test_caldir();
        let mut calendar = caldir
            .create_calendar("writable-cal", Some(task_reading_calendar_config()))
            .unwrap();

        let task = Todo::new("Renew the parking permit");
        calendar.create_event(task.clone()).unwrap();
        calendar
            .record_sync_bases(vec![CalendarItem::Todo(task.clone())])
            .unwrap();

        let transport = Arc::new(MockTransport::empty());
        transport.set_response(
            r#"{"status":"error","error":"unknown command: capabilities","code":"unsupported_method"}"#,
        );
        transport.set_response(serde_json::json!({"status": "success", "data": []}).to_string());
        let provider = Provider::with_transport(ProviderSlug::from("test-provider"), transport);
        let remote = crate::Remote::new(provider, test_remote_params());

        let mut connection = Connection::new(calendar, remote);
        let diff = connection.diff(&DateRange::default()).await.unwrap();

        assert!(
            !diff
                .incoming()
                .iter()
                .chain(diff.outgoing())
                .any(|c| matches!(c, EventChange::Delete(CalendarItem::Todo(_)))),
            "an unknown capabilities method must keep the belt on, got incoming {:?} \
outgoing {:?}",
            diff.incoming(),
            diff.outgoing(),
        );
        assert!(diff.suppressed_task_deletes());
    }

    /// T-27 (R99, A-5). Absence stays meaningful. The remote response is
    /// non-empty and *does* carry a task, so R49's version-skew belt is off;
    /// a tracked task missing from it is a real incoming delete.
    #[test]
    fn a_task_omitted_from_a_non_empty_remote_list_is_still_deleted() {
        let (_tmp, calendar) = test_calendar();

        let gone = Todo::new("Deleted on the server");
        let survivor = Todo::new("Still on the server");

        let local_gone = CalendarEvent::create(&calendar, gone.clone()).unwrap();
        let local_survivor = CalendarEvent::create(&calendar, survivor.clone()).unwrap();

        let mut sync_bases = SyncBases::new();
        sync_bases.insert_event_base(CalendarItem::from(gone.clone()).instance_id(), gone.clone());

        let diff = CalendarDiff::compute(
            vec![local_gone, local_survivor],
            vec![RemoteEvent::new(survivor)],
            &sync_bases,
            &DateRange {
                from: Some(window_from()),
                to: Some(window_to()),
            },
        );

        assert_eq!(
            diff.incoming,
            vec![EventChange::Delete(CalendarItem::Todo(gone))],
            "R99: a task missing from a task-carrying remote list is deleted"
        );
        assert_eq!(diff.outgoing, vec![]);
    }

    /// T-30 (R49, R50, R72). The version-skew belt. A pre-VTODO provider
    /// still filters on VEVENT, so its response carries events but **no
    /// tasks at all**. That total absence is skew, not a delete.
    ///
    /// The remote list here is deliberately **non-empty** — it carries an
    /// event — so a belt keyed on "the remote response is empty" instead of
    /// on the *task* count would not fire and the tasks would be deleted.
    ///
    /// Pair-read with `T-27`: a remote list that *does* carry a task still
    /// deletes the task missing from it. The two are each other's boundary.
    #[test]
    fn a_task_less_remote_response_suppresses_all_task_deletes() {
        let (_tmp, calendar) = test_calendar();

        // Two tracked tasks, neither mentioned by the remote.
        let first_task = Todo::new("Hand in the assignment");
        let second_task = Todo::new("Renew the parking permit");
        let local_first = CalendarEvent::create(&calendar, first_task.clone()).unwrap();
        let local_second = CalendarEvent::create(&calendar, second_task.clone()).unwrap();

        // One tracked event, also unmentioned: its delete is real.
        let deleted_event = test_event();
        let local_event = calendar.create_event(deleted_event.clone()).unwrap();

        // The response is not empty — it just holds no tasks.
        let remote_event = Event::new("Still on the server", test_event_time());

        let mut sync_bases = SyncBases::new();
        sync_bases.insert_event_base(
            CalendarItem::from(first_task.clone()).instance_id(),
            first_task,
        );
        sync_bases.insert_event_base(
            CalendarItem::from(second_task.clone()).instance_id(),
            second_task,
        );
        sync_bases.insert_event_base(deleted_event.event_instance_id(), deleted_event.clone());

        let diff = CalendarDiff::compute(
            vec![local_first, local_second, local_event],
            vec![RemoteEvent::new(remote_event.clone())],
            &sync_bases,
            &DateRange::default(),
        );

        // 1. The belt: not one task delete, in either direction.
        let task_deletes: Vec<_> = diff
            .incoming
            .iter()
            .chain(diff.outgoing.iter())
            .filter(|change| matches!(change, EventChange::Delete(CalendarItem::Todo(_))))
            .collect();
        assert_eq!(
            task_deletes,
            Vec::<&EventChange>::new(),
            "R49: a task-less remote response deletes no task"
        );

        // 2. Task-scoped, never a blanket suppression: the event's own delete
        // still fires, and the remote's create still flows.
        assert_eq!(
            diff.incoming,
            vec![
                EventChange::Delete(deleted_event.into()),
                EventChange::Create(remote_event.into()),
            ],
            "R1/R2: VEVENT delete detection is untouched by the belt"
        );
        assert_eq!(diff.outgoing, vec![]);

        // R49/R50: the suppression must be observable, not silent. Wording is
        // deferred to the CLI surface; the signal is not.
        assert!(
            diff.suppressed_task_deletes(),
            "R49: the belt reports that it fired"
        );
    }

    /// The belt's off-polarity, in both of AC 1's halves. If
    /// `suppressed_task_deletes` could not be **false**, T-30's signal would
    /// be a constant and the CLI would warn on every single sync — including
    /// the overwhelmingly common one where the calendar simply has no tasks.
    #[test]
    fn the_version_skew_signal_is_off_unless_a_task_delete_was_actually_withheld() {
        // Half 1: a calendar of events only. The remote reports no tasks
        // because there are none — that is not skew.
        let (_events_tmp, events_only) = test_calendar();
        let tracked_event = test_event();
        let local_event = events_only.create_event(tracked_event.clone()).unwrap();

        let mut event_bases = SyncBases::new();
        event_bases.insert_event_base(tracked_event.event_instance_id(), tracked_event.clone());

        let no_tasks_at_all = CalendarDiff::compute(
            vec![local_event],
            vec![],
            &event_bases,
            &DateRange::default(),
        );

        assert!(
            !no_tasks_at_all.suppressed_task_deletes(),
            "R49: no local task with a known base means nothing was withheld"
        );
        assert_eq!(
            no_tasks_at_all.incoming,
            vec![EventChange::Delete(tracked_event.into())],
            "R1/R2: the event delete is unaffected"
        );

        // Half 2: the remote does carry a task, so the belt is off outright.
        let (_tmp, calendar) = test_calendar();

        let gone = Todo::new("Deleted on the server");
        let survivor = Todo::new("Still on the server");
        let local_gone = CalendarEvent::create(&calendar, gone.clone()).unwrap();
        let local_survivor = CalendarEvent::create(&calendar, survivor.clone()).unwrap();

        let mut sync_bases = SyncBases::new();
        sync_bases.insert_event_base(CalendarItem::from(gone.clone()).instance_id(), gone.clone());

        let diff = CalendarDiff::compute(
            vec![local_gone, local_survivor],
            vec![RemoteEvent::new(survivor)],
            &sync_bases,
            &DateRange::default(),
        );

        assert!(
            !diff.suppressed_task_deletes(),
            "R49: the belt is off when the remote reports tasks"
        );
        assert_eq!(diff.incoming, vec![EventChange::Delete(gone.into())]);
    }

    /// T-28 (R42). Both skip-rules key on `CANCELLED` only. If `COMPLETED`
    /// ever joined the "missing locally + remote cancelled" rule the MVP's own
    /// `STATUS:COMPLETED` fixture would never be pulled.
    #[test]
    fn a_remote_completed_task_absent_locally_is_created_and_both_cancelled_drift_is_ignored() {
        let mut completed = Todo::new("Hand in the assignment");
        completed.status = TodoStatus::Completed;

        let created = CalendarDiff::compute(
            vec![],
            vec![RemoteEvent::new(completed.clone())],
            &SyncBases::new(),
            &DateRange::default(),
        );

        assert_eq!(
            created.incoming,
            vec![EventChange::Create(CalendarItem::Todo(completed))],
            "R42: COMPLETED is not CANCELLED — the task must be created locally"
        );
        assert_eq!(created.outgoing, vec![]);

        // Both cancelled, with field drift: historical, so no churn.
        let (_tmp, calendar) = test_calendar();
        let mut local_cancelled = Todo::new("Cancelled locally");
        local_cancelled.status = TodoStatus::Cancelled;
        let calendar_event = CalendarEvent::create(&calendar, local_cancelled.clone()).unwrap();

        let mut remote_cancelled = local_cancelled.clone();
        remote_cancelled.summary = Some("Cancelled remotely, renamed".to_string());
        remote_cancelled.description = Some("drift".to_string());

        let drifted = CalendarDiff::compute(
            vec![calendar_event],
            vec![RemoteEvent::new(remote_cancelled)],
            &SyncBases::new(),
            &DateRange::default(),
        );

        assert_eq!(drifted.outgoing, vec![], "R42: both cancelled, no churn");
        assert_eq!(drifted.incoming, vec![]);
    }

    /// T-31 (R36 hazard 3, A-6). A VEVENT and a VTODO sharing a UID are two
    /// identities on **both** sides.
    ///
    /// Locally: only one of the pair is tracked, so exactly **one** delete
    /// fires — against the file whose identity is actually in the bases. A
    /// kind-agnostic key makes the base lookup true on both loop iterations
    /// and fires two (hazard 3).
    ///
    /// Remotely: a same-UID pair must survive `.collect()` as two entries and
    /// produce two creates. A kind-agnostic key silently merges them and one
    /// component is lost (hazard 2).
    #[test]
    fn a_shared_uid_pair_produces_exactly_one_delete_against_the_right_file() {
        let (_tmp, calendar) = test_calendar();

        let uid = |value: &str| crate::event::EventUid::new(value.to_string());

        let mut event = test_event();
        event.uid = uid("shared-123@u-crawler");

        let mut todo = Todo::new("Same UID, different component");
        todo.uid = uid("shared-123@u-crawler");

        let local_event = CalendarEvent::create(&calendar, event.clone()).unwrap();
        let local_todo = CalendarEvent::create(&calendar, todo.clone()).unwrap();

        // A second same-UID pair, this one only on the remote. It also keeps
        // R49's task-less belt off, so the local delete below is authorized.
        let mut remote_event = test_event();
        remote_event.uid = uid("remote-pair@u-crawler");
        remote_event.summary = Some("Remote event of the pair".to_string());

        let mut remote_todo = Todo::new("Remote task of the pair");
        remote_todo.uid = uid("remote-pair@u-crawler");

        // Only the local TASK identity is tracked.
        let mut sync_bases = SyncBases::new();
        sync_bases.insert_event_base(CalendarItem::from(todo.clone()).instance_id(), todo.clone());

        let diff = CalendarDiff::compute(
            vec![local_event, local_todo],
            vec![
                RemoteEvent::new(remote_event.clone()),
                RemoteEvent::new(remote_todo.clone()),
            ],
            &sync_bases,
            &DateRange::default(),
        );

        assert_eq!(
            diff.incoming
                .iter()
                .filter(|c| matches!(c, EventChange::Delete(_)))
                .count(),
            1,
            "R36 hazard 3: one tracked identity means exactly one delete"
        );

        assert_eq!(
            diff.incoming,
            vec![
                EventChange::Delete(CalendarItem::Todo(todo)),
                EventChange::Create(CalendarItem::Event(remote_event)),
                EventChange::Create(CalendarItem::Todo(remote_todo)),
            ],
            "the delete lands on the tracked task, and the remote same-UID pair \
             stays two items (R36 hazard 2)"
        );

        assert_eq!(
            diff.outgoing,
            vec![EventChange::Create(CalendarItem::Event(event))],
            "the untracked local sibling is a create, not a second delete"
        );
    }

    /// T-32 (R41). The Push-biased `(Some, None)` fallback is existing VEVENT
    /// behaviour and must not invert for tasks: a wrong Pull destroys the
    /// user's local edit with no recovery.
    ///
    /// **Changed from asserting `outgoing == [Update { .. }]`.** A task is
    /// never outgoing now (R70), so the push is withheld — but the direction
    /// decision it withholds is exactly what protects the local edit: a Pull
    /// would overwrite the file, and the assertion that `incoming` is empty is
    /// the one that catches an inverted bias.
    #[test]
    fn a_task_with_no_last_modified_and_no_sequence_pushes_rather_than_pulls() {
        let (_tmp, calendar) = test_calendar();

        let mut local = Todo::new("Edited locally");
        local.last_modified = None;
        local.sequence = 0;
        let calendar_event = CalendarEvent::create(&calendar, local.clone()).unwrap();

        let mut remote = local.clone();
        remote.summary = Some("Stale remote copy".to_string());
        remote.last_modified = None;
        remote.sequence = 0;

        let diff = CalendarDiff::compute(
            vec![calendar_event],
            vec![RemoteEvent::new(remote.clone())],
            &SyncBases::new(),
            &DateRange::default(),
        );

        assert_eq!(
            diff.withheld_task_pushes, 1,
            "R41: no LAST-MODIFIED and no SEQUENCE stays Push-biased for tasks"
        );
        assert_eq!(diff.incoming, vec![], "R41: never an incoming update here");
    }

    /// C-3's named risk: `withhold_task_pushes` is called from both
    /// `compute_with` and the `#[cfg(test)]` `from_changes` constructor. The
    /// moment the withholding decision takes a parameter, this is where the
    /// two paths could drift apart — a test diff holding an outgoing shape
    /// production can never produce. Pins that both reach the same withheld
    /// outgoing set and the same withheld count for a non-`true` capability
    /// value (`TaskPushes::Withheld`, the shape every failure mode collapses
    /// into upstream in `Connection::diff`).
    #[test]
    fn from_changes_withholds_tasks_for_an_unknown_capability() {
        let (_tmp, calendar) = test_calendar();
        let event = test_event();
        let event_calendar_event = calendar.create_event(event.clone()).unwrap();
        let todo = Todo::new("Hand in the assignment");
        let todo_calendar_event = CalendarEvent::create(&calendar, todo.clone()).unwrap();

        let via_compute_with = CalendarDiff::compute_with(
            vec![event_calendar_event, todo_calendar_event],
            vec![],
            &SyncBases::new(),
            &DateRange::default(),
            Deletes::Allowed,
            TaskPushes::Withheld,
            TaskReads::NotAdvertised,
        );

        let via_from_changes = CalendarDiff::from_changes(
            vec![
                EventChange::Create(event.into()),
                EventChange::Create(todo.into()),
            ],
            vec![],
        );

        assert_eq!(
            via_compute_with.outgoing, via_from_changes.outgoing,
            "compute_with and from_changes must reach the same withholding decision",
        );
        assert_eq!(
            via_compute_with.withheld_task_pushes,
            via_from_changes.withheld_task_pushes,
        );
    }
}
