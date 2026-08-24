//! Search, import, and refresh for workspace skills.
//!
//! This is a boundary port of Go's `skill.go`, `skill_import_archive.go`, and
//! `skill_refresh.go`. All outbound URLs are either allow-listed entry URLs or
//! generated from validated GitHub/ClawHub identifiers; redirects are disabled
//! so an upstream cannot redirect the server onto an internal address.

use std::collections::HashMap;
use std::io::{Cursor, Read};
use std::time::Duration;

use axum::body::to_bytes;
use axum::extract::{
    DefaultBodyLimit, Extension, FromRequest, Multipart, Path, RawQuery, Request, State,
};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use cordy_db::models::Skill;
use cordy_db::queries::skill;
use cordy_middleware::workspace::WorkspaceContext;
use futures_util::stream::{self, StreamExt as _};
use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::error::error_response;
use crate::skill::{
    db_error, object_config, reserved_content_path, sanitize, skill_event, unique_violation,
    valid_file_path, workspace_id, SkillFileResponse, SkillWithFilesResponse,
};
use crate::state::HandlerState;

const CLAWHUB_API_BASE: &str = "https://clawhub.ai/api/v1";
const MAX_FILE_SIZE: usize = 1 << 20;
const MAX_TOTAL_SIZE: usize = 8 << 20;
const MAX_FILE_COUNT: usize = 256;
const MAX_ARCHIVE_UPLOAD_SIZE: usize = 16 << 20;
const FETCH_TIMEOUT: Duration = Duration::from_secs(45);
const SEARCH_STATS_LIMIT: usize = 10;
const DOWNLOAD_CONCURRENCY: usize = 8;

pub fn router() -> Router<HandlerState> {
    Router::new()
        .route("/api/skills/search", get(search))
        .route(
            "/api/skills/import",
            post(import).layer(DefaultBodyLimit::max(MAX_ARCHIVE_UPLOAD_SIZE + (1 << 20))),
        )
        .route("/api/skills/{id}/refresh", post(refresh))
}

fn http_client() -> Result<Client, Response> {
    Client::builder()
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|error| {
            tracing::error!(%error, "failed to build skill import client");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to initialize skill importer",
            )
        })
}

#[derive(Debug, Serialize)]
struct SearchCandidate {
    name: String,
    url: String,
    source: String,
    repo: Option<String>,
    install_count: Option<i64>,
    github_stars: Option<i64>,
    description: String,
}

#[derive(Debug, Deserialize)]
struct ClawSearchResponse {
    #[serde(default)]
    results: Vec<ClawSearchResult>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClawSearchResult {
    #[serde(default)]
    slug: String,
    #[serde(default)]
    display_name: String,
    #[serde(default)]
    summary: String,
    #[serde(default)]
    owner_handle: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClawStats {
    installs_all_time: i64,
    installs_current: i64,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClawSkill {
    display_name: String,
    summary: String,
    #[serde(default)]
    tags: HashMap<String, String>,
    #[serde(default)]
    stats: ClawStats,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClawSkillResponse {
    skill: ClawSkill,
    latest_version: Option<ClawLatestVersion>,
}

#[derive(Debug, Deserialize)]
struct ClawLatestVersion {
    version: String,
}

#[derive(Debug, Deserialize)]
struct ClawVersionResponse {
    version: ClawVersion,
}

#[derive(Debug, Deserialize)]
struct ClawVersion {
    #[serde(default)]
    files: Vec<ClawFile>,
}

#[derive(Debug, Deserialize)]
struct ClawFile {
    path: String,
}

async fn search(RawQuery(raw_query): RawQuery) -> Response {
    let query = raw_query
        .as_deref()
        .into_iter()
        .flat_map(|query| url::form_urlencoded::parse(query.as_bytes()))
        .find_map(|(key, value)| (key == "q").then(|| value.into_owned()))
        .unwrap_or_default();
    let query = query.trim();
    if query.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "query is required");
    }
    let client = match http_client() {
        Ok(client) => client,
        Err(response) => return response,
    };
    match search_clawhub(&client, CLAWHUB_API_BASE, query).await {
        Ok(candidates) => Json(candidates).into_response(),
        Err(error) => (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "code": "upstream_unavailable", "error": error })),
        )
            .into_response(),
    }
}

async fn search_clawhub(
    client: &Client,
    base: &str,
    query: &str,
) -> Result<Vec<SearchCandidate>, String> {
    let response = client
        .get(format!("{base}/search"))
        .query(&[("q", query)])
        .send()
        .await
        .map_err(|error| format!("failed to reach ClawHub: {error}"))?;
    if response.status() != StatusCode::OK {
        return Err(format!(
            "ClawHub search returned status {}",
            response.status().as_u16()
        ));
    }
    let found: ClawSearchResponse = response
        .json()
        .await
        .map_err(|_| "failed to parse ClawHub search response".to_string())?;
    let mut candidates = Vec::new();
    for (index, item) in found.results.into_iter().enumerate() {
        if item.slug.is_empty() {
            continue;
        }
        let installs = if index < SEARCH_STATS_LIMIT {
            clawhub_install_count(client, base, &item.slug).await
        } else {
            None
        };
        let name = if item.display_name.is_empty() {
            item.slug.clone()
        } else {
            item.display_name
        };
        let url = if item.owner_handle.is_empty() {
            format!("https://clawhub.ai/{}", path_segment(&item.slug))
        } else {
            format!(
                "https://clawhub.ai/{}/{}",
                path_segment(&item.owner_handle),
                path_segment(&item.slug)
            )
        };
        candidates.push(SearchCandidate {
            name,
            url,
            source: "clawhub.ai".into(),
            repo: None,
            install_count: installs,
            github_stars: None,
            description: item.summary,
        });
    }
    Ok(candidates)
}

async fn clawhub_install_count(client: &Client, base: &str, slug: &str) -> Option<i64> {
    let response = client
        .get(format!("{base}/skills/{}", path_segment(slug)))
        .send()
        .await
        .ok()?;
    if response.status() != StatusCode::OK {
        return None;
    }
    let detail: ClawSkillResponse = response.json().await.ok()?;
    Some(if detail.skill.stats.installs_all_time > 0 {
        detail.skill.stats.installs_all_time
    } else {
        detail.skill.stats.installs_current
    })
}

#[derive(Debug, Default, Deserialize)]
struct ImportRequest {
    #[serde(default, deserialize_with = "null_string")]
    url: String,
    #[serde(default, deserialize_with = "null_string")]
    on_conflict: String,
}

fn null_string<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<String>::deserialize(deserializer)?.unwrap_or_default())
}

fn decode_first<T: for<'de> Deserialize<'de> + Default>(bytes: &[u8]) -> Result<T, ()> {
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    Option::<T>::deserialize(&mut deserializer)
        .map(Option::unwrap_or_default)
        .map_err(|_| ())
}

#[derive(Debug, Clone)]
struct ImportedFile {
    path: String,
    content: String,
}

#[derive(Debug, Clone)]
struct ImportedSkill {
    name: String,
    description: String,
    content: String,
    files: Vec<ImportedFile>,
    total_size: usize,
    origin: Option<Value>,
}

impl ImportedSkill {
    fn new(name: String, description: String, content: String, origin: Option<Value>) -> Self {
        Self {
            name,
            description,
            content,
            files: Vec::new(),
            total_size: 0,
            origin,
        }
    }

