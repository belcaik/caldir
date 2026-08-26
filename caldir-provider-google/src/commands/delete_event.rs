use anyhow::{Context, Result};
use caldir_core::CalendarItem;
use caldir_core::provider::ProviderStorage;
use caldir_core::rpc::DeleteItem;
use google_calendar::types::SendUpdates;

use crate::app_config::AppConfigStore;
use crate::constants::{PROVIDER_EVENT_ID_PROPERTY, PROVIDER_NAME};
use crate::remote_config::GoogleRemoteConfig;
use crate::session::SessionStore;

pub async fn handle(cmd: DeleteItem) -> Result<()> {
    let event = match cmd.event {
        CalendarItem::Event(event) => event,
        _ => {
            return Err(anyhow::anyhow!(
                "caldir-provider-google does not support deleting tasks"
            ));
        }
    };

    let config = GoogleRemoteConfig::try_from(&cmd.remote)?;
    let account_email = &config.google_account;
    let calendar_id = &config.google_calendar_id;

    let google_event_id = event
        .x_property(PROVIDER_EVENT_ID_PROPERTY)
        .ok_or_else(|| {
            anyhow::anyhow!("Cannot delete event without {PROVIDER_EVENT_ID_PROPERTY}")
        })?;

    let storage = ProviderStorage::for_provider(PROVIDER_NAME)?;
    let session_store = SessionStore::new(storage.clone());
    let app_config_store = AppConfigStore::new(storage);

    let session = session_store
        .load_valid(account_email, &app_config_store)
        .await?;
    let client = session_store.client(&session, &app_config_store)?;

    client
        .events()
        .delete(calendar_id, google_event_id, false, SendUpdates::All)
        .await
        .context("Failed to delete event")?;

    Ok(())
}
