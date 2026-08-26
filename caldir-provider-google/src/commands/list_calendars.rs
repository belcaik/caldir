//! List Google Calendars (name + config) for a given account.

use anyhow::{Context, Result};
use caldir_core::provider::ProviderStorage;
use caldir_core::rpc::ListCalendars;
use caldir_core::{CalendarConfig, ProviderSlug, RemoteConfig};
use google_calendar::types::MinAccessRole;

use crate::app_config::AppConfigStore;
use crate::constants::PROVIDER_NAME;
use crate::google_tasks;
use crate::remote_config::{GoogleRemoteConfig, GoogleTaskListRemoteConfig};
use crate::session::{Session, SessionStore};

pub async fn handle(cmd: ListCalendars) -> Result<Vec<CalendarConfig>> {
    let account_email = &cmd.account_identifier;

    let storage = ProviderStorage::for_provider(PROVIDER_NAME)?;
    let session_store = SessionStore::new(storage.clone());
    let app_config_store = AppConfigStore::new(storage);

    let session = session_store
        .load_valid(account_email, &app_config_store)
        .await?;
    let client = session_store.client(&session, &app_config_store)?;

    let google_calendars = client
        .calendar_list()
        .list_all(MinAccessRole::default(), false, false)
        .await
        .context("Failed to fetch calendars")?
        .body;

    let mut calendar_configs: Vec<CalendarConfig> = google_calendars
        .iter()
        .map(|cal| {
            let params =
                GoogleRemoteConfig::new(account_email, &cal.id).into_remote_config_params();
            let remote_config = RemoteConfig::new(ProviderSlug::from(PROVIDER_NAME), params);
            let read_only = !matches!(cal.access_role.as_str(), "writer" | "owner");

            CalendarConfig::new(
                Some(cal.summary.clone()),
                Some(cal.background_color.clone()),
                Some(read_only),
                Some(remote_config),
            )
        })
        .collect();

    calendar_configs.extend(task_list_calendar_configs(account_email, &session).await?);

    Ok(calendar_configs)
}

/// Enumerates Google Tasks lists alongside Calendar collections (spec §11,
/// `GOOGLE-1`). The scope guard runs *before* the Tasks request — a session
/// predating the Tasks scope degrades to calendars-only rather than
/// surfacing a bare Google 403 (§10, `docs/vtodo/provider-sync/providers/google-tasks.md`
/// "Session migration").
async fn task_list_calendar_configs(
    account_email: &str,
    session: &Session,
) -> Result<Vec<CalendarConfig>> {
    if session.require_tasks_scope().is_err() {
        return Ok(Vec::new());
    }

    let task_lists = google_tasks::list_task_lists(session.access_token())
        .await
        .context("Failed to fetch Google Task lists")?;

    Ok(task_lists
        .iter()
        .map(|list| {
            let params = GoogleTaskListRemoteConfig::new(account_email, &list.id)
                .into_remote_config_params();
            let remote_config = RemoteConfig::new(ProviderSlug::from(PROVIDER_NAME), params);

            // Whether tasks are pushed is decided at execution time by
            // capability negotiation (P4-05), not frozen here (D-2, S3).
            CalendarConfig::new(
                Some(list.title.clone()),
                None,
                Some(false),
                Some(remote_config),
            )
        })
        .collect())
}
