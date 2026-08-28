//! Markdown → Telegram HTML conversion.
//!
//! The
//! implementation mirrors the Go regex pipeline exactly: code spans and
//! links are lifted into placeholders BEFORE html-escaping so their
//! contents can be escaped independently, then bold/italic/strike run on
//! the escaped text.

/// Escapes HTML the way Go's `html.EscapeString` does (five characters).
pub fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '\'' => out.push_str("&#39;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&#34;"),
            _ => out.push(c),
        }
    }
    out
}

/// Converts a markdown body to Telegram-parseable HTML. Fenced code blocks
/// become <pre>/<code class="language-…">, headings become <b>, bullets
/// gain a "• " marker, and inline spans map onto the HTML subset.
pub fn format_html(md: &str) -> String {
    let mut out = String::with_capacity(md.len());
    let mut in_code = false;
    let mut code_lang = String::new();
    let mut code_buf: Vec<&str> = Vec::new();
    for line in md.split('\n') {
        let trimmed = line.trim();
        if let Some(lang) = trimmed.strip_prefix("```") {
            if in_code {
                out.push_str(&render_code_block(&code_lang, &code_buf));
                out.push('\n');
                in_code = false;
                code_buf.clear();
                code_lang.clear();
            } else {
                in_code = true;
                code_lang = lang.to_string();
            }
            continue;
        }
        if in_code {
            code_buf.push(line);
            continue;
        }
        out.push_str(&format_line(line));
        out.push('\n');
    }
    if in_code {
        out.push_str(&render_code_block(&code_lang, &code_buf));
        out.push('\n');
    }
    out.trim_end_matches('\n').to_string()
}

fn render_code_block(lang: &str, lines: &[&str]) -> String {
    let body = escape_html(&lines.join("\n"));
    if !lang.is_empty() {
        format!(
            r#"<pre><code class="language-{}">{}</code></pre>"#,
            escape_html(lang),
            body
        )
    } else {
        format!("<pre>{body}</pre>")
    }
}

fn format_line(line: &str) -> String {
    // Heading: ^#{1,6}\s+(.*)$
    let trimmed_start = line.trim_start();
    let leading_len = line.len() - trimmed_start.len();
    let hashes = trimmed_start.bytes().take_while(|b| *b == b'#').count();
    if (1..=6).contains(&hashes) {
        let rest = &trimmed_start[hashes..];
        if let Some(content) = rest.strip_prefix(' ') {
            return format!("<b>{}</b>", format_inline(content));
        }
        // Go's \s+ requires at least one whitespace char; a bare "##tag"
        // is not a heading.
    }
    let _ = leading_len;

    // Bullet: ^(\s*)[-*]\s+ — keep the original indent, replace the
    // marker with •.
    let indent_len = line.len() - line.trim_start().len();
    let after_indent = &line[indent_len..];
    for marker in ["- ", "* "] {
        if let Some(rest) = after_indent.strip_prefix(marker) {
            return format!("{}• {}", &line[..indent_len], format_inline(rest));
        }
    }
    format_inline(line)
}

fn format_inline(s: &str) -> String {
    // 1. Lift inline code spans into placeholders.
    let mut code_spans: Vec<String> = Vec::new();
    let lifted_code = lift_delimited(s, '`', |content| {
        code_spans.push(content.to_string());
        "\x00CODE\x00".to_string()
    });

    // 2. Lift markdown links [label](url).
    let mut links: Vec<(String, String)> = Vec::new();
    let lifted_links = lift_links(&lifted_code, &mut links);

    // 3. Escape, then apply emphasis regexes on the escaped text.
    let mut s = escape_html(&lifted_links);
    s = replace_bold(&s);
    s = replace_italic(&s);
    s = replace_strike(&s);

    // 4. Re-insert links then code (order matters: links were lifted last,
    //    so they are re-inserted first — matching Go).
    for (label, url) in &links {
        let tag = format!(
            r#"<a href="{}">{}</a>"#,
            escape_html(url),
            escape_html(label)
        );
        s = replacen_once(&s, "\x00LINK\x00", &tag);
    }
    for content in &code_spans {
        let tag = format!("<code>{}</code>", escape_html(content));
        s = replacen_once(&s, "\x00CODE\x00", &tag);
    }
    s
}

