//! Google-specific remote configuration.
//!
//! This provides type safety for Google Calendar remote config while
//! caldir-core remains provider-agnostic with its generic RemoteConfigParams.

use anyhow::Result;
use caldir_core::RemoteConfigParams;
use serde::{Deserialize, Serialize};

/// Strongly-typed remote configuration for Google Calendar.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoogleRemoteConfig {
    pub google_account: String,
    pub google_calendar_id: String,
}

impl GoogleRemoteConfig {
    pub fn new(account: impl Into<String>, calendar_id: impl Into<String>) -> Self {
        Self {
            google_account: account.into(),
            google_calendar_id: calendar_id.into(),
        }
    }

    pub fn into_remote_config_params(self) -> RemoteConfigParams {
        let mut params = RemoteConfigParams::new();
        params.insert(
            "google_account".to_string(),
            toml::Value::String(self.google_account),
        );
        params.insert(
            "google_calendar_id".to_string(),
            toml::Value::String(self.google_calendar_id),
        );
        params
    }
}

impl TryFrom<&RemoteConfigParams> for GoogleRemoteConfig {
    type Error = anyhow::Error;

    fn try_from(params: &RemoteConfigParams) -> Result<Self> {
        let google_account = params
            .get("google_account")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing required field: google_account"))?
            .to_string();

        let google_calendar_id = params
            .get("google_calendar_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing required field: google_calendar_id"))?
            .to_string();

        Ok(Self {
            google_account,
            google_calendar_id,
        })
    }
}

/// Which kind of Google container a remote points at. Recorded explicitly at
/// discovery time (P4-03) — never inferred from an id's shape (spec §11).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoogleResourceKind {
    Calendar,
    TaskList,
}

impl GoogleResourceKind {
    fn as_str(self) -> &'static str {
        match self {
            GoogleResourceKind::Calendar => "calendar",
            GoogleResourceKind::TaskList => "tasklist",
        }
    }
}

/// Resolves the resource kind from `google_resource_kind`. Absence means
/// Calendar via the legacy `google_calendar_id`-only shape (C-6: every
/// Google remote written before this ticket has no kind key at all).
/// An unrecognized value is an error, never a silent fallback to Calendar
/// (D-2 fail closed).
pub fn resolve_resource_kind(params: &RemoteConfigParams) -> Result<GoogleResourceKind> {
    let Some(value) = params.get("google_resource_kind") else {
        return Ok(GoogleResourceKind::Calendar);
    };

    let kind = value
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("google_resource_kind must be a string"))?;

    match kind {
        "calendar" => Ok(GoogleResourceKind::Calendar),
        "tasklist" => Ok(GoogleResourceKind::TaskList),
        other => Err(anyhow::anyhow!(
            "Unknown google_resource_kind: '{other}' (expected 'calendar' or 'tasklist')"
        )),
    }
}

/// Strongly-typed remote configuration for a Google Tasks task list.
/// Provider-private keys inside the free-form `RemoteConfigParams` (C-6) —
/// no core schema change.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoogleTaskListRemoteConfig {
    pub google_account: String,
    pub google_resource_id: String,
}

impl GoogleTaskListRemoteConfig {
    pub fn new(account: impl Into<String>, resource_id: impl Into<String>) -> Self {
        Self {
            google_account: account.into(),
            google_resource_id: resource_id.into(),
        }
    }

    pub fn into_remote_config_params(self) -> RemoteConfigParams {
        let mut params = RemoteConfigParams::new();
        params.insert(
            "google_account".to_string(),
            toml::Value::String(self.google_account),
        );
        params.insert(
            "google_resource_kind".to_string(),
            toml::Value::String(GoogleResourceKind::TaskList.as_str().to_string()),
        );
        params.insert(
            "google_resource_id".to_string(),
            toml::Value::String(self.google_resource_id),
        );
        params
    }
}

impl TryFrom<&RemoteConfigParams> for GoogleTaskListRemoteConfig {
    type Error = anyhow::Error;

    fn try_from(params: &RemoteConfigParams) -> Result<Self> {
        if resolve_resource_kind(params)? != GoogleResourceKind::TaskList {
            anyhow::bail!("Remote config does not identify a Google Tasks task list");
        }

        let google_account = params
            .get("google_account")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing required field: google_account"))?
            .to_string();

        let google_resource_id = params
            .get("google_resource_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing required field: google_resource_id"))?
            .to_string();

        Ok(Self {
            google_account,
            google_resource_id,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_legacy_calendar_remote_without_a_kind_stays_a_calendar() {
        let mut params = RemoteConfigParams::new();
        params.insert(
            "google_account".to_string(),
            toml::Value::String("alice@gmail.com".to_string()),
        );
        params.insert(
            "google_calendar_id".to_string(),
            toml::Value::String("alice@gmail.com".to_string()),
        );

        let kind = resolve_resource_kind(&params).unwrap();
        assert_eq!(kind, GoogleResourceKind::Calendar);

        let config = GoogleRemoteConfig::try_from(&params).unwrap();
        assert_eq!(config.google_calendar_id, "alice@gmail.com");
    }

    #[test]
    fn the_resource_kind_is_never_inferred_from_the_id() {
        // A task-list-shaped opaque id with no kind key still resolves to Calendar.
        let mut params = RemoteConfigParams::new();
        params.insert(
            "google_account".to_string(),
            toml::Value::String("alice@gmail.com".to_string()),
        );
        params.insert(
            "google_calendar_id".to_string(),
            toml::Value::String("MDAxMjM0NTY3ODkwMTIzNDU2Nzg6MDow".to_string()),
        );
        assert_eq!(
            resolve_resource_kind(&params).unwrap(),
            GoogleResourceKind::Calendar
        );

        // An email-shaped id explicitly tagged tasklist still resolves to TaskList.
        let mut params = RemoteConfigParams::new();
        params.insert(
            "google_account".to_string(),
            toml::Value::String("alice@gmail.com".to_string()),
        );
        params.insert(
            "google_resource_kind".to_string(),
            toml::Value::String("tasklist".to_string()),
        );
        params.insert(
            "google_resource_id".to_string(),
            toml::Value::String("alice@gmail.com".to_string()),
        );
        assert_eq!(
            resolve_resource_kind(&params).unwrap(),
            GoogleResourceKind::TaskList
        );
    }

    #[test]
    fn an_unknown_resource_kind_is_an_error_not_a_calendar() {
        let mut params = RemoteConfigParams::new();
        params.insert(
            "google_resource_kind".to_string(),
            toml::Value::String("vjournal".to_string()),
        );

        let err = resolve_resource_kind(&params).unwrap_err().to_string();

        assert!(err.contains("google_resource_kind"));
        assert!(err.contains("vjournal"));
    }

    #[test]
    fn a_task_list_config_round_trips_through_remote_config_params() {
        let config = GoogleTaskListRemoteConfig::new("alice@gmail.com", "task-list-id-123");

        let params = config.into_remote_config_params();

        // Exact key set — BTreeMap serializes keys in sorted order, so this
        // fails the moment a key is added, renamed, or missing.
        let serialized = toml::to_string(&params).unwrap();
        assert_eq!(
            serialized,
            "google_account = \"alice@gmail.com\"\n\
             google_resource_id = \"task-list-id-123\"\n\
             google_resource_kind = \"tasklist\"\n"
        );

        let round_tripped = GoogleTaskListRemoteConfig::try_from(&params).unwrap();
        assert_eq!(round_tripped.google_account, "alice@gmail.com");
        assert_eq!(round_tripped.google_resource_id, "task-list-id-123");
    }
}
