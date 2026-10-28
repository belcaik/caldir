//! Task identity for task stores that keep no iCalendar UID.
//!
//! Google Tasks stores no `UID`: its provider derives one from the task id on
//! every listing (`<id>@google-tasks.com`), while a task caldir pushed keeps
//! the local UID it was created under, plus the provider id the push wrote
//! back (`X-GOOGLE-TASK-ID`). Diffed by UID alone, the same task reads as
//! "local present, remote gone" plus "remote new": a delete pulled into the
//! local file and a second copy pulled under the derived UID, on every run.
//!
//! Before the diff, a remote task is therefore given back the local UID that
//! carries its provider id. Only local *files* are consulted, never sync
//! bases: a base whose file is gone must not turn a remote task into a remote
//! delete just because the two share an id.

use std::collections::{HashMap, HashSet};

use crate::event::{EventInstanceId, EventUid};
use crate::item::CalendarItem;
use crate::todo::Todo;
use crate::{CalendarEvent, RemoteEvent};

/// X-properties carrying a task's id in a store that has no iCalendar UID.
/// Providers stamp them on every task they list and every push response.
const PROVIDER_TASK_ID_PROPERTIES: [&str; 1] = ["X-GOOGLE-TASK-ID"];

/// Result of [`adopt_local_task_uids`].
pub(super) struct AdoptedTasks {
    pub(super) remote_events: Vec<RemoteEvent>,
    /// Local tasks stored under a remote task's provider-derived UID while
    /// another local file already holds that task under its own UID. Stale
    /// copies of a task the diff now sees under its local UID.
    pub(super) shadows: Vec<CalendarItem>,
}

/// Rewrites each remote task's UID to the one local task file carrying the
/// same provider id. Left untouched when no local file or more than one local
/// UID carries that id, when the local UID is itself listed remotely, or when
/// two remote tasks claim the same local UID.
pub(super) fn adopt_local_task_uids(
    remote_events: Vec<RemoteEvent>,
    local_events: &[CalendarEvent],
) -> AdoptedTasks {
    let mut local_uids_by_provider_id: HashMap<(&str, &str), HashSet<&EventUid>> = HashMap::new();
    let mut local_by_id: HashMap<EventInstanceId, &CalendarItem> = HashMap::new();

    for local in local_events {
        let item = local.item();
        local_by_id.insert(item.instance_id(), item);

        let Some(todo) = plain_task(item) else {
            continue;
        };

        for key in provider_ids(todo) {
            local_uids_by_provider_id
                .entry(key)
                .or_default()
                .insert(&todo.uid);
        }
    }

    let remote_uids: HashSet<&EventUid> = remote_events
        .iter()
        .filter_map(|remote| plain_task(remote.item()).map(|todo| &todo.uid))
        .collect();

    // remote index -> local UID it adopts
    let mut adoptions: HashMap<usize, EventUid> = HashMap::new();
    let mut claims: HashMap<&EventUid, usize> = HashMap::new();

    for (index, remote) in remote_events.iter().enumerate() {
        let Some(todo) = plain_task(remote.item()) else {
            continue;
        };

        let candidates: HashSet<&EventUid> = provider_ids(todo)
            .filter_map(|key| local_uids_by_provider_id.get(&key))
            .flatten()
            .copied()
            .filter(|uid| **uid != todo.uid)
            .collect();

        let [local_uid] = candidates.into_iter().collect::<Vec<_>>()[..] else {
            continue;
        };

        if remote_uids.contains(local_uid) {
            continue;
        }

        *claims.entry(local_uid).or_default() += 1;
        adoptions.insert(index, local_uid.clone());
    }

    adoptions.retain(|_, uid| claims.get(&*uid) == Some(&1));

    let mut shadows = Vec::new();

    let remote_events = remote_events
        .into_iter()
        .enumerate()
        .map(|(index, remote)| {
            let Some(local_uid) = adoptions.remove(&index) else {
                return remote;
            };

            let CalendarItem::Todo(mut todo) = remote.item().clone() else {
                unreachable!("only plain tasks are adopted");
            };

            let derived_id = remote.item().instance_id();
            if let Some(shadow) = local_by_id.get(&derived_id) {
                shadows.push((*shadow).clone());
            }

            todo.uid = local_uid;
            RemoteEvent::new(todo)
        })
        .collect();

    AdoptedTasks {
        remote_events,
        shadows,
    }
}

/// A task without a recurrence id: the only kind a UID-less task store holds.
fn plain_task(item: &CalendarItem) -> Option<&Todo> {
    match item {
        CalendarItem::Todo(todo) if todo.recurrence_id.is_none() => Some(todo),
        _ => None,
    }
}

fn provider_ids(todo: &Todo) -> impl Iterator<Item = (&'static str, &str)> {
    todo.x_properties
        .iter()
        .filter(|p| !p.value.is_empty())
        .filter_map(|p| {
            PROVIDER_TASK_ID_PROPERTIES
                .iter()
                .find(|name| p.name.eq_ignore_ascii_case(name))
                .map(|name| (*name, p.value.as_str()))
        })
}
