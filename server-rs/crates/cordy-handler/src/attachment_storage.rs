use async_trait::async_trait;
use aws_config::{default_provider::credentials::DefaultCredentialsChain, Region};
use aws_credential_types::{
    provider::{ProvideCredentials, SharedCredentialsProvider},
    Credentials,
};
use axum::body::Body;
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use hmac::{Hmac, Mac};
use reqwest::header::{HeaderMap, HeaderValue};
use sha2::{Digest, Sha256};
use std::{
    path::{Component, Path, PathBuf},
    sync::Arc,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio_util::io::ReaderStream;
use url::Url;

type HmacSha256 = Hmac<Sha256>;
pub const MAX_STREAM_UPLOAD_BYTES: i64 = 100 << 20;
const STREAMING_UNSIGNED_PAYLOAD_TRAILER: &str = "STREAMING-UNSIGNED-PAYLOAD-TRAILER";
const UNSIGNED_PAYLOAD: &str = "UNSIGNED-PAYLOAD";
const CRC32_TRAILER: &str = "x-amz-checksum-crc32";

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
        let result = async {
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
            anyhow::Ok(())
        }
        .await;
        if let Err(error) = result {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(error);
        }
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
        let endpoint = Url::parse(
            custom
                .as_deref()
                .unwrap_or(&format!("https://s3.{region}.amazonaws.com")),
        )?;
        anyhow::ensure!(
            matches!(endpoint.scheme(), "http" | "https")
                && endpoint.host_str().is_some()
                && endpoint.username().is_empty()
                && endpoint.password().is_none()
                && endpoint.fragment().is_none(),
            "AWS_ENDPOINT_URL must be an HTTP(S) origin without credentials or a fragment"
        );
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
            client: reqwest::Client::new(),
            bucket,
            region,
            endpoint,
            custom_endpoint: custom.is_some(),
            path_style,
            credentials,
            cdn_domain: env("CLOUDFRONT_DOMAIN"),
        }))
    }
    fn request_url(&self, key: &str) -> anyhow::Result<Url> {
        if key.is_empty() || key.split('/').any(|v| v == "." || v == "..") {
            anyhow::bail!("invalid storage key")
        }
        let mut url = self.endpoint.clone();
        let base_path = url.path().trim_end_matches('/').to_string();
        let path_style = self.path_style || (!self.custom_endpoint && self.bucket.contains('.'));
        if path_style {
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
        credentials: &Credentials,
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
        let scope = format!("{date}/{}/s3/aws4_request", self.region);
        let to_sign = format!(
            "AWS4-HMAC-SHA256\n{timestamp}\n{scope}\n{}",
            hex::encode(Sha256::digest(canonical_request.as_bytes()))
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
        let url = self.request_url(key)?;
        let credentials = self
            .credentials
            .provide_credentials()
            .await
            .map_err(|error| anyhow::anyhow!("resolve AWS credentials: {error}"))?;
        let mut headers = self.signed_headers(
            method.as_str(),
            &url,
            payload_hash,
            chrono::Utc::now(),
            &extra,
            &credentials,
        )?;
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

    async fn execute_stream(
        &self,
        key: &str,
        body: Box<dyn AsyncRead + Send + Unpin>,
        size_bytes: i64,
        mut extra: HeaderMap,
    ) -> anyhow::Result<()> {
        validate_stream_size(size_bytes)?;
        let url = self.request_url(key)?;
        let credentials = self
            .credentials
            .provide_credentials()
            .await
            .map_err(|error| anyhow::anyhow!("resolve AWS credentials: {error}"))?;
        let mut headers = self.signed_headers(
            "PUT",
            &url,
            UNSIGNED_PAYLOAD,
            chrono::Utc::now(),
            &extra,
            &credentials,
        )?;
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
            .await?;
        anyhow::ensure!(
            response.status().is_success(),
            "object storage returned {}",
            response.status()
        );
        Ok(())
    }

    async fn presigned_get(
        &self,
        key: &str,
        ttl: std::time::Duration,
        content_disposition: &str,
    ) -> anyhow::Result<String> {
        let expires = if ttl.is_zero() {
            30 * 60
        } else {
            ttl.as_secs()
        };
        anyhow::ensure!(
            (1..=7 * 24 * 60 * 60).contains(&expires),
            "S3 presigned download TTL must be between 1 second and 7 days"
        );
        let mut url = self.request_url(key)?;
        let credentials = self
            .credentials
            .provide_credentials()
            .await
            .map_err(|error| anyhow::anyhow!("resolve AWS credentials: {error}"))?;
        let now = chrono::Utc::now();
        let date = now.format("%Y%m%d").to_string();
        let timestamp = now.format("%Y%m%dT%H%M%SZ").to_string();
        let scope = format!("{date}/{}/s3/aws4_request", self.region);
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
        let k_region = hmac_bytes(&k_date, self.region.as_bytes())?;
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
            client: reqwest::Client::new(),
            bucket: "bucket".into(),
            region: "us-west-2".into(),
            endpoint: Url::parse("https://s3.us-west-2.amazonaws.com").unwrap(),
            custom_endpoint: false,
            path_style: false,
            credentials: SharedCredentialsProvider::new(test_credentials(None)),
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
            credentials: SharedCredentialsProvider::new(test_credentials(None)),
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
                &test_credentials(None),
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
    fn sigv4_canonical_request_covers_checksum_trailer_headers() {
        let store = test_s3("https://s3.us-west-2.amazonaws.com", false, true);
        let url = store.request_url("object").unwrap();
        let prepared = prepare_buffered_put(
            b"body".to_vec(),
            BufferedChecksumCapability::AwsHttpsTrailer,
        )
        .unwrap();
        let headers = store
            .signed_headers(
                "PUT",
                &url,
                &prepared.payload_hash,
                chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
                &prepared.headers,
                &test_credentials(Some("temporary-session-token")),
            )
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
            .signed_headers(
                "GET",
                &url,
                &hex::encode(Sha256::digest([])),
                chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
                &HeaderMap::new(),
                &test_credentials(Some("temporary-session-token")),
            )
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
