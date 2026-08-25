//! Markdown destination parsing and runtime-local target classification.
//!
//! The issue safety guard owns the user-facing error and execution-context
//! policy; this module owns only the parser and filesystem/URL classification.

use std::collections::HashSet;
use std::fs;
use std::path::Path;
use url::Url;

use super::lexical_normalize;

pub(super) fn find_runtime_local_markdown_links(
    markdown: &str,
    current_dir: &Path,
) -> Vec<(String, &'static str)> {
    let mut candidates = Vec::new();
    let mut in_fence: Option<char> = None;
    for line in markdown.lines() {
        let trimmed = line.trim_start();
        let fence = trimmed
            .chars()
            .next()
            .filter(|character| matches!(character, '`' | '~'))
            .filter(|character| {
                trimmed
                    .chars()
                    .take_while(|value| value == character)
                    .count()
                    >= 3
            });
        if let Some(character) = fence {
            match in_fence {
                Some(open) if open == character => in_fence = None,
                None => in_fence = Some(character),
                _ => {}
            }
            continue;
        }
        if in_fence.is_some() || line.starts_with("    ") || line.starts_with('\t') {
            continue;
        }
        collect_inline_markdown_destinations(line, &mut candidates);
        if let Some((_, destination)) = trimmed
            .strip_prefix('[')
            .and_then(|rest| rest.split_once("]:"))
        {
            if let Some(destination) = markdown_destination(destination.trim_start()) {
                candidates.push(destination);
            }
        }
    }
    let mut seen = HashSet::new();
    let mut findings = Vec::new();
    for candidate in candidates {
        let target = candidate.trim().to_string();
        if target.is_empty() || !seen.insert(target.clone()) {
            continue;
        }
        if let Some(reason) = classify_runtime_local_target(&target, current_dir) {
            findings.push((target, reason));
        }
    }
    findings
}

fn collect_inline_markdown_destinations(line: &str, destinations: &mut Vec<String>) {
    let bytes = line.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'`' {
            let run = bytes[index..]
                .iter()
                .take_while(|byte| **byte == b'`')
                .count();
            index += run;
            while index < bytes.len() {
                let closing_run = bytes[index..]
                    .iter()
                    .take_while(|byte| **byte == b'`')
                    .count();
                if closing_run == run {
                    index += run;
                    break;
                }
                index += closing_run.max(1);
            }
            continue;
        }
        if bytes[index] == b'<' {
            if let Some(end) = line[index + 1..].find('>') {
                let target = &line[index + 1..index + 1 + end];
                if target.to_ascii_lowercase().starts_with("file://") {
                    destinations.push(target.into());
                }
                index += end + 2;
                continue;
            }
        }
        if bytes[index] == b']'
            && bytes.get(index + 1) == Some(&b'(')
            && !is_markdown_escaped(bytes, index)
        {
            let start = index + 2;
            if let Some(target) = markdown_destination(&line[start..]) {
                destinations.push(target);
            }
        }
        index += 1;
    }
}

fn is_markdown_escaped(bytes: &[u8], index: usize) -> bool {
    bytes[..index]
        .iter()
        .rev()
        .take_while(|byte| **byte == b'\\')
        .count()
        % 2
        == 1
}

fn markdown_destination(input: &str) -> Option<String> {
    let input = input.trim_start();
    if let Some(input) = input.strip_prefix('<') {
        return input.find('>').map(|end| input[..end].into());
    }
    let mut output = String::new();
    let mut depth = 0_usize;
    let mut escaped = false;
    for character in input.chars() {
        if escaped {
            output.push(character);
            escaped = false;
            continue;
        }
        match character {
            '\\' => escaped = true,
            '(' => {
                depth += 1;
                output.push(character);
            }
            ')' if depth == 0 => break,
            ')' => {
                depth -= 1;
                output.push(character);
            }
            character if character.is_whitespace() && depth == 0 => break,
            _ => output.push(character),
        }
    }
    (!output.is_empty()).then_some(output)
}

fn classify_runtime_local_target(target: &str, current_dir: &Path) -> Option<&'static str> {
    let target = target.trim();
    let path = Path::new(target);
    if path.is_absolute() {
        let base = fs::canonicalize(current_dir).unwrap_or_else(|_| lexical_normalize(current_dir));
        let resolved = fs::canonicalize(path).unwrap_or_else(|_| lexical_normalize(path));
        if resolved.starts_with(base) {
            return Some("it is inside this task's working directory");
        }
        if fs::metadata(path).is_ok_and(|metadata| metadata.is_file()) {
            return Some("it names a file that exists only on this machine");
        }
        return None;
    }
    Url::parse(target)
        .ok()
        .filter(|url| url.scheme().eq_ignore_ascii_case("file"))
        .map(|_| "it is a file:// URL")
}