    fn add_file(&mut self, path: String, content: String) -> Result<(), ImportError> {
        if likely_binary(&path) {
            return Ok(());
        }
        if self.files.len() >= MAX_FILE_COUNT {
            return Err(ImportError::Cap(format!(
                "import bundle exceeds {MAX_FILE_COUNT} file limit"
            )));
        }
        if content.len() > MAX_FILE_SIZE {
            return Err(ImportError::Cap(format!(
                "file exceeds {MAX_FILE_SIZE} byte limit"
            )));
        }
        if self.total_size + content.len() > MAX_TOTAL_SIZE {
            return Err(ImportError::Cap(format!(
                "import bundle exceeds {MAX_TOTAL_SIZE} byte limit"
            )));
        }
        self.total_size += content.len();
        self.files.push(ImportedFile { path, content });
        Ok(())
    }
}

#[derive(Debug)]
enum ImportError {
    Bad(String),
    Cap(String),
    Unavailable(String),
    Timeout,
    Upstream(String),
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bad(message) | Self::Cap(message) | Self::Unavailable(message) | Self::Upstream(message) => formatter.write_str(message),
            Self::Timeout => formatter.write_str("skill import timed out fetching source files; the skill may be too large or the source too slow"),
        }
    }
}

fn import_error_response(error: ImportError) -> Response {
    let status = match error {
        ImportError::Bad(_) => StatusCode::BAD_REQUEST,
        ImportError::Cap(_) => StatusCode::PAYLOAD_TOO_LARGE,
        ImportError::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
        ImportError::Timeout => StatusCode::GATEWAY_TIMEOUT,
        ImportError::Upstream(_) => StatusCode::BAD_GATEWAY,
    };
    error_response(status, &error.to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    ClawHub,
    SkillsSh,
    GitHub,
}

fn detect_source(raw: &str) -> Result<(Source, String), ImportError> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(ImportError::Bad("empty URL".into()));
    }
    if !raw.contains('/') && !raw.contains('.') {
        return Ok((Source::ClawHub, raw.into()));
    }
    let normalized = if raw.starts_with("http://") || raw.starts_with("https://") {
        raw.to_string()
    } else {
        format!("https://{raw}")
    };
    let parsed = Url::parse(&normalized)
        .map_err(|error| ImportError::Bad(format!("invalid URL: {error}")))?;
    if parsed.scheme() != "https" {
        return Err(ImportError::Bad("skill source must use https".into()));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(ImportError::Bad(
            "skill source URL must not contain credentials".into(),
        ));
    }
    let host = parsed.host_str().unwrap_or_default().to_ascii_lowercase();
    match host.as_str() {
        "clawhub.ai" | "www.clawhub.ai" => Ok((Source::ClawHub, normalized)),
        "skills.sh" | "www.skills.sh" => Ok((Source::SkillsSh, normalized)),
        "github.com" | "www.github.com" => Ok((Source::GitHub, normalized)),
        _ => Err(ImportError::Bad(format!(
            "unsupported source: {host} (supported: clawhub.ai, skills.sh, github.com)"
        ))),
    }
}

async fn import(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    headers: HeaderMap,
    request: Request,
) -> Response {
    let content_type = request
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase();

    let (imported, strategy, structured) = if content_type.starts_with("multipart/form-data") {
        match archive_from_request(request, &state).await {
            Ok(value) => value,
            Err(response) => return response,
        }
    } else {
        let bytes = match to_bytes(request.into_body(), MAX_ARCHIVE_UPLOAD_SIZE).await {
            Ok(bytes) => bytes,
            Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid request body"),
        };
        let request: ImportRequest = match decode_first(&bytes) {
            Ok(request) => request,
            Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid request body"),
        };
        if !valid_strategy(&request.on_conflict) {
            return error_response(
                StatusCode::BAD_REQUEST,
                "on_conflict must be one of: fail, overwrite, rename, skip",
            );
        }
        let structured = !request.on_conflict.is_empty();
        let strategy = if request.on_conflict.is_empty() {
            "fail".into()
        } else {
            request.on_conflict
        };
        let (source, normalized) = match detect_source(&request.url) {
            Ok(value) => value,
            Err(error) => return import_error_response(error),
        };
        let client = match http_client() {
            Ok(client) => client,
            Err(response) => return response,
        };
        let fetched =
            tokio::time::timeout(FETCH_TIMEOUT, fetch_source(&client, source, &normalized)).await;
        let imported = match fetched {
            Ok(Ok(imported)) => imported,
            Ok(Err(error)) => return import_error_response(error),
            Err(_) => return import_error_response(ImportError::Timeout),
        };
        (imported, strategy, structured)
    };
    finish_import(&state, &context, &headers, imported, &strategy, structured).await
}

async fn archive_from_request(
    request: Request,
    state: &HandlerState,
) -> Result<(ImportedSkill, String, bool), Response> {
    let mut multipart = Multipart::from_request(request, state).await.map_err(|_| {
        error_response(
            StatusCode::BAD_REQUEST,
            "invalid multipart upload or file exceeds the size limit",
        )
    })?;
    let mut archive = None;
    let mut filename = String::new();
    let mut strategy = String::new();
    while let Some(field) = multipart.next_field().await.map_err(|_| {
        error_response(
            StatusCode::BAD_REQUEST,
            "invalid multipart upload or file exceeds the size limit",
        )
    })? {
        match field.name() {
            Some("file") if archive.is_none() => {
                filename = field.file_name().unwrap_or_default().to_string();
                let data = field.bytes().await.map_err(|_| {
                    error_response(StatusCode::BAD_REQUEST, "failed to read uploaded file")
                })?;
                if data.len() > MAX_ARCHIVE_UPLOAD_SIZE {
                    return Err(error_response(
                        StatusCode::BAD_REQUEST,
                        "invalid multipart upload or file exceeds the size limit",
                    ));
                }
                archive = Some(data);
            }
            Some("on_conflict") if strategy.is_empty() => {
                strategy = field.text().await.map_err(|_| {
                    error_response(StatusCode::BAD_REQUEST, "invalid multipart upload")
                })?;
            }
            _ => {}
        }
    }
    if !valid_strategy(&strategy) {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "on_conflict must be one of: fail, overwrite, rename, skip",
        ));
    }
    let data = archive.ok_or_else(|| {
        error_response(
            StatusCode::BAD_REQUEST,
            "a skill archive file is required (form field \"file\")",
        )
    })?;
    let imported = parse_archive(&data, &filename)
        .map_err(|error| error_response(StatusCode::BAD_REQUEST, &error.to_string()))?;
    Ok((
        imported,
        if strategy.is_empty() {
            "fail".into()
        } else {
            strategy
        },
        true,
    ))
}

fn valid_strategy(strategy: &str) -> bool {
    matches!(strategy, "" | "fail" | "overwrite" | "rename" | "skip")
}

async fn fetch_source(
    client: &Client,
    source: Source,
    url: &str,
) -> Result<ImportedSkill, ImportError> {
    match source {
        Source::ClawHub => fetch_clawhub(client, CLAWHUB_API_BASE, url).await,
        Source::SkillsSh => fetch_skills_sh(client, url).await,
        Source::GitHub => fetch_github(client, url).await,
    }
}

fn clawhub_slug(raw: &str) -> Result<String, ImportError> {
    if !raw.contains('/') && !raw.contains('.') {
        return Ok(raw.to_string());
    }
    let parsed =
        Url::parse(raw).map_err(|error| ImportError::Bad(format!("invalid URL: {error}")))?;
    let parts = parsed
        .path_segments()
        .map(|parts| parts.filter(|part| !part.is_empty()).collect::<Vec<_>>())
        .unwrap_or_default();
    match parts.as_slice() {
        [slug] | [_, slug] => Ok((*slug).to_string()),
        _ => Err(ImportError::Bad(format!(
            "could not extract skill slug from URL: {raw}"
        ))),
    }
}

