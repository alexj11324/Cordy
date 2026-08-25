use clap::Args;

use super::*;

#[derive(Debug, Args)]
pub(super) struct IssueTimelineArgs {
    #[arg(value_name = "ID")]
    pub(super) issue_id: String,
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    pub(super) output: OutputFormat,
    #[arg(long, help = "Drop comments and return activity records only")]
    pub(super) activity_only: bool,
    #[arg(
        long,
        value_delimiter = ',',
        help = "Only return activities with these actions (repeatable or comma-separated)"
    )]
    pub(super) action: Vec<String>,
    #[arg(
        long,
        help = "Only return entries created after this RFC3339 timestamp"
    )]
    pub(super) since: Option<String>,
    #[arg(
        long,
        default_value_t = 0,
        allow_hyphen_values = true,
        help = "Only return the N most recent entries"
    )]
    pub(super) tail: i64,
    #[arg(long, help = "Show full UUIDs in table output")]
    pub(super) full_id: bool,
}
