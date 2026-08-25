//! API transport errors, bounded response handling, and timeout parsing.
//!
//! Keeping these policy helpers separate from the request surface makes
//! network classification and error-body limits reviewable in isolation.

use reqwest::{Method, Response};
use std::fmt;
use std::time::Duration;
use thiserror::Error;

pub(crate) const DEFAULT_HTTP_TIMEOUT: Duration = Duration::from_secs(30);
const ERROR_BODY_LIMIT: usize = 4096;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ErrorKind {
    NetworkTimeout,
    NetworkDns,
    NetworkRefused,
    NetworkTls,
    NetworkOffline,
    AuthRequired,
    Forbidden,
    NotFound,
    Conflict,
    Validation,
    RateLimited,
    Server,
    Unknown,
}

#[derive(Debug, Error)]
#[error("{method} {path} returned {status_code}: {body}")]
pub struct HttpError {
    pub method: Method,
    pub path: String,
    pub status_code: u16,
    pub body: String,
}

impl HttpError {
    pub fn kind(&self) -> ErrorKind {
        match self.status_code {
            401 => ErrorKind::AuthRequired,
            403 => ErrorKind::Forbidden,
            404 => ErrorKind::NotFound,
            409 => ErrorKind::Conflict,
            400 | 422 => ErrorKind::Validation,
            429 => ErrorKind::RateLimited,
            500..=599 => ErrorKind::Server,
            _ => ErrorKind::Unknown,
        }
    }
}

#[derive(Debug, Error)]
#[error("{op}: {source}")]
pub struct NetworkError {
    pub kind: ErrorKind,
    pub op: String,
    #[source]
    pub source: reqwest::Error,
}

/// Errors returned by the unauthenticated setup preflight.  The probe must
/// not leak a bearer token, response body, or an untrusted redirect target in
/// the command error: setup runs before a new profile is persisted.
#[derive(Debug, Error)]
pub enum HealthProbeError {
    #[error("health probe URL is invalid")]
    InvalidUrl,
    #[error("health probe only supports http(s) URLs")]
    UnsupportedScheme,
    #[error("health probe timeout")]
    Timeout,
    #[error("health probe request failed ({kind})")]
    Request { kind: ErrorKind },
    #[error("health endpoint returned HTTP {status_code}")]
    Unhealthy { status_code: u16 },
}

pub fn http_timeout(raw: Option<&str>) -> Duration {
    let Some(raw) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
        return DEFAULT_HTTP_TIMEOUT;
    };
    parse_go_duration(raw)
        .or_else(|| raw.parse::<u64>().ok().map(Duration::from_secs))
        .filter(|duration| !duration.is_zero())
        .unwrap_or(DEFAULT_HTTP_TIMEOUT)
}

pub(crate) async fn read_http_error(
    method: Method,
    path: &str,
    mut response: Response,
) -> HttpError {
    let status_code = response.status().as_u16();
    let mut body = Vec::with_capacity(ERROR_BODY_LIMIT);
    while body.len() < ERROR_BODY_LIMIT {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                let remaining = ERROR_BODY_LIMIT - body.len();
                body.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
            }
            Ok(None) | Err(_) => break,
        }
    }
    HttpError {
        method,
        path: path.into(),
        status_code,
        body: String::from_utf8_lossy(&body).trim().into(),
    }
}

pub(crate) fn classify_network_error(error: &reqwest::Error) -> ErrorKind {
    if error.is_timeout() {
        return ErrorKind::NetworkTimeout;
    }
    let message = error.to_string().to_lowercase();
    match () {
        () if message.contains("dns")
            || message.contains("no such host")
            || message.contains("name resolution") =>
        {
            ErrorKind::NetworkDns
        }
        () if message.contains("connection refused") => ErrorKind::NetworkRefused,
        () if message.contains("tls")
            || message.contains("certificate")
            || message.contains("x509") =>
        {
            ErrorKind::NetworkTls
        }
        () => ErrorKind::NetworkOffline,
    }
}

fn parse_go_duration(raw: &str) -> Option<Duration> {
    if raw.is_empty() || raw.starts_with('-') {
        return None;
    }
    let mut rest = raw;
    let mut seconds = 0.0_f64;
    while !rest.is_empty() {
        let number_len = rest
            .char_indices()
            .take_while(|(_, character)| character.is_ascii_digit() || *character == '.')
            .map(|(index, character)| index + character.len_utf8())
            .last()?;
        let value = rest[..number_len].parse::<f64>().ok()?;
        rest = &rest[number_len..];
        let (unit, multiplier) = [
            ("ns", 1e-9),
            ("us", 1e-6),
            ("µs", 1e-6),
            ("ms", 1e-3),
            ("s", 1.0),
            ("m", 60.0),
            ("h", 3600.0),
        ]
        .into_iter()
        .find(|(unit, _)| rest.starts_with(unit))?;
        rest = &rest[unit.len()..];
        seconds += value * multiplier;
    }
    (seconds.is_finite() && seconds >= 0.0 && seconds < Duration::MAX.as_secs_f64())
        .then(|| Duration::from_secs_f64(seconds))
}

pub(crate) fn normalized_os() -> &'static str {
    std::env::consts::OS
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}
