//! OAuth primitives for remote MCP authorization.
//!
//! The network flows in this module are migrated from
//! `server/pkg/remotemcp/oauth.go`.  The public value types intentionally only
//! contain discovery data and token response fields; client secrets and
//! access tokens must not be placed in discovery metadata.

use std::time::{Duration, SystemTime};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use bytes::Bytes;
use http::{header, Method, Request};
use http_body_util::Full;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use url::{form_urlencoded, Url};

use crate::client::{new_secure_http_client, RequestBody};
use crate::error::Error;
use crate::validate::{validate_public_https_endpoint, SystemResolver};

const MAX_OAUTH_RESPONSE_BYTES: usize = 1 << 20;

/// Safe, public fields returned by the MCP OAuth discovery chain.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OAuthMetadata {
    pub resource_endpoint: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub registration_endpoint: String,
    pub scopes: Vec<String>,
    pub token_auth_methods: Vec<String>,
}

/// OAuth client credentials returned by dynamic registration or supplied by
/// an operator for a pre-registered client.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OAuthRegistration {
    pub client_id: String,
    pub client_secret: String,
    pub token_endpoint_auth_method: String,
}

/// OAuth token response normalized to a Bearer access token.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OAuthToken {
    pub access_token: String,
    pub token_type: String,
    pub expires_in: i64,
    pub refresh_token: String,
    pub scope: String,
}

/// Go-port spelling retained for callers that use the source type names.
pub type OAuthClientRegistration = OAuthRegistration;

/// Go-port spelling retained for callers that use the source type names.
pub type OAuthTokenResponse = OAuthToken;

/// Short names used by the Rust OAuth API.
pub type Registration = OAuthRegistration;

/// Short names used by the Rust OAuth API.
pub type Token = OAuthToken;

/// Builds an authorization-code URL with S256 PKCE, state, and MCP resource
/// binding while preserving unrelated query parameters already on the URL.
pub fn build_authorization_url(
    metadata: &OAuthMetadata,
    registration: &OAuthRegistration,
    redirect_uri: &str,
    state: &str,
    verifier: &str,
    scope: &str,
) -> Result<String, Error> {
    let mut endpoint = Url::parse(&metadata.authorization_endpoint)
        .map_err(|error| Error::ParseEndpoint(error.to_string()))?;
    let digest = Sha256::digest(verifier.as_bytes());
    let challenge = URL_SAFE_NO_PAD.encode(digest);

    let mut query: Vec<(String, String)> = endpoint.query_pairs().into_owned().collect();
    set_query_parameter(&mut query, "response_type", "code");
    set_query_parameter(&mut query, "client_id", &registration.client_id);
    set_query_parameter(&mut query, "redirect_uri", redirect_uri);
    set_query_parameter(&mut query, "state", state);
    set_query_parameter(&mut query, "code_challenge", &challenge);
    set_query_parameter(&mut query, "code_challenge_method", "S256");
    set_query_parameter(&mut query, "resource", &metadata.resource_endpoint);
    if !scope.trim().is_empty() {
        set_query_parameter(&mut query, "scope", scope.trim());
    }

    let encoded = form_urlencoded::Serializer::new(String::new())
        .extend_pairs(query)
        .finish();
    endpoint.set_query(Some(&encoded));
    Ok(endpoint.to_string())
}

fn set_query_parameter(query: &mut Vec<(String, String)>, key: &str, value: &str) {
    query.retain(|(existing, _)| existing != key);
    query.push((key.to_string(), value.to_string()));
}

/// Returns the absolute expiry time for a token.
///
/// `UNIX_EPOCH` is the Rust sentinel for the Go implementation's zero
/// `time.Time`, used when the server omits or invalidates `expires_in`.
pub fn oauth_expiry(now: SystemTime, expires_in: i64) -> SystemTime {
    if expires_in <= 0 {
        return SystemTime::UNIX_EPOCH;
    }
    now.checked_add(Duration::from_secs(expires_in as u64))
        .unwrap_or(SystemTime::UNIX_EPOCH)
}

