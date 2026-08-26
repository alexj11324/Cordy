use async_trait::async_trait;
use aws_config::{default_provider::credentials::DefaultCredentialsChain, Region};
use aws_credential_types::{
    provider::{ProvideCredentials, SharedCredentialsProvider},
    Credentials,
};
use axum::body::Body;
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use futures_util::StreamExt;
use hmac::{Hmac, Mac};
use reqwest::header::{HeaderMap, HeaderValue};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fmt,
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio_util::io::ReaderStream;
use url::Url;

type HmacSha256 = Hmac<Sha256>;
pub const MAX_STREAM_UPLOAD_BYTES: i64 = 100 << 20;
const STREAMING_UNSIGNED_PAYLOAD_TRAILER: &str = "STREAMING-UNSIGNED-PAYLOAD-TRAILER";
const UNSIGNED_PAYLOAD: &str = "UNSIGNED-PAYLOAD";
const CRC32_TRAILER: &str = "x-amz-checksum-crc32";
const S3_MAX_ATTEMPTS: usize = 3;
const S3_MAX_ERROR_BODY: usize = 64 << 10;
const S3_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const S3_READ_TIMEOUT: Duration = Duration::from_secs(60);
const S3_MAX_RETRY_DELAY: Duration = Duration::from_secs(20);
const S3_CORRECTION_TTL: Duration = Duration::from_secs(15 * 60);
const S3_MAX_CLOCK_SKEW_SECONDS: i64 = 24 * 60 * 60;
const S3_MIN_PRESIGN_TTL: Duration = Duration::from_secs(1);
const S3_MAX_PRESIGN_TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);

pub(crate) fn validate_s3_presign_ttl(ttl: Duration) -> anyhow::Result<()> {
    anyhow::ensure!(
        (S3_MIN_PRESIGN_TTL..=S3_MAX_PRESIGN_TTL).contains(&ttl),
        "S3 presigned download TTL must be between 1 second and 7 days"
    );
    Ok(())
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
    async fn upload_stream(
        &self,
        _key: &str,
        _body: Box<dyn AsyncRead + Send + Unpin>,
        _size_bytes: i64,
        _content_type: &str,
        _filename: &str,
    ) -> anyhow::Result<String> {
        anyhow::bail!("attachment storage does not support streaming uploads")
    }
    fn supports_streaming_uploads(&self) -> bool {
        false
    }
    async fn presign_get(&self, _key: &str, _ttl: std::time::Duration) -> anyhow::Result<String> {
        anyhow::bail!("attachment storage does not support presigned downloads")
    }
    async fn presign_get_with_content_disposition(
        &self,
        _key: &str,
        _ttl: std::time::Duration,
        _content_disposition: &str,
    ) -> anyhow::Result<String> {
        anyhow::bail!("attachment storage does not support presigned downloads")
    }
    fn supports_presigned_downloads(&self) -> bool {
        false
    }
    fn key_from_url(&self, raw: &str) -> Option<String>;
    fn object_url(&self, key: &str) -> String;
    fn is_local(&self) -> bool {
        false
    }
}

/// Narrow adapter used by the channel-media reconciler. It delegates to the
/// exact `AttachmentStorage` instance owned by `HandlerState`, so uploads,
/// reads, and cleanup cannot drift onto differently configured backends.
pub struct AttachmentMediaDeleter {
    storage: Arc<dyn AttachmentStorage>,
}

impl AttachmentMediaDeleter {
    pub fn new(storage: Arc<dyn AttachmentStorage>) -> Self {
        Self { storage }
    }
}

#[async_trait]
impl cordy_service::channel_media_reconciler::MediaObjectDeleter for AttachmentMediaDeleter {
    async fn delete_object(&self, key: &str) -> anyhow::Result<()> {
        self.storage.delete(key).await
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
        let mut tmp_guard = LocalTempGuard::new(tmp.clone());
        let mut file = tokio::fs::File::create(&tmp).await?;
        file.write_all(&body).await?;
        file.flush().await?;
        drop(file);
        tokio::fs::rename(&tmp, &path).await?;
        tmp_guard.disarm();
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
    async fn upload_stream(
        &self,
        key: &str,
        body: Box<dyn AsyncRead + Send + Unpin>,
        size_bytes: i64,
        content_type: &str,
        filename: &str,
    ) -> anyhow::Result<String> {
        validate_stream_size(size_bytes)?;
        let path = self.path(key)?;
        let parent = path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("invalid storage key"))?;
        tokio::fs::create_dir_all(parent).await?;
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| anyhow::anyhow!("invalid storage key"))?;
        let tmp = parent.join(format!(".{name}.tmp"));
        let mut tmp_guard = LocalTempGuard::new(tmp.clone());
        let mut file = tokio::fs::File::create(&tmp).await?;
        let mut limited = body.take(size_bytes as u64 + 1);
        let copied = tokio::io::copy(&mut limited, &mut file).await?;
        anyhow::ensure!(
            copied == size_bytes as u64,
            "stream length does not match declared size"
        );
        file.flush().await?;
        drop(file);
        tokio::fs::rename(&tmp, &path).await?;
        tmp_guard.disarm();
        write_local_metadata(&path, key, content_type, filename).await;
        Ok(self.object_url(key))
    }
    fn supports_streaming_uploads(&self) -> bool {
        true
    }
    async fn get(&self, key: &str, range: Option<&str>) -> anyhow::Result<StoredObject> {
        let path = self.path(key)?;
        let mut file = tokio::fs::File::open(&path).await?;
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
            let (start, end) =
                parse_range(raw, total).ok_or_else(|| anyhow::anyhow!("invalid range"))?;
            let length = end - start + 1;
            file.seek(std::io::SeekFrom::Start(start)).await?;
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
        if let Some(tmp) = local_temp_path(&path) {
            let _ = tokio::fs::remove_file(tmp).await;
        }
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

async fn write_local_metadata(path: &Path, key: &str, content_type: &str, filename: &str) {
    if filename.is_empty() {
        return;
    }
    let metadata = serde_json::json!({"filename": filename, "content_type": content_type});
    let body = match serde_json::to_vec(&metadata) {
        Ok(body) => body,
        Err(error) => {
            tracing::warn!(%error, %key, "local upload metadata serialization failed");
            return;
        }
    };
    if let Err(error) = tokio::fs::write(format!("{}.meta.json", path.display()), body).await {
        tracing::warn!(%error, %key, "local upload metadata write failed");
    }
}

fn validate_stream_size(size_bytes: i64) -> anyhow::Result<()> {
    anyhow::ensure!(
        size_bytes > 0,
        "streaming upload requires a positive content length"
    );
    anyhow::ensure!(
        size_bytes <= MAX_STREAM_UPLOAD_BYTES,
        "streaming upload exceeds the 100 MiB limit"
    );
    Ok(())
}

fn local_temp_path(path: &Path) -> Option<PathBuf> {
    let parent = path.parent()?;
    let name = path.file_name()?.to_str()?;
    Some(parent.join(format!(".{name}.tmp")))
}

/// Removes a deterministic staging file on errors and async cancellation.
/// Async cleanup after an `.await` is insufficient because dropping the
/// upload future skips that code; unlinking in `Drop` keeps the old object
/// intact and prevents an abandoned partial body from accumulating.
struct LocalTempGuard {
    path: PathBuf,
    armed: bool,
}

impl LocalTempGuard {
    fn new(path: PathBuf) -> Self {
        Self { path, armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for LocalTempGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ = std::fs::remove_file(&self.path);
        }
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
    credentials: SharedCredentialsProvider,
    cdn_domain: Option<String>,
    transport: Arc<S3TransportState>,
}

#[derive(Default)]
struct S3TransportState {
    corrections: Mutex<HashMap<String, EndpointCorrection>>,
}

#[derive(Clone)]
struct EndpointCorrection {
    clock_skew_seconds: i64,
    region: Option<String>,
    expires_at: Instant,
}

struct S3SigningContext<'a> {
    method: &'a str,
    url: &'a Url,
    payload_hash: &'a str,
    now: chrono::DateTime<chrono::Utc>,
    extra: &'a HeaderMap,
    credentials: &'a Credentials,
    region: &'a str,
}

pub struct S3RequestError {
    pub operation: &'static str,
    pub status: reqwest::StatusCode,
    pub code: Option<String>,
    pub request_id: Option<String>,
    pub host_id: Option<String>,
}

impl fmt::Debug for S3RequestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("S3RequestError")
            .field("operation", &self.operation)
            .field("status", &self.status)
            .field("code", &self.code)
            .field("request_id", &self.request_id)
            .field("host_id", &self.host_id.as_ref().map(|_| "[redacted]"))
            .finish()
    }
}

