//! Update an existing event on iCloud Calendar.

use anyhow::Result;
use caldir_core::CalendarItem;
use caldir_core::provider::ProviderStorage;
use caldir_core::rpc::UpdateItem;
use caldir_provider_caldav::caldav::ops;

use crate::constants::PROVIDER_NAME;
use crate::remote_config::ICloudRemoteConfig;
use crate::session::SessionStore;

pub async fn handle(cmd: UpdateItem) -> Result<CalendarItem> {
    let event = match cmd.event {
        CalendarItem::Event(event) => event,
        _ => {
            return Err(anyhow::anyhow!(
                "caldir-provider-icloud does not support updating tasks"
            ));
        }
    };

    let config = ICloudRemoteConfig::try_from(&cmd.remote)?;
    let store = SessionStore::new(ProviderStorage::for_provider(PROVIDER_NAME)?);
    let session = store.load(&config.icloud_account)?;
    let (username, password) = session.credentials();

    let event = ops::update_event(username, password, &config.icloud_calendar_url, event).await?;
    Ok(CalendarItem::Event(event))
}
