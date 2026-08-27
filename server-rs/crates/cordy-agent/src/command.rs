//! Runtime command identity, argument ownership, and safe diagnostic rendering.

use std::collections::{BTreeMap, BTreeSet};

pub const REDACTED_ARGUMENT: &str = "<redacted>";
const MAX_LOGGED_FLAG_LEN: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockedArgMode {
    WithValue,
    Standalone,
    OptionalValue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilteredArgs {
    pub args: Vec<String>,
    pub blocked_flags: Vec<String>,
}

/// Executable plus the fixed prefix that identifies a custom runtime wrapper.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct RuntimeCommand {
    pub path: String,
    pub prefix: Vec<String>,
}

impl RuntimeCommand {
    pub fn new(path: impl Into<String>, prefix: Vec<String>) -> Self {
        Self {
            path: path.into(),
            prefix,
        }
    }

    /// Prefix always precedes provider-owned protocol arguments.
    pub fn argv(&self, invocation: &[String]) -> Vec<String> {
        let mut argv = Vec::with_capacity(self.prefix.len() + invocation.len());
        argv.extend(self.prefix.iter().cloned());
        argv.extend(invocation.iter().cloned());
        argv
    }

    /// Includes the prefix because one wrapper binary may expose several
    /// independent model catalogs through different fixed subcommands.
    pub fn cache_key(&self) -> String {
        if self.prefix.is_empty() {
            return self.path.clone();
        }
        format!("{}\0{}", self.path, self.prefix.join("\0"))
    }
}

pub fn filter_custom_args(
    args: &[String],
    blocked: &BTreeMap<&str, BlockedArgMode>,
) -> FilteredArgs {
    filter_args(args, blocked, false)
}

/// Fixed-argument prefixes retain every positional token, even one equal to a
/// provider subcommand. Only flags may compete with the managed protocol.
pub fn filter_launch_prefix(
    args: &[String],
    blocked: &BTreeMap<&str, BlockedArgMode>,
) -> FilteredArgs {
    filter_args(args, blocked, true)
}

fn filter_args(
    args: &[String],
    blocked: &BTreeMap<&str, BlockedArgMode>,
    prefix_mode: bool,
) -> FilteredArgs {
    let mut filtered = Vec::with_capacity(args.len());
    let mut blocked_flags = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let arg = unshell_quote_arg(&args[index]);
        let flag = flag_name(&arg);
        if prefix_mode && flag.is_none() {
            filtered.push(arg);
            index += 1;
            continue;
        }
        let blocked_name = flag.or_else(|| {
            (!prefix_mode && blocked.contains_key(arg.as_str())).then_some(arg.as_str())
        });
        let Some((blocked_name, mode)) =
            blocked_name.and_then(|name| blocked.get(name).copied().map(|mode| (name, mode)))
        else {
            filtered.push(arg);
            index += 1;
            continue;
        };
        blocked_flags.push(blocked_name.to_string());
        let inline = arg.contains('=');
        index += 1;
        match mode {
            BlockedArgMode::WithValue if !inline => index += usize::from(index < args.len()),
            BlockedArgMode::OptionalValue
                if !inline
                    && index < args.len()
                    && !unshell_quote_arg(&args[index]).starts_with('-') =>
            {
                index += 1;
            }
            BlockedArgMode::WithValue
            | BlockedArgMode::Standalone
            | BlockedArgMode::OptionalValue => {}
        }
    }
    FilteredArgs {
        args: filtered,
        blocked_flags,
    }
}

/// Renders final argv without values. `trusted_positionals` are accepted only
/// when both their index and literal value match.
pub fn redact_args(args: &[String], trusted_positionals: &[(usize, &str)]) -> Vec<String> {
    let trusted: BTreeSet<usize> = trusted_positionals
        .iter()
        .filter_map(|(index, expected)| {
            args.get(*index)
                .filter(|value| value.as_str() == *expected)
                .map(|_| *index)
        })
        .collect();
    args.iter()
        .enumerate()
        .map(|(index, arg)| {
            if trusted.contains(&index) {
                return arg.clone();
            }
            safe_flag_name(arg)
                .map(str::to_string)
                .unwrap_or_else(|| REDACTED_ARGUMENT.to_string())
        })
        .collect()
}

pub fn overlapping_flags(prefix: &[String], invocation: &[String]) -> BTreeSet<String> {
    let later: BTreeSet<&str> = invocation
        .iter()
        .filter_map(|argument| flag_name(argument))
        .collect();
    prefix
        .iter()
        .filter_map(|argument| flag_name(argument))
        .filter(|flag| later.contains(flag))
        .map(str::to_string)
        .collect()
}

