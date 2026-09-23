use std::{error::Error, fs, net::IpAddr};

use clap::{Args, Subcommand};
use miot_rs::{MipsAction, MipsClient, MipsClientConfig, MipsEvent, MipsProperty, MipsTlsConfig};
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
    /// 列出当前中枢网关可代理的设备。
    ListDevice(MqttListDeviceArguments),
    /// 写入一个 `MIoT` 属性。
    Write(MqttWriteArguments),
    /// 持续监听属性变更通知。
    ListenNotify(MqttListenNotifyArguments),
    /// 持续监听事件通知。
    ListenEvent(MqttListenEventArguments),
    /// 执行一个 `MIoT` action。
    Action(MqttActionArguments),
}

#[derive(Args)]
struct MqttListDeviceArguments {
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

#[derive(Args)]
struct MqttWriteArguments {
    /// 设备 DID。
    did: String,
    /// `MIoT` service instance ID。
    siid: u16,
    /// `MIoT` property instance ID。
    piid: u16,
    /// 要写入的 JSON 值，例如 `true`、`42` 或 `"text"`。
    value: String,
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

#[derive(Args)]
struct MqttListenNotifyArguments {
    /// 设备 DID。
    did: String,
    /// 仅监听指定 service；省略时监听设备全部属性。
    #[arg(long)]
    siid: Option<u16>,
    /// 仅监听指定 property；需同时提供 `--siid`。
    #[arg(long)]
    piid: Option<u16>,
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

#[derive(Args)]
struct MqttListenEventArguments {
    /// 设备 DID。
    did: String,
    /// 仅监听指定 service；省略时监听设备全部事件。
    #[arg(long)]
    siid: Option<u16>,
    /// 仅监听指定 event；需同时提供 `--siid`。
    #[arg(long)]
    eiid: Option<u16>,
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
            MqttSubcommand::ListDevice(arguments) => list_device(arguments).await,
            MqttSubcommand::Write(arguments) => write(arguments).await,
            MqttSubcommand::ListenNotify(arguments) => listen_notify(arguments).await,
            MqttSubcommand::ListenEvent(arguments) => listen_event(arguments).await,
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

async fn list_device(arguments: MqttListDeviceArguments) -> Result<(), Box<dyn Error>> {
    let client = connect(
        arguments.account.as_deref(),
        arguments.ip,
        arguments.port,
        &arguments.region,
    )
    .await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&client.get_device_list().await?)?
    );
    Ok(())
}

async fn write(arguments: MqttWriteArguments) -> Result<(), Box<dyn Error>> {
    let value: Value = parse_json(&arguments.value, "property value")?;
    let client = connect(
        arguments.account.as_deref(),
        arguments.ip,
        arguments.port,
        &arguments.region,
    )
    .await?;
    let result = client
        .set_property(&arguments.did, arguments.siid, arguments.piid, value)
        .await?;
    println!("{result}");
    Ok(())
}

async fn listen_notify(arguments: MqttListenNotifyArguments) -> Result<(), Box<dyn Error>> {
    if arguments.piid.is_some() && arguments.siid.is_none() {
        return Err("--piid requires --siid".into());
    }
    let client = connect(
        arguments.account.as_deref(),
        arguments.ip,
        arguments.port,
        &arguments.region,
    )
    .await?;
    let receiver = client
        .subscribe_properties(&arguments.did, arguments.siid, arguments.piid)
        .await?;
    print_notifications(receiver).await
}

async fn listen_event(arguments: MqttListenEventArguments) -> Result<(), Box<dyn Error>> {
    if arguments.eiid.is_some() && arguments.siid.is_none() {
        return Err("--eiid requires --siid".into());
    }
    let client = connect(
        arguments.account.as_deref(),
        arguments.ip,
        arguments.port,
        &arguments.region,
    )
    .await?;
    let receiver = client
        .subscribe_events(&arguments.did, arguments.siid, arguments.eiid)
        .await?;
    print_events(receiver).await
}

async fn print_notifications(
    mut receiver: tokio::sync::mpsc::Receiver<MipsProperty>,
) -> Result<(), Box<dyn Error>> {
    loop {
        tokio::select! {
            notification = receiver.recv() => match notification {
                Some(notification) => println!("{}", serde_json::to_string(&notification)?),
                None => return Err("MIPS property subscription closed".into()),
            },
            result = tokio::signal::ctrl_c() => {
                result?;
                return Ok(());
            }
        }
    }
}

async fn print_events(
    mut receiver: tokio::sync::mpsc::Receiver<MipsEvent>,
) -> Result<(), Box<dyn Error>> {
    loop {
        tokio::select! {
            event = receiver.recv() => match event {
                Some(event) => println!("{}", serde_json::to_string(&event)?),
                None => return Err("MIPS event subscription closed".into()),
            },
            result = tokio::signal::ctrl_c() => {
                result?;
                return Ok(());
            }
        }
    }
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
