mod commands;
mod credentials;

use std::error::Error;

use clap::{Parser, Subcommand};
use commands::{auth::AuthCommand, update::UpdateArguments};

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
    if let Err(error) = run(Cli::parse()).await {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> Result<(), Box<dyn Error>> {
    match cli.command {
        Some(Command::Auth(command)) => command.run().await,
        Some(Command::Update(arguments)) => arguments.run().await,
        None => {
            println!("miot {}", miot_rs::version());
            Ok(())
        }
    }
}
