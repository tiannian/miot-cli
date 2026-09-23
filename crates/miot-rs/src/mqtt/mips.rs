//! 小米中枢网关 MIPS 协议客户端。

use std::{
    collections::HashMap,
    io::{BufReader, Cursor},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use rumqttc::v5::{
    AsyncClient, Event, EventLoop, MqttOptions,
    mqttbytes::{QoS, v5::Packet},
};
use rumqttc::{TlsConfiguration, Transport};
use rustls::{
    CertificateError, ClientConfig, DigitallySignedStruct, Error as RustlsError, RootCertStore,
    SignatureScheme,
    client::{
        WebPkiServerVerifier,
        danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    },
    pki_types::{CertificateDer, ServerName, UnixTime},
};
use serde_json::{Value, json};
use tokio::{
    sync::{mpsc, oneshot},
    time::timeout,
};

use crate::MiotError;

const DEFAULT_PORT: u16 = 8883;
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);
const COMMAND_QUEUE_CAPACITY: usize = 32;
const MQTT_REQUEST_QUEUE_CAPACITY: usize = 32;

/// MIPS broker 所用双向 TLS 凭据的路径。
#[derive(Clone, Debug)]
pub struct MipsTlsConfig {
    pub ca_certificate: PathBuf,
    pub client_certificate: PathBuf,
    pub private_key: PathBuf,
}

/// [`MipsClient`] 的连接配置。
#[derive(Clone, Debug)]
pub struct MipsClientConfig {
    /// 小米家庭账号中登记的虚拟 DID。
    pub client_id: String,
    /// 中枢网关 MIPS 服务公布的 IP 地址或 DNS 名称。
    pub host: String,
    pub port: u16,
    pub tls: MipsTlsConfig,
    pub request_timeout: Duration,
}

impl MipsClientConfig {
    /// 使用默认 MIPS TLS 端口 8883 创建配置。
    #[must_use]
    pub fn new(client_id: impl Into<String>, host: impl Into<String>, tls: MipsTlsConfig) -> Self {
        Self {
            client_id: client_id.into(),
            host: host.into(),
            port: DEFAULT_PORT,
            tls,
            request_timeout: DEFAULT_TIMEOUT,
        }
    }

    /// 设置其他网关端口。
    #[must_use]
    pub fn with_port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }

    /// 设置 MIPS 请求及初始连接使用的超时。
    #[must_use]
    pub fn with_request_timeout(mut self, request_timeout: Duration) -> Self {
        self.request_timeout = request_timeout;
        self
    }
}

/// 一个 MIPS action 及其 MIoT-Spec 输入值。
#[derive(Clone, Debug)]
pub struct MipsAction {
    pub siid: u16,
    pub aiid: u16,
    pub input: Vec<Value>,
}

/// 中枢网关发布的属性变更通知。
#[derive(Clone, Debug, PartialEq)]
pub struct MipsProperty {
    pub did: String,
    pub siid: u16,
    pub piid: u16,
    pub value: Value,
}

/// 中枢网关发布的事件通知。
#[derive(Clone, Debug, PartialEq)]
pub struct MipsEvent {
    pub did: String,
    pub siid: u16,
    pub eiid: u16,
    pub arguments: Vec<Value>,
}

/// 一个面向中枢网关的异步双向 TLS MIPS 连接。
///
/// 调用方负责签发证书，并通过 [`MipsTlsConfig`] 提供已保存的 PEM 文件。
/// 此客户端负责 MQTT 连接和重连；只要该实例仍被持有，后台任务就会继续运行。
#[derive(Clone, Debug)]
pub struct MipsClient {
    command_tx: mpsc::Sender<Command>,
    request_timeout: Duration,
}

