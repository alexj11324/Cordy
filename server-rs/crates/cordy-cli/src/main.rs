use clap::Parser;
use cordy_cli::{error, run, Cli};

#[tokio::main]
async fn main() {
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
            eprintln!(
                "{}",
                error::format_error(&cause, cli.debug_enabled(&environment))
            );
            std::process::exit(error::exit_code(&cause));
        }
    }
}
