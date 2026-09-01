//! Localized copy shared by channel adapters.
//!
//! Channel adapters do not own a second set of translations.  They pass the
//! normalized message through this module, which accepts an adapter-provided
//! locale when one is available and uses a conservative script fallback for
//! older payloads that have no locale metadata.

use serde_json::Value;

use crate::message::InboundMessage;

const QUOTA_EN: &str = "⚠️ This workspace has reached its hosted messaging limit for the month. Existing runs will finish; upgrade to continue starting new runs.";
const QUOTA_ZH: &str =
    "⚠️ 本月托管消息额度已用尽。现有任务会继续完成；升级套餐后即可继续发送新的消息。";
const QUOTA_JA: &str = "⚠️ このワークスペースの今月のホスト型メッセージ上限に達しました。実行中のタスクは完了します。新しい実行を開始するにはプランをアップグレードしてください。";
const QUOTA_KO: &str = "⚠️ 이 워크스페이스의 이번 달 호스팅 메시지 한도에 도달했습니다. 진행 중인 실행은 완료됩니다. 새 실행을 시작하려면 요금제를 업그레이드하세요.";
const QUOTA_UNAVAILABLE_EN: &str = "⚠️ Hosted messaging usage is temporarily unavailable. No new run was started. Please try again later.";
const QUOTA_UNAVAILABLE_ZH: &str = "⚠️ 暂时无法读取托管消息额度，因此没有启动新任务。请稍后重试。";
const QUOTA_UNAVAILABLE_JA: &str = "⚠️ ホスト型メッセージの利用枠を一時的に確認できないため、新しい実行は開始されませんでした。しばらくしてからもう一度お試しください。";
const QUOTA_UNAVAILABLE_KO: &str = "⚠️ 호스팅 메시지 사용량을 일시적으로 확인할 수 없어 새 실행을 시작하지 않았습니다. 잠시 후 다시 시도하세요.";

/// Returns the shared quota notice for a locale tag such as `zh-Hans`, `ja`
/// or `ko`. Unknown and missing tags intentionally fall back to English.
pub fn quota_exceeded_notice(locale: Option<&str>) -> &'static str {
    let locale = locale.unwrap_or_default().to_ascii_lowercase();
    if locale.starts_with("zh") {
        QUOTA_ZH
    } else if locale.starts_with("ja") {
        QUOTA_JA
    } else if locale.starts_with("ko") {
        QUOTA_KO
    } else {
        QUOTA_EN
    }
}

/// Returns the shared fail-closed notice used when Cloud entitlement cannot
/// be trusted. This is deliberately distinct from a consumed-limit notice.
pub fn quota_unavailable_notice(locale: Option<&str>) -> &'static str {
    let locale = locale.unwrap_or_default().to_ascii_lowercase();
    if locale.starts_with("zh") {
        QUOTA_UNAVAILABLE_ZH
    } else if locale.starts_with("ja") {
        QUOTA_UNAVAILABLE_JA
    } else if locale.starts_with("ko") {
        QUOTA_UNAVAILABLE_KO
    } else {
        QUOTA_UNAVAILABLE_EN
    }
}

/// Reads locale metadata from an adapter-owned raw payload when present.
pub fn locale_from_raw(raw: &Value) -> Option<&str> {
    ["locale", "language", "language_code"]
        .iter()
        .find_map(|key| raw.get(*key).and_then(Value::as_str))
}

/// Uses explicit raw metadata first, then a script fallback for platforms
/// whose historical event envelopes did not carry a locale field.
pub fn quota_exceeded_notice_for_message(message: &InboundMessage) -> &'static str {
    if let Some(locale) = locale_from_raw(&message.raw) {
        return quota_exceeded_notice(Some(locale));
    }
    quota_exceeded_notice(locale_from_text(&message.text))
}

pub fn quota_unavailable_notice_for_message(message: &InboundMessage) -> &'static str {
    if let Some(locale) = locale_from_raw(&message.raw) {
        return quota_unavailable_notice(Some(locale));
    }
    quota_unavailable_notice(locale_from_text(&message.text))
}

/// Lark's native inbound type is intentionally separate from the normalized
/// envelope, so adapters that have only text can use this helper directly.
pub fn quota_exceeded_notice_for_text(text: &str) -> &'static str {
    quota_exceeded_notice(locale_from_text(text))
}

pub fn quota_unavailable_notice_for_text(text: &str) -> &'static str {
    quota_unavailable_notice(locale_from_text(text))
}

fn locale_from_text(text: &str) -> Option<&'static str> {
    if text
        .chars()
        .any(|ch| ('\u{AC00}'..='\u{D7AF}').contains(&ch))
    {
        Some("ko")
    } else if text.chars().any(|ch| {
        ('\u{3040}'..='\u{30FF}').contains(&ch) || ('\u{31F0}'..='\u{31FF}').contains(&ch)
    }) {
        Some("ja")
    } else if text
        .chars()
        .any(|ch| ('\u{3400}'..='\u{9FFF}').contains(&ch))
    {
        Some("zh")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supports_all_product_locales() {
        assert!(quota_exceeded_notice(Some("en")).starts_with("⚠️ This"));
        assert!(quota_exceeded_notice(Some("zh-Hans")).contains("托管"));
        assert!(quota_exceeded_notice(Some("ja")).contains("ワークスペース"));
        assert!(quota_exceeded_notice(Some("ko")).contains("워크스페이스"));
        assert!(quota_unavailable_notice(Some("en")).contains("temporarily unavailable"));
        assert!(quota_unavailable_notice(Some("zh-Hans")).contains("暂时"));
        assert!(quota_unavailable_notice(Some("ja")).contains("一時的"));
        assert!(quota_unavailable_notice(Some("ko")).contains("일시적으로"));
    }

    #[test]
    fn falls_back_to_message_script_when_metadata_is_missing() {
        let message = InboundMessage {
            text: "请继续".into(),
            ..Default::default()
        };
        assert!(quota_exceeded_notice_for_message(&message).contains("托管"));
        assert!(quota_unavailable_notice_for_message(&message).contains("暂时"));
    }
}
