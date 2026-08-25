//! CloudFront private-distribution signing and Secrets Manager key loading.
//!
//! The production key is loaded once at startup and retained only in memory,
//! matching the Go server's cache/rotation boundary: rotate the secret and
//! restart the server after the new CloudFront public key is active.

use std::{collections::BTreeMap, sync::Arc};

use aws_config::{
    default_provider::credentials::DefaultCredentialsChain, meta::region::RegionProviderChain,
    Region,
};
use aws_credential_types::provider::ProvideCredentials;
use axum::{
    extract::{Request, State},
    http::{header, HeaderValue},
    middleware::Next,
    response::Response,
};
use base64::Engine;
use futures_util::StreamExt;
use hmac::{Hmac, Mac};
use rsa::{
    pkcs1::DecodeRsaPrivateKey, pkcs8::DecodePrivateKey, traits::PublicKeyParts, Pkcs1v15Sign,
    RsaPrivateKey,
};
use serde::Deserialize;
use sha1::Sha1;
use sha2::{Digest, Sha256};
use url::Url;

type HmacSha256 = Hmac<Sha256>;
const MAX_SECRET_RESPONSE_BYTES: usize = 1 << 20;
const MAX_COOKIE_TTL: chrono::Duration = chrono::Duration::hours(1);

#[derive(Clone)]
pub struct CloudFrontSigner {
    key_pair_id: Arc<str>,
    private_key: Arc<RsaPrivateKey>,
    domain: Arc<str>,
    cookie_domain: Arc<str>,
}

