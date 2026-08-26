//! Text normalization helpers — port of `server/internal/util/text.go`.

use serde_json::Value;

/// Decodes the literal two-character `\\n` / `\\r` / `\\t` / `\\\\` sequences
/// emitted by agent CLIs. Other escape sequences and a trailing backslash pass
/// through unchanged.
pub fn unescape_backslash_escapes(s: &str) -> String {
    if !s.contains('\\') {
        return s.to_string();
    }
    let bytes = s.as_bytes();
    let mut output = String::with_capacity(s.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            match bytes[i + 1] {
                b'n' => {
                    output.push('\n');
                    i += 2;
                    continue;
                }
                b'r' => {
                    output.push('\r');
                    i += 2;
                    continue;
                }
                b't' => {
                    output.push('\t');
                    i += 2;
                    continue;
                }
                b'\\' => {
                    output.push('\\');
                    i += 2;
                    continue;
                }
                _ => {}
            }
        }
        let start = i;
        i += 1;
        while i < bytes.len() && (bytes[i] & 0xC0) == 0x80 {
            i += 1;
        }
        output.push_str(&s[start..i]);
    }
    output
}

/// Removes NUL bytes before text reaches PostgreSQL. Rust strings are already
/// valid UTF-8, so the invalid-byte replacement branch of the Go helper is
/// enforced by the UTF-8 boundary when external bytes are decoded.
pub fn sanitize_text_for_postgres(s: &str) -> String {
    s.replace('\0', "")
}

const MAX_JSON_DEPTH: usize = 32;

/// Recursively sanitizes JSON strings and object keys before PostgreSQL
/// persistence. Values deeper than the Go guard's maximum depth become null.
pub fn sanitize_json_for_postgres(value: Value) -> Value {
    sanitize_json_value(value, 0)
}

fn sanitize_json_value(value: Value, depth: usize) -> Value {
    if depth > MAX_JSON_DEPTH {
        return Value::Null;
    }
    match value {
        Value::String(value) => Value::String(sanitize_text_for_postgres(&value)),
        Value::Array(values) => Value::Array(
            values
                .into_iter()
                .map(|value| sanitize_json_value(value, depth + 1))
                .collect(),
        ),
        Value::Object(values) => Value::Object(
            values
                .into_iter()
                .map(|(key, value)| {
                    (
                        sanitize_text_for_postgres(&key),
                        sanitize_json_value(value, depth + 1),
                    )
                })
                .collect(),
        ),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unescapes_only_the_four_supported_sequences() {
        assert_eq!(unescape_backslash_escapes("a\\nb"), "a\nb");
        assert_eq!(unescape_backslash_escapes("a\\rb\\tc"), "a\rb\tc");
        assert_eq!(unescape_backslash_escapes("\\\\n"), "\\n");
        assert_eq!(unescape_backslash_escapes("\\x41"), "\\x41");
        assert_eq!(unescape_backslash_escapes("trail\\"), "trail\\");
    }

    #[test]
    fn removes_nested_nuls_from_json_values_and_keys() {
        let value = serde_json::json!({
            "outer\0key": ["a\0b", {"inner": "c\0d"}],
            "number": 7,
        });
        assert_eq!(
            sanitize_json_for_postgres(value),
            serde_json::json!({
                "outerkey": ["ab", {"inner": "cd"}],
                "number": 7,
            })
        );
    }

    #[test]
    fn bounds_deep_json_recursion() {
        let mut value = Value::String("poisoned\0text".into());
        for _ in 0..=MAX_JSON_DEPTH {
            value = Value::Array(vec![value]);
        }
        assert_eq!(sanitize_json_for_postgres(value), {
            let mut expected = Value::Null;
            for _ in 0..=MAX_JSON_DEPTH {
                expected = Value::Array(vec![expected]);
            }
            expected
        });
    }
}
