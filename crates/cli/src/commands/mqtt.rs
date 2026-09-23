use std::{error::Error, fs, net::IpAddr};

use clap::{Args, Subcommand};
use miot_rs::{MipsAction, MipsClient, MipsClientConfig, MipsTlsConfig};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::Value;

use crate::{
    commands::update_cert::ensure_ca_certificate,
    credentials::{filename_component, load_oauth_credential, mqtt_certificate_directory},
};

/// 通过指定 IP 访问中枢网关 MIPS MQTT 服务。
#[derive(Args)]
pub struct MqttCommand {
    #[command(subcommand)]
    command: MqttSubcommand,
}

#[derive(Subcommand)]
enum MqttSubcommand {
    /// 读取一个 `MIoT` 属性。
    Get(MqttGetArguments),
    /// 执行一个 `MIoT` action。
    Action(MqttActionArguments),
}

#[derive(Args)]
struct MqttGetArguments {
    /// 设备 DID。
    did: String,
    /// `MIoT` service instance ID。
    siid: u16,
    /// `MIoT` property instance ID。
    piid: u16,
    /// `miot auth login --account` 使用的账号标识。
    #[arg(long)]
    account: Option<String>,
    /// 中枢网关 MIPS MQTT 服务的 IPv4 或 IPv6 地址；不会执行 mDNS 发现。
    #[arg(long)]
    ip: IpAddr,
    /// 中枢网关 MIPS MQTT 端口。
    #[arg(long, default_value_t = 8883)]
    port: u16,
    /// 证书所属的小米云区域。
    #[arg(long, default_value = "cn")]
    region: String,
}

#[derive(Args)]
struct MqttActionArguments {
    /// 设备 DID。
    did: String,
    /// `MIoT` service instance ID。
    siid: u16,
    /// `MIoT` action instance ID。
    aiid: u16,
    /// action 的输入参数 JSON 数组；默认是空数组。
    #[arg(long, default_value = "[]")]
    input: String,
    /// `miot auth login --account` 使用的账号标识。
    #[arg(long)]
    account: Option<String>,
    /// 中枢网关 `MIPS MQTT` 服务的 IPv4 或 IPv6 地址；不会执行 mDNS 发现。
    #[arg(long)]
    ip: IpAddr,
    /// 中枢网关 `MIPS MQTT` 端口。
    #[arg(long, default_value_t = 8883)]
    port: u16,
    /// 证书所属的小米云区域。
    #[arg(long, default_value = "cn")]
    region: String,
}

#[derive(Deserialize)]
struct MqttIdentity {
    virtual_did: String,
}

impl MqttCommand {
    pub async fn run(self) -> Result<(), Box<dyn Error>> {
        match self.command {
            MqttSubcommand::Get(arguments) => get(arguments).await,
            MqttSubcommand::Action(arguments) => action(arguments).await,
        }
    }
}

async fn get(arguments: MqttGetArguments) -> Result<(), Box<dyn Error>> {
    let client = connect(
        arguments.account.as_deref(),
        arguments.ip,
        arguments.port,
        &arguments.region,
    )
    .await?;
    let value = client
        .get_property(&arguments.did, arguments.siid, arguments.piid)
        .await?;
    println!("{value}");
    Ok(())
}

async fn action(arguments: MqttActionArguments) -> Result<(), Box<dyn Error>> {
    let input: Vec<Value> = parse_json(&arguments.input, "action input")?;
    let client = connect(
        arguments.account.as_deref(),
        arguments.ip,
        arguments.port,
        &arguments.region,
    )
    .await?;
    let output = client
        .execute_action(
            &arguments.did,
            &MipsAction {
                siid: arguments.siid,
                aiid: arguments.aiid,
                input,
            },
        )
        .await?;
    println!("{}", Value::Array(output));
    Ok(())
}

async fn connect(
    account: Option<&str>,
    ip: IpAddr,
    port: u16,
    region: &str,
) -> Result<MipsClient, Box<dyn Error>> {
    let (account_id, _) = load_oauth_credential(account)?;
    let directory = mqtt_certificate_directory()?
        .join(filename_component(&account_id))
        .join(filename_component(region));
    let identity: MqttIdentity =
        toml::from_str(&fs::read_to_string(directory.join("identity.toml"))?)?;
    if identity.virtual_did.trim().is_empty() {
        return Err("saved MQTT identity did not contain a virtual DID".into());
    }
    let tls = MipsTlsConfig {
        ca_certificate: ensure_ca_certificate(&directory)?,
        client_certificate: directory.join("client.cert"),
        private_key: directory.join("client.key"),
    };
    Ok(MipsClient::connect(
        MipsClientConfig::new(identity.virtual_did, ip.to_string(), tls).with_port(port),
    )
    .await?)
}

fn parse_json<T: DeserializeOwned>(value: &str, name: &'static str) -> Result<T, Box<dyn Error>> {
    serde_json::from_str(value).map_err(|error| format!("invalid {name} JSON: {error}").into())
}

#[cfg(test)]
mod tests {
    use super::{MqttIdentity, parse_json};
    use serde_json::Value;

    #[test]
    fn reads_the_saved_virtual_did() {
        let identity: MqttIdentity = toml::from_str("virtual_did = \"12345\"").unwrap();
        assert_eq!(identity.virtual_did, "12345");
    }

    #[test]
    fn parses_action_input_as_an_array() {
        let value: Vec<Value> = parse_json("[true, 42]", "action input").unwrap();
        assert_eq!(value, vec![Value::Bool(true), Value::from(42)]);
    }
}