impl MipsClient {
    /// 通过 MQTT v5 和双向 TLS 连接网关。
    ///
    /// # Errors
    ///
    /// 无法读取 TLS 文件、配置无效、MQTT 无法建立连接或连接超时时返回错误。
    pub async fn connect(config: MipsClientConfig) -> Result<Self, MiotError> {
        validate_config(&config)?;
        let ca = read_pem(&config.tls.ca_certificate, "CA certificate")?;
        let certificate = read_pem(&config.tls.client_certificate, "client certificate")?;
        let key = read_pem(&config.tls.private_key, "private key")?;

        let mut options = MqttOptions::new(&config.client_id, &config.host, config.port);
        options.set_keep_alive(Duration::from_secs(30));
        options.set_clean_start(true);
        options.set_transport(Transport::tls_with_config(TlsConfiguration::Rustls(
            tls_config(ca, certificate, key)?,
        )));

        let (mqtt, event_loop) = AsyncClient::new(options, MQTT_REQUEST_QUEUE_CAPACITY);
        let (command_tx, command_rx) = mpsc::channel(COMMAND_QUEUE_CAPACITY);
        let (connected_tx, connected_rx) = oneshot::channel();
        tokio::spawn(run_connection(
            mqtt,
            event_loop,
            command_rx,
            config.client_id,
            connected_tx,
        ));

        match timeout(config.request_timeout, connected_rx).await {
            Ok(Ok(Ok(()))) => Ok(Self {
                command_tx,
                request_timeout: config.request_timeout,
            }),
            Ok(Ok(Err(error))) => Err(error),
            Ok(Err(_)) => Err(MiotError::Mqtt(
                "connection task stopped before CONNACK".to_owned(),
            )),
            Err(_) => Err(MiotError::Mqtt("MQTT connection timed out".to_owned())),
        }
    }

    /// 通过网关的 `proxy/get` 端点读取一个属性。
    ///
    /// # Errors
    ///
    /// 请求无法发送、超时或网关返回无效响应时返回错误。
    pub async fn get_property(&self, did: &str, siid: u16, piid: u16) -> Result<Value, MiotError> {
        validate_did(did)?;
        let response = self
            .request("proxy/get", json!({"did": did, "siid": siid, "piid": piid}))
            .await?;
        response.get("value").cloned().ok_or(MiotError::Protocol(
            "MIPS get response did not include value",
        ))
    }

    /// 读取当前中枢网关可代理的设备清单。
    ///
    /// # Errors
    ///
    /// 请求无法发送、超时或网关返回无效设备清单时返回错误。
    pub async fn get_device_list(&self) -> Result<Value, MiotError> {
        let response = self.request("proxy/getDevList", json!({})).await?;
        response
            .get("devList")
            .filter(|value| value.is_object())
            .cloned()
            .ok_or(MiotError::Protocol(
                "MIPS device list response did not include devList",
            ))
    }

    /// 通过网关的 `set_properties` RPC 写入一个属性。
    ///
    /// # Errors
    ///
    /// 请求无法发送、超时或网关拒绝属性值时返回错误。
    pub async fn set_property(
        &self,
        did: &str,
        siid: u16,
        piid: u16,
        value: Value,
    ) -> Result<Value, MiotError> {
        validate_did(did)?;
        let response = self
            .request(
                "proxy/rpcReq",
                json!({"did": did, "rpc": {"method": "set_properties", "params": [{
                    "did": did, "siid": siid, "piid": piid, "value": value
                }]}}),
            )
            .await?;
        let result = response
            .get("result")
            .and_then(Value::as_array)
            .and_then(|items| items.first())
            .cloned()
            .ok_or(MiotError::Protocol(
                "MIPS set response did not include a result",
            ))?;
        ensure_success(&result)?;
        Ok(result)
    }

    /// 执行一个 MIoT-Spec action 并返回输出值。
    ///
    /// # Errors
    ///
    /// 请求无法发送、超时或网关拒绝 action 时返回错误。
    pub async fn execute_action(
        &self,
        did: &str,
        action: &MipsAction,
    ) -> Result<Vec<Value>, MiotError> {
        validate_did(did)?;
        let response = self
            .request(
                "proxy/rpcReq",
                json!({"did": did, "rpc": {"method": "action", "params": {
                    "did": did, "siid": action.siid, "aiid": action.aiid, "in": action.input
                }}}),
            )
            .await?;
        let result = response.get("result").ok_or(MiotError::Protocol(
            "MIPS action response did not include result",
        ))?;
        ensure_success(result)?;
        result
            .get("out")
            .and_then(Value::as_array)
            .cloned()
            .ok_or(MiotError::Protocol(
                "MIPS action response did not include output",
            ))
    }

    /// 订阅一个设备或一个属性的属性变更通知。
    ///
    /// 任一实例 ID 为 `None` 时，订阅该 `did` 的全部属性。
    ///
    /// # Errors
    ///
    /// DID 无效或 MQTT 订阅命令无法入队时返回错误。
    pub async fn subscribe_properties(
        &self,
        did: &str,
        siid: Option<u16>,
        piid: Option<u16>,
    ) -> Result<mpsc::Receiver<MipsProperty>, MiotError> {
        validate_did(did)?;
        let filter = property_filter(did, siid, piid);
        let (sender, receiver) = mpsc::channel(COMMAND_QUEUE_CAPACITY);
        self.subscribe(filter, Listener::Property(sender)).await?;
        Ok(receiver)
    }

