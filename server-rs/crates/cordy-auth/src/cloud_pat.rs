//! Cordy Cloud Node PAT (`mcn_`) Fleet verifier and positive Redis cache.

use std::time::Duration;

use redis::aio::ConnectionManager;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

pub const CLOUD_PAT_PREFIX: &str = "mcn_";
const CACHE_PREFIX: &str = "mul:auth:mcn:";
const CACHE_TTL: Duration = Duration::from_secs(60);
const REDIS_TIMEOUT: Duration = Duration::from_millis(250);
const HTTP_TIMEOUT: Duration = Duration::from_secs(5);
const REQUEST_MAX: usize = 4 * 1024;
const RESPONSE_MAX: usize = 64 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CloudPatIdentity {
    #[serde(rename = "o")]
    pub owner_id: String,
    #[serde(rename = "i")]
    pub instance_id: String,
    #[serde(rename = "r")]
    pub instance_record_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedCloudPat {
    pub identity: CloudPatIdentity,
    pub owner_already_validated: bool,
}

#[derive(Debug)]
pub enum CloudPatError {
    Invalid,
    Unavailable,
}

impl std::fmt::Display for CloudPatError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Invalid => "cloud pat invalid",
            Self::Unavailable => "cloud pat verifier unavailable",
        })
    }
}

impl std::error::Error for CloudPatError {}

#[derive(Clone)]
pub struct CloudPatVerifier {
    base_url: String,
    http: reqwest::Client,
    cache: Option<ConnectionManager>,
}

#[derive(Serialize)]
struct FleetRequest<'a> {
    token: &'a str,
}

#[derive(Deserialize)]
struct FleetResponse {
    valid: bool,
    owner_id: Option<String>,
    instance_id: Option<String>,
    instance_record_id: Option<String>,
}

impl CloudPatVerifier {
    pub fn new(base_url: &str) -> Option<Self> {
        let base_url = base_url.trim().trim_end_matches('/');
        if base_url.is_empty() {
            return None;
        }
        let http = reqwest::Client::builder()
            .timeout(HTTP_TIMEOUT)
            .build()
            .ok()?;
        Some(Self {
            base_url: base_url.to_string(),
            http,
            cache: None,
        })
    }

    pub fn set_cache(&mut self, cache: ConnectionManager) {
        self.cache = Some(cache);
    }

