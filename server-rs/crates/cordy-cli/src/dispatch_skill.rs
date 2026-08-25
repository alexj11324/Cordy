//! Skill command dispatch.
//!
//! The root dispatcher delegates the full skill/files branch here so import,
//! search, refresh, and file mutations share one auditable routing boundary.

use std::io::Read;

use super::*;

pub(super) async fn run_skill_command<R: Read>(
    cli: &Cli,
    environment: &Environment,
    args: &SkillArgs,
    input: &mut R,
) -> Result<RunOutput> {
    match args {
        SkillArgs {
            command: SkillCommand::List { output },
        } => run_skill_list(cli, environment, *output).await,
        SkillArgs {
            command: SkillCommand::Get(args),
        } => run_skill_get(cli, environment, args).await,
        SkillArgs {
            command: SkillCommand::Create(args),
        } => run_skill_create(cli, environment, args, input).await,
        SkillArgs {
            command: SkillCommand::Update(args),
        } => run_skill_update(cli, environment, args, input).await,
        SkillArgs {
            command: SkillCommand::Delete(args),
        } => run_skill_delete(cli, environment, args, input).await,
        SkillArgs {
            command: SkillCommand::Import(args),
        } => run_skill_import(cli, environment, args).await,
        SkillArgs {
            command: SkillCommand::Refresh(args),
        } => run_skill_refresh(cli, environment, args).await,
        SkillArgs {
            command: SkillCommand::Search(args),
        } => run_skill_search(cli, environment, args).await,
        SkillArgs {
            command:
                SkillCommand::Files(SkillFilesArgs {
                    command: SkillFilesCommand::List(args),
                }),
        } => run_skill_files_list(cli, environment, args).await,
        SkillArgs {
            command:
                SkillCommand::Files(SkillFilesArgs {
                    command: SkillFilesCommand::Upsert(args),
                }),
        } => run_skill_files_upsert(cli, environment, args, input).await,
        SkillArgs {
            command:
                SkillCommand::Files(SkillFilesArgs {
                    command: SkillFilesCommand::Delete(args),
                }),
        } => run_skill_files_delete(cli, environment, args).await,
    }
}
