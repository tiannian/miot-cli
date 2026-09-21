mod commands;
mod credentials;

use std::error::Error;

use clap::{Parser, Subcommand};
use commands::{auth::AuthCommand, update::UpdateArguments};
use tracing::{debug, error, info};
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(name = "miot", version, about = "MIoT command-line client")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    Auth(Box<AuthCommand>),
    Update(UpdateArguments),
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_target(false)
        .compact()
        .init();

    if let Err(error) = run(Cli::parse()).await {
        error!(error = %error, "command failed");
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> Result<(), Box<dyn Error>> {
    match cli.command {
        Some(Command::Auth(command)) => {
            info!(command = "auth", "running command");
            command.run().await
        }
        Some(Command::Update(arguments)) => {
            info!(command = "update", "running command");
            arguments.run().await
        }
        None => {
            debug!("printing version");
            println!("miot {}", miot_rs::version());
            Ok(())
        }
    }
}
