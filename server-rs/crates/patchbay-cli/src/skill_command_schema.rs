use clap::{Args, Subcommand};
use std::path::PathBuf;

use super::OutputFormat;

#[derive(Debug, Args)]
pub(super) struct SkillArgs {
    #[command(subcommand)]
    pub(super) command: SkillCommand,
}

#[derive(Debug, Subcommand)]
pub(super) enum SkillCommand {
    #[command(about = "List skills in the workspace")]
    List {
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        output: OutputFormat,
    },
    #[command(about = "Get skill details")]
    Get(SkillGetArgs),
    #[command(about = "Create a new skill")]
    Create(SkillCreateArgs),
    #[command(about = "Update a skill")]
    Update(SkillUpdateArgs),
    #[command(about = "Delete a skill")]
    Delete(SkillDeleteArgs),
    #[command(about = "Import a skill from a URL or local archive")]
    Import(SkillImportArgs),
    #[command(about = "Re-download a skill from its imported source")]
    Refresh(SkillRefreshArgs),
    #[command(about = "Search for installable skills")]
    Search(SkillSearchArgs),
    #[command(about = "Work with skill files")]
    Files(SkillFilesArgs),
}

#[derive(Debug, Args)]
pub(super) struct SkillGetArgs {
    #[arg(value_name = "SKILL-ID")]
    pub(super) skill_id: String,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    pub(super) output: OutputFormat,
}

#[derive(Debug, Args)]
pub(super) struct SkillCreateArgs {
    #[arg(long, help = "Skill name (required)")]
    pub(super) name: Option<String>,
    #[arg(long, default_value = "", help = "Skill description")]
    pub(super) description: String,
    #[arg(long, help = "Skill content (SKILL.md body)")]
    pub(super) content: Option<String>,
    #[arg(long, help = "Read skill content from stdin")]
    pub(super) content_stdin: bool,
    #[arg(
        long,
        value_name = "PATH",
        help = "Read skill content from a UTF-8 file"
    )]
    pub(super) content_file: Option<PathBuf>,
    #[arg(long, help = "Skill config as JSON")]
    pub(super) config: Option<String>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    pub(super) output: OutputFormat,
}

#[derive(Debug, Args)]
pub(super) struct SkillUpdateArgs {
    #[arg(value_name = "SKILL-ID")]
    pub(super) skill_id: String,
    #[arg(long, help = "New skill name")]
    pub(super) name: Option<String>,
    #[arg(long, help = "New skill description")]
    pub(super) description: Option<String>,
    #[arg(long, help = "New skill content (SKILL.md body)")]
    pub(super) content: Option<String>,
    #[arg(long, help = "Read new skill content from stdin")]
    pub(super) content_stdin: bool,
    #[arg(
        long,
        value_name = "PATH",
        help = "Read new skill content from a UTF-8 file"
    )]
    pub(super) content_file: Option<PathBuf>,
    #[arg(long, help = "New skill config as JSON")]
    pub(super) config: Option<String>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    pub(super) output: OutputFormat,
}

#[derive(Debug, Args)]
pub(super) struct SkillDeleteArgs {
    #[arg(value_name = "SKILL-ID")]
    pub(super) skill_id: String,
    #[arg(long, help = "Skip the confirmation prompt")]
    pub(super) yes: bool,
}

#[derive(Debug, Args)]
pub(super) struct SkillImportArgs {
    #[arg(long, help = "URL or slug to import")]
    pub(super) url: Option<String>,
    #[arg(long, value_name = "PATH", help = "Local .skill or .zip archive")]
    pub(super) file: Option<PathBuf>,
    #[arg(
        long,
        default_value = "fail",
        help = "Conflict strategy: fail, overwrite, rename, or skip"
    )]
    pub(super) on_conflict: String,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    pub(super) output: OutputFormat,
    #[arg(
        long,
        help = "Allow --file to read outside the current working directory"
    )]
    pub(super) allow_external_file: bool,
}

#[derive(Debug, Args)]
pub(super) struct SkillRefreshArgs {
    #[arg(value_name = "SKILL-ID")]
    pub(super) skill_id: String,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    pub(super) output: OutputFormat,
}

#[derive(Debug, Args)]
pub(super) struct SkillSearchArgs {
    #[arg(value_name = "QUERY")]
    pub(super) query: String,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    pub(super) output: OutputFormat,
}

#[derive(Debug, Args)]
pub(super) struct SkillFilesArgs {
    #[command(subcommand)]
    pub(super) command: SkillFilesCommand,
}

#[derive(Debug, Subcommand)]
pub(super) enum SkillFilesCommand {
    #[command(about = "List files for a skill")]
    List(SkillFilesListArgs),
    #[command(about = "Create or update a skill file")]
    Upsert(SkillFilesUpsertArgs),
    #[command(about = "Delete a skill file")]
    Delete(SkillFilesDeleteArgs),
}

#[derive(Debug, Args)]
pub(super) struct SkillFilesListArgs {
    #[arg(value_name = "SKILL-ID")]
    pub(super) skill_id: String,
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    pub(super) output: OutputFormat,
}

#[derive(Debug, Args)]
pub(super) struct SkillFilesUpsertArgs {
    #[arg(value_name = "SKILL-ID")]
    pub(super) skill_id: String,
    #[arg(long, help = "File path within the skill (required)")]
    pub(super) path: Option<String>,
    #[arg(long, help = "File content")]
    pub(super) content: Option<String>,
    #[arg(long, help = "Read file content from stdin")]
    pub(super) content_stdin: bool,
    #[arg(
        long,
        value_name = "PATH",
        help = "Read file content from a UTF-8 file"
    )]
    pub(super) content_file: Option<PathBuf>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    pub(super) output: OutputFormat,
}

#[derive(Debug, Args)]
pub(super) struct SkillFilesDeleteArgs {
    #[arg(value_name = "SKILL-ID")]
    pub(super) skill_id: String,
    #[arg(value_name = "FILE-ID")]
    pub(super) file_id: String,
}
