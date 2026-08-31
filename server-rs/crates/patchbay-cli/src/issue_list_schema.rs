use clap::Args;

use super::*;

#[derive(Debug, Args)]
pub(super) struct IssueListArgs {
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    pub(super) output: OutputFormat,
    #[arg(long, help = "Show full UUIDs in table output")]
    pub(super) full_id: bool,
    #[arg(long, help = "Filter by status")]
    pub(super) status: Option<String>,
    #[arg(long, help = "Filter by priority")]
    pub(super) priority: Option<String>,
    #[arg(
        long,
        help = "Filter by execution target name (agent or team; fuzzy match)"
    )]
    pub(super) executor: Option<String>,
    #[arg(
        long,
        help = "Filter by execution target UUID — agent or team (mutually exclusive with --executor)"
    )]
    pub(super) executor_id: Option<String>,
    #[arg(long, help = "Filter by human owner name (workspace member; fuzzy match)")]
    pub(super) owner: Option<String>,
    #[arg(long, help = "Filter by human owner UUID (mutually exclusive with --owner)")]
    pub(super) owner_id: Option<String>,
    #[arg(long, help = "Filter by project ID")]
    pub(super) project: Option<String>,
    #[arg(
        long,
        value_delimiter = ',',
        help = "Filter by metadata key=value (repeatable; combined with AND). Value is JSON-parsed: 'true'/'false' → bool, numbers → number, otherwise string. Wrap as '\"42\"' to force a string when the value would otherwise sniff as a number."
    )]
    pub(super) metadata: Vec<String>,
    #[arg(
        long,
        default_value_t = 50,
        help = "Maximum number of issues to return"
    )]
    pub(super) limit: i64,
    #[arg(
        long,
        default_value_t = 0,
        help = "Number of issues to skip (for pagination)"
    )]
    pub(super) offset: i64,
    #[arg(
        long,
        help = "Sort column: position (default, manual board order), title, created_at, start_date, due_date, priority"
    )]
    pub(super) sort: Option<String>,
    #[arg(
        long,
        help = "Sort direction (asc or desc); requires --sort to be a non-position column (position is always ascending)"
    )]
    pub(super) direction: Option<String>,
}
