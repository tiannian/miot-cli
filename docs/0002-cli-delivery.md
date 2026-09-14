# 0002：miot-cli CLI 与交付设计

本文承接 [0001：miot-cli SDK 架构设计](0001-sdk-architecture.md)，定义命令行界面、工程质量约束和分阶段交付计划。

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

- `miot-rs`：请求、响应、签名和 Spec 解析的单元测试，并以 mock HTTP/MQTT server 覆盖 token 刷新、路由回退、重连和错误映射；
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
