//! Keeping member-authored text from becoming markdown the bot appears to
//! have written — port of `markdown.go`.
//!
//! Shared by the inbox card and the /issue confirmation, both of which splice
//! someone else's words into a message that goes out under the bot's name.

use patchbay_channel::break_markdown_link_adjacency;

/// What every caller splicing member-authored text into a bot-signed message
/// runs it through. Both breaks below are needed and they close different
/// constructs, so they live behind one entry point: a call site cannot take
/// one and forget the other.
///
/// Both are pure insertions of a single space, so the two orders agree and
/// the pair is idempotent: neither can produce the pattern the other looks
/// for.
pub fn break_member_links(s: &str) -> String {
    break_link_reference_definitions(&break_link_adjacency(s))
}

/// Stops member-authored text from forming a markdown link in a message the
/// bot signs. An issue titled "[click here](http://evil.example)" otherwise
/// arrives as a working link inside a card the recipient has every reason to
/// trust: it is delivered by the bot, and nothing in it marks which parts are
/// quoted from a user.
///
/// It separates rather than escapes. A link is only formed when "]" and "("
/// are adjacent — CommonMark requires the link text to be followed
/// *immediately* by "(". One plain space between them is enough, and it is
/// the only edit made: text that does not contain "](" comes back
/// byte-identical, so the common "[Bug] 登录失败" title is untouched.
///
/// This function must never emit a backslash: on WeCom's renderer a "\[…\]"
/// reads as a display-math delimiter, which would either be visible or pull
/// member text into a math block.
pub fn break_link_adjacency(s: &str) -> String {
    break_markdown_link_adjacency(s)
}

/// What a line's block scaffolding opens: the blockquote nesting the line
/// sits inside. It is the one notion of a container the rule has, and both
/// halves of the rule work from it — the half that decides what may precede
/// the label produces one, the half that scans past the colon replays it.
///
/// Only ">" markers are counted. Indentation and list bullets are scaffolding
/// too, and they need no replay: CommonMark continues a list item with plain
/// indentation, which the destination scan already steps over as whitespace.
/// A blockquote is different — it repeats its marker on every line it holds,
/// so a destination on the next line arrives as "> https://…", and a scan
/// that does not expect the marker reads it as the destination.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ContainerPrefix {
    pub quotes: usize,
}

