# miot-cli 架构设计

## 目标

本项目以 Rust 实现一套面向 MIoT 设备的异步 SDK，并提供基于该 SDK 的命令行工具。首期能力包括：

- OAuth 登录、凭据保存和自动刷新；
- 列出设备、查询单个设备及其 MIoT Spec；
- 读取和修改属性；
- 监听属性变更及设备事件；
- 执行设备 Action；
- 统一云端、本地 LAN 和中央网关控制路径。

CLI 是 SDK 的薄适配层：所有业务逻辑、传输选择和数据模型均归属 SDK，使应用程序可直接嵌入 SDK，而无需执行子进程。

## 非目标

- 不在首期将 MIoT 功能映射为灯、空调、传感器等高层设备类别；SDK 以原始 `did`、`siid`、`piid`、`aiid` 为稳定接口。
- 不在 SDK 内实现图形界面或设备自动化规则引擎。
- 不承诺所有设备均同时支持云端、LAN 与中央网关控制。

## Workspace 布局

```text
miot-cli/
├── Cargo.toml
├── crates/
│   ├── miot-sdk/                 # 对外发布的异步 Rust SDK
│   ├── miot-protocol/            # MIoT DTO、协议常量、编解码
│   └── miot-cli/                 # clap CLI，依赖 miot-sdk
├── docs/
│   └── architecture.md
├── examples/
│   ├── list_devices.rs
│   └── watch_property.rs
└── tests/
    └── fixtures/                 # 脱敏的 API、MQTT、Spec 录制数据
```

`miot-protocol` 不暴露网络客户端，仅负责可序列化的数据结构、请求/响应格式和验证规则。`miot-sdk` 负责认证、网络、路由、状态和公共 API。`miot-cli` 不应绕过 SDK 调用任何协议层接口。

## 分层设计

```text
CLI / 用户应用
       │
miot-sdk 公共 API：Client、DeviceService、PropertyService、ActionService
       │
路由层：Auto / CloudOnly / LanOnly / GatewayOnly
       │
Cloud HTTP + Cloud MQTT | LAN 控制 | Central Hub MQTT
       │
MIoT 设备与云服务
```

### 公共 SDK

核心入口是 `MiotClient`。它拥有认证状态、设备目录、传输实例及后台订阅任务；通过服务对象暴露能力：

```rust
let client = MiotClient::builder()
    .credentials(credentials)
    .transport_preference(TransportPreference::Auto)
    .build()
    .await?;

let devices = client.devices().list().await?;
let device = client.devices().get(&did).await?;
let value = client.properties().get(&did, PropertyId::new(2, 1)).await?;
client.properties().set(&did, PropertyId::new(2, 1), json!(true)).await?;
let output = client.actions()
    .invoke(&did, ActionId::new(2, 1), vec![])
    .await?;
```

SDK 以 `tokio` 为运行时，以 `Result<T, MiotError>` 返回可分类错误。长期运行的订阅使用 `Stream<Item = Result<DeviceEvent, MiotError>>`，调用者可自行决定重连、过滤与消费节奏。

### 认证与凭据

认证模块分为无状态 OAuth 流程和有状态凭据管理：

- `LoginSession::begin` 生成授权 URL、state、PKCE verifier；
- `LoginSession::complete` 校验回调并交换 token；
- `TokenManager` 在请求前检查有效期，并串行化刷新，避免并发刷新同一 refresh token；
- `CredentialStore` 是持久化抽象，默认实现使用操作系统钥匙串；文件实现仅在用户显式指定时启用。

CLI 的 `login` 命令优先启动临时 loopback callback；无法监听端口时允许用户粘贴最终回调 URL。token、refresh token、设备证书和私钥绝不输出至终端、调试日志或 JSON 结果。

### 设备目录与 Spec

`DeviceService` 负责获取和缓存设备清单，返回统一的 `Device`：

```rust
pub struct Device {
    pub did: DeviceId,
    pub name: String,
    pub model: String,
    pub home_id: Option<String>,
    pub room_id: Option<String>,
    pub online: bool,
    pub routes: DeviceRoutes,
    pub spec_type: Option<String>,
}
```

Spec 通过 `SpecService` 获取并按 `spec_type` 和版本缓存。SDK 既保留原始 Spec JSON，也提供类型化的 `Service`、`Property`、`Action` 和 `Event` 模型。属性写入及 Action 调用可根据 Spec 在本地预校验可写性、值范围、枚举和参数个数。

### 控制与路由

`Transport` 是内部统一接口，提供 `get_property`、`set_property`、`invoke_action` 与 `subscribe`。每种 transport 返回标准化的 `TransportResult` 和错误类型。

