//! Production daemon self-update and restart handoff primitives.
//!
//! This is the Rust counterpart of `internal/cli/update*.go` and
//! `internal/selfexec`. It deliberately owns binary replacement rather than
//! leaving an optional provider hook which could report a false success.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use flate2::read::GzDecoder;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::auto_update::is_release_version;

const CHECKSUM_MANIFEST: &str = "checksums.txt";
const GITHUB_USER_AGENT: &str = concat!("patchbay-daemon/", env!("CARGO_PKG_VERSION"));
const METADATA_TIMEOUT: Duration = Duration::from_secs(10);
pub const DEFAULT_UPDATE_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(120);
const DOWNLOAD_TIMEOUT: Duration = DEFAULT_UPDATE_DOWNLOAD_TIMEOUT;
const MAX_ARCHIVE_BYTES: usize = 256 * 1024 * 1024;
const MAX_BINARY_BYTES: usize = 128 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateFailureKind {
    ResolveExecutable,
    DetectInstall,
    InvalidVersion,
    InvalidTimeout,
    Metadata,
    AssetSelection,
    Download,
    Checksum,
    Extract,
    Permission,
    Install,
    Homebrew,
}

impl UpdateFailureKind {
    pub fn code(self) -> &'static str {
        match self {
            Self::ResolveExecutable => "resolve-executable",
            Self::DetectInstall => "detect-install",
            Self::InvalidVersion => "invalid-version",
            Self::InvalidTimeout => "invalid-timeout",
            Self::Metadata => "metadata",
            Self::AssetSelection => "asset-selection",
            Self::Download => "download",
            Self::Checksum => "checksum",
            Self::Extract => "extract",
            Self::Permission => "permission",
            Self::Install => "install",
            Self::Homebrew => "homebrew",
        }
    }
}

#[derive(Debug)]
pub struct UpdateExecutorError {
    pub kind: UpdateFailureKind,
    message: String,
}

impl UpdateExecutorError {
    fn new(kind: UpdateFailureKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

impl fmt::Display for UpdateExecutorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}", self.kind.code(), self.message)
    }
}

impl std::error::Error for UpdateExecutorError {}

type Result<T> = std::result::Result<T, UpdateExecutorError>;

/// Typed input for the daemon's self-update operation. `target_version =
/// None` means resolve the latest GitHub release.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UpdateRequest {
    pub target_version: Option<String>,
    pub current_version: Option<String>,
    pub download_timeout: Option<Duration>,
}

impl UpdateRequest {
    pub fn latest() -> Self {
        Self::default()
    }

    pub fn for_target(target_version: impl Into<String>) -> Self {
        Self {
            target_version: Some(target_version.into()),
            ..Self::default()
        }
    }

    pub fn with_download_timeout(mut self, timeout: Duration) -> Self {
        self.download_timeout = Some(timeout);
        self
    }

    pub fn with_current_version(mut self, current_version: impl Into<String>) -> Self {
        self.current_version = Some(current_version.into());
        self
    }
}

/// Installation method reported by the typed update boundary. It intentionally
/// does not expose the executor's private paths or command details. `Homebrew`
/// remains for caller compatibility, but the executor now migrates every
/// installation to the maintained direct GitHub Release path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateInstallMethod {
    Direct,
    Homebrew,
}

/// Safe update result for callers such as the future CLI facade. Messages are
/// deliberately path-free; callers can render them without leaking the
/// running binary location or release URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateOutcome {
    pub method: UpdateInstallMethod,
    pub resolved_version: Option<String>,
    pub already_current: bool,
    pub latest_query_failed: bool,
    pub message: String,
}

#[derive(Debug)]
pub struct UpdateExecutor {
    executable: PathBuf,
    metadata_client: reqwest::Client,
    download_client: reqwest::Client,
}

#[derive(Debug, Deserialize)]
struct Release {
    #[serde(rename = "tag_name")]
    tag_name: String,
    assets: Vec<ReleaseAsset>,
}

#[derive(Debug, Deserialize)]
struct ReleaseAsset {
    name: String,
    #[serde(rename = "browser_download_url")]
    download_url: String,
}

