//! User-facing error classification ported from `server/internal/cli/errors.go`.

use crate::api::{ErrorKind, HttpError, NetworkError};
use crate::config::Environment;
use crate::RunOutput;
use anyhow::Error;
use std::io::{self, Write};

#[derive(Debug)]
struct CommandOutputError {
    output: RunOutput,
    cause: anyhow::Error,
}

impl std::fmt::Display for CommandOutputError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.cause.fmt(formatter)
    }
}

impl std::error::Error for CommandOutputError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.cause.as_ref())
    }
}

pub(super) fn command_output_error(output: RunOutput, cause: anyhow::Error) -> anyhow::Error {
    anyhow::Error::new(CommandOutputError { output, cause })
}

pub fn command_error_output(error: &anyhow::Error) -> Option<&RunOutput> {
    error.chain().find_map(|cause| {
        cause
            .downcast_ref::<CommandOutputError>()
            .map(|error| &error.output)
    })
}

impl super::Cli {
    pub fn debug_enabled(&self, environment: &Environment) -> bool {
        self.debug
            || environment.trimmed("CORDY_DEBUG").is_some_and(|value| {
                !matches!(
                    value.to_ascii_lowercase().as_str(),
                    "0" | "false" | "no" | "off"
                )
            })
    }
}

pub fn format_error(error: &Error, debug: bool) -> String {
    let chinese = detect_chinese_locale();
    let base = if let Some(network) = find_network_error(error) {
        message_for(network.kind, chinese)
    } else if let Some(http) = find_http_error(error) {
        http_message(http, chinese)
    } else {
        error.to_string()
    };
    if debug {
        let mut detail = format!("{base}\n\n[debug] {error:#}");
        if let Some(network) = find_network_error(error) {
            detail.push_str(&format!(
                "\n[debug] network: op={:?} kind={:?} cause={}",
                network.op, network.kind, network.source
            ));
        }
        if let Some(http) = find_http_error(error) {
            detail.push_str(&format!(
                "\n[debug] http: {} {} status={} body={}",
                http.method,
                http.path,
                http.status_code,
                http.body.trim()
            ));
        }
        detail
    } else {
        base
    }
}

/// Writes CLI stdout/stderr. A closed pipe (`head`, `true`, etc.) is a
/// normal termination, not a panic or exit-code-101 failure.
pub fn write_output(mut writer: impl Write, data: &str) -> io::Result<()> {
    if data.is_empty() {
        return Ok(());
    }
    match writer.write_all(data.as_bytes()) {
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        other => other,
    }
}

pub fn exit_code(error: &Error) -> i32 {
    if find_network_error(error).is_some() {
        return 2;
    }
    find_http_error(error).map_or(1, |http| match http.kind() {
        ErrorKind::AuthRequired | ErrorKind::Forbidden => 3,
        ErrorKind::NotFound => 4,
        ErrorKind::Validation => 5,
        _ => 1,
    })
}

fn find_network_error(error: &Error) -> Option<&NetworkError> {
    error.chain().find_map(|cause| cause.downcast_ref())
}

fn find_http_error(error: &Error) -> Option<&HttpError> {
    error.chain().find_map(|cause| cause.downcast_ref())
}

fn http_message(error: &HttpError, chinese: bool) -> String {
    match error.kind() {
        ErrorKind::Validation | ErrorKind::Conflict => extract_server_message(&error.body)
            .map(|message| {
                let prefix = if chinese {
                    if error.kind() == ErrorKind::Validation {
                        "请求无效："
                    } else {
                        "请求冲突："
                    }
                } else if error.kind() == ErrorKind::Validation {
                    "Invalid request: "
                } else {
                    "Request conflict: "
                };
                format!("{prefix}{message}")
            })
            .unwrap_or_else(|| message_for(error.kind(), chinese)),
        kind => message_for(kind, chinese),
    }
}