async fn fetch_clawhub(
    client: &Client,
    base: &str,
    raw_url: &str,
) -> Result<ImportedSkill, ImportError> {
    let slug = clawhub_slug(raw_url)?;
    let response = client
        .get(format!("{base}/skills/{}", path_segment(&slug)))
        .send()
        .await
        .map_err(upstream)?;
    if response.status() == StatusCode::NOT_FOUND {
        return Err(ImportError::Upstream(format!(
            "skill not found on ClawHub: {slug}"
        )));
    }
    if response.status() != StatusCode::OK {
        return Err(ImportError::Upstream(format!(
            "ClawHub returned status {}",
            response.status().as_u16()
        )));
    }
    let detail: ClawSkillResponse = response
        .json()
        .await
        .map_err(|_| ImportError::Upstream("failed to parse ClawHub response".into()))?;
    let latest = detail
        .skill
        .tags
        .get("latest")
        .cloned()
        .or_else(|| detail.latest_version.map(|value| value.version));
    let name = if detail.skill.display_name.is_empty() {
        slug.clone()
    } else {
        detail.skill.display_name
    };
    let mut imported = ImportedSkill::new(
        name,
        detail.skill.summary,
        String::new(),
        Some(json!({
            "type": "clawhub", "source_url": raw_url, "slug": slug
        })),
    );
    let mut paths = Vec::new();
    if let Some(version) = latest.as_ref() {
        if let Ok(response) = client
            .get(format!(
                "{base}/skills/{}/versions/{}",
                path_segment(&slug),
                path_segment(version)
            ))
            .send()
            .await
        {
            if response.status() == StatusCode::OK {
                if let Ok(version) = response.json::<ClawVersionResponse>().await {
                    paths = version
                        .version
                        .files
                        .into_iter()
                        .map(|file| file.path)
                        .collect();
                }
            }
        }
    }
    for path in paths {
        let mut url = Url::parse(&format!("{base}/skills/{}/file", path_segment(&slug)))
            .map_err(|error| ImportError::Upstream(error.to_string()))?;
        url.query_pairs_mut().append_pair("path", &path);
        if let Some(version) = latest.as_ref() {
            url.query_pairs_mut().append_pair("version", version);
        }
        match fetch_bytes(client, url, false).await {
            Ok(bytes) if path == "SKILL.md" => {
                imported.content = String::from_utf8_lossy(&bytes).into_owned()
            }
            Ok(bytes) => imported.add_file(path, String::from_utf8_lossy(&bytes).into_owned())?,
            Err(error) if path == "SKILL.md" || matches!(error, ImportError::Cap(_)) => {
                return Err(ImportError::Upstream(format!(
                    "clawhub import: {path}: {error}"
                )))
            }
            Err(error) => tracing::warn!(%error, %path, "clawhub import file skipped"),
        }
    }
    if imported.content.is_empty() {
        return Err(ImportError::Upstream(format!(
            "clawhub import: SKILL.md is empty or missing for {slug}"
        )));
    }
    Ok(imported)
}

#[derive(Debug, Deserialize)]
struct RepoInfo {
    default_branch: String,
}