enum KeySource<'a> {
    SecretsManager(&'a str),
    Environment(&'a str),
}

impl CloudFrontSigner {
    pub async fn from_config(config: &cordy_config::Config) -> anyhow::Result<Option<Self>> {
        let Some(key_pair_id) = nonempty(config.storage.cloudfront_key_pair_id.as_deref()) else {
            return Ok(None);
        };
        let domain = nonempty(config.storage.cloudfront_domain.as_deref()).ok_or_else(|| {
            anyhow::anyhow!(
                "CLOUDFRONT_DOMAIN is required when CLOUDFRONT_KEY_PAIR_ID is configured"
            )
        })?;
        validate_cloudfront_domain(domain)?;
        anyhow::ensure!(
            key_pair_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')),
            "CLOUDFRONT_KEY_PAIR_ID contains unsupported characters"
        );
        let cookie_domain = nonempty(config.auth.cookie_domain.as_deref()).ok_or_else(|| {
            anyhow::anyhow!("COOKIE_DOMAIN is required when CLOUDFRONT_KEY_PAIR_ID is configured")
        })?;
        validate_cookie_domain(cookie_domain)?;

        let pem = match select_key_source(
            config.storage.cloudfront_private_key_secret.as_deref(),
            config.storage.cloudfront_private_key.as_deref(),
        )? {
            KeySource::SecretsManager(secret_id) => load_private_key_secret(secret_id).await?,
            KeySource::Environment(encoded) => base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .map_err(|_| anyhow::anyhow!("CLOUDFRONT_PRIVATE_KEY is not valid base64"))?,
        };
        let private_key = parse_private_key(&pem)?;
        anyhow::ensure!(
            private_key.n().bits() >= 2048,
            "CloudFront RSA private key must be at least 2048 bits"
        );
        Ok(Some(Self {
            key_pair_id: Arc::from(key_pair_id),
            private_key: Arc::new(private_key),
            domain: Arc::from(domain.trim_end_matches('/')),
            cookie_domain: Arc::from(cookie_domain),
        }))
    }

    pub fn signed_url(
        &self,
        raw_url: &str,
        expiry: chrono::DateTime<chrono::Utc>,
        content_disposition: Option<&str>,
    ) -> anyhow::Result<String> {
        let resource = canonical_resource(raw_url, &self.domain, content_disposition)?;
        let policy = policy(&resource, expiry.timestamp());
        let signature = self.sign(&policy)?;
        let separator = if resource.contains('?') { '&' } else { '?' };
        Ok(format!(
            "{resource}{separator}Policy={}&Signature={}&Key-Pair-Id={}",
            cloudfront_base64(policy.as_bytes()),
            cloudfront_base64(&signature),
            self.key_pair_id
        ))
    }

    pub fn can_sign_url(&self, raw_url: &str) -> bool {
        canonical_resource(raw_url, &self.domain, None).is_ok()
    }

    pub fn signed_cookie_headers(
        &self,
        expiry: chrono::DateTime<chrono::Utc>,
    ) -> anyhow::Result<[String; 3]> {
        let resource = format!("https://{}/*", self.domain);
        let policy = policy(&resource, expiry.timestamp());
        let signature = self.sign(&policy)?;
        let expires = expiry.format("%a, %d %b %Y %H:%M:%S GMT");
        let suffix = format!(
            "; Domain={}; Path=/; Expires={expires}; HttpOnly; Secure; SameSite=None",
            self.cookie_domain
        );
        Ok([
            format!(
                "CloudFront-Policy={}{}",
                cloudfront_base64(policy.as_bytes()),
                suffix
            ),
            format!(
                "CloudFront-Signature={}{}",
                cloudfront_base64(&signature),
                suffix
            ),
            format!("CloudFront-Key-Pair-Id={}{}", self.key_pair_id, suffix),
        ])
    }

    pub fn clear_cookie_headers(&self) -> [String; 3] {
        let suffix = format!(
            "; Domain={}; Path=/; Expires=Thu, 01 Jan 1970 00:00:00 GMT; Max-Age=0; HttpOnly; Secure; SameSite=None",
            self.cookie_domain
        );
        [
            format!("CloudFront-Policy={suffix}"),
            format!("CloudFront-Signature={suffix}"),
            format!("CloudFront-Key-Pair-Id={suffix}"),
        ]
    }

    #[cfg(test)]
    pub(crate) fn test_signer() -> Self {
        use rand::rngs::OsRng;
        Self {
            key_pair_id: Arc::from("KTEST"),
            private_key: Arc::new(RsaPrivateKey::new(&mut OsRng, 2048).expect("test RSA key")),
            domain: Arc::from("static.example.test"),
            cookie_domain: Arc::from(".example.test"),
        }
    }

    fn sign(&self, policy: &str) -> anyhow::Result<Vec<u8>> {
        let digest = Sha1::digest(policy.as_bytes());
        self.private_key
            .sign(Pkcs1v15Sign::new::<Sha1>(), &digest)
            .map_err(|_| anyhow::anyhow!("CloudFront policy signing failed"))
    }
}

pub async fn refresh_signed_cookies(
    State(signer): State<Option<Arc<CloudFrontSigner>>>,
    request: Request,
    next: Next,
) -> Response {
    let refresh = signer.is_some() && !has_cookie(request.headers(), "CloudFront-Policy");
    let mut response = next.run(request).await;
    if !refresh {
        return response;
    }
    let Some(signer) = signer else {
        return response;
    };
    let expiry = cloudfront_cookie_expiry(chrono::Utc::now());
    match signer.signed_cookie_headers(expiry) {
        Ok(cookies) => {
            for cookie in cookies {
                if let Ok(value) = HeaderValue::from_str(&cookie) {
                    response.headers_mut().append(header::SET_COOKIE, value);
                }
            }
        }
        Err(error) => tracing::warn!(%error, "failed to sign CloudFront cookies"),
    }
    response
}

pub fn cloudfront_cookie_expiry(
    now: chrono::DateTime<chrono::Utc>,
) -> chrono::DateTime<chrono::Utc> {
    now + chrono::Duration::seconds(cordy_auth::cookie::auth_token_ttl()).min(MAX_COOKIE_TTL)
}

fn has_cookie(headers: &axum::http::HeaderMap, name: &str) -> bool {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .any(|(candidate, _)| candidate == name)
}

fn nonempty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn select_key_source<'a>(
    secret_id: Option<&'a str>,
    encoded_key: Option<&'a str>,
) -> anyhow::Result<KeySource<'a>> {
    if let Some(secret_id) = nonempty(secret_id) {
        return Ok(KeySource::SecretsManager(secret_id));
    }
    if let Some(encoded_key) = nonempty(encoded_key) {
        return Ok(KeySource::Environment(encoded_key));
    }
    anyhow::bail!("CLOUDFRONT_PRIVATE_KEY_SECRET or CLOUDFRONT_PRIVATE_KEY is required")
}

fn validate_cloudfront_domain(domain: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        !domain.contains(['/', '?', '#']) && !domain.chars().any(char::is_whitespace),
        "CLOUDFRONT_DOMAIN must be a hostname"
    );
    let parsed = Url::parse(&format!("https://{domain}/"))?;
    anyhow::ensure!(
        parsed.host_str().is_some(),
        "CLOUDFRONT_DOMAIN must be a hostname"
    );
    Ok(())
}

