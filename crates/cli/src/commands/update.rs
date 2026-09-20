use std::error::Error;

use clap::Args;
use miot_rs::{ApiClient, HomeDeviceListQuery};
use serde_json::Value;

use crate::credentials::{load_cloud_credential, state_directory, write_json};

#[derive(Args)]
pub struct UpdateArguments {
    /// Account identifier passed to `miot auth login --account`.
    #[arg(long)]
    account: Option<String>,
    /// Xiaomi cloud region.
    #[arg(long, default_value = "cn")]
    region: String,
}

impl UpdateArguments {
    pub async fn run(self) -> Result<(), Box<dyn Error>> {
        update(self).await
    }
}

async fn update(arguments: UpdateArguments) -> Result<(), Box<dyn Error>> {
    let (account_id, credential) = load_cloud_credential(arguments.account.as_deref())?;
    let client = ApiClient::new(&arguments.region, credential)?;
    let homes = client.home_merged().await?;
    write_json(&state_directory()?.join("homes.json"), &homes)?;
    let homes_to_update = homes
        .get("homelist")
        .or_else(|| homes.get("home_list"))
        .and_then(Value::as_array)
        .ok_or("cloud home response did not contain homelist")?;
    let mut updated = 0_usize;
    for home in homes_to_update {
        let home_id = required_i64(home, &["id", "home_id"])?;
        let home_owner = required_i64(home, &["owner_id", "home_owner", "uid"])?;
        let devices = update_home_devices(&client, home_owner, home_id).await?;
        write_json(
            &state_directory()?
                .join("devices")
                .join(format!("{home_id}.json")),
            &devices,
        )?;
        updated += 1;
    }
    println!(
        "Updated {updated} home(s) for account {account_id}. State saved to {}.",
        state_directory()?.display()
    );
    Ok(())
}

async fn update_home_devices(
    client: &ApiClient,
    home_owner: i64,
    home_id: i64,
) -> Result<Value, Box<dyn Error>> {
    let mut query = HomeDeviceListQuery::new(home_owner, home_id);
    let mut devices = Vec::new();
    loop {
        let page = client.home_device_list(&query).await?;
        let list = page
            .get("list")
            .or_else(|| page.get("device_info"))
            .and_then(Value::as_array)
            .ok_or("cloud home device response did not contain device list")?;
        devices.extend(list.iter().cloned());
        let next_start_did = page
            .get("next_start_did")
            .or_else(|| page.get("start_did"))
            .or_else(|| page.get("max_did"))
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned);
        if !page
            .get("has_more")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            break;
        }
        let Some(next_start_did) = next_start_did else {
            return Err(
                "cloud home device response indicated more pages without next_start_did".into(),
            );
        };
        if next_start_did == query.start_did {
            return Err("cloud home device pagination did not advance".into());
        }
        query.start_did = next_start_did;
    }
    Ok(serde_json::json!({ "home_id": home_id, "home_owner": home_owner, "devices": devices }))
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

#[cfg(test)]
mod tests {
    use super::required_i64;
    use serde_json::json;
    #[test]
    fn reads_numeric_home_identifiers_from_cloud_response_strings() {
        let home = json!({ "id": "123", "uid": "456" });
        assert_eq!(required_i64(&home, &["id", "home_id"]).unwrap(), 123);
        assert_eq!(required_i64(&home, &["owner_id", "uid"]).unwrap(), 456);
    }
}
