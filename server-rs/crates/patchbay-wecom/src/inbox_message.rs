//! Safe WeCom markdown for member inbox notifications.

const MAX_MARKDOWN_CHARS: usize = 4_000;

pub fn build_inbox_markdown(
    item: &serde_json::Value,
    workspace_id: &str,
    slug: &str,
    app_url: &str,
) -> String {
    let kind = item
        .get("type")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    let title = item
        .get("title")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    if kind.is_empty() && title.is_empty() {
        return String::new();
    }
    let title = crate::markdown::break_member_links(title);
    let body = crate::markdown::break_member_links(
        item.get("body")
            .and_then(|value| value.as_str())
            .unwrap_or_default(),
    );
    let link = inbox_link(item, workspace_id, slug, app_url);
    let prefix = format!("**[{}] {title}**", type_label(kind));
    let suffix = if link.is_empty() {
        String::new()
    } else {
        format!("\n[查看详情]({link})")
    };
    let result = if body.is_empty() {
        format!("{prefix}{suffix}")
    } else {
        format!("{prefix}\n{body}{suffix}")
    };
    if result.chars().count() <= MAX_MARKDOWN_CHARS {
        return result;
    }
    let fixed = prefix.chars().count() + suffix.chars().count() + 4;
    if fixed < MAX_MARKDOWN_CHARS {
        let body: String = body.chars().take(MAX_MARKDOWN_CHARS - fixed).collect();
        return format!("{prefix}\n{body}...{suffix}");
    }
    let title_fixed =
        prefix.chars().count().saturating_sub(title.chars().count()) + suffix.chars().count() + 3;
    let title: String = title
        .chars()
        .take(MAX_MARKDOWN_CHARS.saturating_sub(title_fixed))
        .collect();
    format!("**[{}] {title}...**{suffix}", type_label(kind))
}

fn inbox_link(item: &serde_json::Value, workspace_id: &str, slug: &str, app_url: &str) -> String {
    if !app_url.starts_with("https://") {
        return String::new();
    }
    let segment = if slug.is_empty() { workspace_id } else { slug };
    let Ok(mut link) = url::Url::parse(app_url) else {
        return String::new();
    };
    let Ok(mut path) = link.path_segments_mut() else {
        return String::new();
    };
    path.pop_if_empty().push(segment).push("inbox");
    drop(path);
    if let Some(issue_id) = item
        .get("issue_id")
        .and_then(|value| value.as_str())
        .filter(|value| !value.is_empty())
    {
        link.query_pairs_mut().append_pair("issue", issue_id);
    }
    link.into()
}

fn type_label(kind: &str) -> &'static str {
    match kind {
        "issue_assigned" => "任务指派",
        "mentioned" => "提及你",
        "status_changed" => "状态变更",
        "comment_added" | "new_comment" => "新评论",
        "reaction_added" => "表情反应",
        "task_failed" => "任务失败",
        "unassigned" => "取消指派",
        "assignee_changed" => "指派人变更",
        "priority_changed" => "优先级变更",
        "due_date_changed" => "截止日期变更",
        "start_date_changed" => "开始日期变更",
        _ => "新消息",
    }
}
