use std::{error::Error, time::Duration};

use clap::{Args, Subcommand};
use miot_rs::{Action, LanClient, Property};
use serde_json::Value;
use tracing::debug;

/// 直接通过局域网控制设备或网关下属子设备。
#[derive(Args)]
pub struct LanCommand {
    #[command(subcommand)]
    command: LanSubcommand,
}

#[derive(Subcommand)]
enum LanSubcommand {
    /// 调用任意 miIO 方法，参数为 JSON。
    Request(RequestArguments),
    /// 读取一个设备的 `MIoT` 属性。
    Get(GetArguments),
    /// 写入一个设备的 `MIoT` 属性。
    Set(SetArguments),
    /// 调用一个设备的 `MIoT` action。
    Action(ActionArguments),
}

#[derive(Args)]
struct ConnectionArguments {
    /// 网关或设备的 IP 地址；可附带 UDP 端口，默认端口为 54321。
    #[arg(long)]
    host: String,
    /// 网关或设备的 32 位十六进制 miIO token。
    #[arg(long)]
    token: String,
    /// 单次 UDP 请求超时秒数。
    #[arg(long, default_value_t = 5)]
    timeout: u64,
}

#[derive(Args)]
struct RequestArguments {
    #[command(flatten)]
    connection: ConnectionArguments,
    /// miIO 方法名，例如 `get_properties`。
    #[arg(long)]
    method: String,
    /// 方法参数的 JSON 值，例如 '[{"did":"123","siid":2,"piid":1}]'。
    #[arg(long, default_value = "[]")]
    params: String,
}

#[derive(Args)]
struct GetArguments {
    #[command(flatten)]
    connection: ConnectionArguments,
    /// 目标设备 DID。控制网关子设备时填写子设备 DID。
    #[arg(long)]
    did: String,
    /// 属性标识，格式为 SIID:PIID；可重复指定。
    #[arg(long, required = true)]
    property: Vec<String>,
}

#[derive(Args)]
struct SetArguments {
    #[command(flatten)]
    connection: ConnectionArguments,
    /// 目标设备 DID。控制网关子设备时填写子设备 DID。
    #[arg(long)]
    did: String,
    /// 属性和值，格式为 `SIID:PIID=JSON_VALUE`；可重复指定。
    #[arg(long, required = true)]
    value: Vec<String>,
}

#[derive(Args)]
struct ActionArguments {
    #[command(flatten)]
    connection: ConnectionArguments,
    /// 目标设备 DID。控制网关子设备时填写子设备 DID。
    #[arg(long)]
    did: String,
    /// 服务实例 ID。
    #[arg(long)]
    siid: u16,
    /// action 实例 ID。
    #[arg(long)]
    aiid: u16,
    /// action 输入参数的 JSON 数组。
    #[arg(long, default_value = "[]")]
    input: String,
}

impl LanCommand {
    pub async fn run(self) -> Result<(), Box<dyn Error>> {
        match self.command {
            LanSubcommand::Request(arguments) => request(arguments).await,
            LanSubcommand::Get(arguments) => get(arguments).await,
            LanSubcommand::Set(arguments) => set(arguments).await,
            LanSubcommand::Action(arguments) => action(arguments).await,
        }
    }
}

fn client(connection: &ConnectionArguments) -> Result<LanClient, Box<dyn Error>> {
    if connection.timeout == 0 {
        return Err("--timeout must be greater than zero".into());
    }
    Ok(LanClient::new(&connection.host, &connection.token)?
        .with_timeout(Duration::from_secs(connection.timeout)))
}

async fn request(arguments: RequestArguments) -> Result<(), Box<dyn Error>> {
    let params = parse_json(&arguments.params, "--params")?;
    debug!(method = %arguments.method, "sending local miIO request");
    print_json(
        &client(&arguments.connection)?
            .request(&arguments.method, params)
            .await?,
    )
}

async fn get(arguments: GetArguments) -> Result<(), Box<dyn Error>> {
    let properties = arguments
        .property
        .iter()
        .map(|property| parse_property(property))
        .collect::<Result<Vec<_>, _>>()?;
    debug!(did = %arguments.did, property_count = properties.len(), "reading local MIoT properties");
    let result = client(&arguments.connection)?
        .get_properties(&arguments.did, &properties)
        .await?;
    print_json(&Value::Array(result))
}

async fn set(arguments: SetArguments) -> Result<(), Box<dyn Error>> {
    let values = arguments
        .value
        .iter()
        .map(|value| parse_property_value(value))
        .collect::<Result<Vec<_>, _>>()?;
    debug!(did = %arguments.did, property_count = values.len(), "writing local MIoT properties");
    let result = client(&arguments.connection)?
        .set_properties(&arguments.did, &values)
        .await?;
    print_json(&Value::Array(result))
}

async fn action(arguments: ActionArguments) -> Result<(), Box<dyn Error>> {
    let input = parse_json(&arguments.input, "--input")?
        .as_array()
        .cloned()
        .ok_or("--input must be a JSON array")?;
    debug!(did = %arguments.did, siid = arguments.siid, aiid = arguments.aiid, "calling local MIoT action");
    print_json(
        &client(&arguments.connection)?
            .call_action(
                &arguments.did,
                &Action {
                    siid: arguments.siid,
                    aiid: arguments.aiid,
                    input,
                },
            )
            .await?,
    )
}

fn parse_property(value: &str) -> Result<Property, Box<dyn Error>> {
    let (siid, piid) = value
        .split_once(':')
        .ok_or("property must use the SIID:PIID format")?;
    Ok(Property {
        siid: siid
            .parse()
            .map_err(|_| "SIID must be an unsigned integer")?,
        piid: piid
            .parse()
            .map_err(|_| "PIID must be an unsigned integer")?,
    })
}

fn parse_property_value(value: &str) -> Result<(Property, Value), Box<dyn Error>> {
    let (property, value) = value
        .split_once('=')
        .ok_or("value must use the SIID:PIID=JSON_VALUE format")?;
    Ok((
        parse_property(property)?,
        parse_json(value, "property value")?,
    ))
}

fn parse_json(value: &str, argument: &str) -> Result<Value, Box<dyn Error>> {
    serde_json::from_str(value)
        .map_err(|error| format!("{argument} must be valid JSON: {error}").into())
}

fn print_json(value: &Value) -> Result<(), Box<dyn Error>> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{parse_property, parse_property_value};

    #[test]
    fn parses_property_identifiers() {
        assert_eq!(parse_property("2:1").unwrap().siid, 2);
        assert_eq!(parse_property("2:1").unwrap().piid, 1);
        assert!(parse_property("2").is_err());
    }

    #[test]
    fn parses_property_values_as_json() {
        let (property, value) = parse_property_value("2:1=true").unwrap();
        assert_eq!(property.siid, 2);
        assert_eq!(value, json!(true));
        assert!(parse_property_value("2:1=not-json").is_err());
    }
}
