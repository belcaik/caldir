//! The public shape of D-2: what a provider can do with events and tasks,
//! answered fresh on every call and never persisted (§4, §17).

use serde::{Deserialize, Serialize};

/// A provider's declared support for one kind of item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Capability {
    #[serde(default)]
    pub read: bool,
    #[serde(default)]
    pub write: bool,
}

/// `ProviderCapabilities { events, tasks }` (§4). Public and constructible by
/// downstream crates (Proton) — no `#[non_exhaustive]`, see
/// `tests/public_error_api.rs`. Every field defaults to `false` on decode, so
/// an absent key — a newer provider omitting one it assumes, or an older one
/// that has never heard of it — reads as "not supported", never as support
/// (D-2, this ticket's fail-closed assertion).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ProviderCapabilities {
    #[serde(default)]
    pub events: Capability,
    #[serde(default)]
    pub tasks: Capability,
}

impl ProviderCapabilities {
    /// The `Handler` default (§18): every unmodified provider reads and
    /// writes events, and has never heard of tasks.
    pub fn events_only() -> Self {
        Self {
            events: Capability {
                read: true,
                write: true,
            },
            tasks: Capability::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `P6-03`/D-2: the nested wire shape itself, plus the fail-closed arm,
    /// asserted together on raw JSON. Right proof: absence is only
    /// expressible at the wire layer — a Rust-typed test cannot omit a
    /// field, so a rebase that turned this into a flat `{"tasks_read":...}`
    /// shape would compile and pass every typed test while breaking every
    /// installed provider's parser.
    #[test]
    fn the_capabilities_wire_shape_is_frozen_and_absence_fails_closed() {
        let full = ProviderCapabilities {
            events: Capability {
                read: true,
                write: true,
            },
            tasks: Capability {
                read: true,
                write: false,
            },
        };

        assert_eq!(
            serde_json::to_value(full).unwrap(),
            serde_json::json!({
                "events": {"read": true, "write": true},
                "tasks": {"read": true, "write": false}
            })
        );

        let events_only: ProviderCapabilities =
            serde_json::from_str(r#"{"events":{"read":true,"write":true}}"#)
                .expect("an events-only payload from an older provider must still decode");
        assert_eq!(events_only.tasks, Capability::default());
        assert!(!events_only.tasks.read);
        assert!(!events_only.tasks.write);
    }

    /// D-2's "absent field" clause. A missing `tasks` key and a present-but-
    /// empty `tasks` object must both read `tasks.write` as `false` — absence
    /// read as `true` is the one bug in this ticket that deletes user data
    /// downstream via `P2-06`.
    #[test]
    fn an_absent_capability_field_reads_as_false() {
        let no_tasks_key: ProviderCapabilities =
            serde_json::from_str(r#"{"events":{"read":true,"write":true}}"#).unwrap();
        assert!(!no_tasks_key.tasks.write);
        assert!(!no_tasks_key.tasks.read);

        let empty_tasks: ProviderCapabilities = serde_json::from_str(r#"{"tasks":{}}"#).unwrap();
        assert!(!empty_tasks.tasks.write);
        assert!(!empty_tasks.tasks.read);
    }
}
