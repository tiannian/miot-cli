mod commands;
mod credentials;

use std::error::Error;

use clap::{Parser, Subcommand};
use commands::{
    account::AccountCommand, auth::AuthCommand, device::DeviceCommand, lan::LanCommand,
    update::UpdateArguments,
};
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
    /// List saved accounts.
    Account(AccountCommand),
    Auth(Box<AuthCommand>),
    /// Read devices from locally updated cloud state.
    Device(Box<DeviceCommand>),
    Lan(Box<LanCommand>),
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
        Some(Command::Account(_)) => {
            info!(command = "account", "running command");
            AccountCommand::run()
        }
        Some(Command::Auth(command)) => {
            info!(command = "auth", "running command");
            command.run().await
        }
        Some(Command::Device(command)) => {
            info!(command = "device", "running command");
            command.run()
        }
        Some(Command::Lan(command)) => {
            info!(command = "lan", "running command");
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