    /// 订阅一个设备或一个事件的事件通知。
    ///
    /// 任一实例 ID 为 `None` 时，订阅该 `did` 的全部事件。
    ///
    /// # Errors
    ///
    /// DID 无效或 MQTT 订阅命令无法入队时返回错误。
    pub async fn subscribe_events(
        &self,
        did: &str,
        siid: Option<u16>,
        eiid: Option<u16>,
    ) -> Result<mpsc::Receiver<MipsEvent>, MiotError> {
        validate_did(did)?;
        let filter = event_filter(did, siid, eiid);
        let (sender, receiver) = mpsc::channel(COMMAND_QUEUE_CAPACITY);
        self.subscribe(filter, Listener::Event(sender)).await?;
        Ok(receiver)
    }

    async fn request(&self, topic: &'static str, payload: Value) -> Result<Value, MiotError> {
        let (response_tx, response_rx) = oneshot::channel();
        self.command_tx
            .send(Command::Request {
                topic,
                payload,
                response_tx,
            })
            .await
            .map_err(|_| MiotError::Mqtt("MIPS connection is closed".to_owned()))?;
        match timeout(self.request_timeout, response_rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(MiotError::Mqtt(
                "MIPS connection closed before response".to_owned(),
            )),
            Err(_) => Err(MiotError::Mqtt("MIPS request timed out".to_owned())),
        }
    }

    async fn subscribe(&self, filter: String, listener: Listener) -> Result<(), MiotError> {
        let (response_tx, response_rx) = oneshot::channel();
        self.command_tx
            .send(Command::Subscribe {
                filter,
                listener,
                response_tx,
            })
            .await
            .map_err(|_| MiotError::Mqtt("MIPS connection is closed".to_owned()))?;
        response_rx
            .await
            .map_err(|_| MiotError::Mqtt("MIPS connection closed while subscribing".to_owned()))?
    }
}

enum Command {
    Request {
        topic: &'static str,
        payload: Value,
        response_tx: oneshot::Sender<Result<Value, MiotError>>,
    },
    Subscribe {
        filter: String,
        listener: Listener,
        response_tx: oneshot::Sender<Result<(), MiotError>>,
    },
}

enum Listener {
    Property(mpsc::Sender<MipsProperty>),
    Event(mpsc::Sender<MipsEvent>),
}

async fn run_connection(
    mqtt: AsyncClient,
    mut event_loop: EventLoop,
    mut commands: mpsc::Receiver<Command>,
    client_id: String,
    connected_tx: oneshot::Sender<Result<(), MiotError>>,
) {
    let mut connected_tx = Some(connected_tx);
    let mut next_id = 0_u32;
    let mut requests = HashMap::<u32, oneshot::Sender<Result<Value, MiotError>>>::new();
    let mut listeners = HashMap::<String, Vec<Listener>>::new();

    loop {
        tokio::select! {
            command = commands.recv() => match command {
                Some(Command::Request { topic, mut payload, response_tx }) => {
                    next_id = next_id.wrapping_add(1);
                    let id = next_id;
                    if let Some(rpc) = payload.get_mut("rpc").and_then(Value::as_object_mut) {
                        rpc.insert("id".to_owned(), Value::from(id));
                    }
                    let reply_topic = format!("{client_id}/reply");
                    let bytes = match pack_message(id, &payload.to_string(), Some(&reply_topic)) {
                        Ok(bytes) => bytes,
                        Err(error) => { let _ = response_tx.send(Err(error)); continue; }
                    };
                    match mqtt.publish(format!("master/{topic}"), QoS::ExactlyOnce, false, bytes).await {
                        Ok(()) => { requests.insert(id, response_tx); }
                        Err(error) => { let _ = response_tx.send(Err(MiotError::Mqtt(error.to_string()))); }
                    }
                }
                Some(Command::Subscribe { filter, listener, response_tx }) => {
                    let broker_filter = format!("master/{filter}");
                    match mqtt.subscribe(broker_filter, QoS::ExactlyOnce).await {
                        Ok(()) => {
                            listeners.entry(filter).or_default().push(listener);
                            let _ = response_tx.send(Ok(()));
                        }
                        Err(error) => { let _ = response_tx.send(Err(MiotError::Mqtt(error.to_string()))); }
                    }
                }
                None => break,
            },
            event = event_loop.poll() => match event {
                Ok(Event::Incoming(Packet::ConnAck(_))) => {
                    if let Some(sender) = connected_tx.take() { let _ = sender.send(Ok(())); }
                    if let Err(error) = mqtt.subscribe(format!("{client_id}/#"), QoS::ExactlyOnce).await {
                        tracing::warn!(%error, "could not subscribe to MIPS reply topic");
                    }
                    for filter in listeners.keys() {
                        if let Err(error) = mqtt.subscribe(format!("master/{filter}"), QoS::ExactlyOnce).await {
                            tracing::warn!(%error, %filter, "could not restore MIPS subscription");
                        }
                    }
                }
                Ok(Event::Incoming(Packet::Publish(publish))) => {
                    let topic = String::from_utf8_lossy(&publish.topic);
                    handle_publish(&client_id, &topic, &publish.payload, &mut requests, &mut listeners);
                }
                Ok(_) => {}
                Err(error) => {
                    if let Some(sender) = connected_tx.take() {
                        let _ = sender.send(Err(MiotError::Mqtt(error.to_string())));
                        break;
                    }
                    tracing::warn!(%error, "MIPS MQTT connection interrupted; waiting for reconnect");
                }
            },
        }
    }
    if let Some(sender) = connected_tx {
        let _ = sender.send(Err(MiotError::Mqtt("MIPS connection stopped".to_owned())));
    }
    for (_, sender) in requests {
        let _ = sender.send(Err(MiotError::Mqtt("MIPS connection stopped".to_owned())));
    }
}

