//! Google Calendar provider for caldir.

mod app_config;
mod commands;
mod constants;
mod google_event;
mod google_tasks;
mod remote_config;
mod session;

use async_trait::async_trait;
use caldir_core::provider::ProviderCapabilities;
use caldir_core::rpc::{
    Capabilities, Connect, ConnectResponse, CreateItem, DeleteItem, ListCalendars, ListEvents,
    UpdateItem,
};
use caldir_core::{CalendarConfig, CalendarItem, provider};

struct GoogleProvider;

#[async_trait]
impl provider::Handler for GoogleProvider {
    async fn connect(&self, cmd: Connect) -> provider::Result<ConnectResponse> {
        Ok(commands::connect::handle(cmd).await?)
    }

    async fn capabilities(&self, cmd: Capabilities) -> provider::Result<ProviderCapabilities> {
        Ok(commands::capabilities::handle(cmd))
    }

    async fn list_calendars(&self, cmd: ListCalendars) -> provider::Result<Vec<CalendarConfig>> {
        Ok(commands::list_calendars::handle(cmd).await?)
    }

    async fn list_events(&self, cmd: ListEvents) -> provider::Result<Vec<CalendarItem>> {
        Ok(commands::list_events::handle(cmd).await?)
    }

    async fn create_item(&self, cmd: CreateItem) -> provider::Result<CalendarItem> {
        Ok(commands::create_event::handle(cmd).await?)
    }

    async fn update_item(&self, cmd: UpdateItem) -> provider::Result<CalendarItem> {
        Ok(commands::update_event::handle(cmd).await?)
    }

    async fn delete_item(&self, cmd: DeleteItem) -> provider::Result<()> {
        Ok(commands::delete_event::handle(cmd).await?)
    }
}

#[tokio::main]
async fn main() {
    provider::run_provider(GoogleProvider).await
}
