use async_trait::async_trait;
use aws_config::default_provider::credentials::DefaultCredentialsChain;
use aws_credential_types::provider::{ProvideCredentials, SharedCredentialsProvider};
use aws_types::region::Region;
use axum::body::Body;
use hmac::{Hmac, Mac};
use reqwest::header::{HeaderMap, HeaderValue};
use sha2::{Digest, Sha256};
use std::{
    path::{Component, Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio_util::io::ReaderStream;
use url::Url;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, thiserror::Error)]
pub enum StorageGetError {
    #[error("attachment object was not found")]
    NotFound,
    #[error("requested range is not satisfiable")]
    InvalidRange { total: Option<u64> },
}

#[async_trait]
pub trait AttachmentStorage: Send + Sync {
    async fn upload(
        &self,
        key: &str,
        body: Vec<u8>,
        content_type: &str,
        filename: &str,
    ) -> anyhow::Result<String>;
    async fn get(&self, key: &str, range: Option<&str>) -> anyhow::Result<StoredObject>;
    async fn delete(&self, key: &str) -> anyhow::Result<()>;
    async fn presign_get(
        &self,
        _key: &str,
        _ttl: Duration,
        _content_disposition: Option<&str>,
    ) -> anyhow::Result<Option<String>> {
        Ok(None)
    }
    fn key_from_url(&self, raw: &str) -> Option<String>;
    fn object_url(&self, key: &str) -> String;
    fn has_public_base_url(&self) -> bool {
        false
    }
    fn supports_presign(&self) -> bool {
        false
    }
    fn is_local(&self) -> bool {
        false
    }
}

pub struct StoredObject {
    pub body: Body,
    pub content_length: Option<u64>,
    pub content_range: Option<String>,
    pub status: reqwest::StatusCode,
    pub content_type: Option<String>,
    pub filename: Option<String>,
}

#[derive(Clone)]
pub struct LocalStorage {
    root: Arc<PathBuf>,
    base_url: String,
}

impl LocalStorage {
    pub fn new(root: PathBuf, base_url: String) -> anyhow::Result<Self> {
        std::fs::create_dir_all(&root)?;
        Ok(Self {
            root: Arc::new(root.canonicalize()?),
            base_url: base_url.trim_end_matches('/').into(),
        })
    }
    fn path(&self, key: &str) -> anyhow::Result<PathBuf> {
        let relative = Path::new(key);
        if key.is_empty()
            || relative.is_absolute()
            || relative
                .components()
                .any(|part| !matches!(part, Component::Normal(_)))
            || key.ends_with(".meta.json")
            || key
                .split('/')
                .any(|part| part.starts_with('.') && part.ends_with(".tmp"))
        {
            anyhow::bail!("invalid storage key")
        }
        Ok(self.root.join(relative))
    }
}