fn validate_cookie_domain(domain: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        !domain.contains([';', ',', '\r', '\n']) && !domain.chars().any(char::is_whitespace),
        "COOKIE_DOMAIN contains unsupported characters"
    );
    let bare = domain.strip_prefix('.').unwrap_or(domain);
    anyhow::ensure!(!bare.is_empty(), "COOKIE_DOMAIN cannot be empty");
    anyhow::ensure!(
        bare.parse::<std::net::IpAddr>().is_err(),
        "COOKIE_DOMAIN cannot be an IP address"
    );
    Ok(())
}

fn canonical_resource(
    raw_url: &str,
    expected_domain: &str,
    content_disposition: Option<&str>,
) -> anyhow::Result<String> {
    let mut url = Url::parse(raw_url)?;
    anyhow::ensure!(
        url.scheme() == "https" && url.host_str().is_some(),
        "CloudFront resource URL must be absolute HTTPS"
    );
    anyhow::ensure!(
        url.username().is_empty() && url.password().is_none(),
        "CloudFront resource URL cannot contain user information"
    );
    let expected = Url::parse(&format!("https://{expected_domain}/"))?;
    anyhow::ensure!(
        url.host_str().map(str::to_ascii_lowercase)
            == expected.host_str().map(str::to_ascii_lowercase)
            && url.port_or_known_default() == expected.port_or_known_default(),
        "CloudFront resource URL host does not match CLOUDFRONT_DOMAIN"
    );
    let Some(disposition) = content_disposition.filter(|value| !value.is_empty()) else {
        return Ok(raw_url.to_string());
    };
    let mut query = BTreeMap::<String, Vec<String>>::new();
    for (key, value) in url.query_pairs() {
        query
            .entry(key.into_owned())
            .or_default()
            .push(value.into_owned());
    }
    query.insert(
        "response-content-disposition".to_string(),
        vec![disposition.to_string()],
    );
    {
        let mut pairs = url.query_pairs_mut();
        pairs.clear();
        for (key, values) in query {
            for value in values {
                pairs.append_pair(&key, &value);
            }
        }
    }
    Ok(url.to_string())
}

fn policy(resource: &str, expiry: i64) -> String {
    serde_json::json!({
        "Statement": [{
            "Resource": resource,
            "Condition": {"DateLessThan": {"AWS:EpochTime": expiry}}
        }]
    })
    .to_string()
}

fn cloudfront_base64(value: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD
        .encode(value)
        .replace('+', "-")
        .replace('=', "_")
        .replace('/', "~")
}

fn parse_private_key(pem: &[u8]) -> anyhow::Result<RsaPrivateKey> {
    let pem =
        std::str::from_utf8(pem).map_err(|_| anyhow::anyhow!("private key is not UTF-8 PEM"))?;
    RsaPrivateKey::from_pkcs8_pem(pem)
        .or_else(|_| RsaPrivateKey::from_pkcs1_pem(pem))
        .map_err(|_| anyhow::anyhow!("private key is not a valid RSA PKCS8 or PKCS1 PEM"))
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct SecretResponse {
    secret_string: Option<String>,
}

async fn load_private_key_secret(secret_id: &str) -> anyhow::Result<Vec<u8>> {
    let fallback_region = std::env::var("S3_REGION")
        .ok()
        .and_then(|value| nonempty(Some(&value)).map(str::to_string))
        .unwrap_or_else(|| "us-west-2".to_string());
    let region = RegionProviderChain::default_provider()
        .or_else(Region::new(fallback_region))
        .region()
        .await
        .ok_or_else(|| anyhow::anyhow!("resolve Secrets Manager region"))?
        .to_string();
    let provider = DefaultCredentialsChain::builder()
        .region(Region::new(region.clone()))
        .build()
        .await;
    let credentials = provider
        .provide_credentials()
        .await
        .map_err(|error| anyhow::anyhow!("resolve Secrets Manager credentials: {error}"))?;
    let endpoint = secrets_manager_endpoint(&region)?;
    let body = serde_json::to_vec(&serde_json::json!({"SecretId": secret_id}))?;
    let payload_hash = hex::encode(Sha256::digest(&body));
    let now = chrono::Utc::now();
    let date = now.format("%Y%m%d").to_string();
    let timestamp = now.format("%Y%m%dT%H%M%SZ").to_string();
    let hostname = endpoint
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("Secrets Manager endpoint has no host"))?;
    let host = endpoint
        .port()
        .map_or_else(|| hostname.to_string(), |port| format!("{hostname}:{port}"));
    let mut canonical = vec![
        ("content-type", "application/x-amz-json-1.1".to_string()),
        ("host", host),
        ("x-amz-date", timestamp.clone()),
        ("x-amz-target", "secretsmanager.GetSecretValue".to_string()),
    ];
    if let Some(token) = credentials.session_token() {
        canonical.push(("x-amz-security-token", token.trim().to_string()));
    }
    canonical.sort_unstable_by(|left, right| left.0.cmp(right.0));
    let signed_headers = canonical
        .iter()
        .map(|(name, _)| *name)
        .collect::<Vec<_>>()
        .join(";");
    let canonical_headers = canonical
        .iter()
        .map(|(name, value)| format!("{name}:{value}\n"))
        .collect::<String>();
    let canonical_request = format!(
        "POST\n{}\n{}\n{canonical_headers}\n{signed_headers}\n{payload_hash}",
        endpoint.path(),
        endpoint.query().unwrap_or_default()
    );
    let scope = format!("{date}/{region}/secretsmanager/aws4_request");
    let to_sign = format!(
        "AWS4-HMAC-SHA256\n{timestamp}\n{scope}\n{}",
        hex::encode(Sha256::digest(canonical_request.as_bytes()))
    );
    let signature = aws_signature(
        credentials.secret_access_key(),
        &date,
        &region,
        "secretsmanager",
        &to_sign,
    )?;
    let client = secrets_manager_http_client()?;
    let mut request = client
        .post(endpoint)
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-date", &timestamp)
        .header("x-amz-target", "secretsmanager.GetSecretValue")
        .header(
            header::AUTHORIZATION,
            format!(
                "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed_headers}, Signature={signature}",
                credentials.access_key_id()
            ),
        );
    if let Some(token) = credentials.session_token() {
        request = request.header("x-amz-security-token", token);
    }
    let response = request.body(body).send().await?;
    anyhow::ensure!(
        response.status().is_success(),
        "Secrets Manager returned {}",
        response.status()
    );
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        anyhow::ensure!(
            bytes.len().saturating_add(chunk.len()) <= MAX_SECRET_RESPONSE_BYTES,
            "Secrets Manager response is too large"
        );
        bytes.extend_from_slice(&chunk);
    }
    let secret: SecretResponse = serde_json::from_slice(&bytes)?;
    secret
        .secret_string
        .map(String::into_bytes)
        .ok_or_else(|| anyhow::anyhow!("CloudFront private-key secret has no string value"))
}

