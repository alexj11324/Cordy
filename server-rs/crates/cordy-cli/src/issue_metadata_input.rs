//! Input parsing for issue metadata commands.
//!
//! HTTP read and mutation workflows live in their focused command modules;
//! this helper owns only the Go-compatible scalar type inference and errors.

use anyhow::{bail, Result};
use serde_json::Value;

pub(super) fn parse_metadata_value(raw: &str, forced_type: Option<&str>) -> Result<Value> {
    match forced_type.unwrap_or_default() {
        "string" => Ok(Value::String(raw.into())),
        "number" => match serde_json::from_str::<Value>(raw) {
            Ok(value @ Value::Number(_)) => Ok(value),
            _ => bail!("value {raw:?} is not a valid number"),
        },
        "bool" => match raw {
            "true" => Ok(Value::Bool(true)),
            "false" => Ok(Value::Bool(false)),
            _ => bail!("value {raw:?} is not a valid bool (expected true or false)"),
        },
        "" => match serde_json::from_str::<Value>(raw) {
            Ok(value @ (Value::String(_) | Value::Bool(_) | Value::Number(_))) => Ok(value),
            _ => Ok(Value::String(raw.into())),
        },
        value_type => {
            bail!("unknown --type {value_type:?} (expected string, number, or bool)")
        }
    }
}
