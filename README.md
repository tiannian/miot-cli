# miot-cli

`miot-cli` 是一个使用 Rust 编写的 MIoT 命令行工具与异步 SDK。它提供统一的设备访问接口，用于登录、发现设备、读取和修改属性、监听设备状态，以及执行 MIoT Action。

项目由两部分组成：

- `miot-sdk`：供 Rust 应用嵌入使用的异步 SDK；
- `miot`：基于 SDK 的命令行工具，适用于终端、脚本和自动化流程。

> 项目目前处于架构与初始化阶段，以下命令和 API 是目标接口，尚未全部实现。

## 计划功能

- OAuth 登录、token 自动刷新与安全凭据保存；
- 查询家庭及设备列表、设备详情和 MIoT Spec；
- 读取、写入 MIoT 属性；
- 监听属性变更、设备事件与在线状态；
- 执行 MIoT Action；
- 在云端、局域网和中央网关控制方式之间统一路由。

## CLI 使用示例

```bash
# 登录
miot login --region cn

# 列出设备
miot devices list

# 获取设备详情和能力描述
miot devices get <did>
miot spec get <did>

# 读取或修改属性
miot props get <did> <siid> <piid>
miot props set <did> <siid> <piid> --value true

# 持续监听属性变化（NDJSON 输出）
miot props watch <did> --siid 2 --piid 1

# 执行设备动作
miot actions invoke <did> <siid> <aiid> --input '["hello", true]'
```

读取类命令将支持 `--format table|json|yaml`；监听命令默认输出 NDJSON，方便通过 `jq` 或 shell 管道处理。

## SDK 使用示例

```rust,no_run
use miot_sdk::{MiotClient, PropertyId, TransportPreference};
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

项目采用 Rust workspace，SDK 是核心，CLI 仅封装 SDK 的公共 API：

```text
CLI / Rust 应用
       │
miot-sdk
       │
传输路由：Cloud / LAN / Gateway
       │
MIoT 服务与设备
```

完整的模块划分、认证流程、路由策略、数据模型、错误处理与实施路线见[架构设计](docs/architecture.md)。

## 规划目录

```text
.
├── crates/
│   ├── miot-sdk/       # 异步 Rust SDK
│   ├── miot-protocol/  # 协议数据结构和编解码
│   └── miot-cli/       # CLI
├── docs/
│   └── architecture.md
├── examples/
└── tests/
```

## 开发计划

1. 初始化 workspace、公共数据模型和错误体系；
2. 完成 OAuth、凭据保存、云端设备与 Spec 查询；
3. 完成云端属性读写、Action 和订阅，形成 MVP；
4. 逐步增加 LAN 和中央网关 transport。

## 开发规范

实现开始后，提交前应运行：

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
```
