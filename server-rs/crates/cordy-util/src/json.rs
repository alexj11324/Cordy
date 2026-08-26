//! JSONB shape normalization — port of `server/internal/util/json.go`.

use serde_json::{Map, Value};

/// Preserves JSON objects and normalizes empty, null, and non-object values to
/// an empty object. This keeps API fields backed by JSONB indexable without a
/// client-side null/type guard, matching Go's `JSONObjectOrEmpty` contract.
pub fn object_or_empty(value: &Value) -> Value {
    match value {
        Value::Object(_) => value.clone(),
        _ => Value::Object(Map::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_objects() {
        let value = serde_json::json!({"priority": "high", "nested": {"ok": true}});
        assert_eq!(object_or_empty(&value), value);
    }

    #[test]
    fn normalizes_non_objects_to_empty_objects() {
        for value in [
            Value::Null,
            Value::Bool(true),
            Value::Number(serde_json::Number::from(7)),
            Value::String("text".into()),
            Value::Array(vec![Value::Null]),
        ] {
            assert_eq!(object_or_empty(&value), serde_json::json!({}));
        }
    }
}