fn handle_publish(
    client_id: &str,
    topic: &str,
    payload: &[u8],
    requests: &mut HashMap<u32, oneshot::Sender<Result<Value, MiotError>>>,
    listeners: &mut HashMap<String, Vec<Listener>>,
) {
    let message = match unpack_message(payload) {
        Ok(message) => message,
        Err(error) => {
            tracing::debug!(%error, %topic, "ignoring invalid MIPS message");
            return;
        }
    };
    if topic == format!("{client_id}/reply") {
        if let Some(sender) = requests.remove(&message.id) {
            let _ = sender.send(parse_response(message.payload.as_ref()));
        }
        return;
    }
    let Some(payload) = message.payload else {
        return;
    };
    let client_prefix = format!("{client_id}/");
    let logical_topic = topic
        .strip_prefix(&client_prefix)
        .or_else(|| topic.strip_prefix("master/"))
        .unwrap_or(topic);
    let property = parse_property(&payload);
    let event = parse_event(&payload);
    for (filter, entries) in listeners.iter_mut() {
        if !topic_matches(filter, logical_topic) {
            continue;
        }
        entries.retain(|entry| match entry {
            Listener::Property(sender) => {
                if let Some(item) = &property {
                    let _ = sender.try_send(item.clone());
                }
                !sender.is_closed()
            }
            Listener::Event(sender) => {
                if let Some(item) = &event {
                    let _ = sender.try_send(item.clone());
                }
                !sender.is_closed()
            }
        });
    }
}

fn parse_response(payload: Option<&String>) -> Result<Value, MiotError> {
    let value: Value = serde_json::from_str(payload.map_or("{}", String::as_str))
        .map_err(|_| MiotError::Protocol("could not decode MIPS response JSON"))?;
    if let Some(error) = value.get("error") {
        return Err(MiotError::LocalDevice(error.to_string()));
    }
    Ok(value)
}

fn parse_property(payload: &str) -> Option<MipsProperty> {
    let value: Value = serde_json::from_str(payload).ok()?;
    Some(MipsProperty {
        did: value.get("did")?.as_str()?.to_owned(),
        siid: u16::try_from(value.get("siid")?.as_u64()?).ok()?,
        piid: u16::try_from(value.get("piid")?.as_u64()?).ok()?,
        value: value.get("value")?.clone(),
    })
}

fn parse_event(payload: &str) -> Option<MipsEvent> {
    let value: Value = serde_json::from_str(payload).ok()?;
    Some(MipsEvent {
        did: value.get("did")?.as_str()?.to_owned(),
        siid: u16::try_from(value.get("siid")?.as_u64()?).ok()?,
        eiid: u16::try_from(value.get("eiid")?.as_u64()?).ok()?,
        arguments: value
            .get("arguments")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
    })
}

