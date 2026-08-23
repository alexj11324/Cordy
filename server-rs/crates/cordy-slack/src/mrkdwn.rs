//! The formatMrkdwn function below is a Rust port of the Markdown-to-mrkdwn
//! converter (format_message) from Nous Research's Hermes Agent, used under the
//! MIT License. Source:
//! https://github.com/NousResearch/hermes-agent/blob/main/plugins/platforms/slack/adapter.py
//!
//! Copyright (c) 2025 Nous Research
//!
//! Permission is hereby granted, free of charge, to any person obtaining a copy
//! of this software and associated documentation files (the "Software"), to deal
//! in the Software without restriction, including without limitation the rights
//! to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
//! copies of the Software, and to permit persons to whom the Software is
//! furnished to do so, subject to the following conditions:
//!
//! The above copyright notice and this permission notice shall be included in
//! all copies or substantial portions of the Software.
//!
//! THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
//! IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
//! FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
//! AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
//! LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
//! OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
//! SOFTWARE.

//! Slack renders its own "mrkdwn" dialect, not standard Markdown: bold is *one*
//! star (not two), italic is _underscore_, links are <url|label>, headers and
//! ~~strike~~ are not supported. The agent emits standard Markdown, so an
//! unconverted reply shows literal `**`, `##`, and `[text](url)` in Slack. This
//! converter is a faithful port of Hermes Agent's slack `format_message`
//! (MIT; see the license notice at the top of this file): protected regions
//! (code, converted links, existing Slack entities) are stashed behind NUL-delimited
//! placeholders so later passes never mangle them, then restored last in reverse
//! order so nested placeholders resolve.

use std::sync::OnceLock;

struct Patterns {
    fenced: regex::Regex,
    inline_code: regex::Regex,
    md_link: regex::Regex,
    slack_entity: regex::Regex,
    blockquote: regex::Regex,
    header: regex::Regex,
    inner_bold: regex::Regex,
    bold_italic: regex::Regex,
    bold: regex::Regex,
    italic: regex::Regex,
    strike: regex::Regex,
}

fn patterns() -> &'static Patterns {
    static P: OnceLock<Patterns> = OnceLock::new();
    P.get_or_init(|| Patterns {
        // Go: (?s)(```(?:[^\n]*\n)?.*?```) — dot-matches-newline, lazy body.
        fenced: regex::Regex::new(r"(?s)(```(?:[^\n]*\n)?.*?```)").unwrap(),
        inline_code: regex::Regex::new(r"(`[^`]+`)").unwrap(),
        md_link: regex::Regex::new(r"(!?)\[([^\]]+)\]\(([^()]*(?:\([^()]*\)[^()]*)*)\)").unwrap(),
        slack_entity: regex::Regex::new(r"(<(?:[@#!]|(?:https?|mailto|tel):)[^>\n]+>)").unwrap(),
        blockquote: regex::Regex::new(r"(?m)^(>+\s)").unwrap(),
        header: regex::Regex::new(r"(?m)^#{1,6}\s+(.+)$").unwrap(),
        inner_bold: regex::Regex::new(r"\*\*(.+?)\*\*").unwrap(),
        bold_italic: regex::Regex::new(r"\*\*\*(.+?)\*\*\*").unwrap(),
        bold: regex::Regex::new(r"\*\*(.+?)\*\*").unwrap(),
        italic: regex::Regex::new(r"\*(\S(?:[^*\n]*?\S)?)\*").unwrap(),
        strike: regex::Regex::new(r"~~(.+?)~~").unwrap(),
    })
}

