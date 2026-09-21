use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    path::Path,
};

use clap::{ArgGroup, Args};
use miot_rs::{
    ApiClient, DevRoomPageQuery, DeviceListPageQuery, HomeDeviceListQuery, MiHomeApiClient,
};
use serde_json::{Value, json};
use tracing::{debug, info};

use crate::credentials::{
    load_cloud_credential, load_oauth_credential, update_state_directory, write_json,
};

#[derive(Args)]
#[command(group(
    ArgGroup::new("api")
        .required(true)
        .args(["miio", "mihome"])
))]
pub struct UpdateArguments {
    /// Account identifier passed to `miot auth login --account`.
    #[arg(long)]
    account: Option<String>,
    /// Xiaomi cloud region.
    #[arg(long, default_value = "cn")]
    region: String,
    /// Update state through the legacy `MiIO` API.
    #[arg(long)]
    miio: bool,
    /// Update state through the Xiaomi Home API.
    #[arg(long)]
    mihome: bool,
}

impl UpdateArguments {
    pub async fn run(self) -> Result<(), Box<dyn Error>> {
        let api = if self.miio { "miio" } else { "mihome" };
        if self.miio {
            let (account_id, credential) = load_cloud_credential(self.account.as_deref())?;
            let update_directory = update_state_directory(&account_id, api)?;
            info!(account = %account_id, region = %self.region, api, "updating cloud state");
            let client = ApiClient::new(&self.region, credential)?;
            update_miio(&client, &update_directory, &account_id).await
        } else {
            let (account_id, credential) = load_oauth_credential(self.account.as_deref())?;
            let update_directory = update_state_directory(&account_id, api)?;
            info!(account = %account_id, region = %self.region, api, "updating cloud state");
            let client = MiHomeApiClient::new(&self.region, credential)?;
            update_mihome(&client, &update_directory, &account_id).await
        }
    }
}

async fn update_miio(
    client: &ApiClient,
    update_directory: &Path,
    account_id: &str,
) -> Result<(), Box<dyn Error>> {
    let homes = client.home_merged().await?;
    write_json(&update_directory.join("homes.json"), &homes)?;
    let homes_to_update = homes
        .get("homelist")
        .or_else(|| homes.get("home_list"))
        .and_then(Value::as_array)
        .ok_or("cloud home response did not contain homelist")?;
    let mut updated = 0_usize;
    for home in homes_to_update {
        let home_id = required_i64(home, &["id", "home_id"])?;
        let home_owner = required_i64(home, &["owner_id", "home_owner", "uid"])?;
        let devices = update_miio_home_devices(client, home_owner, home_id).await?;
        debug!(
            home_id,
            device_count = devices["devices"].as_array().map_or(0, Vec::len),
            "updated MiIO home devices"
        );
        write_json(
            &update_directory
                .join("devices")
                .join(format!("{home_id}.json")),
            &devices,
        )?;
        updated += 1;
    }
    println!(
        "Updated {updated} MiIO home(s) for account {account_id}. State saved to {}.",
        update_directory.display()
    );
    Ok(())
}

async fn update_miio_home_devices(
    client: &ApiClient,
    home_owner: i64,
    home_id: i64,
) -> Result<Value, Box<dyn Error>> {
    let mut query = HomeDeviceListQuery::new(home_owner, home_id);
    let mut devices = Vec::new();
    loop {
        debug!(home_id, start_did = %query.start_did, "requesting MiIO home device page");
        let page = client.home_device_list(&query).await?;
        let list = page
            .get("list")
            .or_else(|| page.get("device_info"))
            .and_then(Value::as_array)
            .ok_or("cloud home device response did not contain device list")?;
        devices.extend(list.iter().cloned());
        if !has_more(&page) {
            break;
        }
        let next_start_did = next_page_value(&page, &["next_start_did", "start_did", "max_did"])
            .ok_or("cloud home device response indicated more pages without next_start_did")?;
        if next_start_did == query.start_did {
            return Err("cloud home device pagination did not advance".into());
        }
        query.start_did = next_start_did;
    }
    Ok(json!({ "home_id": home_id, "home_owner": home_owner, "devices": devices }))
}

async fn update_mihome(
    client: &MiHomeApiClient,
    update_directory: &Path,
    account_id: &str,
) -> Result<(), Box<dyn Error>> {
    let home_info = client.home_info().await?;
    let dev_room_pages = get_dev_room_pages(client, &home_info).await?;
    let mut groups = home_device_groups(&home_info, &dev_room_pages);
    let mut dids: BTreeSet<String> = groups.values().flatten().cloned().collect();

    for device in get_all_device_pages(client, DeviceListPageQuery::all_visible()).await? {
        if let Some((did, owner)) = shared_device_owner(&device) {
            groups
                .entry(format!("separated_shared_{owner}"))
                .or_default()
                .insert(did.clone());
            dids.insert(did);
        }
    }

    let mut devices = BTreeMap::new();
    for did_chunk in dids.iter().cloned().collect::<Vec<_>>().chunks(150) {
        for device in
            get_all_device_pages(client, DeviceListPageQuery::new(did_chunk.to_vec())).await?
        {
            let did = device
                .get("did")
                .and_then(Value::as_str)
                .ok_or("cloud Xiaomi Home device response did not contain did")?;
            devices.insert(did.to_owned(), device);
        }
    }

    write_json(
        &update_directory.join("homes.json"),
        &json!({ "home_info": home_info, "dev_room_pages": dev_room_pages }),
    )?;
    write_json(&update_directory.join("devices.json"), &devices)?;
    for (group, group_dids) in &groups {
        let group_devices = group_dids
            .iter()
            .filter_map(|did| devices.get(did).cloned())
            .collect::<Vec<_>>();
        write_json(
            &update_directory
                .join("devices")
                .join(format!("{group}.json")),
            &json!({ "group": group, "dids": group_dids, "devices": group_devices }),
        )?;
    }
    println!(
        "Updated {} Xiaomi Home device(s) in {} group(s) for account {account_id}. State saved to {}.",
        devices.len(),
        groups.len(),
        update_directory.display()
    );
    Ok(())
}

