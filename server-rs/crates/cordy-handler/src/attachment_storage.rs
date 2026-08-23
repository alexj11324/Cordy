use async_trait::async_trait;
use axum::body::Body;
use hmac::{Hmac, Mac};
use reqwest::header::{HeaderMap, HeaderValue};
use sha2::{Digest, Sha256};
use std::{
    path::{Component, Path, PathBuf},
    sync::Arc,
};
use tokio::io::AsyncWriteExt;
use url::Url;

type HmacSha256 = Hmac<Sha256>;

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
    fn key_from_url(&self, raw: &str) -> Option<String>;
    fn object_url(&self, key: &str) -> String;
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
        let data = tokio::fs::read(self.path(key)?).await?;
        let meta = tokio::fs::read(format!("{}.meta.json", self.path(key)?.display()))
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
        let total = data.len() as u64;
        if let Some(raw) = range.filter(|raw| !raw.contains(',') && total > 0) {
            let (start, end) =
                parse_range(raw, total).ok_or_else(|| anyhow::anyhow!("invalid range"))?;
            let bytes = data[start as usize..=end as usize].to_vec();
            return Ok(StoredObject {
                content_length: Some(bytes.len() as u64),
                content_range: Some(format!("bytes {start}-{end}/{total}")),
                status: reqwest::StatusCode::PARTIAL_CONTENT,
                body: Body::from(bytes),
                content_type,
                filename,
            });
        }
        Ok(StoredObject {
            body: Body::from(data),
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
        let path = if self.base_url.is_empty() {
            raw
        } else {
            raw.strip_prefix(&self.base_url)?
        };
        let key = path.strip_prefix("/uploads/")?.to_string();
        self.path(&key).is_ok().then_some(key)
    }
    fn object_url(&self, key: &str) -> String {
        format!("{}/uploads/{key}", self.base_url)
    }
    fn is_local(&self) -> bool {
        true
    }
}

#[derive(Clone)]
pub struct S3Storage {
    client: reqwest::Client,
    bucket: String,
    region: String,
    endpoint: Url,
    custom_endpoint: bool,
    path_style: bool,
    access_key: String,
    secret_key: String,
    session_token: Option<String>,
    cdn_domain: Option<String>,
}