#[derive(Debug, Deserialize)]
struct ProtectedResourceMetadata {
    #[serde(default)]
    resource: String,
    #[serde(default)]
    authorization_servers: Vec<String>,
    #[serde(default)]
    scopes_supported: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct AuthorizationServerMetadata {
    authorization_endpoint: String,
    token_endpoint: String,
    #[serde(default)]
    registration_endpoint: String,
    #[serde(default)]
    code_challenge_methods_supported: Vec<String>,
    #[serde(default)]
    token_endpoint_auth_methods_supported: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct DynamicRegistrationResponse {
    client_id: String,
    #[serde(default)]
    client_secret: String,
    #[serde(default)]
    token_endpoint_auth_method: String,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    #[serde(default)]
    access_token: String,
    #[serde(default)]
    token_type: String,
    #[serde(default)]
    expires_in: Value,
    #[serde(default)]
    refresh_token: String,
    #[serde(default)]
    scope: String,
}

/// Discovers the OAuth endpoints advertised by an MCP protected resource.
///
/// The endpoint and every discovered URL are independently checked against
/// the public-HTTPS SSRF boundary. The initial endpoint additionally honors
/// the caller's allowed-host policy, matching the Go service boundary.
pub async fn discover_oauth(
    raw_endpoint: &str,
    allowed_hosts: &[String],
) -> Result<OAuthMetadata, Error> {
    let endpoint =
        validate_public_https_endpoint(raw_endpoint, allowed_hosts, Some(&SystemResolver)).await?;

    let metadata_url = if let Some(url) = probe_resource_metadata_url(&endpoint).await {
        validate_oauth_url(&url).await?
    } else {
        let url = protected_resource_metadata_url(&endpoint);
        // Validate the exact fallback URL with the same boundary and host
        // policy as the original request before using it for discovery.
        validate_public_https_endpoint(
            url.as_str(),
            allowed_hosts,
            Some(&SystemResolver),
        )
        .await?
    };

    let resource: ProtectedResourceMetadata = get_oauth_json(&metadata_url)
        .await
        .map_err(|error| Error::Request(format!("load protected resource metadata: {error}")))?;
    if !resource.resource.is_empty() && resource.resource != endpoint.as_str() {
        return Err(Error::Request(
            "protected resource metadata does not match the MCP endpoint".into(),
        ));
    }
    let issuer = resource.authorization_servers.first().ok_or_else(|| {
        Error::Request(
            "protected resource metadata does not advertise an authorization server".into(),
        )
    })?;
    let issuer = validate_oauth_url(issuer)
        .await
        .map_err(|error| Error::Request(format!("authorization server: {error}")))?;

    let mut server = None;
    let mut last_error = None;
    for candidate in authorization_metadata_urls(&issuer) {
        let candidate = match validate_oauth_url(candidate.as_str()).await {
            Ok(candidate) => candidate,
            Err(error) => {
                last_error = Some(error);
                continue;
            }
        };
        match get_oauth_json::<AuthorizationServerMetadata>(&candidate).await {
            Ok(metadata) => {
                server = Some(metadata);
                break;
            }
            Err(error) => last_error = Some(error),
        }
    }
    let server = server.ok_or_else(|| {
        Error::Request(format!(
            "load authorization server metadata: {}",
            last_error
                .map(|error| error.to_string())
                .unwrap_or_else(|| "no discovery document succeeded".into())
        ))
    })?;
    if server.authorization_endpoint.is_empty() || server.token_endpoint.is_empty() {
        return Err(Error::Request(
            "authorization server metadata is missing required endpoints".into(),
        ));
    }
    if !server.code_challenge_methods_supported.is_empty()
        && !server
            .code_challenge_methods_supported
            .iter()
            .any(|method| method == "S256")
    {
        return Err(Error::Request(
            "authorization server does not support PKCE S256".into(),
        ));
    }

    let metadata = OAuthMetadata {
        resource_endpoint: endpoint.to_string(),
        authorization_endpoint: server.authorization_endpoint,
        token_endpoint: server.token_endpoint,
        registration_endpoint: server.registration_endpoint,
        scopes: resource.scopes_supported,
        token_auth_methods: server.token_endpoint_auth_methods_supported,
    };
    validate_oauth_metadata(&metadata).await?;
    Ok(metadata)
}

/// Validates operator-supplied OAuth endpoint overrides using the same public
/// HTTPS and DNS-rebinding boundary as discovered endpoints.
pub async fn validate_oauth_metadata(metadata: &OAuthMetadata) -> Result<(), Error> {
    if metadata.authorization_endpoint.is_empty() || metadata.token_endpoint.is_empty() {
        return Err(Error::Request(
            "OAuth metadata is missing required endpoints".into(),
        ));
    }
    for raw in [
        metadata.authorization_endpoint.as_str(),
        metadata.token_endpoint.as_str(),
        metadata.registration_endpoint.as_str(),
    ] {
        if !raw.is_empty() {
            validate_oauth_url(raw).await?;
        }
    }
    Ok(())
}

/// Registers a public OAuth client using RFC 7591 dynamic registration.
pub async fn register_oauth_client(
    metadata: &OAuthMetadata,
    redirect_uri: &str,
) -> Result<OAuthRegistration, Error> {
    if metadata.registration_endpoint.is_empty() {
        return Err(Error::Request(
            "authorization server requires a pre-registered OAuth client".into(),
        ));
    }
    let endpoint = validate_oauth_url(&metadata.registration_endpoint).await?;
    let body = serde_json::to_vec(&serde_json::json!({
        "client_name": "Cordy",
        "redirect_uris": [redirect_uri],
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "token_endpoint_auth_method": "none",
    }))
    .map_err(|error| Error::Request(error.to_string()))?;
    let request = json_request(Method::POST, &endpoint, body)?;
    let response: DynamicRegistrationResponse = send_json(request, &endpoint).await?;
    if response.client_id.trim().is_empty() {
        return Err(Error::Request(
            "dynamic client registration returned no client id".into(),
        ));
    }
    Ok(OAuthRegistration {
        client_id: response.client_id,
        client_secret: response.client_secret,
        token_endpoint_auth_method: if response.token_endpoint_auth_method.is_empty() {
            "none".into()
        } else {
            response.token_endpoint_auth_method
        },
    })
}

/// Exchanges an authorization code for a Bearer token.
pub async fn exchange_oauth_code(
    token_endpoint: &str,
    resource: &str,
    code: &str,
    redirect_uri: &str,
    verifier: &str,
    registration: &OAuthRegistration,
) -> Result<OAuthToken, Error> {
    let values = vec![
        ("grant_type".to_string(), "authorization_code".to_string()),
        ("code".to_string(), code.to_string()),
        ("redirect_uri".to_string(), redirect_uri.to_string()),
        ("client_id".to_string(), registration.client_id.clone()),
        ("code_verifier".to_string(), verifier.to_string()),
        ("resource".to_string(), resource.to_string()),
    ];
    request_oauth_token(token_endpoint, values, registration).await
}

/// Exchanges a refresh token for a new Bearer token.
pub async fn refresh_oauth_token(
    token_endpoint: &str,
    resource: &str,
    refresh_token: &str,
    registration: &OAuthRegistration,
) -> Result<OAuthToken, Error> {
    let values = vec![
        ("grant_type".to_string(), "refresh_token".to_string()),
        ("refresh_token".to_string(), refresh_token.to_string()),
        ("client_id".to_string(), registration.client_id.clone()),
        ("resource".to_string(), resource.to_string()),
    ];
    request_oauth_token(token_endpoint, values, registration).await
}

async fn request_oauth_token(
    raw_endpoint: &str,
    mut values: Vec<(String, String)>,
    registration: &OAuthRegistration,
) -> Result<OAuthToken, Error> {
    let endpoint = validate_oauth_url(raw_endpoint).await?;
    let mut serializer = form_urlencoded::Serializer::new(String::new());
    if !registration.client_secret.is_empty()
        && registration.token_endpoint_auth_method == "client_secret_post"
    {
        values.push((
            "client_secret".to_string(),
            registration.client_secret.clone(),
        ));
    }
    for (key, value) in &values {
        serializer.append_pair(key, value);
    }
    let body = serializer.finish().into_bytes();
    let mut request = Request::builder()
        .method(Method::POST)
        .uri(endpoint.as_str())
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::ACCEPT, "application/json")
        .body(Full::new(Bytes::from(body)))
        .map_err(|error| Error::InvalidUri(error.to_string()))?;
    if !registration.client_secret.is_empty()
        && registration.token_endpoint_auth_method == "client_secret_basic"
    {
        let credentials = format!("{}:{}", registration.client_id, registration.client_secret);
        let encoded = base64::engine::general_purpose::STANDARD.encode(credentials);
        let value = format!("Basic {encoded}");
        request.headers_mut().insert(
            header::AUTHORIZATION,
            value
                .parse()
                .map_err(|error| Error::Request(format!("basic auth: {error}")))?,
        );
    }
    let response: TokenResponse = send_json(request, &endpoint).await?;
    if response.access_token.is_empty() || !response.token_type.eq_ignore_ascii_case("Bearer") {
        return Err(Error::Request(
            "token endpoint did not return a Bearer access token".into(),
        ));
    }
    Ok(OAuthToken {
        access_token: response.access_token,
        token_type: "Bearer".into(),
        expires_in: parse_expires_in(&response.expires_in),
        refresh_token: response.refresh_token,
        scope: response.scope,
    })
}

fn parse_expires_in(value: &Value) -> i64 {
    value
        .as_i64()
        .or_else(|| value.as_str().and_then(|value| value.trim().parse().ok()))
        .unwrap_or(0)
}

async fn validate_oauth_url(raw: &str) -> Result<Url, Error> {
    let endpoint =
        Url::parse(raw.trim()).map_err(|error| Error::ParseEndpoint(error.to_string()))?;
    if endpoint.scheme() != "https"
        || endpoint.host_str().is_none_or(str::is_empty)
        || has_userinfo(&endpoint)
        || endpoint.fragment().is_some()
    {
        return Err(Error::NotPublicHttps);
    }
    let mut check = endpoint.clone();
    check.set_query(None);
    validate_public_https_endpoint(check.as_str(), &[], Some(&SystemResolver)).await?;
    Ok(endpoint)
}

fn has_userinfo(endpoint: &Url) -> bool {
    let serialized = endpoint.as_str();
    let Some((_, rest)) = serialized.split_once("://") else {
        return false;
    };
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    rest[..authority_end].contains('@')
}

async fn probe_resource_metadata_url(endpoint: &Url) -> Option<String> {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-03-26",
            "capabilities": {},
            "clientInfo": {"name": "cordy-oauth-discovery", "version": "1"}
        }
    });
    let body = serde_json::to_vec(&body).ok()?;
    let request = json_request(Method::POST, endpoint, body).ok()?;
    let response = new_secure_http_client(endpoint).send(request).await.ok()?;
    response
        .headers()
        .get_all(header::WWW_AUTHENTICATE)
        .iter()
        .find_map(|value| parse_resource_metadata_parameter(value.to_str().ok()?))
}

