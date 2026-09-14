# 0003：登录客户端设计

本文细化 [0001：miot-cli SDK 架构设计](0001-sdk-architecture.md) 的第一部分。登录不是 `MiotClient` 的职责：SDK 提供两个独立的登录客户端，调用者选择其中一个创建相应凭据，再由后续云端客户端使用。

## 边界与命名

| 结构体 | 参考实现 | 登录方式 | 产物 |
| --- | --- | --- | --- |
| `OAuthLoginClient` | `deps/ha_xiaomi_home` 的 `MIoTOauthClient` | OAuth 授权码 | `OAuthCredential` |
| `CloudLoginClient` | `deps/hass-xiaomi-miot` 的 `MiotCloud` | 小米账号与密码 | `CloudCredential` |

这两个 `struct` 没有共同的登录 trait、父类型或可互换的 `login` 返回值。它们仅可复用不包含协议语义的 HTTP client、时钟、随机数与 URL 编解码。`OAuthCredential` 与 `CloudCredential` 是不同的私有字段集合；一个凭据绝不能被当作另一个客户端的输入。

两个登录客户端只在内存中维护进行中的登录会话和挑战状态，成功时通过返回值交付 `OAuthCredential` 或 `CloudCredential`。它们不得写入文件、钥匙串、配置或任何其他持久化介质；本期 SDK 也不定义凭据存储抽象。`MiotClient` 不持有账号密码，不暴露登录方法，也不保存登录过程状态。其构造参数应明确要求调用方提供某一类已验证凭据；云端 API 在首期支持哪些凭据类型，由各 transport 的构造函数显式声明。

## OAuthLoginClient

`OAuthLoginClient` 实现 Xiaomi Home 所使用的 OAuth 授权码流程。它由 `client_id`、`redirect_url`、区域、稳定的本机设备标识及请求权限范围构造；客户端根据区域选择 OAuth 服务地址，并为本次授权生成不可预测的 `state`。`state` 的具体生成方式必须使用密码学安全随机数，不能照搬参考实现中从设备标识确定性推导的做法。

建议的公共接口如下，名称以最终 Rust API 为准：

```rust
pub struct OAuthLoginClient { /* OAuth-specific configuration and transient state */ }

impl OAuthLoginClient {
    pub fn authorization_url(&mut self, request: OAuthAuthorizationRequest)
        -> Result<Url, MiotError>;
    pub async fn complete_callback(&mut self, callback: Url)
        -> Result<OAuthCredential, MiotError>;
    pub async fn refresh(&self, credential: &OAuthCredential)
        -> Result<OAuthCredential, MiotError>;
}
```

调用 `authorization_url` 后，客户端仅在内存中保存本次授权的 `state`、回调地址和有效期。`complete_callback` 必须先验证回调中的 `state`，再读取授权码并向 OAuth token 端点交换 `access_token`、`refresh_token` 和过期信息。回调被消费、过期或校验失败后，该临时状态立即失效，不能再次交换。刷新使用 `refresh_token`，并以新的 `OAuthCredential` 返回给调用方；客户端不得替换任何外部存储中的旧凭据。

CLI 的 OAuth 模式优先启动临时 loopback callback；无法监听端口时，可以让用户粘贴完整回调 URL。浏览器授权 URL、授权码和回调 URL 都可能包含敏感信息，不能写入日志或 JSON 输出。

## CloudLoginClient

`CloudLoginClient` 实现 Xiaomi Miot 参考实现所使用的账号密码登录协议，而不是 OAuth 的替代入口。它由账号、密码、区域、目标服务标识和设备标识构造，并按该协议完成服务登录、密码认证及最终服务 token 获取。

```rust
pub struct CloudLoginClient { /* cloud-login-specific configuration and session */ }

impl CloudLoginClient {
    pub async fn login(&mut self, request: CloudLoginRequest)
        -> Result<CloudLoginOutcome, MiotError>;
    pub async fn continue_verification(&mut self, proof: VerificationProof)
        -> Result<CloudLoginOutcome, MiotError>;
    pub async fn submit_captcha(&mut self, captcha: SecretString)
        -> Result<CloudLoginOutcome, MiotError>;
}
```

正常路径遵循参考实现的三段会话流程：先请求服务登录上下文并取得协议所需字段，再提交账号和协议要求的密码摘要，最后跟随服务返回的位置完成会话并提取服务 token。实现细节（字段、摘要与签名算法、请求地址）封装在私有协议模块中，并以脱敏录制数据测试；它们不是稳定的公共 API。

账号密码登录可能要求二次验证或图形验证码。此时 `login` 不应把挑战伪装为普通网络错误，而应返回不含秘密的 `CloudLoginOutcome::VerificationRequired` 或 `CloudLoginOutcome::CaptchaRequired`：

- 验证结果只携带供用户打开的验证地址；验证票据通过 `VerificationProof` 的秘密字段提交；
- 验证码挑战可以提供内存中的图片字节或一次性 challenge 标识，不能在日志或持久化配置中保存图片、cookie 或原始响应；
- 挑战状态绑定到单个 `CloudLoginClient` 实例，完成、取消、超时或重试后必须清除。

成功结果包含 `CloudCredential`，其中包括后续目标云 API 所需的服务 token、用户标识和协议安全字段（例如 `ssecurity`）。这些字段均使用秘密容器，序列化、`Debug`、`Display` 与 `tracing` 字段必须脱敏。账号密码仅用于当前登录或明确的重新登录；登录客户端不得将账号密码或 `CloudCredential` 写入任何持久化介质。

## 生命周期、并发与错误

凭据只通过登录方法的返回值交付。SDK 不会保存、恢复或删除凭据；调用方若有持久化需求，必须在 SDK 外部自行处理返回结构体。OAuth 刷新以调用方的凭据实例为粒度 single-flight；Cloud 登录和挑战续办以 `CloudLoginClient` 实例为粒度串行，避免 cookie 与挑战交叉污染。

错误至少区分 `Authentication`、`Authorization`、`VerificationRequired`、`CaptchaRequired`、`Network`、`Timeout` 与 `Protocol`。错误消息可说明下一步操作，但不得包含账号、密码、cookie、授权码、refresh token、服务 token、`ssecurity`、完整回调 URL 或完整服务响应。

## CLI 映射与验收

`miot login oauth` 调用 `OAuthLoginClient`；`miot login cloud` 调用 `CloudLoginClient`。二者分别提示所需参数与挑战信息，并将成功结果交给 CLI 调用层；登录客户端本身不保存结果。CLI 是否持久化凭据不属于本期登录 SDK 的设计范围，且不得默认将秘密内容输出至终端或 JSON。

首期验收覆盖 OAuth 的授权 URL、state 校验、授权码交换、刷新与刷新并发；以及 Cloud 的成功三段登录、错误密码、二次验证、验证码、服务 token 缺失和挑战清理。所有 fixture 必须脱敏，且测试必须断言 `Debug`、日志和 CLI JSON 中不出现任何秘密字段。