fn message_for(kind: ErrorKind, chinese: bool) -> String {
    let messages = match kind {
        ErrorKind::NetworkTimeout => ("Request timed out: the server did not respond in time. Check your network connection or try again later. You can raise the limit with CORDY_HTTP_TIMEOUT.", "请求超时：服务器未在规定时间内响应。请检查网络连接或稍后重试。可通过 CORDY_HTTP_TIMEOUT 调高超时时间。"),
        ErrorKind::NetworkDns => ("Could not resolve the Cordy server address. Check your network connection or the --server-url setting.", "无法解析 Cordy 服务器地址。请检查网络连接或 --server-url 配置。"),
        ErrorKind::NetworkRefused => ("Could not connect to the Cordy server. Make sure the server address is correct and reachable.", "无法连接到 Cordy 服务器。请确认服务器地址正确且网络可达。"),
        ErrorKind::NetworkTls => ("Could not establish a secure connection to the Cordy server (TLS/certificate error). Check your system clock and CA certificates.", "无法与 Cordy 服务器建立安全连接（TLS/证书错误）。请检查系统时间和 CA 证书。"),
        ErrorKind::NetworkOffline => ("Could not reach the Cordy server. Check your network connection.", "无法访问 Cordy 服务器。请检查网络连接。"),
        ErrorKind::AuthRequired => ("Your session has expired or you are not signed in. Run `cordy login` to sign in again. On a self-hosted or non-OAuth setup, ask your administrator for valid credentials.", "登录已过期或尚未登录。请运行 `cordy login` 重新登录。自托管或非 OAuth 场景请联系管理员获取有效凭证。"),
        ErrorKind::Forbidden => ("You do not have permission to access this resource. Check that you are in the right workspace, or ask an administrator to grant access.", "无权访问该资源。请确认当前 workspace 是否正确，或联系管理员授予权限。"),
        ErrorKind::NotFound => ("The requested resource was not found. Check the ID, or run the matching `list` command to see what exists in this workspace.", "未找到请求的资源。请核对 ID，或运行对应的 list 命令查看当前 workspace 中已有的内容。"),
        ErrorKind::Conflict => ("The request conflicts with the current state of the resource (it may already exist or have changed since you last fetched it). Re-fetch the latest state and try again.", "请求与资源的当前状态冲突（可能已存在，或自上次获取后已被修改）。请重新获取最新状态后再试。"),
        ErrorKind::Validation => ("The request was invalid. Check the values you provided; run the command with --help to see the expected format.", "请求无效。请检查所填写的参数；可用 --help 查看期望的格式。"),
        ErrorKind::RateLimited => ("Too many requests. Please wait a moment and try again; if this keeps happening, reduce how frequently you call the API.", "请求过于频繁。请稍候重试；若持续出现，请降低 API 调用频率。"),
        ErrorKind::Server => ("The Cordy service is temporarily unavailable (server error). Please try again later; if it persists, contact support. Re-run with --debug to see the raw server response.", "Cordy 服务暂时不可用（服务器错误）。请稍后重试；若持续出现请联系支持。可加 --debug 查看服务器原始响应。"),
        ErrorKind::Unknown => ("An unexpected error occurred.", "发生未知错误。"),
    };
    if chinese { messages.1 } else { messages.0 }.into()
}

fn detect_chinese_locale() -> bool {
    for key in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        let Ok(value) = std::env::var(key) else {
            continue;
        };
        let value = value.trim().to_ascii_lowercase();
        if !value.is_empty() {
            return value.starts_with("zh");
        }
    }
    false
}

fn extract_server_message(body: &str) -> Option<String> {
    let object = serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(body).ok()?;
    let mut code = None;
    for key in ["error", "message", "detail", "title"] {
        let Some(value) = object.get(key).and_then(serde_json::Value::as_str) else {
            continue;
        };
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        if value.chars().all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || "_-.".contains(character)
        }) {
            code.get_or_insert_with(|| value.to_owned());
        } else {
            return Some(value.to_owned());
        }
    }
    code
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::Method;

    #[test]
    fn validation_surfaces_actionable_server_message() {
        let error = Error::new(HttpError {
            method: Method::GET,
            path: "/api/me".into(),
            status_code: 422,
            body: r#"{"error":"bad_profile","message":"Profile is invalid"}"#.into(),
        })
        .context("get user profile");
        let http = find_http_error(&error).expect("HTTP error");
        assert_eq!(
            http_message(http, false),
            "Invalid request: Profile is invalid"
        );
        assert_eq!(exit_code(&error), 5);
        let debug = format_error(&error, true);
        assert!(
            debug.contains("get user profile"),
            "debug output should include the outer context: {debug}"
        );
        assert!(
            debug.contains("HTTP 422")
                || debug.contains("422")
                || debug.contains("Profile is invalid"),
            "debug output should include the cause chain: {debug}"
        );
        assert!(debug.contains("[debug]"));
    }

    #[test]
    fn write_output_treats_broken_pipe_as_success() {
        struct BrokenPipeWriter;
        impl Write for BrokenPipeWriter {
            fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
                Err(io::Error::new(io::ErrorKind::BrokenPipe, "closed"))
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        write_output(BrokenPipeWriter, "hello\n").expect("broken pipe is success");
    }
}