#[derive(Debug, Deserialize)]
struct TreeResponse {
    #[serde(default)]
    tree: Vec<TreeEntry>,
    truncated: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct TreeEntry {
    path: String,
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    size: i64,
}

#[derive(Debug, Deserialize)]
struct ContentEntry {
    name: String,
    path: String,
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    download_url: String,
}

async fn fetch_skills_sh(client: &Client, raw_url: &str) -> Result<ImportedSkill, ImportError> {
    let parsed = Url::parse(raw_url).map_err(|error| ImportError::Bad(error.to_string()))?;
    let parts = path_parts(&parsed);
    if parts.len() != 3 {
        return Err(ImportError::Bad(format!(
            "expected URL format: skills.sh/{{owner}}/{{repo}}/{{skill-name}}, got: {}",
            parsed.path()
        )));
    }
    let (owner, repo, requested) = (&parts[0], &parts[1], &parts[2]);
    let branch = github_default_branch(client, owner, repo).await;
    let tree = github_tree(client, owner, repo, &branch).await.map_err(|error| ImportError::Unavailable(format!(
        "import source temporarily unavailable: could not read the {owner}/{repo} repository tree (usually GitHub API rate limiting — set GITHUB_TOKEN on the server or retry): {error}"
    )))?;
    let candidates = tree
        .tree
        .iter()
        .filter(|entry| {
            entry.kind == "blob" && (entry.path == "SKILL.md" || entry.path.ends_with("/SKILL.md"))
        })
        .collect::<Vec<_>>();
    let raw_prefix = raw_prefix(owner, repo, &branch);
    let mut selected = None;
    let conventional = [
        format!("skills/{requested}/SKILL.md"),
        format!(".claude/skills/{requested}/SKILL.md"),
        format!("plugin/skills/{requested}/SKILL.md"),
        format!("{requested}/SKILL.md"),
    ];
    for entry in &candidates {
        let likely = conventional.iter().any(|path| path == &entry.path)
            || entry
                .path
                .rsplit('/')
                .nth(1)
                .is_some_and(|name| name.eq_ignore_ascii_case(requested));
        if !likely {
            continue;
        }
        if let Ok(body) = fetch_bytes(client, raw_url_for(&raw_prefix, &entry.path)?, true).await {
            let (name, _) = parse_frontmatter(&String::from_utf8_lossy(&body));
            if name == *requested || conventional.iter().any(|path| path == &entry.path) {
                selected = Some((entry.path.clone(), body));
                break;
            }
        }
    }
    if selected.is_none() {
        for entry in candidates {
            if let Ok(body) =
                fetch_bytes(client, raw_url_for(&raw_prefix, &entry.path)?, true).await
            {
                if parse_frontmatter(&String::from_utf8_lossy(&body)).0 == *requested {
                    selected = Some((entry.path.clone(), body));
                    break;
                }
            }
        }
    }
    let (skill_path, body) = selected.ok_or_else(|| {
        ImportError::Upstream(format!(
            "SKILL.md not found in repository {owner}/{repo} for skill {requested}"
        ))
    })?;
    let skill_dir = skill_path.strip_suffix("/SKILL.md").unwrap_or("");
    let body_text = String::from_utf8_lossy(&body).into_owned();
    let (front_name, description) = parse_frontmatter(&body_text);
    let name = if front_name.is_empty() {
        requested.clone()
    } else {
        front_name
    };
    let mut imported = ImportedSkill::new(
        name,
        description,
        body_text,
        Some(json!({
            "type": "skills_sh", "source_url": raw_url, "owner": owner, "repo": repo, "skill": requested
        })),
    );
    if tree.truncated {
        add_files_via_crawl(client, &mut imported, owner, repo, &branch, skill_dir).await?;
    } else {
        add_tree_files(client, &mut imported, &tree, &raw_prefix, skill_dir).await?;
    }
    Ok(imported)
}

#[derive(Debug)]
struct GitHubSpec {
    owner: String,
    repo: String,
    reference: String,
    skill_dir: String,
    tree_segments: Vec<String>,
}

async fn fetch_github(client: &Client, raw_url: &str) -> Result<ImportedSkill, ImportError> {
    let mut spec = parse_github(raw_url)?;
    if !spec.tree_segments.is_empty() {
        resolve_github_ref(client, &mut spec).await?;
    }
    if spec.reference.is_empty() {
        spec.reference = github_default_branch(client, &spec.owner, &spec.repo).await;
    }
    let prefix = raw_prefix(&spec.owner, &spec.repo, &spec.reference);
    let skill_path = if spec.skill_dir.is_empty() {
        "SKILL.md".into()
    } else {
        format!("{}/SKILL.md", spec.skill_dir)
    };
    let body = fetch_bytes(client, raw_url_for(&prefix, &skill_path)?, true)
        .await
        .map_err(|error| {
            ImportError::Upstream(format!(
                "SKILL.md not found at {skill_path} in {}/{}@{}: {error}",
                spec.owner, spec.repo, spec.reference
            ))
        })?;
    let body_text = String::from_utf8_lossy(&body).into_owned();
    let (front_name, description) = parse_frontmatter(&body_text);
    let name = if front_name.is_empty() {
        spec.skill_dir
            .rsplit('/')
            .next()
            .filter(|value| !value.is_empty())
            .unwrap_or(&spec.repo)
            .to_string()
    } else {
        front_name
    };
    let mut imported = ImportedSkill::new(
        name,
        description,
        body_text,
        Some(json!({
            "type": "github", "source_url": raw_url, "owner": spec.owner, "repo": spec.repo,
            "ref": spec.reference, "path": spec.skill_dir
        })),
    );
    match github_tree(client, &spec.owner, &spec.repo, &spec.reference).await {
        Ok(tree) if !tree.truncated => {
            add_tree_files(client, &mut imported, &tree, &prefix, &spec.skill_dir).await?;
        }
        _ => {
            add_files_via_crawl(
                client,
                &mut imported,
                &spec.owner,
                &spec.repo,
                &spec.reference,
                &spec.skill_dir,
            )
            .await?;
        }
    }
    Ok(imported)
}

fn parse_github(raw: &str) -> Result<GitHubSpec, ImportError> {
    let parsed =
        Url::parse(raw).map_err(|error| ImportError::Bad(format!("invalid URL: {error}")))?;
    let parts = path_parts(&parsed);
    if parts.len() < 2 {
        return Err(ImportError::Bad(
            "expected github.com/{owner}/{repo}[/tree/{ref}/{path}]".into(),
        ));
    }
    let mut spec = GitHubSpec {
        owner: parts[0].clone(),
        repo: parts[1].trim_end_matches(".git").to_string(),
        reference: String::new(),
        skill_dir: String::new(),
        tree_segments: Vec::new(),
    };
    if parts.len() > 2 {
        let kind = parts.get(2).map(String::as_str).unwrap_or_default();
        if !matches!(kind, "tree" | "blob") || parts.len() < 4 {
            return Err(ImportError::Bad(
                "GitHub skill URL must point to a repository, tree directory, or SKILL.md blob"
                    .into(),
            ));
        }
        let mut encoded = parts[3..].to_vec();
        if kind == "blob" {
            if !encoded
                .last()
                .is_some_and(|value| value.eq_ignore_ascii_case("SKILL.md"))
            {
                return Err(ImportError::Bad(
                    "blob URL must point to a SKILL.md file".into(),
                ));
            }
            encoded.pop();
            if encoded.is_empty() {
                return Err(ImportError::Bad("missing ref after /blob/".into()));
            }
        }
        spec.tree_segments = encoded
            .into_iter()
            .map(|segment| decode_path_segment(&segment))
            .collect::<Result<Vec<_>, _>>()?;
        spec.reference = spec.tree_segments[0].clone();
        spec.skill_dir = spec.tree_segments[1..].join("/");
    }
    Ok(spec)
}

fn decode_path_segment(segment: &str) -> Result<String, ImportError> {
    let bytes = segment.as_bytes();
    for index in 0..bytes.len() {
        if bytes[index] == b'%'
            && (index + 2 >= bytes.len()
                || !bytes[index + 1].is_ascii_hexdigit()
                || !bytes[index + 2].is_ascii_hexdigit())
        {
            return Err(ImportError::Bad(format!(
                "invalid path segment {segment:?}"
            )));
        }
    }
    let decoded = percent_encoding::percent_decode_str(segment)
        .decode_utf8()
        .map_err(|error| ImportError::Bad(format!("invalid path segment {segment:?}: {error}")))?
        .into_owned();
    if decoded.is_empty() {
        return Err(ImportError::Bad("empty path segment in URL".into()));
    }
    Ok(decoded)
}

async fn resolve_github_ref(client: &Client, spec: &mut GitHubSpec) -> Result<(), ImportError> {
    let mut blocked = false;
    for count in (1..=spec.tree_segments.len()).rev() {
        let candidate = spec.tree_segments[..count].join("/");
        let response = github_get(
            client,
            format!(
                "https://api.github.com/repos/{}/{}/commits/{}",
                path_segment(&spec.owner),
                path_segment(&spec.repo),
                ref_path(&candidate)
            ),
        )
        .await?;
        match response.status() {
            StatusCode::OK => {
                spec.reference = candidate;
                spec.skill_dir = spec.tree_segments[count..].join("/");
                return Ok(());
            }
            StatusCode::NOT_FOUND | StatusCode::UNPROCESSABLE_ENTITY => continue,
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN | StatusCode::TOO_MANY_REQUESTS => {
                blocked = true;
                continue;
            }
            status => {
                return Err(ImportError::Upstream(format!(
                    "github API returned status {} for ref {candidate:?}",
                    status.as_u16()
                )));
            }
        }
    }
    if blocked {
        // Preserve the optimistic single-segment split populated by
        // parse_github. The raw SKILL.md request will produce the useful error
        // if this guess was wrong.
        return Ok(());
    }
    Err(ImportError::Bad(
        "could not resolve the GitHub tree ref".into(),
    ))
}

async fn github_default_branch(client: &Client, owner: &str, repo: &str) -> String {
    match github_get(
        client,
        format!(
            "https://api.github.com/repos/{}/{}",
            path_segment(owner),
            path_segment(repo)
        ),
    )
    .await
    {
        Ok(response) if response.status() == StatusCode::OK => response
            .json::<RepoInfo>()
            .await
            .ok()
            .map(|info| info.default_branch)
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "main".into()),
        _ => "main".into(),
    }
}

async fn github_tree(
    client: &Client,
    owner: &str,
    repo: &str,
    reference: &str,
) -> Result<TreeResponse, ImportError> {
    let response = github_get(
        client,
        format!(
            "https://api.github.com/repos/{}/{}/git/trees/{}?recursive=1",
            path_segment(owner),
            path_segment(repo),
            ref_path(reference)
        ),
    )
    .await?;
    if response.status() != StatusCode::OK {
        return Err(ImportError::Upstream(format!(
            "HTTP {}",
            response.status().as_u16()
        )));
    }
    response
        .json()
        .await
        .map_err(|error| ImportError::Upstream(error.to_string()))
}

async fn github_get(client: &Client, url: String) -> Result<reqwest::Response, ImportError> {
    let parsed = Url::parse(&url).map_err(|error| ImportError::Upstream(error.to_string()))?;
    if parsed.scheme() != "https" || parsed.host_str() != Some("api.github.com") {
        return Err(ImportError::Upstream(
            "refused to attach GitHub credentials to a non-GitHub API URL".into(),
        ));
    }
    let mut request = client
        .get(parsed)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "Cordy-Skill-Importer");
    if let Ok(token) = std::env::var("GITHUB_TOKEN") {
        if !token.trim().is_empty() {
            request = request.bearer_auth(token);
        }
    }
    request.send().await.map_err(upstream)
}

