# miot-cli

`miot-cli` 是一个面向 MIoT 设备的 Rust 命令行工具及异步 SDK。项目计划提供认证、设备发现、属性读写、设备状态订阅与 MIoT Action 调用等能力。

当前已初始化 Rust workspace，包含两个 crate：

- `crates/miot-rs`：可嵌入应用的异步 SDK；Rust 中通过 `miot_rs` 导入；
- `crates/cli`：包和二进制名均为 `miot` 的命令行工具，只依赖 `miot-rs` 的公共 API。

> 项目当前处于引导阶段。以下命令与 API 是计划中的公共接口，尚未全部实现。

## 计划能力

- OAuth login, automatic token refresh, and secure credential storage;
- Home and device discovery, device details, and MIoT Spec lookup;
- MIoT property reads and writes;
- Property-change, device-event, and availability subscriptions;
- MIoT action invocation;
- Unified routing across cloud, local LAN, and central-hub transports.

## CLI 示例

```bash
# 使用账号密码登录；密码含冒号时，只有第一个冒号作为分隔符
miot auth login --region cn --userpass 'USERNAME:PASSWORD'

# 避免密码出现在命令参数和 shell 历史记录中
printf '%s\n%s\n' 'USERNAME' 'PASSWORD' | miot auth login --region cn --userpass-stdin

# 更新家庭状态及每个家庭下的设备列表
miot update --miio --account <ACCOUNT_ID> --region cn
miot update --mihome --account <ACCOUNT_ID> --region cn

# List devices
miot devices list

# Inspect a device and its capabilities
miot devices get <did>
miot spec get <did>

# Read or write a property
miot props get <did> <siid> <piid>
miot props set <did> <siid> <piid> --value true

# Stream property updates as NDJSON
miot props watch <did> --siid 2 --piid 1

# Invoke a device action
miot actions invoke <did> <siid> <aiid> --input '["hello", true]'
```

## 账号密码登录验证

`--userpass` 按照 `hass-xiaomi-miot` 的小米账号登录协议请求服务。未传入 `--device-id` 时，CLI 会生成与该参考实现相同格式的 16 位大写字母和数字 client ID，并将它和凭据一同保存。若小米要求图形验证码，CLI 会将图片写入系统临时目录并提示路径；输入验证码后临时图片会删除。若要求账号二次验证，先在终端显示的 URL 中完成验证，再将页面给出的 verification ticket 粘贴回终端。

不要将含密码的命令写入 shell 历史记录或共享的脚本。建议使用 `--userpass-stdin`，从标准输入依次读取账号和密码两行。登录成功后，凭据保存在 `~/.local/miot.rs/accounts/`。

执行 `miot update` 时必须选择一种 API。`--miio` 使用原有 MiIO 接口，并将家庭聚合状态写入 `~/.local/miot.rs/miio/<account>/homes.json`，将每个家庭的完整设备列表写入 `~/.local/miot.rs/miio/<account>/devices/<home-id>.json`。`--mihome` 使用 Xiaomi Home 接口，枚举自有家庭、共享家庭和独立分享设备；原始家庭及房间分页信息写入 `~/.local/miot.rs/mihome/<account>/homes.json`，全部设备详情写入 `devices.json`，每个家庭或独立分享者的设备详情写入 `devices/` 目录。`<account>` 使用已保存账号的文件名形式，且会将不适合作为路径的字符替换为下划线。有多个已保存账号时必须使用 `--account` 指定账号；只有一个账号时可省略该参数。

Read-oriented commands will support `--format table|json|yaml`. Watch commands will emit NDJSON by default, making them easy to consume through `jq`, log collectors, and shell pipelines.

## SDK 示例

```rust,no_run
use miot_rs::{MiotClient, PropertyId, TransportPreference};
use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = MiotClient::builder()
        .credentials_from_default_store()
        .transport_preference(TransportPreference::Auto)
        .build()
        .await?;

    for device in client.devices().list().await? {
        println!("{}: {}", device.did, device.name);
    }

    let did = "device-id";
    let property = PropertyId::new(2, 1);
    let value = client.properties().get(did, property).await?;
    println!("value: {value}");

    client.properties().set(did, property, json!(true)).await?;
    Ok(())
}
```

## 架构

仓库是 Rust workspace。SDK 承载实现；CLI 只消费 SDK 的公共 API。

```text
CLI / Rust application
       │
miot-rs
       │
Transport router: Cloud / LAN / Gateway
       │
MIoT services and devices
```

模块边界、认证流程、路由策略、数据模型、错误处理与交付计划见设计文档：

- [0001 SDK 架构设计](docs/0001-sdk-architecture.md)
- [0002 CLI 与交付设计](docs/0002-cli-delivery.md)

## 当前目录结构

```text
.
├── crates/
│   ├── miot-rs/        # 异步 Rust SDK
│   └── cli/            # miot CLI
├── docs/
│   ├── 0001-sdk-architecture.md
│   └── 0002-cli-delivery.md
├── examples/
└── tests/
```

## 交付计划

1. 建立 workspace、公共数据模型与错误体系。
2. 实现 OAuth、凭据处理、云端设备发现与 Spec 查询。
3. 实现云端属性访问、Action 和订阅，形成 MVP。
4. 逐步加入 LAN 与中央网关 transport。

## 开发检查

提交前运行：

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
```
