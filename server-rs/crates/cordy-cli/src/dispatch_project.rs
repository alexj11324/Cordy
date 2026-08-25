//! Project and project-resource command dispatch.
//!
//! Project CRUD, status, and resource routing stays together while preserving
//! identifier, output, and nested-resource semantics.

use super::*;

pub(super) async fn run_project_command(
    cli: &Cli,
    environment: &Environment,
    args: &ProjectArgs,
) -> Result<RunOutput> {
    match args {
        ProjectArgs {
            command:
                ProjectCommand::List {
                    output,
                    full_id,
                    status,
                },
        } => run_project_list(cli, environment, *output, *full_id, status.as_deref()).await,
        ProjectArgs {
            command: ProjectCommand::Get { id, output },
        } => run_project_get(cli, environment, id, *output).await,
        ProjectArgs {
            command: ProjectCommand::Create(args),
        } => run_project_create(cli, environment, args).await,
        ProjectArgs {
            command: ProjectCommand::Update(args),
        } => run_project_update(cli, environment, args).await,
        ProjectArgs {
            command: ProjectCommand::Delete { id, output },
        } => run_project_delete(cli, environment, id, *output).await,
        ProjectArgs {
            command: ProjectCommand::Status { id, status, output },
        } => run_project_status(cli, environment, id, status, *output).await,
        ProjectArgs {
            command:
                ProjectCommand::Resource(ProjectResourceArgs {
                    command:
                        ProjectResourceCommand::List {
                            project_id,
                            output,
                            full_id,
                        },
                }),
        } => run_project_resource_list(cli, environment, project_id, *output, *full_id).await,
        ProjectArgs {
            command:
                ProjectCommand::Resource(ProjectResourceArgs {
                    command: ProjectResourceCommand::Add(args),
                }),
        } => run_project_resource_add(cli, environment, args).await,
        ProjectArgs {
            command:
                ProjectCommand::Resource(ProjectResourceArgs {
                    command: ProjectResourceCommand::Update(args),
                }),
        } => run_project_resource_update(cli, environment, args).await,
        ProjectArgs {
            command:
                ProjectCommand::Resource(ProjectResourceArgs {
                    command:
                        ProjectResourceCommand::Remove {
                            project_id,
                            resource_id,
                            output,
                        },
                }),
        } => run_project_resource_remove(cli, environment, project_id, resource_id, *output).await,
    }
}