fn parse_resource_metadata_parameter(challenge: &str) -> Option<String> {
    let lower = challenge.to_ascii_lowercase();
    let marker = "resource_metadata=\"";
    let start = lower.find(marker)?;
    let value_start = start + marker.len();
    let value_end = challenge[value_start..].find('"')? + value_start;
    Some(challenge[value_start..value_end].to_string())
}

fn protected_resource_metadata_url(resource: &Url) -> Url {
    let mut result = resource.clone();
    let path = resource.path().trim_end_matches('/');
    let path = if path.is_empty() {
        "/.well-known/oauth-protected-resource".to_string()
    } else {
        format!("/.well-known/oauth-protected-resource{path}")
    };
    result.set_path(&path);
    result.set_query(None);
    result.set_fragment(None);
    result
}

fn authorization_metadata_urls(issuer: &Url) -> [Url; 2] {
    let path = issuer.path().trim_end_matches('/');
    [
        well_known_url(issuer, "oauth-authorization-server", path),
        well_known_url(issuer, "openid-configuration", path),
    ]
}

fn well_known_url(issuer: &Url, name: &str, path: &str) -> Url {
    let mut result = issuer.clone();
    result.set_path(&format!("/.well-known/{name}{path}"));
    result.set_query(None);
    result.set_fragment(None);
    result
}

