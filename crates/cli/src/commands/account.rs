use std::error::Error;

use clap::Args;

use crate::{commands::table::print_table, credentials::saved_accounts};

/// List accounts saved by `miot auth login`.
#[derive(Args)]
pub struct AccountCommand;

impl AccountCommand {
    pub fn run() -> Result<(), Box<dyn Error>> {
        let accounts = saved_accounts()?;
        if accounts.is_empty() {
            println!("No saved accounts. Run `miot auth login` first.");
            return Ok(());
        }
        let rows = accounts
            .into_iter()
            .map(|account| vec![account.id, account.credential_kind.to_owned()])
            .collect::<Vec<_>>();
        print_table(&["ACCOUNT", "CREDENTIAL"], &rows);
        Ok(())
    }
}