impl UpdateExecutor {
    #[cfg(test)]
    pub(crate) fn direct_for_test(executable: PathBuf) -> Self {
        Self {
            executable,
            metadata_client: http_client(METADATA_TIMEOUT)
                .expect("build update metadata client for test"),
            download_client: http_client(DOWNLOAD_TIMEOUT)
                .expect("build update download client for test"),
        }
    }

    /// Resolves the running inode. All installations, including binaries left
    /// in a Homebrew Cellar by the retired tap, update from GitHub Releases.
    pub async fn detect() -> Result<Self> {
        let executable = resolve_executable()?;
        cleanup_stale_update_artifacts(&executable);
        let resolved = fs::canonicalize(&executable).map_err(|err| {
            UpdateExecutorError::new(
                UpdateFailureKind::ResolveExecutable,
                format!("resolve executable symlink: {err}"),
            )
        })?;
        let metadata_client = http_client(METADATA_TIMEOUT)?;
        let download_client = http_client(DOWNLOAD_TIMEOUT)?;
        Ok(Self {
            executable: resolved,
            metadata_client,
            download_client,
        })
    }

    pub fn restart_target_binary(&self) -> &Path {
        &self.executable
    }

    /// Executes a typed update request against GitHub Releases.
    pub async fn update_with_request(
        &self,
        request: UpdateRequest,
    ) -> anyhow::Result<UpdateOutcome> {
        let timeout =
            validate_download_timeout(request.download_timeout.unwrap_or(DOWNLOAD_TIMEOUT))?;

        let requested_version = request.target_version.as_deref();
        let current_version = request.current_version.as_deref();
        if let Some(target) = requested_version {
            validate_target_version(target)?;
            if current_version.is_some_and(|current| same_release_version(target, current)) {
                return Ok(already_current_outcome(
                    UpdateInstallMethod::Direct,
                    Some(normalize_release_tag(target)),
                ));
            }
        }

        let target = match request.target_version {
            Some(target) => normalize_release_tag(&target),
            None => self.fetch_latest_release_tag().await?,
        };
        if current_version.is_some_and(|current| same_release_version(&target, current)) {
            return Ok(already_current_outcome(
                UpdateInstallMethod::Direct,
                Some(target),
            ));
        }
        let message = self
            .update_direct_with_timeout(&target, timeout)
            .await
            .map_err(anyhow::Error::from)?;
        Ok(UpdateOutcome {
            method: UpdateInstallMethod::Direct,
            resolved_version: Some(target),
            already_current: false,
            latest_query_failed: false,
            message,
        })
    }

    /// Alias kept as the concise facade entry point for CLI/service callers.
    pub async fn update_request(&self, request: UpdateRequest) -> anyhow::Result<UpdateOutcome> {
        self.update_with_request(request).await
    }

    pub async fn update(&self, target_version: &str) -> anyhow::Result<String> {
        Ok(self
            .update_with_request(UpdateRequest::for_target(target_version))
            .await?
            .message)
    }

    /// Compatibility facade for callers that supply a bounded download
    /// timeout while still targeting an explicit release.
    pub async fn update_with_timeout(
        &self,
        target_version: &str,
        download_timeout: Duration,
    ) -> anyhow::Result<String> {
        Ok(self
            .update_with_request(UpdateRequest {
                target_version: Some(target_version.to_string()),
                current_version: None,
                download_timeout: Some(download_timeout),
            })
            .await?
            .message)
    }

    async fn fetch_latest_release_tag(&self) -> Result<String> {
        let endpoint = "https://api.github.com/repos/patchbay-ai/patchbay/releases/latest";
        let response = self
            .metadata_client
            .get(endpoint)
            .header(reqwest::header::ACCEPT, "application/vnd.github+json")
            .send()
            .await
            .map_err(|err| {
                network_error(UpdateFailureKind::Metadata, "fetch latest release", &err)
            })?;
        if !response.status().is_success() {
            return Err(UpdateExecutorError::new(
                UpdateFailureKind::Metadata,
                format!(
                    "GitHub latest-release API returned HTTP {}",
                    response.status().as_u16()
                ),
            ));
        }
        let release: Release = response.json().await.map_err(|err| {
            network_error(UpdateFailureKind::Metadata, "decode latest release", &err)
        })?;
        release_tag(&release.tag_name)
    }

