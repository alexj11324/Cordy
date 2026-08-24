use std::sync::Arc;
use std::time::Duration;

use axum::http::HeaderMap;
use cordy_db::models::Attachment;
use url::Url;

use crate::cloudfront::CloudFrontSigner;

const STABLE_URL_CAPABILITY: &str = "stable_attachment_urls";

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct AttachmentUrls {
    pub download_url: String,
    pub markdown_url: String,
}

/// The subset of deployment configuration needed to render attachment rows.
/// Download routes and storage I/O remain in their own migration layers.
#[derive(Clone)]
pub struct AttachmentUrlPolicy {
    public_url: String,
    storage_has_public_base: bool,
    ttl: Duration,
    signer: Option<Arc<CloudFrontSigner>>,
}

impl Default for AttachmentUrlPolicy {
    fn default() -> Self {
        Self {
            public_url: String::new(),
            storage_has_public_base: false,
            ttl: Duration::from_secs(30 * 60),
            signer: None,
        }
    }
}

impl AttachmentUrlPolicy {
    pub async fn from_config(config: &cordy_config::Config) -> anyhow::Result<Self> {
        let ttl = config
            .storage
            .attachment_download_url_ttl
            .as_deref()
            .map(parse_ttl)
            .transpose()?
            .unwrap_or_else(|| Duration::from_secs(30 * 60));
        let signer = CloudFrontSigner::from_config(config).await?.map(Arc::new);
        let storage_has_public_base = [
            config.storage.local_upload_base_url.as_deref(),
            config.storage.cloudfront_domain.as_deref(),
        ]
        .into_iter()
        .flatten()
        .any(|value| !value.trim().is_empty());
        Ok(Self {
            public_url: config
                .urls
                .public_url
                .as_deref()
                .unwrap_or_default()
                .trim()
                .trim_end_matches('/')
                .to_string(),
            storage_has_public_base,
            ttl,
            signer,
        })
    }

    pub(crate) fn urls(&self, headers: &HeaderMap, attachment: &Attachment) -> AttachmentUrls {
        let stable = format!("/api/attachments/{}/download", attachment.id);
        let download_url = if request_has_stable_urls(headers) {
            stable.clone()
        } else if let Some(signer) = self.signer.as_ref() {
            signer
                .signed_url(&attachment.url, self.ttl)
                .unwrap_or_else(|error| {
                    tracing::warn!(
                        %error,
                        attachment_id = %attachment.id,
                        "failed to sign attachment URL"
                    );
                    stable.clone()
                })
        } else {
            stable.clone()
        };
        let markdown_url = if self.storage_has_public_base
            && self.signer.is_none()
            && is_durable_public_url(&attachment.url)
        {
            attachment.url.clone()
        } else if self.public_url.is_empty() {
            stable.clone()
        } else {
            format!("{}{}", self.public_url, stable)
        };
        AttachmentUrls {
            download_url,
            markdown_url,
        }
    }
}

fn request_has_stable_urls(headers: &HeaderMap) -> bool {
    headers
        .get_all("x-client-capabilities")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .any(|value| value.trim() == STABLE_URL_CAPABILITY)
}

fn is_durable_public_url(raw: &str) -> bool {
    let Ok(url) = Url::parse(raw) else {
        return false;
    };
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return false;
    }
    let expiring = [
        "signature",
        "x-amz-signature",
        "key-pair-id",
        "expires",
        "x-amz-expires",
    ];
    !url.query_pairs().any(|(key, _)| {
        expiring
            .iter()
            .any(|candidate| key.eq_ignore_ascii_case(candidate))
    })
}

