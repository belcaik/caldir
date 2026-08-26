//! Update an existing event on a CalDAV calendar.

use anyhow::Result;
use caldir_core::CalendarItem;
use caldir_core::provider::{ProviderStorage, classified_error};
use caldir_core::rpc::{ErrorCode, UpdateItem};
use caldir_provider_caldav::caldav::ops;
use caldir_provider_caldav::caldav::ops::update_event::StaleEtag;

use crate::constants::PROVIDER_NAME;
use crate::remote_config::CaldavRemoteConfig;
use crate::session::SessionStore;

pub async fn handle(cmd: UpdateItem) -> Result<CalendarItem> {
    let config = CaldavRemoteConfig::try_from(&cmd.remote)?;
    let store = SessionStore::new(ProviderStorage::for_provider(PROVIDER_NAME)?);
    let session = store.load(&config.caldav_account)?;
    let (username, password) = session.credentials();

    ops::update_event::update_item(username, password, &config.caldav_calendar_url, cmd.event)
        .await
        .map_err(attach_conflict_class)
}

/// A stale-ETag precondition failure is the `ErrorCode::Conflict` class:
/// attach it here, at the RPC boundary, rather than leaving it as a generic
/// provider error. Any other failure passes through unchanged.
fn attach_conflict_class(err: anyhow::Error) -> anyhow::Error {
    match err.downcast::<StaleEtag>() {
        Ok(stale) => {
            anyhow::Error::from_boxed(classified_error(ErrorCode::Conflict, stale.to_string()))
        }
        Err(err) => err,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **W2-C3.** A stale-ETag failure must arrive at the RPC boundary
    /// carrying `ErrorCode::Conflict`, not the generic provider class — this
    /// is the first path where that class earns its place.
    #[test]
    fn a_stale_etag_error_is_reclassified_as_conflict() {
        let stale = StaleEtag::new("/cal/task-1@caldir.ics");
        let message = stale.to_string();

        let err = attach_conflict_class(anyhow::Error::new(stale));

        assert_eq!(
            err.to_string(),
            message,
            "the message survives reclassification"
        );
        assert!(
            err.downcast_ref::<StaleEtag>().is_none(),
            "a reclassified error is no longer a bare StaleEtag: \
             it must now carry ErrorCode::Conflict instead"
        );
    }

    /// Any other failure must pass through unchanged — this boundary must
    /// not swallow or reclassify errors it doesn't recognise.
    #[test]
    fn an_unrelated_error_passes_through_unchanged() {
        let original = anyhow::anyhow!("network unreachable");
        let message = original.to_string();

        let err = attach_conflict_class(original);

        assert_eq!(err.to_string(), message);
    }
}