    async fn update_direct_with_timeout(
        &self,
        target_version: &str,
        download_timeout: Duration,
    ) -> Result<String> {
        let tag = normalize_release_tag(target_version);
        let endpoint =
            format!("https://api.github.com/repos/patchbay-ai/patchbay/releases/tags/{tag}");
        let response = self
            .metadata_client
            .get(endpoint)
            .header(reqwest::header::ACCEPT, "application/vnd.github+json")
            .send()
            .await
            .map_err(|err| {
                network_error(UpdateFailureKind::Metadata, "fetch release metadata", &err)
            })?;
        if !response.status().is_success() {
            return Err(UpdateExecutorError::new(
                UpdateFailureKind::Metadata,
                format!(
                    "GitHub release API returned HTTP {}",
                    response.status().as_u16()
                ),
            ));
        }
        let release: Release = response.json().await.map_err(|err| {
            network_error(UpdateFailureKind::Metadata, "decode release metadata", &err)
        })?;
        if release.tag_name.trim() != tag {
            return Err(UpdateExecutorError::new(
                UpdateFailureKind::Metadata,
                "release metadata tag does not match the requested version",
            ));
        }

        let candidates = release_asset_candidates(target_version)?;
        let asset = candidates
            .iter()
            .find_map(|candidate| release.assets.iter().find(|asset| asset.name == *candidate))
            .ok_or_else(|| {
                UpdateExecutorError::new(
                    UpdateFailureKind::AssetSelection,
                    format!(
                        "release has no matching asset (tried {})",
                        candidates.join(", ")
                    ),
                )
            })?;
        let manifest = release
            .assets
            .iter()
            .find(|asset| asset.name == CHECKSUM_MANIFEST)
            .ok_or_else(|| {
                UpdateExecutorError::new(
                    UpdateFailureKind::AssetSelection,
                    "release has no checksums.txt asset",
                )
            })?;

        let manifest_bytes = self
            .fetch_asset_with_timeout(manifest, 2 * 1024 * 1024, download_timeout)
            .await?;
        let expected = checksum_for_asset(&manifest_bytes, &asset.name)?;
        let archive = self
            .fetch_asset_with_timeout(asset, MAX_ARCHIVE_BYTES, download_timeout)
            .await?;
        verify_checksum(&archive, &expected, &asset.name)?;
        let binary = extract_binary(&archive)?;
        install_binary(&self.executable, &binary)?;
        Ok(format!(
            "Downloaded {} and replaced the current executable",
            asset.name
        ))
    }

    async fn fetch_asset_with_timeout(
        &self,
        asset: &ReleaseAsset,
        limit: usize,
        timeout: Duration,
    ) -> Result<Vec<u8>> {
        validate_download_url(&asset.download_url)?;
        let mut response = self
            .download_client
            .get(&asset.download_url)
            .timeout(timeout)
            .send()
            .await
            .map_err(|err| {
                network_error(UpdateFailureKind::Download, "download release asset", &err)
            })?;
        if !response.status().is_success() {
            return Err(UpdateExecutorError::new(
                UpdateFailureKind::Download,
                format!(
                    "download of {} returned HTTP {}",
                    asset.name,
                    response.status().as_u16()
                ),
            ));
        }
        if response
            .content_length()
            .is_some_and(|length| length > limit as u64)
        {
            return Err(UpdateExecutorError::new(
                UpdateFailureKind::Download,
                format!("download of {} exceeds the size limit", asset.name),
            ));
        }
        let capacity = response
            .content_length()
            .and_then(|length| usize::try_from(length).ok())
            .unwrap_or(0)
            .min(limit);
        let mut bytes = Vec::with_capacity(capacity);
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|err| network_error(UpdateFailureKind::Download, "read release asset", &err))?
        {
            append_download_chunk(&mut bytes, &chunk, limit, &asset.name)?;
        }
        Ok(bytes)
    }
}

fn append_download_chunk(
    destination: &mut Vec<u8>,
    chunk: &[u8],
    limit: usize,
    asset_name: &str,
) -> Result<()> {
    if chunk.len() > limit.saturating_sub(destination.len()) {
        return Err(UpdateExecutorError::new(
            UpdateFailureKind::Download,
            format!("download of {asset_name} exceeds the size limit"),
        ));
    }
    destination.extend_from_slice(chunk);
    Ok(())
}

