//! Authenticated API request construction.
//!
//! Header and timeout policy is kept separate from response transport so all
//! CLI API methods share one request contract.

use reqwest::{Method, RequestBuilder};

use super::api::{normalized_os, ApiClient};

const CLIENT_CAPABILITIES: &str = "stable_attachment_urls";

impl ApiClient {
    pub(super) fn request(&self, method: Method, path: &str) -> RequestBuilder {
        let mut request = self
            .client
            .request(method, format!("{}{path}", self.base_url))
            .header("X-Client-Capabilities", CLIENT_CAPABILITIES)
            .header("X-Client-Platform", "cli")
            .header("X-Client-Version", self.version)
            .header("X-Client-OS", normalized_os());
        if !self.token.is_empty() {
            request = request.bearer_auth(&self.token);
        }
        if !self.workspace_id.is_empty() {
            request = request.header("X-Workspace-ID", &self.workspace_id);
        }
        if !self.agent_id.is_empty() {
            request = request.header("X-Agent-ID", &self.agent_id);
        }
        if !self.task_id.is_empty() {
            request = request.header("X-Task-ID", &self.task_id);
        }

        if let Some(timeout) = self.request_timeout {
            request = request.timeout(timeout);
        }
        request
    }
}
