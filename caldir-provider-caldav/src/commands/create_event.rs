//! Create a new event on a CalDAV calendar.

use anyhow::Result;
use caldir_core::CalendarItem;
use caldir_core::provider::ProviderStorage;
use caldir_core::rpc::CreateItem;
use caldir_provider_caldav::caldav::ops;

use crate::constants::PROVIDER_NAME;
use crate::remote_config::CaldavRemoteConfig;
use crate::session::SessionStore;

pub async fn handle(cmd: CreateItem) -> Result<CalendarItem> {
    let event = match cmd.event {
        CalendarItem::Event(event) => event,
        _ => {
            return Err(anyhow::anyhow!(
                "caldir-provider-caldav does not support creating tasks"
            ));
        }
    };

    let config = CaldavRemoteConfig::try_from(&cmd.remote)?;
    let store = SessionStore::new(ProviderStorage::for_provider(PROVIDER_NAME)?);
    let session = store.load(&config.caldav_account)?;
    let (username, password) = session.credentials();

    let event = ops::create_event(username, password, &config.caldav_calendar_url, event).await?;
    Ok(CalendarItem::Event(event))
}
