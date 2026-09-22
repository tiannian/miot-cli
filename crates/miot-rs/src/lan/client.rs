use std::{net::SocketAddr, time::Duration};

use aes::Aes128;
use cbc::cipher::{BlockDecryptMut, BlockEncryptMut, KeyIvInit, block_padding::Pkcs7};
use cbc::{Decryptor, Encryptor};
use md5::{Digest, Md5};
use serde::Serialize;
use serde_json::{Value, json};
use tokio::{net::UdpSocket, time::timeout};
use tracing::{debug, trace};

use crate::MiotError;

const MIIO_PORT: u16 = 54_321;
const HEADER_SIZE: usize = 32;
const MAX_PACKET_SIZE: usize = 65_535;
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

/// One `MIoT` property addressed by its service and property instance IDs.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Property {
    pub siid: u16,
    pub piid: u16,
}

/// One `MIoT` action addressed by its service and action instance IDs.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Action {
    pub siid: u16,
    pub aiid: u16,
    #[serde(rename = "in")]
    pub input: Vec<Value>,
}

/// An authenticated client for Xiaomi's local miIO UDP protocol.
///
/// The token belongs to the IP-addressable device. For a gateway-proxied child
/// device, construct this client with the gateway host and token, then pass the
/// child DID to [`Self::get_properties`], [`Self::set_properties`], or
/// [`Self::call_action`].
#[derive(Clone, Debug)]
pub struct Client {
    address: SocketAddr,
    token: [u8; 16],
    timeout: Duration,
}

impl Client {
    /// Creates a local client for a device or gateway at `host`.
    ///
    /// `token` must be the 32-character hexadecimal miIO token for that host.
    ///
    /// # Errors
    ///
    /// Returns an error when `host` is not an IP socket address or `token` is
    /// not a 16-byte hexadecimal miIO token.
    pub fn new(host: &str, token: &str) -> Result<Self, MiotError> {
        let address = parse_address(host)?;
        let decoded = hex::decode(token)
            .map_err(|_| MiotError::InvalidInput("local token must be hexadecimal"))?;
        let token: [u8; 16] = decoded
            .try_into()
            .map_err(|_| MiotError::InvalidInput("local token must be 16 bytes"))?;
        Ok(Self {
            address,
            token,
            timeout: DEFAULT_TIMEOUT,
        })
    }

    /// Sets the maximum time for the hello exchange and every command response.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Returns the target device or gateway address.
    #[must_use]
    pub fn address(&self) -> SocketAddr {
        self.address
    }

    /// Sends an arbitrary miIO method and returns its `result` value.
    ///
    /// # Errors
    ///
    /// Returns an error when the local UDP exchange, authentication, packet
    /// decoding, or device request fails.
    pub async fn request(&self, method: &str, params: Value) -> Result<Value, MiotError> {
        if method.is_empty() {
            return Err(MiotError::InvalidInput("local method must not be empty"));
        }
        let socket = UdpSocket::bind("0.0.0.0:0")
            .await
            .map_err(|error| network_error(&error))?;
        socket
            .connect(self.address)
            .await
            .map_err(|error| network_error(&error))?;
        let (device_id, timestamp) = self.hello(&socket).await?;
        let request = json!({ "id": 1, "method": method, "params": params });
        let packet = encode_packet(device_id, timestamp.wrapping_add(1), &self.token, &request)?;
        trace!(address = %self.address, method, "sending local miIO request");
        socket
            .send(&packet)
            .await
            .map_err(|error| network_error(&error))?;
        let response = self.receive_packet(&socket).await?;
        let payload = decode_packet(&response, &self.token)?;
        debug!(address = %self.address, method, "received local miIO response");
        response_result(&payload)
    }

    /// Reads `MIoT` properties for `did`, which may name a child behind a gateway.
    ///
    /// # Errors
    ///
    /// Returns an error when the DID or property list is invalid, or the local
    /// device does not return an array of property results.
    pub async fn get_properties(
        &self,
        did: &str,
        properties: &[Property],
    ) -> Result<Vec<Value>, MiotError> {
        validate_did(did)?;
        if properties.is_empty() {
            return Err(MiotError::InvalidInput("at least one property is required"));
        }
        let params = properties
            .iter()
            .map(|property| {
                json!({
                    "did": did,
                    "siid": property.siid,
                    "piid": property.piid,
                })
            })
            .collect();
        let result = self.request("get_properties", Value::Array(params)).await?;
        result.as_array().cloned().ok_or(MiotError::Protocol(
            "local get_properties response was not an array",
        ))
    }

