use std::fmt::Write as _;

use base64::Engine;
use reqwest::{Client, Url};
use serde_json::Value;
use sha1::{Digest, Sha1};
use sha2::Sha256;
use tracing::{debug, trace};

use crate::{CloudCredential, MiotError};

const USER_AGENT: &str =
    "Android-7.1.1-1.0.0-ONEPLUS A3010-136-MIOTCLI APP/xiaomi.smarthome APPV/62830";

/// A Xiaomi Home API client authenticated with a `xiaomiio` service token.
#[derive(Clone, Debug)]
pub struct ApiClient {
    client: Client,
    region: String,
    credential: CloudCredential,
}

impl ApiClient {
    /// Creates a client for a Xiaomi cloud region such as `cn` or `us`.
    ///
    /// # Errors
    ///
    /// Returns an error when the region is empty or the HTTP client cannot be built.
    pub fn new(region: impl Into<String>, credential: CloudCredential) -> Result<Self, MiotError> {
        let region = region.into();
        if region.is_empty() {
            return Err(MiotError::InvalidInput("cloud region must not be empty"));
        }
        debug!(region, "creating Xiaomi cloud API client");
        let client = Client::builder().user_agent(USER_AGENT).build()?;
        Ok(Self {
            client,
            region,
            credential,
        })
    }

    #[must_use]
    pub fn region(&self) -> &str {
        &self.region
    }

    pub(super) async fn post(&self, path: &str, data: Value) -> Result<Value, MiotError> {
        let url = self.api_url(path)?;
        debug!(region = %self.region, path, "sending Xiaomi cloud API request");
        let nonce = nonce()?;
        let signed_nonce = signed_nonce(self.credential.ssecurity(), &nonce)?;
        let mut params = vec![(
            "data".to_owned(),
            serde_json::to_string(&data)
                .map_err(|_| MiotError::Protocol("could not encode cloud API request"))?,
        )];
        let hash = sha1_sign("POST", &url, &params, &signed_nonce);
        params.push(("rc4_hash__".to_owned(), hash));
        for (_, value) in &mut params {
            *value = rc4_crypt(&signed_nonce, value.as_bytes())?;
        }
        let signature = sha1_sign("POST", &url, &params, &signed_nonce);
        params.extend([
            ("signature".to_owned(), signature),
            (
                "ssecurity".to_owned(),
                self.credential.ssecurity().to_owned(),
            ),
            ("_nonce".to_owned(), nonce),
        ]);
        let response = self
            .client
            .post(url)
            .header("X-XIAOMI-PROTOCAL-FLAG-CLI", "PROTOCAL-HTTP2")
            .header("MIOT-ENCRYPT-ALGORITHM", "ENCRYPT-RC4")
            .header("Accept-Encoding", "identity")
            .header("Cookie", self.cookie_header())
            .form(&params)
            .send()
            .await?;
        let status = response.status();
        let body = response.text().await?;
        trace!(path, status = %status, response_bytes = body.len(), "received Xiaomi cloud API response");
        let value = decode_response(&body, &signed_nonce)?;
        if !status.is_success()
            || value
                .get("code")
                .and_then(Value::as_i64)
                .is_some_and(|code| code != 0)
        {
            let cloud_code = value.get("code").and_then(Value::as_i64);
            debug!(path, status = %status, ?cloud_code, "Xiaomi cloud API request failed");
            return Err(MiotError::CloudApiResponse {
                status: status.as_u16(),
                code: value.get("code").and_then(Value::as_i64),
                message: value
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown cloud API error")
                    .to_owned(),
            });
        }
        Ok(value)
    }

    fn api_url(&self, path: &str) -> Result<Url, MiotError> {
        let host = if self.region.eq_ignore_ascii_case("cn") {
            "api.io.mi.com".to_owned()
        } else {
            format!("{}.api.io.mi.com", self.region)
        };
        Url::parse(&format!(
            "https://{host}/app/{}",
            path.trim_start_matches('/')
        ))
        .map_err(|_| MiotError::InvalidInput("invalid cloud API path"))
    }