fn http_client(timeout: Duration) -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(timeout)
        .user_agent(GITHUB_USER_AGENT)
        .build()
        .map_err(|err| {
            classified_io(
                UpdateFailureKind::DetectInstall,
                "build update HTTP client",
                err,
            )
        })
}

fn classified_io(
    kind: UpdateFailureKind,
    operation: &str,
    err: impl fmt::Display,
) -> UpdateExecutorError {
    // Do not include request URLs or response bodies: release URLs can contain
    // signed query parameters and command output can include local paths.
    UpdateExecutorError::new(kind, format!("{operation}: {err}"))
}

fn network_error(
    kind: UpdateFailureKind,
    operation: &str,
    error: &reqwest::Error,
) -> UpdateExecutorError {
    // reqwest's Display includes the complete URL, including signed query
    // parameters. Preserve useful failure class/status without formatting it.
    let detail = if error.is_timeout() {
        "request timed out".to_string()
    } else if error.is_connect() {
        "connection failed".to_string()
    } else if let Some(status) = error.status() {
        format!("HTTP {}", status.as_u16())
    } else if error.is_decode() {
        "response decode failed".to_string()
    } else {
        "request failed".to_string()
    };
    UpdateExecutorError::new(kind, format!("{operation}: {detail}"))
}

fn normalize_release_tag(version: &str) -> String {
    let version = version.trim();
    if version.starts_with('v') {
        version.to_string()
    } else {
        format!("v{version}")
    }
}

fn validate_target_version(version: &str) -> Result<()> {
    if !is_release_version(version) {
        return Err(UpdateExecutorError::new(
            UpdateFailureKind::InvalidVersion,
            "update target is not a three-component release version",
        ));
    }
    Ok(())
}

fn same_release_version(left: &str, right: &str) -> bool {
    normalize_release_tag(left) == normalize_release_tag(right)
}

fn already_current_outcome(
    method: UpdateInstallMethod,
    resolved_version: Option<String>,
) -> UpdateOutcome {
    UpdateOutcome {
        method,
        resolved_version,
        already_current: true,
        latest_query_failed: false,
        message: "Already up to date.".to_string(),
    }
}

fn validate_download_timeout(timeout: Duration) -> Result<Duration> {
    if timeout.is_zero() {
        return Err(UpdateExecutorError::new(
            UpdateFailureKind::InvalidTimeout,
            "download timeout must be greater than zero",
        ));
    }
    Ok(timeout)
}

fn release_tag(raw_tag: &str) -> Result<String> {
    validate_target_version(raw_tag)?;
    Ok(normalize_release_tag(raw_tag))
}

fn release_asset_candidates(version: &str) -> Result<Vec<String>> {
    let version = version.trim().trim_start_matches('v');
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        "linux" => "linux",
        "windows" => "windows",
        other => {
            return Err(UpdateExecutorError::new(
                UpdateFailureKind::AssetSelection,
                format!("self-update is not supported on {other}"),
            ));
        }
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        other => {
            return Err(UpdateExecutorError::new(
                UpdateFailureKind::AssetSelection,
                format!("self-update is not supported on architecture {other}"),
            ));
        }
    };
    let extension = if cfg!(windows) { "zip" } else { "tar.gz" };
    Ok(vec![
        format!("patchbay-cli-{version}-{os}-{arch}.{extension}"),
        format!("patchbay_{os}_{arch}.{extension}"),
    ])
}

fn validate_download_url(raw: &str) -> Result<()> {
    let url = reqwest::Url::parse(raw).map_err(|_| {
        UpdateExecutorError::new(UpdateFailureKind::Download, "release asset URL is invalid")
    })?;
    let trusted_host = url
        .host_str()
        .is_some_and(|host| host == "github.com" || host.ends_with(".githubusercontent.com"));
    if url.scheme() != "https" || !trusted_host || url.username() != "" || url.password().is_some()
    {
        return Err(UpdateExecutorError::new(
            UpdateFailureKind::Download,
            "release asset URL is not a trusted HTTPS GitHub URL",
        ));
    }
    Ok(())
}

