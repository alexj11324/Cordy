//! Sequential ACP JSON-RPC transport with headless permission handling.

use std::io;
use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;
use tokio::io::{AsyncBufRead, AsyncWrite, AsyncWriteExt};

use crate::stream::AgentLineReader;

pub const ACP_NOTIFICATION_QUIET_TIME: Duration = Duration::from_millis(250);
pub const ACP_NOTIFICATION_DRAIN_GRACE: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, PartialEq)]
pub struct AcpNotification {
    pub method: String,
    pub params: Value,
}

#[derive(Debug, thiserror::Error)]
pub enum AcpError {
    #[error("ACP transport error: {0}")]
    Transport(#[from] io::Error),
    #[error("serialize ACP request: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("{method}: {message} (code={code}{data})")]
    Rpc {
        method: String,
        code: i64,
        message: String,
        data: String,
    },
    #[error("ACP stream ended while awaiting {0}")]
    UnexpectedEof(String),
    #[error("invalid ACP response for {0}: missing result or error")]
    InvalidResponse(String),
}

impl AcpError {
    pub fn is_session_not_found(&self) -> bool {
        let Self::Rpc {
            code,
            message,
            data,
            ..
        } = self
        else {
            return false;
        };
        if !matches!(code, -32603 | -32602 | -32002) {
            return false;
        }
        let text = format!("{message} {data}").to_ascii_lowercase();
        ["session not found", "no session found", "unknown session"]
            .iter()
            .any(|needle| text.contains(needle))
    }
}

pub struct AcpClient<R, W> {
    reader: AgentLineReader<R>,
    writer: W,
    next_id: u64,
    permission_selector: Box<PermissionSelector>,
}

type PermissionSelector = dyn FnMut(Option<&Value>) -> Option<String> + Send;

impl<R, W> AcpClient<R, W>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    pub fn new(reader: R, writer: W) -> Self {
        Self {
            reader: AgentLineReader::new(reader),
            writer,
            next_id: 1,
            permission_selector: Box::new(select_permission),
        }
    }

    /// Replaces the generic headless permission policy with the provider's
    /// own offered-option selector. The selector must return an option ID
    /// that appeared in the request, or `None` to fail closed.
    pub fn with_permission_selector<F>(mut self, selector: F) -> Self
    where
        F: FnMut(Option<&Value>) -> Option<String> + Send + 'static,
    {
        self.permission_selector = Box::new(selector);
        self
    }

