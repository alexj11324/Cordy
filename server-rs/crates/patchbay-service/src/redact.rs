//! Detects and masks secrets in agent output before it reaches the database
//! or WebSocket broadcast.
//!
//! Patterns are checked in order; first match wins per position. The nested
//! walk is load-bearing, not defensive tidying: providers record structured
//! tool inputs (Codex records a file edit as changes[]{path, diff, content}),
//! so a top-level-only pass leaves a credential inside a patch body untouched
//! on its way to the database.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

struct SecretPattern {
    re: Regex,
    replacement: &'static str,
}

static PATTERNS: LazyLock<Vec<SecretPattern>> = LazyLock::new(|| {
    vec![
        // AWS access key IDs (always start with AKIA).
        SecretPattern {
            re: Regex::new(r"\bAKIA[0-9A-Z]{16}\b").unwrap(),
            replacement: "[REDACTED AWS KEY]",
        },
        // AWS secret access keys (40 char base64-ish, preceded by a common separator).
        SecretPattern {
            re: Regex::new(
                r"(?i)(?:aws_secret_access_key|secret_?access_?key)\s*[=:]\s*[A-Za-z0-9/+=]{40}",
            )
            .unwrap(),
            replacement: "[REDACTED AWS SECRET]",
        },
        // PEM private keys (multi-line).
        SecretPattern {
            re: Regex::new(
                r"(?s)-----BEGIN[A-Z\s]*PRIVATE KEY-----.*?-----END[A-Z\s]*PRIVATE KEY-----",
            )
            .unwrap(),
            replacement: "[REDACTED PRIVATE KEY]",
        },
        // GitHub tokens (classic PAT, OAuth, user-to-server, server-to-server, refresh).
        SecretPattern {
            re: Regex::new(r"\b(ghp|gho|ghu|ghs|ghr)_[A-Za-z0-9_]{36,255}\b").unwrap(),
            replacement: "[REDACTED GITHUB TOKEN]",
        },
        // GitHub fine-grained personal access tokens — the classic ghp_/gho_/
        // pattern above does not cover the github_pat_ prefix.
        SecretPattern {
            re: Regex::new(r"\bgithub_pat_[A-Za-z0-9_]{20,255}\b").unwrap(),
            replacement: "[REDACTED GITHUB TOKEN]",
        },
        // OpenAI / Anthropic API keys.
        SecretPattern {
            re: Regex::new(r"\bsk-[A-Za-z0-9_-]{20,}\b").unwrap(),
            replacement: "[REDACTED API KEY]",
        },
        // Slack bot/user/legacy tokens; 'e' covers the newer xoxe-
        // config/refresh tokens alongside xoxb/p/o/r/a/s.
        SecretPattern {
            re: Regex::new(r"\bxox[bporase]-[A-Za-z0-9\-]{10,}\b").unwrap(),
            replacement: "[REDACTED SLACK TOKEN]",
        },
        // Slack app-level tokens use the xapp- prefix.
        SecretPattern {
            re: Regex::new(r"\bxapp-[A-Za-z0-9-]{10,}\b").unwrap(),
            replacement: "[REDACTED SLACK TOKEN]",
        },
        // GitLab personal access tokens.
        SecretPattern {
            re: Regex::new(r"\bglpat-[A-Za-z0-9_-]{20,}\b").unwrap(),
            replacement: "[REDACTED GITLAB TOKEN]",
        },
        // Google API keys (AIza + 35). Capture and restore the trailing
        // delimiter so keys ending in a non-word character such as '-' are
        // still redacted.
        SecretPattern {
            re: Regex::new(r"\bAIza[0-9A-Za-z_-]{35}([^0-9A-Za-z_-]|$)").unwrap(),
            replacement: "[REDACTED GOOGLE API KEY]$1",
        },
        // Stripe secret / restricted live keys. Publishable keys (pk_live_)
        // are intentionally excluded — they are not secret.
        SecretPattern {
            re: Regex::new(r"\b(?:sk|rk)_live_[0-9A-Za-z]{16,}\b").unwrap(),
            replacement: "[REDACTED STRIPE KEY]",
        },
        // JWT tokens (three base64url segments).
        SecretPattern {
            re: Regex::new(r"\bey[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\b")
                .unwrap(),
            replacement: "[REDACTED JWT]",
        },
        // Generic "Bearer <token>" in output.
        SecretPattern {
            re: Regex::new(r"(?i)\bBearer\s+[A-Za-z0-9\-._~+/]+=*\b").unwrap(),
            replacement: "Bearer [REDACTED]",
        },
        // Connection strings with embedded passwords.
        SecretPattern {
            re: Regex::new(r"(?i)(?:postgres|mysql|mongodb|redis|amqp)(?:ql)?://[^:\s]+:[^@\s]+@")
                .unwrap(),
            replacement: "[REDACTED CONNECTION STRING]@",
        },
        // Generic key=value patterns for common secret env var names.
        SecretPattern {
            re: Regex::new(
                r"(?i)(?:API_KEY|API_SECRET|SECRET_KEY|SECRET|ACCESS_TOKEN|AUTH_TOKEN|PRIVATE_KEY|DATABASE_URL|DB_PASSWORD|DB_URL|REDIS_URL|PASSWORD|TOKEN)\s*[=:]\s*\S+",
            )
            .unwrap(),
            replacement: "[REDACTED CREDENTIAL]",
        },
    ]
});

