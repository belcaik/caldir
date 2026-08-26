//! Capability declaration for the generic CalDAV provider.

use caldir_core::provider::{Capability, ProviderCapabilities};
use caldir_core::rpc::Capabilities;

/// CalDAV events are always read/write.
///
/// Tasks are per-remote (D-2), not per-binary: `list_calendars` records
/// `caldav_supports_tasks` for this remote from the collection's advertised
/// `supported-calendar-component-set` (C-6, a provider-private
/// `RemoteConfigParams` key). Fail closed on anything but an explicit `true`
/// (C-8) — absent (unknown), unreadable, or `false` all report
/// `tasks.read = false`. This is what keeps a rejected VTODO REPORT's
/// degrade-to-events-only listing (`caldav/ops/list_events.rs`) from being
/// read by core as complete task state and deleting every local VTODO
/// holding a sync base (C-7a).
///
/// `tasks.write` is held `false` unconditionally: `P3-02`..`P3-04` haven't
/// landed the VTODO write path yet, so there is no honest `true` case. This
/// file MUST NOT flip it to a per-remote answer ahead of those tickets.
pub fn handle(cmd: Capabilities) -> ProviderCapabilities {
    let tasks_read = cmd
        .remote
        .get("caldav_supports_tasks")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    ProviderCapabilities {
        events: Capability {
            read: true,
            write: true,
        },
        tasks: Capability {
            read: tasks_read,
            write: false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caldir_core::RemoteConfigParams;

    /// §4's matrix row for CalDAV, events half. Events are always read/write
    /// regardless of remote params.
    #[test]
    fn capabilities_declares_read_and_write_for_events() {
        let caps = handle(Capabilities {
            remote: RemoteConfigParams::new(),
        });

        assert!(caps.events.read);
        assert!(caps.events.write);
    }

    /// W3-C1's hazard, reproduced directly: a collection whose
    /// `supported-calendar-component-set` was observed (at `list_calendars`
    /// time) to omit `VTODO` recorded `caldav_supports_tasks = false` in this
    /// remote's params. A rejected VTODO REPORT later degrades `list_events`
    /// into an events-only `Ok` for exactly this remote. If `capabilities`
    /// still answered `tasks.read = true` here, core would read that
    /// degraded listing as complete task state and delete every local VTODO
    /// holding a sync base (C-7a). `tasks.read` MUST be `false` for this
    /// remote.
    #[test]
    fn a_remote_whose_component_set_omits_vtodo_reports_tasks_read_false() {
        let mut params = RemoteConfigParams::new();
        params.insert(
            "caldav_supports_tasks".to_string(),
            toml::Value::Boolean(false),
        );

        let caps = handle(Capabilities { remote: params });

        assert!(
            !caps.tasks.read,
            "a remote known to lack VTODO support must not advertise tasks.read = true"
        );
    }

    /// C-8, fail closed. `list_calendars` omits `caldav_supports_tasks`
    /// entirely when it couldn't determine the component set (R91) — it
    /// never writes `false` for "unknown". A missing key here MUST still
    /// read as `tasks.read = false`, never `true`.
    #[test]
    fn an_absent_task_capability_key_reports_tasks_read_false() {
        let caps = handle(Capabilities {
            remote: RemoteConfigParams::new(),
        });

        assert!(
            !caps.tasks.read,
            "absent task-capability information must fail closed to tasks.read = false"
        );
    }

    /// C-8, fail closed. A value that isn't a bool (corrupt config, a stray
    /// string) is unreadable, not "false" and not "true" — it must still
    /// collapse to `tasks.read = false`.
    #[test]
    fn an_unreadable_task_capability_value_reports_tasks_read_false() {
        let mut params = RemoteConfigParams::new();
        params.insert(
            "caldav_supports_tasks".to_string(),
            toml::Value::String("maybe".to_string()),
        );

        let caps = handle(Capabilities { remote: params });

        assert!(
            !caps.tasks.read,
            "an unreadable task-capability value must fail closed to tasks.read = false"
        );
    }

    /// The honest positive case: a remote known to support VTODO reports
    /// `tasks.read = true`.
    #[test]
    fn a_remote_known_to_support_vtodo_reports_tasks_read_true() {
        let mut params = RemoteConfigParams::new();
        params.insert(
            "caldav_supports_tasks".to_string(),
            toml::Value::Boolean(true),
        );

        let caps = handle(Capabilities { remote: params });

        assert!(caps.tasks.read);
    }

    /// Sequencing note (W3-C1): `P3-02`..`P3-04` (the VTODO write path)
    /// haven't landed, so `tasks.write` is held at `false` regardless of
    /// remote params — a task write reaching this provider today would hit
    /// `P1-04`'s unsupported-arm gate and error instead of syncing. Declared
    /// unconditionally, not per-remote: there's no honest `true` case yet.
    #[test]
    fn tasks_write_is_held_false_until_the_write_path_lands() {
        let mut params = RemoteConfigParams::new();
        params.insert(
            "caldav_supports_tasks".to_string(),
            toml::Value::Boolean(true),
        );

        let caps = handle(Capabilities { remote: params });

        assert!(!caps.tasks.write);
    }
}
