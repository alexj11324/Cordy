//! Slack-specific raw event envelope shared by the resolvers and media resolver
//! through [`cordy_channel::InboundMessage::raw`].
//!
//! Per the boundary rule, these fields are read ONLY inside this adapter; the
//! core never inspects `raw`.

use serde::{Deserialize, Serialize};

/// Carries the Slack-specific fields the cross-platform envelope does not —
/// read back only inside the Slack resolvers (team_id routes the installation;
/// the core never reads Raw).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SlackRawEvent {
    #[serde(rename = "team_id", default)]
    pub team_id: String,
    #[serde(rename = "api_app_id", default)]
    pub api_app_id: String,
    #[serde(rename = "event_type", default)]
    pub event_type: String,
    #[serde(rename = "subtype", default)]
    pub subtype: String,
    #[serde(rename = "channel_type", default)]
    pub channel_type: String,
    #[serde(rename = "files", default)]
    pub files: Vec<SlackRawFile>,
}

/// The subset of a Slack file object the media resolver needs to fetch the
/// file later, off the connector ACK path. The download URL is a Slack-hosted
/// url_private(_download) that requires the installation's bot token — it is
/// never handed to the core or persisted beyond Raw.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SlackRawFile {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub mimetype: String,
    #[serde(default)]
    pub size: i64,
    #[serde(rename = "download_url", default)]
    pub download_url: String,
}

/// Decodes the adapter's own raw payload back out of an inbound message.
pub fn decode_slack_raw(msg: &cordy_channel::InboundMessage) -> anyhow::Result<SlackRawEvent> {
    if msg.raw.is_null() {
        anyhow::bail!("slack: inbound message Raw is empty");
    }
    serde_json::from_value(msg.raw.clone())
        .map_err(|e| anyhow::anyhow!("decode slack inbound raw: {e}"))
}

/// Reports whether host is Slack's own. url_private URLs live on
/// files.slack.com (enterprise variants are still *.slack.com subdomains).
pub fn is_slack_file_host(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    host == "slack.com" || host.ends_with(".slack.com")
}

/// The pure form of the download-URL validation, pinned to the production
/// host predicate. The inbound translation uses it to drop files the resolver
/// could never fetch before they ever reach the envelope.
pub fn is_fetchable_slack_file_url(raw_url: &str) -> bool {
    let Ok(parsed) = url::Url::parse(raw_url) else {
        return false;
    };
    if parsed.host_str().is_none_or(str::is_empty) || !parsed.username().is_empty() {
        return false;
    }
    parsed.scheme().eq_ignore_ascii_case("https")
        && parsed.host_str().is_some_and(is_slack_file_host)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slack_host_predicate() {
        assert!(is_slack_file_host("files.slack.com"));
        assert!(is_slack_file_host("FILES.SLACK.COM"));
        assert!(is_slack_file_host("slack.com"));
        assert!(is_slack_file_host("files.origin.enterprise.slack.com"));
        assert!(!is_slack_file_host("notslack.com"));
        assert!(!is_slack_file_host("evil.slack.com.evil.io"));
        assert!(!is_slack_file_host(""));
    }

    #[test]
    fn fetchable_url_requires_https_on_slack_host() {
        assert!(is_fetchable_slack_file_url(
            "https://files.slack.com/files-pri/T1-F1/x.png"
        ));
        assert!(!is_fetchable_slack_file_url(
            "http://files.slack.com/files-pri/T1-F1/x.png"
        ));
        assert!(!is_fetchable_slack_file_url(
            "https://drive.google.com/file/d/x"
        ));
        assert!(!is_fetchable_slack_file_url(
            "https://user@files.slack.com/x.png"
        ));
        assert!(!is_fetchable_slack_file_url("not a url"));
    }

    #[test]
    fn decode_raw_roundtrips() {
        let msg = cordy_channel::InboundMessage {
            raw: serde_json::json!({
                "team_id": "T1",
                "api_app_id": "A1",
                "event_type": "message",
                "files": [{"id": "F1", "size": 3, "download_url": "https://files.slack.com/f"}],
            }),
            ..Default::default()
        };
        let raw = decode_slack_raw(&msg).unwrap();
        assert_eq!(raw.team_id, "T1");
        assert_eq!(raw.api_app_id, "A1");
        assert_eq!(raw.files.len(), 1);
        assert_eq!(raw.files[0].id, "F1");
    }

    #[test]
    fn decode_raw_rejects_missing_payload() {
        let msg = cordy_channel::InboundMessage::default();
        assert!(decode_slack_raw(&msg).is_err());
    }
}