/// Bounds the walk in [`redact_value`]. Tool inputs are decoded from
/// daemon-supplied JSON, so nesting depth is attacker-influenced; without a
/// bound a pathologically nested payload would recurse until the stack blows.
const MAX_REDACT_DEPTH: usize = 32;

/// Replaces anything below [`MAX_REDACT_DEPTH`]. Returning the raw value
/// there would hand back an unscrubbed string — exactly what this package
/// exists to prevent — so the fail-safe direction is to drop it.
const DEPTH_LIMIT_PLACEHOLDER: &str = "[REDACTED DEPTH LIMIT]";

fn home_dir() -> Option<String> {
    #[cfg(windows)]
    {
        std::env::var("USERPROFILE").ok()
    }
    #[cfg(not(windows))]
    {
        std::env::var("HOME").ok()
    }
}

fn username() -> Option<String> {
    #[cfg(windows)]
    {
        std::env::var("USERNAME").ok()
    }
    #[cfg(not(windows))]
    {
        std::env::var("USER").ok()
    }
}

static HOME_MASK: LazyLock<Option<(String, String)>> = LazyLock::new(|| {
    let home = home_dir()?;
    let user = username()?;
    if user.is_empty() {
        return None;
    }
    Some((home.clone(), home.replacen(&user, "****", 1)))
});

/// Scans the input string for known secret patterns and replaces matches
/// with safe placeholders. Also masks the local user's home directory path
/// to prevent leaking the username.
pub fn text(s: &str) -> String {
    let mut s = s.to_string();
    for p in PATTERNS.iter() {
        s = p.re.replace_all(&s, p.replacement).into_owned();
    }
    if let Some((home, masked)) = HOME_MASK.as_ref() {
        s = s.replace(home, masked);
    }
    s
}

/// Returns a copy of `m` with every string value passed through [`text`],
/// including strings nested inside maps and arrays.
pub fn input_map(m: &serde_json::Map<String, Value>) -> serde_json::Map<String, Value> {
    redact_map(m, 0)
}

fn redact_map(m: &serde_json::Map<String, Value>, depth: usize) -> serde_json::Map<String, Value> {
    if depth >= MAX_REDACT_DEPTH {
        let mut out = serde_json::Map::new();
        out.insert(
            "_".to_string(),
            Value::String(DEPTH_LIMIT_PLACEHOLDER.to_string()),
        );
        return out;
    }
    let mut out = serde_json::Map::with_capacity(m.len());
    for (k, v) in m {
        out.insert(k.clone(), redact_value(v, depth + 1));
    }
    out
}

