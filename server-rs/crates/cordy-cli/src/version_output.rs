use anyhow::Result;

use super::{
    RunOutput, VersionOutput, BUILD_ARCH, BUILD_COMMIT, BUILD_DATE, BUILD_GO_VERSION, BUILD_OS,
    CLIENT_VERSION,
};

pub(super) fn run_version(output: VersionOutput) -> Result<RunOutput> {
    let stdout = match output {
        VersionOutput::Text => format!(
            "cordy {CLIENT_VERSION} (commit: {BUILD_COMMIT}, built: {BUILD_DATE})\ngo: {BUILD_GO_VERSION}, os/arch: {BUILD_OS}/{BUILD_ARCH}\n"
        ),
        VersionOutput::Json => format!(
            "{}\n",
            serde_json::to_string_pretty(&serde_json::json!({
                "version": CLIENT_VERSION,
                "commit": BUILD_COMMIT,
                "date": BUILD_DATE,
                "go": BUILD_GO_VERSION,
                "os": BUILD_OS,
                "arch": BUILD_ARCH
            }))?
        ),
    };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}
