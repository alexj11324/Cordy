use std::io;

use clap::Parser;
use cordy_cli::{error, run, Cli};

#[tokio::main]
async fn main() {
    // Environment preparation is deliberately isolated from the long-lived
    // daemon process. The parent adapter invokes this private mode with a
    // JSON request over stdin; dispatch before Clap/config parsing so the
    // helper does not require profile credentials or normal CLI state.
    if std::env::args().nth(1).as_deref()
        == Some(cordy_daemon::execenv::isolation::PREPARATION_HELPER_ARG)
    {
        let stdin = io::stdin();
        let mut stdout = io::stdout().lock();
        if let Err(error) =
            cordy_daemon::execenv::isolation::run_preparation_helper(stdin.lock(), &mut stdout)
                .await
        {
            eprintln!("{error:#}");
            std::process::exit(1);
        }
        return;
    }

    let cli = Cli::parse();
    let environment = match cordy_cli::config::Environment::from_process() {
        Ok(environment) => environment,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    };
    match run(&cli, &environment).await {
        Ok(output) => {
            print!("{}", output.stdout);
            eprint!("{}", output.stderr);
        }
        Err(cause) => {
            if let Some(output) = cordy_cli::output_for_error(&cause) {
                print!("{}", output.stdout);
                eprint!("{}", output.stderr);
            }
            if !cordy_cli::suppress_error_message(&cause) {
                eprintln!(
                    "{}",
                    error::format_error(&cause, cli.debug_enabled(&environment))
                );
            }
            std::process::exit(error::exit_code(&cause));
        }
    }
}
