use crate::RemoteConfigParams;
use crate::provider::ProviderCapabilities;
use crate::rpc::{Method, Rpc};
use serde::{Deserialize, Serialize};

/// Per-remote, not per-binary: the request flattens `RemoteConfigParams`
/// exactly like `ListEvents`, so one binary can answer differently for a
/// Google Calendar remote and a Google Tasks-list remote. A provider whose
/// answer never varies is free to ignore the params.
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct Capabilities {
    #[serde(flatten)]
    pub remote: RemoteConfigParams,
}

impl Rpc for Capabilities {
    const METHOD: Method = Method::Capabilities;
    type Response = ProviderCapabilities;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mirrors `list_events_serializes_json`. Right proof: the flattened
    /// remote config is how `P4-05` answers differently for a Google
    /// Calendar remote than for a Tasks-list remote — a capabilities call
    /// with no remote context cannot express that at all.
    #[test]
    fn capabilities_serializes_json() {
        let mut params = RemoteConfigParams::new();
        params.insert(
            "hooli_account".to_string(),
            toml::Value::String("user@hmail.com".to_string()),
        );

        let cmd = Capabilities { remote: params };

        let json = cmd.to_json().unwrap();

        assert_eq!(json["command"], "capabilities");
        assert_eq!(json["params"]["hooli_account"], "user@hmail.com");
    }
}
