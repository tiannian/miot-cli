# 0001：miot-cli SDK 架构设计

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
│   ├── miot-rs/                  # 对外发布的异步 Rust SDK
│   └── miot/                     # clap CLI，依赖 miot-rs
├── docs/
│   ├── 0001-sdk-architecture.md
│   └── 0002-cli-delivery.md
├── examples/
│   ├── list_devices.rs
│   └── watch_property.rs
└── tests/
    └── fixtures/                 # 脱敏的 API、MQTT、Spec 录制数据
```

`miot-rs` 负责协议数据结构、认证、网络、路由、状态和公共 API。`miot` 不应绕过 SDK 调用任何协议层接口。若协议数据模型未来需要拆分，仍应作为 `miot-rs` 的内部模块，避免在首期扩大公共 crate 边界。

## 分层设计

```text
CLI / 用户应用
       │
miot-rs 公共 API：Client、DeviceService、PropertyService、ActionService
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

SDK package 名为 `miot-rs`，在 Rust 代码中以 `miot_rs` 导入。它以 `tokio` 为运行时，以 `Result<T, MiotError>` 返回可分类错误。长期运行的订阅使用 `Stream<Item = Result<DeviceEvent, MiotError>>`，调用者可自行决定重连、过滤与消费节奏。

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
