mod error;

use std::collections::HashMap;

use crate::calendar::{CalendarError, SyncBases};
use crate::diff::{EventChange, TaskPushes, TaskReads};
use crate::event::EventInstanceId;
use crate::item::CalendarItem;
use crate::{Calendar, CalendarDiff, CalendarEvent, DateRange, Remote, RemoteEvent};
pub use error::ConnectionError;

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

        // C-8/D-2, fail closed: `capabilities()` never propagates an error —
        // an absent, unknown, errored or timed-out answer already reads as
        // `ProviderCapabilities::default()` (all false), so the reason is
        // only for a future surface (P2-05) and is dropped here.
        let (capabilities, _reason) = self.remote().capabilities().await;
        let task_pushes = if capabilities.tasks.write {
            TaskPushes::Allowed
        } else {
            TaskPushes::Withheld
        };
        // C-7/C-8: only an explicit `tasks.read = true` lifts the version-skew
        // belt. Every failure mode `Remote::capabilities()` collapses to
        // `ProviderCapabilities::default()`, whose `tasks.read` is `false` —
        // read the same as an honest `false` here, no second capability path.
        let task_reads = if capabilities.tasks.read {
            TaskReads::Advertised
        } else {
            TaskReads::NotAdvertised
        };

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

        let mut diff = CalendarDiff::compute_from_listing(
            local_listing,
            remote_events,
            sync_bases,
            range,
            task_pushes,
            task_reads,
        );

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

    fn events_by_instance_id(
        &self,
    ) -> Result<HashMap<EventInstanceId, CalendarEvent>, ConnectionError> {
        Ok(self
            .local
            .events()?
            .into_events()
            .into_iter()
            // Keyed by the kind-carrying identity, so a task is
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
    use crate::provider::ProviderCapabilities;
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
        mock.reply::<rpc::Capabilities>(ProviderCapabilities::events_only());
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

    /// A read-only calendar pushes *nothing*, so singling tasks out there
    /// would report a restriction that is not the one in force: the user would
    /// read "1 task change not pushed" on a calendar where no event pushed
    /// either. `discard_outgoing` clears the count with the set.
    #[tokio::test]
    async fn a_read_only_calendar_reports_no_withheld_tasks() {
        let (_tmp, caldir) = test_caldir();
        let calendar = caldir
            .create_calendar("read-only-cal", Some(calendar_config(Some(true))))
            .unwrap();
        calendar.create_event(test_event()).unwrap();
        calendar
            .create_event(Todo::new("Renew the permit"))
            .unwrap();

        let mock = test_mock_provider();
        mock.reply::<rpc::Capabilities>(ProviderCapabilities::events_only());
        mock.reply::<rpc::ListEvents>(vec![]);
        let remote = Remote::new(mock.provider(), test_remote_params());

        let mut connection = Connection::new(calendar, remote);
        let diff = connection.diff(&DateRange::default()).await.unwrap();

        assert!(diff.outgoing().is_empty());
        assert_eq!(
            diff.withheld_task_pushes(),
            0,
            "read-only is the reason nothing pushes here, not the task",
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
        mock.reply::<rpc::Capabilities>(ProviderCapabilities::events_only());
        mock.reply::<rpc::ListEvents>(vec![]);
        let remote = Remote::new(mock.provider(), test_remote_params());

        let mut connection = Connection::new(calendar, remote);
        let diff = connection.diff(&DateRange::default()).await.unwrap();

        assert_eq!(diff.outgoing(), &[EventChange::Create(event.into())]);
    }

    /// P1-03/S3, the connection-level proof. The module-level `CalendarDiff`
    /// tests pass just as well against a hard-coded flag (C-3's whole point
    /// is that `CalendarDiff` cannot see a `Remote`), so only driving the
    /// real `Connection::diff` against a `MockProvider` proves the value is
    /// actually threaded from the provider's answer.
    #[tokio::test]
    async fn the_capability_reported_by_the_provider_decides_task_pushes() {
        use crate::provider::Capability;

        let todo = Todo::new("Hand in the assignment");

        let (_tmp, caldir) = test_caldir();
        let calendar = caldir
            .create_calendar("writable-cal", Some(calendar_config(Some(false))))
            .unwrap();
        calendar.create_event(todo.clone()).unwrap();
        let mock = test_mock_provider();
        mock.reply::<rpc::Capabilities>(ProviderCapabilities {
            events: Capability {
                read: true,
                write: true,
            },
            tasks: Capability {
                read: true,
                write: true,
            },
        });
        mock.reply::<rpc::ListEvents>(vec![]);
        let remote = Remote::new(mock.provider(), test_remote_params());
        let mut connection = Connection::new(calendar, remote);
        let allowed = connection.diff(&DateRange::default()).await.unwrap();

        assert_eq!(
            allowed.outgoing(),
            &[EventChange::Create(CalendarItem::Todo(todo.clone()))],
            "tasks.write=true must let the task reach the outgoing set",
        );
        assert_eq!(allowed.withheld_task_pushes(), 0);

        let (_tmp2, caldir2) = test_caldir();
        let calendar2 = caldir2
            .create_calendar("writable-cal", Some(calendar_config(Some(false))))
            .unwrap();
        calendar2.create_event(todo.clone()).unwrap();
        let mock2 = test_mock_provider();
        mock2.reply::<rpc::Capabilities>(ProviderCapabilities::default());
        mock2.reply::<rpc::ListEvents>(vec![]);
        let remote2 = Remote::new(mock2.provider(), test_remote_params());
        let mut connection2 = Connection::new(calendar2, remote2);
        let withheld = connection2.diff(&DateRange::default()).await.unwrap();

        assert!(
            withheld.outgoing().is_empty(),
            "tasks.write=false must withhold the same task, got {:?}",
            withheld.outgoing(),
        );
        assert_eq!(withheld.withheld_task_pushes(), 1);
    }

    /// P2-01/S5, the connection-level proof that `S3` (capability-aware
    /// diff) and `S5` (item-typed dispatch) are actually wired to each
    /// other, not merely each correct in isolation: `remote::tests` drives
    /// `apply_change` directly and bypasses the diff entirely. Here a task
    /// create is built through `Connection::diff` against a provider
    /// advertising `tasks.write=true`, then pushed through
    /// `apply_outgoing_diff` end to end, and the transport must actually
    /// receive it while the local file survives.
    #[tokio::test]
    async fn a_task_change_reaches_the_provider_when_task_writes_are_advertised() {
        use crate::provider::Capability;

        let (_tmp, caldir) = test_caldir();
        let calendar = caldir
            .create_calendar("writable-cal", Some(calendar_config(Some(false))))
            .unwrap();
        let todo = Todo::new("Hand in the assignment");
        let task_path = calendar
            .create_event(todo.clone())
            .unwrap()
            .path()
            .to_path_buf();

        let mock = test_mock_provider();
        mock.reply::<rpc::Capabilities>(ProviderCapabilities {
            events: Capability {
                read: true,
                write: true,
            },
            tasks: Capability {
                read: true,
                write: true,
            },
        });
        mock.reply::<rpc::ListEvents>(vec![]);
        let remote = Remote::new(mock.provider(), test_remote_params());
        let mut connection = Connection::new(calendar, remote);

        let diff = connection.diff(&DateRange::default()).await.unwrap();
        assert_eq!(
            diff.outgoing(),
            &[EventChange::Create(CalendarItem::Todo(todo.clone()))],
            "tasks.write=true must let the task reach the outgoing set",
        );

        mock.reply::<rpc::CreateItem>(CalendarItem::Todo(todo.clone()));

        connection
            .apply_outgoing_diff(&diff)
            .await
            .expect("a task change must reach the provider, not abort the push");

        assert_eq!(
            mock.captured_request::<rpc::CreateItem>().event,
            CalendarItem::Todo(todo.clone()),
            "the transport must actually receive the task's own item",
        );
        assert!(task_path.is_file(), "the local file must survive the push");
        assert_eq!(CalendarItem::load(&task_path).unwrap(), todo.into());
    }

    /// C-8/`CROSS-5`. A provider whose capabilities call cannot be answered
    /// must not fail the calendar: `Remote::capabilities()` already reads
    /// that as task-less (P1-02), and this pins that `Connection::diff` acts
    /// on it rather than propagating the transport error — the calendar's
    /// events sync exactly as they would for a task-free calendar.
    #[tokio::test]
    async fn a_provider_that_cannot_answer_capabilities_still_syncs_its_events() {
        use crate::provider::transport::ProviderTransportError;
        use std::time::Duration;

        let (_tmp, caldir) = test_caldir();
        let calendar = caldir
            .create_calendar("writable-cal", Some(calendar_config(Some(false))))
            .unwrap();
        let event = test_event();
        calendar.create_event(event.clone()).unwrap();
        let todo = Todo::new("Hand in the assignment");
        calendar.create_event(todo.clone()).unwrap();

        let mock = test_mock_provider();
        mock.reply_error(ProviderTransportError::Timeout(Duration::from_secs(1)));
        mock.reply::<rpc::ListEvents>(vec![]);
        let remote = Remote::new(mock.provider(), test_remote_params());

        let mut connection = Connection::new(calendar, remote);
        let diff = connection.diff(&DateRange::default()).await.unwrap();

        assert_eq!(
            diff.outgoing(),
            &[EventChange::Create(event.into())],
            "the calendar's events must sync exactly as a task-free calendar would",
        );
        assert_eq!(
            diff.withheld_task_pushes(),
            1,
            "a capabilities failure must fail closed, not lose the task silently",
        );
        let _ = todo;
    }

    /// C-8/D-2. A transport error is the failure mode most likely to be
    /// handled with `unwrap_or_default()` — and `Default` on `Capability` is
    /// `false` for `write` *but also* `false` for `read`; only an explicit
    /// test says which reading actually shipped. Pins that a capabilities
    /// transport error withholds the task, counts it, and the withheld
    /// push never reaches the write RPC at all.
    #[tokio::test]
    async fn a_capability_call_that_errors_withholds_every_task_push() {
        use crate::provider::transport::ProviderTransportError;

        let (_tmp, caldir) = test_caldir();
        let calendar = caldir
            .create_calendar("writable-cal", Some(calendar_config(Some(false))))
            .unwrap();
        let todo = Todo::new("Hand in the assignment");
        let todo_path = calendar
            .create_event(todo.clone())
            .unwrap()
            .path()
            .to_path_buf();
        let todo_bytes_before = std::fs::read(&todo_path).unwrap();

        let mock = test_mock_provider();
        mock.reply_error(ProviderTransportError::Io(std::io::Error::other(
            "pipe closed",
        )));
        mock.reply::<rpc::ListEvents>(vec![]);
        let remote = Remote::new(mock.provider(), test_remote_params());

        let mut connection = Connection::new(calendar, remote);
        let diff = connection.diff(&DateRange::default()).await.unwrap();

        assert!(
            diff.outgoing().is_empty(),
            "a capabilities transport error must withhold the task, got {:?}",
            diff.outgoing(),
        );
        assert_eq!(
            diff.withheld_task_pushes(),
            1,
            "the withheld task must be counted, never silently dropped",
        );

        // No further response is queued for a write RPC: if the push
        // attempted to send the withheld task, `MockTransport::exchange`
        // would panic here rather than let this pass.
        connection
            .apply_outgoing_diff(&diff)
            .await
            .expect("a task-less push must still succeed");

        assert_eq!(
            std::fs::read(&todo_path).unwrap(),
            todo_bytes_before,
            "the withheld task's file must be byte-identical to before the push",
        );
    }

    /// D-2's absent-field clause, driven through `Connection::diff` rather
    /// than `ProviderCapabilities` alone: `serde`'s `#[serde(default)]` makes
    /// an omitted `tasks` object compile and decode into a struct — whether
    /// that struct is safe to act on is a decision, not a default.
    #[tokio::test]
    async fn an_absent_tasks_field_is_read_as_no_task_write_support() {
        let (_tmp, caldir) = test_caldir();
        let calendar = caldir
            .create_calendar("writable-cal", Some(calendar_config(Some(false))))
            .unwrap();
        let todo = Todo::new("Hand in the assignment");
        let todo_path = calendar
            .create_event(todo.clone())
            .unwrap()
            .path()
            .to_path_buf();
        let todo_bytes_before = std::fs::read(&todo_path).unwrap();

        let mock = test_mock_provider();
        // The shape an older, partially-upgraded provider sends: an `events`
        // object and no `tasks` object at all.
        mock.reply_raw_data(serde_json::json!({
            "events": { "read": true, "write": true },
        }));
        mock.reply::<rpc::ListEvents>(vec![]);
        let remote = Remote::new(mock.provider(), test_remote_params());

        let mut connection = Connection::new(calendar, remote);
        let diff = connection.diff(&DateRange::default()).await.unwrap();

        assert!(
            diff.outgoing().is_empty(),
            "an absent tasks field must withhold the task, got {:?}",
            diff.outgoing(),
        );
        assert_eq!(diff.withheld_task_pushes(), 1);

        connection
            .apply_outgoing_diff(&diff)
            .await
            .expect("a task-less push must still succeed");

        assert_eq!(std::fs::read(&todo_path).unwrap(), todo_bytes_before);
    }

    /// C-4/§17. The real old-binary path: a provider built before
    /// `capabilities` existed answers `unsupported_method`, which must
    /// withhold the task exactly as an honest `false` would — while the
    /// reason `Remote::capabilities()` preserves still names the provider
    /// and the method, never degrading to `Failed to parse request`.
    #[tokio::test]
    async fn an_unknown_capabilities_method_withholds_every_task_push() {
        use crate::provider::ProviderError;
        use crate::provider::transport::mock_transport::MockTransport;
        use crate::{Provider, ProviderSlug, RemoteError};
        use std::sync::Arc;

        // The reason surfaced directly off `Remote::capabilities()`.
        let error_transport = Arc::new(MockTransport::with_response(
            r#"{"status":"error","error":"test-provider does not support the \"capabilities\" command; the provider may need updating","code":"unsupported_method"}"#,
        ));
        let error_provider =
            Provider::with_transport(ProviderSlug::from("test-provider"), error_transport);
        let error_remote = Remote::new(error_provider, test_remote_params());
        let (capabilities, reason) = error_remote.capabilities().await;

        assert_eq!(capabilities, ProviderCapabilities::default());
        match reason {
            Some(RemoteError::Provider(ProviderError::UnsupportedMethod(msg))) => {
                assert!(
                    msg.contains("test-provider"),
                    "the reason must name the provider: {msg}",
                );
                assert!(
                    msg.contains("capabilities"),
                    "the reason must name the method: {msg}",
                );
            }
            other => panic!(
                "expected an UnsupportedMethod reason naming the provider and the method, got {other:?}"
            ),
        }

        // The withholding itself, through `Connection::diff`.
        let (_tmp, caldir) = test_caldir();
        let calendar = caldir
            .create_calendar("writable-cal", Some(calendar_config(Some(false))))
            .unwrap();
        let todo = Todo::new("Hand in the assignment");
        let todo_path = calendar
            .create_event(todo.clone())
            .unwrap()
            .path()
            .to_path_buf();
        let todo_bytes_before = std::fs::read(&todo_path).unwrap();

        let transport = Arc::new(MockTransport::empty());
        transport.set_response(
            r#"{"status":"error","error":"unknown command: capabilities","code":"unsupported_method"}"#,
        );
        transport.set_response(serde_json::json!({"status": "success", "data": []}).to_string());
        let provider = Provider::with_transport(ProviderSlug::from("test-provider"), transport);
        let remote = Remote::new(provider, test_remote_params());

        let mut connection = Connection::new(calendar, remote);
        let diff = connection.diff(&DateRange::default()).await.unwrap();

        assert!(
            diff.outgoing().is_empty(),
            "an unknown capabilities method must withhold the task, got {:?}",
            diff.outgoing(),
        );
        assert_eq!(diff.withheld_task_pushes(), 1);

        connection
            .apply_outgoing_diff(&diff)
            .await
            .expect("a task-less push must still succeed");

        assert_eq!(std::fs::read(&todo_path).unwrap(), todo_bytes_before);
    }

    /// `CROSS-5` names deletion *and* corruption: a withheld push that still
    /// rewrote the local file from a stale base would pass a "not pushed"
    /// assertion and still corrupt the file. Drives a real create-only cycle
    /// through both `diff` and `apply_outgoing_diff` and checks the task's
    /// file byte-for-byte, not merely that no RPC fired for it.
    #[tokio::test]
    async fn a_provider_without_task_writes_never_loses_a_local_vtodo() {
        let (_tmp, mock, mut connection) = writable_connection();

        let event = test_event();
        let todo = Todo::new("Hand in the assignment");
        let todo_path = connection
            .local()
            .create_event(todo.clone())
            .unwrap()
            .path()
            .to_path_buf();
        connection.local().create_event(event.clone()).unwrap();
        let todo_bytes_before = std::fs::read(&todo_path).unwrap();

        mock.reply::<rpc::Capabilities>(ProviderCapabilities::events_only());
        mock.reply::<rpc::ListEvents>(vec![]);
        let diff = connection.diff(&DateRange::default()).await.unwrap();

        assert_eq!(
            diff.outgoing(),
            &[EventChange::Create(event.clone().into())],
            "the event pushes, the task does not: {:?}",
            diff.outgoing(),
        );
        assert!(
            !diff
                .incoming()
                .iter()
                .any(|c| matches!(c, EventChange::Delete(CalendarItem::Todo(_)))),
            "no task delete may appear on the incoming side either, got {:?}",
            diff.incoming(),
        );
        assert!(
            !diff
                .outgoing()
                .iter()
                .any(|c| matches!(c, EventChange::Delete(CalendarItem::Todo(_)))),
            "no task delete may appear on the outgoing side either, got {:?}",
            diff.outgoing(),
        );

        mock.reply::<rpc::CreateItem>(CalendarItem::Event(event.clone()));
        connection
            .apply_outgoing_diff(&diff)
            .await
            .expect("a task in the calendar may not fail the push");

        assert_eq!(
            std::fs::read(&todo_path).unwrap(),
            todo_bytes_before,
            "the task file must be byte-identical to before the push",
        );
        assert_eq!(
            CalendarItem::load(&todo_path).unwrap(),
            CalendarItem::Todo(todo),
            "the task file must still parse to the same item",
        );
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
        mock.reply::<rpc::Capabilities>(ProviderCapabilities::events_only());
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

        mock.reply::<rpc::Capabilities>(ProviderCapabilities::events_only());
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
        mock.reply::<rpc::Capabilities>(ProviderCapabilities::events_only());
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

        mock.reply::<rpc::CreateItem>(CalendarItem::Event(event.clone()));
        connection
            .apply_outgoing_diff(&outgoing_create_diff(event.clone()))
            .await
            .unwrap();

        assert_eq!(
            mock.captured_request::<rpc::CreateItem>().event,
            CalendarItem::Event(event)
        );
    }

    #[tokio::test]
    async fn apply_outgoing_diff_sends_update_event_for_outgoing_update() {
        let (_tmp, mock, mut connection) = writable_connection();
        let from = test_event();
        let mut to = from.clone();
        to.summary = Some("Updated".into());
        connection.local().create_event(to.clone()).unwrap();

        mock.reply::<rpc::UpdateItem>(CalendarItem::Event(to.clone()));
        connection
            .apply_outgoing_diff(&outgoing_update_diff(from, to.clone()))
            .await
            .unwrap();

        assert_eq!(
            mock.captured_request::<rpc::UpdateItem>().event,
            CalendarItem::Event(to)
        );
    }

    #[tokio::test]
    async fn apply_outgoing_diff_sends_delete_event_for_outgoing_delete() {
        let (_tmp, mock, mut connection) = writable_connection();
        let event = test_event();

        mock.reply::<rpc::DeleteItem>(());
        connection
            .apply_outgoing_diff(&outgoing_delete_diff(event.clone()))
            .await
            .unwrap();

        assert_eq!(
            mock.captured_request::<rpc::DeleteItem>().event,
            CalendarItem::Event(event)
        );
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
        mock.reply::<rpc::CreateItem>(CalendarItem::Event(canonical.clone()));

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
        mock.reply::<rpc::CreateItem>(CalendarItem::Event(canonical.clone()));

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

        mock.reply::<rpc::CreateItem>(CalendarItem::Event(event.clone()));
        connection
            .apply_outgoing_diff(&outgoing_create_diff(event.clone()))
            .await
            .unwrap();

        assert_eq!(connection.local().state().sync_base(&id), Some(&event));
    }

    /// P2-02: a provider advertising task writes, used to drive a real
    /// outgoing task change through `Connection::diff`. `CalendarDiff::
    /// from_changes` (behind `outgoing_create_diff`/`outgoing_update_diff`)
    /// withholds every task unconditionally (R70), so it cannot produce the
    /// shape these tests need — only the capability-aware production path can.
    fn task_writable_capabilities() -> ProviderCapabilities {
        use crate::provider::Capability;
        ProviderCapabilities {
            events: Capability {
                read: true,
                write: true,
            },
            tasks: Capability {
                read: true,
                write: true,
            },
        }
    }

    /// P2-02/§15. `create -> sync -> sync`: the churn gate for tasks. The
    /// first pass must do real work or the second pass proving nothing is
    /// free; the second, against a remote that now reports the pushed task
    /// back, must be empty.
    #[tokio::test]
    async fn creating_a_task_then_syncing_twice_yields_an_empty_second_diff() {
        let (_tmp, mock, mut connection) = writable_connection();
        let todo = Todo::new("Hand in the assignment");
        connection.local().create_event(todo.clone()).unwrap();

        mock.reply::<rpc::Capabilities>(task_writable_capabilities());
        mock.reply::<rpc::ListEvents>(vec![]);
        let first = connection.diff(&DateRange::default()).await.unwrap();

        assert!(
            !first.is_empty(),
            "the first sync must actually do work, or this gate proves nothing",
        );
        assert_eq!(
            first.outgoing(),
            &[EventChange::Create(CalendarItem::Todo(todo.clone()))],
        );

        mock.reply::<rpc::CreateItem>(CalendarItem::Todo(todo.clone()));
        connection.apply_outgoing_diff(&first).await.unwrap();

        // A fresh process reads its bases from disk, not the in-memory map
        // the apply just populated.
        connection.local = Calendar::load(connection.local().path()).unwrap();

        mock.reply::<rpc::Capabilities>(task_writable_capabilities());
        mock.reply::<rpc::ListEvents>(vec![CalendarItem::Todo(todo)]);
        let second = connection.diff(&DateRange::default()).await.unwrap();

        assert!(
            second.is_empty(),
            "second sync churned a task create — incoming: {:#?}\noutgoing: {:#?}",
            second.incoming(),
            second.outgoing(),
        );
    }

    /// P2-02/§15. `update -> sync -> sync`. The create-only gate above cannot
    /// see this: a base recorded on create but never refreshed on update
    /// flips `update_direction` forever, which only a *second* update-path
    /// diff exposes.
    #[tokio::test]
    async fn updating_a_task_then_syncing_twice_yields_an_empty_second_diff() {
        let (_tmp, mock, mut connection) = writable_connection();

        let original = Todo::new("Hand in the assignment");
        let mut cal_event = connection.local().create_event(original.clone()).unwrap();
        connection
            .local
            .record_sync_bases(vec![CalendarItem::Todo(original.clone())])
            .unwrap();

        let mut updated = original.clone();
        updated.summary = Some("Hand in the final assignment".to_string());
        cal_event
            .update(CalendarItem::Todo(updated.clone()))
            .unwrap();

        mock.reply::<rpc::Capabilities>(task_writable_capabilities());
        mock.reply::<rpc::ListEvents>(vec![CalendarItem::Todo(original.clone())]);
        let first = connection.diff(&DateRange::default()).await.unwrap();

        assert!(
            !first.is_empty(),
            "the first sync must actually do work, or this gate proves nothing",
        );
        assert_eq!(
            first.outgoing(),
            &[EventChange::Update {
                from: CalendarItem::Todo(original),
                to: CalendarItem::Todo(updated.clone()),
            }],
        );

        mock.reply::<rpc::UpdateItem>(CalendarItem::Todo(updated.clone()));
        connection.apply_outgoing_diff(&first).await.unwrap();

        connection.local = Calendar::load(connection.local().path()).unwrap();

        mock.reply::<rpc::Capabilities>(task_writable_capabilities());
        mock.reply::<rpc::ListEvents>(vec![CalendarItem::Todo(updated)]);
        let second = connection.diff(&DateRange::default()).await.unwrap();

        assert!(
            second.is_empty(),
            "second sync churned a task update — incoming: {:#?}\noutgoing: {:#?}",
            second.incoming(),
            second.outgoing(),
        );
    }

    /// P2-02/§7. Recording the *sent* item (the natural-looking
    /// implementation) is invisible against a provider that echoes its
    /// input, and is exactly what makes a canonicalizing provider churn
    /// forever. The base must be the item the provider *returned*.
    #[tokio::test]
    async fn a_pushed_task_records_the_providers_canonical_item_as_its_base() {
        let (_tmp, mock, mut connection) = writable_connection();
        let todo = Todo::new("Hand in the assignment");
        connection.local().create_event(todo.clone()).unwrap();

        mock.reply::<rpc::Capabilities>(task_writable_capabilities());
        mock.reply::<rpc::ListEvents>(vec![]);
        let diff = connection.diff(&DateRange::default()).await.unwrap();

        let mut canonical = todo.clone();
        canonical.x_properties.push(XProperty::new(
            "X-PROVIDER-TASK-ID".to_string(),
            "abc123".to_string(),
        ));
        mock.reply::<rpc::CreateItem>(CalendarItem::Todo(canonical.clone()));

        connection.apply_outgoing_diff(&diff).await.unwrap();

        let id = CalendarItem::Todo(canonical.clone()).instance_id();
        assert_eq!(
            connection.local().state().sync_bases().get(&id),
            Some(&Some(Box::new(CalendarItem::Todo(canonical)))),
            "the base must be the provider's returned item, not the item that was sent",
        );
    }

    /// P2-02/§6. `push_outgoing_changes` looks the local event up by
    /// `original_event_id`, computed from the *sent* change. If that lookup
    /// misses for a task, the push silently leaves the old file in place and
    /// the next sync creates a duplicate.
    #[tokio::test]
    async fn a_provider_reassigned_task_uid_rewrites_the_local_file() {
        let (_tmp, mock, mut connection) = writable_connection();
        let todo = Todo::new("Hand in the assignment");
        let original_id = CalendarItem::Todo(todo.clone()).instance_id();
        connection.local().create_event(todo.clone()).unwrap();

        mock.reply::<rpc::Capabilities>(task_writable_capabilities());
        mock.reply::<rpc::ListEvents>(vec![]);
        let diff = connection.diff(&DateRange::default()).await.unwrap();

        let mut canonical = todo.clone();
        canonical.uid = EventUid::new("provider-assigned-task-uid@example.com");
        let canonical_id = CalendarItem::Todo(canonical.clone()).instance_id();
        assert_ne!(original_id, canonical_id);
        mock.reply::<rpc::CreateItem>(CalendarItem::Todo(canonical.clone()));

        connection.apply_outgoing_diff(&diff).await.unwrap();

        let reloaded = connection.local().events().unwrap().into_events();
        assert_eq!(
            reloaded.len(),
            1,
            "no second file may be left behind, got {reloaded:?}",
        );
        assert_eq!(reloaded[0].item(), &CalendarItem::Todo(canonical.clone()));

        let synced = connection.local().state().synced_event_ids();
        assert!(synced.contains(&canonical_id));
        assert!(!synced.contains(&original_id));
    }

    /// P2-02/§7. `record` writes both `known_event_ids` and the per-item base
    /// directory; a base that never reached disk is churn on the next
    /// process, which is the only run that matters.
    #[tokio::test]
    async fn a_pushed_task_base_survives_a_reload_from_disk() {
        let (_tmp, mock, mut connection) = writable_connection();
        let todo = Todo::new("Hand in the assignment");
        connection.local().create_event(todo.clone()).unwrap();

        mock.reply::<rpc::Capabilities>(task_writable_capabilities());
        mock.reply::<rpc::ListEvents>(vec![]);
        let diff = connection.diff(&DateRange::default()).await.unwrap();

        mock.reply::<rpc::CreateItem>(CalendarItem::Todo(todo.clone()));
        connection.apply_outgoing_diff(&diff).await.unwrap();

        let id = CalendarItem::Todo(todo.clone()).instance_id();
        let reloaded = Calendar::load(connection.local().path()).unwrap();
        assert_eq!(
            reloaded.state().sync_bases().get(&id),
            Some(&Some(Box::new(CalendarItem::Todo(todo)))),
            "the base must survive a reload from disk, not just live in memory",
        );
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
        mock.reply::<rpc::CreateItem>(CalendarItem::Event(event_a.clone()));
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
    /// `BEGIN:VTODO` payload can never reach `CreateEvent`.
    ///
    /// What survives, and is the half R1/A-1 item 3 actually calls inviolable:
    /// **no VEVENT is synthesized for a task anywhere in the chain**. Proven
    /// end to end here — real scan, real diff, real apply — by the transport
    /// having seen nothing at all.
    ///
    /// **Changed from `a_task_push_aborts_and_never_reaches_the_provider_as_a_
    /// vevent`, which locked the whole-push refusal.** It asserted
    /// `ConnectionError::UnpushableTask { uid, file, pending: 0 }`; a task is
    /// now dropped from the outgoing set instead of refusing the push, so the
    /// call *succeeds* with nothing to do. Every no-VEVENT assertion below is
    /// verbatim from the old test — only the error destructuring is gone,
    /// replaced by the withheld count, which is what R19 now hangs on.
    #[tokio::test]
    async fn a_task_never_reaches_the_provider_as_a_vevent() {
        let (_tmp, mock, mut connection) = writable_connection();
        let todo = Todo::new("Hand in the assignment");
        let cal_event = connection.local().create_event(todo.clone()).unwrap();
        let path = cal_event.path().to_path_buf();

        let diff = outgoing_create_diff(todo.clone());
        assert!(
            diff.outgoing().is_empty(),
            "a task is never outgoing, got {:?}",
            diff.outgoing(),
        );
        assert_eq!(diff.withheld_task_pushes(), 1);

        connection
            .apply_outgoing_diff(&diff)
            .await
            .expect("a task-only calendar has nothing to push, not a failure");

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

    /// **The wedge.** CalDAV is the only provider that pulls tasks, so the
    /// intended user pulls a mixed calendar, edits one task, and pushes. Under
    /// the whole-push refusal that ticked-off task refused the *calendar*, and
    /// their meetings stopped syncing on every run until they undid the pull.
    ///
    /// The contract this pins: a task is never outgoing at all. The events
    /// around it push exactly as they would in a task-free calendar, and the
    /// withheld task is counted so the CLI can say so (R19).
    ///
    /// Built through the real path — real files, real `diff()` — because the
    /// withholding has to happen where the outgoing set is *computed*, not at
    /// a pre-flight a caller may skip.
    #[tokio::test]
    async fn a_task_in_a_mixed_calendar_is_withheld_and_every_event_still_pushes() {
        let (_tmp, mock, mut connection) = writable_connection();

        let events: Vec<Event> = (0..6)
            .map(|i| Event::new(format!("Lecture {i}"), test_event_time()))
            .collect();
        for event in &events {
            connection.local().create_event(event.clone()).unwrap();
        }
        let todo = Todo::new("Hand in the assignment");
        let todo_path = connection
            .local()
            .create_event(todo.clone())
            .unwrap()
            .path()
            .to_path_buf();
        let todo_bytes_before = std::fs::read(&todo_path).unwrap();

        mock.reply::<rpc::Capabilities>(ProviderCapabilities::events_only());
        mock.reply::<rpc::ListEvents>(vec![]);
        for event in &events {
            mock.reply::<rpc::CreateItem>(CalendarItem::Event(event.clone()));
        }

        let diff = connection.diff(&DateRange::default()).await.unwrap();

        assert_eq!(
            diff.outgoing().len(),
            events.len(),
            "every event is outgoing and the task is not, got {:?}",
            diff.outgoing(),
        );
        assert!(
            !diff
                .outgoing()
                .iter()
                .any(|c| matches!(c, EventChange::Create(CalendarItem::Todo(_)))),
            "no task may be outgoing, got {:?}",
            diff.outgoing(),
        );
        assert_eq!(
            diff.withheld_task_pushes(),
            1,
            "the withheld task must be counted, never silently dropped (R19)",
        );

        connection
            .apply_outgoing_diff(&diff)
            .await
            .expect("a task in the calendar may not fail the push");

        // Every event really went out, and every base was recorded: the next
        // run must not re-push them.
        let reloaded = Calendar::load(connection.local().path()).unwrap();
        for event in &events {
            let id = event.event_instance_id();
            assert_eq!(reloaded.state().sync_base(&id), Some(event), "{id}");
        }

        // The task is untouched on disk and has no base: it never synced.
        assert_eq!(std::fs::read(&todo_path).unwrap(), todo_bytes_before);
        let todo_id = CalendarItem::Todo(todo).instance_id();
        assert_eq!(reloaded.state().sync_bases().get(&todo_id), None);
        assert!(!reloaded.state().synced_event_ids().contains(&todo_id));
    }

    /// `T-50` — R56, the first accepted skew failure: **new core + old
    /// provider**.
    ///
    /// **Renamed from `pushing_a_task_to_a_task_less_provider_fails_that_
    /// calendar_without_data_loss`, which locked the whole-push refusal.** It
    /// asserted `expect_err(...)` plus `ConnectionError::UnpushableTask` and
    /// `!mock.received_a_request()` — the task failed the calendar whole. That
    /// contract is what the upstream review rejected: CalDAV is the only
    /// provider that pulls tasks, so failing the calendar is what the intended
    /// user gets on every run, and their meetings stop syncing.
    ///
    /// The contract now: the task is simply not outgoing. The event create and
    /// the delete queued around it push exactly as they would in a task-free
    /// calendar, and the withheld task is counted (R19). Every no-data-loss
    /// assertion below is unchanged; the "nothing reached the wire" assertions
    /// became their opposite for the *events*, which is the fix.
    #[tokio::test]
    async fn pushing_a_task_to_a_task_less_provider_withholds_only_the_task() {
        let (_tmp, mock, mut connection) = writable_connection();

        let event = test_event();
        let todo = Todo::new("Hand in the assignment");
        let doomed = Event::new("Deleted remotely", test_event_time());
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

        mock.reply::<rpc::CreateItem>(CalendarItem::Event(event.clone()));
        mock.reply::<rpc::DeleteItem>(());

        let diff = CalendarDiff::from_changes(
            vec![
                EventChange::Create(event.clone().into()),
                EventChange::Create(todo.clone().into()),
                EventChange::Delete(doomed.into()),
            ],
            vec![],
        );

        assert_eq!(
            diff.outgoing().len(),
            2,
            "the event create and the delete survive, the task does not: {:?}",
            diff.outgoing(),
        );
        assert_eq!(
            diff.withheld_task_pushes(),
            1,
            "the withheld task must be counted, never silently dropped",
        );

        let todo_bytes_before = std::fs::read(&todo_path).unwrap();

        connection
            .apply_outgoing_diff(&diff)
            .await
            .expect("a task may not fail the calendar's push");

        // Not data loss: both files are still there, and still themselves.
        assert!(event_path.is_file());
        assert!(todo_path.is_file());
        assert_eq!(CalendarItem::load(&todo_path).unwrap(), todo.clone().into());
        assert_eq!(
            std::fs::read(&todo_path).unwrap(),
            todo_bytes_before,
            "the withheld item's file must be byte-identical to before the push",
        );

        // The events really went out, and their bases were recorded.
        let reloaded = Calendar::load(connection.local().path()).unwrap();
        let event_id = event.event_instance_id();
        assert_eq!(reloaded.state().sync_base(&event_id), Some(&event));
        assert_eq!(
            mock.captured_request::<rpc::DeleteItem>()
                .event
                .instance_id(),
            doomed_id,
            "the delete queued behind the task must reach the wire too",
        );

        // The task synced nothing, so it has no base: the next run must not
        // read it as already pushed.
        let todo_id = CalendarItem::Todo(todo).instance_id();
        assert_eq!(reloaded.state().sync_bases().get(&todo_id), None);
        assert!(!reloaded.state().synced_event_ids().contains(&todo_id));
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
        mock.reply::<rpc::Capabilities>(ProviderCapabilities::events_only());
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

        mock.reply::<rpc::Capabilities>(ProviderCapabilities::events_only());
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
        mock.reply::<rpc::Capabilities>(ProviderCapabilities::events_only());
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
        mock.reply::<rpc::Capabilities>(ProviderCapabilities::events_only());
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
    /// `Delete` reach the same event-only wire, but nothing pinned them.
    ///
    /// It matters because `push_outgoing_changes` carries
    /// `unreachable!("apply_change returns None for Delete")`. That branch is
    /// safe only while a `Delete` can never return `Some`, and a task-only
    /// delete is the one shape that would otherwise walk into it. The other
    /// half of that invariant — an *event* delete really does yield `None` —
    /// is pinned in `remote::tests::apply_change_yields_no_remote_event_for_a_delete`.
    ///
    /// **Changed from `a_task_only_delete_aborts_before_reaching_the_provider`,
    /// which locked the whole-push refusal**: it asserted `expect_err` and
    /// `ConnectionError::UnpushableTask`. The delete is now withheld from the
    /// outgoing set instead, so it still never reaches the loop — the reason
    /// the `unreachable!` stays safe is unchanged, only the mechanism is.
    ///
    /// R50's counterpart applies here: caldir will not delete a task remotely,
    /// so a locally-deleted task stays on the server and is reported on every
    /// run rather than acted on.
    #[tokio::test]
    async fn a_task_only_delete_is_withheld_before_reaching_the_provider() {
        let (_tmp, mock, mut connection) = writable_connection();

        let task = Todo::new("Renew the parking permit");
        let task_path = connection
            .local()
            .create_event(task.clone())
            .unwrap()
            .path()
            .to_path_buf();

        let diff = outgoing_delete_diff(task.clone());
        assert!(
            diff.outgoing().is_empty(),
            "a task delete is never outgoing, got {:?}",
            diff.outgoing(),
        );
        assert_eq!(diff.withheld_task_pushes(), 1);

        connection
            .apply_outgoing_diff(&diff)
            .await
            .expect("a withheld delete is not a failure");

        assert!(
            !mock.received_a_request(),
            "nothing may reach the transport for a task delete",
        );
        assert!(task_path.is_file(), "the local file must survive");
        assert_eq!(CalendarItem::load(&task_path).unwrap(), task.into());
    }

    /// **L6-01 / QA.** The silent-abandonment lock, kept and inverted.
    ///
    /// Reproduced against real Radicale: a calendar holding one task plus
    /// several pending event pushes pushed *some* of the events, aborted on
    /// the task, and abandoned the rest — silently, and in `read_dir` order,
    /// so *which* changes survived was arbitrary. The whole-push refusal fixed
    /// the arbitrariness by pushing *none* of them, which the upstream review
    /// rejected: on the only task-carrying provider that is the normal path,
    /// and it wedges the user's meetings.
    ///
    /// **Changed from `a_task_in_the_outgoing_set_refuses_the_whole_push_
    /// before_the_first_rpc`**, whose `!mock.received_a_request()`, snapshot
    /// equality and "no base for any event" assertions all inverted here. What
    /// the two share, and what the original defect actually was, is that
    /// *which* changes apply must not depend on `read_dir` order: every event
    /// applies, on every run, regardless of where the task sorts.
    #[tokio::test]
    async fn a_task_mid_set_is_withheld_and_every_event_around_it_still_pushes() {
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
        let todo_before = std::fs::read(&todo_path).unwrap();

        for event in &events {
            mock.reply::<rpc::CreateItem>(CalendarItem::Event(event.clone()));
        }

        let mut changes: Vec<EventChange> = events
            .iter()
            .map(|e| EventChange::Create(e.clone().into()))
            .collect();
        // Mid-set, not last: the original defect abandoned whatever followed.
        changes.insert(3, EventChange::Create(todo.clone().into()));

        let diff = CalendarDiff::from_changes(changes, vec![]);
        assert_eq!(diff.outgoing().len(), events.len());
        assert_eq!(diff.withheld_task_pushes(), 1);

        connection
            .apply_outgoing_diff(&diff)
            .await
            .expect("a task mid-set may not fail the push");

        // Every single event applied — not the ones that happened to sort
        // before the task, all of them.
        let reloaded = Calendar::load(connection.local().path()).unwrap();
        for event in &events {
            let id = event.event_instance_id();
            assert_eq!(reloaded.state().sync_base(&id), Some(event), "{id}");
        }

        // The task moved nowhere: same bytes, no base, no synced id.
        assert_eq!(std::fs::read(&todo_path).unwrap(), todo_before);
        let todo_id = CalendarItem::Todo(todo).instance_id();
        assert_eq!(reloaded.state().sync_bases().get(&todo_id), None);
        assert!(!reloaded.state().synced_event_ids().contains(&todo_id));
    }

    /// **R65.** A same-UID event/task pair is a perfectly good *local*
    /// directory (R4, and `T-52` above proves it works). What cannot work is
    /// putting *both* into one CalDAV collection, which enforces UID
    /// uniqueness component-agnostically.
    ///
    /// It cannot happen any more, and this pins why: the task is never
    /// outgoing, so only one of the pair is ever offered to the collection.
    /// The pre-flight that used to refuse the push whole is gone with it.
    ///
    /// **Replaces three tests, all locks on that refusal:**
    /// `a_same_uid_event_task_pair_pushed_to_one_collection_is_an_error`
    /// (asserted `expect_err` + `!mock.received_a_request()`),
    /// `a_same_uid_pair_is_refused_as_an_unpushable_task_not_as_a_uid_clash`
    /// (pinned *which* of two refusals fired — a question with no referent
    /// once neither exists), and
    /// `a_pair_split_across_a_delete_and_a_create_is_refused_too` (the same
    /// refusal under a delete/create split, covered by the second half here).
    ///
    /// The server's own protection is untouched and is where it belongs:
    /// `caldav`'s `If-None-Match: *` makes a create onto an occupied UID fail
    /// rather than blind-overwrite, pinned live in
    /// `live::radicale::pushing_a_same_uid_pair_into_one_collection_surfaces_an_error`.
    #[tokio::test]
    async fn a_same_uid_event_task_pair_offers_only_the_event_to_the_collection() {
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

        mock.reply::<rpc::CreateItem>(CalendarItem::Event(event.clone()));

        let diff = CalendarDiff::from_changes(
            vec![
                EventChange::Create(event.clone().into()),
                EventChange::Create(todo.clone().into()),
            ],
            vec![],
        );

        assert_eq!(
            diff.outgoing(),
            &[EventChange::Create(event.clone().into())],
            "only the event half of the pair may be outgoing",
        );
        assert_eq!(diff.withheld_task_pushes(), 1);

        connection.apply_outgoing_diff(&diff).await.unwrap();

        assert_eq!(
            mock.captured_request::<rpc::CreateItem>().event.uid(),
            &uid,
            "the event is what reaches the collection",
        );

        // The local mixed directory stays supported (R4): both files survive,
        // and the task's bytes are untouched by a push it took no part in.
        assert!(event_path.is_file() && todo_path.is_file());
        assert_eq!(std::fs::read(&todo_path).unwrap(), before.1);

        // The same pair split across a delete and a create: still one half.
        let split = CalendarDiff::from_changes(
            vec![
                EventChange::Delete(event.into()),
                EventChange::Create(todo.into()),
            ],
            vec![],
        );
        assert_eq!(split.outgoing().len(), 1);
        assert_eq!(split.withheld_task_pushes(), 1);
        assert!(matches!(
            split.outgoing()[0],
            EventChange::Delete(CalendarItem::Event(_))
        ));
    }

    /// R1/R2, the other side of the withholding predicate: it keys on the
    /// component **kind** and nothing wider. Two same-UID items of the *same*
    /// kind — a recurrence override and its master — are ordinary event sync
    /// and must still push, in full. Widening the predicate breaks that.
    #[tokio::test]
    async fn two_same_uid_events_are_not_a_pair_and_still_push() {
        let (_tmp, mock, mut connection) = writable_connection();

        let first = test_event();
        let mut second = Event::new("Same UID, same kind", test_event_time());
        second.uid = first.uid.clone();

        connection.local().create_event(first.clone()).unwrap();

        mock.reply::<rpc::CreateItem>(CalendarItem::Event(first.clone()));
        mock.reply::<rpc::CreateItem>(CalendarItem::Event(second.clone()));

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
        mock.reply::<rpc::Capabilities>(ProviderCapabilities::events_only());
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
        mock.reply::<rpc::CreateItem>(CalendarItem::Event(event.clone()));
        connection.apply_outgoing_diff(&first).await.unwrap();

        // A second `caldir pull` is a fresh process: reload the calendar from
        // disk so pass two reads its bases from the state directory rather than
        // from the in-memory map the apply just populated. A base that never
        // reached disk is churn too.
        connection.local = Calendar::load(connection.local().path()).unwrap();

        // Pass two: nothing changed on either side. The remote now reports all
        // three items — the same bytes it was handed for the tasks, and the
        // event it just accepted.
        mock.reply::<rpc::Capabilities>(ProviderCapabilities::events_only());
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
