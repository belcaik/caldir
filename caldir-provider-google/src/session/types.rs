//! Pure session data — no IO, no env vars.

use chrono::{DateTime, Duration, Utc};
use google_calendar::AccessToken;
use serde::{Deserialize, Serialize};

use crate::constants::TASKS_SCOPE;

/// Whether this session was created via hosted (caldir.org) or local (self-hosted) OAuth.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum AuthMode {
    #[default]
    Local,
    Hosted,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SessionData {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: DateTime<Utc>,
    #[serde(default)]
    pub auth_mode: AuthMode,
    /// OAuth scopes actually granted, per the last token response. Absent
    /// on session files written before Tasks support (`#[serde(default)]`
    /// keeps them loadable) — empty means "unknown, therefore not granted"
    /// (D-2 fail closed), never "assume yes".
    #[serde(default)]
    pub granted_scopes: Vec<String>,
}

fn split_scopes(scope: &str) -> Vec<String> {
    scope.split_whitespace().map(str::to_string).collect()
}

impl From<&AccessToken> for SessionData {
    fn from(tokens: &AccessToken) -> Self {
        let expires_at = Utc::now() + Duration::seconds(tokens.expires_in);

        SessionData {
            access_token: tokens.access_token.clone(),
            refresh_token: tokens.refresh_token.clone(),
            expires_at,
            auth_mode: AuthMode::Local,
            granted_scopes: split_scopes(&tokens.scope),
        }
    }
}

impl SessionData {
    pub fn from_tokens(
        access_token: String,
        refresh_token: String,
        expires_in: i64,
        scope: Option<&str>,
    ) -> Self {
        let expires_at = Utc::now() + Duration::seconds(expires_in);

        SessionData {
            access_token,
            refresh_token,
            expires_at,
            auth_mode: AuthMode::Hosted,
            granted_scopes: scope.map(split_scopes).unwrap_or_default(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Session {
    pub account_email: String,
    pub data: SessionData,
}

impl Session {
    pub fn new(account_email: &str, session_data: &SessionData, auth_mode: AuthMode) -> Self {
        let mut data = session_data.clone();
        data.auth_mode = auth_mode;

        Session {
            account_email: account_email.to_string(),
            data,
        }
    }

    pub fn access_token(&self) -> &str {
        &self.data.access_token
    }

    pub fn auth_mode(&self) -> &AuthMode {
        &self.data.auth_mode
    }

    pub fn is_expired(&self) -> bool {
        Utc::now() >= self.data.expires_at
    }

    /// Filesystem slug. Forward-deterministic from the account email so
    /// `SessionStore::load` can compute the path directly. Preserved
    /// byte-for-byte from the pre-migration implementation — users may
    /// have existing files on disk under these names.
    pub(super) fn slug(account_email: &str) -> String {
        account_email.replace(['/', '\\', ':'], "_")
    }

    /// Fails closed (D-2): a session recorded with no Tasks scope — the
    /// state of every pre-Tasks session file — is treated as "not granted",
    /// never "unknown, assume yes". Pure and IO-free so call sites can check
    /// it before making any Tasks API request, turning what would otherwise
    /// surface as a bare Google 403 into an actionable instruction (§16
    /// "authentication scope missing").
    ///
    /// Called before every Tasks request: `list_calendars`, the read path,
    /// and the degrade-warning path all check this first.
    pub fn require_tasks_scope(&self) -> anyhow::Result<()> {
        if self
            .data
            .granted_scopes
            .iter()
            .any(|scope| scope == TASKS_SCOPE)
        {
            return Ok(());
        }

        anyhow::bail!(
            "Google account {} is not authorized for Google Tasks. Run `caldir connect google` to reauthorize and grant access.",
            self.account_email
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn access_token(scope: &str) -> AccessToken {
        serde_json::from_value(serde_json::json!({ "scope": scope })).unwrap()
    }

    #[test]
    fn a_session_file_without_granted_scopes_loads_as_no_scopes() {
        let toml = r#"
            access_token = "access-abc"
            refresh_token = "refresh-xyz"
            expires_at = "2099-01-01T00:00:00Z"
        "#;

        let data: SessionData = toml::from_str(toml).unwrap();

        assert!(data.granted_scopes.is_empty());
    }

    #[test]
    fn granted_scopes_are_read_from_the_token_response() {
        let token = access_token(&format!("{TASKS_SCOPE} https://example.com/other"));

        let data: SessionData = (&token).into();

        assert_eq!(
            data.granted_scopes,
            vec![
                TASKS_SCOPE.to_string(),
                "https://example.com/other".to_string()
            ]
        );
    }

    #[test]
    fn a_missing_tasks_scope_names_the_account_and_the_fix() {
        let session = Session {
            account_email: "alice@gmail.com".to_string(),
            data: SessionData {
                access_token: "a".to_string(),
                refresh_token: "r".to_string(),
                expires_at: Utc::now(),
                auth_mode: AuthMode::Local,
                granted_scopes: vec![],
            },
        };

        let err = session.require_tasks_scope().unwrap_err().to_string();

        assert!(err.contains("alice@gmail.com"));
        assert!(err.contains("caldir connect google"));
        assert!(!err.contains("403"));
    }

    #[test]
    fn a_session_holding_the_tasks_scope_passes_the_guard() {
        let session = Session {
            account_email: "alice@gmail.com".to_string(),
            data: SessionData {
                access_token: "a".to_string(),
                refresh_token: "r".to_string(),
                expires_at: Utc::now(),
                auth_mode: AuthMode::Local,
                granted_scopes: vec![TASKS_SCOPE.to_string()],
            },
        };

        assert!(session.require_tasks_scope().is_ok());
    }
}
