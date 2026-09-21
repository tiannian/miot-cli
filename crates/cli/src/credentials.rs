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
        accounts_directory.join(format!("{}.toml", filename_component(account)))
    } else {
        let mut paths = fs::read_dir(&accounts_directory)?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "toml")
            })
            .collect::<Vec<_>>();
        paths.sort();
        match paths.as_slice() {
            [path] => path.clone(),
            [] => return Err("no saved account found; run `miot auth login` first".into()),
            _ => return Err("multiple saved accounts found; specify one with --account".into()),
        }
    };
    let stored: StoredCredential = toml::from_str(&fs::read_to_string(&path)?)?;
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

pub fn write_toml(path: &std::path::Path, value: &impl Serialize) -> Result<(), Box<dyn Error>> {
    let parent = path.parent().ok_or("state path has no parent directory")?;
    fs::create_dir_all(parent)?;
    fs::write(path, toml::to_string_pretty(value)?)?;
    Ok(())
}

pub fn write_json(path: &std::path::Path, value: &impl Serialize) -> Result<(), Box<dyn Error>> {
    let parent = path.parent().ok_or("state path has no parent directory")?;
    fs::create_dir_all(parent)?;
    fs::write(path, serde_json::to_string_pretty(value)?)?;
    Ok(())
}

pub fn state_directory() -> Result<PathBuf, Box<dyn Error>> {
    let home = std::env::var_os("HOME").ok_or("HOME is not set")?;
    Ok(PathBuf::from(home).join(".local/miot.rs"))
}

pub fn update_state_directory(account_id: &str, api: &str) -> Result<PathBuf, Box<dyn Error>> {
    let home = std::env::var_os("HOME").ok_or("HOME is not set")?;
    Ok(PathBuf::from(home)
        .join(".local")
        .join("miot.rs")
        .join(filename_component(api))
        .join(filename_component(account_id)))
}

pub fn credential_path(account_id: &str) -> Result<PathBuf, Box<dyn Error>> {
    Ok(state_directory()?
        .join("accounts")
        .join(format!("{}.toml", filename_component(account_id))))
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

#[cfg(test)]
mod tests {
    use super::{StoredCredential, filename_component, write_json, write_toml};
    use serde_json::json;

    #[test]
    fn writes_credentials_as_toml() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("account.toml");
        let credential = StoredCredential::Cloud {
            user_id: "user".to_owned(),
            service_token: "token".to_owned(),
            ssecurity: "security".to_owned(),
            device_id: "device".to_owned(),
        };

        write_toml(&path, &credential).unwrap();

        let contents = std::fs::read_to_string(path).unwrap();
        assert!(contents.contains("kind = \"cloud\""));
        assert!(!contents.trim_start().starts_with('{'));
        let restored: StoredCredential = toml::from_str(&contents).unwrap();
        assert!(matches!(restored, StoredCredential::Cloud { .. }));
    }

    #[test]
    fn writes_cloud_state_as_json() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("homes.json");
        let state = json!({ "home": null, "devices": [] });

        write_json(&path, &state).unwrap();

        let contents = std::fs::read_to_string(path).unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&contents).unwrap(),
            state
        );
    }

    #[test]
    fn normalizes_update_directory_account_component() {
        assert_eq!(filename_component("a/b@example.com"), "a_b_example_com");
    }
}