fn parse_ttl(raw: &str) -> anyhow::Result<Duration> {
    let raw = raw.trim();
    anyhow::ensure!(
        !raw.is_empty(),
        "ATTACHMENT_DOWNLOAD_URL_TTL cannot be empty"
    );
    let split = raw
        .find(|character: char| !character.is_ascii_digit())
        .unwrap_or(raw.len());
    let amount = raw[..split].parse::<u64>()?;
    anyhow::ensure!(amount > 0, "ATTACHMENT_DOWNLOAD_URL_TTL must be positive");
    let multiplier = match &raw[split..] {
        "" | "s" => 1,
        "m" => 60,
        "h" => 60 * 60,
        "d" => 24 * 60 * 60,
        _ => anyhow::bail!("ATTACHMENT_DOWNLOAD_URL_TTL must use s, m, h, or d"),
    };
    Ok(Duration::from_secs(
        amount
            .checked_mul(multiplier)
            .ok_or_else(|| anyhow::anyhow!("ATTACHMENT_DOWNLOAD_URL_TTL is too large"))?,
    ))
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use uuid::Uuid;

    use super::*;

    fn attachment(url: &str) -> Attachment {
        Attachment {
            chat_message_id: None,
            chat_session_id: None,
            comment_id: None,
            content_type: "image/png".into(),
            created_at: Utc.with_ymd_and_hms(2026, 8, 23, 3, 30, 0).unwrap(),
            filename: "diagram.png".into(),
            id: Uuid::parse_str("018f03a0-c4d2-7a37-ae4d-5aa45de12f13").unwrap(),
            issue_id: None,
            size_bytes: 42,
            task_id: None,
            uploader_id: Uuid::nil(),
            uploader_type: "member".into(),
            url: url.into(),
            workspace_id: Uuid::nil(),
        }
    }

    #[test]
    fn private_deployments_sign_default_download_and_anchor_markdown() {
        let policy = AttachmentUrlPolicy {
            public_url: "https://api.cordy.test".into(),
            storage_has_public_base: true,
            ttl: Duration::from_secs(1800),
            signer: Some(Arc::new(CloudFrontSigner::for_test())),
        };
        let row = attachment("https://cdn.cordy.test/workspaces/w/diagram.png");

        let urls = policy.urls(&HeaderMap::new(), &row);
        assert!(urls.download_url.starts_with(&row.url));
        assert!(urls.download_url.contains("Key-Pair-Id=KTEST"));
        assert_eq!(
            urls.markdown_url,
            format!("https://api.cordy.test/api/attachments/{}/download", row.id)
        );

        let mut stable_headers = HeaderMap::new();
        stable_headers.insert(
            "x-client-capabilities",
            "other, stable_attachment_urls".parse().unwrap(),
        );
        let stable = policy.urls(&stable_headers, &row);
        assert_eq!(
            stable.download_url,
            format!("/api/attachments/{}/download", row.id)
        );
    }

    #[test]
    fn public_storage_keeps_durable_raw_markdown_url() {
        let policy = AttachmentUrlPolicy {
            public_url: "https://api.cordy.test".into(),
            storage_has_public_base: true,
            ttl: Duration::from_secs(1800),
            signer: None,
        };
        let row = attachment("https://cdn.cordy.test/workspaces/w/diagram.png");
        let urls = policy.urls(&HeaderMap::new(), &row);
        assert_eq!(urls.markdown_url, row.url);

        let expiring =
            attachment("https://cdn.cordy.test/workspaces/w/diagram.png?X-Amz-Signature=temporary");
        let urls = policy.urls(&HeaderMap::new(), &expiring);
        assert_eq!(
            urls.markdown_url,
            format!(
                "https://api.cordy.test/api/attachments/{}/download",
                expiring.id
            )
        );
    }

    #[test]
    fn ttl_parser_matches_go_duration_units_used_by_deployments() {
        assert_eq!(parse_ttl("30m").unwrap(), Duration::from_secs(1800));
        assert_eq!(parse_ttl("2h").unwrap(), Duration::from_secs(7200));
        assert!(parse_ttl("0s").is_err());
        assert!(parse_ttl("1ms").is_err());
    }
}
