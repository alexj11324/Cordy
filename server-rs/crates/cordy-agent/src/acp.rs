//! Sequential ACP JSON-RPC transport with headless permission handling.

use std::io;
use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;
use tokio::io::{AsyncBufRead, AsyncWrite, AsyncWriteExt};

use crate::stream::AgentLineReader;

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
}

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
        }
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

        loop {
            let line = self
                .reader
                .next_line()
                .await?
                .ok_or_else(|| AcpError::UnexpectedEof(method.to_string()))?;
            let Ok(frame) = serde_json::from_str::<Value>(line.trim()) else {
                continue;
            };
            let Some(frame) = frame.as_object() else {
                continue;
            };
            let frame_id = frame.get("id").and_then(Value::as_u64);
            let frame_method = frame.get("method").and_then(Value::as_str);
            match (frame_id, frame_method) {
                (Some(request_id), Some(agent_method)) => {
                    self.answer_agent_request(request_id, agent_method, frame.get("params"))
                        .await?;
                }
                (Some(response_id), None) if response_id == id => {
                    if let Some(error) = frame.get("error") {
                        return Err(rpc_error(method, error));
                    }
                    return Ok(frame.get("result").cloned().unwrap_or(Value::Null));
                }
                (None, Some(notification_method)) => on_notification(AcpNotification {
                    method: notification_method.to_string(),
                    params: frame.get("params").cloned().unwrap_or(Value::Null),
                }),
                _ => {}
            }
        }
    }

    /// Drains post-response notifications until the wire has been quiet for
    /// `quiet` or the absolute `maximum` bound expires. Some ACP agents emit
    /// their final message/tool update immediately after the terminal prompt
    /// response; treating that response as EOF loses the user-facing tail.
    pub async fn drain_notifications(
        &mut self,
        quiet: Duration,
        maximum: Duration,
        mut on_notification: impl FnMut(AcpNotification),
    ) -> Result<(), AcpError> {
        let deadline = tokio::time::Instant::now() + maximum;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return Ok(());
            }
            let wait = quiet.min(remaining);
            let line = tokio::select! {
                line = self.reader.next_line() => line?,
                () = tokio::time::sleep(wait) => return Ok(()),
            };
            let Some(line) = line else {
                return Ok(());
            };
            let Ok(frame) = serde_json::from_str::<Value>(line.trim()) else {
                continue;
            };
            let Some(frame) = frame.as_object() else {
                continue;
            };
            let frame_id = frame.get("id").and_then(Value::as_u64);
            let frame_method = frame.get("method").and_then(Value::as_str);
            match (frame_id, frame_method) {
                (Some(request_id), Some(agent_method)) => {
                    self.answer_agent_request(request_id, agent_method, frame.get("params"))
                        .await?;
                }
                (None, Some(notification_method)) => on_notification(AcpNotification {
                    method: notification_method.to_string(),
                    params: frame.get("params").cloned().unwrap_or(Value::Null),
                }),
                _ => {}
            }
        }
    }

    async fn answer_agent_request(
        &mut self,
        id: u64,
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
        if let Some(option_id) = select_permission(params) {
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
            agent_write.write_all(br#"{"jsonrpc":"2.0","id":91,"method":"session/request_permission","params":{"options":[{"optionId":"permanent","kind":"allow_always"},{"optionId":"once","kind":"allow_once"}]}}
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