/// Stops member-authored text from *defining* a link the bot's card then
/// resolves. Pushed through a live tenant, a comment body containing
///
/// ```text
/// [重置密码]: https://evil.example
/// [重置密码]
/// ```
///
/// came back with the first line gone — swallowed as a definition — and the
/// second rendered as a blue underlined link to evil.example. Nothing in it
/// is adjacent, so [`break_link_adjacency`] passes the whole attack through
/// untouched. Killing the definition kills the shortcut "[label]", the
/// collapsed "[label][]" and the full "[text][label]" alike: all three
/// resolve through the same map, and with no definition in the message none
/// of them can.
///
/// # The rule
///
/// A "]" is separated from the ":" that follows it when all three hold:
///
/// 1. Only block scaffolding precedes the opening "[" on its line —
///    indentation, ">" blockquote markers, list bullets. Necessary, not
///    decorative: a definition inside a blockquote or a list item still
///    populates the whole document's reference map and resolves outside it.
/// 2. The brackets form a CommonMark link label: closed by the first
///    unescaped "]", no unescaped "[" inside, at least one non-whitespace
///    character, at most 999. The label may span lines and still resolve, so
///    the scan does too.
/// 3. What follows the colon is plausibly a link *destination* — see
///    [`looks_like_link_destination`].
///
/// Condition 3 is the whole design. Breaking every "]:" would be simpler and
/// is the wrong trade: "[Bug]: 登录失败" is a form real people write, and it
/// comes back from here byte-identical because 登录失败 names no host — as a
/// destination it is relative, so it resolves against whatever base the
/// client itself is on and cannot aim the reader at someone else's server.
/// That is the line the rule draws: a destination that leaves for another
/// host is broken, a destination that cannot is left alone.
///
/// # Where it over-fires, and why that is the safe side
///
/// It models block containers only as a count of ">" markers, so it fires on
/// lines CommonMark would fold into a preceding paragraph, and on a
/// next-line destination whose blockquote nesting is shallower than the
/// definition's. Every one of those costs a single space on a line that
/// already carries a URL. Under-firing costs a working link under the bot's
/// name, so the approximation runs this way round on purpose.
///
/// Like [`break_link_adjacency`] this adds one rune per occurrence, so
/// callers that budget a length cap must call it before measuring; and its
/// output holds no "]:" it would fire on again, so a second pass is a no-op.
pub fn break_link_reference_definitions(s: &str) -> String {
    if !s.contains("]:") {
        return s.to_string();
    }
    let bytes = s.as_bytes();
    let n = bytes.len();
    let mut out = String::new();
    let mut prev = 0usize;
    let mut i = 0usize;
    while i < n {
        if bytes[i] != b'[' {
            i += 1;
            continue;
        }
        let Some(container) = container_prefix_before(bytes, i) else {
            i += 1;
            continue;
        };
        let Some((end, has_content)) = link_label_end(bytes, i) else {
            i += 1;
            continue;
        };
        // An empty label ("[]") is not a definition — CommonMark requires the
        // label to hold at least one non-whitespace character — so "[]: url"
        // is prose and stays untouched. A label holds no unescaped "[" either,
        // so nothing inside it can open another one: resume past it either way.
        if !has_content {
            i = end;
            continue;
        }
        i = end;
        if end + 1 >= n || bytes[end + 1] != b':' {
            i += 1;
            continue;
        }
        if !looks_like_link_destination(&s[end + 2..], container) {
            i += 1;
            continue;
        }
        out.push_str(&s[prev..end + 1]);
        out.push(' ');
        prev = end + 1;
        i += 1;
    }
    if prev == 0 {
        return s.to_string();
    }
    out.push_str(&s[prev..]);
    out
}

/// Reports the containers holding the "[" at `bytes[i]`, and whether
/// everything between the start of its line and i is block scaffolding at
/// all: indentation, ">" markers and list bullets. Anything else — a word, a
/// "**" — means the "[" is inside a paragraph, where no definition can begin.
///
/// It walks back only as far as the first byte that cannot be scaffolding, so
/// a "[" in the middle of prose is settled by reading one byte rather than
/// the whole line, and a body full of them stays linear.
fn container_prefix_before(bytes: &[u8], i: usize) -> Option<ContainerPrefix> {
    let mut start = i;
    while start > 0 && is_block_scaffold_byte(bytes[start - 1]) {
        start -= 1;
    }
    if start > 0 && bytes[start - 1] != b'\n' {
        return None;
    }
    parse_container_prefix(&bytes[start..i])
}

/// Reads p as a whole run of block scaffolding and reports the containers it
/// opens. None when p holds anything that is not scaffolding.
fn parse_container_prefix(p: &[u8]) -> Option<ContainerPrefix> {
    let mut prefix = ContainerPrefix::default();
    let mut p = p;
    while !p.is_empty() {
        match p[0] {
            b' ' | b'\t' => p = &p[1..],
            b'>' => {
                prefix.quotes += 1;
                p = &p[1..];
            }
            b'-' | b'+' | b'*' if p.len() > 1 && (p[1] == b' ' || p[1] == b'\t') => {
                p = &p[2..];
            }
            _ => {
                // An ordered list marker: up to 9 digits, then "." or ")",
                // then a space.
                let mut n = 0usize;
                while n < p.len() && n < 9 && p[n].is_ascii_digit() {
                    n += 1;
                }
                if n == 0
                    || n + 1 >= p.len()
                    || (p[n] != b'.' && p[n] != b')')
                    || (p[n + 1] != b' ' && p[n + 1] != b'\t')
                {
                    return None;
                }
                p = &p[n + 2..];
            }
        }
    }
    Some(prefix)
}

