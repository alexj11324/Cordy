use anyhow::Result;
use reqwest::Method;
use serde::de::DeserializeOwned;

use super::api::ApiClient;

impl ApiClient {
    /// Import a local skill archive through the same multipart endpoint used
    /// by the Go CLI. Archive parsing and all decompression/path limits remain
    /// server-side; this method only builds the authenticated request.
    pub async fn import_skill_file<T: DeserializeOwned>(
        &self,
        file_data: Vec<u8>,
        filename: &str,
        on_conflict: &str,
    ) -> Result<T> {
        let filename = std::path::Path::new(filename)
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .unwrap_or("skill.zip")
            .to_owned();
        let mut form = reqwest::multipart::Form::new().part(
            "file",
            reqwest::multipart::Part::bytes(file_data).file_name(filename),
        );
        if !on_conflict.is_empty() {
            form = form.text("on_conflict", on_conflict.to_owned());
        }
        self.send_json(
            Method::POST,
            "/api/skills/import",
            self.request(Method::POST, "/api/skills/import")
                .multipart(form),
        )
        .await
    }
}
