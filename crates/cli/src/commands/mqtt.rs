use std::{error::Error, fs, net::IpAddr};

use clap::{Args, Subcommand};
use miot_rs::{MipsClient, MipsClientConfig, MipsTlsConfig};
use serde::Deserialize;

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

#[derive(Deserialize)]
struct MqttIdentity {
    virtual_did: String,
}

impl MqttCommand {
    pub async fn run(self) -> Result<(), Box<dyn Error>> {
        match self.command {
            MqttSubcommand::Get(arguments) => get(arguments).await,
        }
    }
}

async fn get(arguments: MqttGetArguments) -> Result<(), Box<dyn Error>> {
    let (account_id, _) = load_oauth_credential(arguments.account.as_deref())?;
    let directory = mqtt_certificate_directory()?
        .join(filename_component(&account_id))
        .join(filename_component(&arguments.region));
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
    let client = MipsClient::connect(
        MipsClientConfig::new(identity.virtual_did, arguments.ip.to_string(), tls)
            .with_port(arguments.port),
    )
    .await?;
    let value = client
        .get_property(&arguments.did, arguments.siid, arguments.piid)
        .await?;
    println!("{value}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::MqttIdentity;

    #[test]
    fn reads_the_saved_virtual_did() {
        let identity: MqttIdentity = toml::from_str("virtual_did = \"12345\"").unwrap();
        assert_eq!(identity.virtual_did, "12345");
    }
}
