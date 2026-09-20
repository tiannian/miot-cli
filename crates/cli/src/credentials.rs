use std::{error::Error, fs, path::PathBuf};

use miot_rs::CloudCredential;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StoredCredential {
    OAuth {
        access_token: String,
        refresh_token: String,
        expires_at_unix_seconds: u64,
    },
    Cloud {
        user_id: String,
        service_token: String,
        ssecurity: String,
        device_id: String,
    },
}

pub fn load_cloud_credential(
    account: Option<&str>,
) -> Result<(String, CloudCredential), Box<dyn Error>> {
    let accounts_directory = state_directory()?.join("accounts");
    let path = if let Some(account) = account {
        accounts_directory.join(format!("{}.json", filename_component(account)))
    } else {
        let mut paths = fs::read_dir(&accounts_directory)?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "json")
            })
            .collect::<Vec<_>>();
        paths.sort();
        match paths.as_slice() {
            [path] => path.clone(),
            [] => return Err("no saved account found; run `miot auth login` first".into()),
            _ => return Err("multiple saved accounts found; specify one with --account".into()),
        }
    };
    let stored: StoredCredential = serde_json::from_slice(&fs::read(&path)?)?;
    let StoredCredential::Cloud {
        user_id,
        service_token,
        ssecurity,
        device_id: _,
    } = stored
    else {
        return Err("the selected account does not have a Xiaomi cloud credential".into());
    };
    let account_id = path
        .file_stem()
        .and_then(|name| name.to_str())
        .ok_or("saved account path has no valid file name")?
        .to_owned();
    Ok((
        account_id,
        CloudCredential::new(user_id, service_token, ssecurity),
    ))
}

pub fn write_json(path: &std::path::Path, value: &impl Serialize) -> Result<(), Box<dyn Error>> {
    let parent = path.parent().ok_or("state path has no parent directory")?;
    fs::create_dir_all(parent)?;
    fs::write(path, serde_json::to_vec_pretty(value)?)?;
    Ok(())
}

pub fn state_directory() -> Result<PathBuf, Box<dyn Error>> {
    let home = std::env::var_os("HOME").ok_or("HOME is not set")?;
    Ok(PathBuf::from(home).join(".local/miot.rs"))
}

pub fn credential_path(account_id: &str) -> Result<PathBuf, Box<dyn Error>> {
    Ok(state_directory()?
        .join("accounts")
        .join(format!("{}.json", filename_component(account_id))))
}

pub fn filename_component(value: &str) -> String {
    let component: String = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect();
    if component.is_empty() {
        "account".to_owned()
    } else {
        component
    }
}