/// Steps over the markers of the containers in prefix at the start of a
/// continuation line, and returns where the line's content begins. It is the
/// replay half of [`container_prefix_before`].
///
/// It consumes at most prefix.quotes markers and never more. Fewer is allowed
/// because a blockquote takes lazy continuation lines: "> [x]:" followed by a
/// bare "https://…" with no marker still defines the reference. More is not a
/// continuation of this definition at all — a line deeper than the block it
/// continues opens a new one — so the scan stops and lets the leftover ">"
/// speak for itself, which is to say it is not a destination.
fn skip_continuation_prefix(bytes: &[u8], mut i: usize, prefix: ContainerPrefix) -> usize {
    i = skip_spaces_tabs(bytes, i);
    for _ in 0..prefix.quotes {
        if i >= bytes.len() || bytes[i] != b'>' {
            return i;
        }
        i = skip_spaces_tabs(bytes, i + 1);
    }
    i
}

/// The character set a scaffolding prefix can draw on — a superset of the
/// markers [`parse_container_prefix`] then parses exactly. It only decides
/// how far back to walk; whether the run really is scaffolding is
/// [`parse_container_prefix`]'s answer.
fn is_block_scaffold_byte(c: u8) -> bool {
    matches!(
        c,
        b' ' | b'\t' | b'>' | b'-' | b'+' | b'*' | b'.' | b')' | b'0'..=b'9'
    )
}

/// Returns the byte offset of the "]" closing the link label that opens at
/// `open`, plus whether what it encloses is a label at all (at least one
/// non-whitespace character). Follows CommonMark: the first unescaped "]"
/// closes it, an unescaped "[" disqualifies it, at most 999 runes, and it may
/// run across line endings.
fn link_label_end(bytes: &[u8], open: usize) -> Option<(usize, bool)> {
    const MAX_LABEL_RUNES: usize = 999;
    let s = std::str::from_utf8(bytes).ok()?;
    let mut runes = 0usize;
    let mut content = false;
    let mut i = open + 1;
    while i < s.len() {
        let Some(r) = s[i..].chars().next() else {
            break;
        };
        let size = r.len_utf8();
        if r == ']' {
            return Some((i, content));
        }
        if r == '[' {
            return None;
        }
        runes += 1;
        if runes > MAX_LABEL_RUNES {
            return None;
        }
        if r == '\\' && i + size < s.len() {
            // The backslash escapes what follows, so "\]" does not close the
            // label and "\[" does not disqualify it.
            let n = s[i + size..].chars().next().map_or(1, char::len_utf8);
            i += size + n;
            runes += 1;
            content = true;
            continue;
        }
        if !matches!(r, ' ' | '\t' | '\n' | '\r') {
            content = true;
        }
        i += size;
    }
    None
}

/// Reports whether rest — everything after a "[label]:" — opens a link
/// destination that could send the reader to another host. This is the test
/// that keeps "[Bug]: 登录失败" whole.
///
/// A destination qualifies when it carries a scheme ("https:", and equally
/// "javascript:" or "data:"), or is scheme-relative ("//host/path"), or uses
/// the escape machinery that spells either of those after decoding.
///
/// prefix is the containers the definition's own line sits inside, and it is
/// a parameter rather than something rediscovered here for the reason given
/// on [`ContainerPrefix`]: when the destination is on the next line it
/// arrives behind the same markers, and a scan that walks past the colon
/// without them reads the marker as the destination and clears a definition
/// that resolves.
fn looks_like_link_destination(rest: &str, prefix: ContainerPrefix) -> bool {
    let bytes = rest.as_bytes();
    let mut i = skip_spaces_tabs(bytes, 0);
    // CommonMark allows up to one line ending between the colon and the
    // destination, so the definition can span two lines. A second line ending
    // ends the block and there is no definition — falling through with the
    // newline still in place takes care of that, since a line ending never
    // begins a destination.
    if i < bytes.len() && bytes[i] == b'\r' {
        i += 1;
    }
    if i < bytes.len() && bytes[i] == b'\n' {
        i = skip_continuation_prefix(bytes, i + 1, prefix);
    }
    if i < bytes.len() && (bytes[i] == b'<' || bytes[i] == b'(') {
        // "<…>" is the spec's bracketed destination form, and it may hold
        // leading spaces that a client strips back off the URL.
        i = skip_spaces_tabs(bytes, i + 1);
    }
    let mut dest = &rest[i..];
    if let Some(j) = dest.find([' ', '\t', '\r', '\n']) {
        dest = &dest[..j];
    }
    if dest.is_empty() {
        return false;
    }
    if dest.starts_with("//") || has_uri_scheme(dest) {
        return true;
    }
    // Backslash escapes and character references are recognised inside a
    // destination, and all of "https\://evil.example", "\/\/evil.example"
    // and "&#x68;ttps://evil.example" resolve to a working off-host URL.
    // Rather than decode them, take any use of that machinery as a
    // destination.
    //
    // The machinery, not the characters. A lone "\" or "&" spells nothing:
    // "R&D", "\d+", "docs\setup" and "foo&bar" are all relative destinations,
    // which this function promises to leave whole, and they carry no escape a
    // parser would act on.
    has_backslash_escape(dest) || has_character_reference(dest)
}