fn safe_flag_name(argument: &str) -> Option<&str> {
    let flag = argument.split_once('=').map_or(argument, |(flag, _)| flag);
    let bytes = flag.as_bytes();
    if bytes.len() == 2 && bytes[0] == b'-' && bytes[1].is_ascii_alphabetic() {
        return Some(flag);
    }
    if bytes.len() < 3
        || bytes.len() > MAX_LOGGED_FLAG_LEN
        || !flag.starts_with("--")
        || !bytes[2].is_ascii_alphabetic()
    {
        return None;
    }
    bytes[3..]
        .iter()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        .then_some(flag)
}

fn flag_name(argument: &str) -> Option<&str> {
    let argument = argument.trim_matches(|character| character == '\'' || character == '"');
    if !argument.starts_with('-') || matches!(argument, "-" | "--") {
        return None;
    }
    Some(argument.split_once('=').map_or(argument, |(flag, _)| flag))
}

fn unshell_quote_arg(argument: &str) -> String {
    if argument.starts_with('-') {
        if let Some((flag, value)) = argument.split_once('=') {
            return strip_surrounding_quotes(value)
                .map_or_else(|| argument.to_string(), |value| format!("{flag}={value}"));
        }
    }
    strip_surrounding_quotes(argument)
        .unwrap_or(argument)
        .to_string()
}

fn strip_surrounding_quotes(value: &str) -> Option<&str> {
    if value.len() < 2 {
        return None;
    }
    let bytes = value.as_bytes();
    ((bytes[0] == b'\'' && bytes[value.len() - 1] == b'\'')
        || (bytes[0] == b'"' && bytes[value.len() - 1] == b'"'))
        .then(|| &value[1..value.len() - 1])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn prefix_precedes_protocol_arguments_without_aliasing() {
        let prefix = strings(&["start", "q36"]);
        let command = RuntimeCommand::new("ccms", prefix.clone());
        let invocation = strings(&["-p", "--output-format", "stream-json"]);
        assert_eq!(
            command.argv(&invocation),
            strings(&["start", "q36", "-p", "--output-format", "stream-json"])
        );
        assert_eq!(prefix, strings(&["start", "q36"]));
        assert_eq!(
            invocation,
            strings(&["-p", "--output-format", "stream-json"])
        );
    }

    #[test]
    fn launch_prefix_keeps_subcommands_and_drops_blocked_flag_values() {
        let blocked = BTreeMap::from([
            ("acp", BlockedArgMode::Standalone),
            ("--output-format", BlockedArgMode::WithValue),
        ]);
        let result = filter_launch_prefix(
            &strings(&["acp", "tenant", "--output-format", "text", "--model=o3"]),
            &blocked,
        );
        assert_eq!(result.args, strings(&["acp", "tenant", "--model=o3"]));
        assert_eq!(result.blocked_flags, strings(&["--output-format"]));
    }

    #[test]
    fn custom_args_strip_quotes_and_consume_optional_values_safely() {
        let blocked = BTreeMap::from([("--worktree", BlockedArgMode::OptionalValue)]);
        let result = filter_custom_args(
            &strings(&["--worktree", "branch", "'--safe'", "--model='o3'"]),
            &blocked,
        );
        assert_eq!(result.args, strings(&["--safe", "--model=o3"]));
    }

    #[test]
    fn custom_args_strip_blocked_positional_subcommands() {
        let blocked = BTreeMap::from([
            ("acp", BlockedArgMode::Standalone),
            ("serve", BlockedArgMode::Standalone),
        ]);
        let result = filter_custom_args(
            &strings(&["acp", "tenant", "serve", "--model=o3"]),
            &blocked,
        );
        assert_eq!(result.args, strings(&["tenant", "--model=o3"]));
        assert_eq!(result.blocked_flags, strings(&["acp", "serve"]));
    }

    #[test]
    fn redaction_preserves_only_safe_flags_and_proven_subcommands() {
        let args = strings(&[
            "acp",
            "--model=o3",
            "o3",
            "-x",
            "-secret-value",
            "--bad/value",
            "{\"token\":\"secret\"}",
        ]);
        assert_eq!(
            redact_args(&args, &[(0, "acp"), (2, "wrong")]),
            strings(&[
                "acp",
                "--model",
                REDACTED_ARGUMENT,
                "-x",
                REDACTED_ARGUMENT,
                REDACTED_ARGUMENT,
                REDACTED_ARGUMENT,
            ])
        );
    }

    #[test]
    fn cache_key_and_overlap_include_fixed_prefix_identity() {
        let a = RuntimeCommand::new("ccms", strings(&["start", "q36", "--model=base"]));
        let b = RuntimeCommand::new("ccms", strings(&["start", "opus"]));
        assert_ne!(a.cache_key(), b.cache_key());
        assert_eq!(
            overlapping_flags(&a.prefix, &strings(&["--model", "o3"])),
            BTreeSet::from(["--model".to_string()])
        );
    }
}
