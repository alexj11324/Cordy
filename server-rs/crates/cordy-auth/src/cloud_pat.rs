//! Cordy Cloud node PAT (`mcn_`) verification against Fleet.
//!
//! Fleet owns the token lifecycle and binding. The API server therefore must
//! never look these tokens up in its local PAT table; it resolves the owner by
//! calling Fleet's private verification endpoint and lets the middleware
//! confirm that owner still exists locally.

use std::time::Duration;

use serde::{Deserialize, Serialize};

pub const CLOUD_PAT_PREFIX: &str = "mcn_";

const VERIFY_PATH: &str = "/api/v1/pat/verify";
const MAX_REQUEST_BYTES: usize = 4 * 1024;
const MAX_RESPONSE_BYTES: usize = 64 * 1024;
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CloudPatIdentity {
    pub owner_id: String,
    pub instance_id: String,
    pub instance_record_id: String,
}

#[derive(Debug)]
pub enum CloudPatVerifyError {
    Invalid { reason: String },
    Unavailable { message: String },
}

impl CloudPatVerifyError {
    pub fn is_invalid(&self) -> bool {
        matches!(self, Self::Invalid { .. })
    }
}

impl std::fmt::Display for CloudPatVerifyError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid { reason } if !reason.is_empty() => {
                write!(formatter, "cloud PAT invalid: {reason}")
            }
            Self::Invalid { .. } => formatter.write_str("cloud PAT invalid"),
            Self::Unavailable { message } => {
                write!(formatter, "cloud PAT verifier unavailable: {message}")
            }
        }
    }
}

impl std::error::Error for CloudPatVerifyError {}

#[derive(Clone)]
pub struct CloudPatVerifier {
    base_url: String,
    client: reqwest::Client,
    timeout: Duration,
}

impl CloudPatVerifier {
    /// Returns `None` when Fleet is not configured. Callers fail closed for
    /// `mcn_` tokens instead of falling through to local PAT/JWT validation.
    pub fn new(base_url: impl Into<String>) -> Option<Self> {
        let base_url = base_url.into().trim().trim_end_matches('/').to_string();
        (!base_url.is_empty()).then(|| Self {
            base_url,
            client: reqwest::Client::new(),
            timeout: DEFAULT_TIMEOUT,
        })
    }

    #[cfg(test)]
    fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub async fn verify(&self, token: &str) -> Result<CloudPatIdentity, CloudPatVerifyError> {
        if token.is_empty() {
            return Err(CloudPatVerifyError::Invalid {
                reason: String::new(),
            });
        }

        let body = serde_json::to_vec(&FleetVerifyRequest { token }).map_err(|error| {
            CloudPatVerifyError::Unavailable {
                message: format!("encode request: {error}"),
            }
        })?;
        if body.len() > MAX_REQUEST_BYTES {
            return Err(CloudPatVerifyError::Unavailable {
                message: format!("request exceeds {MAX_REQUEST_BYTES} bytes"),
            });
        }

        let mut response = self
            .client
            .post(format!("{}{VERIFY_PATH}", self.base_url))
            .timeout(self.timeout)
            .header(reqwest::header::ACCEPT, "application/json")
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body)
            .send()
            .await
            .map_err(|error| CloudPatVerifyError::Unavailable {
                message: error.to_string(),
            })?;

        if response.status() != reqwest::StatusCode::OK {
            return Err(CloudPatVerifyError::Unavailable {
                message: format!("Fleet returned HTTP {}", response.status().as_u16()),
            });
        }

        let mut raw = Vec::new();
        while let Some(chunk) =
            response
                .chunk()
                .await
                .map_err(|error| CloudPatVerifyError::Unavailable {
                    message: format!("read response: {error}"),
                })?
        {
            if raw.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
                return Err(CloudPatVerifyError::Unavailable {
                    message: format!("response exceeds {MAX_RESPONSE_BYTES} bytes"),
                });
            }
            raw.extend_from_slice(&chunk);
        }

        let parsed: FleetVerifyResponse =
            serde_json::from_slice(&raw).map_err(|error| CloudPatVerifyError::Unavailable {
                message: format!("decode response: {error}"),
            })?;
        if !parsed.valid {
            return Err(CloudPatVerifyError::Invalid {
                reason: parsed.reason,
            });
        }
        if parsed.owner_id.is_empty() {
            return Err(CloudPatVerifyError::Unavailable {
                message: "valid response omitted owner_id".to_string(),
            });
        }

        Ok(CloudPatIdentity {
            owner_id: parsed.owner_id,
            instance_id: parsed.instance_id,
            instance_record_id: parsed.instance_record_id,
        })
    }
}

#[derive(Serialize)]
struct FleetVerifyRequest<'a> {
    token: &'a str,
}

#[derive(Deserialize)]
struct FleetVerifyResponse {
    valid: bool,
    #[serde(default)]
    reason: String,
    #[serde(default)]
    owner_id: String,
    #[serde(default)]
    instance_id: String,
    #[serde(default)]
    instance_record_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_url_disables_verifier_and_trailing_slash_is_normalized() {
        assert!(CloudPatVerifier::new("  ").is_none());
        let verifier = CloudPatVerifier::new("https://fleet.test///")
            .unwrap()
            .with_timeout(Duration::from_millis(1));
        assert_eq!(verifier.base_url, "https://fleet.test");
        assert_eq!(verifier.timeout, Duration::from_millis(1));
    }
}