impl S3Storage {
    pub fn from_env() -> anyhow::Result<Option<Self>> {
        let Some(bucket) = env("S3_BUCKET") else {
            return Ok(None);
        };
        if bucket.contains("amazonaws.com") {
            tracing::warn!(
                value = bucket,
                "S3_BUCKET looks like an endpoint hostname; configure only the bucket name"
            );
        }
        let region = env("S3_REGION").unwrap_or_else(|| "us-west-2".into());
        let custom = env("AWS_ENDPOINT_URL");
        let endpoint = Url::parse(
            custom
                .as_deref()
                .unwrap_or(&format!("https://s3.{region}.amazonaws.com")),
        )?;
        let path_style = s3_use_path_style(
            std::env::var("S3_USE_PATH_STYLE").ok().as_deref(),
            custom.is_some(),
        );
        let access_key = env("AWS_ACCESS_KEY_ID");
        let secret_key = env("AWS_SECRET_ACCESS_KEY");
        let (access_key, secret_key) = match (access_key, secret_key) {
            (Some(access_key), Some(secret_key)) => (access_key, secret_key),
            (None, None) => anyhow::bail!(
                "S3_BUCKET requires AWS credentials; the Rust server does not yet support the IAM credential chain"
            ),
            _ => anyhow::bail!(
                "AWS_ACCESS_KEY_ID and AWS_SECRET_ACCESS_KEY must be configured together"
            ),
        };
        Ok(Some(Self {
            client: reqwest::Client::new(),
            bucket,
            region,
            endpoint,
            custom_endpoint: custom.is_some(),
            path_style,
            access_key,
            secret_key,
            session_token: env("AWS_SESSION_TOKEN"),
            cdn_domain: env("CLOUDFRONT_DOMAIN"),
        }))
    }
    fn request_url(&self, key: &str) -> anyhow::Result<Url> {
        if key.is_empty() || key.split('/').any(|v| v == "." || v == "..") {
            anyhow::bail!("invalid storage key")
        }
        let mut url = self.endpoint.clone();
        let base_path = url.path().trim_end_matches('/').to_string();
        if self.path_style {
            url.set_path(&format!("{base_path}/{}/{key}", self.bucket));
        } else {
            let host = self
                .endpoint
                .host_str()
                .ok_or_else(|| anyhow::anyhow!("S3 endpoint has no host"))?;
            url.set_host(Some(&format!("{}.{host}", self.bucket)))?;
            url.set_path(&format!("{base_path}/{key}"));
        }
        Ok(url)
    }
    fn signed_headers(
        &self,
        method: &str,
        url: &Url,
        payload_hash: &str,
        now: chrono::DateTime<chrono::Utc>,
        extra: &HeaderMap,
    ) -> anyhow::Result<HeaderMap> {
        let date = now.format("%Y%m%d").to_string();
        let timestamp = now.format("%Y%m%dT%H%M%SZ").to_string();
        let hostname = url
            .host_str()
            .ok_or_else(|| anyhow::anyhow!("S3 endpoint has no host"))?;
        let host = url
            .port()
            .map_or_else(|| hostname.to_string(), |port| format!("{hostname}:{port}"));
        let mut canonical = vec![
            ("host".to_string(), host),
            ("x-amz-content-sha256".to_string(), payload_hash.to_string()),
            ("x-amz-date".to_string(), timestamp.clone()),
        ];
        if let Some(token) = &self.session_token {
            canonical.push(("x-amz-security-token".to_string(), token.trim().to_string()));
        }
        for (name, value) in extra {
            let name = name.as_str();
            if name.starts_with("x-amz-") && !canonical.iter().any(|(existing, _)| existing == name)
            {
                canonical.push((name.to_string(), canonical_header_value(value)?));
            }
        }
        canonical.sort_unstable_by(|left, right| left.0.cmp(&right.0));
        let signed = canonical
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>()
            .join(";");
        let canonical_headers = canonical
            .iter()
            .map(|(name, value)| format!("{name}:{value}\n"))
            .collect::<String>();
        let canonical_request = format!(
            "{method}\n{}\n{}\n{canonical_headers}\n{signed}\n{payload_hash}",
            url.path(),
            url.query().unwrap_or("")
        );
        let scope = format!("{date}/{}/s3/aws4_request", self.region);
        let to_sign = format!(
            "AWS4-HMAC-SHA256\n{timestamp}\n{scope}\n{}",
            hex::encode(Sha256::digest(canonical_request.as_bytes()))
        );
        let k_date = hmac_bytes(
            format!("AWS4{}", self.secret_key).as_bytes(),
            date.as_bytes(),
        )?;
        let k_region = hmac_bytes(&k_date, self.region.as_bytes())?;
        let k_service = hmac_bytes(&k_region, b"s3")?;
        let k_signing = hmac_bytes(&k_service, b"aws4_request")?;
        let signature = hex::encode(hmac_bytes(&k_signing, to_sign.as_bytes())?);
        let mut headers = HeaderMap::new();
        headers.insert("x-amz-content-sha256", HeaderValue::from_str(payload_hash)?);
        headers.insert("x-amz-date", HeaderValue::from_str(&timestamp)?);
        if let Some(token) = &self.session_token {
            headers.insert("x-amz-security-token", HeaderValue::from_str(token)?);
        }
        headers.insert(reqwest::header::AUTHORIZATION, HeaderValue::from_str(&format!("AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed}, Signature={signature}", self.access_key))?);
        Ok(headers)
    }
    async fn execute(
        &self,
        method: reqwest::Method,
        key: &str,
        body: Option<Vec<u8>>,
        extra: HeaderMap,
    ) -> anyhow::Result<reqwest::Response> {
        let url = self.request_url(key)?;
        let bytes = body.unwrap_or_default();
        let hash = hex::encode(Sha256::digest(&bytes));
        let mut headers =
            self.signed_headers(method.as_str(), &url, &hash, chrono::Utc::now(), &extra)?;
        headers.extend(extra);
        let response = self
            .client
            .request(method, url)
            .headers(headers)
            .body(bytes)
            .send()
            .await?;
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
        headers.insert(
            reqwest::header::CACHE_CONTROL,
            HeaderValue::from_static("max-age=432000,public"),
        );
        headers.insert(
            "x-amz-storage-class",
            HeaderValue::from_static(if self.custom_endpoint {
                "STANDARD"
            } else {
                "INTELLIGENT_TIERING"
            }),
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
        let response = self
            .execute(reqwest::Method::GET, key, None, headers)
            .await?;
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
    fn key_from_url(&self, raw: &str) -> Option<String> {
        let url = Url::parse(raw).ok()?;
        let decoded = url
            .path_segments()?
            .map(|part| {
                percent_encoding::percent_decode_str(part)
                    .decode_utf8()
                    .ok()
                    .map(|v| v.into_owned())
            })
            .collect::<Option<Vec<_>>>()?
            .join("/");
        let mut path = decoded.trim_start_matches('/');
        if self.path_style && path.starts_with(&format!("{}/", self.bucket)) {
            path = &path[self.bucket.len() + 1..];
        }
        (!path.is_empty() && !path.split('/').any(|p| p == "." || p == ".."))
            .then(|| path.to_string())
    }
    fn object_url(&self, key: &str) -> String {
        if let Some(domain) = &self.cdn_domain {
            format!("https://{}/{key}", domain.trim_end_matches('/'))
        } else if !self.custom_endpoint && self.bucket.contains('.') {
            let Ok(mut url) = Url::parse(&format!("https://s3.{}.amazonaws.com", self.region))
            else {
                return String::new();
            };
            url.set_path(&format!("/{}/{key}", self.bucket));
            url.to_string()
        } else {
            self.request_url(key)
                .map(|u| u.to_string())
                .unwrap_or_default()
        }
    }
}

pub fn from_env(
    local_dir: Option<&str>,
    local_base: Option<&str>,
) -> anyhow::Result<Arc<dyn AttachmentStorage>> {
    if let Some(s3) = S3Storage::from_env()? {
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
fn s3_use_path_style(raw: Option<&str>, endpoint_configured: bool) -> bool {
    let default = endpoint_configured;
    let Some(raw) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
        return default;
    };
    match raw.to_ascii_lowercase().as_str() {
        "1" | "t" | "true" | "y" | "yes" | "on" => true,
        "0" | "f" | "false" | "n" | "no" | "off" => false,
        _ => {
            tracing::warn!(value = raw, default, "invalid S3_USE_PATH_STYLE value");
            default
        }
    }
}
fn canonical_header_value(value: &HeaderValue) -> anyhow::Result<String> {
    Ok(value
        .to_str()?
        .split_ascii_whitespace()
        .collect::<Vec<_>>()
        .join(" "))
}
fn hmac_bytes(key: &[u8], body: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut mac =
        HmacSha256::new_from_slice(key).map_err(|_| anyhow::anyhow!("invalid HMAC key"))?;
    mac.update(body);
    Ok(mac.finalize().into_bytes().to_vec())
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
            custom_endpoint: false,
            path_style: false,
            access_key: "key".into(),
            secret_key: "secret".into(),
            session_token: None,
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

    fn test_s3(endpoint: &str, custom_endpoint: bool, path_style: bool) -> S3Storage {
        S3Storage {
            client: reqwest::Client::new(),
            bucket: "bucket".into(),
            region: "us-west-2".into(),
            endpoint: Url::parse(endpoint).unwrap(),
            custom_endpoint,
            path_style,
            access_key: "key".into(),
            secret_key: "secret".into(),
            session_token: None,
            cdn_domain: None,
        }
    }

    #[test]
    fn s3_path_style_boolean_matches_go() {
        for value in ["1", "t", "true", "y", "yes", "on", " TRUE "] {
            assert!(s3_use_path_style(Some(value), false), "{value}");
        }
        for value in ["0", "f", "false", "n", "no", "off", " FALSE "] {
            assert!(!s3_use_path_style(Some(value), true), "{value}");
        }
        assert!(!s3_use_path_style(None, false));
        assert!(s3_use_path_style(None, true));
        assert!(s3_use_path_style(Some("invalid"), true));
        assert!(!s3_use_path_style(Some("invalid"), false));
    }

    #[test]
    fn custom_endpoint_keeps_base_path_for_both_addressing_styles() {
        let path = test_s3("https://objects.example.test/base", true, true)
            .request_url("workspaces/a file.txt")
            .unwrap();
        assert_eq!(
            path.as_str(),
            "https://objects.example.test/base/bucket/workspaces/a%20file.txt"
        );

        let virtual_host = test_s3("https://objects.example.test/base", true, false)
            .request_url("workspaces/a file.txt")
            .unwrap();
        assert_eq!(
            virtual_host.as_str(),
            "https://bucket.objects.example.test/base/workspaces/a%20file.txt"
        );
    }

    #[test]
    fn sigv4_signs_amz_extension_headers() {
        let store = test_s3("https://s3.us-west-2.amazonaws.com", false, true);
        let url = store.request_url("object").unwrap();
        let mut extra = HeaderMap::new();
        extra.insert(
            "x-amz-storage-class",
            HeaderValue::from_static("INTELLIGENT_TIERING"),
        );
        let headers = store
            .signed_headers(
                "PUT",
                &url,
                &hex::encode(Sha256::digest(b"body")),
                chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
                &extra,
            )
            .unwrap();
        let authorization = headers
            .get(reqwest::header::AUTHORIZATION)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(authorization
            .contains("SignedHeaders=host;x-amz-content-sha256;x-amz-date;x-amz-storage-class"));
    }

    #[test]
    fn dotted_aws_bucket_uses_tls_safe_public_url() {
        let mut store = test_s3("https://s3.us-west-2.amazonaws.com", false, false);
        store.bucket = "assets.example.com".into();
        assert_eq!(
            store.object_url("workspaces/a file.txt"),
            "https://s3.us-west-2.amazonaws.com/assets.example.com/workspaces/a%20file.txt"
        );
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