    /// Writes `MIoT` properties for `did`, which may name a child behind a gateway.
    ///
    /// # Errors
    ///
    /// Returns an error when the DID or values are invalid, or the local device
    /// does not return an array of property results.
    pub async fn set_properties(
        &self,
        did: &str,
        values: &[(Property, Value)],
    ) -> Result<Vec<Value>, MiotError> {
        validate_did(did)?;
        if values.is_empty() {
            return Err(MiotError::InvalidInput(
                "at least one property value is required",
            ));
        }
        let params = values
            .iter()
            .map(|(property, value)| {
                json!({
                    "did": did,
                    "siid": property.siid,
                    "piid": property.piid,
                    "value": value,
                })
            })
            .collect();
        let result = self.request("set_properties", Value::Array(params)).await?;
        result.as_array().cloned().ok_or(MiotError::Protocol(
            "local set_properties response was not an array",
        ))
    }

    /// Calls a `MIoT` action for `did`, which may name a child behind a gateway.
    ///
    /// # Errors
    ///
    /// Returns an error when the DID is invalid or the local device rejects the action.
    pub async fn call_action(&self, did: &str, action: &Action) -> Result<Value, MiotError> {
        validate_did(did)?;
        self.request(
            "action",
            json!({
                "did": did,
                "siid": action.siid,
                "aiid": action.aiid,
                "in": action.input,
            }),
        )
        .await
    }

    async fn hello(&self, socket: &UdpSocket) -> Result<(u32, u32), MiotError> {
        let mut hello = [0_u8; HEADER_SIZE];
        hello[0..2].copy_from_slice(&[0x21, 0x31]);
        hello[2..4].copy_from_slice(
            &u16::try_from(HEADER_SIZE)
                .expect("the fixed miIO header size fits in u16")
                .to_be_bytes(),
        );
        hello[8..12].copy_from_slice(&u32::MAX.to_be_bytes());
        hello[12..16].copy_from_slice(&u32::MAX.to_be_bytes());
        socket
            .send(&hello)
            .await
            .map_err(|error| network_error(&error))?;
        let response = self.receive_packet(socket).await?;
        if response.len() < HEADER_SIZE || response[0..2] != [0x21, 0x31] {
            return Err(MiotError::Protocol("invalid local miIO hello response"));
        }
        let device_id = u32::from_be_bytes(response[8..12].try_into().expect("fixed slice length"));
        let timestamp =
            u32::from_be_bytes(response[12..16].try_into().expect("fixed slice length"));
        if device_id == u32::MAX || timestamp == u32::MAX {
            return Err(MiotError::Protocol(
                "local miIO hello response did not include device state",
            ));
        }
        Ok((device_id, timestamp))
    }

    async fn receive_packet(&self, socket: &UdpSocket) -> Result<Vec<u8>, MiotError> {
        let mut buffer = vec![0_u8; MAX_PACKET_SIZE];
        let received = timeout(self.timeout, socket.recv(&mut buffer))
            .await
            .map_err(|_| MiotError::Protocol("local miIO request timed out"))?
            .map_err(|error| network_error(&error))?;
        buffer.truncate(received);
        Ok(buffer)
    }
}

fn parse_address(host: &str) -> Result<SocketAddr, MiotError> {
    let address = if host.contains(':') {
        host.to_owned()
    } else {
        format!("{host}:{MIIO_PORT}")
    };
    address
        .parse()
        .map_err(|_| MiotError::InvalidInput("invalid local device address"))
}

fn validate_did(did: &str) -> Result<(), MiotError> {
    if did.is_empty() {
        Err(MiotError::InvalidInput("device DID must not be empty"))
    } else {
        Ok(())
    }
}

fn network_error(error: &std::io::Error) -> MiotError {
    MiotError::LocalNetwork(error.to_string())
}

