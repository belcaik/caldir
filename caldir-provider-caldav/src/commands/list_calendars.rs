//! List CalDAV calendars for a given account.

use anyhow::Result;
use caldir_core::provider::ProviderStorage;
use caldir_core::rpc::ListCalendars;
use caldir_core::{CalendarConfig, ProviderSlug, RemoteConfig};
use caldir_provider_caldav::caldav::ops::{self, RawCalendar};

use crate::constants::PROVIDER_NAME;
use crate::remote_config::CaldavRemoteConfig;
use crate::session::{Session, SessionStore};

pub async fn handle(cmd: ListCalendars) -> Result<Vec<CalendarConfig>> {
    let store = SessionStore::new(ProviderStorage::for_provider(PROVIDER_NAME)?);
    let session = store.load(&cmd.account_identifier)?;
    let (username, password) = session.credentials();

    let raw_calendars =
        ops::list_calendars_raw(username, password, &session.calendar_home_url).await?;

    let account_id = Session::account_identifier(&session.username, &session.server_url);

    Ok(raw_calendars
        .into_iter()
        .map(|cal| raw_to_config(&account_id, cal))
        .collect())
}

/// Build a caldir CalendarConfig from a raw CalDAV calendar entry.
///
/// Pure transformation — no IO — so it can be unit-tested without a server.
fn raw_to_config(account_id: &str, cal: RawCalendar) -> CalendarConfig {
    let mut params = CaldavRemoteConfig::new(account_id, &cal.url).into_remote_config_params();
    // Provider-private key (C-6): the advertised component set is CalDAV
    // metadata, not a core concept, so it rides the free-form remote params
    // rather than a typed `CalendarConfig` field. Omitted entirely when
    // unreadable, so a missing key means "unknown", never "false" (R91).
    if let Some(supports_tasks) = cal.supports_tasks {
        params.insert(
            "caldav_supports_tasks".to_string(),
            toml::Value::Boolean(supports_tasks),
        );
    }
    let remote_config = RemoteConfig::new(ProviderSlug::from(PROVIDER_NAME), params);

    CalendarConfig::new(
        Some(cal.name),
        cal.color,
        cal.read_only,
        Some(remote_config),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(name: &str, url: &str, color: Option<&str>, read_only: Option<bool>) -> RawCalendar {
        raw_with_tasks(name, url, color, read_only, None)
    }

    fn raw_with_tasks(
        name: &str,
        url: &str,
        color: Option<&str>,
        read_only: Option<bool>,
        supports_tasks: Option<bool>,
    ) -> RawCalendar {
        RawCalendar {
            href: "/dav/cal/1/".to_string(),
            name: name.to_string(),
            color: color.map(str::to_string),
            url: url.to_string(),
            read_only,
            supports_tasks,
        }
    }

    #[test]
    fn carries_name_and_read_only() {
        let cfg = raw_to_config(
            "me@fastmail.com",
            raw(
                "Personal",
                "https://server/cal/1/",
                Some("#0099ff"),
                Some(false),
            ),
        );

        assert_eq!(cfg.name(), Some("Personal"));
        assert_eq!(cfg.read_only(), Some(false));
    }

    #[test]
    fn remote_config_carries_account_url_and_provider_slug() {
        let cfg = raw_to_config(
            "me@fastmail.com",
            raw("Personal", "https://server/cal/1/", None, None),
        );

        let remote = cfg.remote_config().unwrap();
        assert_eq!(remote.provider_slug().to_string(), PROVIDER_NAME);
        assert_eq!(
            remote.get("caldav_account").and_then(|v| v.as_str()),
            Some("me@fastmail.com")
        );
        assert_eq!(
            remote.get("caldav_calendar_url").and_then(|v| v.as_str()),
            Some("https://server/cal/1/")
        );
    }

    #[test]
    fn read_only_unknown_passes_through_as_none() {
        let cfg = raw_to_config(
            "me@fastmail.com",
            raw("Personal", "https://server/cal/1/", None, None),
        );

        assert_eq!(cfg.read_only(), None);
    }

    /// C-6. The advertised component set is CalDAV metadata, not a core
    /// concept: it must ride the free-form `RemoteConfigParams` under a
    /// provider-private key, never grow a typed `CalendarConfig` field. A
    /// typed field here is exactly what C-6 forbids.
    #[test]
    fn task_capability_is_recorded_in_the_provider_private_params() {
        let cfg = raw_to_config(
            "me@fastmail.com",
            raw_with_tasks("Personal", "https://server/cal/1/", None, None, Some(true)),
        );
        let remote = cfg.remote_config().unwrap();
        assert_eq!(
            remote
                .get("caldav_supports_tasks")
                .and_then(|v| v.as_bool()),
            Some(true)
        );

        let cfg_false = raw_to_config(
            "me@fastmail.com",
            raw_with_tasks("Personal", "https://server/cal/1/", None, None, Some(false)),
        );
        assert_eq!(
            cfg_false
                .remote_config()
                .unwrap()
                .get("caldav_supports_tasks")
                .and_then(|v| v.as_bool()),
            Some(false)
        );

        // Unreadable stays unreadable: the key is omitted rather than
        // written as `false`, so a later reader can't mistake "unknown" for
        // "no tasks" (R91).
        let cfg_unknown = raw_to_config(
            "me@fastmail.com",
            raw("Personal", "https://server/cal/1/", None, None),
        );
        assert!(
            cfg_unknown
                .remote_config()
                .unwrap()
                .get("caldav_supports_tasks")
                .is_none()
        );
    }
}
