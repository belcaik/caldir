//! Webcal (ICS subscription) provider for caldir.
//!
//! webcal subscriptions are readonly, so they only implement "connect" and "list_events"

mod commands;
mod constants;
mod http;
mod remote_config;

use async_trait::async_trait;
use caldir_core::rpc::{Connect, ConnectResponse, ListEvents};
use caldir_core::{CalendarItem, provider};

struct WebcalProvider;

#[async_trait]
impl provider::Handler for WebcalProvider {
    async fn connect(&self, cmd: Connect) -> provider::Result<ConnectResponse> {
        Ok(commands::connect::handle(cmd).await?)
    }

    async fn list_events(&self, cmd: ListEvents) -> provider::Result<Vec<CalendarItem>> {
        let events = commands::list_events::handle(cmd).await?;

        Ok(events.into_iter().map(CalendarItem::from).collect())
    }
}

#[tokio::main]
async fn main() {
    provider::run_provider(WebcalProvider).await
}