fn github_contents_url(owner: &str, repo: &str, repo_path: &str, reference: &str) -> String {
    let encoded_path = repo_path
        .split('/')
        .filter(|part| !part.is_empty())
        .map(path_segment)
        .collect::<Vec<_>>()
        .join("/");
    let suffix = if encoded_path.is_empty() {
        String::new()
    } else {
        format!("/{encoded_path}")
    };
    format!(
        "https://api.github.com/repos/{}/{}/contents{}?ref={}",
        path_segment(owner),
        path_segment(repo),
        suffix,
        path_segment(reference)
    )
}

async fn add_files_via_crawl(
    client: &Client,
    imported: &mut ImportedSkill,
    owner: &str,
    repo: &str,
    reference: &str,
    skill_dir: &str,
) -> Result<(), ImportError> {
    // Queue repository-relative paths, not API-provided URLs. Every request is
    // rebuilt against api.github.com before the GitHub token is attached.
    let mut queue = vec![skill_dir.trim_matches('/').to_string()];
    let mut files = Vec::new();
    while let Some(repo_path) = queue.pop() {
        let response = github_get(
            client,
            github_contents_url(owner, repo, &repo_path, reference),
        )
        .await?;
        if response.status() != StatusCode::OK {
            return Err(ImportError::Upstream(format!(
                "github directory listing returned HTTP {}",
                response.status().as_u16()
            )));
        }
        let entries: Vec<ContentEntry> = response
            .json()
            .await
            .map_err(|error| ImportError::Upstream(error.to_string()))?;
        for entry in entries {
            match entry.kind.as_str() {
                "dir" => queue.push(entry.path),
                "file" => {
                    let lower = entry.name.to_ascii_lowercase();
                    if lower == "skill.md"
                        || matches!(lower.as_str(), "license" | "license.md" | "license.txt")
                        || likely_binary(&entry.path)
                        || entry.download_url.is_empty()
                    {
                        continue;
                    }
                    let base = format!("{}/", skill_dir.trim_matches('/'));
                    let relative = if skill_dir.is_empty() {
                        entry.path.clone()
                    } else {
                        entry
                            .path
                            .strip_prefix(&base)
                            .unwrap_or(&entry.path)
                            .to_string()
                    };
                    let download = Url::parse(&entry.download_url)
                        .map_err(|error| ImportError::Upstream(error.to_string()))?;
                    if download.scheme() != "https"
                        || download.host_str() != Some("raw.githubusercontent.com")
                    {
                        return Err(ImportError::Upstream(
                            "github contents API returned an unsafe download URL".into(),
                        ));
                    }
                    files.push((relative, download));
                    if files.len() > MAX_FILE_COUNT {
                        return Err(ImportError::Cap(format!(
                            "import bundle exceeds {MAX_FILE_COUNT} file limit"
                        )));
                    }
                }
                _ => {}
            }
        }
    }
    files.sort_by(|left, right| left.0.cmp(&right.0));
    let mut downloads = stream::iter(files.into_iter().map(|(path, url)| async move {
        let bytes = fetch_bytes(client, url, true).await?;
        Ok::<_, ImportError>((path, String::from_utf8_lossy(&bytes).into_owned()))
    }))
    .buffered(DOWNLOAD_CONCURRENCY);
    while let Some(result) = downloads.next().await {
        let (path, content) = result?;
        imported.add_file(path, content)?;
    }
    Ok(())
}

async fn add_tree_files(
    client: &Client,
    imported: &mut ImportedSkill,
    tree: &TreeResponse,
    prefix: &str,
    skill_dir: &str,
) -> Result<(), ImportError> {
    let base = if skill_dir.is_empty() {
        String::new()
    } else {
        format!("{skill_dir}/")
    };
    let mut files = tree
        .tree
        .iter()
        .filter_map(|entry| {
            if entry.kind != "blob" || (!base.is_empty() && !entry.path.starts_with(&base)) {
                return None;
            }
            let relative = entry.path.strip_prefix(&base).unwrap_or(&entry.path);
            let lower = relative
                .rsplit('/')
                .next()
                .unwrap_or(relative)
                .to_ascii_lowercase();
            if relative.is_empty()
                || lower == "skill.md"
                || matches!(lower.as_str(), "license" | "license.md" | "license.txt")
                || likely_binary(relative)
            {
                return None;
            }
            Some((
                entry.path.clone(),
                relative.to_string(),
                entry.size.max(0) as usize,
            ))
        })
        .collect::<Vec<_>>();
    files.sort_by(|left, right| left.1.cmp(&right.1));
    if files.len() > MAX_FILE_COUNT {
        return Err(ImportError::Cap(format!(
            "import bundle would contain {} files, exceeding the {MAX_FILE_COUNT} file limit",
            files.len()
        )));
    }
    if let Some(file) = files.iter().find(|file| file.2 > MAX_FILE_SIZE) {
        return Err(ImportError::Cap(format!(
            "{} is {} bytes, exceeding the {MAX_FILE_SIZE} byte per-file limit",
            file.1, file.2
        )));
    }
    let total = files.iter().map(|file| file.2).sum::<usize>();
    if total > MAX_TOTAL_SIZE {
        return Err(ImportError::Cap(format!(
            "import bundle is {total} bytes, exceeding the {MAX_TOTAL_SIZE} byte limit"
        )));
    }
    let downloads = stream::iter(
        files
            .into_iter()
            .map(|(repo_path, relative, _)| async move {
                let url = raw_url_for(prefix, &repo_path)?;
                let bytes = fetch_bytes(client, url, true).await?;
                Ok::<_, ImportError>((relative, String::from_utf8_lossy(&bytes).into_owned()))
            }),
    )
    .buffered(DOWNLOAD_CONCURRENCY)
    .collect::<Vec<_>>()
    .await;
    for result in downloads {
        let (path, content) = result?;
        imported.add_file(path, content)?;
    }
    Ok(())
}

