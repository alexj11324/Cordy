//! Autopilot command dispatch.
//!
//! Keeping the full autopilot routing branch together makes the root dispatcher
//! a small, auditable command router without changing handler ownership or
//! input forwarding.

use std::io::Read;

use super::*;

pub(super) async fn run_autopilot_command<R: Read>(
    cli: &Cli,
    environment: &Environment,
    args: &AutopilotArgs,
    input: &mut R,
) -> Result<RunOutput> {
    match args {
        AutopilotArgs {
            command:
                AutopilotCommand::List {
                    status,
                    output,
                    full_id,
                },
        } => run_autopilot_list(cli, environment, status, *output, *full_id).await,
        AutopilotArgs {
            command: AutopilotCommand::Get { id, output },
        } => run_autopilot_get(cli, environment, id, *output).await,
        AutopilotArgs {
            command: AutopilotCommand::Create(args),
        } => run_autopilot_create(cli, environment, args).await,
        AutopilotArgs {
            command: AutopilotCommand::Update(args),
        } => run_autopilot_update(cli, environment, args).await,
        AutopilotArgs {
            command: AutopilotCommand::Delete { id },
        } => run_autopilot_delete(cli, environment, id).await,
        AutopilotArgs {
            command: AutopilotCommand::Trigger { id, output },
        } => run_autopilot_trigger(cli, environment, id, *output).await,
        AutopilotArgs {
            command:
                AutopilotCommand::Runs {
                    id,
                    limit,
                    offset,
                    output,
                },
        } => run_autopilot_runs(cli, environment, id, *limit, *offset, *output).await,
        AutopilotArgs {
            command: AutopilotCommand::TriggerAdd(args),
        } => run_autopilot_trigger_add(cli, environment, args).await,
        AutopilotArgs {
            command: AutopilotCommand::TriggerUpdate(args),
        } => run_autopilot_trigger_update(cli, environment, args).await,
        AutopilotArgs {
            command:
                AutopilotCommand::TriggerDelete {
                    autopilot_id,
                    trigger_id,
                },
        } => run_autopilot_trigger_delete(cli, environment, autopilot_id, trigger_id).await,
        AutopilotArgs {
            command: AutopilotCommand::TriggerRotateUrl(args),
        } => run_autopilot_trigger_rotate_url(cli, environment, args, input).await,
    }
}
