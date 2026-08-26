//! Capability declaration for the generic CalDAV provider.

use caldir_core::provider::{Capability, ProviderCapabilities};
use caldir_core::rpc::Capabilities;

/// CalDAV events are always read/write.
///
/// Tasks are per-remote (D-2), not per-binary: `list_calendars` records
/// `caldav_supports_tasks` for this remote from the collection's advertised
/// `supported-calendar-component-set` (a provider-private `RemoteConfigParams`
/// key). Fail closed on anything but an explicit `true` — absent (unknown),
/// unreadable, or `false` all report `tasks.read = false` and
/// `tasks.write = false`. This is what keeps a rejected VTODO REPORT's
/// degrade-to-events-only listing (`caldav/ops/list_events.rs`) from being
/// read by core as complete task state and deleting every local VTODO
/// holding a sync base.
pub fn handle(cmd: Capabilities) -> ProviderCapabilities {
    let tasks_supported = cmd
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
            read: tasks_supported,
            write: tasks_supported,
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

    /// The VTODO write path has landed: a remote known to support tasks
    /// reports `tasks.write = true`, driven by the same
    /// `caldav_supports_tasks` key as `tasks.read`.
    #[test]
    fn a_remote_known_to_support_vtodo_reports_tasks_write_true() {
        let mut params = RemoteConfigParams::new();
        params.insert(
            "caldav_supports_tasks".to_string(),
            toml::Value::Boolean(true),
        );

        let caps = handle(Capabilities { remote: params });

        assert!(caps.tasks.write);
    }

    /// Fail closed: every input that isn't an explicit `true` must still
    /// report `tasks.write = false` — absent, unreadable, and explicit
    /// `false` alike.
    #[test]
    fn tasks_write_fails_closed_on_every_non_true_input() {
        let absent = handle(Capabilities {
            remote: RemoteConfigParams::new(),
        });
        assert!(!absent.tasks.write, "absent key must fail closed");

        let mut explicit_false = RemoteConfigParams::new();
        explicit_false.insert(
            "caldav_supports_tasks".to_string(),
            toml::Value::Boolean(false),
        );
        let caps = handle(Capabilities {
            remote: explicit_false,
        });
        assert!(!caps.tasks.write, "explicit false must fail closed");

        let mut unreadable = RemoteConfigParams::new();
        unreadable.insert(
            "caldav_supports_tasks".to_string(),
            toml::Value::String("maybe".to_string()),
        );
        let caps = handle(Capabilities { remote: unreadable });
        assert!(!caps.tasks.write, "unreadable value must fail closed");
    }
}
