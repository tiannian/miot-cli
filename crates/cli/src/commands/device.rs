use std::{collections::BTreeMap, error::Error, fs, path::Path};

use clap::{Args, Subcommand, ValueEnum};
use miot_rs::MiotSpecClient;
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
    /// Print a device's supported `MIoT` properties, events, and actions.
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
    pub async fn run(self) -> Result<(), Box<dyn Error>> {
        match self.command {
            DeviceSubcommand::List(arguments) => list(&arguments),
            DeviceSubcommand::Get(arguments) => get(&arguments).await,
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
                online_status(device),
            ]
        })
        .collect::<Vec<_>>();
    print_table(&["NAME", "DID", "MODEL", "ONLINE"], &rows);
    Ok(())
}

async fn get(arguments: &DeviceGetArguments) -> Result<(), Box<dyn Error>> {
    let state = devices(arguments.account.as_deref(), arguments.api)?;
    let device = state.devices.get(&arguments.did).ok_or_else(|| {
        format!(
            "device `{}` was not found in the cached state",
            arguments.did
        )
    })?;
    let model = field(device, "model");
    if model.is_empty() {
        return Err("cached device record did not contain model".into());
    }
    let mut headers = vec!["NAME", "DID", "MODEL", "ONLINE"];
    let mut row = vec![
        field(device, "name").to_owned(),
        field(device, "did").to_owned(),
        model.to_owned(),
        online_status(device),
    ];
    if let Some(ip) = ip_address(device) {
        headers.push("IP");
        row.push(ip.to_owned());
    }
    print_table(&headers, &[row]);
    let spec = MiotSpecClient::new()?.instance_for_model(model).await?;
    print_spec(&spec);
    Ok(())
}

fn print_spec(spec: &Value) {
    let services = spec
        .get("services")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    let property_rows = services
        .iter()
        .flat_map(|service| {
            service
                .get("properties")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(move |property| {
                    vec![
                        iid(service),
                        iid(property),
                        label(service),
                        label(property),
                        string_list(property.get("access")),
                        field_value(property, "format"),
                        field_value(property, "unit"),
                        compact_json(property.get("value-range")),
                        compact_json(property.get("value-list")),
                    ]
                })
        })
        .collect::<Vec<_>>();
    println!("\nProperties");
    print_table(
        &[
            "SIID", "PIID", "SERVICE", "PROPERTY", "ACCESS", "FORMAT", "UNIT", "RANGE", "VALUES",
        ],
        &property_rows,
    );

    let event_rows = services
        .iter()
        .flat_map(|service| {
            service
                .get("events")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(move |event| {
                    vec![
                        iid(service),
                        iid(event),
                        label(service),
                        label(event),
                        iid_list(event.get("argument")),
                    ]
                })
        })
        .collect::<Vec<_>>();
    println!("\nEvents");
    print_table(
        &["SIID", "EIID", "SERVICE", "EVENT", "ARGUMENT PIIDS"],
        &event_rows,
    );

    let action_rows = services
        .iter()
        .flat_map(|service| {
            service
                .get("actions")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(move |action| {
                    vec![
                        iid(service),
                        iid(action),
                        label(service),
                        label(action),
                        iid_list(action.get("in")),
                        iid_list(action.get("out")),
                    ]
                })
        })
        .collect::<Vec<_>>();
    println!("\nActions");
    print_table(
        &[
            "SIID",
            "AIID",
            "SERVICE",
            "ACTION",
            "INPUT PIIDS",
            "OUTPUT PIIDS",
        ],
        &action_rows,
    );
}

fn iid(value: &Value) -> String {
    value.get("iid").map_or_else(String::new, Value::to_string)
}

fn label(value: &Value) -> String {
    let name = field_value(value, "description");
    if name.is_empty() {
        field_value(value, "type")
    } else {
        name
    }
}

fn field_value(value: &Value, name: &str) -> String {
    value
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned()
}

fn string_list(value: Option<&Value>) -> String {
    value
        .and_then(Value::as_array)
        .map_or_else(String::new, |values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(",")
        })
}

fn iid_list(value: Option<&Value>) -> String {
    value
        .and_then(Value::as_array)
        .map_or_else(String::new, |values| {
            values
                .iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join(",")
        })
}

fn compact_json(value: Option<&Value>) -> String {
    value.map_or_else(String::new, Value::to_string)
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

fn online_status(device: &Value) -> String {
    device
        .get("isOnline")
        .or_else(|| device.get("online"))
        .and_then(Value::as_bool)
        .map_or_else(|| "unknown".to_owned(), |online| online.to_string())
}

fn ip_address(device: &Value) -> Option<&str> {
    ["ip", "localip", "local_ip", "lan_ip"]
        .iter()
        .find_map(|key| device.get(key).and_then(Value::as_str))
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::{collect_devices, iid_list, ip_address, online_status, string_list};
    use serde_json::json;

    #[test]
    fn indexes_devices_by_did() {
        let devices = [json!({ "did": "2", "name": "Two" }), json!({ "did": "1" })];
        let result = collect_devices(devices.iter()).unwrap();
        assert_eq!(result.keys().collect::<Vec<_>>(), vec!["1", "2"]);
    }

    #[test]
    fn reads_online_status_from_both_cloud_field_names() {
        assert_eq!(online_status(&json!({ "isOnline": true })), "true");
        assert_eq!(online_status(&json!({ "online": false })), "false");
        assert_eq!(online_status(&json!({})), "unknown");
    }

    #[test]
    fn finds_an_available_device_ip_address() {
        assert_eq!(
            ip_address(&json!({ "localip": "192.168.1.2" })),
            Some("192.168.1.2")
        );
        assert_eq!(ip_address(&json!({ "ip": "" })), None);
    }

    #[test]
    fn renders_spec_lists_compactly() {
        assert_eq!(string_list(Some(&json!(["read", "write"]))), "read,write");
        assert_eq!(iid_list(Some(&json!([1, 2]))), "1,2");
    }
}