async fn fetch_bytes(client: &Client, url: Url, github_auth: bool) -> Result<Vec<u8>, ImportError> {
    if github_auth
        && (url.scheme() != "https" || url.host_str() != Some("raw.githubusercontent.com"))
    {
        return Err(ImportError::Upstream(
            "refused to attach GitHub credentials to a non-GitHub raw URL".into(),
        ));
    }
    let mut request = client.get(url);
    if github_auth {
        if let Ok(token) = std::env::var("GITHUB_TOKEN") {
            if !token.trim().is_empty() {
                request = request.bearer_auth(token);
            }
        }
    }
    let response = request.send().await.map_err(upstream)?;
    if response.status() != StatusCode::OK {
        return Err(ImportError::Upstream(format!(
            "HTTP {}",
            response.status().as_u16()
        )));
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_FILE_SIZE as u64)
    {
        return Err(ImportError::Cap(format!(
            "file exceeds {MAX_FILE_SIZE} byte limit"
        )));
    }
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(upstream)?;
        if bytes.len() + chunk.len() > MAX_FILE_SIZE {
            return Err(ImportError::Cap(format!(
                "file exceeds {MAX_FILE_SIZE} byte limit"
            )));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn upstream(error: reqwest::Error) -> ImportError {
    ImportError::Upstream(error.to_string())
}

fn raw_prefix(owner: &str, repo: &str, reference: &str) -> String {
    format!(
        "https://raw.githubusercontent.com/{}/{}/{}",
        path_segment(owner),
        path_segment(repo),
        ref_path(reference)
    )
}

fn raw_url_for(prefix: &str, path: &str) -> Result<Url, ImportError> {
    Url::parse(&format!(
        "{}/{}",
        prefix.trim_end_matches('/'),
        path.split('/')
            .filter(|part| !part.is_empty())
            .map(path_segment)
            .collect::<Vec<_>>()
            .join("/")
    ))
    .map_err(|error| ImportError::Upstream(error.to_string()))
}

fn path_segment(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes())
        .collect::<String>()
        .replace('+', "%20")
}
fn ref_path(value: &str) -> String {
    value
        .split('/')
        .map(path_segment)
        .collect::<Vec<_>>()
        .join("/")
}
fn path_parts(url: &Url) -> Vec<String> {
    url.path_segments()
        .map(|parts| {
            parts
                .filter(|part| !part.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn parse_frontmatter(content: &str) -> (String, String) {
    let normalized = content
        .strip_prefix("---\r\n")
        .or_else(|| content.strip_prefix("---\n"));
    let Some(rest) = normalized else {
        return (String::new(), String::new());
    };
    let end = rest.find("\n---").or_else(|| rest.find("\r\n---"));
    let Some(end) = end else {
        return (String::new(), String::new());
    };
    let Ok(value) = serde_yaml::from_str::<HashMap<String, serde_yaml::Value>>(&rest[..end]) else {
        return (String::new(), String::new());
    };
    let string = |key: &str| {
        value
            .get(key)
            .and_then(|value| match value {
                serde_yaml::Value::String(value) => Some(value.clone()),
                serde_yaml::Value::Number(value) => Some(value.to_string()),
                serde_yaml::Value::Bool(value) => Some(value.to_string()),
                _ => None,
            })
            .unwrap_or_default()
    };
    (string("name"), string("description"))
}

fn likely_binary(path: &str) -> bool {
    matches!(
        path.rsplit('.')
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "png"
            | "jpg"
            | "jpeg"
            | "gif"
            | "webp"
            | "ico"
            | "pdf"
            | "zip"
            | "gz"
            | "tar"
            | "woff"
            | "woff2"
            | "ttf"
            | "otf"
            | "mp3"
            | "mp4"
            | "mov"
            | "avi"
            | "doc"
            | "docx"
            | "xls"
            | "xlsx"
            | "ppt"
            | "pptx"
    )
}

fn parse_archive(data: &[u8], filename: &str) -> Result<ImportedSkill, ImportError> {
    let mut archive = zip::ZipArchive::new(Cursor::new(data))
        .map_err(|_| ImportError::Bad("uploaded file is not a valid .skill/.zip archive".into()))?;
    let mut primary = None;
    for index in 0..archive.len() {
        let file = archive
            .by_index(index)
            .map_err(|error| ImportError::Bad(error.to_string()))?;
        if file.is_dir() {
            continue;
        }
        let name = file.name().replace('\\', "/");
        if name
            .rsplit('/')
            .next()
            .is_some_and(|name| name.eq_ignore_ascii_case("SKILL.md"))
            && safe_archive_path(&name)
        {
            let prefix = name
                .strip_suffix("SKILL.md")
                .unwrap_or_default()
                .to_string();
            if primary
                .as_ref()
                .is_none_or(|(_, existing): &(usize, String)| prefix.len() < existing.len())
            {
                primary = Some((index, prefix));
            }
        }
    }
    let (primary_index, prefix) =
        primary.ok_or_else(|| ImportError::Bad("archive does not contain a SKILL.md".into()))?;
    let content = read_zip(&mut archive, primary_index)?;
    let (front_name, description) = parse_frontmatter(&content);
    let fallback = if prefix.is_empty() {
        filename
            .rsplit('/')
            .next()
            .unwrap_or(filename)
            .rsplit_once('.')
            .map(|value| value.0)
            .unwrap_or(filename)
            .trim()
            .to_string()
    } else {
        prefix
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .to_string()
    };
    let name = if front_name.is_empty() {
        fallback
    } else {
        front_name
    };
    if name.is_empty() {
        return Err(ImportError::Bad("could not determine the skill name: SKILL.md has no name field and the archive is unnamed".into()));
    }
    let mut imported = ImportedSkill::new(name, description, content, None);
    for index in 0..archive.len() {
        if index == primary_index {
            continue;
        }
        let file = archive
            .by_index(index)
            .map_err(|error| ImportError::Bad(error.to_string()))?;
        if file.is_dir() {
            continue;
        }
        let full = file.name().replace('\\', "/");
        drop(file);
        if !prefix.is_empty() && !full.starts_with(&prefix) {
            continue;
        }
        let relative = full.strip_prefix(&prefix).unwrap_or(&full).to_string();
        if relative.is_empty()
            || !safe_archive_path(&relative)
            || ignored_archive_path(&relative)
            || relative
                .rsplit('/')
                .next()
                .is_some_and(|name| name.eq_ignore_ascii_case("SKILL.md"))
        {
            continue;
        }
        match read_zip(&mut archive, index) {
            Ok(content) => imported.add_file(relative, content)?,
            Err(ImportError::Cap(_)) => continue,
            Err(error) => return Err(error),
        }
    }
    imported
        .files
        .sort_by(|left, right| left.path.cmp(&right.path));
    Ok(imported)
}

fn read_zip(
    archive: &mut zip::ZipArchive<Cursor<&[u8]>>,
    index: usize,
) -> Result<String, ImportError> {
    let mut file = archive
        .by_index(index)
        .map_err(|error| ImportError::Bad(error.to_string()))?;
    let mut bytes = Vec::new();
    file.by_ref()
        .take((MAX_FILE_SIZE + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| ImportError::Bad(error.to_string()))?;
    if bytes.len() > MAX_FILE_SIZE {
        return Err(ImportError::Cap(format!(
            "file {:?} exceeds {MAX_FILE_SIZE} bytes",
            file.name()
        )));
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn safe_archive_path(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.contains('\\')
        && !path.split('/').any(|part| part == "..")
}

fn ignored_archive_path(path: &str) -> bool {
    if path
        .split('/')
        .any(|part| part.is_empty() || part == "__MACOSX" || part.starts_with('.'))
    {
        return true;
    }
    matches!(
        path.rsplit('/')
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "license" | "license.md" | "license.txt"
    )
}

#[derive(Debug, Serialize)]
struct ExistingSkill {
    id: Uuid,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    created_by: Option<Uuid>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    can_overwrite: bool,
}

impl ExistingSkill {
    fn from_skill(value: &Skill, user: Uuid) -> Self {
        Self {
            id: value.id,
            name: value.name.clone(),
            created_by: value.created_by,
            can_overwrite: value.created_by == Some(user),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum OverwriteFailure {
    NotFound,
    Forbidden,
    NameMismatch,
}

impl std::fmt::Display for OverwriteFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::NotFound => "target skill not found",
            Self::Forbidden => "not permitted to overwrite target skill",
            Self::NameMismatch => "target skill name does not match the imported skill",
        })
    }
}

impl std::error::Error for OverwriteFailure {}

async fn finish_import(
    state: &HandlerState,
    context: &WorkspaceContext,
    headers: &HeaderMap,
    imported: ImportedSkill,
    strategy: &str,
    structured: bool,
) -> Response {
    let workspace = match workspace_id(context) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let name = sanitize(&imported.name);
    let existing = match skill::get_skill_by_workspace_and_name(&state.pool, workspace, &name).await
    {
        Ok(value) => value,
        Err(error) => return db_error(error, "failed to check for existing skill"),
    };
    if let Some(existing) = existing {
        if structured {
            return resolve_conflict(state, context, headers, imported, existing, strategy).await;
        }
        return (StatusCode::CONFLICT, Json(json!({ "error": "a skill with this name already exists", "existing_skill": ExistingSkill::from_skill(&existing, context.member.user_id) }))).into_response();
    }
    match create_imported(state, context, &imported, &name).await {
        Ok(response) => {
            publish_skill(
                state,
                context,
                headers,
                cordy_protocol::EVENT_SKILL_CREATED,
                &response,
            )
            .await;
            if structured {
                (
                    StatusCode::CREATED,
                    Json(json!({ "status": "created", "skill": response })),
                )
                    .into_response()
            } else {
                (StatusCode::CREATED, Json(response)).into_response()
            }
        }
        Err(error) if unique_violation(&error) => {
            match skill::get_skill_by_workspace_and_name(&state.pool, workspace, &name).await {
                Ok(Some(existing)) if structured => {
                    resolve_conflict(state, context, headers, imported, existing, strategy).await
                }
                Ok(Some(existing)) => (
                    StatusCode::CONFLICT,
                    Json(json!({
                        "error": "a skill with this name already exists",
                        "existing_skill": ExistingSkill::from_skill(
                            &existing,
                            context.member.user_id,
                        ),
                    })),
                )
                    .into_response(),
                _ => error_response(
                    StatusCode::CONFLICT,
                    "a skill with this name already exists",
                ),
            }
        }
        Err(error) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("failed to create skill: {error}"),
        ),
    }
}

async fn resolve_conflict(
    state: &HandlerState,
    context: &WorkspaceContext,
    headers: &HeaderMap,
    imported: ImportedSkill,
    existing: Skill,
    strategy: &str,
) -> Response {
    let identity = ExistingSkill::from_skill(&existing, context.member.user_id);
    match strategy {
        "skip" => Json(json!({ "status": "skipped", "reason": "a skill with this name already exists", "existing_skill": identity })).into_response(),
        "overwrite" if existing.created_by != Some(context.member.user_id) => (StatusCode::FORBIDDEN, Json(json!({ "status": "failed", "reason": "only the skill creator can overwrite this skill", "existing_skill": identity }))).into_response(),
        "overwrite" => match overwrite_imported(state, context, &existing, &imported, false).await {
            Ok(response) => { publish_skill(state, context, headers, cordy_protocol::EVENT_SKILL_UPDATED, &response).await; Json(json!({ "status": "updated", "skill": response })).into_response() }
            Err(error) => {
                let (status, reason) = match error.downcast_ref::<OverwriteFailure>() {
                    Some(OverwriteFailure::NotFound) => (StatusCode::CONFLICT, "target skill no longer exists".to_string()),
                    Some(OverwriteFailure::Forbidden) => (StatusCode::FORBIDDEN, "only the skill creator can overwrite this skill".to_string()),
                    Some(OverwriteFailure::NameMismatch) => (StatusCode::CONFLICT, "target skill name no longer matches the imported skill".to_string()),
                    None => (StatusCode::INTERNAL_SERVER_ERROR, format!("failed to overwrite skill: {error}")),
                };
                (status, Json(json!({ "status": "failed", "reason": reason, "existing_skill": identity }))).into_response()
            }
        },
        "rename" => {
            for suffix in 2..52 {
                let candidate = format!("{}-{suffix}", sanitize(&imported.name));
                match create_imported(state, context, &imported, &candidate).await {
                    Ok(response) => { publish_skill(state, context, headers, cordy_protocol::EVENT_SKILL_CREATED, &response).await; return (StatusCode::CREATED, Json(json!({ "status": "created", "reason": "renamed to avoid an existing skill", "skill": response, "existing_skill": identity }))).into_response(); }
                    Err(error) if unique_violation(&error) => continue,
                    Err(error) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, &format!("failed to create renamed skill: {error}")),
                }
            }
            error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to find an available renamed skill name after 50 attempts")
        }
        _ => (StatusCode::CONFLICT, Json(json!({ "status": "conflict", "reason": "a skill with this name already exists; use --on-conflict overwrite to replace it or --on-conflict rename to import a copy", "existing_skill": identity }))).into_response(),
    }
}

async fn create_imported(
    state: &HandlerState,
    context: &WorkspaceContext,
    imported: &ImportedSkill,
    name: &str,
) -> Result<SkillWithFilesResponse, anyhow::Error> {
    let workspace = Uuid::parse_str(&context.workspace_id)?;
    let mut tx = state.pool.begin().await?;
    let config = imported
        .origin
        .as_ref()
        .map(|origin| json!({ "origin": origin }))
        .unwrap_or_else(|| json!({}));
    let value = skill::create_skill(
        &mut *tx,
        workspace,
        name,
        &sanitize(&imported.description),
        &sanitize(&imported.content),
        &config,
        context.member.user_id,
    )
    .await?
    .ok_or_else(|| anyhow::anyhow!("create returned no row"))?;
    let files = replace_files(&mut tx, value.id, &imported.files, false).await?;
    tx.commit().await?;
    Ok(SkillWithFilesResponse {
        skill: value.into(),
        files,
    })
}

async fn overwrite_imported(
    state: &HandlerState,
    context: &WorkspaceContext,
    existing: &Skill,
    imported: &ImportedSkill,
    allow_admin: bool,
) -> Result<SkillWithFilesResponse, anyhow::Error> {
    let mut tx = state.pool.begin().await?;
    let current = skill::get_skill_in_workspace(&mut *tx, existing.id, existing.workspace_id)
        .await?
        .ok_or_else(|| anyhow::Error::new(OverwriteFailure::NotFound))?;
    if !(current.created_by == Some(context.member.user_id)
        || (allow_admin && matches!(context.member.role.as_str(), "owner" | "admin")))
    {
        return Err(anyhow::Error::new(OverwriteFailure::Forbidden));
    }
    if !allow_admin && current.name != sanitize(&imported.name) {
        return Err(anyhow::Error::new(OverwriteFailure::NameMismatch));
    }
    let config = if allow_admin {
        let mut config = object_config(current.config.clone())
            .as_object()
            .cloned()
            .unwrap_or_default();
        if let Some(origin) = imported.origin.clone() {
            config.insert("origin".into(), origin);
        }
        Value::Object(config)
    } else {
        imported
            .origin
            .as_ref()
            .map(|origin| json!({ "origin": origin }))
            .unwrap_or_else(|| json!({}))
    };
    let new_name = if allow_admin {
        Some(sanitize(&imported.name))
    } else {
        None
    };
    let value = skill::update_skill(
        &mut *tx,
        current.id,
        new_name.as_deref(),
        Some(&sanitize(&imported.description)),
        Some(&sanitize(&imported.content)),
        Some(&config),
    )
    .await?
    .ok_or_else(|| anyhow::anyhow!("skill not found"))?;
    skill::delete_skill_files_by_skill(&mut *tx, value.id).await?;
    let files = replace_files(&mut tx, value.id, &imported.files, true).await?;
    tx.commit().await?;
    Ok(SkillWithFilesResponse {
        skill: value.into(),
        files,
    })
}

async fn replace_files(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    skill_id: Uuid,
    files: &[ImportedFile],
    _replacing: bool,
) -> Result<Vec<SkillFileResponse>, anyhow::Error> {
    let mut responses = Vec::new();
    for file in files
        .iter()
        .filter(|file| valid_file_path(&file.path) && !reserved_content_path(&file.path))
    {
        let value = skill::upsert_skill_file(
            &mut **tx,
            skill_id,
            &sanitize(&file.path),
            &sanitize(&file.content),
        )
        .await?
        .ok_or_else(|| anyhow::anyhow!("upsert returned no row"))?;
        responses.push(value.into());
    }
    Ok(responses)
}

async fn publish_skill(
    state: &HandlerState,
    context: &WorkspaceContext,
    headers: &HeaderMap,
    event: &str,
    response: &SkillWithFilesResponse,
) {
    let (actor_type, actor_id, _) = crate::issue::mutation_actor(state, context, headers).await;
    state.bus.publish(&skill_event(
        event,
        response.skill.workspace_id,
        &actor_type,
        actor_id,
        json!({ "skill": response }),
    ));
}

async fn refresh(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let id = match Uuid::parse_str(&id) {
        Ok(value) => value,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid skill id"),
    };
    let workspace = match workspace_id(&context) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let existing = match skill::get_skill_in_workspace(&state.pool, id, workspace).await {
        Ok(Some(value)) => value,
        _ => return error_response(StatusCode::NOT_FOUND, "skill not found"),
    };
    let admin = matches!(context.member.role.as_str(), "owner" | "admin");
    if !admin && existing.created_by != Some(context.member.user_id) {
        return error_response(
            StatusCode::FORBIDDEN,
            "only the skill creator or a workspace admin can update this skill from its source",
        );
    }
    let origin = existing.config.get("origin").and_then(Value::as_object);
    let origin_type = origin
        .and_then(|origin| origin.get("type"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let source_url = origin
        .and_then(|origin| origin.get("source_url"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
    let (source, normalized) = match detect_source(source_url) {
        Ok((source, normalized))
            if matches!(
                (origin_type, source),
                ("github", Source::GitHub)
                    | ("skills_sh", Source::SkillsSh)
                    | ("clawhub", Source::ClawHub)
            ) =>
        {
            (source, normalized)
        }
        _ => return error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "this skill was not imported from a refreshable source (GitHub, skills.sh, or ClawHub)",
        ),
    };
    let client = match http_client() {
        Ok(value) => value,
        Err(response) => return response,
    };
    let imported =
        match tokio::time::timeout(FETCH_TIMEOUT, fetch_source(&client, source, &normalized)).await
        {
            Ok(Ok(value)) => value,
            Ok(Err(error)) => return import_error_response(error),
            Err(_) => return import_error_response(ImportError::Timeout),
        };
    match overwrite_imported(&state, &context, &existing, &imported, true).await {
        Ok(response) => {
            publish_skill(
                &state,
                &context,
                &headers,
                cordy_protocol::EVENT_SKILL_UPDATED,
                &response,
            )
            .await;
            Json(response).into_response()
        }
        Err(error) if unique_violation(&error) => error_response(
            StatusCode::CONFLICT,
            &format!(
                "a skill named \"{}\" already exists in this workspace",
                sanitize(&imported.name)
            ),
        ),
        Err(error) => match error.downcast_ref::<OverwriteFailure>() {
            Some(OverwriteFailure::NotFound) => {
                error_response(StatusCode::NOT_FOUND, "skill not found")
            }
            Some(OverwriteFailure::Forbidden) => error_response(
                StatusCode::FORBIDDEN,
                "only the skill creator or a workspace admin can update this skill from its source",
            ),
            _ => error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("failed to update skill from source: {error}"),
            ),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    #[test]
    fn source_detection_is_https_allow_listed() {
        assert_eq!(detect_source("review").unwrap().0, Source::ClawHub);
        assert_eq!(
            detect_source("https://github.com/acme/review").unwrap().0,
            Source::GitHub
        );
        assert!(detect_source("http://github.com/acme/review").is_err());
        assert!(detect_source("https://ghp_secret@github.com/acme/review").is_err());
        assert!(detect_source("https://user:secret@github.com/acme/review").is_err());
        assert!(detect_source("https://127.0.0.1/skill").is_err());
    }

    #[test]
    fn github_canonical_blob_and_escaped_tree_paths_match_go() {
        let blob =
            parse_github("https://github.com/acme/skills/blob/main/skills/foo/SKILL.md").unwrap();
        assert_eq!(blob.reference, "main");
        assert_eq!(blob.skill_dir, "skills/foo");
        assert_eq!(blob.tree_segments, ["main", "skills", "foo"]);

        let escaped = parse_github("https://github.com/acme/skills/tree/main/my%20skill").unwrap();
        assert_eq!(escaped.skill_dir, "my skill");
        assert!(raw_url_for(
            &raw_prefix("acme", "skills", "main"),
            &format!("{}/SKILL.md", escaped.skill_dir),
        )
        .unwrap()
        .as_str()
        .ends_with("/my%20skill/SKILL.md"));

        assert!(parse_github("https://github.com/acme/skills/blob/main/README.md").is_err());
    }

    #[test]
    fn overwrite_failure_types_survive_anyhow_for_http_mapping() {
        for failure in [
            OverwriteFailure::NotFound,
            OverwriteFailure::Forbidden,
            OverwriteFailure::NameMismatch,
        ] {
            let error = anyhow::Error::new(failure);
            assert!(error.downcast_ref::<OverwriteFailure>().is_some());
        }
    }

    #[test]
    fn import_decoder_matches_go_null_and_first_value_contract() {
        let null: ImportRequest = decode_first(b"null").unwrap();
        assert!(null.url.is_empty());
        let request: ImportRequest =
            decode_first(br#"{"url":null,"on_conflict":null} {"url":"ignored"}"#).unwrap();
        assert!(request.url.is_empty());
        assert!(request.on_conflict.is_empty());
    }

    #[test]
    fn archive_rooting_caps_and_traversal_match_go() {
        let mut bytes = Vec::new();
        {
            let mut writer = zip::ZipWriter::new(Cursor::new(&mut bytes));
            let options = zip::write::SimpleFileOptions::default();
            writer.start_file("review/SKILL.md", options).unwrap();
            writer
                .write_all(b"---\nname: review\ndescription: Reviews\n---\nbody")
                .unwrap();
            writer.start_file("review/docs/a.md", options).unwrap();
            writer.write_all(b"guide").unwrap();
            writer.start_file("review/../evil", options).unwrap();
            writer.write_all(b"bad").unwrap();
            writer.finish().unwrap();
        }
        let imported = parse_archive(&bytes, "review.skill").unwrap();
        assert_eq!(imported.name, "review");
        assert_eq!(imported.files.len(), 1);
        assert_eq!(imported.files[0].path, "docs/a.md");
    }

    #[test]
    fn frontmatter_and_bundle_caps_are_stable() {
        assert_eq!(
            parse_frontmatter("---\nname: x\ndescription: y\n---\n").0,
            "x"
        );
        let mut imported = ImportedSkill::new("x".into(), String::new(), "body".into(), None);
        assert!(imported
            .add_file("logo.png".into(), "binary".into())
            .is_ok());
        assert!(imported.files.is_empty());
        assert!(matches!(
            imported.add_file("large.md".into(), "x".repeat(MAX_FILE_SIZE + 1)),
            Err(ImportError::Cap(_))
        ));
    }
}