fn checksum_for_asset(manifest: &[u8], asset_name: &str) -> Result<String> {
    let text = std::str::from_utf8(manifest).map_err(|_| {
        UpdateExecutorError::new(
            UpdateFailureKind::Checksum,
            "checksum manifest is not UTF-8",
        )
    })?;
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut fields = line.split_whitespace();
        let (Some(checksum), Some(name)) = (fields.next(), fields.next()) else {
            continue;
        };
        if name == asset_name {
            if checksum.len() != 64 || !checksum.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err(UpdateExecutorError::new(
                    UpdateFailureKind::Checksum,
                    format!("checksum for {asset_name} is not a SHA-256 digest"),
                ));
            }
            return Ok(checksum.to_ascii_lowercase());
        }
    }
    Err(UpdateExecutorError::new(
        UpdateFailureKind::Checksum,
        format!("checksum for {asset_name} is absent from the manifest"),
    ))
}

fn verify_checksum(data: &[u8], expected: &str, asset_name: &str) -> Result<()> {
    let actual = hex::encode(Sha256::digest(data));
    if actual != expected {
        return Err(UpdateExecutorError::new(
            UpdateFailureKind::Checksum,
            format!("SHA-256 verification failed for {asset_name}"),
        ));
    }
    Ok(())
}

fn extract_binary(archive: &[u8]) -> Result<Vec<u8>> {
    if cfg!(windows) {
        extract_zip(archive, binary_name())
    } else {
        extract_tar_gz(archive, binary_name())
    }
}

fn extract_zip(archive: &[u8], wanted: &str) -> Result<Vec<u8>> {
    let mut zip = zip::ZipArchive::new(Cursor::new(archive))
        .map_err(|err| classified_io(UpdateFailureKind::Extract, "open zip archive", err))?;
    for index in 0..zip.len() {
        let mut entry = zip
            .by_index(index)
            .map_err(|err| classified_io(UpdateFailureKind::Extract, "read zip entry", err))?;
        if !entry.is_dir() && Path::new(entry.name()).file_name() == Some(OsStr::new(wanted)) {
            if entry.size() > MAX_BINARY_BYTES as u64 {
                return Err(UpdateExecutorError::new(
                    UpdateFailureKind::Extract,
                    "binary in release archive exceeds the size limit",
                ));
            }
            return read_bounded(&mut entry, MAX_BINARY_BYTES);
        }
    }
    Err(binary_missing(wanted))
}

fn extract_tar_gz(archive: &[u8], wanted: &str) -> Result<Vec<u8>> {
    let mut tar = GzDecoder::new(archive);
    let mut header = [0_u8; 512];
    loop {
        tar.read_exact(&mut header)
            .map_err(|err| classified_io(UpdateFailureKind::Extract, "read tar header", err))?;
        if header.iter().all(|byte| *byte == 0) {
            return Err(binary_missing(wanted));
        }
        let size = parse_tar_size(&header[124..136])?;
        let name = tar_name(&header);
        let is_regular = matches!(header[156], 0 | b'0');
        if is_regular && Path::new(&name).file_name() == Some(OsStr::new(wanted)) {
            if size > MAX_BINARY_BYTES {
                return Err(UpdateExecutorError::new(
                    UpdateFailureKind::Extract,
                    "binary in release archive exceeds the size limit",
                ));
            }
            let mut binary = vec![0; size];
            tar.read_exact(&mut binary).map_err(|err| {
                classified_io(
                    UpdateFailureKind::Extract,
                    "read binary from tar archive",
                    err,
                )
            })?;
            return Ok(binary);
        }
        let padded = size
            .checked_add(511)
            .map(|value| value / 512 * 512)
            .ok_or_else(|| {
                UpdateExecutorError::new(UpdateFailureKind::Extract, "tar entry size overflow")
            })?;
        let copied = std::io::copy(
            &mut Read::by_ref(&mut tar).take(padded as u64),
            &mut std::io::sink(),
        )
        .map_err(|err| classified_io(UpdateFailureKind::Extract, "skip tar entry", err))?;
        if copied != padded as u64 {
            return Err(UpdateExecutorError::new(
                UpdateFailureKind::Extract,
                "release tar archive is truncated",
            ));
        }
    }
}