/// Reports whether s uses a CommonMark backslash escape — a "\" before ASCII
/// punctuation, which is the only place the backslash means anything. In
/// "\d+" and "docs\setup" it stays literal text.
fn has_backslash_escape(s: &str) -> bool {
    let bytes = s.as_bytes();
    for w in bytes.windows(2) {
        if w[0] == b'\\' && is_ascii_punct(w[1]) {
            return true;
        }
    }
    false
}

/// CommonMark's ASCII punctuation set — every ASCII character that is neither
/// a letter, a digit, nor a space.
fn is_ascii_punct(c: u8) -> bool {
    c.is_ascii_punctuation()
}

/// Reports whether s holds a character reference — "&", a name or a "#"-led
/// numeric body, then ";". "R&D" and "foo&bar" have no ";" to close one, so
/// nothing in them decodes.
///
/// It does not check the name against HTML5's table: an unknown name decodes
/// to nothing and breaking it costs one space, which is the side of this call
/// the rest of the file already runs on.
fn has_character_reference(s: &str) -> bool {
    let bytes = s.as_bytes();
    for i in 0..bytes.len() {
        if bytes[i] != b'&' {
            continue;
        }
        for (j, &c) in bytes.iter().enumerate().skip(i + 1) {
            if c == b';' {
                return j > i + 1;
            }
            if !(c.is_ascii_alphanumeric() || c == b'#') {
                break;
            }
        }
    }
    false
}

/// Reports whether s begins with a URI scheme followed by ":" — a letter,
/// then letters, digits, "+", "-" or "." (RFC 3986).
fn has_uri_scheme(s: &str) -> bool {
    let bytes = s.as_bytes();
    for (i, &c) in bytes.iter().enumerate() {
        match c {
            b'a'..=b'z' | b'A'..=b'Z' => {}
            b'0'..=b'9' | b'+' | b'-' | b'.' if i > 0 => {}
            b':' => return i > 0,
            _ => return false,
        }
    }
    false
}