    pub async fn verify(
        &self,
        token: &str,
        cancel: &CancellationToken,
    ) -> Result<VerifiedCloudPat, CloudPatError> {
        if token.is_empty() {
            return Err(CloudPatError::Invalid);
        }
        let hash = crate::jwt::hash_token(token);
        if let Some(identity) = self.cache_get(&hash, cancel).await {
            return Ok(VerifiedCloudPat {
                identity,
                owner_already_validated: true,
            });
        }
        if cancel.is_cancelled() {
            return Err(CloudPatError::Unavailable);
        }

        let body =
            serde_json::to_vec(&FleetRequest { token }).map_err(|_| CloudPatError::Unavailable)?;
        if body.len() > REQUEST_MAX {
            return Err(CloudPatError::Unavailable);
        }
        let request = self
            .http
            .post(format!("{}/api/v1/pat/verify", self.base_url))
            .header(reqwest::header::ACCEPT, "application/json")
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body)
            .send();
        let response = tokio::select! {
            _ = cancel.cancelled() => return Err(CloudPatError::Unavailable),
            result = request => result.map_err(|error| {
                tracing::warn!(%error, "cloud PAT Fleet request failed");
                CloudPatError::Unavailable
            })?,
        };
        if response.status() != reqwest::StatusCode::OK {
            tracing::warn!(status = %response.status(), "cloud PAT Fleet returned non-200");
            return Err(CloudPatError::Unavailable);
        }
        let mut response = response;
        let mut raw = Vec::new();
        loop {
            let chunk = tokio::select! {
                _ = cancel.cancelled() => return Err(CloudPatError::Unavailable),
                result = response.chunk() => result.map_err(|error| {
                    tracing::warn!(%error, "cloud PAT Fleet response read failed");
                    CloudPatError::Unavailable
                })?,
            };
            let Some(chunk) = chunk else { break };
            if raw.len().saturating_add(chunk.len()) > RESPONSE_MAX {
                tracing::warn!(limit = RESPONSE_MAX, "cloud PAT Fleet response too large");
                return Err(CloudPatError::Unavailable);
            }
            raw.extend_from_slice(&chunk);
        }
        let parsed: FleetResponse = serde_json::from_slice(&raw).map_err(|error| {
            tracing::warn!(%error, "cloud PAT Fleet response decode failed");
            CloudPatError::Unavailable
        })?;
        if !parsed.valid {
            return Err(CloudPatError::Invalid);
        }
        let owner_id = parsed
            .owner_id
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                tracing::warn!("cloud PAT Fleet returned valid identity without owner");
                CloudPatError::Unavailable
            })?;
        Ok(VerifiedCloudPat {
            identity: CloudPatIdentity {
                owner_id,
                instance_id: parsed.instance_id.unwrap_or_default(),
                instance_record_id: parsed.instance_record_id.unwrap_or_default(),
            },
            owner_already_validated: false,
        })
    }

    pub async fn cache_validated(
        &self,
        token: &str,
        identity: &CloudPatIdentity,
        cancel: &CancellationToken,
    ) {
        let Some(cache) = self.cache.as_ref() else {
            return;
        };
        let Ok(raw) = serde_json::to_vec(identity) else {
            return;
        };
        let mut cache = cache.clone();
        let key = format!("{CACHE_PREFIX}{}", crate::jwt::hash_token(token));
        let operation = redis::cmd("SET")
            .arg(key)
            .arg(raw)
            .arg("PX")
            .arg(CACHE_TTL.as_millis() as u64)
            .query_async::<()>(&mut cache);
        let result = tokio::select! {
            _ = cancel.cancelled() => return,
            result = tokio::time::timeout(REDIS_TIMEOUT, operation) => result,
        };
        if !matches!(result, Ok(Ok(()))) {
            tracing::warn!("cloud PAT cache set failed; continuing without cache");
        }
    }

    async fn cache_get(&self, hash: &str, cancel: &CancellationToken) -> Option<CloudPatIdentity> {
        let cache = self.cache.as_ref()?;
        let mut cache = cache.clone();
        let operation = tokio::time::timeout(
            REDIS_TIMEOUT,
            redis::cmd("GET")
                .arg(format!("{CACHE_PREFIX}{hash}"))
                .query_async::<Option<Vec<u8>>>(&mut cache),
        );
        let raw = tokio::select! {
            _ = cancel.cancelled() => return None,
            result = operation => match result {
                Ok(Ok(Some(raw))) => raw,
                Ok(Ok(None)) => return None,
                Ok(Err(error)) => {
                    tracing::warn!(%error, "cloud PAT cache get failed; falling back to Fleet");
                    return None;
                }
                Err(_) => {
                    tracing::warn!("cloud PAT cache get timed out; falling back to Fleet");
                    return None;
                }
            }
        };
        match serde_json::from_slice::<CloudPatIdentity>(&raw) {
            Ok(identity) if !identity.owner_id.is_empty() => Some(identity),
            Ok(_) | Err(_) => {
                tracing::warn!("cloud PAT cache entry malformed; falling back to Fleet");
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn namespace_ttl_and_wire_shape_match_go() {
        assert_eq!(CACHE_PREFIX, "mul:auth:mcn:");
        assert_eq!(CACHE_TTL, Duration::from_secs(60));
        let identity = CloudPatIdentity {
            owner_id: "owner".into(),
            instance_id: "instance".into(),
            instance_record_id: "record".into(),
        };
        assert_eq!(
            serde_json::to_string(&identity).unwrap(),
            r#"{"o":"owner","i":"instance","r":"record"}"#
        );
    }
}
