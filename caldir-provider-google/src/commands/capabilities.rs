//! Capability declaration for the Google provider.
//!
//! One binary serves both Google Calendar and Google Tasks remotes, so the
//! answer is resolved per-remote via `resolve_resource_kind` — never
//! inferred from an id's shape (spec §11). Pure: no session load, no
//! network. Capabilities describe what this remote's kind supports, refined
//! at execution time on every call, not the account's authorization state.

use caldir_core::provider::{Capability, ProviderCapabilities};
use caldir_core::rpc::Capabilities;

use crate::remote_config::GoogleResourceKind;

/// A task-list remote reads and writes tasks; a Calendar remote supports no
/// tasks at all. Fail closed: an unresolvable remote kind reports no task
/// support, falling back to today's events-only behaviour rather than
/// erroring the capability query itself.
pub fn handle(cmd: Capabilities) -> ProviderCapabilities {
    match crate::remote_config::resolve_resource_kind(&cmd.remote) {
        Ok(GoogleResourceKind::TaskList) => ProviderCapabilities {
            events: Capability {
                read: false,
                write: false,
            },
            tasks: Capability {
                read: true,
                write: true,
            },
        },
        Ok(GoogleResourceKind::Calendar) | Err(_) => ProviderCapabilities::events_only(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caldir_core::RemoteConfigParams;

    fn task_list_params() -> RemoteConfigParams {
        let mut params = RemoteConfigParams::new();
        params.insert(
            "google_account".to_string(),
            toml::Value::String("alice@gmail.com".to_string()),
        );
        params.insert(
            "google_resource_kind".to_string(),
            toml::Value::String("tasklist".to_string()),
        );
        params.insert(
            "google_resource_id".to_string(),
            toml::Value::String("task-list-id".to_string()),
        );
        params
    }

    fn calendar_params() -> RemoteConfigParams {
        let mut params = RemoteConfigParams::new();
        params.insert(
            "google_account".to_string(),
            toml::Value::String("alice@gmail.com".to_string()),
        );
        params.insert(
            "google_calendar_id".to_string(),
            toml::Value::String("alice@gmail.com".to_string()),
        );
        params
    }

    /// This is the single fact that turns off C-7's `suppressed_task_deletes`
    /// belt for Google — the assertion that authorizes a delete path to run
    /// at all.
    #[test]
    fn a_task_list_remote_advertises_task_reads() {
        let caps = handle(Capabilities {
            remote: task_list_params(),
        });

        assert!(caps.tasks.read);
    }

    /// P5-07 closes Wave 5: create, update and delete are all coherent
    /// (D-2), so a task-list remote now advertises `tasks.write = true` —
    /// the flag `withhold_task_pushes` reads to let a VTODO leave the
    /// outgoing set at all (`CROSS-3`).
    #[test]
    fn a_task_list_remote_advertises_task_writes() {
        let caps = handle(Capabilities {
            remote: task_list_params(),
        });

        assert!(caps.tasks.write);
    }

    /// The reachable wrong edit is flipping the field unconditionally,
    /// which would tell core it may push VTODOs at a Google **Calendar**
    /// remote — §9 and `CROSS-6` in one step, and it would look correct in
    /// every task-list test.
    #[test]
    fn a_calendar_remote_still_advertises_no_task_support() {
        let caps = handle(Capabilities {
            remote: calendar_params(),
        });

        assert!(!caps.tasks.read);
        assert!(!caps.tasks.write);
    }

    /// `GOOGLE-12`: the capability reply is the first thing every sync
    /// reads, so this flag flip must not perturb the events half for
    /// either resource kind.
    #[test]
    fn the_event_capabilities_are_unchanged_by_this_flag() {
        let task_list = handle(Capabilities {
            remote: task_list_params(),
        });
        assert!(!task_list.events.read);
        assert!(!task_list.events.write);

        let calendar = handle(Capabilities {
            remote: calendar_params(),
        });
        assert!(calendar.events.read);
        assert!(calendar.events.write);
    }

    /// A Google Calendar has no Tasks API behind it at all; advertising
    /// `tasks.read` there would send core looking for tasks down a path
    /// P4-04 deliberately routes away from. The events half is the
    /// `GOOGLE-12` control.
    #[test]
    fn a_calendar_remote_advertises_no_task_support() {
        let caps = handle(Capabilities {
            remote: calendar_params(),
        });

        assert!(caps.events.read);
        assert!(caps.events.write);
        assert!(!caps.tasks.read);
        assert!(!caps.tasks.write);
    }

    /// D-2's fail-closed clause names exactly this case: the natural
    /// implementation of `?`-ing the resolver and propagating would turn a
    /// capability query into a hard error, losing the ability to sync
    /// events at all. An unresolvable kind falls back to events-only.
    #[test]
    fn an_unresolvable_remote_falls_back_to_events_only() {
        let empty = handle(Capabilities {
            remote: RemoteConfigParams::new(),
        });
        assert!(empty.events.read);
        assert!(empty.events.write);
        assert!(!empty.tasks.read);
        assert!(!empty.tasks.write);

        let mut unknown_kind = RemoteConfigParams::new();
        unknown_kind.insert(
            "google_resource_kind".to_string(),
            toml::Value::String("vjournal".to_string()),
        );
        let caps = handle(Capabilities {
            remote: unknown_kind,
        });
        assert!(caps.events.read);
        assert!(caps.events.write);
        assert!(!caps.tasks.read);
        assert!(!caps.tasks.write);
    }

    /// Capabilities are fetched at execution time on every run (D-2); an
    /// implementation that loads a session or calls Google makes every sync
    /// slower and makes a disconnected or unauthorized account report *no*
    /// capabilities — a fail-closed answer for the wrong reason.
    #[test]
    fn the_declaration_needs_no_session_and_no_network() {
        let dir = tempfile::TempDir::new().unwrap();
        // SAFETY: test-only, single-threaded within this process's test
        // harness for this env var.
        unsafe {
            std::env::set_var("CALDIR_PROVIDER_STORAGE_DIR", dir.path());
        }

        let caps = handle(Capabilities {
            remote: task_list_params(),
        });

        assert!(caps.tasks.read);

        unsafe {
            std::env::remove_var("CALDIR_PROVIDER_STORAGE_DIR");
        }
    }
}
