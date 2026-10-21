use crate::calendar::CalendarError;
use crate::remote::RemoteError;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ConnectionError {
    #[error("Remote error: {0}")]
    Remote(#[from] RemoteError),

    #[error("Local calendar error: {0}")]
    Calendar(#[from] CalendarError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::ProviderError;

    /// `GOOGLE-13`'s core half: the distinguishability has to exist before
    /// there is anything to distinguish. A provider reply tagged
    /// `ErrorCode::MissingAuthScope` reaches `Connection`'s callers as a
    /// `ConnectionError` a caller can `match` on — never a string a caller
    /// has to parse. `RemoteError::Provider` is `#[from] ProviderError`, so
    /// this drives the same conversion path a real `Connection::diff`/
    /// `apply_outgoing_diff` failure would.
    #[test]
    fn a_missing_scope_failure_is_distinguishable_without_string_matching() {
        let provider_error = ProviderError::MissingAuthScope("reauth required".to_string());
        let err: ConnectionError = RemoteError::from(provider_error).into();

        let described = match &err {
            ConnectionError::Remote(RemoteError::Provider(ProviderError::MissingAuthScope(_))) => {
                "missing_auth_scope"
            }
            ConnectionError::Remote(_) => "some other remote failure",
            ConnectionError::Calendar(_) => "calendar",
        };

        assert_eq!(described, "missing_auth_scope");
    }
}