fn secrets_manager_http_client() -> anyhow::Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        // A 307/308 can replay the signed body and reqwest only strips the
        // standard Authorization header on a cross-host redirect. The AWS
        // session token lives in x-amz-security-token, so following a service
        // redirect could disclose temporary credentials to another host.
        .redirect(reqwest::redirect::Policy::none())
        .build()?)
}

fn secrets_manager_endpoint(region: &str) -> anyhow::Result<Url> {
    let configured = std::env::var("AWS_ENDPOINT_URL_SECRETS_MANAGER")
        .ok()
        .and_then(|value| nonempty(Some(&value)).map(str::to_string))
        .or_else(|| {
            std::env::var("AWS_ENDPOINT_URL")
                .ok()
                .and_then(|value| nonempty(Some(&value)).map(str::to_string))
        });
    if let Some(endpoint) = configured {
        let endpoint = Url::parse(&endpoint)?;
        anyhow::ensure!(
            endpoint.username().is_empty()
                && endpoint.password().is_none()
                && endpoint.query().is_none()
                && endpoint.fragment().is_none(),
            "Secrets Manager endpoint cannot contain credentials, a query, or a fragment"
        );
        let loopback = endpoint
            .host_str()
            .is_some_and(|host| host.eq_ignore_ascii_case("localhost"))
            || matches!(endpoint.host(), Some(url::Host::Ipv4(ip)) if ip.is_loopback())
            || matches!(endpoint.host(), Some(url::Host::Ipv6(ip)) if ip.is_loopback());
        anyhow::ensure!(
            endpoint.scheme() == "https" || (endpoint.scheme() == "http" && loopback),
            "Secrets Manager endpoint must use HTTPS (HTTP is allowed only for loopback testing)"
        );
        return Ok(endpoint);
    }
    let suffix = if region.starts_with("cn-") {
        "amazonaws.com.cn"
    } else {
        "amazonaws.com"
    };
    Url::parse(&format!("https://secretsmanager.{region}.{suffix}/")).map_err(Into::into)
}

fn aws_signature(
    secret: &str,
    date: &str,
    region: &str,
    service: &str,
    to_sign: &str,
) -> anyhow::Result<String> {
    let k_date = hmac(format!("AWS4{secret}").as_bytes(), date.as_bytes())?;
    let k_region = hmac(&k_date, region.as_bytes())?;
    let k_service = hmac(&k_region, service.as_bytes())?;
    let k_signing = hmac(&k_service, b"aws4_request")?;
    Ok(hex::encode(hmac(&k_signing, to_sign.as_bytes())?))
}

