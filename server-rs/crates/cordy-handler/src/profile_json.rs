//! Runtime-profile wire shape — port of Go `runtimeProfileToResponse`
//! (server/internal/handler/runtime_profile.go:48).

use serde_json::{json, Value};

use cordy_db::models::RuntimeProfile;

pub fn profile_to_map(p: &RuntimeProfile) -> Value {
    let args: Vec<Value> = p.fixed_args.as_array().cloned().unwrap_or_default();
    json!({
        "id": p.id.to_string(),
        "workspace_id": p.workspace_id.to_string(),
        "display_name": p.display_name,
        "protocol_family": p.protocol_family,
        "command_name": p.command_name,
        "description": p.description.clone(),
        "fixed_args": args,
        "visibility": p.visibility,
        "created_by": p.created_by.map(|u| u.to_string()),
        "enabled": p.enabled,
        "created_at": crate::timefmt::rfc3339(p.created_at),
        "updated_at": crate::timefmt::rfc3339(p.updated_at),
    })
}
