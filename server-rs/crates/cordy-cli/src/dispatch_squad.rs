//! Squad command dispatch.
//!
//! Squad membership and activity routing remains in one domain module while
//! preserving the existing handler and output semantics.

use super::*;

pub(super) async fn run_squad_command(
    cli: &Cli,
    environment: &Environment,
    args: &SquadArgs,
) -> Result<RunOutput> {
    match args {
        SquadArgs {
            command: SquadCommand::List { output },
        } => run_squad_list(cli, environment, *output).await,
        SquadArgs {
            command: SquadCommand::Get { squad_id, output },
        } => run_squad_get(cli, environment, squad_id, *output).await,
        SquadArgs {
            command: SquadCommand::Create(args),
        } => run_squad_create(cli, environment, args).await,
        SquadArgs {
            command: SquadCommand::Update(args),
        } => run_squad_update(cli, environment, args).await,
        SquadArgs {
            command: SquadCommand::Delete { squad_id, output },
        } => run_squad_delete(cli, environment, squad_id, *output).await,
        SquadArgs {
            command:
                SquadCommand::Member(SquadMemberArgs {
                    command: SquadMemberCommand::List { squad_id, output },
                }),
        } => run_squad_member_list(cli, environment, squad_id, *output).await,
        SquadArgs {
            command:
                SquadCommand::Member(SquadMemberArgs {
                    command: SquadMemberCommand::Add(args),
                }),
        } => run_squad_member_add(cli, environment, args).await,
        SquadArgs {
            command:
                SquadCommand::Member(SquadMemberArgs {
                    command: SquadMemberCommand::SetRole(args),
                }),
        } => run_squad_member_set_role(cli, environment, args).await,
        SquadArgs {
            command:
                SquadCommand::Member(SquadMemberArgs {
                    command: SquadMemberCommand::Remove(args),
                }),
        } => run_squad_member_remove(cli, environment, args).await,
        SquadArgs {
            command: SquadCommand::Activity(args),
        } => run_squad_activity(cli, environment, args).await,
    }
}