/// Scrubs a single decoded JSON value, recursing through composites. Go's
/// variant additionally special-cases []string and map[string]string shapes;
/// under serde_json those are plain Array/Object values covered by the same
/// recursion. Composites are copied rather than scrubbed in place: the caller
/// still holds the original map and keeps using it after redaction.
pub fn redact_value(v: &Value, depth: usize) -> Value {
    if depth >= MAX_REDACT_DEPTH {
        return Value::String(DEPTH_LIMIT_PLACEHOLDER.to_string());
    }
    match v {
        Value::String(s) => Value::String(text(s)),
        Value::Object(m) => Value::Object(redact_map(m, depth)),
        Value::Array(items) => {
            Value::Array(items.iter().map(|e| redact_value(e, depth + 1)).collect())
        }
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn masks_aws_github_openai_keys() {
        assert_eq!(
            text("key AKIAIOSFODNN7EXAMPLE end"),
            "key [REDACTED AWS KEY] end"
        );
        assert_eq!(
            text("ghp_abcdefghijklmnopqrstuvwxyz0123456789ABCD done"),
            "[REDACTED GITHUB TOKEN] done"
        );
        assert_eq!(
            text("sk-proj-abcdefghijklmnopqrstuvwx"),
            "[REDACTED API KEY]"
        );
        assert_eq!(
            text("github_pat_ABCDEFGHIJKLMNOPQRSTUVWXYZ1234567890"),
            "[REDACTED GITHUB TOKEN]"
        );
    }

    #[test]
    fn masks_jwt_bearer_connection_strings_and_env_pairs() {
        assert_eq!(text("x.y.z".to_string().as_str()), "x.y.z");
        let jwt = format!(
            "{}.{}.{}",
            "eyJhbGciOiJIUzI1NiJ9",
            "eyJzdWIiOiIxMjM0NTY3ODkwIn0",
            "SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJVadQssw5c"
        );
        assert_eq!(text(&jwt), "[REDACTED JWT]");
        assert_eq!(
            text("Authorization: Bearer abc.def.ghi"),
            "Authorization: Bearer [REDACTED]"
        );
        assert_eq!(
            text("postgres://admin:s3cret@db.example.com/x"),
            "[REDACTED CONNECTION STRING]@db.example.com/x"
        );
        assert_eq!(text("MY_API_KEY=supersecret"), "MY_[REDACTED CREDENTIAL]");
    }

    #[test]
    fn google_key_restores_trailing_delimiter() {
        let key = format!("AIza{}", "a".repeat(35));
        assert_eq!(text(&format!("{key};")), "[REDACTED GOOGLE API KEY];");
        assert_eq!(text(&key), "[REDACTED GOOGLE API KEY]");
    }

    #[test]
    fn pem_private_key_multiline() {
        let pem =
            "-----BEGIN RSA PRIVATE KEY-----\nMIIEow\nmore\n-----END RSA PRIVATE KEY-----\ntail";
        assert_eq!(text(pem), "[REDACTED PRIVATE KEY]\ntail");
    }

    #[test]
    fn stripe_publishable_not_redacted_but_live_is() {
        assert_eq!(text("pk_live_abcdefghijklmnop"), "pk_live_abcdefghijklmnop");
        assert_eq!(text("sk_live_abcdefghijklmnop"), "[REDACTED STRIPE KEY]");
    }

    #[test]
    fn benign_text_passes_through() {
        assert_eq!(
            text("hello world, nothing here"),
            "hello world, nothing here"
        );
    }

    #[test]
    fn nested_walk_scrubs_deep_values() {
        let m = json!({
            "cmd": ["echo", "AKIAIOSFODNN7EXAMPLE"],
            "edit": {
                "path": "/tmp/x",
                "diff": "MY_TOKEN=abc123",
                "nested": {"deep": {"deeper": "postgres://u:p@h/db"}}
            }
        });
        let out = input_map(m.as_object().unwrap());
        assert_eq!(out["cmd"][1], "[REDACTED AWS KEY]");
        // Boundary-less credential match keeps the "MY_" prefix, same as Go.
        assert_eq!(out["edit"]["diff"], "MY_[REDACTED CREDENTIAL]");
        assert_eq!(
            out["edit"]["nested"]["deep"]["deeper"],
            "[REDACTED CONNECTION STRING]@h/db"
        );
        // Original untouched (copy semantics).
        assert_eq!(m["edit"]["diff"], "MY_TOKEN=abc123");
    }

    #[test]
    fn depth_limit_drops_instead_of_leaking() {
        // Build nesting deeper than MAX_REDACT_DEPTH.
        let mut v = json!("AKIAIOSFODNN7EXAMPLE");
        for _ in 0..(MAX_REDACT_DEPTH + 4) {
            v = json!({ "wrap": v });
        }
        let out = redact_value(&v, 0);
        // Walk down to the cutoff: the node AT depth MAX_REDACT_DEPTH is
        // replaced wholesale by the bare placeholder string.
        let mut cur = &out;
        for _ in 0..MAX_REDACT_DEPTH {
            cur = &cur["wrap"];
        }
        assert_eq!(*cur, json!("[REDACTED DEPTH LIMIT]"));
        let dumped = out.to_string();
        assert!(!dumped.contains("AKIA"));
    }
}
