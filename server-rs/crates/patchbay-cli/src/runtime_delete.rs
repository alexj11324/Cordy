use anyhow::Result;
use serde::Deserialize;
use serde_json::Value;
use std::fmt::Write;

use super::{value_string, HttpError, OutputFormat, RunOutput};

#[derive(Debug, Deserialize)]
pub(super) struct RuntimeDeleteConflict {
    code: String,
    #[serde(default)]
    active_agents: Vec<RuntimeDeleteAgent>,
}

#[derive(Debug, Deserialize)]
struct RuntimeDeleteAgent {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
}

impl RuntimeDeleteConflict {
    pub(super) fn ids(&self) -> Vec<&str> {
        self.active_agents
            .iter()
            .map(|agent| agent.id.as_str())
            .filter(|id| !id.is_empty())
            .collect()
    }

    pub(super) fn displays(&self) -> Vec<String> {
        self.active_agents
            .iter()
            .filter_map(|agent| match (agent.name.is_empty(), agent.id.is_empty()) {
                (false, false) => Some(format!("{} ({})", agent.name, agent.id)),
                (false, true) => Some(agent.name.clone()),
                (true, false) => Some(agent.id.clone()),
                (true, true) => None,
            })
            .collect()
    }
}

pub(super) fn runtime_delete_conflict(error: &anyhow::Error) -> Option<RuntimeDeleteConflict> {
    let http = error.downcast_ref::<HttpError>()?;
    if http.status_code != 409 {
        return None;
    }
    let conflict: RuntimeDeleteConflict = serde_json::from_str(&http.body).ok()?;
    (conflict.code == "runtime_has_active_agents" && !conflict.active_agents.is_empty())
        .then_some(conflict)
}

pub(super) fn format_runtime_delete_result(
    result: &Value,
    output: OutputFormat,
) -> Result<RunOutput> {
    if output == OutputFormat::Json {
        return Ok(RunOutput {
            stdout: format!("{}\n", serde_json::to_string_pretty(result)?),
            stderr: String::new(),
        });
    }
    let id = value_string(result, "id");
    let stderr = if result.get("agents_unbound").is_some() {
        let mut message = format!(
            "Runtime {id} deleted; unbound {} agent(s)",
            value_string(result, "agents_unbound")
        );
        if result.get("automations_paused").is_some() {
            let _ = write!(
                message,
                " and paused {} automation(s)",
                value_string(result, "automations_paused")
            );
        }
        message + ".\n"
    } else if result.get("agents_archived").is_some() {
        format!(
            "Runtime {id} deleted; processed {} agent(s).\n",
            value_string(result, "agents_archived")
        )
    } else {
        format!("Runtime {id} deleted.\n")
    };
    Ok(RunOutput {
        stdout: String::new(),
        stderr,
    })
}
