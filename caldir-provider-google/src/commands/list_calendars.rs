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

/// Enumerates Google Tasks lists alongside Calendar collections (spec §11).
/// The scope guard runs *before* the Tasks request — a session
/// predating the Tasks scope degrades to calendars-only rather than
/// surfacing a bare Google 403 (§10, `docs/vtodo/provider-sync/providers/google-tasks.md`
/// "Session migration").
async fn task_list_calendar_configs(
    account_email: &str,
    session: &Session,
) -> Result<Vec<CalendarConfig>> {
    if let Some(warning) = tasks_scope_warning(session) {
        eprintln!("{warning}");
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
            // capability negotiation, not frozen here.
            CalendarConfig::new(
                Some(list.title.clone()),
                None,
                Some(false),
                Some(remote_config),
            )
        })
        .collect())
}

/// The reauthorization warning for a session predating the Tasks scope
/// (`require_tasks_scope`'s error, prefixed for stderr), or `None` when the
/// scope is already granted. `None` on the happy path keeps discovery quiet
/// — a warning that always fires is noise.
fn tasks_scope_warning(session: &Session) -> Option<String> {
    session
        .require_tasks_scope()
        .err()
        .map(|err| format!("warning: {err:#}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{AuthMode, SessionData};
    use chrono::Utc;

    fn session_with_scopes(scopes: Vec<String>) -> Session {
        Session::new(
            "alice@gmail.com",
            &SessionData {
                access_token: "a".to_string(),
                refresh_token: "r".to_string(),
                expires_at: Utc::now(),
                auth_mode: AuthMode::Local,
                granted_scopes: scopes,
            },
            AuthMode::Local,
        )
    }

    #[test]
    fn a_missing_tasks_scope_produces_a_warning_naming_the_account_and_the_command() {
        let session = session_with_scopes(vec![]);

        let warning = tasks_scope_warning(&session).expect("expected a warning");

        assert!(warning.starts_with("warning:"));
        assert!(warning.contains("alice@gmail.com"));
        assert!(warning.contains("caldir connect google"));
    }

    #[test]
    fn a_session_holding_the_tasks_scope_produces_no_warning() {
        let session =
            session_with_scopes(vec!["https://www.googleapis.com/auth/tasks".to_string()]);

        assert!(tasks_scope_warning(&session).is_none());
    }
}
