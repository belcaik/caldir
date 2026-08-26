mod error;

use std::collections::HashMap;

use crate::calendar::{CalendarError, SyncBases};
use crate::diff::EventChange;
use crate::event::{ComponentKind, EventInstanceId, EventUid};
use crate::item::CalendarItem;
use crate::{Calendar, CalendarDiff, CalendarEvent, DateRange, Remote, RemoteEvent};
use error::ConnectionError;

/// A connection is a [local calendar] + [remote calendar] pair
pub struct Connection {
    local: Calendar,
    remote: Remote,
}

impl Connection {
    pub fn new(local: Calendar, remote: Remote) -> Self {
        Self { local, remote }
    }

    pub fn local(&self) -> &Calendar {
        &self.local
    }

    pub fn remote(&self) -> &Remote {
        &self.remote
    }

    pub fn read_only(&self) -> bool {
        self.local
            .config()
            .and_then(|c| c.read_only())
            .unwrap_or(false)
    }

    pub async fn diff(&mut self, range: &DateRange) -> Result<CalendarDiff, ConnectionError> {
        let local_listing = self.local().events()?;
        let remote_events = self.remote().list_events(range).await?;

        // State migration: in-sync pairs never produce a change to apply, so
        // this is the only place their base can be recorded. Without it,
        // legacy known-id entries would sit on the mtime fallback forever.
        let backfill = bases_to_backfill(
            local_listing.events(),
            &remote_events,
            self.local.state().sync_bases(),
        );
        if !backfill.is_empty() {
            self.local.record_sync_bases(backfill)?;
        }

        let sync_bases = self.local().state().sync_bases();

        let mut diff =
            CalendarDiff::compute_from_listing(local_listing, remote_events, sync_bases, range);

        if self.read_only() {
            diff.discard_outgoing();
        }

        Ok(diff)
    }

    // pull
    pub fn apply_incoming_diff(&mut self, diff: &CalendarDiff) -> Result<(), ConnectionError> {
        let mut events_by_instance_id = self.events_by_instance_id()?;

        let mut sync_bases = Vec::new();

        // Same partial-failure flush pattern as `apply_outgoing_diff`: a
        // local-fs error mid-loop must not drop changes already applied to disk.
        let loop_result = pull_incoming_changes(
            &self.local,
            diff,
            &mut events_by_instance_id,
            &mut sync_bases,
        );

        let record_result = self.local.record_sync_bases(sync_bases);

        loop_result?;
        record_result?;
        Ok(())
    }

    /// Refuse the whole outgoing push if it holds a change the wire cannot
    /// carry — before anything is sent, and before a caller renders the diff
    /// as work about to happen.
    ///
    /// Public so `caldir push` can ask *first* and not paint a refused item
    /// with the green `+` that means "pushed". `apply_outgoing_diff` runs the
    /// same guard regardless, so skipping this can never push a partial diff.
    pub fn check_outgoing(&self, diff: &CalendarDiff) -> Result<(), ConnectionError> {
        refuse_unpushable(diff, &self.events_by_instance_id()?)
    }

    fn events_by_instance_id(
        &self,
    ) -> Result<HashMap<EventInstanceId, CalendarEvent>, ConnectionError> {
        Ok(self
            .local
            .events()?
            .into_events()
            .into_iter()
            // Keyed by the kind-carrying identity (R36), so a task is
            // reachable here and an event/task UID pair never collides.
            .map(|e| (e.item().instance_id(), e))
            .collect())
    }

    // push
    pub async fn apply_outgoing_diff(
        &mut self,
        diff: &CalendarDiff,
    ) -> Result<(), ConnectionError> {
        let mut events_by_instance_id = self.events_by_instance_id()?;

        let mut sync_bases = Vec::new();

        // Handles mid-loop errors gracefully
        let loop_result = push_outgoing_changes(
            &self.remote,
            diff,
            &mut events_by_instance_id,
            &mut sync_bases,
        )
        .await;

        let record_result = self.local.record_sync_bases(sync_bases);

        loop_result?;
        record_result?;
        Ok(())
    }

    // discard
    pub fn discard_outgoing_diff(&self, diff: &CalendarDiff) -> Result<(), ConnectionError> {
        let mut events_by_instance_id = self.events_by_instance_id()?;

        for change in diff.outgoing() {
            match change {
                EventChange::Create(item) => {
                    if let Some(cal_event) = events_by_instance_id.remove(&item.instance_id()) {
                        cal_event.delete().map_err(CalendarError::from)?;
                    }
                }
                EventChange::Update { from, to } => {
                    if let Some(cal_event) = events_by_instance_id.get_mut(&to.instance_id()) {
                        cal_event
                            .update(from.clone())
                            .map_err(CalendarError::from)?;
                    }
                }
                EventChange::Delete(item) => {
                    let cal_event = self.local.create_event(item.clone())?;
                    events_by_instance_id.insert(item.instance_id(), cal_event);
                }
            }
        }

        Ok(())
    }
}

/// Events present and identical on both sides whose base is missing (legacy
/// known-id entry) or stale. Both sides agreeing *is* the base — record it.
/// Pairs with no sync state at all are left alone: they were never synced,
/// and recording a base would silently change their delete semantics.
fn bases_to_backfill(
    local_events: &[CalendarEvent],
    remote_events: &[RemoteEvent],
    sync_bases: &SyncBases,
) -> Vec<CalendarItem> {
    let local_by_id: HashMap<_, _> = local_events
        .iter()
        .map(|e| (e.item().instance_id(), e.item()))
        .collect();

    let mut backfill = Vec::new();

    for remote in remote_events {
        let remote = remote.item();
        let id = remote.instance_id();

        let in_sync = local_by_id.get(&id).is_some_and(|local| *local == remote);

        let base_is_current = match sync_bases.get(&id) {
            None => continue,
            Some(Some(base)) => &**base == remote,
            Some(None) => false,
        };

        if in_sync && !base_is_current {
            backfill.push(remote.clone());
        }
    }

    backfill
}

fn pull_incoming_changes(
    local: &Calendar,
    diff: &CalendarDiff,
    events_by_instance_id: &mut HashMap<EventInstanceId, CalendarEvent>,
    sync_bases: &mut Vec<CalendarItem>,
) -> Result<(), ConnectionError> {
    for change in diff.incoming() {
        match change {
            EventChange::Create(item) => {
                let cal_event = local.create_event(item.clone())?;
                let id = item.instance_id();
                events_by_instance_id.insert(id, cal_event);
                sync_bases.push(item.clone());
            }
            EventChange::Update { to, .. } => {
                if let Some(cal_event) = events_by_instance_id.get_mut(&to.instance_id()) {
                    cal_event.update(to.clone()).map_err(CalendarError::from)?;
                }
                sync_bases.push(to.clone());
            }
            EventChange::Delete(item) => {
                if let Some(cal_event) = events_by_instance_id.remove(&item.instance_id()) {
                    cal_event.delete().map_err(CalendarError::from)?;
                }
            }
        }
    }

    Ok(())
}

/// The UID of an outgoing event/task pair, if the outgoing set holds one.
///
/// Keyed on `(uid, kind)`: two changes touching the same UID under *different*
/// kinds are the pair. Same-UID, same-kind changes — a master and its
/// recurrence override — are ordinary sync and must stay untouched (R1/R2).
fn same_uid_pair(diff: &CalendarDiff) -> Option<EventUid> {
    let mut kinds_by_uid: HashMap<&EventUid, ComponentKind> = HashMap::new();

    for change in diff.outgoing() {
        let item = written_item(change);
        let kind = item.instance_id().kind();

        match kinds_by_uid.insert(item.uid(), kind) {
            Some(seen) if seen != kind => return Some(item.uid().clone()),
            _ => {}
        }
    }

    None
}

