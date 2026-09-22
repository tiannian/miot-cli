use std::{collections::BTreeSet, error::Error, fs};

use clap::Args;
use serde_json::Value;

use crate::{
    commands::device::{Api, resolve_account},
    credentials::state_directory,
};

/// List homes and rooms from cloud state written by `miot update`.
#[derive(Args)]
pub struct HomeCommand {
    /// Account identifier passed to `miot auth login --account`.
    #[arg(long)]
    account: Option<String>,
    /// State API to read. Defaults to Xiaomi Home when available.
    #[arg(long, value_enum)]
    api: Option<Api>,
}

struct Home {
    id: String,
    name: String,
    rooms: Vec<Room>,
}

struct Room {
    id: String,
    name: String,
    dids: BTreeSet<String>,
}

impl HomeCommand {
    pub fn run(self) -> Result<(), Box<dyn Error>> {
        let account = resolve_account(self.account.as_deref())?;
        let (_, homes) = homes(&account, self.api)?;
        println!("HOME_ID\tHOME_NAME\tROOM_ID\tROOM_NAME");
        for home in homes {
            if home.rooms.is_empty() {
                println!("{}\t{}\t\t", home.id, home.name);
            }
            for room in home.rooms {
                println!("{}\t{}\t{}\t{}", home.id, home.name, room.id, room.name);
            }
        }
        Ok(())
    }
}

pub(crate) fn room_dids(
    account: &str,
    api: Api,
    room_selector: &str,
) -> Result<BTreeSet<String>, Box<dyn Error>> {
    let (_, homes) = homes(account, Some(api))?;
    let rooms = homes
        .iter()
        .flat_map(|home| home.rooms.iter())
        .collect::<Vec<_>>();
    let matches = rooms
        .iter()
        .filter(|room| room.id == room_selector)
        .collect::<Vec<_>>();
    let matches = if matches.is_empty() {
        rooms
            .iter()
            .filter(|room| room.name == room_selector)
            .collect::<Vec<_>>()
    } else {
        matches
    };
    match matches.as_slice() {
        [] => Err(format!("room `{room_selector}` was not found in the cached state").into()),
        [room] => Ok(room.dids.clone()),
        _ => Err(format!("room name `{room_selector}` is ambiguous; use its room ID").into()),
    }
}

fn homes(account: &str, api: Option<Api>) -> Result<(Api, Vec<Home>), Box<dyn Error>> {
    let root = state_directory()?;
    let preferred = match api {
        Some(api) => vec![api],
        None => vec![Api::Mihome, Api::Miio],
    };
    for api in preferred {
        let path = match api {
            Api::Mihome => root.join("mihome").join(account).join("homes.json"),
            Api::Miio => root.join("miio").join(account).join("homes.json"),
        };
        if path.exists() {
            return Ok((
                api,
                parse_homes(api, &serde_json::from_str(&fs::read_to_string(path)?)?)?,
            ));
        }
    }
    Err(format!("no cached homes found for account `{account}`; run `miot update` first").into())
}

fn parse_homes(api: Api, value: &Value) -> Result<Vec<Home>, Box<dyn Error>> {
    let mut sources = Vec::new();
    match api {
        Api::Miio => sources.push(value),
        Api::Mihome => {
            sources.push(value.get("home_info").unwrap_or(value));
            for page in value
                .get("dev_room_pages")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                sources.push(page);
            }
        }
    }
    let mut homes = Vec::new();
    for source in sources {
        for key in ["homelist", "home_list", "share_home_list", "info"] {
            for home in source
                .get(key)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let id = value_as_string(home.get("id").or_else(|| home.get("home_id")))
                    .ok_or("cached home record did not contain id")?;
                let rooms = parse_rooms(home);
                if let Some(existing) = homes
                    .iter_mut()
                    .find(|existing: &&mut Home| existing.id == id)
                {
                    merge_rooms(&mut existing.rooms, rooms);
                } else {
                    homes.push(Home {
                        id,
                        name: string_field(home, &["name", "home_name"]),
                        rooms,
                    });
                }
            }
        }
    }
    Ok(homes)
}

fn merge_rooms(existing: &mut Vec<Room>, rooms: Vec<Room>) {
    for room in rooms {
        if let Some(current) = existing.iter_mut().find(|current| current.id == room.id) {
            current.dids.extend(room.dids);
        } else {
            existing.push(room);
        }
    }
}

fn parse_rooms(home: &Value) -> Vec<Room> {
    home.get("roomlist")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|room| {
            let id = value_as_string(room.get("id").or_else(|| room.get("room_id")))?;
            let dids = room
                .get("dids")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|did| value_as_string(Some(did)))
                .collect();
            Some(Room {
                id,
                name: string_field(room, &["name", "room_name"]),
                dids,
            })
        })
        .collect()
}

fn string_field(value: &Value, keys: &[&str]) -> String {
    keys.iter()
        .find_map(|key| value.get(key).and_then(Value::as_str))
        .unwrap_or("")
        .to_owned()
}

fn value_as_string(value: Option<&Value>) -> Option<String> {
    value?
        .as_str()
        .map(str::to_owned)
        .or_else(|| value?.as_i64().map(|number| number.to_string()))
        .or_else(|| value?.as_u64().map(|number| number.to_string()))
}

#[cfg(test)]
mod tests {
    use super::{Api, parse_homes};
    use serde_json::json;

    #[test]
    fn reads_home_rooms_and_device_membership() {
        let value = json!({ "home_info": { "homelist": [{ "id": 1, "name": "Home", "roomlist": [{ "id": "2", "name": "Living", "dids": ["a"] }] }] } });
        let homes = parse_homes(Api::Mihome, &value).unwrap();
        assert_eq!(homes[0].rooms[0].name, "Living");
        assert!(homes[0].rooms[0].dids.contains("a"));
    }
}
