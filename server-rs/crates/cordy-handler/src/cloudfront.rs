use std::{sync::Arc, time::Duration};

use aws_config::default_provider::credentials::DefaultCredentialsChain;
use aws_credential_types::provider::ProvideCredentials;
use aws_types::region::Region;
use base64::Engine;
use hmac::{Hmac, Mac};
use rsa::{pkcs1::DecodeRsaPrivateKey, pkcs8::DecodePrivateKey, Pkcs1v15Sign, RsaPrivateKey};
use serde::Deserialize;
use sha1::Sha1;
use sha2::{Digest, Sha256};
use url::Url;

type HmacSha256 = Hmac<Sha256>;

#[derive(Clone)]
pub(crate) struct CloudFrontSigner {
    key_pair_id: Arc<str>,
    private_key: Arc<RsaPrivateKey>,
}

impl CloudFrontSigner {
    pub(crate) async fn from_config(config: &cordy_config::Config) -> anyhow::Result<Option<Self>> {
        let Some(key_pair_id) = config
            .storage
            .cloudfront_key_pair_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return Ok(None);
        };
        anyhow::ensure!(
            config
                .storage
                .cloudfront_domain
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty()),
            "CLOUDFRONT_DOMAIN is required when CLOUDFRONT_KEY_PAIR_ID is configured"
        );

        let encoded_or_pem = if let Some(secret_name) = config
            .storage
            .cloudfront_private_key_secret
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            load_private_key_secret(secret_name).await?
        } else {
            config
                .storage
                .cloudfront_private_key
                .clone()
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "CLOUDFRONT_PRIVATE_KEY or CLOUDFRONT_PRIVATE_KEY_SECRET is required"
                    )
                })?
        };
        let pem = base64::engine::general_purpose::STANDARD
            .decode(encoded_or_pem.trim())
            .unwrap_or_else(|_| encoded_or_pem.into_bytes());
        let pem = std::str::from_utf8(&pem)?;
        let private_key =
            RsaPrivateKey::from_pkcs8_pem(pem).or_else(|_| RsaPrivateKey::from_pkcs1_pem(pem))?;
        Ok(Some(Self {
            key_pair_id: Arc::from(key_pair_id),
            private_key: Arc::new(private_key),
        }))
    }

    pub(crate) fn signed_url(&self, raw_url: &str, ttl: Duration) -> anyhow::Result<String> {
        let url = Url::parse(raw_url)?;
        let resource = url.to_string();
        let expires =
            chrono::Utc::now().timestamp() + i64::try_from(ttl.as_secs()).unwrap_or(i64::MAX);
        let policy = serde_json::json!({
            "Statement": [{
                "Resource": resource,
                "Condition": {"DateLessThan": {"AWS:EpochTime": expires}}
            }]
        })
        .to_string();
        let digest = Sha1::digest(policy.as_bytes());
        let signature = self
            .private_key
            .sign(Pkcs1v15Sign::new::<Sha1>(), digest.as_ref())?;
        let separator = if resource.contains('?') { '&' } else { '?' };
        Ok(format!(
            "{resource}{separator}Policy={}&Signature={}&Key-Pair-Id={}",
            cloudfront_base64(policy.as_bytes()),
            cloudfront_base64(&signature),
            self.key_pair_id
        ))
    }

    #[cfg(test)]
    pub(crate) fn for_test() -> Self {
        Self {
            key_pair_id: Arc::from("KTEST"),
            private_key: Arc::new(
                RsaPrivateKey::new(&mut rand::thread_rng(), 1024)
                    .expect("test RSA key generation must succeed"),
            ),
        }
    }
}

fn cloudfront_base64(value: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD
        .encode(value)
        .replace('+', "-")
        .replace('=', "_")
        .replace('/', "~")
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct SecretResponse {
    secret_string: Option<String>,
    secret_binary: Option<String>,
}

async fn load_private_key_secret(secret_id: &str) -> anyhow::Result<String> {
    let region = std::env::var("AWS_REGION")
        .or_else(|_| std::env::var("AWS_DEFAULT_REGION"))
        .or_else(|_| std::env::var("S3_REGION"))
        .unwrap_or_else(|_| "us-west-2".to_string());
    let provider = DefaultCredentialsChain::builder()
        .region(Region::new(region.clone()))
        .build()
        .await;
    let credentials = provider.provide_credentials().await?;
    let endpoint = Url::parse(&format!("https://secretsmanager.{region}.amazonaws.com/"))?;
    let body = serde_json::to_vec(&serde_json::json!({"SecretId": secret_id}))?;
    let payload_hash = hex::encode(Sha256::digest(&body));
    let now = chrono::Utc::now();
    let date = now.format("%Y%m%d").to_string();
    let timestamp = now.format("%Y%m%dT%H%M%SZ").to_string();
    let host = endpoint
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("Secrets Manager endpoint has no host"))?;
    let mut canonical_headers =
        format!("content-type:application/x-amz-json-1.1\nhost:{host}\nx-amz-date:{timestamp}\n");
    let mut signed_headers = "content-type;host;x-amz-date".to_string();
    if let Some(token) = credentials.session_token() {
        canonical_headers.push_str(&format!("x-amz-security-token:{}\n", token.trim()));
        signed_headers.push_str(";x-amz-security-token");
    }
    canonical_headers.push_str("x-amz-target:secretsmanager.GetSecretValue\n");
    signed_headers.push_str(";x-amz-target");
    let canonical = format!("POST\n/\n\n{canonical_headers}\n{signed_headers}\n{payload_hash}");
    let scope = format!("{date}/{region}/secretsmanager/aws4_request");
    let to_sign = format!(
        "AWS4-HMAC-SHA256\n{timestamp}\n{scope}\n{}",
        hex::encode(Sha256::digest(canonical.as_bytes()))
    );
    let signature = aws_signature(
        credentials.secret_access_key(),
        &date,
        &region,
        "secretsmanager",
        &to_sign,
    )?;
    let mut request = reqwest::Client::new()
        .post(endpoint)
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-date", &timestamp)
        .header("x-amz-target", "secretsmanager.GetSecretValue")
        .header(
            reqwest::header::AUTHORIZATION,
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
    let secret: SecretResponse = response.json().await?;
    if let Some(value) = secret.secret_string {
        return Ok(value);
    }
    let binary = secret
        .secret_binary
        .ok_or_else(|| anyhow::anyhow!("CloudFront private-key secret is empty"))?;
    let bytes = base64::engine::general_purpose::STANDARD.decode(binary)?;
    Ok(String::from_utf8(bytes)?)
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