fn skip_spaces_tabs(bytes: &[u8], mut i: usize) -> usize {
    while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
        i += 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adjacency_break_separates_inline_links() {
        assert_eq!(
            break_member_links("[click here](http://evil.example)"),
            "[click here] (http://evil.example)"
        );
        // Image syntax needs the same adjacency and is covered by the same
        // rule: only "](" is separated, so the bang stays attached.
        assert_eq!(
            break_member_links("![x](https://evil.example/payload.png)"),
            "![x] (https://evil.example/payload.png)"
        );
        // The common title shape is untouched.
        assert_eq!(break_member_links("[Bug] 登录失败"), "[Bug] 登录失败");
    }

    #[test]
    fn reference_definition_attack_is_broken() {
        let attack = "[重置密码]: https://evil.example\n[重置密码]";
        let got = break_member_links(attack);
        assert!(got.starts_with("[重置密码] :"), "{got}");
        // The shortcut reference below can no longer resolve: no definition.
        assert!(!got.contains("]: https"));
    }

    #[test]
    fn human_colon_form_is_left_whole() {
        assert_eq!(break_member_links("[Bug]: 登录失败"), "[Bug]: 登录失败");
        assert_eq!(
            break_member_links("[文件]: report.pdf"),
            "[文件]: report.pdf"
        );
        assert_eq!(break_member_links("[页面]: /inbox"), "[页面]: /inbox");
        assert_eq!(break_member_links("[Regex]: \\d+"), "[Regex]: \\d+");
        assert_eq!(break_member_links("[Owner]: R&D"), "[Owner]: R&D");
    }

    #[test]
    fn definition_inside_blockquote_is_broken() {
        let got = break_member_links("> [x]: https://evil.example");
        assert!(got.contains("> [x] : https"), "{got}");
    }

    #[test]
    fn next_line_destination_behind_matching_markers_is_broken() {
        let attack = "> [x]:\n> https://evil.example";
        let got = break_member_links(attack);
        assert!(got.contains("[x] :"), "{got}");
    }

    #[test]
    fn deeper_continuation_marker_is_not_a_destination() {
        // A line deeper than the block it continues opens a new block; the
        // leftover ">" is not a destination, so nothing breaks.
        let body = "> [x]:\n>> https://evil.example";
        assert_eq!(break_member_links(body), body);
    }

    #[test]
    fn escape_machinery_destinations_are_broken() {
        for attack in [
            "[x]: https\\://evil.example",
            "[x]: \\/\\/evil.example",
            "[x]: &#x68;ttps://evil.example",
        ] {
            let got = break_member_links(attack);
            assert!(got.contains("] :"), "{attack} → {got}");
        }
    }

    #[test]
    fn prose_brackets_are_never_touched() {
        let body = "see **[note]** below\nand [a](b) inline";
        let got = break_member_links(body);
        // The "**" before "[" disqualifies a definition; the inline link is
        // handled by the adjacency pass instead.
        assert!(got.contains("**[note]**"));
        assert!(got.contains("[a] (b)"));
    }

    #[test]
    fn ordered_and_bullet_list_items_still_count_as_scaffolding() {
        let got = break_member_links("- [x]: https://evil.example");
        assert!(got.contains("- [x] : https"), "{got}");
        let got = break_member_links("1. [x]: https://evil.example");
        assert!(got.contains("1. [x] : https"), "{got}");
    }

    #[test]
    fn second_pass_is_a_no_op() {
        let once = break_member_links("[x]: https://evil.example");
        let twice = break_member_links(&once);
        assert_eq!(once, twice);
    }

    #[test]
    fn labels_spanning_lines_still_resolve_to_a_break() {
        let attack = "[multi\nline]: https://evil.example";
        let got = break_member_links(attack);
        assert!(got.contains("line] :"), "{got}");
    }

    #[test]
    fn empty_labels_do_not_fire() {
        // "[]" holds no non-whitespace character, so it is not a label.
        let body = "[]: https://evil.example";
        assert_eq!(break_member_links(body), body);
    }

    #[test]
    fn uri_scheme_predicate_matches_rfc3986() {
        assert!(has_uri_scheme("https://x"));
        assert!(has_uri_scheme("javascript:alert(1)"));
        assert!(has_uri_scheme("data:text/html,x"));
        assert!(!has_uri_scheme("登录失败"));
        assert!(!has_uri_scheme("/inbox"));
        assert!(!has_uri_scheme(":nocolon"));
        assert!(!has_uri_scheme("1http://x")); // scheme must start with a letter
    }

    #[test]
    fn character_reference_predicate_requires_a_semicolon() {
        assert!(has_character_reference("&#x68;ttps"));
        assert!(has_character_reference("&amp;more"));
        assert!(!has_character_reference("R&D"));
        assert!(!has_character_reference("foo&bar"));
        assert!(!has_character_reference("&;")); // empty body
    }

    #[test]
    fn backslash_escape_predicate_is_punctuation_only() {
        assert!(has_backslash_escape("https\\://x"));
        assert!(has_backslash_escape("\\/\\/x"));
        assert!(!has_backslash_escape("\\d+"));
        assert!(!has_backslash_escape("docs\\setup"));
        assert!(!has_backslash_escape("plain"));
    }
}
