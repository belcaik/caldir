//! Capability declaration for the generic CalDAV provider.

use caldir_core::provider::{Capability, ProviderCapabilities};
use caldir_core::rpc::Capabilities;

/// CalDAV supports full read/write of both events and tasks (§4's matrix row:
/// `CalDAV yes yes`).
///
/// Honest only as a unit with Wave 3: `P3-02`..`P3-04` implement the task
/// writes this declaration promises. Per §4's merge-order rule this file
/// MUST NOT reach a release ahead of them — see the ticket commit for the
/// record of that ordering.
pub fn handle(_cmd: Capabilities) -> ProviderCapabilities {
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

#[cfg(test)]
mod tests {
    use super::*;
    use caldir_core::RemoteConfigParams;

    /// §4's matrix row for CalDAV. A provider that implements the writes but
    /// forgets to say so is invisible to `S3`: core withholds every task
    /// push and the user sees the "does not support task writes" message
    /// from a provider that actually does.
    #[test]
    fn capabilities_declares_read_and_write_for_both_events_and_tasks() {
        let caps = handle(Capabilities {
            remote: RemoteConfigParams::new(),
        });

        assert!(caps.events.read);
        assert!(caps.events.write);
        assert!(caps.tasks.read);
        assert!(caps.tasks.write);
    }
}