impl fmt::Display for S3RequestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "object storage {} failed with HTTP {}",
            self.operation, self.status
        )?;
        if let Some(code) = &self.code {
            write!(formatter, " ({code})")?;
        }
        if let Some(request_id) = &self.request_id {
            write!(formatter, " [request_id={request_id}]")?;
        }
        Ok(())
    }
}

impl std::error::Error for S3RequestError {}

#[derive(Default)]
struct S3ErrorDocument {
    code: Option<String>,
    request_id: Option<String>,
    host_id: Option<String>,
}

struct S3Failure {
    error: S3RequestError,
    retry_after: Option<Duration>,
    server_date: Option<chrono::DateTime<chrono::Utc>>,
    bucket_region: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BufferedChecksumCapability {
    AwsHttpsTrailer,
    AwsHttpHeader,
    CompatibleSha256,
}

struct BufferedPut {
    body: Vec<u8>,
    payload_hash: String,
    headers: HeaderMap,
}

fn buffered_checksum_capability(endpoint: &Url) -> BufferedChecksumCapability {
    if !uses_aws_endpoint(endpoint) {
        return BufferedChecksumCapability::CompatibleSha256;
    }
    if endpoint.scheme() == "https" {
        BufferedChecksumCapability::AwsHttpsTrailer
    } else {
        BufferedChecksumCapability::AwsHttpHeader
    }
}

fn uses_aws_endpoint(endpoint: &Url) -> bool {
    let Some(host) = endpoint.host_str() else {
        return false;
    };
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    host == "amazonaws.com"
        || host.ends_with(".amazonaws.com")
        || host == "amazonaws.com.cn"
        || host.ends_with(".amazonaws.com.cn")
}

fn prepare_buffered_put(
    body: Vec<u8>,
    capability: BufferedChecksumCapability,
) -> anyhow::Result<BufferedPut> {
    if capability == BufferedChecksumCapability::CompatibleSha256 {
        return Ok(BufferedPut {
            payload_hash: hex::encode(Sha256::digest(&body)),
            body,
            headers: HeaderMap::new(),
        });
    }

    let checksum = BASE64_STANDARD.encode(crc32fast::hash(&body).to_be_bytes());
    let mut headers = HeaderMap::new();
    headers.insert(
        "x-amz-sdk-checksum-algorithm",
        HeaderValue::from_static("CRC32"),
    );
    if capability == BufferedChecksumCapability::AwsHttpHeader || body.is_empty() {
        headers.insert(CRC32_TRAILER, HeaderValue::from_str(&checksum)?);
        return Ok(BufferedPut {
            payload_hash: if capability == BufferedChecksumCapability::AwsHttpsTrailer {
                UNSIGNED_PAYLOAD.to_string()
            } else {
                hex::encode(Sha256::digest(&body))
            },
            body,
            headers,
        });
    }

    let decoded_length = body.len();
    let chunk_prefix = format!("{decoded_length:x}\r\n");
    let trailer = format!("\r\n0\r\n{CRC32_TRAILER}:{checksum}\r\n\r\n");
    let encoded_length = chunk_prefix
        .len()
        .checked_add(decoded_length)
        .and_then(|length| length.checked_add(trailer.len()))
        .ok_or_else(|| anyhow::anyhow!("buffered S3 upload length overflow"))?;
    let mut encoded = Vec::with_capacity(encoded_length);
    encoded.extend_from_slice(chunk_prefix.as_bytes());
    encoded.extend_from_slice(&body);
    encoded.extend_from_slice(trailer.as_bytes());
    anyhow::ensure!(
        encoded.len() == encoded_length,
        "buffered S3 checksum encoding length mismatch"
    );
    headers.insert(
        reqwest::header::CONTENT_ENCODING,
        HeaderValue::from_static("aws-chunked"),
    );
    headers.insert(
        reqwest::header::CONTENT_LENGTH,
        HeaderValue::from_str(&encoded_length.to_string())?,
    );
    headers.insert(
        "x-amz-decoded-content-length",
        HeaderValue::from_str(&decoded_length.to_string())?,
    );
    headers.insert("x-amz-trailer", HeaderValue::from_static(CRC32_TRAILER));
    Ok(BufferedPut {
        body: encoded,
        payload_hash: STREAMING_UNSIGNED_PAYLOAD_TRAILER.to_string(),
        headers,
    })
}

fn s3_http_client() -> anyhow::Result<reqwest::Client> {
    let builder = reqwest::Client::builder()
        .connect_timeout(S3_CONNECT_TIMEOUT)
        .read_timeout(S3_READ_TIMEOUT)
        // A redirect changes the canonical URI/host after SigV4 signing and
        // may replay an upload body to a different origin. Region correction
        // is handled explicitly above, so every endpoint fails closed here.
        .redirect(reqwest::redirect::Policy::none());
    Ok(builder.build()?)
}

fn validate_s3_endpoint(endpoint: &Url) -> anyhow::Result<()> {
    anyhow::ensure!(
        matches!(endpoint.scheme(), "http" | "https")
            && endpoint.host_str().is_some()
            && endpoint.username().is_empty()
            && endpoint.password().is_none()
            && endpoint.query().is_none()
            && endpoint.fragment().is_none(),
        "AWS_ENDPOINT_URL must be an HTTP(S) URL without credentials, a query, or a fragment"
    );
    Ok(())
}

fn aws_dns_suffix(region: &str) -> &'static str {
    if region.starts_with("cn-") {
        "amazonaws.com.cn"
    } else {
        "amazonaws.com"
    }
}

fn default_s3_endpoint(region: &str) -> anyhow::Result<Url> {
    anyhow::ensure!(valid_aws_region(region), "invalid AWS region");
    Ok(Url::parse(&format!(
        "https://s3.{region}.{}",
        aws_dns_suffix(region)
    ))?)
}

impl S3Storage {
    pub async fn from_env() -> anyhow::Result<Option<Self>> {
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
        let endpoint = if let Some(custom) = custom.as_deref() {
            Url::parse(custom)?
        } else {
            default_s3_endpoint(&region)?
        };
        validate_s3_endpoint(&endpoint)?;
        let path_style = s3_use_path_style(
            std::env::var("S3_USE_PATH_STYLE").ok().as_deref(),
            custom.is_some(),
        );
        let credentials = SharedCredentialsProvider::new(
            DefaultCredentialsChain::builder()
                .region(Region::new(region.clone()))
                .build()
                .await,
        );
        Ok(Some(Self {
            client: s3_http_client()?,
            bucket,
            region,
            endpoint,
            custom_endpoint: custom.is_some(),
            path_style,
            credentials,
            cdn_domain: env("CLOUDFRONT_DOMAIN"),
            transport: Arc::new(S3TransportState::default()),
        }))
    }
    fn correction_key(&self) -> String {
        format!(
            "{}://{}:{}/{}",
            self.endpoint.scheme(),
            self.endpoint.host_str().unwrap_or_default(),
            self.endpoint.port_or_known_default().unwrap_or_default(),
            self.bucket
        )
    }

    fn correction(&self) -> EndpointCorrection {
        let empty = EndpointCorrection {
            clock_skew_seconds: 0,
            region: None,
            expires_at: Instant::now(),
        };
        if self.custom_endpoint {
            return empty;
        }
        let key = self.correction_key();
        let mut corrections = self
            .transport
            .corrections
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(correction) = corrections.get(&key).cloned() else {
            return empty;
        };
        if correction.expires_at <= Instant::now() {
            corrections.remove(&key);
            return empty;
        }
        correction
    }

    fn update_correction(&self, clock_skew_seconds: Option<i64>, region: Option<String>) -> bool {
        if self.custom_endpoint {
            return false;
        }
        if let Some(region) = region.as_deref() {
            if !valid_aws_region(region) {
                return false;
            }
        }
        if clock_skew_seconds
            .is_some_and(|seconds| seconds.unsigned_abs() > S3_MAX_CLOCK_SKEW_SECONDS as u64)
        {
            return false;
        }
        let key = self.correction_key();
        let mut corrections = self
            .transport
            .corrections
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let current = corrections.get(&key);
        let next = EndpointCorrection {
            clock_skew_seconds: clock_skew_seconds
                .or_else(|| current.map(|value| value.clock_skew_seconds))
                .unwrap_or_default(),
            region: region.or_else(|| current.and_then(|value| value.region.clone())),
            expires_at: Instant::now() + S3_CORRECTION_TTL,
        };
        let changed = current.is_none_or(|current| {
            current.clock_skew_seconds != next.clock_skew_seconds || current.region != next.region
        });
        corrections.insert(key, next);
        changed
    }

    fn request_url(&self, key: &str) -> anyhow::Result<Url> {
        let correction = self.correction();
        self.request_url_for_region(key, correction.region.as_deref().unwrap_or(&self.region))
    }