#[async_trait]
impl AttachmentStorage for LocalStorage {
    async fn upload(
        &self,
        key: &str,
        body: Vec<u8>,
        content_type: &str,
        filename: &str,
    ) -> anyhow::Result<String> {
        let path = self.path(key)?;
        let parent = path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("invalid storage key"))?;
        tokio::fs::create_dir_all(parent).await?;
        let name = path
            .file_name()
            .and_then(|v| v.to_str())
            .ok_or_else(|| anyhow::anyhow!("invalid storage key"))?;
        let tmp = parent.join(format!(".{name}.tmp"));
        let mut file = tokio::fs::File::create(&tmp).await?;
        file.write_all(&body).await?;
        file.flush().await?;
        drop(file);
        tokio::fs::rename(&tmp, &path).await?;
        let meta = serde_json::json!({"filename": filename, "content_type": content_type});
        if let Err(error) = tokio::fs::write(
            format!("{}.meta.json", path.display()),
            serde_json::to_vec(&meta)?,
        )
        .await
        {
            tracing::warn!(%error, %key, "local upload metadata write failed");
        }
        Ok(self.object_url(key))
    }
    async fn get(&self, key: &str, range: Option<&str>) -> anyhow::Result<StoredObject> {
        let path = self.path(key)?;
        let mut file = tokio::fs::File::open(&path).await.map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                anyhow::Error::new(StorageGetError::NotFound)
            } else {
                anyhow::Error::from(error)
            }
        })?;
        let total = file.metadata().await?.len();
        let meta = tokio::fs::read(format!("{}.meta.json", path.display()))
            .await
            .ok()
            .and_then(|v| serde_json::from_slice::<serde_json::Value>(&v).ok());
        let content_type = meta
            .as_ref()
            .and_then(|v| v.get("content_type"))
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let filename = meta
            .as_ref()
            .and_then(|v| v.get("filename"))
            .and_then(|v| v.as_str())
            .map(str::to_string);
        if let Some(raw) = range.filter(|raw| !raw.contains(',') && total > 0) {
            let (start, end) = parse_range(raw, total).ok_or_else(|| {
                anyhow::Error::new(StorageGetError::InvalidRange { total: Some(total) })
            })?;
            file.seek(std::io::SeekFrom::Start(start)).await?;
            let length = end - start + 1;
            return Ok(StoredObject {
                content_length: Some(length),
                content_range: Some(format!("bytes {start}-{end}/{total}")),
                status: reqwest::StatusCode::PARTIAL_CONTENT,
                body: Body::from_stream(ReaderStream::new(file.take(length))),
                content_type,
                filename,
            });
        }
        Ok(StoredObject {
            body: Body::from_stream(ReaderStream::new(file)),
            content_length: Some(total),
            content_range: None,
            status: reqwest::StatusCode::OK,
            content_type,
            filename,
        })
    }
    async fn delete(&self, key: &str) -> anyhow::Result<()> {
        let path = self.path(key)?;
        let _ = tokio::fs::remove_file(format!("{}.meta.json", path.display())).await;
        match tokio::fs::remove_file(path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
    fn key_from_url(&self, raw: &str) -> Option<String> {
        let path = Url::parse(raw)
            .ok()
            .map(|url| url.path().to_string())
            .unwrap_or_else(|| raw.split(['?', '#']).next().unwrap_or(raw).to_string());
        let marker = "/uploads/";
        let index = path.find(marker)?;
        let key = percent_encoding::percent_decode_str(&path[index + marker.len()..])
            .decode_utf8()
            .ok()?
            .into_owned();
        self.path(&key).is_ok().then_some(key)
    }
    fn object_url(&self, key: &str) -> String {
        format!("{}/uploads/{key}", self.base_url)
    }
    fn is_local(&self) -> bool {
        true
    }
    fn has_public_base_url(&self) -> bool {
        !self.base_url.is_empty()
    }
}

#[derive(Clone)]
pub struct S3Storage {
    client: reqwest::Client,
    bucket: String,
    region: String,
    endpoint: Url,
    path_style: bool,
    credentials: SharedCredentialsProvider,
    cdn_domain: Option<String>,
}

impl S3Storage {
    pub async fn from_env(cloudfront_domain: Option<&str>) -> anyhow::Result<Option<Self>> {
        let Some(bucket) = env("S3_BUCKET") else {
            return Ok(None);
        };
        let region = env("S3_REGION").unwrap_or_else(|| "us-west-2".into());
        let custom = env("AWS_ENDPOINT_URL");
        let endpoint = Url::parse(
            custom
                .as_deref()
                .unwrap_or(&format!("https://s3.{region}.amazonaws.com")),
        )?;
        let configured_path_style = env("S3_USE_PATH_STYLE")
            .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"));
        let path_style = use_path_style(&bucket, custom.is_some(), configured_path_style);
        let credentials = SharedCredentialsProvider::new(
            DefaultCredentialsChain::builder()
                .region(Region::new(region.clone()))
                .build()
                .await,
        );
        Ok(Some(Self {
            client: reqwest::Client::new(),
            bucket,
            region,
            endpoint,
            path_style,
            credentials,
            // Keep object URLs and the CloudFront signer on the same loaded
            // configuration source; config-file deployments must not split.
            cdn_domain: cloudfront_domain
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string),
        }))
    }
    fn request_url(&self, key: &str) -> anyhow::Result<Url> {
        if key.is_empty() || key.split('/').any(|v| v == "." || v == "..") {
            anyhow::bail!("invalid storage key")
        }
        let mut url = self.endpoint.clone();
        if !self.path_style {
            let host = self
                .endpoint
                .host_str()
                .ok_or_else(|| anyhow::anyhow!("S3 endpoint has no host"))?;
            url.set_host(Some(&format!("{}.{host}", self.bucket)))?;
        }
        {
            let mut segments = url
                .path_segments_mut()
                .map_err(|_| anyhow::anyhow!("S3 endpoint cannot be a base URL"))?;
            // Custom S3 gateways may be mounted below a path prefix. Retain
            // those configured segments and append the bucket/key beneath it.
            segments.pop_if_empty();
            if self.path_style {
                segments.push(&self.bucket);
            }
            segments.extend(key.split('/'));
        }
        Ok(url)
    }
    fn signed_headers(
        &self,
        method: &str,
        url: &Url,
        payload_hash: &str,
        now: chrono::DateTime<chrono::Utc>,
        credentials: &aws_credential_types::Credentials,
    ) -> anyhow::Result<HeaderMap> {
        let date = now.format("%Y%m%d").to_string();
        let timestamp = now.format("%Y%m%dT%H%M%SZ").to_string();
        let hostname = url
            .host_str()
            .ok_or_else(|| anyhow::anyhow!("S3 endpoint has no host"))?;
        let host = url
            .port()
            .map_or_else(|| hostname.to_string(), |port| format!("{hostname}:{port}"));
        let mut canonical_headers =
            format!("host:{host}\nx-amz-content-sha256:{payload_hash}\nx-amz-date:{timestamp}\n");
        let mut signed = "host;x-amz-content-sha256;x-amz-date".to_string();
        if let Some(token) = credentials.session_token() {
            canonical_headers.push_str(&format!("x-amz-security-token:{}\n", token.trim()));
            signed.push_str(";x-amz-security-token");
        }
        let canonical = format!(
            "{method}\n{}\n{}\n{canonical_headers}\n{signed}\n{payload_hash}",
            url.path(),
            url.query().unwrap_or("")
        );
        let scope = format!("{date}/{}/s3/aws4_request", self.region);
        let to_sign = format!(
            "AWS4-HMAC-SHA256\n{timestamp}\n{scope}\n{}",
            hex::encode(Sha256::digest(canonical.as_bytes()))
        );
        let k_date = hmac_bytes(
            format!("AWS4{}", credentials.secret_access_key()).as_bytes(),
            date.as_bytes(),
        )?;
        let k_region = hmac_bytes(&k_date, self.region.as_bytes())?;
        let k_service = hmac_bytes(&k_region, b"s3")?;
        let k_signing = hmac_bytes(&k_service, b"aws4_request")?;
        let signature = hex::encode(hmac_bytes(&k_signing, to_sign.as_bytes())?);
        let mut headers = HeaderMap::new();
        headers.insert("x-amz-content-sha256", HeaderValue::from_str(payload_hash)?);
        headers.insert("x-amz-date", HeaderValue::from_str(&timestamp)?);
        if let Some(token) = credentials.session_token() {
            headers.insert("x-amz-security-token", HeaderValue::from_str(token)?);
        }
        headers.insert(reqwest::header::AUTHORIZATION, HeaderValue::from_str(&format!("AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed}, Signature={signature}", credentials.access_key_id()))?);
        Ok(headers)
    }
    async fn send(
        &self,
        method: reqwest::Method,
        key: &str,
        body: Option<Vec<u8>>,
        extra: HeaderMap,
    ) -> anyhow::Result<reqwest::Response> {
        let url = self.request_url(key)?;
        let bytes = body.unwrap_or_default();
        let hash = hex::encode(Sha256::digest(&bytes));
        let credentials = self.credentials.provide_credentials().await?;
        let mut headers = self.signed_headers(
            method.as_str(),
            &url,
            &hash,
            chrono::Utc::now(),
            &credentials,
        )?;
        headers.extend(extra);
        self.client
            .request(method, url)
            .headers(headers)
            .body(bytes)
            .send()
            .await
            .map_err(Into::into)
    }
    async fn execute(
        &self,
        method: reqwest::Method,
        key: &str,
        body: Option<Vec<u8>>,
        extra: HeaderMap,
    ) -> anyhow::Result<reqwest::Response> {
        let response = self.send(method, key, body, extra).await?;
        if !response.status().is_success() {
            anyhow::bail!("object storage returned {}", response.status())
        }
        Ok(response)
    }
}

