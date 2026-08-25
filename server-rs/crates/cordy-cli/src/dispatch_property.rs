//! Issue-property command dispatch.
//!
//! Property CRUD and archive toggles stay in one focused route module while
//! preserving the include-archived and output semantics.

use super::*;

pub(super) async fn run_property_command(
    cli: &Cli,
    environment: &Environment,
    args: &PropertyArgs,
) -> Result<RunOutput> {
    match args {
        PropertyArgs {
            command:
                PropertyCommand::List {
                    output,
                    include_archived,
                },
        } => run_property_list(cli, environment, *output, *include_archived).await,
        PropertyArgs {
            command: PropertyCommand::Get { property, output },
        } => run_property_get(cli, environment, property, *output).await,
        PropertyArgs {
            command: PropertyCommand::Create(args),
        } => run_property_create(cli, environment, args).await,
        PropertyArgs {
            command: PropertyCommand::Update(args),
        } => run_property_update(cli, environment, args).await,
        PropertyArgs {
            command: PropertyCommand::Archive(args),
        } => run_property_archive(cli, environment, args, true).await,
        PropertyArgs {
            command: PropertyCommand::Unarchive(args),
        } => run_property_archive(cli, environment, args, false).await,
    }
}