    /// Sends one request and continues servicing agent→client requests and
    /// notifications until its matching response arrives. ACP requests in a
    /// daemon turn are deliberately sequential, so an unrelated response is
    /// ignored rather than being misdelivered to the active method.
    pub async fn request(
        &mut self,
        method: &str,
        params: Value,
        mut on_notification: impl FnMut(AcpNotification),
    ) -> Result<Value, AcpError> {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.write(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }))
        .await?;

        let expected_id = Value::from(id);
        loop {
            let frame = self
                .next_frame()
                .await?
                .ok_or_else(|| AcpError::UnexpectedEof(method.to_string()))?;
            let has_id = frame.contains_key("id");
            let frame_id = frame.get("id");
            let frame_method = frame.get("method").and_then(Value::as_str);
            if has_id {
                if let Some(agent_method) = frame_method {
                    self.answer_agent_request(
                        frame_id.unwrap_or(&Value::Null),
                        agent_method,
                        frame.get("params"),
                    )
                    .await?;
                } else if frame_id == Some(&expected_id) {
                    if let Some(error) = frame.get("error") {
                        return Err(rpc_error(method, error));
                    }
                    if let Some(result) = frame.get("result") {
                        return Ok(result.clone());
                    }
                    return Err(AcpError::InvalidResponse(method.to_string()));
                }
            } else if let Some(notification_method) = frame_method {
                on_notification(AcpNotification {
                    method: notification_method.to_string(),
                    params: frame.get("params").cloned().unwrap_or(Value::Null),
                });
            }
        }
    }

    /// Drains notifications emitted after a completed request. The drain ends
    /// after `quiet` without a notification, EOF, or the `hard` bound. Agent
    /// requests continue to receive responses during the drain.
    pub async fn drain_notifications(
        &mut self,
        quiet: Duration,
        hard: Duration,
        mut on_notification: impl FnMut(AcpNotification),
    ) -> Result<(), AcpError> {
        let hard_deadline = tokio::time::Instant::now() + hard;
        let mut quiet_deadline = tokio::time::Instant::now() + quiet;
        loop {
            let frame = tokio::select! {
                _ = tokio::time::sleep_until(hard_deadline) => return Ok(()),
                _ = tokio::time::sleep_until(quiet_deadline) => return Ok(()),
                frame = self.next_frame() => frame?,
            };
            let Some(frame) = frame else {
                return Ok(());
            };
            let has_id = frame.contains_key("id");
            let frame_id = frame.get("id");
            let frame_method = frame.get("method").and_then(Value::as_str);
            if has_id {
                if let Some(agent_method) = frame_method {
                    self.answer_agent_request(
                        frame_id.unwrap_or(&Value::Null),
                        agent_method,
                        frame.get("params"),
                    )
                    .await?;
                }
            } else if let Some(notification_method) = frame_method {
                on_notification(AcpNotification {
                    method: notification_method.to_string(),
                    params: frame.get("params").cloned().unwrap_or(Value::Null),
                });
                quiet_deadline = tokio::time::Instant::now() + quiet;
            }
        }
    }

    async fn next_frame(&mut self) -> Result<Option<serde_json::Map<String, Value>>, AcpError> {
        loop {
            let Some(line) = self.reader.next_line().await? else {
                return Ok(None);
            };
            let Ok(Value::Object(frame)) = serde_json::from_str::<Value>(line.trim()) else {
                continue;
            };
            return Ok(Some(frame));
        }
    }

    async fn answer_agent_request(
        &mut self,
        id: &Value,
        method: &str,
        params: Option<&Value>,
    ) -> Result<(), AcpError> {
        if method != "session/request_permission" {
            return self
                .write(&serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": {"code": -32601, "message": format!("method not found: {method}")},
                }))
                .await;
        }
        if let Some(option_id) = (self.permission_selector)(params) {
            return self
                .write(&serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {"outcome": {"outcome": "selected", "optionId": option_id}},
                }))
                .await;
        }
        self.write(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {"code": -32603, "message": "no auto-selectable permission option offered"},
        }))
        .await
    }

    async fn write(&mut self, frame: &Value) -> Result<(), AcpError> {
        let mut encoded = serde_json::to_vec(frame)?;
        encoded.push(b'\n');
        self.writer.write_all(&encoded).await?;
        self.writer.flush().await?;
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
struct PermissionOption {
    #[serde(default, rename = "optionId")]
    id: String,
    #[serde(default)]
    kind: String,
}

fn select_permission(params: Option<&Value>) -> Option<String> {
    let options: Vec<PermissionOption> = params
        .and_then(|params| params.get("options"))
        .cloned()
        .and_then(|options| serde_json::from_value(options).ok())
        .unwrap_or_default();
    for wanted in ["allow_session", "approve_for_session"] {
        if let Some(option) = options.iter().find(|option| {
            option.id == wanted
                && matches!(
                    option.kind.trim().to_ascii_lowercase().as_str(),
                    "allow_once" | "allow_always"
                )
        }) {
            return Some(option.id.clone());
        }
    }
    for kind in ["allow_once", "reject_once"] {
        if let Some(option) = options
            .iter()
            .find(|option| !option.id.is_empty() && option.kind.trim().eq_ignore_ascii_case(kind))
        {
            return Some(option.id.clone());
        }
    }
    None
}

fn rpc_error(method: &str, error: &Value) -> AcpError {
    let code = error.get("code").and_then(Value::as_i64).unwrap_or(0);
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("RPC error without message")
        .to_string();
    let data = error
        .get("data")
        .filter(|data| !data.is_null())
        .map_or_else(
            || String::new(),
            |data| {
                let rendered = data
                    .as_str()
                    .map_or_else(|| data.to_string(), str::to_string);
                format!(", data={rendered}")
            },
        );
    AcpError::Rpc {
        method: method.to_string(),
        code,
        message,
        data,
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncBufReadExt, BufReader};

    use super::*;

    #[tokio::test]
    async fn request_services_permission_and_notification_before_response() {
        let (client_io, agent_io) = tokio::io::duplex(16 * 1024);
        let (client_read, client_write) = tokio::io::split(client_io);
        let (agent_read, mut agent_write) = tokio::io::split(agent_io);
        let agent = tokio::spawn(async move {
            let mut lines = BufReader::new(agent_read).lines();
            let request = lines
                .next_line()
                .await
                .unwrap_or_else(|error| panic!("read request: {error}"))
                .unwrap_or_else(|| panic!("request line"));
            assert!(request.contains("session/prompt"));
            agent_write.write_all(br#"{"jsonrpc":"2.0","id":"permission-91","method":"session/request_permission","params":{"options":[{"optionId":"permanent","kind":"allow_always"},{"optionId":"once","kind":"allow_once"}]}}
{"jsonrpc":"2.0","method":"session/update","params":{"update":{"type":"AgentMessageChunk"}}}
"#).await.unwrap_or_else(|error| panic!("write agent frames: {error}"));
            let permission = lines
                .next_line()
                .await
                .unwrap_or_else(|error| panic!("read permission: {error}"))
                .unwrap_or_else(|| panic!("permission line"));
            assert!(permission.contains("\"optionId\":\"once\""));
            assert!(!permission.contains("permanent"));
            agent_write
                .write_all(
                    br#"{"jsonrpc":"2.0","id":1,"result":{"stopReason":"end_turn"}}
"#,
                )
                .await
                .unwrap_or_else(|error| panic!("write response: {error}"));
        });
        let mut client = AcpClient::new(BufReader::new(client_read), client_write);
        let mut notifications = Vec::new();
        let result = client
            .request("session/prompt", serde_json::json!({}), |notification| {
                notifications.push(notification)
            })
            .await
            .unwrap_or_else(|error| panic!("ACP request: {error}"));
        agent
            .await
            .unwrap_or_else(|error| panic!("agent task: {error}"));
        assert_eq!(result["stopReason"], "end_turn");
        assert_eq!(notifications.len(), 1);
    }

    #[tokio::test]
    async fn request_rejects_response_without_result_or_error() {
        let (client_io, agent_io) = tokio::io::duplex(16 * 1024);
        let (client_read, client_write) = tokio::io::split(client_io);
        let (agent_read, mut agent_write) = tokio::io::split(agent_io);
        let agent = tokio::spawn(async move {
            let mut lines = BufReader::new(agent_read).lines();
            lines
                .next_line()
                .await
                .unwrap_or_else(|error| panic!("read request: {error}"));
            agent_write
                .write_all(
                    br#"{"jsonrpc":"2.0","id":1}
"#,
                )
                .await
                .unwrap_or_else(|error| panic!("write malformed response: {error}"));
        });
        let mut client = AcpClient::new(BufReader::new(client_read), client_write);
        let error = client
            .request("session/prompt", serde_json::json!({}), |_| {})
            .await
            .expect_err("missing result/error must fail the request");
        assert!(matches!(
            error,
            AcpError::InvalidResponse(method) if method == "session/prompt"
        ));
        agent
            .await
            .unwrap_or_else(|error| panic!("agent task: {error}"));
    }

    #[tokio::test]
    async fn drain_notifications_captures_updates_after_response() {
        let (client_io, agent_io) = tokio::io::duplex(16 * 1024);
        let (client_read, client_write) = tokio::io::split(client_io);
        let (agent_read, mut agent_write) = tokio::io::split(agent_io);
        let agent = tokio::spawn(async move {
            let mut lines = BufReader::new(agent_read).lines();
            lines
                .next_line()
                .await
                .unwrap_or_else(|error| panic!("read request: {error}"));
            agent_write
                .write_all(
                    br#"{"jsonrpc":"2.0","id":1,"result":{"stopReason":"end_turn"}}
"#,
                )
                .await
                .unwrap_or_else(|error| panic!("write response: {error}"));
            tokio::time::sleep(Duration::from_millis(10)).await;
            agent_write
                .write_all(br#"{"jsonrpc":"2.0","method":"session/update","params":{"update":{"type":"AgentMessageChunk"}}}
"#)
                .await
                .unwrap_or_else(|error| panic!("write trailing notification: {error}"));
        });
        let mut client = AcpClient::new(BufReader::new(client_read), client_write);
        client
            .request("session/prompt", serde_json::json!({}), |_| {})
            .await
            .unwrap_or_else(|error| panic!("ACP request: {error}"));
        let mut notifications = Vec::new();
        client
            .drain_notifications(
                Duration::from_millis(50),
                Duration::from_millis(500),
                |notification| notifications.push(notification),
            )
            .await
            .unwrap_or_else(|error| panic!("ACP notification drain: {error}"));
        agent
            .await
            .unwrap_or_else(|error| panic!("agent task: {error}"));
        assert_eq!(notifications.len(), 1);
        assert_eq!(notifications[0].method, "session/update");
    }

    #[tokio::test]
    async fn request_uses_provider_permission_selector() {
        let (client_io, agent_io) = tokio::io::duplex(16 * 1024);
        let (client_read, client_write) = tokio::io::split(client_io);
        let (agent_read, mut agent_write) = tokio::io::split(agent_io);
        let agent = tokio::spawn(async move {
            let mut lines = BufReader::new(agent_read).lines();
            lines
                .next_line()
                .await
                .unwrap_or_else(|error| panic!("read request: {error}"));
            agent_write
                .write_all(br#"{"jsonrpc":"2.0","id":"reasonix-permission","method":"session/request_permission","params":{"options":[{"optionId":"protected-decision","kind":"question"}]}}
"#)
                .await
                .unwrap_or_else(|error| panic!("write permission request: {error}"));
            let permission = lines
                .next_line()
                .await
                .unwrap_or_else(|error| panic!("read permission response: {error}"))
                .unwrap_or_else(|| panic!("permission response line"));
            assert!(permission.contains("\"optionId\":\"protected-decision\""));
            agent_write
                .write_all(
                    br#"{"jsonrpc":"2.0","id":1,"result":null}
"#,
                )
                .await
                .unwrap_or_else(|error| panic!("write response: {error}"));
        });
        let mut client = AcpClient::new(BufReader::new(client_read), client_write)
            .with_permission_selector(|params| {
                params?
                    .get("options")?
                    .as_array()?
                    .first()?
                    .get("optionId")?
                    .as_str()
                    .map(str::to_string)
            });
        client
            .request("session/prompt", serde_json::json!({}), |_| {})
            .await
            .unwrap_or_else(|error| panic!("ACP request: {error}"));
        agent
            .await
            .unwrap_or_else(|error| panic!("agent task: {error}"));
    }

    #[test]
    fn session_not_found_requires_known_rpc_code_and_wording() {
        let rejected = AcpError::Rpc {
            method: "session/prompt".into(),
            code: -32603,
            message: "Session not found".into(),
            data: String::new(),
        };
        assert!(rejected.is_session_not_found());
        let unrelated = AcpError::Rpc {
            method: "session/prompt".into(),
            code: -32000,
            message: "Session not found".into(),
            data: String::new(),
        };
        assert!(!unrelated.is_session_not_found());
    }
}
