//! Version command dispatch.
//!
//! Version output remains a tiny explicit boundary in the root command router.

use super::*;

pub(super) fn run_version_command(output: OutputFormat) -> Result<RunOutput> {
    run_version(output)
}
