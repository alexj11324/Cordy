//! Patchbay Cloud Node PAT (`mcn_`) Fleet verifier and positive Redis cache.

use std::time::Duration;

use patchbay_redis::RecoveringConnection;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

pub const CLOUD_PAT_PREFIX: &str = "mcn_";
const CACHE_PREFIX: &str = "patchbay:auth:mcn:";
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
    cache: Option<RecoveringConnection>,
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
            // The request body contains the plaintext PAT. A 307/308 would
            // replay that body to the Location host, so Fleet verification
            // must never follow redirects across this trust boundary.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .ok()?;
        Some(Self {
            base_url: base_url.to_string(),
            http,
            cache: None,
        })
    }

    pub fn set_cache(&mut self, cache: RecoveringConnection) {
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
        let mut command = redis::cmd("SET");
        command
            .arg(key)
            .arg(raw)
            .arg("PX")
            .arg(CACHE_TTL.as_millis() as u64);
        let operation = command.query_async::<()>(&mut cache);
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
        let mut command = redis::cmd("GET");
        command.arg(format!("{CACHE_PREFIX}{hash}"));
        let operation = tokio::time::timeout(
            REDIS_TIMEOUT,
            command.query_async::<Option<Vec<u8>>>(&mut cache),
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
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn fleet_server(
        status: &str,
        body: Vec<u8>,
    ) -> (String, tokio::task::JoinHandle<Vec<u8>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let status = status.to_string();
        let task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let header_end = loop {
                let mut chunk = [0u8; 1024];
                let read = stream.read(&mut chunk).await.unwrap();
                assert_ne!(read, 0, "Fleet request ended before its headers");
                request.extend_from_slice(&chunk[..read]);
                if let Some(position) = request.windows(4).position(|value| value == b"\r\n\r\n") {
                    break position + 4;
                }
            };
            let headers = std::str::from_utf8(&request[..header_end]).unwrap();
            let content_length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().unwrap())
                })
                .unwrap_or_default();
            while request.len() < header_end + content_length {
                let mut chunk = [0u8; 1024];
                let read = stream.read(&mut chunk).await.unwrap();
                assert_ne!(read, 0, "Fleet request ended before its body");
                request.extend_from_slice(&chunk[..read]);
            }
            let response = format!(
                "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            );
            // The response-cap test deliberately makes the client stop
            // reading early, so a reset after the request has been captured
            // is expected and must not make the mock task fail.
            let _ = stream.write_all(response.as_bytes()).await;
            let _ = stream.write_all(&body).await;
            request
        });
        (format!("http://{address}"), task)
    }

    #[test]
    fn namespace_ttl_and_wire_shape_match_go() {
        assert_eq!(CACHE_PREFIX, "patchbay:auth:mcn:");
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

    #[tokio::test]
    async fn fleet_success_returns_uncached_owner_binding() {
        let (url, request) = fleet_server(
            "200 OK",
            br#"{"valid":true,"owner_id":"owner","instance_id":"instance","instance_record_id":"record"}"#
                .to_vec(),
        )
        .await;
        let verifier = CloudPatVerifier::new(&url).unwrap();
        let verified = verifier
            .verify("mcn_secret", &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(
            verified,
            VerifiedCloudPat {
                identity: CloudPatIdentity {
                    owner_id: "owner".into(),
                    instance_id: "instance".into(),
                    instance_record_id: "record".into(),
                },
                owner_already_validated: false,
            }
        );
        let request = String::from_utf8(request.await.unwrap()).unwrap();
        assert!(request.starts_with("POST /api/v1/pat/verify HTTP/1.1\r\n"));
        assert!(request.ends_with(r#"{"token":"mcn_secret"}"#));
    }

    #[tokio::test]
    async fn fleet_invalid_and_non_200_fail_closed() {
        let (url, request) = fleet_server("200 OK", br#"{"valid":false}"#.to_vec()).await;
        let error = CloudPatVerifier::new(&url)
            .unwrap()
            .verify("mcn_revoked", &CancellationToken::new())
            .await
            .unwrap_err();
        assert!(matches!(error, CloudPatError::Invalid));
        request.await.unwrap();

        let (url, request) = fleet_server("503 Service Unavailable", Vec::new()).await;
        let error = CloudPatVerifier::new(&url)
            .unwrap()
            .verify("mcn_unknown", &CancellationToken::new())
            .await
            .unwrap_err();
        assert!(matches!(error, CloudPatError::Unavailable));
        request.await.unwrap();
    }

    #[tokio::test]
    async fn request_response_caps_and_cancellation_fail_closed() {
        let verifier = CloudPatVerifier::new("http://127.0.0.1:9").unwrap();
        let oversized_token = format!("mcn_{}", "x".repeat(REQUEST_MAX));
        assert!(matches!(
            verifier
                .verify(&oversized_token, &CancellationToken::new())
                .await,
            Err(CloudPatError::Unavailable)
        ));

        let (url, request) = fleet_server("200 OK", vec![b' '; RESPONSE_MAX + 1]).await;
        assert!(matches!(
            CloudPatVerifier::new(&url)
                .unwrap()
                .verify("mcn_large_response", &CancellationToken::new())
                .await,
            Err(CloudPatError::Unavailable)
        ));
        request.await.unwrap();

        let cancel = CancellationToken::new();
        cancel.cancel();
        assert!(matches!(
            verifier.verify("mcn_cancelled", &cancel).await,
            Err(CloudPatError::Unavailable)
        ));
    }

    #[tokio::test]
    async fn fleet_redirect_does_not_replay_plaintext_pat() {
        let target = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target_url = format!("http://{}/stolen", target.local_addr().unwrap());
        let redirect = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", redirect.local_addr().unwrap());
        let response = tokio::spawn(async move {
            let (mut stream, _) = redirect.accept().await.unwrap();
            let mut request = [0u8; 2048];
            let read = stream.read(&mut request).await.unwrap();
            assert!(read > 0, "redirect source received an empty request");
            stream
                .write_all(
                    format!(
                        "HTTP/1.1 307 Temporary Redirect\r\nLocation: {target_url}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
        });

        let error = CloudPatVerifier::new(&base_url)
            .unwrap()
            .verify("mcn_must_not_leak", &CancellationToken::new())
            .await
            .unwrap_err();

        response.await.unwrap();
        assert!(matches!(error, CloudPatError::Unavailable));
        assert!(
            tokio::time::timeout(Duration::from_millis(100), target.accept())
                .await
                .is_err(),
            "Fleet verifier replayed the plaintext PAT to a redirect target"
        );
    }
}