async fn get_oauth_json<T: DeserializeOwned>(endpoint: &Url) -> Result<T, Error> {
    let request = Request::builder()
        .method(Method::GET)
        .uri(endpoint.as_str())
        .header(header::ACCEPT, "application/json")
        .body(Full::new(Bytes::new()))
        .map_err(|error| Error::InvalidUri(error.to_string()))?;
    send_json(request, endpoint).await
}

fn json_request(
    method: Method,
    endpoint: &Url,
    body: Vec<u8>,
) -> Result<Request<RequestBody>, Error> {
    Request::builder()
        .method(method)
        .uri(endpoint.as_str())
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ACCEPT, "application/json")
        .body(Full::new(Bytes::from(body)))
        .map_err(|error| Error::InvalidUri(error.to_string()))
}

async fn send_json<T: DeserializeOwned>(
    request: Request<RequestBody>,
    endpoint: &Url,
) -> Result<T, Error> {
    let response = new_secure_http_client(endpoint).send(request).await?;
    let status = response.status();
    let body = response.into_body();
    if !(200..300).contains(&status.as_u16()) {
        return Err(Error::Request(format!("HTTP {}", status.as_u16())));
    }
    if body.len() > MAX_OAUTH_RESPONSE_BYTES {
        return Err(Error::ResponseTooLarge);
    }
    serde_json::from_slice(&body).map_err(|error| Error::Request(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authorization_url_preserves_query_and_pins_pkce_state_and_resource() {
        let metadata = OAuthMetadata {
            resource_endpoint: "https://api.example.com/mcp".into(),
            authorization_endpoint: "https://login.example.com/authorize?audience=existing".into(),
            ..OAuthMetadata::default()
        };
        let registration = OAuthRegistration {
            client_id: "client-1".into(),
            ..OAuthRegistration::default()
        };
        let raw = build_authorization_url(
            &metadata,
            &registration,
            "https://cordy.example/api/callback",
            "state-1",
            "verifier-1",
            "search read",
        )
        .unwrap();
        let parsed = Url::parse(&raw).unwrap();
        let query: std::collections::HashMap<_, _> = parsed.query_pairs().into_owned().collect();
        let expected = URL_SAFE_NO_PAD.encode(Sha256::digest(b"verifier-1"));
        assert_eq!(query.get("audience").map(String::as_str), Some("existing"));
        assert_eq!(query.get("response_type").map(String::as_str), Some("code"));
        assert_eq!(query.get("client_id").map(String::as_str), Some("client-1"));
        assert_eq!(
            query.get("redirect_uri").map(String::as_str),
            Some("https://cordy.example/api/callback")
        );
        assert_eq!(query.get("state").map(String::as_str), Some("state-1"));
        assert_eq!(
            query.get("code_challenge").map(String::as_str),
            Some(expected.as_str())
        );
        assert_eq!(
            query.get("code_challenge_method").map(String::as_str),
            Some("S256")
        );
        assert_eq!(
            query.get("resource").map(String::as_str),
            Some("https://api.example.com/mcp")
        );
        assert_eq!(query.get("scope").map(String::as_str), Some("search read"));
    }

    #[test]
    fn authorization_url_omits_blank_scope_and_replaces_reserved_parameters() {
        let metadata = OAuthMetadata {
            authorization_endpoint:
                "https://login.example.com/authorize?state=old&scope=old&keep=yes".into(),
            ..OAuthMetadata::default()
        };
        let registration = OAuthRegistration {
            client_id: "client".into(),
            ..OAuthRegistration::default()
        };
        let parsed = Url::parse(
            &build_authorization_url(
                &metadata,
                &registration,
                "https://cordy/cb",
                "new",
                "v",
                " ",
            )
            .unwrap(),
        )
        .unwrap();
        let query: Vec<_> = parsed.query_pairs().into_owned().collect();
        assert_eq!(query.iter().filter(|(key, _)| key == "state").count(), 1);
        assert_eq!(
            query.iter().find(|(key, _)| key == "state").unwrap().1,
            "new"
        );
        assert!(!query.iter().any(|(key, _)| key == "scope"));
        assert!(query
            .iter()
            .any(|(key, value)| key == "keep" && value == "yes"));
    }

    #[test]
    fn oauth_expiry_uses_epoch_for_missing_or_invalid_lifetime() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        assert_eq!(oauth_expiry(now, 3600), now + Duration::from_secs(3600));
        assert_eq!(oauth_expiry(now, 0), SystemTime::UNIX_EPOCH);
        assert_eq!(oauth_expiry(now, -1), SystemTime::UNIX_EPOCH);
    }

    #[test]
    fn authorization_metadata_urls_are_converted_before_validation() {
        let issuer = Url::parse("https://login.example.com/tenant").unwrap();
        let candidates = authorization_metadata_urls(&issuer);
        assert_eq!(
            candidates[0].as_str(),
            "https://login.example.com/.well-known/oauth-authorization-server/tenant"
        );
        assert_eq!(
            candidates[1].as_str(),
            "https://login.example.com/.well-known/openid-configuration/tenant"
        );
    }

    #[tokio::test]
    async fn oauth_url_rejects_empty_userinfo() {
        assert!(validate_oauth_url("https://@8.8.8.8/token").await.is_err());
    }
}