fn parse_tar_size(field: &[u8]) -> Result<usize> {
    let value = std::str::from_utf8(field)
        .map_err(|_| UpdateExecutorError::new(UpdateFailureKind::Extract, "invalid tar size"))?
        .trim_matches(|ch: char| ch == '\0' || ch.is_ascii_whitespace());
    usize::from_str_radix(value, 8)
        .map_err(|_| UpdateExecutorError::new(UpdateFailureKind::Extract, "invalid tar size"))
}

fn tar_name(header: &[u8; 512]) -> String {
    fn field(bytes: &[u8]) -> String {
        String::from_utf8_lossy(bytes.split(|byte| *byte == 0).next().unwrap_or_default())
            .into_owned()
    }
    let name = field(&header[..100]);
    let prefix = field(&header[345..500]);
    if prefix.is_empty() {
        name
    } else {
        format!("{prefix}/{name}")
    }
}

fn read_bounded(reader: &mut impl Read, limit: usize) -> Result<Vec<u8>> {
    let mut data = Vec::new();
    reader
        .take(limit as u64 + 1)
        .read_to_end(&mut data)
        .map_err(|err| {
            classified_io(UpdateFailureKind::Extract, "read binary from archive", err)
        })?;
    if data.len() > limit {
        return Err(UpdateExecutorError::new(
            UpdateFailureKind::Extract,
            "binary in release archive exceeds the size limit",
        ));
    }
    Ok(data)
}

fn binary_missing(wanted: &str) -> UpdateExecutorError {
    UpdateExecutorError::new(
        UpdateFailureKind::Extract,
        format!("binary {wanted} is absent from the release archive"),
    )
}

fn install_binary(executable: &Path, binary: &[u8]) -> Result<()> {
    let directory = executable.parent().ok_or_else(|| {
        UpdateExecutorError::new(
            UpdateFailureKind::Install,
            "executable has no parent directory",
        )
    })?;
    let permissions = fs::metadata(executable)
        .map_err(|err| {
            classified_io(
                UpdateFailureKind::Permission,
                "stat running executable",
                err,
            )
        })?
        .permissions();
    let mut temporary = tempfile::Builder::new()
        .prefix("patchbay-update-")
        .tempfile_in(directory)
        .map_err(|err| classified_io(UpdateFailureKind::Install, "create update file", err))?;
    temporary
        .write_all(binary)
        .and_then(|()| temporary.flush())
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(|err| classified_io(UpdateFailureKind::Install, "write update file", err))?;
    temporary
        .as_file()
        .set_permissions(permissions)
        .map_err(|err| {
            classified_io(
                UpdateFailureKind::Permission,
                "set update file permissions",
                err,
            )
        })?;
    let temporary_path = temporary.into_temp_path().keep().map_err(|err| {
        classified_io(UpdateFailureKind::Install, "retain update file", err.error)
    })?;
    let result = replace_binary(&temporary_path, executable);
    if result.is_err() {
        let _ = fs::remove_file(&temporary_path);
    }
    result
}

#[cfg(not(windows))]
fn replace_binary(temporary: &Path, executable: &Path) -> Result<()> {
    fs::rename(temporary, executable).map_err(|err| {
        classified_io(
            UpdateFailureKind::Install,
            "replace running executable",
            err,
        )
    })
}

#[cfg(windows)]
fn replace_binary(temporary: &Path, executable: &Path) -> Result<()> {
    let old = PathBuf::from(format!("{}.old", executable.display()));
    let _ = fs::remove_file(&old);
    fs::rename(executable, &old).map_err(|err| {
        classified_io(
            UpdateFailureKind::Install,
            "move running executable aside",
            err,
        )
    })?;
    if let Err(install_error) = fs::rename(temporary, executable) {
        if let Err(restore_error) = fs::rename(&old, executable) {
            return Err(UpdateExecutorError::new(
                UpdateFailureKind::Install,
                format!("install new executable: {install_error}; restore previous executable: {restore_error}"),
            ));
        }
        return Err(classified_io(
            UpdateFailureKind::Install,
            "install new executable",
            install_error,
        ));
    }
    Ok(())
}