`TransportRouter` 根据策略及设备可达性选择路径：

| 策略 | 行为 |
| --- | --- |
| `Auto` | 写入和 Action 按 Gateway → LAN → Cloud 尝试；读取优先 Cloud 缓存，失败后回退本地。 |
| `CloudOnly` | 只允许云端 HTTP/MQTT。 |
| `LanOnly` | 只允许同一局域网的可发现设备。 |
| `GatewayOnly` | 只使用中央网关的 MQTT 路径。 |

路由决策和最终使用的 transport 会包含在调试元数据中，但默认命令输出保持简洁。对于非幂等 Action，路由器不得在已发送后自动跨 transport 重试，避免动作执行两次。

### 订阅

订阅层将云端 MQTT、中央网关 MQTT 和 LAN 通知归一为以下事件：

```rust
pub enum DeviceEvent {
    PropertyChanged { did: DeviceId, property: PropertyId, value: Value, timestamp: DateTime<Utc> },
    EventOccurred { did: DeviceId, event: EventId, arguments: Vec<Value>, timestamp: DateTime<Utc> },
    DeviceOnlineChanged { did: DeviceId, online: bool, timestamp: DateTime<Utc> },
}
```

订阅器需要维护重连退避、认证刷新后的重鉴权、重复消息去重和关闭语义。公共 API 不暴露 MQTT topic，以便后续协议调整不破坏 SDK 兼容性。

## CLI 设计

```text
miot login [--region <region>]
miot logout
miot devices list [--home <id>]
miot devices get <did>
miot spec get <did>
miot props get <did> <siid> <piid>
miot props set <did> <siid> <piid> --value <json>
miot props watch <did> [--siid <n> --piid <n>]
miot actions invoke <did> <siid> <aiid> [--input <json-array>]
```

所有读取类命令支持 `--format table|json|yaml`。`watch` 默认输出 NDJSON，每一行一个 `DeviceEvent`，以便 `jq`、日志采集和 shell 管道消费。写入和 Action 默认要求明确参数；可增加 `--yes` 供自动化场景跳过交互确认。

退出码应稳定：`0` 成功，`2` 参数或本地校验错误，`3` 认证错误，`4` 设备不可达，`5` 远端或协议错误。

## 错误、日志与安全

错误采用可匹配的 `MiotError`，至少区分 `Authentication`、`Authorization`、`Network`、`Timeout`、`DeviceNotFound`、`DeviceOffline`、`Validation`、`Remote`、`TransportUnavailable` 与 `Protocol`。CLI 将错误翻译为可读文本，JSON 模式同时输出稳定的机器可读 error code。

使用 `tracing` 记录结构化日志。所有凭据、Authorization header、回调 code、证书和私钥都必须通过 redaction 过滤。默认日志级别为 `warn`；`--verbose` 提升至 `info`，`--debug` 仍不得显示敏感字段。

## 依赖建议

- 异步与流：`tokio`、`futures-util`、`tokio-stream`
- HTTP：`reqwest`
- 序列化：`serde`、`serde_json`
- 错误：`thiserror`
- CLI：`clap`、`clap_complete`
- 日志：`tracing`、`tracing-subscriber`
- 秘密数据：`secrecy`、`keyring`
- MQTT 与 LAN 发现：作为可选 Cargo feature，避免纯云端客户端携带额外依赖。

## 测试策略

- `miot-protocol`：请求、响应、签名和 Spec 解析的单元测试；
- `miot-sdk`：以 mock HTTP/MQTT server 覆盖 token 刷新、路由回退、重连和错误映射；
- fixtures：只提交脱敏录制数据，禁止提交真实 token、did、证书、家庭或网络信息；
- CLI：用快照测试验证 table/JSON/NDJSON 输出和退出码；
- 集成测试：通过环境变量注入专用测试账户，默认不运行并标注为 `#[ignore]`。

## 交付阶段

1. 建立 workspace、数据模型、错误体系和 CLI 输出规范。
2. 实现 OAuth、凭据存储、云端设备列表与 Spec 查询。
3. 实现云端属性读写、Action 和云端订阅，形成可用 MVP。
4. 加入 LAN transport、设备发现与 `LanOnly` 路由。
5. 加入中央网关 MQTT、证书生命周期与 `GatewayOnly` 路由。
6. 基于真实设备兼容性数据补充设备能力探测、重试策略和文档。

每个阶段都应保持 SDK 可独立使用、CLI 仅调用公共 SDK API，并在发布前通过 `cargo fmt`、`cargo clippy --all-targets --all-features` 和测试套件。
