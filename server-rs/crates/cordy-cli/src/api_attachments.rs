use anyhow::Result;
use reqwest::Method;
use serde::Deserialize;
use serde_json::Value;

use super::api::ApiClient;

#[derive(Debug, Deserialize)]
pub struct AttachmentUploadResponse {
    pub id: String,
    #[serde(default)]
    pub markdown_url: String,
    #[serde(default)]
    pub content_type: String,
}

#[derive(Debug, Deserialize)]
pub struct FileUploadResponse {
    pub id: String,
    pub url: String,
}

impl ApiClient {
    pub async fn upload_file(
        &self,
        file_data: Vec<u8>,
        filename: &str,
        issue_id: &str,
    ) -> Result<String> {
        let filename = std::path::Path::new(filename)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(filename)
            .to_string();
        let part = reqwest::multipart::Part::bytes(file_data).file_name(filename);
        let mut form = reqwest::multipart::Form::new().part("file", part);
        if !issue_id.is_empty() {
            form = form.text("issue_id", issue_id.to_string());
        }
        let result: Value = self
            .send_json(
                Method::POST,
                "/api/upload-file",
                self.request(Method::POST, "/api/upload-file")
                    .multipart(form),
            )
            .await?;
        let id = result.get("id").and_then(Value::as_str).unwrap_or_default();
        if id.is_empty() {
            anyhow::bail!("upload response missing attachment id");
        }
        Ok(id.into())
    }

    pub async fn upload_file_with_url(
        &self,
        file_data: Vec<u8>,
        filename: &str,
    ) -> Result<FileUploadResponse> {
        let filename = std::path::Path::new(filename)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(filename)
            .to_string();
        let form = reqwest::multipart::Form::new().part(
            "file",
            reqwest::multipart::Part::bytes(file_data).file_name(filename),
        );
        let result: FileUploadResponse = self
            .send_json(
                Method::POST,
                "/api/upload-file",
                self.request(Method::POST, "/api/upload-file")
                    .multipart(form),
            )
            .await?;
        if result.id.is_empty() {
            anyhow::bail!("upload response missing attachment id");
        }
        Ok(result)
    }

    pub async fn upload_chat_attachment(
        &self,
        file_data: Vec<u8>,
        filename: &str,
        task_id: &str,
    ) -> Result<AttachmentUploadResponse> {
        let filename = std::path::Path::new(filename)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(filename)
            .to_string();
        let form = reqwest::multipart::Form::new()
            .part(
                "file",
                reqwest::multipart::Part::bytes(file_data).file_name(filename),
            )
            .text("task_id", task_id.to_string());
        let result: AttachmentUploadResponse = self
            .send_json(
                Method::POST,
                "/api/upload-file",
                self.request(Method::POST, "/api/upload-file")
                    .multipart(form),
            )
            .await?;
        if result.id.is_empty() {
            anyhow::bail!("upload response missing attachment id");
        }
        Ok(result)
    }
}