    fn cookie_header(&self) -> String {
        format!(
            "userId={}; yetAnotherServiceToken={}; serviceToken={}; locale=zh_CN; timezone=GMT+08:00; channel=MI_APP_STORE",
            self.credential.user_id(),
            self.credential.service_token(),
            self.credential.service_token()
        )
    }
}

fn nonce() -> Result<String, MiotError> {
    let mut bytes = [0_u8; 12];
    getrandom::fill(&mut bytes)
        .map_err(|_| MiotError::Protocol("could not generate cloud API nonce"))?;
    Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
}

fn signed_nonce(ssecurity: &str, nonce: &str) -> Result<String, MiotError> {
    let secret = base64::engine::general_purpose::STANDARD
        .decode(ssecurity)
        .map_err(|_| MiotError::Protocol("invalid ssecurity"))?;
    let nonce = base64::engine::general_purpose::STANDARD
        .decode(nonce)
        .map_err(|_| MiotError::Protocol("invalid cloud API nonce"))?;
    Ok(base64::engine::general_purpose::STANDARD.encode(Sha256::digest([secret, nonce].concat())))
}

fn sha1_sign(method: &str, url: &Url, params: &[(String, String)], signed_nonce: &str) -> String {
    let path = url.path().strip_prefix("/app").unwrap_or(url.path());
    let mut source = format!("{method}&{path}");
    for (key, value) in params {
        write!(&mut source, "&{key}={value}").expect("writing to String cannot fail");
    }
    source.push('&');
    source.push_str(signed_nonce);
    base64::engine::general_purpose::STANDARD.encode(Sha1::digest(source.as_bytes()))
}

fn decode_response(body: &str, signed_nonce: &str) -> Result<Value, MiotError> {
    if let Ok(value) = serde_json::from_str::<Value>(body) {
        if value.get("message").is_some() {
            return Ok(value);
        }
    }
    let plaintext = rc4_decrypt(signed_nonce, body)?;
    serde_json::from_slice(&plaintext)
        .map_err(|_| MiotError::Protocol("could not decode cloud API response"))
}

fn rc4_crypt(key: &str, plaintext: &[u8]) -> Result<String, MiotError> {
    Ok(base64::engine::general_purpose::STANDARD.encode(rc4(key, plaintext)?))
}

fn rc4_decrypt(key: &str, ciphertext: &str) -> Result<Vec<u8>, MiotError> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(ciphertext)
        .map_err(|_| MiotError::Protocol("invalid encrypted cloud API response"))?;
    rc4(key, &bytes)
}

fn rc4(key: &str, input: &[u8]) -> Result<Vec<u8>, MiotError> {
    let key = base64::engine::general_purpose::STANDARD
        .decode(key)
        .map_err(|_| MiotError::Protocol("invalid signed nonce"))?;
    if key.is_empty() {
        return Err(MiotError::Protocol("empty signed nonce"));
    }
    let mut state: Vec<u8> = (0..=255).collect();
    let mut j = 0_usize;
    for i in 0..256 {
        j = (j + usize::from(state[i]) + usize::from(key[i % key.len()])) & 255;
        state.swap(i, j);
    }
    let mut i = 0_usize;
    j = 0;
    for _ in 0..1024 {
        i = (i + 1) & 255;
        j = (j + usize::from(state[i])) & 255;
        state.swap(i, j);
        let _ = state[(usize::from(state[i]) + usize::from(state[j])) & 255];
    }
    Ok(input
        .iter()
        .map(|byte| {
            i = (i + 1) & 255;
            j = (j + usize::from(state[i])) & 255;
            state.swap(i, j);
            byte ^ state[(usize::from(state[i]) + usize::from(state[j])) & 255]
        })
        .collect())
}
