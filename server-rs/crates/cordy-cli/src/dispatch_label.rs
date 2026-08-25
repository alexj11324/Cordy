//! Label command dispatch.
//!
//! Label CRUD routing is kept in a small domain module with the existing
//! identifier and output handling unchanged.

use super::*;

pub(super) async fn run_label_command(
    cli: &Cli,
    environment: &Environment,
    args: &LabelArgs,
) -> Result<RunOutput> {
    match args {
        LabelArgs {
            command: LabelCommand::List { output, full_id },
        } => run_label_list(cli, environment, *output, *full_id).await,
        LabelArgs {
            command: LabelCommand::Get { id, output },
        } => run_label_get(cli, environment, id, *output).await,
        LabelArgs {
            command: LabelCommand::Create(args),
        } => run_label_create(cli, environment, args).await,
        LabelArgs {
            command: LabelCommand::Update(args),
        } => run_label_update(cli, environment, args).await,
        LabelArgs {
            command: LabelCommand::Delete { id, output },
        } => run_label_delete(cli, environment, id, *output).await,
    }
}
