use std::error::Error;

use clap::Args;

use crate::credentials::saved_accounts;

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
        println!("ACCOUNT\tCREDENTIAL");
        for account in accounts {
            println!("{}\t{}", account.id, account.credential_kind);
        }
        Ok(())
    }
}