    fn request_url_for_region(&self, key: &str, region: &str) -> anyhow::Result<Url> {
        if key.is_empty() || key.split('/').any(|v| v == "." || v == "..") {
            anyhow::bail!("invalid storage key")
        }
        let encoded_key = key.split('/').map(aws_encode).collect::<Vec<_>>().join("/");
        let mut url = if !self.custom_endpoint && region != self.region {
            default_s3_endpoint(region)?
        } else {
            self.endpoint.clone()
        };
        let base_path = url.path().trim_end_matches('/').to_string();
        let path_style = self.path_style || (!self.custom_endpoint && self.bucket.contains('.'));
        if path_style {
            url.set_path(&format!("{base_path}/{}/{encoded_key}", self.bucket));
        } else {
            let host = url
                .host_str()
                .ok_or_else(|| anyhow::anyhow!("S3 endpoint has no host"))?;
            url.set_host(Some(&format!("{}.{host}", self.bucket)))?;
            url.set_path(&format!("{base_path}/{encoded_key}"));
        }
        Ok(url)
    }
    fn signed_headers(&self, context: S3SigningContext<'_>) -> anyhow::Result<HeaderMap> {
        let S3SigningContext {
            method,
            url,
            payload_hash,
            now,
            extra,
            credentials,
            region: signing_region,
        } = context;
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
        if let Some(token) = credentials.session_token() {
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
        let scope = format!("{date}/{signing_region}/s3/aws4_request");
        let to_sign = format!(
            "AWS4-HMAC-SHA256\n{timestamp}\n{scope}\n{}",
            hex::encode(Sha256::digest(canonical_request.as_bytes()))
        );
        let k_date = hmac_bytes(
            format!("AWS4{}", credentials.secret_access_key()).as_bytes(),
            date.as_bytes(),
        )?;
        let k_region = hmac_bytes(&k_date, signing_region.as_bytes())?;
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
    async fn execute(
        &self,
        method: reqwest::Method,
        key: &str,
        body: Option<Vec<u8>>,
        extra: HeaderMap,
    ) -> anyhow::Result<reqwest::Response> {
        let bytes = body.unwrap_or_default();
        let hash = hex::encode(Sha256::digest(&bytes));
        self.execute_bytes(method, key, bytes, extra, &hash).await
    }

    async fn execute_buffered_put(
        &self,
        key: &str,
        body: Vec<u8>,
        mut extra: HeaderMap,
    ) -> anyhow::Result<reqwest::Response> {
        let prepared = prepare_buffered_put(body, buffered_checksum_capability(&self.endpoint))?;
        extra.extend(prepared.headers);
        self.execute_bytes(
            reqwest::Method::PUT,
            key,
            prepared.body,
            extra,
            &prepared.payload_hash,
        )
        .await
    }

    async fn execute_bytes(
        &self,
        method: reqwest::Method,
        key: &str,
        bytes: Vec<u8>,
        extra: HeaderMap,
        payload_hash: &str,
    ) -> anyhow::Result<reqwest::Response> {
        let credentials = self
            .credentials
            .provide_credentials()
            .await
            .map_err(|error| anyhow::anyhow!("resolve AWS credentials: {error}"))?;
        let operation = s3_operation(&method);
        for attempt in 1..=S3_MAX_ATTEMPTS {
            let correction = self.correction();
            let signing_region = correction.region.as_deref().unwrap_or(&self.region);
            let url = self.request_url_for_region(key, signing_region)?;
            let now = chrono::Utc::now() + chrono::Duration::seconds(correction.clock_skew_seconds);
            let mut headers = self.signed_headers(S3SigningContext {
                method: method.as_str(),
                url: &url,
                payload_hash,
                now,
                extra: &extra,
                credentials: &credentials,
                region: signing_region,
            })?;
            headers.extend(extra.clone());
            let response = self
                .client
                .request(method.clone(), url)
                .headers(headers)
                .body(bytes.clone())
                .send()
                .await;
            let response = match response {
                Ok(response) => response,
                Err(error) if attempt < S3_MAX_ATTEMPTS && retryable_transport_error(&error) => {
                    tokio::time::sleep(retry_delay(attempt, None, rand::random())).await;
                    continue;
                }
                Err(error) => {
                    return Err(anyhow::anyhow!(
                        "object storage {operation} transport failed: {}",
                        transport_error_kind(&error)
                    ));
                }
            };
            if response.status().is_success() {
                return Ok(response);
            }
            let failure = parse_s3_failure(operation, response).await;
            if attempt < S3_MAX_ATTEMPTS && self.apply_aws_correction(&failure) {
                continue;
            }
            if attempt < S3_MAX_ATTEMPTS && retryable_s3_failure(&failure.error) {
                tokio::time::sleep(retry_delay(attempt, failure.retry_after, rand::random())).await;
                continue;
            }
            return Err(failure.error.into());
        }
        unreachable!("bounded S3 retry loop always returns")
    }

    fn apply_aws_correction(&self, failure: &S3Failure) -> bool {
        if self.custom_endpoint {
            return false;
        }
        if is_region_redirect(&failure.error) || failure.bucket_region.is_some() {
            if let Some(region) = failure.bucket_region.clone() {
                return self.update_correction(None, Some(region));
            }
        }
        if is_clock_skew_error(failure.error.code.as_deref()) {
            if let Some(server_date) = failure.server_date {
                let skew = server_date
                    .signed_duration_since(chrono::Utc::now())
                    .num_seconds();
                return self.update_correction(Some(skew), None);
            }
        }
        false
    }

    async fn execute_stream(
        &self,
        key: &str,
        body: Box<dyn AsyncRead + Send + Unpin>,
        size_bytes: i64,
        mut extra: HeaderMap,
    ) -> anyhow::Result<()> {
        validate_stream_size(size_bytes)?;
        let correction = self.correction();
        let signing_region = correction.region.as_deref().unwrap_or(&self.region);
        let url = self.request_url_for_region(key, signing_region)?;
        let credentials = self
            .credentials
            .provide_credentials()
            .await
            .map_err(|error| anyhow::anyhow!("resolve AWS credentials: {error}"))?;
        let mut headers = self.signed_headers(S3SigningContext {
            method: "PUT",
            url: &url,
            payload_hash: UNSIGNED_PAYLOAD,
            now: chrono::Utc::now() + chrono::Duration::seconds(correction.clock_skew_seconds),
            extra: &extra,
            credentials: &credentials,
            region: signing_region,
        })?;
        extra.insert(
            reqwest::header::CONTENT_LENGTH,
            HeaderValue::from_str(&size_bytes.to_string())?,
        );
        headers.extend(extra);
        let response = self
            .client
            .put(url)
            .headers(headers)
            .body(reqwest::Body::wrap_stream(exact_reader_stream(
                body,
                size_bytes as u64,
            )))
            .send()
            .await
            .map_err(|error| {
                anyhow::anyhow!(
                    "object storage PutObjectStream transport failed: {}",
                    transport_error_kind(&error)
                )
            })?;
        if response.status().is_success() {
            return Ok(());
        }
        let failure = parse_s3_failure("PutObjectStream", response).await;
        // The reader is not replayable, so never retry this request. Retain a
        // bounded AWS correction for the next upload instead.
        let _ = self.apply_aws_correction(&failure);
        Err(failure.error.into())
    }

    async fn presigned_get(
        &self,
        key: &str,
        ttl: std::time::Duration,
        content_disposition: &str,
    ) -> anyhow::Result<String> {
        validate_s3_presign_ttl(ttl)?;
        let expires = ttl.as_secs();
        let correction = self.correction();
        let signing_region = correction.region.as_deref().unwrap_or(&self.region);
        let mut url = self.request_url_for_region(key, signing_region)?;
        let credentials = self
            .credentials
            .provide_credentials()
            .await
            .map_err(|error| anyhow::anyhow!("resolve AWS credentials: {error}"))?;
        let now = chrono::Utc::now() + chrono::Duration::seconds(correction.clock_skew_seconds);
        let date = now.format("%Y%m%d").to_string();
        let timestamp = now.format("%Y%m%dT%H%M%SZ").to_string();
        let scope = format!("{date}/{signing_region}/s3/aws4_request");
        let hostname = url
            .host_str()
            .ok_or_else(|| anyhow::anyhow!("S3 endpoint has no host"))?;
        let host = url
            .port()
            .map_or_else(|| hostname.to_string(), |port| format!("{hostname}:{port}"));
        let mut query = url
            .query_pairs()
            .map(|(name, value)| (name.into_owned(), value.into_owned()))
            .collect::<Vec<_>>();
        query.retain(|(name, _)| {
            !name.to_ascii_lowercase().starts_with("x-amz-")
                && !name.eq_ignore_ascii_case("response-content-disposition")
        });
        if !content_disposition.is_empty() {
            query.push((
                "response-content-disposition".to_string(),
                content_disposition.to_string(),
            ));
        }
        query.extend([
            (
                "X-Amz-Algorithm".to_string(),
                "AWS4-HMAC-SHA256".to_string(),
            ),
            (
                "X-Amz-Credential".to_string(),
                format!("{}/{scope}", credentials.access_key_id()),
            ),
            ("X-Amz-Date".to_string(), timestamp.clone()),
            ("X-Amz-Expires".to_string(), expires.to_string()),
            ("X-Amz-SignedHeaders".to_string(), "host".to_string()),
        ]);
        if let Some(token) = credentials.session_token() {
            query.push(("X-Amz-Security-Token".to_string(), token.to_string()));
        }
        let canonical_query = canonical_query(&query);
        let canonical_request = format!(
            "GET\n{}\n{canonical_query}\nhost:{host}\n\nhost\nUNSIGNED-PAYLOAD",
            url.path()
        );
        let to_sign = format!(
            "AWS4-HMAC-SHA256\n{timestamp}\n{scope}\n{}",
            hex::encode(Sha256::digest(canonical_request.as_bytes()))
        );
        let k_date = hmac_bytes(
            format!("AWS4{}", credentials.secret_access_key()).as_bytes(),
            date.as_bytes(),
        )?;
        let k_region = hmac_bytes(&k_date, signing_region.as_bytes())?;
        let k_service = hmac_bytes(&k_region, b"s3")?;
        let k_signing = hmac_bytes(&k_service, b"aws4_request")?;
        let signature = hex::encode(hmac_bytes(&k_signing, to_sign.as_bytes())?);
        url.set_query(Some(&format!(
            "{canonical_query}&X-Amz-Signature={signature}"
        )));
        Ok(url.to_string())
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
        self.execute_buffered_put(key, body, headers).await?;
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
    async fn upload_stream(
        &self,
        key: &str,
        body: Box<dyn AsyncRead + Send + Unpin>,
        size_bytes: i64,
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
        self.execute_stream(key, body, size_bytes, headers).await?;
        Ok(self.object_url(key))
    }
    fn supports_streaming_uploads(&self) -> bool {
        true
    }
    async fn delete(&self, key: &str) -> anyhow::Result<()> {
        self.execute(reqwest::Method::DELETE, key, None, HeaderMap::new())
            .await
            .map(|_| ())
    }
    async fn presign_get(&self, key: &str, ttl: std::time::Duration) -> anyhow::Result<String> {
        self.presigned_get(key, ttl, "").await
    }
    async fn presign_get_with_content_disposition(
        &self,
        key: &str,
        ttl: std::time::Duration,
        content_disposition: &str,
    ) -> anyhow::Result<String> {
        self.presigned_get(key, ttl, content_disposition).await
    }
    fn supports_presigned_downloads(&self) -> bool {
        true
    }
    fn key_from_url(&self, raw: &str) -> Option<String> {
        let url = Url::parse(raw).ok()?;
        if !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return None;
        }
        if !self.custom_endpoint {
            if let Some(key) = aws_bucket_key_from_url(&url, &self.bucket) {
                return Some(key);
            }
        }
        const MARKER: &str = "__cordy_object_scope__";
        let mut expected = Vec::new();
        if let Ok(url) = Url::parse(&self.object_url(MARKER)) {
            expected.push(url);
        }
        if let Ok(url) = self.request_url(MARKER) {
            expected.push(url);
        }
        expected.into_iter().find_map(|candidate| {
            if !same_origin(&url, &candidate) {
                return None;
            }
            let prefix = candidate.path().strip_suffix(MARKER)?;
            let encoded = url.path().strip_prefix(prefix)?;
            decode_storage_key(encoded)
        })
    }
    fn object_url(&self, key: &str) -> String {
        if let Some(domain) = &self.cdn_domain {
            format!("https://{}/{key}", domain.trim_end_matches('/'))
        } else if !self.custom_endpoint && self.bucket.contains('.') {
            let Ok(mut url) = default_s3_endpoint(&self.region) else {
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

pub async fn from_env(
    local_dir: Option<&str>,
    local_base: Option<&str>,
) -> anyhow::Result<Arc<dyn AttachmentStorage>> {
    if let Some(s3) = S3Storage::from_env().await? {
        return Ok(Arc::new(s3));
    }
    Ok(Arc::new(LocalStorage::new(
        PathBuf::from(local_dir.unwrap_or("./data/uploads")),
        local_base.unwrap_or("").to_string(),
    )?))
}

#[derive(Clone)]
pub struct ChannelMediaStorage {
    inner: Arc<dyn AttachmentStorage>,
}

impl ChannelMediaStorage {
    pub fn new(inner: Arc<dyn AttachmentStorage>) -> Self {
        Self { inner }
    }
}

impl cordy_lark::media_ingest::MediaStorage for ChannelMediaStorage {
    fn upload(
        &self,
        key: &str,
        data: Vec<u8>,
        content_type: &str,
        filename: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send + '_>> {
        let key = key.to_string();
        let content_type = content_type.to_string();
        let filename = filename.to_string();
        Box::pin(async move {
            self.inner
                .upload(&key, data, &content_type, &filename)
                .await
                .map(|_| ())
        })
    }

    fn object_url(&self, key: &str) -> String {
        self.inner.object_url(key)
    }

    fn as_stream_storage(&self) -> Option<&dyn cordy_lark::media_ingest::MediaStreamStorage> {
        self.inner.supports_streaming_uploads().then_some(self)
    }
}

#[async_trait]
impl cordy_lark::media_ingest::MediaStreamStorage for ChannelMediaStorage {
    async fn upload_stream(
        &self,
        ctx: tokio_util::sync::CancellationToken,
        key: &str,
        body: Box<dyn AsyncRead + Send + Unpin>,
        size_bytes: i64,
        content_type: &str,
        filename: &str,
    ) -> anyhow::Result<()> {
        tokio::select! {
            _ = ctx.cancelled() => anyhow::bail!("channel media upload cancelled"),
            result = self.inner.upload_stream(key, body, size_bytes, content_type, filename) => result.map(|_| ()),
        }
    }
}

#[async_trait]
impl cordy_wecom::media_ingest::MediaStorage for ChannelMediaStorage {
    async fn upload(
        &self,
        key: &str,
        data: Vec<u8>,
        content_type: &str,
        filename: &str,
    ) -> anyhow::Result<String> {
        self.inner.upload(key, data, content_type, filename).await
    }

    fn object_url(&self, key: &str) -> String {
        self.inner.object_url(key)
    }

    fn as_stream_storage(&self) -> Option<&dyn cordy_wecom::media_ingest::MediaStreamStorage> {
        self.inner.supports_streaming_uploads().then_some(self)
    }
}

#[async_trait]
impl cordy_wecom::media_ingest::MediaStreamStorage for ChannelMediaStorage {
    async fn upload_stream(
        &self,
        ctx: tokio_util::sync::CancellationToken,
        key: &str,
        body: Box<dyn AsyncRead + Send + Unpin>,
        size_bytes: i64,
        content_type: &str,
        filename: &str,
    ) -> anyhow::Result<String> {
        tokio::select! {
            _ = ctx.cancelled() => anyhow::bail!("channel media upload cancelled"),
            result = self.inner.upload_stream(key, body, size_bytes, content_type, filename) => result,
        }
    }
}

fn s3_operation(method: &reqwest::Method) -> &'static str {
    if method == reqwest::Method::GET {
        "GetObject"
    } else if method == reqwest::Method::PUT {
        "PutObject"
    } else if method == reqwest::Method::DELETE {
        "DeleteObject"
    } else {
        "Request"
    }
}

async fn parse_s3_failure(operation: &'static str, response: reqwest::Response) -> S3Failure {
    let status = response.status();
    let headers = response.headers();
    let retry_after = headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(parse_retry_after);
    let server_date = headers
        .get(reqwest::header::DATE)
        .and_then(|value| value.to_str().ok())
        .and_then(parse_http_date);
    let bucket_region = headers
        .get("x-amz-bucket-region")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| valid_aws_region(value))
        .map(str::to_string);
    let header_request_id = safe_error_identifier(headers.get("x-amz-request-id"), 256);
    let header_host_id = safe_error_identifier(headers.get("x-amz-id-2"), 2048);
    let mut body = Vec::new();
    let mut overflow = false;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let Ok(chunk) = chunk else {
            break;
        };
        if body
            .len()
            .checked_add(chunk.len())
            .is_none_or(|length| length > S3_MAX_ERROR_BODY)
        {
            overflow = true;
            break;
        }
        body.extend_from_slice(&chunk);
    }
    let document = if overflow {
        S3ErrorDocument::default()
    } else {
        parse_s3_error_document(&body)
    };
    S3Failure {
        error: S3RequestError {
            operation,
            status,
            code: document.code,
            request_id: header_request_id.or(document.request_id),
            host_id: header_host_id.or(document.host_id),
        },
        retry_after,
        server_date,
        bucket_region,
    }
}

fn parse_s3_error_document(body: &[u8]) -> S3ErrorDocument {
    let Ok(xml) = std::str::from_utf8(body) else {
        return S3ErrorDocument::default();
    };
    S3ErrorDocument {
        code: xml_error_field(xml, "Code", 128).filter(|value| {
            value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        }),
        request_id: xml_error_field(xml, "RequestId", 256),
        host_id: xml_error_field(xml, "HostId", 2048),
    }
}

fn xml_error_field(xml: &str, field: &str, max_len: usize) -> Option<String> {
    let value = xml
        .split_once(&format!("<{field}>"))?
        .1
        .split_once(&format!("</{field}>"))?
        .0
        .trim();
    (!value.is_empty()
        && value.len() <= max_len
        && value
            .bytes()
            .all(|byte| byte.is_ascii_graphic() && !matches!(byte, b'<' | b'>')))
    .then(|| value.to_string())
}

fn safe_error_identifier(value: Option<&HeaderValue>, max_len: usize) -> Option<String> {
    let value = value?.to_str().ok()?.trim();
    (!value.is_empty()
        && value.len() <= max_len
        && value.bytes().all(|byte| byte.is_ascii_graphic()))
    .then(|| value.to_string())
}

fn retryable_transport_error(error: &reqwest::Error) -> bool {
    error.is_connect() || error.is_timeout() || error.is_body()
}

fn transport_error_kind(error: &reqwest::Error) -> &'static str {
    if error.is_timeout() {
        "timeout"
    } else if error.is_connect() {
        "connection"
    } else if error.is_body() {
        "request body"
    } else if error.is_builder() {
        "invalid request"
    } else {
        "request"
    }
}

fn retryable_s3_failure(error: &S3RequestError) -> bool {
    matches!(
        error.status,
        reqwest::StatusCode::REQUEST_TIMEOUT
            | reqwest::StatusCode::TOO_MANY_REQUESTS
            | reqwest::StatusCode::INTERNAL_SERVER_ERROR
            | reqwest::StatusCode::BAD_GATEWAY
            | reqwest::StatusCode::SERVICE_UNAVAILABLE
            | reqwest::StatusCode::GATEWAY_TIMEOUT
    ) || matches!(
        error.code.as_deref(),
        Some(
            "RequestTimeout"
                | "RequestTimeoutException"
                | "SlowDown"
                | "Throttling"
                | "ThrottlingException"
                | "InternalError"
                | "ServiceUnavailable"
        )
    )
}

fn is_region_redirect(error: &S3RequestError) -> bool {
    matches!(
        error.status,
        reqwest::StatusCode::MOVED_PERMANENTLY | reqwest::StatusCode::TEMPORARY_REDIRECT
    ) || matches!(
        error.code.as_deref(),
        Some("PermanentRedirect" | "AuthorizationHeaderMalformed" | "IncorrectEndpoint")
    )
}

fn is_clock_skew_error(code: Option<&str>) -> bool {
    matches!(
        code,
        Some(
            "RequestTimeTooSkewed"
                | "RequestExpired"
                | "InvalidSignatureException"
                | "SignatureDoesNotMatch"
        )
    )
}

fn valid_aws_region(region: &str) -> bool {
    (3..=64).contains(&region.len())
        && region
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && region
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && region
            .as_bytes()
            .last()
            .is_some_and(u8::is_ascii_alphanumeric)
        && region.contains('-')
}

fn aws_bucket_key_from_url(url: &Url, bucket: &str) -> Option<String> {
    if url.scheme() != "https" || url.port().is_some() {
        return None;
    }
    let host = url.host_str()?;
    let (aws_host, suffix) = host
        .strip_suffix(".amazonaws.com.cn")
        .map(|host| (host, "amazonaws.com.cn"))
        .or_else(|| {
            host.strip_suffix(".amazonaws.com")
                .map(|host| (host, "amazonaws.com"))
        })?;
    let virtual_prefix = format!("{bucket}.s3.");
    if let Some(region) = aws_host.strip_prefix(&virtual_prefix) {
        if valid_aws_region(region) && aws_dns_suffix(region) == suffix {
            return decode_storage_key(url.path().strip_prefix('/')?);
        }
        return None;
    }
    let region = aws_host.strip_prefix("s3.")?;
    if !valid_aws_region(region) || aws_dns_suffix(region) != suffix {
        return None;
    }
    let prefix = format!("/{bucket}/");
    decode_storage_key(url.path().strip_prefix(&prefix)?)
}

fn parse_http_date(value: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc2822(value.trim())
        .ok()
        .map(|value| value.with_timezone(&chrono::Utc))
}

fn parse_retry_after(value: &str) -> Option<Duration> {
    let value = value.trim();
    let delay = if let Ok(seconds) = value.parse::<u64>() {
        Duration::from_secs(seconds)
    } else {
        let deadline = parse_http_date(value)?;
        let milliseconds = deadline
            .signed_duration_since(chrono::Utc::now())
            .num_milliseconds()
            .max(0) as u64;
        Duration::from_millis(milliseconds)
    };
    Some(delay.min(S3_MAX_RETRY_DELAY))
}

fn retry_delay(attempt: usize, retry_after: Option<Duration>, jitter: u64) -> Duration {
    if let Some(delay) = retry_after {
        return delay.min(S3_MAX_RETRY_DELAY);
    }
    let exponent = attempt.saturating_sub(1).min(8) as u32;
    let window_ms = 50u64
        .saturating_mul(1u64 << exponent)
        .min(S3_MAX_RETRY_DELAY.as_millis() as u64);
    let delay_ms = ((jitter as u128 * (window_ms as u128 + 1)) >> 64) as u64;
    Duration::from_millis(delay_ms)
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
fn canonical_query(pairs: &[(String, String)]) -> String {
    let mut encoded = pairs
        .iter()
        .map(|(name, value)| (aws_encode(name), aws_encode(value)))
        .collect::<Vec<_>>();
    encoded.sort_unstable();
    encoded
        .into_iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("&")
}
fn exact_reader_stream(
    body: Box<dyn AsyncRead + Send + Unpin>,
    size: u64,
) -> impl futures_util::Stream<Item = Result<Vec<u8>, std::io::Error>> + Send {
    futures_util::stream::try_unfold((body, size), |(mut body, remaining)| async move {
        if remaining == 0 {
            return Ok(None);
        }
        let mut chunk = vec![0; remaining.min(64 << 10) as usize];
        let read = body.read(&mut chunk).await?;
        if read == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "stream ended before its declared content length",
            ));
        }
        chunk.truncate(read);
        let remaining = remaining - read as u64;
        if remaining == 0 {
            let mut extra = [0u8; 1];
            if body.read(&mut extra).await? != 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "stream exceeded its declared content length",
                ));
            }
        }
        Ok(Some((chunk, (body, remaining))))
    })
}
fn same_origin(left: &Url, right: &Url) -> bool {
    left.scheme().eq_ignore_ascii_case(right.scheme())
        && left.host_str().map(str::to_ascii_lowercase)
            == right.host_str().map(str::to_ascii_lowercase)
        && left.port_or_known_default() == right.port_or_known_default()
}
fn decode_storage_key(encoded: &str) -> Option<String> {
    let segments = encoded
        .split('/')
        .map(|part| {
            percent_encoding::percent_decode_str(part)
                .decode_utf8()
                .ok()
        })
        .collect::<Option<Vec<_>>>()?;
    if segments.is_empty()
        || segments.iter().any(|part| {
            part.is_empty()
                || matches!(part.as_ref(), "." | "..")
                || part.contains(['/', '\\', '\0'])
        })
    {
        return None;
    }
    Some(
        segments
            .into_iter()
            .map(|part| part.into_owned())
            .collect::<Vec<_>>()
            .join("/"),
    )
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
    use std::{
        collections::VecDeque,
        sync::atomic::{AtomicUsize, Ordering},
    };

    async fn s3_status_server(statuses: Vec<reqwest::StatusCode>) -> (String, Arc<AtomicUsize>) {
        let statuses = Arc::new(Mutex::new(VecDeque::from(statuses)));
        let attempts = Arc::new(AtomicUsize::new(0));
        let app = axum::Router::new().fallback(axum::routing::any({
            let statuses = statuses.clone();
            let attempts = attempts.clone();
            move || {
                let statuses = statuses.clone();
                let attempts = attempts.clone();
                async move {
                    attempts.fetch_add(1, Ordering::SeqCst);
                    let status = statuses
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .pop_front()
                        .unwrap_or(reqwest::StatusCode::SERVICE_UNAVAILABLE);
                    let mut response = axum::response::Response::new(Body::from(
                        "<Error><Code>SlowDown</Code><RequestId>REQ123</RequestId></Error>",
                    ));
                    *response.status_mut() = status;
                    response
                        .headers_mut()
                        .insert(reqwest::header::RETRY_AFTER, HeaderValue::from_static("0"));
                    response.headers_mut().insert(
                        "x-amz-bucket-region",
                        HeaderValue::from_static("eu-central-1"),
                    );
                    response
                }
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{address}"), attempts)
    }

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

    #[tokio::test]
    async fn local_stream_upload_is_atomic_and_checks_exact_size() {
        let dir = tempfile::tempdir().unwrap();
        let store = LocalStorage::new(dir.path().to_path_buf(), String::new()).unwrap();
        let key = "workspaces/w/media.bin";
        store
            .upload(key, b"old".to_vec(), "application/octet-stream", "old.bin")
            .await
            .unwrap();
        assert!(store
            .upload_stream(
                key,
                Box::new(std::io::Cursor::new(b"too-long".to_vec())),
                3,
                "application/octet-stream",
                "new.bin",
            )
            .await
            .is_err());
        let old = store.get(key, None).await.unwrap();
        assert_eq!(old.body.collect().await.unwrap().to_bytes(), &b"old"[..]);

        store
            .upload_stream(
                key,
                Box::new(std::io::Cursor::new(b"replacement".to_vec())),
                11,
                "application/octet-stream",
                "new.bin",
            )
            .await
            .unwrap();
        let replacement = store.get(key, None).await.unwrap();
        assert_eq!(
            replacement.body.collect().await.unwrap().to_bytes(),
            &b"replacement"[..]
        );
        assert!(!local_temp_path(&store.path(key).unwrap()).unwrap().exists());
    }

    #[tokio::test]
    async fn local_stream_upload_cancellation_removes_partial_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let store = LocalStorage::new(dir.path().to_path_buf(), String::new()).unwrap();
        let key = "workspaces/w/cancelled.bin";
        store
            .upload(key, b"old".to_vec(), "application/octet-stream", "old.bin")
            .await
            .unwrap();

        let (mut writer, reader) = tokio::io::duplex(8);
        writer.write_all(b"partial").await.unwrap();
        let upload_store = store.clone();
        let upload = tokio::spawn(async move {
            upload_store
                .upload_stream(
                    key,
                    Box::new(reader),
                    10,
                    "application/octet-stream",
                    "new.bin",
                )
                .await
        });
        let tmp = local_temp_path(&store.path(key).unwrap()).unwrap();
        for _ in 0..100 {
            if tmp.exists() {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(tmp.exists());
        upload.abort();
        assert!(upload.await.unwrap_err().is_cancelled());
        assert!(!tmp.exists());

        let old = store.get(key, None).await.unwrap();
        assert_eq!(old.body.collect().await.unwrap().to_bytes(), &b"old"[..]);
    }

    #[tokio::test]
    async fn exact_reader_stream_rejects_short_and_long_bodies() {
        use futures_util::StreamExt;

        let mut exact = Box::pin(exact_reader_stream(
            Box::new(std::io::Cursor::new(b"exact".to_vec())),
            5,
        ));
        assert_eq!(exact.next().await.unwrap().unwrap(), b"exact");
        assert!(exact.next().await.is_none());

        let mut short = Box::pin(exact_reader_stream(
            Box::new(std::io::Cursor::new(b"short".to_vec())),
            6,
        ));
        assert_eq!(short.next().await.unwrap().unwrap(), b"short");
        assert!(short.next().await.unwrap().is_err());

        let mut long = Box::pin(exact_reader_stream(
            Box::new(std::io::Cursor::new(b"longer".to_vec())),
            4,
        ));
        assert!(long.next().await.unwrap().is_err());
    }

    fn test_credentials(session_token: Option<&str>) -> Credentials {
        Credentials::new(
            "key",
            "secret",
            session_token.map(str::to_string),
            None,
            "attachment-storage-test",
        )
    }

    #[derive(Debug)]
    struct MissingCredentials;

    impl ProvideCredentials for MissingCredentials {
        fn provide_credentials<'a>(
            &'a self,
        ) -> aws_credential_types::provider::future::ProvideCredentials<'a>
        where
            Self: 'a,
        {
            aws_credential_types::provider::future::ProvideCredentials::ready(Err(
                aws_credential_types::provider::error::CredentialsError::not_loaded_no_source(),
            ))
        }
    }

    #[test]
    fn s3_unicode_key_round_trips_through_stored_url() {
        let store = S3Storage {
            client: s3_http_client().unwrap(),
            bucket: "bucket".into(),
            region: "us-west-2".into(),
            endpoint: Url::parse("https://s3.us-west-2.amazonaws.com").unwrap(),
            custom_endpoint: false,
            path_style: false,
            credentials: SharedCredentialsProvider::new(test_credentials(None)),
            cdn_domain: None,
            transport: Arc::new(S3TransportState::default()),
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
            client: s3_http_client().unwrap(),
            bucket: "bucket".into(),
            region: "us-west-2".into(),
            endpoint: Url::parse(endpoint).unwrap(),
            custom_endpoint,
            path_style,
            credentials: SharedCredentialsProvider::new(test_credentials(None)),
            cdn_domain: None,
            transport: Arc::new(S3TransportState::default()),
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
    fn s3_endpoint_allows_base_paths_but_rejects_url_credentials_and_queries() {
        for allowed in [
            "https://objects.example.test/base/path",
            "http://127.0.0.1:9000/minio",
        ] {
            validate_s3_endpoint(&Url::parse(allowed).unwrap()).unwrap();
        }
        for rejected in [
            "ftp://objects.example.test",
            "https://user:password@objects.example.test",
            "https://objects.example.test/base?token=secret",
            "https://objects.example.test/base#fragment",
        ] {
            assert!(
                validate_s3_endpoint(&Url::parse(rejected).unwrap()).is_err(),
                "{rejected}"
            );
        }
    }

    #[test]
    fn default_s3_endpoint_uses_the_region_partition() {
        assert_eq!(
            default_s3_endpoint("us-west-2").unwrap().as_str(),
            "https://s3.us-west-2.amazonaws.com/"
        );
        assert_eq!(
            default_s3_endpoint("us-gov-west-1").unwrap().as_str(),
            "https://s3.us-gov-west-1.amazonaws.com/"
        );
        assert_eq!(
            default_s3_endpoint("cn-north-1").unwrap().as_str(),
            "https://s3.cn-north-1.amazonaws.com.cn/"
        );
        assert!(default_s3_endpoint("cn-north-1/attacker.test").is_err());
    }

    #[test]
    fn region_correction_uses_the_correct_aws_partition() {
        let store = test_s3("https://s3.us-west-2.amazonaws.com", false, false);
        assert_eq!(
            store
                .request_url_for_region("workspaces/w/file.txt", "cn-northwest-1")
                .unwrap()
                .as_str(),
            "https://bucket.s3.cn-northwest-1.amazonaws.com.cn/workspaces/w/file.txt"
        );
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
    fn s3_checksum_capability_requires_an_aws_hostname_boundary() {
        for endpoint in [
            "https://s3.us-west-2.amazonaws.com",
            "https://S3.US-WEST-2.AMAZONAWS.COM",
            "https://bucket.s3.amazonaws.com.",
            "https://s3-fips.us-gov-west-1.amazonaws.com",
            "https://s3.dualstack.us-east-1.amazonaws.com",
            "https://bucket.vpce-012345.s3.us-east-1.vpce.amazonaws.com",
            "https://s3.cn-north-1.amazonaws.com.cn",
        ] {
            assert_eq!(
                buffered_checksum_capability(&Url::parse(endpoint).unwrap()),
                BufferedChecksumCapability::AwsHttpsTrailer,
                "{endpoint}"
            );
        }
        assert_eq!(
            buffered_checksum_capability(&Url::parse("http://s3.us-west-2.amazonaws.com").unwrap()),
            BufferedChecksumCapability::AwsHttpHeader
        );
        for endpoint in [
            "https://s3.amazonaws.com.example.test",
            "https://notamazonaws.com",
            "https://oss-cn-hangzhou.aliyuncs.com",
            "https://cos.ap-guangzhou.myqcloud.com",
            "https://minio.internal/amazonaws.com",
            "https://minio.internal?probe=amazonaws.com",
            "http://minio.internal:9000",
        ] {
            assert_eq!(
                buffered_checksum_capability(&Url::parse(endpoint).unwrap()),
                BufferedChecksumCapability::CompatibleSha256,
                "{endpoint}"
            );
        }
    }

    #[test]
    fn aws_https_buffered_put_uses_crc32_trailer_and_exact_lengths() {
        let prepared = prepare_buffered_put(
            b"123456789".to_vec(),
            BufferedChecksumCapability::AwsHttpsTrailer,
        )
        .unwrap();
        assert_eq!(prepared.payload_hash, STREAMING_UNSIGNED_PAYLOAD_TRAILER);
        assert_eq!(
            prepared.body,
            b"9\r\n123456789\r\n0\r\nx-amz-checksum-crc32:y/Q5Jg==\r\n\r\n"
        );
        assert_eq!(
            prepared
                .headers
                .get(reqwest::header::CONTENT_ENCODING)
                .unwrap(),
            "aws-chunked"
        );
        assert_eq!(
            prepared
                .headers
                .get("x-amz-decoded-content-length")
                .unwrap(),
            "9"
        );
        assert_eq!(
            prepared
                .headers
                .get(reqwest::header::CONTENT_LENGTH)
                .unwrap(),
            prepared.body.len().to_string().as_str()
        );
        assert_eq!(
            prepared.headers.get("x-amz-trailer").unwrap(),
            CRC32_TRAILER
        );
        assert_eq!(
            prepared
                .headers
                .get("x-amz-sdk-checksum-algorithm")
                .unwrap(),
            "CRC32"
        );
        assert!(prepared.headers.get(CRC32_TRAILER).is_none());
    }

    #[test]
    fn aws_empty_or_http_put_uses_checksum_header_without_trailer() {
        let empty =
            prepare_buffered_put(Vec::new(), BufferedChecksumCapability::AwsHttpsTrailer).unwrap();
        assert_eq!(empty.payload_hash, UNSIGNED_PAYLOAD);
        assert_eq!(empty.headers.get(CRC32_TRAILER).unwrap(), "AAAAAA==");
        assert!(empty.headers.get("x-amz-trailer").is_none());
        assert!(empty
            .headers
            .get(reqwest::header::CONTENT_ENCODING)
            .is_none());

        let http = prepare_buffered_put(
            b"123456789".to_vec(),
            BufferedChecksumCapability::AwsHttpHeader,
        )
        .unwrap();
        assert_eq!(http.headers.get(CRC32_TRAILER).unwrap(), "y/Q5Jg==");
        assert_eq!(http.payload_hash, hex::encode(Sha256::digest(b"123456789")));
        assert!(http.headers.get("x-amz-trailer").is_none());
    }

    #[test]
    fn compatible_buffered_put_keeps_raw_sha256_body_without_checksum_headers() {
        let body = b"compatible endpoint body".to_vec();
        let prepared =
            prepare_buffered_put(body.clone(), BufferedChecksumCapability::CompatibleSha256)
                .unwrap();
        assert_eq!(prepared.body, body);
        assert_eq!(
            prepared.payload_hash,
            hex::encode(Sha256::digest(&prepared.body))
        );
        assert!(prepared.headers.is_empty());
    }

    #[test]
    fn retry_policy_is_bounded_and_honors_retry_after() {
        assert_eq!(S3_CONNECT_TIMEOUT, Duration::from_secs(30));
        assert_eq!(S3_READ_TIMEOUT, Duration::from_secs(60));
        assert_eq!(
            retry_delay(1, Some(Duration::from_secs(7)), 0),
            Duration::from_secs(7)
        );
        assert_eq!(
            retry_delay(1, Some(Duration::from_secs(60)), 0),
            S3_MAX_RETRY_DELAY
        );
        assert_eq!(retry_delay(1, None, 0), Duration::ZERO);
        assert!(retry_delay(1, None, u64::MAX) <= Duration::from_millis(50));
        assert!(retry_delay(2, None, u64::MAX) <= Duration::from_millis(100));
        assert_eq!(parse_retry_after("5"), Some(Duration::from_secs(5)));
    }

    #[tokio::test]
    async fn s3_client_never_replays_requests_across_redirects() {
        let target_hits = Arc::new(AtomicUsize::new(0));
        let target_app = axum::Router::new().fallback(axum::routing::any({
            let target_hits = target_hits.clone();
            move || {
                let target_hits = target_hits.clone();
                async move {
                    target_hits.fetch_add(1, Ordering::SeqCst);
                    reqwest::StatusCode::OK
                }
            }
        }));
        let target_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target_url = format!("http://{}", target_listener.local_addr().unwrap());
        tokio::spawn(async move {
            axum::serve(target_listener, target_app).await.unwrap();
        });

        let redirect_app = axum::Router::new().fallback(axum::routing::any(move || {
            let target_url = target_url.clone();
            async move {
                (
                    reqwest::StatusCode::TEMPORARY_REDIRECT,
                    [(reqwest::header::LOCATION, target_url)],
                )
            }
        }));
        let redirect_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let redirect_url = format!("http://{}", redirect_listener.local_addr().unwrap());
        tokio::spawn(async move {
            axum::serve(redirect_listener, redirect_app).await.unwrap();
        });

        let response = s3_http_client()
            .unwrap()
            .put(&redirect_url)
            .body("secret attachment")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::TEMPORARY_REDIRECT);
        assert_eq!(target_hits.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn retry_and_correction_error_classes_match_s3_boundaries() {
        let mut error = S3RequestError {
            operation: "GetObject",
            status: reqwest::StatusCode::BAD_REQUEST,
            code: Some("RequestTimeout".into()),
            request_id: Some("request-123".into()),
            host_id: Some("host-id-is-not-rendered".into()),
        };
        assert!(retryable_s3_failure(&error));
        error.code = Some("AccessDenied".into());
        assert!(!retryable_s3_failure(&error));
        assert_eq!(
            error.to_string(),
            "object storage GetObject failed with HTTP 400 Bad Request (AccessDenied) [request_id=request-123]"
        );
        assert!(!error.to_string().contains("host-id"));
        assert!(!format!("{error:?}").contains("host-id-is-not-rendered"));

        error.status = reqwest::StatusCode::MOVED_PERMANENTLY;
        assert!(is_region_redirect(&error));
        assert!(is_clock_skew_error(Some("RequestTimeTooSkewed")));
        assert!(!is_clock_skew_error(Some("AccessDenied")));
    }

    #[test]
    fn structured_s3_error_parser_ignores_messages_and_unsafe_identifiers() {
        let document = parse_s3_error_document(
            br#"<Error><Code>SlowDown</Code><Message>Authorization: secret</Message><RequestId>REQ123</RequestId><HostId>HOST456</HostId></Error>"#,
        );
        assert_eq!(document.code.as_deref(), Some("SlowDown"));
        assert_eq!(document.request_id.as_deref(), Some("REQ123"));
        assert_eq!(document.host_id.as_deref(), Some("HOST456"));
        assert!(
            safe_error_identifier(Some(&HeaderValue::from_static("unsafe value")), 256).is_none()
        );
    }

    #[test]
    fn aws_corrections_are_bounded_cached_and_never_touch_custom_endpoints() {
        let aws = test_s3("https://s3.us-west-2.amazonaws.com", false, false);
        assert!(aws.update_correction(Some(120), Some("eu-west-1".into())));
        let correction = aws.correction();
        assert_eq!(correction.clock_skew_seconds, 120);
        assert_eq!(correction.region.as_deref(), Some("eu-west-1"));
        let url = aws.request_url("workspaces/w/object").unwrap();
        assert_eq!(url.host_str(), Some("bucket.s3.eu-west-1.amazonaws.com"));
        assert!(!aws.update_correction(Some(S3_MAX_CLOCK_SKEW_SECONDS + 1), None));
        assert!(!aws.update_correction(None, Some("invalid/region".into())));

        let custom = test_s3("https://objects.example.test/base", true, true);
        assert!(!custom.update_correction(Some(120), Some("eu-west-1".into())));
        assert_eq!(custom.correction().clock_skew_seconds, 0);
        assert_eq!(
            custom.request_url("object").unwrap().as_str(),
            "https://objects.example.test/base/bucket/object"
        );
    }

    #[tokio::test]
    async fn replayable_requests_retry_at_most_three_attempts() {
        let (endpoint, attempts) = s3_status_server(vec![
            reqwest::StatusCode::SERVICE_UNAVAILABLE,
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            reqwest::StatusCode::OK,
        ])
        .await;
        let store = test_s3(&endpoint, true, true);
        let response = store
            .execute(reqwest::Method::GET, "object", None, HeaderMap::new())
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        assert_eq!(attempts.load(Ordering::SeqCst), S3_MAX_ATTEMPTS);
    }

    #[tokio::test]
    async fn non_replayable_streaming_put_is_never_retried() {
        let (endpoint, attempts) =
            s3_status_server(vec![reqwest::StatusCode::SERVICE_UNAVAILABLE]).await;
        let mut store = test_s3(&endpoint, true, true);
        store.custom_endpoint = false;
        let error = store
            .upload_stream(
                "object",
                Box::new(std::io::Cursor::new(b"stream".to_vec())),
                6,
                "application/octet-stream",
                "stream.bin",
            )
            .await
            .unwrap_err();
        let error = error.downcast_ref::<S3RequestError>().unwrap();
        assert_eq!(error.code.as_deref(), Some("SlowDown"));
        assert_eq!(error.request_id.as_deref(), Some("REQ123"));
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        assert_eq!(store.correction().region.as_deref(), Some("eu-central-1"));
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
            .signed_headers(S3SigningContext {
                method: "PUT",
                url: &url,
                payload_hash: &hex::encode(Sha256::digest(b"body")),
                now: chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
                extra: &extra,
                credentials: &test_credentials(None),
                region: &store.region,
            })
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
    fn s3_request_paths_use_sigv4_segment_encoding() {
        let virtual_host = test_s3("https://s3.us-west-2.amazonaws.com", false, false);
        assert_eq!(
            virtual_host
                .request_url("workspace/report.c++/%done")
                .unwrap()
                .path(),
            "/workspace/report.c%2B%2B/%25done"
        );

        let path_style = test_s3("https://minio.example.test/base", true, true);
        assert_eq!(
            path_style
                .request_url("workspace/report c++.txt")
                .unwrap()
                .path(),
            "/base/bucket/workspace/report%20c%2B%2B.txt"
        );
    }

    #[test]
    fn sigv4_canonical_request_covers_checksum_trailer_headers() {
        let store = test_s3("https://s3.us-west-2.amazonaws.com", false, true);
        let url = store.request_url("object").unwrap();
        let prepared = prepare_buffered_put(
            b"body".to_vec(),
            BufferedChecksumCapability::AwsHttpsTrailer,
        )
        .unwrap();
        let headers = store
            .signed_headers(S3SigningContext {
                method: "PUT",
                url: &url,
                payload_hash: &prepared.payload_hash,
                now: chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
                extra: &prepared.headers,
                credentials: &test_credentials(Some("temporary-session-token")),
                region: &store.region,
            })
            .unwrap();
        let authorization = headers
            .get(reqwest::header::AUTHORIZATION)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(authorization.contains(concat!(
            "SignedHeaders=host;x-amz-content-sha256;x-amz-date;",
            "x-amz-decoded-content-length;x-amz-sdk-checksum-algorithm;",
            "x-amz-security-token;x-amz-trailer"
        )));
    }

    #[test]
    fn sigv4_signs_temporary_credential_session_token() {
        let store = test_s3("https://s3.us-west-2.amazonaws.com", false, true);
        let url = store.request_url("object").unwrap();
        let headers = store
            .signed_headers(S3SigningContext {
                method: "GET",
                url: &url,
                payload_hash: &hex::encode(Sha256::digest([])),
                now: chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
                extra: &HeaderMap::new(),
                credentials: &test_credentials(Some("temporary-session-token")),
                region: &store.region,
            })
            .unwrap();
        assert_eq!(
            headers.get("x-amz-security-token").unwrap(),
            "temporary-session-token"
        );
        assert!(headers
            .get(reqwest::header::AUTHORIZATION)
            .unwrap()
            .to_str()
            .unwrap()
            .contains("SignedHeaders=host;x-amz-content-sha256;x-amz-date;x-amz-security-token"));
    }

    #[tokio::test]
    async fn missing_credentials_fail_before_the_storage_request() {
        let mut store = test_s3("http://127.0.0.1:9", true, true);
        store.credentials = SharedCredentialsProvider::new(MissingCredentials);
        let error = store
            .execute(reqwest::Method::GET, "object", None, HeaderMap::new())
            .await
            .unwrap_err();
        assert!(error.to_string().starts_with("resolve AWS credentials:"));
    }

    #[tokio::test]
    async fn s3_presign_get_covers_ttl_base_path_and_session_token() {
        let mut store = test_s3("https://objects.example.test/base", true, true);
        store.credentials = SharedCredentialsProvider::new(test_credentials(Some("session/token")));
        let signed = store
            .presign_get(
                "workspaces/w/a file.txt",
                std::time::Duration::from_secs(300),
            )
            .await
            .unwrap();
        let url = Url::parse(&signed).unwrap();
        assert_eq!(url.path(), "/base/bucket/workspaces/w/a%20file.txt");
        let query = url
            .query_pairs()
            .collect::<std::collections::HashMap<_, _>>();
        assert_eq!(
            query.get("X-Amz-Expires").map(|value| value.as_ref()),
            Some("300")
        );
        assert_eq!(
            query
                .get("X-Amz-Security-Token")
                .map(|value| value.as_ref()),
            Some("session/token")
        );
        assert!(!query.get("X-Amz-Signature").unwrap().is_empty());
    }

    #[tokio::test]
    async fn s3_presign_rejects_subsecond_and_overlong_ttls() {
        let store = test_s3("https://s3.us-west-2.amazonaws.com", false, true);
        for ttl in [
            Duration::from_millis(500),
            Duration::from_secs(7 * 24 * 60 * 60 + 1),
        ] {
            let error = store.presign_get("object", ttl).await.unwrap_err();
            assert_eq!(
                error.to_string(),
                "S3 presigned download TTL must be between 1 second and 7 days"
            );
        }
    }

    #[tokio::test]
    async fn s3_presign_binds_content_disposition_and_virtual_host_scope() {
        let store = test_s3("https://objects.example.test/base", true, false);
        let signed = store
            .presign_get_with_content_disposition(
                "workspaces/w/report.txt",
                std::time::Duration::from_secs(60),
                "attachment; filename=\"report 1.txt\"",
            )
            .await
            .unwrap();
        let url = Url::parse(&signed).unwrap();
        assert_eq!(url.host_str(), Some("bucket.objects.example.test"));
        assert_eq!(url.path(), "/base/workspaces/w/report.txt");
        assert_eq!(
            url.query_pairs()
                .find(|(name, _)| name == "response-content-disposition")
                .map(|(_, value)| value.into_owned())
                .as_deref(),
            Some("attachment; filename=\"report 1.txt\"")
        );
    }

    #[test]
    fn s3_key_scope_rejects_foreign_or_credential_bearing_urls() {
        let store = test_s3("https://objects.example.test/base", true, true);
        assert_eq!(
            store
                .key_from_url("https://objects.example.test/base/bucket/workspaces/w/file.txt")
                .as_deref(),
            Some("workspaces/w/file.txt")
        );
        assert!(store
            .key_from_url("https://attacker.example/base/bucket/workspaces/w/file.txt")
            .is_none());
        assert!(store
            .key_from_url(
                "https://objects.example.test/base/bucket/workspaces/w/file.txt?X-Amz-Signature=x"
            )
            .is_none());
        assert!(store
            .key_from_url("https://objects.example.test/base/bucket/workspaces%2Fw/file.txt")
            .is_none());
    }

    #[test]
    fn s3_key_scope_accepts_bounded_aws_region_redirect_urls() {
        let store = test_s3("https://s3.us-west-2.amazonaws.com", false, false);
        assert_eq!(
            store
                .key_from_url(
                    "https://bucket.s3.eu-central-1.amazonaws.com/workspaces/w/file%2B.txt"
                )
                .as_deref(),
            Some("workspaces/w/file+.txt")
        );
        assert_eq!(
            store
                .key_from_url("https://bucket.s3.cn-north-1.amazonaws.com.cn/workspaces/w/file.txt")
                .as_deref(),
            Some("workspaces/w/file.txt")
        );
        assert_eq!(
            store
                .key_from_url(
                    "https://s3.cn-northwest-1.amazonaws.com.cn/bucket/workspaces/w/file.txt"
                )
                .as_deref(),
            Some("workspaces/w/file.txt")
        );
        assert!(store
            .key_from_url("https://bucket.s3.cn-north-1.amazonaws.com/workspaces/w/file.txt")
            .is_none());
        assert!(store
            .key_from_url("https://bucket.s3.eu-central-1.amazonaws.com.cn/workspaces/w/file.txt")
            .is_none());
        assert!(store
            .key_from_url(
                "https://test-bucket.s3.evil.amazonaws.com.attacker.test/workspaces/w/file.txt"
            )
            .is_none());
        assert!(store
            .key_from_url(
                "https://other-bucket.s3.eu-central-1.amazonaws.com/workspaces/w/file.txt"
            )
            .is_none());
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
