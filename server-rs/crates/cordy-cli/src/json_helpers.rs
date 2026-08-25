use serde_json::Value;

pub(super) fn value_string(object: &Value, key: &str) -> String {
    match object.get(key) {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(value)) => value.clone(),
        Some(value) => value.to_string(),
    }
}