/// Replaces each `<delim>…<delim>` span with the mapping of its content
/// (non-greedy, no escaping inside — same as Go's `([^`]+)`).
fn lift_delimited<F: FnMut(&str) -> String>(s: &str, delim: char, mut f: F) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    loop {
        let Some(start) = rest.find(delim) else {
            out.push_str(rest);
            return out;
        };
        let after = &rest[start + delim.len_utf8()..];
        let Some(end) = after.find(delim) else {
            out.push_str(rest);
            return out;
        };
        if end == 0 {
            // Empty span (``) does not match [^`]+; emit literally.
            out.push_str(&rest[..start + 1]);
            rest = after;
            continue;
        }
        out.push_str(&rest[..start]);
        out.push_str(&f(&after[..end]));
        rest = &after[end + delim.len_utf8()..];
    }
}

/// Lifts `[label](url)` spans. URL is non-space, up to the first ')'.
fn lift_links(s: &str, links: &mut Vec<(String, String)>) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'[' {
            if let Some(close) = s[i + 1..].find(']') {
                let label_end = i + 1 + close;
                if let Some(rest) = s[label_end + 1..].strip_prefix('(') {
                    if let Some(paren) = rest.find(|c: char| c == ')' || c.is_whitespace()) {
                        if rest.as_bytes()[paren] == b')' && paren > 0 {
                            let label = &s[i + 1..label_end];
                            let url = &rest[..paren];
                            out.push_str("\x00LINK\x00");
                            links.push((label.to_string(), url.to_string()));
                            let consumed = label_end + 2 + url.len() + 1;
                            i = consumed;
                            continue;
                        }
                    }
                }
            }
        }
        // Advance one full UTF-8 scalar.
        let ch_len = s[i..].chars().next().map(char::len_utf8).unwrap_or(1);
        out.push_str(&s[i..i + ch_len]);
        i += ch_len;
    }
    out
}

/// `\*\*(.+?)\*\*` → `<b>$1</b>`.
fn replace_bold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    loop {
        match find_sub(rest, "**") {
            Some((start, after_start)) => match find_sub(after_start, "**") {
                Some((end_rel, after_end)) if end_rel > 0 && end_rel <= after_start.len() => {
                    out.push_str(&rest[..start]);
                    out.push_str("<b>");
                    out.push_str(&after_start[..end_rel]);
                    out.push_str("</b>");
                    rest = after_end;
                }
                _ => {
                    out.push_str(rest);
                    return out;
                }
            },
            None => {
                out.push_str(rest);
                return out;
            }
        }
    }
}

/// `(^|[^*])\*([^*]+?)\*` → `$1<i>$2</i>` — an asterisk pair not preceded
/// by another asterisk (bold already consumed above), non-greedy content.
fn replace_italic(s: &str) -> String {
    let chars: Vec<(usize, char)> = s.char_indices().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        let (byte_pos, c) = chars[i];
        if c == '*' {
            // Not preceded by '*' (bold already handled)…
            let prev_star = i > 0 && chars[i - 1].1 == '*';
            // …find the nearest closer whose following char is not '*'.
            if !prev_star {
                if let Some(close_rel) = chars[i + 1..].iter().position(|(_, cc)| *cc == '*') {
                    let close_idx = close_rel + i + 1;
                    let next_is_star = chars.get(close_idx + 1).is_some_and(|(_, cc)| *cc == '*');
                    if close_idx > i + 1 && !next_is_star {
                        out.push_str(&s[..byte_pos]);
                        out.push_str("<i>");
                        out.push_str(&s[chars[i + 1].0..chars[close_idx].0]);
                        out.push_str("</i>");
                        // Recurse on the tail after the closing star.
                        let tail_byte =
                            chars.get(close_idx + 1).map(|(b, _)| *b).unwrap_or(s.len());
                        out.push_str(&replace_italic(&s[tail_byte..]));
                        return out;
                    }
                }
            }
        }
        i += 1;
    }
    s.to_string()
}