fn ensure_success(result: &Value) -> Result<(), MiotError> {
    match result.get("code").and_then(Value::as_i64) {
        Some(0 | 1) => Ok(()),
        Some(code) => Err(MiotError::LocalDevice(format!(
            "MIPS result code {code}: {result}"
        ))),
        None => Err(MiotError::Protocol("MIPS result did not include code")),
    }
}

fn property_filter(did: &str, siid: Option<u16>, piid: Option<u16>) -> String {
    match (siid, piid) {
        (Some(siid), Some(piid)) => format!("appMsg/notify/iot/{did}/property/{siid}.{piid}"),
        _ => format!("appMsg/notify/iot/{did}/property/#"),
    }
}

fn event_filter(did: &str, siid: Option<u16>, eiid: Option<u16>) -> String {
    match (siid, eiid) {
        (Some(siid), Some(eiid)) => format!("appMsg/notify/iot/{did}/event/{siid}.{eiid}"),
        _ => format!("appMsg/notify/iot/{did}/event/#"),
    }
}

fn topic_matches(filter: &str, topic: &str) -> bool {
    filter
        .strip_suffix("/#")
        .is_some_and(|prefix| topic == prefix || topic.starts_with(&format!("{prefix}/")))
        || filter == topic
}

fn validate_config(config: &MipsClientConfig) -> Result<(), MiotError> {
    if config.client_id.trim().is_empty() {
        return Err(MiotError::InvalidInput("MIPS client ID must not be empty"));
    }
    if config.host.trim().is_empty() {
        return Err(MiotError::InvalidInput("MIPS host must not be empty"));
    }
    if config.port == 0 {
        return Err(MiotError::InvalidInput("MIPS port must not be zero"));
    }
    if config.request_timeout.is_zero() {
        return Err(MiotError::InvalidInput("MIPS timeout must not be zero"));
    }
    Ok(())
}

fn validate_did(did: &str) -> Result<(), MiotError> {
    if did.trim().is_empty() {
        Err(MiotError::InvalidInput("device DID must not be empty"))
    } else {
        Ok(())
    }
}

fn read_pem(path: &PathBuf, name: &'static str) -> Result<Vec<u8>, MiotError> {
    let bytes = std::fs::read(path).map_err(|error| {
        MiotError::Mqtt(format!("could not read {name} {}: {error}", path.display()))
    })?;
    if bytes.is_empty() {
        return Err(MiotError::Mqtt(format!(
            "{name} {} is empty",
            path.display()
        )));
    }
    Ok(bytes)
}

fn tls_config(
    ca_pem: Vec<u8>,
    certificate_pem: Vec<u8>,
    private_key_pem: Vec<u8>,
) -> Result<Arc<ClientConfig>, MiotError> {
    let mut roots = RootCertStore::empty();
    let ca_certificates = rustls_pemfile::certs(&mut BufReader::new(Cursor::new(ca_pem)))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| {
            MiotError::Mqtt(format!("could not parse MIPS CA certificate: {error}"))
        })?;
    roots.add_parsable_certificates(ca_certificates);
    if roots.is_empty() {
        return Err(MiotError::Mqtt(
            "MIPS CA file contained no certificates".to_owned(),
        ));
    }

    let client_certificates =
        rustls_pemfile::certs(&mut BufReader::new(Cursor::new(certificate_pem)))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| {
                MiotError::Mqtt(format!("could not parse MIPS client certificate: {error}"))
            })?;
    if client_certificates.is_empty() {
        return Err(MiotError::Mqtt(
            "MIPS client certificate file is empty".to_owned(),
        ));
    }
    let private_key =
        rustls_pemfile::private_key(&mut BufReader::new(Cursor::new(private_key_pem)))
            .map_err(|error| MiotError::Mqtt(format!("could not parse MIPS private key: {error}")))?
            .ok_or_else(|| {
                MiotError::Mqtt("MIPS private key file contained no private key".to_owned())
            })?;
    let verifier = WebPkiServerVerifier::builder(Arc::new(roots))
        .build()
        .map_err(|error| MiotError::Mqtt(format!("could not build MIPS CA verifier: {error}")))?;
    let config = ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(GatewayServerVerifier { verifier }))
        .with_client_auth_cert(client_certificates, private_key)
        .map_err(|error| {
            MiotError::Mqtt(format!(
                "could not configure MIPS client certificate: {error}"
            ))
        })?;
    Ok(Arc::new(config))
}

/// Keeps certificate-chain and signature validation, but permits a gateway
/// certificate whose DNS name does not match the direct local IP address.
#[derive(Debug)]
struct GatewayServerVerifier {
    verifier: Arc<WebPkiServerVerifier>,
}