fn resolve_executable() -> Result<PathBuf> {
    if let Ok(path) = std::env::current_exe() {
        return validate_executable(path);
    }
    let argv0 = std::env::args_os()
        .next()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            UpdateExecutorError::new(UpdateFailureKind::ResolveExecutable, "argv[0] is empty")
        })?;
    let candidate = if Path::new(&argv0).components().count() > 1 {
        std::env::current_dir()
            .map_err(|err| {
                classified_io(
                    UpdateFailureKind::ResolveExecutable,
                    "read current directory",
                    err,
                )
            })?
            .join(argv0)
    } else {
        find_on_path(&argv0).ok_or_else(|| {
            UpdateExecutorError::new(
                UpdateFailureKind::ResolveExecutable,
                "executable is absent from PATH",
            )
        })?
    };
    validate_executable(candidate)
}

fn validate_executable(path: PathBuf) -> Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .map_err(|err| {
                classified_io(
                    UpdateFailureKind::ResolveExecutable,
                    "read current directory",
                    err,
                )
            })?
            .join(path)
    };
    let metadata = fs::metadata(&absolute).map_err(|err| {
        classified_io(UpdateFailureKind::ResolveExecutable, "stat executable", err)
    })?;
    if !metadata.is_file() {
        return Err(UpdateExecutorError::new(
            UpdateFailureKind::ResolveExecutable,
            "resolved executable is not a regular file",
        ));
    }
    Ok(absolute)
}

fn find_on_path(name: &OsStr) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?).find_map(|directory| {
        let candidate = directory.join(name);
        candidate.is_file().then_some(candidate)
    })
}

fn binary_name() -> &'static str {
    if cfg!(windows) {
        "patchbay.exe"
    } else {
        "patchbay"
    }
}

#[cfg(not(windows))]
fn cleanup_stale_update_artifacts(_executable: &Path) {}

#[cfg(windows)]
fn cleanup_stale_update_artifacts(executable: &Path) {
    let _ = fs::remove_file(format!("{}.old", executable.display()));
}

/// Builds the detached successor command used after graceful daemon drain.
/// The binary and argv remain separate OS strings; callers add bounded log
/// file handles and perform the PID-file/bootstrap handoff.
pub fn restart_command(binary: &Path, args: &[OsString]) -> Command {
    let mut command = Command::new(binary);
    command.args(args);
    configure_detached(&mut command);
    command
}

/// Builds the Windows access-denied retry without BREAKAWAY_FROM_JOB. On
/// Unix this is identical to [`restart_command`].
pub fn restart_command_after_access_denied(binary: &Path, args: &[OsString]) -> Command {
    let mut command = Command::new(binary);
    command.args(args);
    configure_detached_retry(&mut command);
    command
}