#[async_trait]
impl AttachmentStorage for S3Storage {
    async fn upload(
        &self,
        key: &str,
        body: Vec<u8>,
        content_type: &str,
        filename: &str,
    ) -> anyhow::Result<String> {
        let mut headers = HeaderMap::new();
        headers.insert(
            reqwest::header::CONTENT_TYPE,
            HeaderValue::from_str(content_type)?,
        );
        headers.insert(
            reqwest::header::CONTENT_DISPOSITION,
            HeaderValue::from_str(&content_disposition(content_type, filename, false))?,
        );
        self.execute(reqwest::Method::PUT, key, Some(body), headers)
            .await?;
        Ok(self.object_url(key))
    }
    async fn get(&self, key: &str, range: Option<&str>) -> anyhow::Result<StoredObject> {
        let mut headers = HeaderMap::new();
        if let Some(range) = range {
            headers.insert(reqwest::header::RANGE, HeaderValue::from_str(range)?);
        }
        let response = self.send(reqwest::Method::GET, key, None, headers).await?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(StorageGetError::NotFound.into());
        }
        if response.status() == reqwest::StatusCode::RANGE_NOT_SATISFIABLE {
            let total = response
                .headers()
                .get(reqwest::header::CONTENT_RANGE)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.strip_prefix("bytes */"))
                .and_then(|value| value.parse().ok());
            return Err(StorageGetError::InvalidRange { total }.into());
        }
        if !response.status().is_success() {
            anyhow::bail!("object storage returned {}", response.status())
        }
        let status = response.status();
        let content_length = response.content_length();
        let content_range = response
            .headers()
            .get(reqwest::header::CONTENT_RANGE)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        Ok(StoredObject {
            body: Body::from_stream(response.bytes_stream()),
            content_length,
            content_range,
            status,
            content_type,
            filename: None,
        })
    }
    async fn delete(&self, key: &str) -> anyhow::Result<()> {
        self.execute(reqwest::Method::DELETE, key, None, HeaderMap::new())
            .await
            .map(|_| ())
    }
    async fn presign_get(
        &self,
        key: &str,
        ttl: Duration,
        content_disposition: Option<&str>,
    ) -> anyhow::Result<Option<String>> {
        let credentials = self.credentials.provide_credentials().await?;
        let mut url = self.request_url(key)?;
        let now = chrono::Utc::now();
        let date = now.format("%Y%m%d").to_string();
        let timestamp = now.format("%Y%m%dT%H%M%SZ").to_string();
        let scope = format!("{date}/{}/s3/aws4_request", self.region);
        let expires = ttl.as_secs().clamp(1, 604_800);
        let hostname = url
            .host_str()
            .ok_or_else(|| anyhow::anyhow!("S3 endpoint has no host"))?;
        let host = url
            .port()
            .map_or_else(|| hostname.to_string(), |port| format!("{hostname}:{port}"));
        let mut query = vec![
            ("X-Amz-Algorithm", "AWS4-HMAC-SHA256".to_string()),
            (
                "X-Amz-Credential",
                format!("{}/{scope}", credentials.access_key_id()),
            ),
            ("X-Amz-Date", timestamp.clone()),
            ("X-Amz-Expires", expires.to_string()),
            ("X-Amz-SignedHeaders", "host".to_string()),
        ];
        if let Some(token) = credentials.session_token() {
            query.push(("X-Amz-Security-Token", token.to_string()));
        }
        if let Some(value) = content_disposition.filter(|value| !value.is_empty()) {
            query.push(("response-content-disposition", value.to_string()));
        }
        let canonical_query = canonical_query(query);
        let canonical = format!(
            "GET\n{}\n{canonical_query}\nhost:{host}\n\nhost\nUNSIGNED-PAYLOAD",
            url.path()
        );
        let to_sign = format!(
            "AWS4-HMAC-SHA256\n{timestamp}\n{scope}\n{}",
            hex::encode(Sha256::digest(canonical.as_bytes()))
        );
        let signature = aws_signature(
            credentials.secret_access_key(),
            &date,
            &self.region,
            "s3",
            &to_sign,
        )?;
        url.set_query(Some(&format!(
            "{canonical_query}&X-Amz-Signature={signature}"
        )));
        Ok(Some(url.to_string()))
    }
    fn key_from_url(&self, raw: &str) -> Option<String> {
        let url = Url::parse(raw).ok()?;
        let mut segments = url
            .path_segments()?
            .map(|part| {
                percent_encoding::percent_decode_str(part)
                    .decode_utf8()
                    .ok()
                    .map(|v| v.into_owned())
            })
            .collect::<Option<Vec<_>>>()?;
        let endpoint_host = self.endpoint.host_str()?;
        let raw_host = url.host_str()?;
        let endpoint_origin = raw_host.eq_ignore_ascii_case(endpoint_host);
        let virtual_origin = raw_host
            .strip_suffix(endpoint_host)
            .is_some_and(|prefix| prefix.trim_end_matches('.') == self.bucket);
        if endpoint_origin || virtual_origin {
            let endpoint_segments = self
                .endpoint
                .path_segments()?
                .filter(|part| !part.is_empty())
                .map(|part| {
                    percent_encoding::percent_decode_str(part)
                        .decode_utf8()
                        .ok()
                        .map(|value| value.into_owned())
                })
                .collect::<Option<Vec<_>>>()?;
            if segments.starts_with(&endpoint_segments) {
                segments.drain(..endpoint_segments.len());
            }
        }
        // A persisted path-style URL remains readable after switching the
        // current client to virtual-hosted addressing.
        if endpoint_origin && segments.first().is_some_and(|part| part == &self.bucket) {
            segments.remove(0);
        }
        let path = segments.join("/");
        (!path.is_empty() && !path.split('/').any(|p| p == "." || p == "..")).then_some(path)
    }
    fn object_url(&self, key: &str) -> String {
        if let Some(domain) = &self.cdn_domain {
            format!("https://{}/{key}", domain.trim_end_matches('/'))
        } else {
            self.request_url(key)
                .map(|u| u.to_string())
                .unwrap_or_default()
        }
    }
    fn has_public_base_url(&self) -> bool {
        self.cdn_domain.is_some()
    }
    fn supports_presign(&self) -> bool {
        true
    }
}

