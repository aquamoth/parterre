//! The HTTPS client both forges' APIs are asked with.

use std::fmt;
use std::time::Duration;

use ureq::Body;
use ureq::http::Response;

use crate::{Forge, ForgeError};

const TIMEOUT: Duration = Duration::from_secs(30);
const MAX_RESPONSE_BYTES: u64 = 64 * 1024 * 1024;
/// Of an error answer that isn't the forge's JSON, such as a proxy's page.
const MAX_ERROR_MESSAGE_CHARS: usize = 200;

/// Never printed: its `Debug` shows stars.
pub(crate) struct Token(pub(crate) String);

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Token(***)")
    }
}

pub(crate) fn agent() -> ureq::Agent {
    use ureq::tls::{RootCerts, TlsConfig};
    // The system's certificate authorities, including any a company adds.
    let tls = TlsConfig::builder()
        .root_certs(RootCerts::PlatformVerifier)
        .build();
    ureq::Agent::config_builder()
        .tls_config(tls)
        .timeout_global(Some(TIMEOUT))
        // Error answers carry the reason, and the rate limit in their headers.
        .http_status_as_error(false)
        .max_redirects(0)
        .user_agent(concat!("parterre/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

pub(crate) fn network_error(forge: Forge) -> impl Fn(ureq::Error) -> ForgeError {
    move |e| ForgeError::Network(forge, e.to_string())
}

pub(crate) fn read_body(response: &mut Response<Body>, forge: Forge) -> Result<String, ForgeError> {
    response
        .body_mut()
        .with_config()
        .limit(MAX_RESPONSE_BYTES)
        .read_to_string()
        .map_err(network_error(forge))
}

pub(crate) fn number_header(response: &Response<Body>, name: &str) -> Option<u64> {
    response
        .headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse().ok())
}

/// With the `message` of the answer's JSON, or the start of its body.
pub(crate) fn status_error(forge: Forge, status: u16, body: &str) -> ForgeError {
    #[derive(serde::Deserialize)]
    struct Message {
        message: String,
    }
    let message = serde_json::from_str::<Message>(body)
        .map(|m| m.message)
        .unwrap_or_else(|_| body.chars().take(MAX_ERROR_MESSAGE_CHARS).collect());
    ForgeError::Status {
        forge,
        status,
        message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_not_printed() {
        let token = Token("gho_secret".into());
        assert_eq!(format!("{token:?}"), "Token(***)");
        assert!(!format!("{:?}", Some(token)).contains("secret"));
    }
}
