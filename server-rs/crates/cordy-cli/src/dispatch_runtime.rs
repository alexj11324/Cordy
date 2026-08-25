//! Runtime and runtime-profile command dispatch.
//!
//! Runtime lifecycle, usage, activity, and profile operations share this
//! module while preserving ID, wait/cascade, and path semantics.

use super::*;

pub(super) async fn run_runtime_command(
    cli: &Cli,
    environment: &Environment,
    args: &RuntimeArgs,
) -> Result<RunOutput> {
    match args {
        RuntimeArgs {
            command: RuntimeCommand::List { output },
        } => run_runtime_list(cli, environment, *output).await,
        RuntimeArgs {
            command:
                RuntimeCommand::Usage {
                    runtime_id,
                    output,
                    days,
                },
        } => run_runtime_usage(cli, environment, runtime_id, *output, *days).await,
        RuntimeArgs {
            command: RuntimeCommand::Activity { runtime_id, output },
        } => run_runtime_activity(cli, environment, runtime_id, *output).await,
        RuntimeArgs {
            command:
                RuntimeCommand::Rename {
                    runtime_id,
                    name,
                    machine,
                    output,
                },
        } => run_runtime_rename(cli, environment, runtime_id, name, *machine, *output).await,
        RuntimeArgs {
            command:
                RuntimeCommand::Delete {
                    runtime_id,
                    cascade,
                    output,
                },
        } => run_runtime_delete(cli, environment, runtime_id, *cascade, *output).await,
        RuntimeArgs {
            command:
                RuntimeCommand::Update {
                    runtime_id,
                    target_version,
                    output,
                    wait,
                },
        } => {
            run_runtime_update(
                cli,
                environment,
                runtime_id,
                target_version.as_deref(),
                *output,
                *wait,
            )
            .await
        }
        RuntimeArgs {
            command:
                RuntimeCommand::Profile(RuntimeProfileArgs {
                    command: RuntimeProfileCommand::List { output },
                }),
        } => run_runtime_profile_list(cli, environment, *output).await,
        RuntimeArgs {
            command:
                RuntimeCommand::Profile(RuntimeProfileArgs {
                    command: RuntimeProfileCommand::Create(args),
                }),
        } => run_runtime_profile_create(cli, environment, args).await,
        RuntimeArgs {
            command:
                RuntimeCommand::Profile(RuntimeProfileArgs {
                    command: RuntimeProfileCommand::Update(args),
                }),
        } => run_runtime_profile_update(cli, environment, args).await,
        RuntimeArgs {
            command:
                RuntimeCommand::Profile(RuntimeProfileArgs {
                    command: RuntimeProfileCommand::Delete { profile_id },
                }),
        } => run_runtime_profile_delete(cli, environment, profile_id).await,
        RuntimeArgs {
            command:
                RuntimeCommand::Profile(RuntimeProfileArgs {
                    command: RuntimeProfileCommand::SetPath { profile_id, path },
                }),
        } => run_runtime_profile_set_path(cli, environment, profile_id, path.as_deref()),
        RuntimeArgs {
            command:
                RuntimeCommand::Profile(RuntimeProfileArgs {
                    command: RuntimeProfileCommand::UnsetPath { profile_id },
                }),
        } => run_runtime_profile_unset_path(cli, environment, profile_id),
    }
}