pub async fn from_env(
    local_dir: Option<&str>,
    local_base: Option<&str>,
    cloudfront_domain: Option<&str>,
) -> anyhow::Result<Arc<dyn AttachmentStorage>> {
    if let Some(s3) = S3Storage::from_env(cloudfront_domain).await? {
        return Ok(Arc::new(s3));
    }
    Ok(Arc::new(LocalStorage::new(
        PathBuf::from(local_dir.unwrap_or("./data/uploads")),
        local_base.unwrap_or("").to_string(),
    )?))
}
fn env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}
fn use_path_style(bucket: &str, custom_endpoint: bool, configured: Option<bool>) -> bool {
    if !custom_endpoint && bucket.contains('.') {
        true
    } else {
        configured.unwrap_or(custom_endpoint)
    }
}
fn hmac_bytes(key: &[u8], body: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut mac =
        HmacSha256::new_from_slice(key).map_err(|_| anyhow::anyhow!("invalid HMAC key"))?;
    mac.update(body);
    Ok(mac.finalize().into_bytes().to_vec())
}
fn aws_signature(
    secret: &str,
    date: &str,
    region: &str,
    service: &str,
    to_sign: &str,
) -> anyhow::Result<String> {
    let k_date = hmac_bytes(format!("AWS4{secret}").as_bytes(), date.as_bytes())?;
    let k_region = hmac_bytes(&k_date, region.as_bytes())?;
    let k_service = hmac_bytes(&k_region, service.as_bytes())?;
    let k_signing = hmac_bytes(&k_service, b"aws4_request")?;
    Ok(hex::encode(hmac_bytes(&k_signing, to_sign.as_bytes())?))
}
fn canonical_query(values: Vec<(&str, String)>) -> String {
    let mut encoded = values
        .into_iter()
        .map(|(key, value)| (aws_encode(key), aws_encode(&value)))
        .collect::<Vec<_>>();
    encoded.sort();
    encoded
        .into_iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("&")
}
fn aws_encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}
pub fn content_disposition(content_type: &str, filename: &str, force: bool) -> String {
    let media_type = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let kind = if !force
        && media_type != "image/svg+xml"
        && (media_type.starts_with("image/")
            || media_type.starts_with("video/")
            || media_type.starts_with("audio/")
            || media_type == "application/pdf")
    {
        "inline"
    } else {
        "attachment"
    };
    let ascii: String = filename
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | ' ') {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!(
        "{kind}; filename=\"{}\"; filename*=UTF-8''{}",
        ascii.replace(['\r', '\n', '\"'], "_"),
        aws_encode(filename)
    )
}
fn parse_range(raw: &str, total: u64) -> Option<(u64, u64)> {
    let value = raw.strip_prefix("bytes=")?;
    if value.contains(',') || total == 0 {
        return None;
    }
    let (start, end) = value.split_once('-')?;
    if start.is_empty() {
        let count: u64 = end.parse().ok()?;
        if count == 0 {
            return None;
        }
        return Some((total.saturating_sub(count), total - 1));
    }
    let start: u64 = start.parse().ok()?;
    if start >= total {
        return None;
    }
    let end = if end.is_empty() {
        total - 1
    } else {
        end.parse::<u64>().ok()?.min(total - 1)
    };
    (end >= start).then_some((start, end))
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;

    #[tokio::test]
    async fn local_round_trip_preserves_metadata_and_range() {
        let dir = tempfile::tempdir().unwrap();
        let store = LocalStorage::new(dir.path().to_path_buf(), String::new()).unwrap();
        let url = store
            .upload(
                "workspaces/w/file.txt",
                b"abcdef".to_vec(),
                "text/plain",
                "报告.txt",
            )
            .await
            .unwrap();
        assert_eq!(url, "/uploads/workspaces/w/file.txt");
        let object = store
            .get("workspaces/w/file.txt", Some("bytes=2-4"))
            .await
            .unwrap();
        assert_eq!(object.status, reqwest::StatusCode::PARTIAL_CONTENT);
        assert_eq!(object.content_range.as_deref(), Some("bytes 2-4/6"));
        assert_eq!(object.filename.as_deref(), Some("报告.txt"));
        assert_eq!(object.body.collect().await.unwrap().to_bytes(), &b"cde"[..]);
    }

    #[tokio::test]
    async fn local_rejects_traversal_and_internal_files() {
        let dir = tempfile::tempdir().unwrap();
        let store = LocalStorage::new(dir.path().to_path_buf(), String::new()).unwrap();
        assert!(store.get("../secret", None).await.is_err());
        assert!(store.get("x.meta.json", None).await.is_err());
        assert!(store.get(".x.tmp", None).await.is_err());
    }

    #[test]
    fn s3_unicode_key_round_trips_through_stored_url() {
        let store = S3Storage {
            client: reqwest::Client::new(),
            bucket: "bucket".into(),
            region: "us-west-2".into(),
            endpoint: Url::parse("https://s3.us-west-2.amazonaws.com").unwrap(),
            path_style: false,
            credentials: SharedCredentialsProvider::new(aws_credential_types::Credentials::new(
                "key",
                "secret",
                None,
                None,
                "attachment-test",
            )),
            cdn_domain: None,
        };
        let key = "workspaces/w/file.微信";
        let url = store.object_url(key);
        assert_eq!(store.key_from_url(&url).as_deref(), Some(key));
        assert!(!store
            .request_url(store.key_from_url(&url).as_deref().unwrap())
            .unwrap()
            .as_str()
            .contains("%25"));
    }

    #[test]
    fn custom_endpoint_path_is_preserved_for_requests_and_key_recovery() {
        let store = test_s3_store("https://gateway.example/s3/", true);
        let key = "workspaces/w/file.txt";
        let url = store.request_url(key).unwrap();
        assert_eq!(
            url.as_str(),
            "https://gateway.example/s3/bucket/workspaces/w/file.txt"
        );
        assert_eq!(store.key_from_url(url.as_str()).as_deref(), Some(key));
    }

    #[test]
    fn persisted_path_style_url_survives_virtual_hosted_switch() {
        let store = test_s3_store("https://s3.us-west-2.amazonaws.com", false);
        assert_eq!(
            store
                .key_from_url("https://s3.us-west-2.amazonaws.com/bucket/workspaces/w/file.txt")
                .as_deref(),
            Some("workspaces/w/file.txt")
        );
    }

    #[test]
    fn local_key_recovery_does_not_depend_on_current_public_origin() {
        let dir = tempfile::tempdir().unwrap();
        let store =
            LocalStorage::new(dir.path().to_path_buf(), "https://new.example".to_string()).unwrap();
        assert_eq!(
            store
                .key_from_url("https://old.example/uploads/workspaces/w/file.txt")
                .as_deref(),
            Some("workspaces/w/file.txt")
        );
    }

    #[test]
    fn dotted_aws_buckets_force_path_style_for_tls() {
        assert!(use_path_style("my.bucket", false, None));
        assert!(use_path_style("my.bucket", false, Some(false)));
        assert!(!use_path_style("my-bucket", false, None));
        assert!(!use_path_style("my.bucket", true, Some(false)));
    }

    fn test_s3_store(endpoint: &str, path_style: bool) -> S3Storage {
        S3Storage {
            client: reqwest::Client::new(),
            bucket: "bucket".into(),
            region: "us-west-2".into(),
            endpoint: Url::parse(endpoint).unwrap(),
            path_style,
            credentials: SharedCredentialsProvider::new(aws_credential_types::Credentials::new(
                "key",
                "secret",
                None,
                None,
                "attachment-test",
            )),
            cdn_domain: None,
        }
    }

    #[test]
    fn svg_is_never_inline_regardless_of_mime_spelling() {
        for content_type in [
            "image/svg+xml",
            "image/svg+xml; charset=utf-8",
            " IMAGE/SVG+XML ",
        ] {
            assert!(content_disposition(content_type, "x.svg", false).starts_with("attachment;"));
        }
        assert!(content_disposition("image/png", "x.png", false).starts_with("inline;"));
    }
}
