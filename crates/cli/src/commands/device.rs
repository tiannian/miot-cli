use std::{collections::BTreeMap, error::Error, fs, path::Path};

use clap::{Args, Subcommand, ValueEnum};
use serde_json::Value;

use crate::{
    commands::home,
    commands::table::print_table,
    credentials::{filename_component, saved_accounts, state_directory},
};

/// Read devices from cloud state written by `miot update`.
#[derive(Args)]
pub struct DeviceCommand {
    #[command(subcommand)]
    command: DeviceSubcommand,
}

#[derive(Subcommand)]
enum DeviceSubcommand {
    /// List each device's name, DID, and model.
    List(DeviceListArguments),
    /// Print the complete cached record for one device.
    Get(DeviceGetArguments),
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum Api {
    Miio,
    Mihome,
}

struct DeviceState {
    account: String,
    api: Api,
    devices: BTreeMap<String, Value>,
}

#[derive(Args)]
struct DeviceListArguments {
    /// Account identifier passed to `miot auth login --account`.
    #[arg(long)]
    account: Option<String>,
    /// State API to read. Defaults to Xiaomi Home when available.
    #[arg(long, value_enum)]
    api: Option<Api>,
    /// Room ID or unique room name to filter by.
    #[arg(long)]
    room: Option<String>,
}

#[derive(Args)]
struct DeviceGetArguments {
    /// Device identifier.
    did: String,
    /// Account identifier passed to `miot auth login --account`.
    #[arg(long)]
    account: Option<String>,
    /// State API to read. Defaults to Xiaomi Home when available.
    #[arg(long, value_enum)]
    api: Option<Api>,
}

impl DeviceCommand {
    pub fn run(self) -> Result<(), Box<dyn Error>> {
        match self.command {
            DeviceSubcommand::List(arguments) => list(&arguments),
            DeviceSubcommand::Get(arguments) => get(&arguments),
        }
    }
}

fn list(arguments: &DeviceListArguments) -> Result<(), Box<dyn Error>> {
    let mut state = devices(arguments.account.as_deref(), arguments.api)?;
    if let Some(room) = &arguments.room {
        let dids = home::room_dids(&state.account, state.api, room)?;
        state.devices.retain(|did, _| dids.contains(did));
    }
    let rows = state
        .devices
        .values()
        .map(|device| {
            vec![
                field(device, "name").to_owned(),
                field(device, "did").to_owned(),
                field(device, "model").to_owned(),
            ]
        })
        .collect::<Vec<_>>();
    print_table(&["NAME", "DID", "MODEL"], &rows);
    Ok(())
}

fn get(arguments: &DeviceGetArguments) -> Result<(), Box<dyn Error>> {
    let state = devices(arguments.account.as_deref(), arguments.api)?;
    let device = state.devices.get(&arguments.did).ok_or_else(|| {
        format!(
            "device `{}` was not found in the cached state",
            arguments.did
        )
    })?;
    println!("{}", serde_json::to_string_pretty(device)?);
    Ok(())
}

fn devices(account: Option<&str>, api: Option<Api>) -> Result<DeviceState, Box<dyn Error>> {
    let account = resolve_account(account)?;
    let root = state_directory()?;
    let preferred = match api {
        Some(api) => vec![api],
        None => vec![Api::Mihome, Api::Miio],
    };
    for api in preferred {
        let devices = match api {
            Api::Mihome => load_mihome_devices(&root, &account)?,
            Api::Miio => load_miio_devices(&root, &account)?,
        };
        if !devices.is_empty() {
            return Ok(DeviceState {
                account,
                api,
                devices,
            });
        }
    }
    Err(format!("no cached devices found for account `{account}`; run `miot update` first").into())
}

pub(crate) fn resolve_account(account: Option<&str>) -> Result<String, Box<dyn Error>> {
    if let Some(account) = account {
        return Ok(filename_component(account));
    }
    let accounts = saved_accounts()?;
    match accounts.as_slice() {
        [account] => Ok(account.id.clone()),
        [] => Err("no saved account found; run `miot auth login` first".into()),
        _ => Err("multiple saved accounts found; specify one with --account".into()),
    }
}

fn load_mihome_devices(
    root: &Path,
    account: &str,
) -> Result<BTreeMap<String, Value>, Box<dyn Error>> {
    let path = root.join("mihome").join(account).join("devices.json");
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    let value: Value = serde_json::from_str(&fs::read_to_string(path)?)?;
    collect_devices(
        value
            .as_object()
            .into_iter()
            .flat_map(|devices| devices.values()),
    )
}

fn load_miio_devices(
    root: &Path,
    account: &str,
) -> Result<BTreeMap<String, Value>, Box<dyn Error>> {
    let directory = root.join("miio").join(account).join("devices");
    if !directory.exists() {
        return Ok(BTreeMap::new());
    }
    let mut paths = fs::read_dir(directory)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .collect::<Vec<_>>();
    paths.sort();
    let mut result = BTreeMap::new();
    for path in paths {
        let page: Value = serde_json::from_str(&fs::read_to_string(path)?)?;
        result.extend(collect_devices(
            page.get("devices")
                .and_then(Value::as_array)
                .into_iter()
                .flatten(),
        )?);
    }
    Ok(result)
}

fn collect_devices<'a>(
    devices: impl Iterator<Item = &'a Value>,
) -> Result<BTreeMap<String, Value>, Box<dyn Error>> {
    let mut result = BTreeMap::new();
    for device in devices {
        let did = device
            .get("did")
            .and_then(Value::as_str)
            .ok_or("cached device record did not contain did")?;
        result.insert(did.to_owned(), device.clone());
    }
    Ok(result)
}

fn field<'a>(device: &'a Value, name: &str) -> &'a str {
    device.get(name).and_then(Value::as_str).unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::collect_devices;
    use serde_json::json;

    #[test]
    fn indexes_devices_by_did() {
        let devices = [json!({ "did": "2", "name": "Two" }), json!({ "did": "1" })];
        let result = collect_devices(devices.iter()).unwrap();
        assert_eq!(result.keys().collect::<Vec<_>>(), vec!["1", "2"]);
    }
}