#[cfg(unix)]
fn configure_detached(command: &mut Command) {
    use std::os::unix::process::CommandExt;

    // SAFETY: `setsid` is async-signal-safe and touches no Rust-owned memory.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

#[cfg(unix)]
fn configure_detached_retry(command: &mut Command) {
    configure_detached(command);
}

#[cfg(windows)]
fn configure_detached(command: &mut Command) {
    use std::os::windows::process::CommandExt;

    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
    command.creation_flags(DETACHED_PROCESS | CREATE_BREAKAWAY_FROM_JOB);
}

#[cfg(windows)]
fn configure_detached_retry(command: &mut Command) {
    use std::os::windows::process::CommandExt;

    const DETACHED_PROCESS: u32 = 0x0000_0008;
    command.creation_flags(DETACHED_PROCESS);
}

/// Windows callers retry without BREAKAWAY_FROM_JOB when a containing job
/// disallows it. This predicate keeps that policy out of provider execution.
pub fn is_access_denied_spawn_error(error: &std::io::Error) -> bool {
    error.raw_os_error() == Some(5)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksum_manifest_and_verification_fail_closed() {
        let data = b"release archive";
        let sum = hex::encode(Sha256::digest(data));
        let manifest = format!("{sum}  patchbay_linux_arm64.tar.gz\n");
        let parsed = checksum_for_asset(manifest.as_bytes(), "patchbay_linux_arm64.tar.gz")
            .expect("checksum should be selected by exact asset name");
        assert_eq!(parsed, sum);
        verify_checksum(data, &parsed, "patchbay_linux_arm64.tar.gz")
            .expect("matching archive must verify");

        let error = verify_checksum(b"tampered", &parsed, "patchbay_linux_arm64.tar.gz")
            .expect_err("changed archive must fail verification");
        assert_eq!(error.kind, UpdateFailureKind::Checksum);
    }

    #[test]
    fn download_url_rejects_credentials_and_untrusted_hosts_without_echoing_secret() {
        for url in [
            "http://github.com/release",
            "https://token:secret@github.com/release",
            "https://example.com/release?signature=secret",
        ] {
            let error = validate_download_url(url).expect_err("URL must be rejected");
            assert_eq!(error.kind, UpdateFailureKind::Download);
            assert!(!error.to_string().contains("secret"));
        }
    }

    #[test]
    fn retired_homebrew_cellar_uses_the_resolved_binary_directly() {
        let executable = PathBuf::from("/opt/homebrew/Cellar/patchbay/0.3.0/bin/patchbay");
        let executor = UpdateExecutor::direct_for_test(executable.clone());

        assert_eq!(executor.restart_target_binary(), executable.as_path());
    }

    #[test]
    fn streaming_download_limit_is_enforced_across_chunk_boundaries() {
        let mut bytes = Vec::new();
        append_download_chunk(&mut bytes, b"123456", 10, "asset.tar.gz").unwrap();
        let error = append_download_chunk(&mut bytes, b"78901", 10, "asset.tar.gz")
            .expect_err("the second chunk must fail before allocation");

        assert_eq!(error.kind, UpdateFailureKind::Download);
        assert!(error.to_string().contains("exceeds the size limit"));
        assert_eq!(bytes, b"123456");
    }

    #[test]
    fn typed_update_request_resolves_release_tags_and_rejects_dev_versions() {
        assert_eq!(release_tag("1.2.3").unwrap(), "v1.2.3");
        assert_eq!(release_tag(" v1.2.3 ").unwrap(), "v1.2.3");
        let error = release_tag("v1.2.3-17-gdeadbeef").expect_err("dev build tag");
        assert_eq!(error.kind, UpdateFailureKind::InvalidVersion);

        let request =
            UpdateRequest::for_target("1.2.3").with_download_timeout(Duration::from_secs(7));
        assert_eq!(request.target_version.as_deref(), Some("1.2.3"));
        assert_eq!(request.current_version, None);
        assert_eq!(request.download_timeout, Some(Duration::from_secs(7)));
        assert_eq!(UpdateRequest::latest(), UpdateRequest::default());
    }

    #[test]
    fn already_current_decision_normalizes_v_prefix_without_downloading() {
        let request = UpdateRequest::latest().with_current_version("v1.2.3");
        assert_eq!(request.current_version.as_deref(), Some("v1.2.3"));
        assert!(same_release_version("1.2.3", "v1.2.3"));
        assert!(!same_release_version("1.2.4", "v1.2.3"));

        let outcome = already_current_outcome(UpdateInstallMethod::Direct, Some("v1.2.3".into()));
        assert!(outcome.already_current);
        assert_eq!(outcome.message, "Already up to date.");
        assert!(!outcome.latest_query_failed);
    }

    #[test]
    fn typed_update_request_rejects_zero_download_timeout() {
        let error = validate_download_timeout(Duration::ZERO).expect_err("zero timeout");
        assert_eq!(error.kind, UpdateFailureKind::InvalidTimeout);
        assert!(error.to_string().contains("greater than zero"));
    }

    #[test]
    fn tar_gz_extracts_only_the_named_regular_binary() {
        use flate2::write::GzEncoder;
        use flate2::Compression;

        let payload = b"real patchbay binary";
        let mut header = [0_u8; 512];
        let name = b"release/patchbay\0";
        header[..name.len()].copy_from_slice(name);
        let size = format!("{:011o}\0", payload.len());
        header[124..136].copy_from_slice(size.as_bytes());
        header[156] = b'0';
        let mut tar = Vec::from(header);
        tar.extend_from_slice(payload);
        tar.resize(tar.len().div_ceil(512) * 512, 0);
        tar.extend_from_slice(&[0_u8; 1024]);

        let mut gzip = GzEncoder::new(Vec::new(), Compression::default());
        gzip.write_all(&tar).unwrap();
        let archive = gzip.finish().unwrap();
        assert_eq!(extract_tar_gz(&archive, "patchbay").unwrap(), payload);
    }
}
