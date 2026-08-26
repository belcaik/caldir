use std::path::PathBuf;

use super::transport::ProviderTransportError;
use crate::rpc;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ProviderError {
    #[error("Provider file is not executable: {0}")]
    NotExecutable(PathBuf),

    #[error("Provider filename does not match `caldir-provider-<name>`: {0}")]
    InvalidProviderFilename(PathBuf),

    #[error("Provider {0} not found")]
    ProviderNotFound(String),

    #[error("Provider transport error: {0}")]
    Transport(#[from] ProviderTransportError),

    #[error("Failed to serialize provider request: {0}")]
    Serialize(serde_json::Error),

    #[error("Failed to deserialize provider response: {0}")]
    Deserialize(serde_json::Error),

    #[error("Provider returned error: {0}")]
    Provider(String),

    /// The provider replied that it does not recognise the RPC method sent
    /// to it (`rpc::ErrorCode::UnsupportedMethod`) — distinct from
    /// `Provider` so a caller can tell "too old to answer" from "answered
    /// and failed" without matching on message text (§16, `CROSS-4`).
    #[error("Provider does not support this method: {0}")]
    UnsupportedMethod(String),

    /// §16 class 1: the provider understood the method but refuses the
    /// write (e.g. task writes on an events-only provider).
    #[error("Provider does not support this operation: {0}")]
    UnsupportedOperation(String),

    /// §16 class 2: the provider's credentials lack a scope the request
    /// needs (`GOOGLE-13`'s core half).
    #[error("Provider is missing a required authorization scope: {0}")]
    MissingAuthScope(String),

    /// §16 class 3: the remote object the request named does not exist.
    #[error("Remote object not found: {0}")]
    RemoteNotFound(String),

    /// §16 class 4: a conflict or precondition failure (e.g. a stale ETag).
    #[error("Conflict with remote state: {0}")]
    Conflict(String),

    /// §16 class 5: the provider cannot represent what was asked of it,
    /// distinct from an outright unsupported operation.
    #[error("Provider cannot represent this request: {0}")]
    SemanticLimitation(String),
}

/// Maps a wire-level `rpc::ErrorCode` to the `ProviderError` variant a caller
/// matches on. `None` and `Some(ErrorCode::Unrecognized)` both fall through
/// to the pre-existing generic `Provider` variant: an absent code is every
/// provider built before this ticket, and an unrecognised one is a *newer*
/// provider using a class this binary doesn't know yet — neither is grounds
/// to fail (§18, C-4).
pub(crate) fn classify(error: String, code: Option<rpc::ErrorCode>) -> ProviderError {
    match code {
        Some(rpc::ErrorCode::UnsupportedMethod) => ProviderError::UnsupportedMethod(error),
        Some(rpc::ErrorCode::UnsupportedOperation) => ProviderError::UnsupportedOperation(error),
        Some(rpc::ErrorCode::MissingAuthScope) => ProviderError::MissingAuthScope(error),
        Some(rpc::ErrorCode::RemoteNotFound) => ProviderError::RemoteNotFound(error),
        Some(rpc::ErrorCode::Conflict) => ProviderError::Conflict(error),
        Some(rpc::ErrorCode::SemanticLimitation) => ProviderError::SemanticLimitation(error),
        Some(rpc::ErrorCode::Network) => {
            ProviderError::Transport(ProviderTransportError::Network(error))
        }
        Some(rpc::ErrorCode::Unrecognized) | None => ProviderError::Provider(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `provider::error::tests::every_error_class_round_trips_over_the_wire`
    /// (P2-04). Table-driven over all six §16 classes: serialize a
    /// `Response::Error` carrying each code, parse it back, and confirm the
    /// code survives and none of the six collapse into each other. Five of
    /// six have no production producer yet (Wave 3/5); without this table
    /// they would ship untested and a serde rename could silently merge two.
    #[test]
    fn every_error_class_round_trips_over_the_wire() {
        let classes = [
            rpc::ErrorCode::UnsupportedOperation,
            rpc::ErrorCode::MissingAuthScope,
            rpc::ErrorCode::RemoteNotFound,
            rpc::ErrorCode::Conflict,
            rpc::ErrorCode::SemanticLimitation,
            rpc::ErrorCode::Network,
        ];

        let mut seen_wire_strings = std::collections::HashSet::new();

        for code in classes {
            let json = rpc::Response::<()>::error("boom", Some(code));

            let decoded: rpc::Response<()> =
                serde_json::from_str(&json).expect("an envelope this crate wrote must decode");

            match decoded {
                rpc::Response::Error {
                    code: decoded_code, ..
                } => {
                    assert_eq!(decoded_code, Some(code), "class did not survive the wire");
                }
                rpc::Response::Success { .. } => panic!("expected an error envelope"),
            }

            // The wire string itself is the thing a real cross-version
            // decode keys on; two classes serializing to the same string
            // would silently collapse regardless of what the Rust enum says.
            let wire_string = serde_json::to_value(code).unwrap();
            assert!(
                seen_wire_strings.insert(wire_string.clone()),
                "two classes serialize to the same wire value: {wire_string:?}"
            );
        }
    }
}