fn hmac(key: &[u8], body: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut mac =
        HmacSha256::new_from_slice(key).map_err(|_| anyhow::anyhow!("invalid HMAC key"))?;
    mac.update(body);
    Ok(mac.finalize().into_bytes().to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::OsRng;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn signer() -> CloudFrontSigner {
        CloudFrontSigner {
            key_pair_id: Arc::from("KTEST"),
            private_key: Arc::new(RsaPrivateKey::new(&mut OsRng, 2048).unwrap()),
            domain: Arc::from("static.example.test"),
            cookie_domain: Arc::from(".example.test"),
        }
    }

    #[test]
    fn secrets_manager_has_strict_priority_over_environment_key() {
        assert!(matches!(
            select_key_source(Some("secret-name"), Some("base64-key")).unwrap(),
            KeySource::SecretsManager("secret-name")
        ));
        assert!(matches!(
            select_key_source(None, Some("base64-key")).unwrap(),
            KeySource::Environment("base64-key")
        ));
        assert!(select_key_source(None, None).is_err());
    }

    #[test]
    fn signed_url_canonicalizes_and_binds_content_disposition() {
        let url = signer()
            .signed_url(
                "https://static.example.test/report.md?z=2&a=1&response-content-disposition=old",
                chrono::DateTime::from_timestamp(1_893_456_000, 0).unwrap(),
                Some("attachment; filename=\"report.md\""),
            )
            .unwrap();
        let parsed = Url::parse(&url).unwrap();
        let query = parsed.query_pairs().collect::<BTreeMap<_, _>>();
        assert_eq!(
            query.get("Key-Pair-Id").map(|value| value.as_ref()),
            Some("KTEST")
        );
        assert_eq!(
            query
                .get("response-content-disposition")
                .map(|value| value.as_ref()),
            Some("attachment; filename=\"report.md\"")
        );
        let encoded_policy = query.get("Policy").unwrap();
        let standard = encoded_policy
            .replace('-', "+")
            .replace('_', "=")
            .replace('~', "/");
        let policy = base64::engine::general_purpose::STANDARD
            .decode(standard)
            .unwrap();
        let policy = String::from_utf8(policy).unwrap();
        assert!(policy.contains(
            "a=1&response-content-disposition=attachment%3B+filename%3D%22report.md%22&z=2"
        ));
    }

    #[test]
    fn signer_refuses_to_authorize_a_foreign_host() {
        assert!(signer()
            .signed_url("https://attacker.example/object", chrono::Utc::now(), None)
            .is_err());
        assert!(!signer().can_sign_url("https://attacker.example/object"));
        assert!(signer().can_sign_url("https://static.example.test/object"));
    }

    #[test]
    fn cookies_use_auth_expiry_security_attributes_and_cloudfront_encoding() {
        let cookies = signer()
            .signed_cookie_headers(chrono::DateTime::from_timestamp(1_893_456_000, 0).unwrap())
            .unwrap();
        assert_eq!(cookies.len(), 3);
        for cookie in cookies {
            assert!(cookie.contains("Domain=.example.test"));
            assert!(cookie.contains("HttpOnly; Secure; SameSite=None"));
            let value = cookie.split_once(';').unwrap().0;
            assert!(!value.contains(['+', '/']));
        }
    }

    #[test]
    fn cookie_authorization_is_short_lived_and_revocable_on_logout() {
        let now = chrono::DateTime::from_timestamp(1_893_456_000, 0).unwrap();
        assert!(cloudfront_cookie_expiry(now) <= now + MAX_COOKIE_TTL);
        for cookie in signer().clear_cookie_headers() {
            assert!(cookie.contains("Max-Age=0"));
            assert!(cookie.contains("Expires=Thu, 01 Jan 1970 00:00:00 GMT"));
        }
    }

    #[tokio::test]
    async fn secrets_manager_client_does_not_follow_redirects() {
        let target = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target_url = format!("http://{}/secret", target.local_addr().unwrap());
        let redirect = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let redirect_url = format!("http://{}/", redirect.local_addr().unwrap());
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

        let result = secrets_manager_http_client()
            .unwrap()
            .post(redirect_url)
            .header("x-amz-security-token", "temporary-session-token")
            .body("signed secret request")
            .send()
            .await
            .unwrap();

        response.await.unwrap();
        assert_eq!(result.status(), reqwest::StatusCode::TEMPORARY_REDIRECT);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(100), target.accept())
                .await
                .is_err(),
            "Secrets Manager client followed a redirect carrying AWS session credentials"
        );
    }
}