fn encode_packet(
    device_id: u32,
    timestamp: u32,
    token: &[u8; 16],
    payload: &Value,
) -> Result<Vec<u8>, MiotError> {
    let plaintext = serde_json::to_vec(payload)
        .map_err(|_| MiotError::Protocol("could not encode local miIO request"))?;
    let encrypted = encrypt(token, &plaintext);
    let mut packet = vec![0_u8; HEADER_SIZE];
    packet[0..2].copy_from_slice(&[0x21, 0x31]);
    packet[2..4].copy_from_slice(
        &u16::try_from(HEADER_SIZE + encrypted.len())
            .map_err(|_| MiotError::Protocol("local miIO packet is too large"))?
            .to_be_bytes(),
    );
    packet[8..12].copy_from_slice(&device_id.to_be_bytes());
    packet[12..16].copy_from_slice(&timestamp.to_be_bytes());
    packet.extend(encrypted);
    let checksum = packet_checksum(&packet, token);
    packet[16..32].copy_from_slice(&checksum);
    Ok(packet)
}

fn decode_packet(packet: &[u8], token: &[u8; 16]) -> Result<Value, MiotError> {
    if packet.len() < HEADER_SIZE || packet[0..2] != [0x21, 0x31] {
        return Err(MiotError::Protocol("invalid local miIO response"));
    }
    let length = usize::from(u16::from_be_bytes(
        packet[2..4].try_into().expect("fixed slice length"),
    ));
    if length != packet.len() {
        return Err(MiotError::Protocol(
            "local miIO response length does not match header",
        ));
    }
    if packet[16..32] != packet_checksum(packet, token) {
        return Err(MiotError::Protocol(
            "local miIO response checksum did not match token",
        ));
    }
    let plaintext = decrypt(token, &packet[HEADER_SIZE..])?;
    serde_json::from_slice(&plaintext)
        .map_err(|_| MiotError::Protocol("could not decode local miIO response"))
}

fn response_result(payload: &Value) -> Result<Value, MiotError> {
    if let Some(error) = payload.get("error") {
        return Err(MiotError::LocalDevice(error.to_string()));
    }
    payload.get("result").cloned().ok_or(MiotError::Protocol(
        "local miIO response did not include result",
    ))
}

fn packet_checksum(packet: &[u8], token: &[u8; 16]) -> [u8; 16] {
    let mut hasher = Md5::new();
    hasher.update(&packet[..16]);
    hasher.update(token);
    hasher.update(&packet[HEADER_SIZE..]);
    hasher.finalize().into()
}

fn cipher_material(token: &[u8; 16]) -> ([u8; 16], [u8; 16]) {
    let key: [u8; 16] = Md5::digest(token).into();
    let iv: [u8; 16] = Md5::digest([key.as_slice(), token].concat()).into();
    (key, iv)
}

fn encrypt(token: &[u8; 16], plaintext: &[u8]) -> Vec<u8> {
    let (key, iv) = cipher_material(token);
    Encryptor::<Aes128>::new(&key.into(), &iv.into()).encrypt_padded_vec_mut::<Pkcs7>(plaintext)
}

fn decrypt(token: &[u8; 16], ciphertext: &[u8]) -> Result<Vec<u8>, MiotError> {
    let (key, iv) = cipher_material(token);
    Decryptor::<Aes128>::new(&key.into(), &iv.into())
        .decrypt_padded_vec_mut::<Pkcs7>(ciphertext)
        .map_err(|_| MiotError::Protocol("could not decrypt local miIO response"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: [u8; 16] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];

    #[test]
    fn encrypts_and_decrypts_a_miio_packet() {
        let payload = json!({"id": 1, "method": "get_properties", "params": []});
        let packet = encode_packet(0x1234_5678, 42, &TOKEN, &payload).unwrap();
        assert_eq!(&packet[0..4], &[0x21, 0x31, 0, packet.len() as u8]);
        assert_eq!(decode_packet(&packet, &TOKEN).unwrap(), payload);
    }

    #[test]
    fn rejects_packets_authenticated_with_another_token() {
        let packet = encode_packet(1, 1, &TOKEN, &json!({"result": "ok"})).unwrap();
        assert!(decode_packet(&packet, &[1; 16]).is_err());
    }

    #[test]
    fn parses_default_and_explicit_local_ports() {
        assert_eq!(parse_address("192.0.2.1").unwrap().port(), MIIO_PORT);
        assert_eq!(parse_address("192.0.2.1:1234").unwrap().port(), 1234);
    }
}