impl ServerCertVerifier for GatewayServerVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, RustlsError> {
        match self.verifier.verify_server_cert(
            end_entity,
            intermediates,
            server_name,
            ocsp_response,
            now,
        ) {
            Err(RustlsError::InvalidCertificate(CertificateError::NotValidForName)) => {
                Ok(ServerCertVerified::assertion())
            }
            result => result,
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, RustlsError> {
        self.verifier.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, RustlsError> {
        self.verifier.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.verifier.supported_verify_schemes()
    }
}

struct MipsMessage {
    id: u32,
    payload: Option<String>,
}

fn pack_message(id: u32, payload: &str, reply_topic: Option<&str>) -> Result<Vec<u8>, MiotError> {
    let mut result = Vec::new();
    result.extend_from_slice(&4_u32.to_le_bytes());
    result.push(0);
    result.extend_from_slice(&id.to_le_bytes());
    add_string_field(&mut result, 3, b"local")?;
    if let Some(topic) = reply_topic {
        add_string_field(&mut result, 1, topic.as_bytes())?;
    }
    add_string_field(&mut result, 2, payload.as_bytes())?;
    Ok(result)
}

fn add_string_field(output: &mut Vec<u8>, kind: u8, value: &[u8]) -> Result<(), MiotError> {
    let length = value
        .len()
        .checked_add(1)
        .ok_or(MiotError::Protocol("MIPS field is too large"))?;
    let length =
        u32::try_from(length).map_err(|_| MiotError::Protocol("MIPS field is too large"))?;
    output.extend_from_slice(&length.to_le_bytes());
    output.push(kind);
    output.extend_from_slice(value);
    output.push(0);
    Ok(())
}

fn unpack_message(data: &[u8]) -> Result<MipsMessage, MiotError> {
    let mut offset = 0;
    let mut id = None;
    let mut payload = None;
    while offset < data.len() {
        if data.len().saturating_sub(offset) < 5 {
            return Err(MiotError::Protocol("truncated MIPS message field"));
        }
        let length = u32::from_le_bytes(
            data[offset..offset + 4]
                .try_into()
                .expect("fixed field length"),
        );
        let kind = data[offset + 4];
        offset += 5;
        let length = usize::try_from(length)
            .map_err(|_| MiotError::Protocol("invalid MIPS field length"))?;
        if length == 0 || data.len().saturating_sub(offset) < length {
            return Err(MiotError::Protocol("invalid MIPS field length"));
        }
        let value = &data[offset..offset + length];
        match kind {
            0 if value.len() == 4 => {
                id = Some(u32::from_le_bytes(
                    value.try_into().expect("fixed ID length"),
                ));
            }
            2 => {
                payload = Some(
                    String::from_utf8(value.strip_suffix(&[0]).unwrap_or(value).to_vec())
                        .map_err(|_| MiotError::Protocol("MIPS payload is not UTF-8"))?,
                );
            }
            _ => {}
        }
        offset += length;
    }
    Ok(MipsMessage {
        id: id.ok_or(MiotError::Protocol("MIPS message did not include ID"))?,
        payload,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packs_and_unpacks_a_mips_message() {
        let bytes = pack_message(42, r#"{"value":true}"#, Some("virtual-did/reply")).unwrap();
        assert_eq!(&bytes[..9], &[4, 0, 0, 0, 0, 42, 0, 0, 0]);
        let message = unpack_message(&bytes).unwrap();
        assert_eq!(message.id, 42);
        assert_eq!(message.payload.as_deref(), Some(r#"{"value":true}"#));
    }

    #[test]
    fn builds_mips_notification_filters() {
        assert_eq!(
            property_filter("did", Some(2), Some(1)),
            "appMsg/notify/iot/did/property/2.1"
        );
        assert_eq!(
            event_filter("did", None, None),
            "appMsg/notify/iot/did/event/#"
        );
        assert!(topic_matches(
            "appMsg/notify/iot/did/event/#",
            "appMsg/notify/iot/did/event/2.1"
        ));
    }

    #[test]
    fn parses_property_and_event_notifications() {
        assert_eq!(
            parse_property(r#"{"did":"x","siid":2,"piid":1,"value":true}"#)
                .unwrap()
                .value,
            Value::Bool(true)
        );
        assert_eq!(
            parse_event(r#"{"did":"x","siid":2,"eiid":1}"#)
                .unwrap()
                .arguments,
            Vec::<Value>::new()
        );
    }
}