/// `~~(.+?)~~` → `<s>$1</s>`.
fn replace_strike(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    loop {
        match find_sub(rest, "~~") {
            Some((start, after_start)) => match find_sub(after_start, "~~") {
                Some((end_rel, after_end)) if end_rel > 0 && end_rel <= after_start.len() => {
                    out.push_str(&rest[..start]);
                    out.push_str("<s>");
                    out.push_str(&after_start[..end_rel]);
                    out.push_str("</s>");
                    rest = after_end;
                }
                _ => {
                    out.push_str(rest);
                    return out;
                }
            },
            None => {
                out.push_str(rest);
                return out;
            }
        }
    }
}

/// Finds the first `needle` occurrence, returning (start, rest-after-needle).
fn find_sub<'a>(haystack: &'a str, needle: &str) -> Option<(usize, &'a str)> {
    haystack
        .find(needle)
        .map(|start| (start, &haystack[start + needle.len()..]))
}

fn replacen_once(s: &str, from: &str, to: &str) -> String {
    s.replacen(from, to, 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_html_like_go_five_chars() {
        assert_eq!(escape_html("&'<>\""), "&amp;&#39;&lt;&gt;&#34;");
        assert_eq!(escape_html("plain"), "plain");
    }

    #[test]
    fn bold_italic_strike_inline() {
        assert_eq!(format_inline("**hi**"), "<b>hi</b>");
        assert_eq!(format_inline("*hey*"), "<i>hey</i>");
        assert_eq!(format_inline("~~gone~~"), "<s>gone</s>");
        // Escaping happens before emphasis insertion.
        assert_eq!(format_inline("**a<b**"), "<b>a&lt;b</b>");
        // Italic preceded by text keeps the prefix.
        assert_eq!(format_inline("x*y*"), "x<i>y</i>");
        // Bold is not re-matched as italic.
        assert_eq!(format_inline("**bold only**"), "<b>bold only</b>");
    }

    #[test]
    fn inline_code_is_lifted_before_escaping() {
        assert_eq!(format_inline("`<b>&`"), "<code>&lt;b&gt;&amp;</code>");
        // Code content survives emphasis scanning.
        assert_eq!(format_inline("*a* `b*c`"), "<i>a</i> <code>b*c</code>");
    }

    #[test]
    fn links_escape_label_and_url_independently() {
        assert_eq!(
            format_inline("[Go](https://go.dev?a=1&b=2)"),
            r#"<a href="https://go.dev?a=1&amp;b=2">Go</a>"#
        );
        assert_eq!(
            format_inline("[a<b](https://x.test)"),
            r#"<a href="https://x.test">a&lt;b</a>"#
        );
        // A link containing spaces in the URL is not matched (Go's [^)\s]+).
        assert_eq!(format_inline("[a](u v)"), "[a](u v)");
        assert_eq!(
            format_inline("See [docs](https://example.com)"),
            r#"See <a href="https://example.com">docs</a>"#
        );
    }

    #[test]
    fn headings_and_bullets() {
        assert_eq!(format_line("# Title"), "<b>Title</b>");
        assert_eq!(format_line("###### Deep"), "<b>Deep</b>");
        assert_eq!(format_line("####### seven"), "####### seven");
        assert_eq!(format_line("- item"), "• item");
        assert_eq!(format_line("* item"), "• item");
        assert_eq!(format_line("  - indented"), "  • indented");
        assert_eq!(format_line("-no-space"), "-no-space");
    }

    #[test]
    fn fenced_code_blocks_render_pre() {
        assert_eq!(
            format_html("before\n```rust\nlet x = 1;\n```\nafter"),
            "before\n<pre><code class=\"language-rust\">let x = 1;</code></pre>\nafter"
        );
        assert_eq!(format_html("```\nplain\n```"), "<pre>plain</pre>");
        // Unterminated fence still renders (Go flushes at EOF).
        assert_eq!(
            format_html("```js\nvar x;"),
            "<pre><code class=\"language-js\">var x;</code></pre>"
        );
    }

    #[test]
    fn multiline_document_roundtrip() {
        let md = "# Header\n\ntext **bold** and `code`\n\n- one\n- two";
        assert_eq!(
            format_html(md),
            "<b>Header</b>\n\ntext <b>bold</b> and <code>code</code>\n\n• one\n• two"
        );
    }
}
