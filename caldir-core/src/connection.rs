mod error;

use std::collections::HashMap;

use crate::calendar::{CalendarError, SyncBases};
use crate::diff::EventChange;
use crate::event::EventInstanceId;
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
        let mut events_by_instance_id: HashMap<EventInstanceId, CalendarEvent> = self
            .local
            .events()?
            .into_events()
            .into_iter()
            // Keyed by the kind-carrying identity (R36), so a task is
            // reachable here and an event/task UID pair never collides.
            .map(|e| (e.item().instance_id(), e))
            .collect();

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

    // push
    pub async fn apply_outgoing_diff(
        &mut self,
        diff: &CalendarDiff,
    ) -> Result<(), ConnectionError> {
        let mut events_by_instance_id: HashMap<EventInstanceId, CalendarEvent> = self
            .local
            .events()?
            .into_events()
            .into_iter()
            // Keyed by the kind-carrying identity (R36), so a task is
            // reachable here and an event/task UID pair never collides.
            .map(|e| (e.item().instance_id(), e))
            .collect();

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
        let mut events_by_instance_id: HashMap<EventInstanceId, CalendarEvent> = self
            .local
            .events()?
            .into_events()
            .into_iter()
            // Keyed by the kind-carrying identity (R36), so a task is
            // reachable here and an event/task UID pair never collides.
            .map(|e| (e.item().instance_id(), e))
            .collect();

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

async fn push_outgoing_changes(
    remote: &Remote,
    diff: &CalendarDiff,
    events_by_instance_id: &mut HashMap<EventInstanceId, CalendarEvent>,
    sync_bases: &mut Vec<CalendarItem>,
) -> Result<(), ConnectionError> {
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
        incoming_create_diff, incoming_delete_diff, incoming_update_diff, outgoing_create_diff,
        outgoing_delete_diff, outgoing_update_diff, test_caldir, test_event, test_event_time,
        test_mock_provider, test_remote_config, test_remote_params,
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

        let ConnectionError::Remote(remote_err) = err else {
            panic!("expected a remote error, got {err:?}");
        };
        assert_eq!(
            remote_err.to_string(),
            "expected 1 event(s) in ICS, found 0"
        );

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
    /// provider**. Pinned as a *failure*, never a quiet no-op. A task in the
    /// outgoing set aborts the rest of that calendar's push, and the accepted
    /// price is "ugly", not "destructive": every local file survives byte-for-
    /// byte-equivalent, the delete queued behind the task never reaches the
    /// wire, and the base recorded before the abort is still flushed (AC 4).
    #[tokio::test]
    async fn pushing_a_task_to_a_task_less_provider_fails_that_calendar_without_data_loss() {
        let (_tmp, mock, mut connection) = writable_connection();

        let event = test_event();
        let todo = Todo::new("Hand in the assignment");
        let doomed = Event::new("Would be deleted remotely", test_event_time());

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

        // Only the first change ever reaches the wire; queueing exactly one
        // reply is itself the assertion that the other two do not.
        mock.reply::<rpc::CreateEvent>(event.clone());

        let diff = CalendarDiff::from_changes(
            vec![
                EventChange::Create(event.clone().into()),
                EventChange::Create(todo.clone().into()),
                EventChange::Delete(doomed.into()),
            ],
            vec![],
        );

        let result = connection.apply_outgoing_diff(&diff).await;

        assert!(result.is_err(), "the task push must fail the calendar");

        // Not data loss: both files are still there, and still themselves.
        assert!(event_path.is_file());
        assert!(todo_path.is_file());
        assert_eq!(CalendarItem::load(&todo_path).unwrap(), todo.into());

        // The delete queued behind the task never happened. `captured_request`
        // asserts the method of the *last* request, so this is exactly "no
        // DeleteEvent was ever sent".
        assert_eq!(mock.captured_request::<rpc::CreateEvent>().event, event);

        // AC 4: the base recorded before the abort still reached disk.
        let reloaded = Calendar::load(connection.local().path()).unwrap();
        assert_eq!(
            reloaded.state().sync_base(&event.event_instance_id()),
            Some(&event),
        );
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