/// The item a change *writes*: an update's `to`, never its `from`.
fn written_item(change: &EventChange) -> &CalendarItem {
    match change {
        EventChange::Create(item) | EventChange::Delete(item) => item,
        EventChange::Update { to, .. } => to,
    }
}

/// The first outgoing change the event-only wire cannot carry (R70/R71).
///
/// Keyed on the kind, not on the Rust type: anything that is not a `VEVENT`
/// is refused, so a component kind added later is stopped here rather than
/// smuggled onto the wire — or, worse, abandoning the changes behind it.
fn unpushable_item(diff: &CalendarDiff) -> Option<&CalendarItem> {
    diff.outgoing()
        .iter()
        .map(written_item)
        .find(|item| item.instance_id().kind() != ComponentKind::Event)
}

/// The whole-push pre-flight, run before *any* RPC.
///
/// Both refusals share one reason: a half-pushed calendar is the destructive
/// outcome, so the push either happens completely or not at all.
///
/// - R65 — one collection cannot hold a same-UID event/task pair, and at the
///   same href the server answers a silent `204` overwrite rather than an
///   error, so it will not protect the user here.
/// - R70/R71 — the outbound RPC field is an `Event`, so a task aborts the
///   loop. Aborting *mid-loop* silently abandoned every change queued behind
///   it, in `read_dir` order, on every run.
fn refuse_unpushable(
    diff: &CalendarDiff,
    events_by_instance_id: &HashMap<EventInstanceId, CalendarEvent>,
) -> Result<(), ConnectionError> {
    if let Some(uid) = same_uid_pair(diff) {
        return Err(ConnectionError::SameUidPair { uid });
    }

    if let Some(item) = unpushable_item(diff) {
        let unpushable = diff
            .outgoing()
            .iter()
            .filter(|change| written_item(change).instance_id().kind() != ComponentKind::Event)
            .count();

        return Err(ConnectionError::UnpushableTask {
            uid: item.uid().clone(),
            // A delete's file is already gone; the message falls back to the
            // UID rather than inventing a path.
            file: events_by_instance_id
                .get(&item.instance_id())
                .map(|e| e.path().to_path_buf()),
            pending: diff.outgoing().len() - unpushable,
        });
    }

    Ok(())
}

