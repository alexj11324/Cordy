use clap::Parser;
use cordy_cli::{error, run, run_private_helper, Cli};

#[tokio::main]
async fn main() {
    let args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    match run_private_helper(&args, stdin.lock(), &mut stdout).await {
        Ok(true) => return,
        Ok(false) => {}
        Err(error) => {
            eprintln!("{error:#}");
            std::process::exit(1);
        }
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
            if let Some(output) = cordy_cli::command_error_output(&cause) {
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