/// Converts standard Markdown to Slack mrkdwn.
pub fn format_mrkdwn(content: &str) -> String {
    if content.is_empty() {
        return content.to_string();
    }
    let mut p = Placeholders::default();
    let pats = patterns();
    let mut text = content.to_string();

    // 1) Protect fenced code blocks, then 2) inline code.
    text = pats
        .fenced
        .replace_all(&text, |caps: &regex::Captures| p.stash(&caps[0]))
        .into_owned();
    text = pats
        .inline_code
        .replace_all(&text, |caps: &regex::Captures| p.stash(&caps[0]))
        .into_owned();

    // 3) Markdown links [text](url) -> <url|text>; image links (![..]) are left
    //    untouched (Slack does not render inline images from markdown).
    text = pats
        .md_link
        .replace_all(&text, |caps: &regex::Captures| {
            if caps[1] == *"!" {
                return caps[0].to_string();
            }
            let mut url = caps[3].trim().to_string();
            if url.starts_with('<') && url.ends_with('>') {
                url = url[1..url.len() - 1].trim().to_string();
            }
            p.stash(&format!("<{}|{}>", url, &caps[2]))
        })
        .into_owned();

    // 4) Protect existing Slack entities / manual links, 5) blockquote markers.
    text = pats
        .slack_entity
        .replace_all(&text, |caps: &regex::Captures| p.stash(&caps[0]))
        .into_owned();
    text = pats
        .blockquote
        .replace_all(&text, |caps: &regex::Captures| p.stash(&caps[0]))
        .into_owned();

    // 6) Escape Slack control chars (unescape first so input isn't double-escaped).
    let text = text
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">");
    let text = text
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");

    // 7) Headers (## Title) -> *Title* (strip redundant bold inside).
    let text = pats
        .header
        .replace_all(&text, |caps: &regex::Captures| {
            let inner = caps[1].trim();
            let inner = pats.inner_bold.replace_all(inner, "$1");
            p.stash(&format!("*{inner}*"))
        })
        .into_owned();

    // 8) ***bold italic*** -> *_text_*, 9) **bold** -> *bold*,
    // 10) *italic* -> _italic_, 11) ~~strike~~ -> ~strike~.
    let text = pats
        .bold_italic
        .replace_all(&text, |caps: &regex::Captures| {
            p.stash(&format!("*_{}_*", &caps[1]))
        })
        .into_owned();
    let text = pats
        .bold
        .replace_all(&text, |caps: &regex::Captures| {
            p.stash(&format!("*{}*", &caps[1]))
        })
        .into_owned();
    let text = pats
        .italic
        .replace_all(&text, |caps: &regex::Captures| {
            p.stash(&format!("_{}_", &caps[1]))
        })
        .into_owned();
    let text = pats
        .strike
        .replace_all(&text, |caps: &regex::Captures| {
            p.stash(&format!("~{}~", &caps[1]))
        })
        .into_owned();

    // 13) Restore placeholders in reverse insertion order (nested ones resolve).
    let mut text = text;
    for key in p.order.iter().rev() {
        let value = p.values[key].clone();
        text = text.replace(key.as_str(), &value);
    }
    text
}

#[derive(Default)]
struct Placeholders {
    values: std::collections::HashMap<String, String>,
    order: Vec<String>,
    n: usize,
}

impl Placeholders {
    fn stash(&mut self, v: &str) -> String {
        let key = format!("\x00SL{}\x00", self.n);
        self.n += 1;
        self.values.insert(key.clone(), v.to_string());
        self.order.push(key.clone());
        key
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_bold_headers_and_links() {
        assert_eq!(format_mrkdwn("**bold**"), "*bold*");
        assert_eq!(format_mrkdwn("## Title"), "*Title*");
        assert_eq!(
            format_mrkdwn("[click](https://example.com)"),
            "<https://example.com|click>"
        );
        assert_eq!(format_mrkdwn("*italic*"), "_italic_");
        assert_eq!(format_mrkdwn("~~gone~~"), "~gone~");
        assert_eq!(format_mrkdwn("***both***"), "*_both_*");
    }

    #[test]
    fn leaves_code_blocks_and_inline_code_alone() {
        assert_eq!(format_mrkdwn("`a **b** c`"), "`a **b** c`");
        let fenced = "```\n**not bold**\n```";
        assert_eq!(format_mrkdwn(fenced), fenced);
    }

    #[test]
    fn image_links_pass_through_unconverted() {
        let img = "![alt](https://example.com/x.png)";
        assert_eq!(format_mrkdwn(img), img);
    }

    #[test]
    fn preserves_existing_slack_entities() {
        assert_eq!(format_mrkdwn("<@U123> hi"), "<@U123> hi");
        assert_eq!(
            format_mrkdwn("<https://x.com|label>"),
            "<https://x.com|label>"
        );
    }

    #[test]
    fn escapes_bare_angle_chars_and_ampersands() {
        assert_eq!(format_mrkdwn("a < b & c > d"), "a &lt; b &amp; c &gt; d");
        // But an entity that was already escaped round-trips to escaped form.
        assert_eq!(format_mrkdwn("&amp;"), "&amp;");
    }

    #[test]
    fn empty_input_stays_empty() {
        assert_eq!(format_mrkdwn(""), "");
    }
}