async fn get_dev_room_pages(
    client: &MiHomeApiClient,
    home_info: &Value,
) -> Result<Vec<Value>, Box<dyn Error>> {
    if !has_more(home_info) {
        return Ok(Vec::new());
    }
    let mut start_id = next_page_value(home_info, &["max_id"])
        .ok_or("cloud home response indicated more pages without max_id")?;
    let mut pages = Vec::new();
    loop {
        let page = client
            .dev_room_page(&DevRoomPageQuery {
                start_id: Some(start_id),
                ..DevRoomPageQuery::default()
            })
            .await?;
        if !has_more(&page) {
            pages.push(page);
            break;
        }
        start_id = next_page_value(&page, &["max_id"])
            .ok_or("cloud device-room response indicated more pages without max_id")?;
        pages.push(page);
    }
    Ok(pages)
}

async fn get_all_device_pages(
    client: &MiHomeApiClient,
    mut query: DeviceListPageQuery,
) -> Result<Vec<Value>, Box<dyn Error>> {
    let mut devices = Vec::new();
    loop {
        let page = client.device_list_page(&query).await?;
        let list = page
            .get("list")
            .and_then(Value::as_array)
            .ok_or("cloud Xiaomi Home device response did not contain list")?;
        devices.extend(list.iter().cloned());
        if !has_more(&page) {
            break;
        }
        let next_start_did = next_page_value(&page, &["next_start_did"]).ok_or(
            "cloud Xiaomi Home device response indicated more pages without next_start_did",
        )?;
        if query.start_did.as_deref() == Some(&next_start_did) {
            return Err("cloud Xiaomi Home device pagination did not advance".into());
        }
        query.start_did = Some(next_start_did);
    }
    Ok(devices)
}

fn home_device_groups(
    home_info: &Value,
    dev_room_pages: &[Value],
) -> BTreeMap<String, BTreeSet<String>> {
    let mut groups = BTreeMap::new();
    for source in ["homelist", "share_home_list"] {
        for home in home_info
            .get(source)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            add_home_dids(&mut groups, home);
        }
    }
    for page in dev_room_pages {
        for home in page
            .get("info")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            add_home_dids(&mut groups, home);
        }
    }
    groups
}

fn add_home_dids(groups: &mut BTreeMap<String, BTreeSet<String>>, home: &Value) {
    let Some(home_id) = home.get("id").and_then(value_as_string) else {
        return;
    };
    let group = groups.entry(home_id).or_default();
    for did in home
        .get("dids")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(did) = value_as_string(did) {
            group.insert(did);
        }
    }
    for room in home
        .get("roomlist")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        for did in room
            .get("dids")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(did) = value_as_string(did) {
                group.insert(did);
            }
        }
    }
}

fn shared_device_owner(device: &Value) -> Option<(String, String)> {
    let did = device.get("did").and_then(Value::as_str)?.to_owned();
    let owner = device.get("owner")?;
    let owner_id = owner.get("userid").and_then(value_as_string)?;
    owner.get("nickname").and_then(Value::as_str)?;
    Some((did, owner_id))
}

fn has_more(page: &Value) -> bool {
    page.get("has_more")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn next_page_value(page: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        page.get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    })
}

fn required_i64(home: &Value, keys: &[&str]) -> Result<i64, Box<dyn Error>> {
    keys.iter()
        .find_map(|key| home.get(key).and_then(value_as_i64))
        .ok_or_else(|| {
            format!(
                "cloud home record did not contain any of {}",
                keys.join(", ")
            )
            .into()
        })
}

fn value_as_i64(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|number| i64::try_from(number).ok()))
        .or_else(|| value.as_str().and_then(|number| number.parse().ok()))
}

fn value_as_string(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::to_owned)
        .or_else(|| value.as_i64().map(|value| value.to_string()))
        .or_else(|| value.as_u64().map(|value| value.to_string()))
}

#[cfg(test)]
mod tests {
    use super::{home_device_groups, required_i64, shared_device_owner};
    use serde_json::json;

    #[test]
    fn reads_numeric_home_identifiers_from_cloud_response_strings() {
        let home = json!({ "id": "123", "uid": "456" });
        assert_eq!(required_i64(&home, &["id", "home_id"]).unwrap(), 123);
        assert_eq!(required_i64(&home, &["owner_id", "uid"]).unwrap(), 456);
    }

    #[test]
    fn groups_owned_and_shared_home_devices() {
        let home_info = json!({ "homelist": [{ "id": 1, "dids": ["a"], "roomlist": [{ "dids": ["b"] }] }], "share_home_list": [{ "id": "2", "dids": ["c"] }] });
        let groups = home_device_groups(&home_info, &[]);
        assert_eq!(groups["1"].len(), 2);
        assert!(groups["2"].contains("c"));
    }

    #[test]
    fn identifies_directly_shared_devices() {
        let device = json!({ "did": "a", "owner": { "userid": 42, "nickname": "Alice" } });
        assert_eq!(
            shared_device_owner(&device),
            Some(("a".to_owned(), "42".to_owned()))
        );
    }
}