async fn push_outgoing_changes(
    remote: &Remote,
    diff: &CalendarDiff,
    events_by_instance_id: &mut HashMap<EventInstanceId, CalendarEvent>,
    sync_bases: &mut Vec<CalendarItem>,
) -> Result<(), ConnectionError> {
    refuse_unpushable(diff, events_by_instance_id)?;

    for change in diff.outgoing() {
        if let Some(remote_event) = remote.apply_change(change).await? {
            let returned_item = remote_event.item();

            // Sometimes provider overwrite the event's UID:
            let original_event_id = match change {
                EventChange::Create(item) => item.instance_id(),
                EventChange::Update { to, .. } => to.instance_id(),
                EventChange::Delete(_) => unreachable!("apply_change returns None for Delete"),
            };

            if let Some(cal_event) = events_by_instance_id.get_mut(&original_event_id) {
                cal_event
                    .update(returned_item.clone())
                    .map_err(CalendarError::from)?;
            }

            sync_bases.push(returned_item.clone());
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Event;
    use crate::diff::EventChange;
    use crate::event::{EventUid, XProperty};
    use crate::provider::mock_provider::MockProvider;
    use crate::test_utils::{
        VEVENT_CONTROL_ICS, VTODO_ZONED_ICS, incoming_create_diff, incoming_delete_diff,
        incoming_update_diff, outgoing_create_diff, outgoing_delete_diff, outgoing_update_diff,
        test_caldir, test_event, test_event_time, test_mock_provider, test_remote_config,
        test_remote_params,
    };
    use crate::todo::Todo;
    use crate::{CalendarConfig, rpc};
    use pretty_assertions::assert_eq;

    fn calendar_config(read_only: Option<bool>) -> CalendarConfig {
        CalendarConfig::new(
            Some("Read-only test".to_string()),
            None,
            read_only,
            Some(test_remote_config("test-provider")),
        )
    }

    fn writable_connection() -> (tempfile::TempDir, MockProvider, Connection) {
        let (tmp, caldir) = test_caldir();
        let calendar = caldir
            .create_calendar("writable-cal", Some(calendar_config(Some(false))))
            .unwrap();
        let mock = test_mock_provider();
        let remote = Remote::new(mock.provider(), test_remote_params());
        let connection = Connection::new(calendar, remote);
        (tmp, mock, connection)
    }

    #[tokio::test]
    async fn diff_discards_outgoing_when_read_only() {
        let (_tmp, caldir) = test_caldir();
        let calendar = caldir
            .create_calendar("read-only-cal", Some(calendar_config(Some(true))))
            .unwrap();
        calendar.create_event(test_event()).unwrap();

        let mock = test_mock_provider();
        mock.reply::<rpc::ListEvents>(vec![]);
        let remote = Remote::new(mock.provider(), test_remote_params());

        let mut connection = Connection::new(calendar, remote);
        let diff = connection.diff(&DateRange::default()).await.unwrap();

        assert!(
            diff.outgoing().is_empty(),
            "outgoing should be empty for read-only calendars, got {:?}",
            diff.outgoing()
        );
    }

    #[tokio::test]
    async fn diff_includes_outgoing_when_not_read_only() {
        let (_tmp, caldir) = test_caldir();
        let calendar = caldir
            .create_calendar("writable-cal", Some(calendar_config(Some(false))))
            .unwrap();
        let event = test_event();
        calendar.create_event(event.clone()).unwrap();

        let mock = test_mock_provider();
        mock.reply::<rpc::ListEvents>(vec![]);
        let remote = Remote::new(mock.provider(), test_remote_params());

        let mut connection = Connection::new(calendar, remote);
        let diff = connection.diff(&DateRange::default()).await.unwrap();

        assert_eq!(diff.outgoing(), &[EventChange::Create(event.into())]);
    }

    #[tokio::test]
    async fn diff_backfills_base_for_in_sync_legacy_known_id() {
        let (_tmp, caldir) = test_caldir();
        let calendar = caldir
            .create_calendar("writable-cal", Some(calendar_config(Some(false))))
            .unwrap();
        let event = test_event();
        let id = event.event_instance_id();
        calendar.create_event(event.clone()).unwrap();

        // Legacy state: id is known, but no base was ever recorded.
        let state_dir = calendar.path().join(".caldir/state");
        std::fs::create_dir_all(&state_dir).unwrap();
        std::fs::write(state_dir.join("known_event_ids"), id.to_string()).unwrap();
        let calendar = Calendar::load(calendar.path()).unwrap();
        assert_eq!(calendar.state().sync_base(&id), None);

        let mock = test_mock_provider();
        mock.reply::<rpc::ListEvents>(vec![event.clone().into()]);
        let remote = Remote::new(mock.provider(), test_remote_params());

        let mut connection = Connection::new(calendar, remote);
        let diff = connection.diff(&DateRange::default()).await.unwrap();

        assert!(diff.is_empty());
        let reloaded = Calendar::load(connection.local().path()).unwrap();
        assert_eq!(reloaded.state().sync_base(&id), Some(&event));
    }

    #[tokio::test]
    async fn diff_does_not_backfill_base_for_never_synced_pair() {
        let (_tmp, mock, mut connection) = writable_connection();
        let event = test_event();
        let id = event.event_instance_id();
        connection.local().create_event(event.clone()).unwrap();

        mock.reply::<rpc::ListEvents>(vec![event.into()]);
        connection.diff(&DateRange::default()).await.unwrap();

        let reloaded = Calendar::load(connection.local().path()).unwrap();
        assert!(!reloaded.state().synced_event_ids().contains(&id));
    }

    /// R48/C7 at the sync seam: one unreadable file in the calendar directory
    /// suppresses every delete the diff would otherwise emit, both directions.
    #[tokio::test]
    async fn diff_emits_no_deletes_when_a_local_file_is_unreadable() {
        let (_tmp, caldir) = test_caldir();
        let calendar = caldir
            .create_calendar("writable-cal", Some(calendar_config(Some(false))))
            .unwrap();

        // Local, base known, absent remotely → would be an incoming Delete.
        let local_only = test_event();
        calendar.create_event(local_only.clone()).unwrap();

        // Remote, base known, absent locally → would be an outgoing Delete.
        let remote_only = Event::new("Remote only", crate::test_utils::test_event_time());

        std::fs::write(calendar.path().join("broken.ics"), "not an ics file at all").unwrap();

        let state_dir = calendar.path().join(".caldir/state");
        std::fs::create_dir_all(&state_dir).unwrap();
        std::fs::write(
            state_dir.join("known_event_ids"),
            format!(
                "{}\n{}",
                local_only.event_instance_id(),
                remote_only.event_instance_id()
            ),
        )
        .unwrap();
        let calendar = Calendar::load(calendar.path()).unwrap();
        assert!(calendar.events().unwrap().is_incomplete());

        let mock = test_mock_provider();
        mock.reply::<rpc::ListEvents>(vec![remote_only.into()]);
        let remote = Remote::new(mock.provider(), test_remote_params());

        let mut connection = Connection::new(calendar, remote);
        let diff = connection.diff(&DateRange::default()).await.unwrap();

        assert!(
            !diff
                .outgoing()
                .iter()
                .any(|c| matches!(c, EventChange::Delete(_))),
            "outgoing must contain no deletes, got {:?}",
            diff.outgoing()
        );
        assert!(
            !diff
                .incoming()
                .iter()
                .any(|c| matches!(c, EventChange::Delete(_))),
            "incoming must contain no deletes, got {:?}",
            diff.incoming()
        );
    }

    #[tokio::test]
    async fn apply_incoming_diff_creates_file_for_incoming_create() {
        let (_tmp, _mock, mut connection) = writable_connection();
        let event = test_event();

        connection
            .apply_incoming_diff(&incoming_create_diff(event))
            .unwrap();

        let expected_path = connection
            .local()
            .path()
            .join("2026-01-01T1200__test-event.ics");
        assert!(expected_path.is_file());
    }

    #[tokio::test]
    async fn apply_incoming_diff_updates_file_for_incoming_update() {
        let (_tmp, _mock, mut connection) = writable_connection();
        let from = test_event();
        let cal_event = connection.local().create_event(from.clone()).unwrap();
        let old_path = cal_event.path().to_path_buf();

        let mut to = from.clone();
        to.summary = Some("Updated Test Event".to_string());

        connection
            .apply_incoming_diff(&incoming_update_diff(from, to))
            .unwrap();

        let new_path = connection
            .local()
            .path()
            .join("2026-01-01T1200__updated-test-event.ics");
        assert!(new_path.is_file());
        assert!(!old_path.exists());
    }

    #[tokio::test]
    async fn apply_incoming_diff_deletes_file_for_incoming_delete() {
        let (_tmp, _mock, mut connection) = writable_connection();
        let event = test_event();
        let cal_event = connection.local().create_event(event.clone()).unwrap();
        let path = cal_event.path().to_path_buf();

        connection
            .apply_incoming_diff(&incoming_delete_diff(event))
            .unwrap();

        assert!(!path.exists());
    }

    #[tokio::test]
    async fn apply_incoming_diff_records_incoming_create_in_state() {
        let (_tmp, _mock, mut connection) = writable_connection();
        let event = test_event();
        let id = event.event_instance_id();

        connection
            .apply_incoming_diff(&incoming_create_diff(event))
            .unwrap();

        assert!(connection.local().state().synced_event_ids().contains(&id));
    }

    #[tokio::test]
    async fn apply_incoming_diff_persists_base_to_disk() {
        let (_tmp, _mock, mut connection) = writable_connection();
        let event = test_event();
        let id = event.event_instance_id();

        connection
            .apply_incoming_diff(&incoming_create_diff(event.clone()))
            .unwrap();

        let reloaded = Calendar::load(connection.local().path()).unwrap();
        assert_eq!(reloaded.state().sync_base(&id), Some(&event));
    }

    #[tokio::test]
    async fn apply_incoming_diff_does_not_record_deletes_in_state() {
        let (_tmp, _mock, mut connection) = writable_connection();
        let event = test_event();
        let id = event.event_instance_id();
        connection.local().create_event(event.clone()).unwrap();

        connection
            .apply_incoming_diff(&incoming_delete_diff(event))
            .unwrap();

        assert!(!connection.local().state().synced_event_ids().contains(&id));
    }

    #[tokio::test]
    async fn apply_outgoing_diff_sends_create_event_for_outgoing_create() {
        let (_tmp, mock, mut connection) = writable_connection();
        let event = test_event();
        connection.local().create_event(event.clone()).unwrap();

        mock.reply::<rpc::CreateEvent>(event.clone());
        connection
            .apply_outgoing_diff(&outgoing_create_diff(event.clone()))
            .await
            .unwrap();

        assert_eq!(mock.captured_request::<rpc::CreateEvent>().event, event);
    }

    #[tokio::test]
    async fn apply_outgoing_diff_sends_update_event_for_outgoing_update() {
        let (_tmp, mock, mut connection) = writable_connection();
        let from = test_event();
        let mut to = from.clone();
        to.summary = Some("Updated".into());
        connection.local().create_event(to.clone()).unwrap();

        mock.reply::<rpc::UpdateEvent>(to.clone());
        connection
            .apply_outgoing_diff(&outgoing_update_diff(from, to.clone()))
            .await
            .unwrap();

        assert_eq!(mock.captured_request::<rpc::UpdateEvent>().event, to);
    }

    #[tokio::test]
    async fn apply_outgoing_diff_sends_delete_event_for_outgoing_delete() {
        let (_tmp, mock, mut connection) = writable_connection();
        let event = test_event();

        mock.reply::<rpc::DeleteEvent>(());
        connection
            .apply_outgoing_diff(&outgoing_delete_diff(event.clone()))
            .await
            .unwrap();

        assert_eq!(mock.captured_request::<rpc::DeleteEvent>().event, event);
    }

    #[tokio::test]
    async fn apply_outgoing_diff_rewrites_local_with_canonical_event_from_provider() {
        let (_tmp, mock, mut connection) = writable_connection();
        let local = test_event();
        connection.local().create_event(local.clone()).unwrap();

        let mut canonical = local.clone();
        canonical.x_properties.push(XProperty::new(
            "X-GOOGLE-EVENT-ID".to_string(),
            "abc123".to_string(),
        ));
        mock.reply::<rpc::CreateEvent>(canonical.clone());

        connection
            .apply_outgoing_diff(&outgoing_create_diff(local))
            .await
            .unwrap();

        let reloaded = connection.local().events().unwrap().into_events();
        assert_eq!(reloaded.len(), 1);
        assert_eq!(
            reloaded[0].event().unwrap().x_properties,
            canonical.x_properties
        );
        assert_eq!(
            connection
                .local()
                .state()
                .sync_base(&canonical.event_instance_id()),
            Some(&canonical)
        );
    }

    #[tokio::test]
    async fn apply_outgoing_diff_rewrites_local_when_provider_reassigns_uid() {
        // Some providers (e.g. CalDAV servers) re-assign UID server-side. The
        // local file must still be rewritten with the provider's canonical
        // event, and the synced state must record the *returned* identity so
        // the next sync doesn't see a phantom delete + duplicate create.
        let (_tmp, mock, mut connection) = writable_connection();
        let local = test_event();
        let original_id = local.event_instance_id();
        connection.local().create_event(local.clone()).unwrap();

        let mut canonical = local.clone();
        canonical.uid = EventUid::new("provider-assigned-uid@example.com");
        let canonical_id = canonical.event_instance_id();
        assert_ne!(original_id, canonical_id);
        mock.reply::<rpc::CreateEvent>(canonical.clone());

        connection
            .apply_outgoing_diff(&outgoing_create_diff(local))
            .await
            .unwrap();

        let reloaded = connection.local().events().unwrap().into_events();
        assert_eq!(reloaded.len(), 1);
        assert_eq!(reloaded[0].event().unwrap().uid, canonical.uid);

        let synced = connection.local().state().synced_event_ids();
        assert!(synced.contains(&canonical_id));
        assert!(!synced.contains(&original_id));
    }

    #[tokio::test]
    async fn apply_outgoing_diff_records_base_for_outgoing_create() {
        let (_tmp, mock, mut connection) = writable_connection();
        let event = test_event();
        let id = event.event_instance_id();
        connection.local().create_event(event.clone()).unwrap();

        mock.reply::<rpc::CreateEvent>(event.clone());
        connection
            .apply_outgoing_diff(&outgoing_create_diff(event.clone()))
            .await
            .unwrap();

        assert_eq!(connection.local().state().sync_base(&id), Some(&event));
    }

    #[tokio::test]
    async fn apply_outgoing_diff_persists_synced_ids_on_partial_success() {
        use crate::provider::transport::ProviderTransportError;
        use std::time::Duration;

        let (_tmp, mock, mut connection) = writable_connection();

        let event_a = test_event();
        let event_b = test_event();
        let id_a = event_a.event_instance_id();
        connection.local().create_event(event_a.clone()).unwrap();
        connection.local().create_event(event_b.clone()).unwrap();

        // First Create succeeds!
        // Second errors mid-loop!
        mock.reply::<rpc::CreateEvent>(event_a.clone());
        mock.reply_error(ProviderTransportError::Timeout(Duration::from_secs(1)));

        let diff = CalendarDiff::from_changes(
            vec![
                EventChange::Create(event_a.clone().into()),
                EventChange::Create(event_b.into()),
            ],
            vec![],
        );

        let result = connection.apply_outgoing_diff(&diff).await;

        assert!(
            result.is_err(),
            "expected the second create to propagate an error",
        );

        let reloaded = Calendar::load(connection.local().path()).unwrap();

        assert_eq!(
            reloaded.state().sync_base(&id_a),
            Some(&event_a),
            "event A's base should survive a later push failure",
        );
    }

    // --- L4-03: items through the whole chain, transport mocked ---

    /// `T-48`, renamed. **The ticket's `a_task_is_pushed_as_a_vtodo_and_never_
    /// as_a_vevent` is not implementable and the name would assert a
    /// falsehood.** The outbound RPC field is `pub event: Event` (R70), so a
    /// `BEGIN:VTODO` payload can never reach `CreateEvent`; the same ticket's
    /// R56 and the ADJUDICATION's ADR-07 ruling say the push *aborts* with
    /// `expected 1 event(s) in ICS, found 0`. The constraint wins.
    ///
    /// What survives, and is the half R1/A-1 item 3 actually calls inviolable:
    /// **no VEVENT is synthesized for a task anywhere in the chain**. Proven
    /// end to end here — real scan, real diff, real apply — by the transport
    /// having seen nothing at all.
    ///
    /// The *error* the user reads changed with the pre-flight guard: the abort
    /// now happens before the loop, so this seam reports the refusal, not the
    /// wire's arity message. The arity message is unchanged and still locked
    /// where it is raised — `remote::tests::
    /// a_task_change_aborts_instead_of_reaching_the_provider`.
    #[tokio::test]
    async fn a_task_push_aborts_and_never_reaches_the_provider_as_a_vevent() {
        let (_tmp, mock, mut connection) = writable_connection();
        let todo = Todo::new("Hand in the assignment");
        let cal_event = connection.local().create_event(todo.clone()).unwrap();
        let path = cal_event.path().to_path_buf();

        let err = connection
            .apply_outgoing_diff(&outgoing_create_diff(todo.clone()))
            .await
            .expect_err("a task must never be pushed over the event-only wire");

        let ConnectionError::UnpushableTask { uid, file, pending } = &err else {
            panic!("expected the pre-flight refusal, got {err:?}");
        };
        assert_eq!(uid, &todo.uid);
        assert_eq!(file.as_deref(), Some(path.as_path()));
        assert_eq!(*pending, 0, "the task was the only outgoing change");

        assert!(
            !mock.received_a_request(),
            "nothing may reach the transport for a task push",
        );

        // The kind survived on disk: still a VTODO, still no VEVENT.
        let on_disk = std::fs::read_to_string(&path).unwrap();
        assert!(on_disk.contains("BEGIN:VTODO"), "got {on_disk:?}");
        assert!(!on_disk.contains("BEGIN:VEVENT"), "got {on_disk:?}");
        assert_eq!(CalendarItem::load(&path).unwrap(), todo.into());
    }

    /// `T-50` — R56, the first accepted skew failure: **new core + old
    /// provider**. Pinned as a *failure*, never a quiet no-op.
    ///
    /// This test used to pin the *partial* abort: the first event really was
    /// created remotely and its base flushed, then the task aborted the loop
    /// and everything behind it was abandoned. QA reproduced the consequence
    /// against Radicale — 6 events + 1 task edit, 4 pushed, 2 silently
    /// abandoned, in `read_dir` order — so what it pinned was the defect.
    ///
    /// The contract now: the task fails the calendar **whole**. Nothing is
    /// pushed, no base is flushed for anything, and the delete queued behind
    /// the task is still pending rather than lost. Every no-data-loss
    /// assertion below is unchanged; the "one event got through" assertions
    /// became their opposite, which is the fix.
    #[tokio::test]
    async fn pushing_a_task_to_a_task_less_provider_fails_that_calendar_without_data_loss() {
        let (_tmp, mock, mut connection) = writable_connection();

        let event = test_event();
        let todo = Todo::new("Hand in the assignment");
        let doomed = Event::new("Would be deleted remotely", test_event_time());
        let doomed_id = doomed.event_instance_id();

        let event_path = connection
            .local()
            .create_event(event.clone())
            .unwrap()
            .path()
            .to_path_buf();
        let todo_path = connection
            .local()
            .create_event(todo.clone())
            .unwrap()
            .path()
            .to_path_buf();

        // A reply is queued for the event, so the transport *could* accept it.
        // Nothing may consume it.
        mock.reply::<rpc::CreateEvent>(event.clone());

        let diff = CalendarDiff::from_changes(
            vec![
                EventChange::Create(event.clone().into()),
                EventChange::Create(todo.clone().into()),
                EventChange::Delete(doomed.into()),
            ],
            vec![],
        );

        let todo_bytes_before = std::fs::read(&todo_path).unwrap();

        let result = connection.apply_outgoing_diff(&diff).await;

        let err = result.expect_err("the task push must fail the calendar");

        // `L6-01`/R71: the pre-flight refusal, not the wire's arity message.
        // The arity message stays byte-locked where it is raised (`remote::
        // tests::a_task_change_aborts_instead_of_reaching_the_provider`); a
        // user who upgraded to fix that string must not be shown its twin.
        let ConnectionError::UnpushableTask { uid, file, pending } = &err else {
            panic!("expected the pre-flight refusal, got {err:?}");
        };
        assert_eq!(uid, &todo.uid);
        assert_eq!(file.as_deref(), Some(todo_path.as_path()));
        assert_eq!(
            *pending, 2,
            "the event create and the delete are still pending"
        );

        // What the user actually reads, via `caldir push`'s renderer. It names
        // the offending file, so the fix is actionable without guesswork.
        let message = err.to_string();
        let filename = todo_path.file_name().unwrap().to_string_lossy();
        assert!(message.contains(&*filename), "got {message:?}");
        assert!(
            !message.contains("expected 1 event(s) in ICS"),
            "got {message:?}"
        );

        // Not data loss: both files are still there, and still themselves.
        assert!(event_path.is_file());
        assert!(todo_path.is_file());
        assert_eq!(CalendarItem::load(&todo_path).unwrap(), todo.clone().into());
        assert_eq!(
            std::fs::read(&todo_path).unwrap(),
            todo_bytes_before,
            "the failed item's file must be byte-identical to before the push",
        );

        // Nothing at all reached the wire: not the event create ahead of the
        // task, and not the delete queued behind it.
        assert!(
            !mock.received_a_request(),
            "a refused push may not apply a single change",
        );

        // No base for the event either. Recording one for a change that was
        // never pushed is what would make the *next* run read it as synced.
        let reloaded = Calendar::load(connection.local().path()).unwrap();
        let event_id = event.event_instance_id();
        assert_eq!(reloaded.state().sync_bases().get(&event_id), None);
        assert!(!reloaded.state().synced_event_ids().contains(&event_id));

        // Same for the item that failed and the delete queued behind it.
        let todo_id = CalendarItem::Todo(todo).instance_id();
        assert_eq!(reloaded.state().sync_bases().get(&todo_id), None);
        assert!(!reloaded.state().synced_event_ids().contains(&todo_id));
        assert_eq!(reloaded.state().sync_bases().get(&doomed_id), None);
        assert!(!reloaded.state().synced_event_ids().contains(&doomed_id));
    }

    /// `T-51` — R57 (INVIOLABLE), the second accepted skew failure: **old core
    /// + new provider**. One undecodable element fails the whole vector, the
    /// call errors, and the sync aborts *before any apply*. Pinned as a
    /// failure: the catastrophic alternative is a silent drop, which makes the
    /// remote list arrive short and feeds the delete path.
    #[tokio::test]
    async fn a_task_payload_from_a_newer_provider_fails_the_whole_list_before_any_apply() {
        let (_tmp, mock, mut connection) = writable_connection();

        // Local, base known, absent from the (decodable half of the) response:
        // if the short list were accepted, this file is what would be deleted.
        let local = test_event();
        let local_path = connection
            .local()
            .create_event(local.clone())
            .unwrap()
            .path()
            .to_path_buf();
        connection
            .local
            .record_sync_bases(vec![local.clone()])
            .unwrap();

        let before = calendar_snapshot(connection.local().path());
        assert!(
            before
                .iter()
                .any(|(_, item)| item.as_ref() == Some(&local.clone().into())),
            "the snapshot must actually see the file it is guarding, got {before:?}",
        );

        // A component this core does not support, beside a perfectly good one.
        let decodable = CalendarItem::Event(local.clone()).to_ics_string();
        let undecodable = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VJOURNAL\r\nUID:journal@u-crawler\r\nDTSTAMP:20260801T090000Z\r\n\
END:VJOURNAL\r\nEND:VCALENDAR\r\n";
        mock.reply_raw_data(serde_json::json!([decodable, undecodable]));

        let Err(err) = connection.diff(&DateRange::default()).await else {
            panic!("one undecodable element must fail the whole list");
        };

        assert!(
            err.to_string().contains("VJOURNAL"),
            "the offending component must survive to the connection tier, got {err}",
        );

        // Nothing applied: no file created, updated or deleted, no base added.
        assert!(local_path.is_file());
        assert_eq!(calendar_snapshot(connection.local().path()), before);
    }

    /// `T-52` — A-6/R100. A VEVENT and a VTODO sharing one UID are two items
    /// all the way down: two identities, two base files, two `known_event_ids`
    /// lines. With both present locally, neither reported remotely and the
    /// shared id known, exactly **one** delete is emitted, against the event's
    /// file — the task's is withheld by R49's version-skew belt.
    ///
    /// Driven through `diff` twice so the backfill path is exercised too: it is
    /// the one place a task could be judged "in sync" against an *event* (AC 3),
    /// and a uid-keyed lookup there silently backfills only one of the pair.
    ///
    /// Deliberately not a same-direction dual push into one collection: `L5-04`
    /// makes that a push error (R65/R80), and A-6 never asked for one.
    #[tokio::test]
    async fn a_mixed_local_directory_keeps_both_items_distinct_end_to_end() {
        let (_tmp, mock, mut connection) = writable_connection();

        let uid = EventUid::new("shared-123@u-crawler");
        let mut event = Event::new("Assignment 3 - submission window", test_event_time());
        event.uid = uid.clone();
        let mut todo = Todo::new("Assignment 3 - hand in");
        todo.uid = uid.clone();

        let event_path = connection
            .local()
            .create_event(event.clone())
            .unwrap()
            .path()
            .to_path_buf();
        let todo_path = connection
            .local()
            .create_event(todo.clone())
            .unwrap()
            .path()
            .to_path_buf();
        assert_ne!(event_path, todo_path, "one UID, two files");

        // Two identities out of one UID.
        let event_id = CalendarItem::Event(event.clone()).instance_id();
        let todo_id = CalendarItem::Todo(todo.clone()).instance_id();
        assert_ne!(event_id, todo_id);

        let mut listed: Vec<EventInstanceId> = connection
            .local()
            .events()
            .unwrap()
            .into_events()
            .into_iter()
            .map(|e| e.item().instance_id())
            .collect();
        listed.sort_by_key(|id| id.to_string());
        let mut expected = vec![event_id.clone(), todo_id.clone()];
        expected.sort_by_key(|id| id.to_string());
        assert_eq!(listed, expected);

        // Legacy state: both ids known, neither base recorded. The remote
        // reports both, unchanged, so the backfill is the only thing that can
        // record them — and it has to tell the pair apart to record *two*.
        let state_dir = connection.local().path().join(".caldir/state");
        std::fs::create_dir_all(&state_dir).unwrap();
        std::fs::write(
            state_dir.join("known_event_ids"),
            format!("{event_id}\n{todo_id}\n"),
        )
        .unwrap();
        connection.local = Calendar::load(connection.local().path()).unwrap();

        mock.reply::<rpc::ListEvents>(vec![
            CalendarItem::Event(event.clone()),
            CalendarItem::Todo(todo.clone()),
        ]);
        let in_sync = connection.diff(&DateRange::default()).await.unwrap();
        assert!(in_sync.is_empty(), "both sides already agree");

        // Two base files, two known-id lines — one of them sigil-prefixed.
        let bases: Vec<_> = std::fs::read_dir(state_dir.join("bases"))
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        assert_eq!(bases.len(), 2, "one base file per identity, got {bases:?}");
        let base_for = |id: &EventInstanceId| -> Option<CalendarItem> {
            connection
                .local()
                .state()
                .sync_bases()
                .get(id)
                .and_then(|base| base.as_deref().cloned())
        };
        assert_eq!(
            base_for(&event_id),
            Some(CalendarItem::Event(event.clone()))
        );
        assert_eq!(base_for(&todo_id), Some(CalendarItem::Todo(todo.clone())));

        let known = std::fs::read_to_string(state_dir.join("known_event_ids")).unwrap();
        let mut lines: Vec<&str> = known.lines().filter(|l| !l.is_empty()).collect();
        lines.sort_unstable();
        assert_eq!(
            lines,
            vec!["VTODO:shared-123@u-crawler", "shared-123@u-crawler"],
        );

        // Now neither item is reported remotely.
        mock.reply::<rpc::ListEvents>(vec![]);
        let diff = connection.diff(&DateRange::default()).await.unwrap();

        assert_eq!(
            diff.incoming(),
            &[EventChange::Delete(CalendarItem::Event(event.clone()))],
            "exactly one delete, and it is the event's",
        );
        assert!(diff.suppressed_task_deletes());

        connection.apply_incoming_diff(&diff).unwrap();

        assert!(!event_path.exists(), "the event's file is the one deleted");
        assert!(todo_path.is_file(), "the task's file must survive");
        assert_eq!(CalendarItem::load(&todo_path).unwrap(), todo.into());
    }

    // --- L6-01: the accepted ADR-07 consequences, pinned ---

    /// **R71/R72/R49 at the `Connection` seam.** The webcal shape: a feed that
    /// *does* answer — it returns events — but silently drops every `VTODO`
    /// (`caldir-provider-webcal` filters them out before core ever sees them).
    /// Absence there means "this provider cannot see tasks", never "the user
    /// deleted them", so no local task may be deleted for it.
    ///
    /// Distinct from `T-52`, which drives an *empty* response and a same-UID
    /// pair. Here the response is non-empty and the identities are unrelated,
    /// which is the case a naive "the remote answered, so absence is a delete"
    /// reading gets wrong.
    ///
    /// The event side is the control: an event with a known base that the feed
    /// also omits **is** deleted, exactly as it is today (R1, R2).
    #[tokio::test]
    async fn a_task_less_feed_does_not_delete_local_tasks() {
        let (_tmp, mock, mut connection) = writable_connection();

        let task = Todo::new("Renew the parking permit");
        let still_there = Event::new("Weekly standup", test_event_time());
        let gone = Event::new("Cancelled offsite", test_event_time());

        let task_path = connection
            .local()
            .create_event(task.clone())
            .unwrap()
            .path()
            .to_path_buf();
        let still_there_path = connection
            .local()
            .create_event(still_there.clone())
            .unwrap()
            .path()
            .to_path_buf();
        let gone_path = connection
            .local()
            .create_event(gone.clone())
            .unwrap()
            .path()
            .to_path_buf();

        // All three have synced before, so absence from the feed is the only
        // thing that could authorise a delete.
        connection
            .local
            .record_sync_bases(vec![
                CalendarItem::Todo(task.clone()),
                CalendarItem::Event(still_there.clone()),
                CalendarItem::Event(gone.clone()),
            ])
            .unwrap();

        // Events, and only events — the webcal feed shape.
        mock.reply::<rpc::ListEvents>(vec![CalendarItem::Event(still_there.clone())]);
        let diff = connection.diff(&DateRange::default()).await.unwrap();

        assert_eq!(
            diff.incoming(),
            &[EventChange::Delete(CalendarItem::Event(gone.clone()))],
            "the event side is diffed exactly as today; the task is withheld",
        );
        assert!(
            !diff
                .incoming()
                .iter()
                .chain(diff.outgoing())
                .any(|c| matches!(c, EventChange::Delete(CalendarItem::Todo(_)))),
            "no delete of any kind may target a task, got incoming {:?} outgoing {:?}",
            diff.incoming(),
            diff.outgoing(),
        );
        assert!(
            diff.suppressed_task_deletes(),
            "the belt actually cost a delete here, so the signal must be set",
        );

        connection.apply_incoming_diff(&diff).unwrap();

        assert!(task_path.is_file(), "the task's file must survive the feed");
        assert_eq!(CalendarItem::load(&task_path).unwrap(), task.into());
        assert!(still_there_path.is_file());
        assert!(
            !gone_path.exists(),
            "the event control must still be deleted"
        );
    }

    /// **R71**, the delete arm. `L4-03` pinned the `Create` arm; `Update` and
    /// `Delete` abort through the same `into_event()` on the way to an
    /// event-only wire, but nothing pinned it — the ADJUDICATION records the
    /// gap as this ticket's ("a pure task-only outgoing `Delete` has no lock").
    ///
    /// It matters because `push_outgoing_changes` carries
    /// `unreachable!("apply_change returns None for Delete")`. That branch is
    /// safe only while a `Delete` can never return `Some`, and a task-only
    /// delete is the one shape that would otherwise walk into it. The other
    /// half of that invariant — an *event* delete really does yield `None` —
    /// is pinned in `remote::tests::apply_change_yields_no_remote_event_for_a_delete`.
    #[tokio::test]
    async fn a_task_only_delete_aborts_before_reaching_the_provider() {
        let (_tmp, mock, mut connection) = writable_connection();

        let task = Todo::new("Renew the parking permit");
        let task_path = connection
            .local()
            .create_event(task.clone())
            .unwrap()
            .path()
            .to_path_buf();

        let err = connection
            .apply_outgoing_diff(&outgoing_delete_diff(task.clone()))
            .await
            .expect_err("a task delete must never ride the event-only wire");

        // Refused by the pre-flight, so the `Delete` never reaches the loop at
        // all — which is also why the `unreachable!` below it stays safe.
        let ConnectionError::UnpushableTask { uid, .. } = &err else {
            panic!("expected the pre-flight refusal, got {err:?}");
        };
        assert_eq!(uid, &task.uid);

        assert!(
            !mock.received_a_request(),
            "nothing may reach the transport for a task delete",
        );
        assert!(task_path.is_file(), "the local file must survive the abort");
        assert_eq!(CalendarItem::load(&task_path).unwrap(), task.into());
    }

    /// **L6-01 / QA.** The silent-abandonment lock.
    ///
    /// Reproduced against real Radicale: a calendar holding one task plus
    /// several pending event pushes pushed *some* of the events, aborted on
    /// the task, and abandoned the rest — silently, and in `read_dir` order,
    /// so *which* changes survived was arbitrary. Every later run repeated it,
    /// because the task fails identically forever.
    ///
    /// The contract this pins: an outgoing set the event-only wire cannot
    /// carry is refused **whole, before the first RPC**. Not one change is
    /// applied, so there is no partial push and nothing to reconcile.
    ///
    /// Without the pre-flight guard this test fails on the very first
    /// assertion: the transport receives the event creates that happen to sort
    /// before the task.
    #[tokio::test]
    async fn a_task_in_the_outgoing_set_refuses_the_whole_push_before_the_first_rpc() {
        let (_tmp, mock, mut connection) = writable_connection();

        // QA's shape: six event pushes queued behind (and around) one task.
        let events: Vec<Event> = (0..6)
            .map(|i| Event::new(format!("Lecture {i}"), test_event_time()))
            .collect();
        let todo = Todo::new("Hand in the assignment");

        for event in &events {
            connection.local().create_event(event.clone()).unwrap();
        }
        let todo_path = connection
            .local()
            .create_event(todo.clone())
            .unwrap()
            .path()
            .to_path_buf();

        // Replies are queued for every event, so the transport *could* accept
        // them all. Nothing may consume one.
        for event in &events {
            mock.reply::<rpc::CreateEvent>(event.clone());
        }

        let mut changes: Vec<EventChange> = events
            .iter()
            .map(|e| EventChange::Create(e.clone().into()))
            .collect();
        // Mid-set, not last: the bug abandoned whatever followed the task.
        changes.insert(3, EventChange::Create(todo.clone().into()));

        let before = ics_snapshot(connection.local().path());

        let err = connection
            .apply_outgoing_diff(&CalendarDiff::from_changes(changes, vec![]))
            .await
            .expect_err("an outgoing set holding a task must be refused whole");

        assert!(
            !mock.received_a_request(),
            "not one change may reach the wire when the push is refused",
        );

        // The error names the offending item by file, not merely by type.
        let ConnectionError::UnpushableTask { .. } = &err else {
            panic!("expected the pre-flight refusal, got {err:?}");
        };
        let message = err.to_string();
        let filename = todo_path.file_name().unwrap().to_string_lossy();
        assert!(
            message.contains(&*filename),
            "the message must name the offending file, got {message:?}",
        );

        // Distinct from the wire-level arity abort, which stays byte-locked at
        // the `Remote` seam and must not be what the user reads here.
        assert!(
            !message.contains("expected 1 event(s) in ICS"),
            "the pre-flight refusal must not wear the arity error's message, got {message:?}",
        );

        // Nothing moved on disk, and no base was recorded for anything: a base
        // for an event would make the next run read it as already pushed.
        assert_eq!(ics_snapshot(connection.local().path()), before);
        let reloaded = Calendar::load(connection.local().path()).unwrap();
        for event in &events {
            let id = event.event_instance_id();
            assert_eq!(reloaded.state().sync_bases().get(&id), None, "{id}");
            assert!(!reloaded.state().synced_event_ids().contains(&id), "{id}");
        }
    }

    /// **R65.** A same-UID event/task pair is a perfectly good *local*
    /// directory (R4, and `T-52` above proves it works), but it cannot be
    /// pushed into one CalDAV collection: Radicale enforces UID uniqueness per
    /// collection component-agnostically — `409` across hrefs, `400` within one
    /// resource, and, at the same href, a **silent `204` overwrite**. The
    /// server will not protect the user here, so caldir stops before the wire.
    ///
    /// The destructive case this pins is the *partial* push: a reply is queued
    /// for the event, so without the guard the event really is created
    /// remotely and only the task then fails — leaving the pair half-pushed
    /// into a collection that cannot hold both.
    #[tokio::test]
    async fn a_same_uid_event_task_pair_pushed_to_one_collection_is_an_error() {
        let (_tmp, mock, mut connection) = writable_connection();

        let uid = EventUid::new("shared-123@u-crawler");
        let mut event = Event::new("Assignment 3 - submission window", test_event_time());
        event.uid = uid.clone();
        let mut todo = Todo::new("Assignment 3 - hand in");
        todo.uid = uid.clone();

        let event_path = connection
            .local()
            .create_event(event.clone())
            .unwrap()
            .path()
            .to_path_buf();
        let todo_path = connection
            .local()
            .create_event(todo.clone())
            .unwrap()
            .path()
            .to_path_buf();
        let before = (
            std::fs::read(&event_path).unwrap(),
            std::fs::read(&todo_path).unwrap(),
        );

        // Queued, and must go unused: without the guard the event's create
        // succeeds and the pair ends up half-pushed.
        mock.reply::<rpc::CreateEvent>(event.clone());

        let diff = CalendarDiff::from_changes(
            vec![
                EventChange::Create(event.clone().into()),
                EventChange::Create(todo.clone().into()),
            ],
            vec![],
        );

        let err = connection
            .apply_outgoing_diff(&diff)
            .await
            .expect_err("a same-UID pair cannot be pushed to one collection");

        assert!(
            err.to_string().contains("shared-123@u-crawler"),
            "the error must name the UID the user has to act on, got: {err}"
        );

        assert!(
            !mock.received_a_request(),
            "no RPC may be sent for ANY change in that push: a partial push is \
             the destructive case Radicale's silent 204 overwrite produces",
        );

        assert!(event_path.is_file() && todo_path.is_file());
        assert_eq!(
            (
                std::fs::read(&event_path).unwrap(),
                std::fs::read(&todo_path).unwrap()
            ),
            before,
            "both local files must survive byte-for-byte: the local mixed \
             directory stays supported (R4), only the push is refused"
        );
    }

    /// The pair is a property of the outgoing *set*, not of one change shape.
    /// Deleting the event while creating the task still puts both kinds under
    /// one UID into one collection, and the changes carry no guaranteed order:
    /// create-first is a `409`/silent-`204` against the event's own resource,
    /// delete-first destroys the event and then fails. Refused either way.
    #[tokio::test]
    async fn a_pair_split_across_a_delete_and_a_create_is_refused_too() {
        let (_tmp, mock, mut connection) = writable_connection();

        let uid = EventUid::new("shared-123@u-crawler");
        let mut event = Event::new("Assignment 3 - submission window", test_event_time());
        event.uid = uid.clone();
        let mut todo = Todo::new("Assignment 3 - hand in");
        todo.uid = uid.clone();

        connection.local().create_event(todo.clone()).unwrap();

        let diff = CalendarDiff::from_changes(
            vec![
                EventChange::Delete(event.into()),
                EventChange::Create(todo.into()),
            ],
            vec![],
        );

        let err = connection
            .apply_outgoing_diff(&diff)
            .await
            .expect_err("a delete/create pair under one UID is still a pair");

        assert!(
            err.to_string().contains("shared-123@u-crawler"),
            "got: {err}"
        );
        assert!(
            !mock.received_a_request(),
            "the delete must not reach the wire either",
        );
    }

    /// The guard is about a same-UID **pair across kinds**, and nothing wider.
    /// A same-kind push of two unrelated items, and two same-UID items of the
    /// *same* kind (a recurrence override and its master), must still push:
    /// widening this predicate would break ordinary event sync (R1/R2).
    #[tokio::test]
    async fn two_outgoing_events_are_not_a_pair_and_still_push() {
        let (_tmp, mock, mut connection) = writable_connection();

        let first = test_event();
        let mut second = Event::new("Same UID, same kind", test_event_time());
        second.uid = first.uid.clone();

        connection.local().create_event(first.clone()).unwrap();

        mock.reply::<rpc::CreateEvent>(first.clone());
        mock.reply::<rpc::CreateEvent>(second.clone());

        let diff = CalendarDiff::from_changes(
            vec![
                EventChange::Create(first.clone().into()),
                EventChange::Create(second.into()),
            ],
            vec![],
        );

        connection
            .apply_outgoing_diff(&diff)
            .await
            .expect("two same-kind changes are not a pair and must still push");

        assert!(
            mock.received_a_request(),
            "two same-kind changes must still reach the wire",
        );
    }

    /// The u_crawler VTODO, widened to the properties that churn. `VALARM`
    /// (R24 — a fresh alarm UID per serialize ping-pongs forever), a provider
    /// `X-*` property (stripped by a local write => churn), and
    /// `PERCENT-COMPLETE:0` (open question Q-4: `Some(0)` must not read back
    /// as `None`).
    const CHURN_VTODO_ICS: &str = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//u_crawler//EN\r\n\
CALSCALE:GREGORIAN\r\n\
BEGIN:VTODO\r\n\
UID:assignment-123@u-crawler\r\n\
DTSTAMP:20260801T090000Z\r\n\
DUE:20260815T235900Z\r\n\
SUMMARY:Assignment 3 - Recursive descent parser\r\n\
DESCRIPTION:Submit the parser and a short report via the course portal.\r\n\
URL:https://example.edu/courses/cs101/assignments/123\r\n\
PRIORITY:1\r\n\
STATUS:COMPLETED\r\n\
COMPLETED:20260814T120000Z\r\n\
PERCENT-COMPLETE:0\r\n\
X-GOOGLE-EVENT-ID:abc123\r\n\
BEGIN:VALARM\r\n\
ACTION:DISPLAY\r\n\
DESCRIPTION:Reminder\r\n\
TRIGGER;RELATED=START:-PT30M\r\n\
END:VALARM\r\n\
END:VTODO\r\n\
END:VCALENDAR\r\n";

    /// `T-49` — **A-2 / R96, the load-bearing idempotence gate.** Sync twice
    /// against the same (mocked) remote; the *second* diff must be empty.
    ///
    /// This is the one test that catches sync churn — an item perceived as
    /// changed on every run — including mechanisms nobody enumerated. The
    /// fixture is deliberately wide: `CHURN_VTODO_ICS` above, plus a second,
    /// *zoned* task whose `DUE` carries a `TZID` and whose `COMPLETED` is
    /// floating (R22 — a floating `COMPLETED` reading back as `None` is
    /// churn), plus the VEVENT control, which travels the *other* direction so
    /// both halves of one sync are exercised.
    ///
    /// **Both remote responses are the same raw ICS documents**, handed to the
    /// transport verbatim (`reply_raw_data`) rather than re-encoded from domain
    /// values by the code under test. That matters: the remote is an
    /// independent store that returns what it holds on every GET, so the
    /// fixture text — not caldir's own encoder — is the source of truth on that
    /// side. Anything caldir's local write/read loses or invents therefore
    /// shows up as a real change on pass two instead of cancelling out.
    ///
    /// **The comparison is structural, never textual (R30/R31).** Nothing here
    /// asserts on ICS text, on bytes, or on an ETag: caldir re-stamps `DTSTAMP`
    /// on every serialize (R25), so a byte- or ETag-based "the push was a
    /// no-op" claim is false by construction, and Radicale re-sorts properties,
    /// re-folds at 75 octets and synthesizes `VTIMEZONE` anyway. ICS text is an
    /// *input* here and never an expected value; the only equality asserted is
    /// `CalendarDiff::is_empty()`, i.e. parsed-domain `==` on `CalendarItem`.
    ///
    /// If this goes red, walk ADR-06's churn table; the diagnostic below prints
    /// the offending changes, because a bare boolean here is undebuggable.
    #[tokio::test]
    async fn syncing_twice_against_a_fake_remote_yields_an_empty_second_diff() {
        let (_tmp, mock, mut connection) = writable_connection();

        // Fixed date => a deterministic window; `Local` makes it move with the
        // TZ matrix (R106).
        let range = DateRange::sync_window_at(
            chrono::NaiveDate::from_ymd_opt(2026, 8, 25).unwrap(),
            chrono::Local,
        );

        let CalendarItem::Event(event) =
            CalendarItem::from_single_ics_str(VEVENT_CONTROL_ICS).unwrap()
        else {
            panic!("the VEVENT control fixture must parse as an event");
        };

        // The control event is local-only; both tasks live on the remote, so
        // one sync moves items in *both* directions.
        connection.local().create_event(event.clone()).unwrap();
        mock.reply_raw_data(serde_json::json!([CHURN_VTODO_ICS, VTODO_ZONED_ICS]));

        let first = connection.diff(&range).await.unwrap();

        // Non-vacuity: "the second diff is empty" is free if the first was.
        assert!(
            !first.is_empty(),
            "the first sync must actually do work, or this gate proves nothing",
        );
        assert_eq!(first.incoming().len(), 2, "got {:#?}", first.incoming());
        let mut pulled: Vec<String> = first
            .incoming()
            .iter()
            .map(|change| match change {
                EventChange::Create(item) => item.uid().as_str().to_string(),
                other => panic!("expected two incoming creates, got {other:?}"),
            })
            .collect();
        pulled.sort();
        assert_eq!(
            pulled,
            vec!["assignment-123@u-crawler", "zoned-123@u-crawler"],
        );
        assert_eq!(
            first.outgoing(),
            &[EventChange::Create(event.clone().into())],
        );

        connection.apply_incoming_diff(&first).unwrap();
        mock.reply::<rpc::CreateEvent>(event.clone());
        connection.apply_outgoing_diff(&first).await.unwrap();

        // A second `caldir pull` is a fresh process: reload the calendar from
        // disk so pass two reads its bases from the state directory rather than
        // from the in-memory map the apply just populated. A base that never
        // reached disk is churn too.
        connection.local = Calendar::load(connection.local().path()).unwrap();

        // Pass two: nothing changed on either side. The remote now reports all
        // three items — the same bytes it was handed for the tasks, and the
        // event it just accepted.
        mock.reply_raw_data(serde_json::json!([
            VEVENT_CONTROL_ICS,
            CHURN_VTODO_ICS,
            VTODO_ZONED_ICS
        ]));

        let second = connection.diff(&range).await.unwrap();

        assert!(
            second.is_empty(),
            "second sync churned — consult ADR-06's churn table.\n\
             incoming: {:#?}\noutgoing: {:#?}",
            second.incoming(),
            second.outgoing(),
        );
    }

    /// Every `.ics` in the calendar directory, parsed, plus the state dir's
    /// contents. Parsed values, never raw bytes (R30): `to_ics_string` stamps a
    /// fresh `DTSTAMP` on every serialize.
    fn calendar_snapshot(path: &std::path::Path) -> Vec<(String, Option<CalendarItem>)> {
        let mut snapshot: Vec<(String, Option<CalendarItem>)> = walk(path)
            .into_iter()
            .map(|file| {
                let name = file
                    .strip_prefix(path)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                let parsed = CalendarItem::load(&file).ok();
                (name, parsed)
            })
            .collect();
        snapshot.sort_by(|a, b| a.0.cmp(&b.0));
        snapshot
    }

    /// `calendar_snapshot` restricted to calendar data. The sync-state file is
    /// created by the empty-vec flush even on a refused push, and its presence
    /// is not a change to any event.
    fn ics_snapshot(path: &std::path::Path) -> Vec<(String, Option<CalendarItem>)> {
        calendar_snapshot(path)
            .into_iter()
            .filter(|(name, _)| name.ends_with(".ics"))
            .collect()
    }

    fn walk(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
        let mut files = Vec::new();
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                files.extend(walk(&path));
            } else {
                files.push(path);
            }
        }
        files
    }

    #[tokio::test]
    async fn discard_outgoing_diff_deletes_file_for_outgoing_create() {
        let (_tmp, _mock, connection) = writable_connection();
        let event = test_event();
        let cal_event = connection.local().create_event(event.clone()).unwrap();
        let path = cal_event.path().to_path_buf();

        connection
            .discard_outgoing_diff(&outgoing_create_diff(event))
            .unwrap();

        assert!(!path.exists());
    }

    #[tokio::test]
    async fn discard_outgoing_diff_reverts_local_update_to_remote_version() {
        let (_tmp, _mock, connection) = writable_connection();
        let original = test_event();
        let mut modified = original.clone();
        modified.summary = Some("Locally Edited".to_string());
        connection.local().create_event(modified.clone()).unwrap();

        connection
            .discard_outgoing_diff(&outgoing_update_diff(original.clone(), modified))
            .unwrap();

        let reloaded = connection.local().events().unwrap().into_events();
        assert_eq!(reloaded.len(), 1);
        assert_eq!(reloaded[0].event().unwrap().summary, original.summary);
    }

    #[tokio::test]
    async fn discard_outgoing_diff_recreates_file_for_outgoing_delete() {
        let (_tmp, _mock, connection) = writable_connection();
        let event = test_event();

        connection
            .discard_outgoing_diff(&outgoing_delete_diff(event))
            .unwrap();

        let expected_path = connection
            .local()
            .path()
            .join("2026-01-01T1200__test-event.ics");
        assert!(expected_path.is_file());
    }
}
