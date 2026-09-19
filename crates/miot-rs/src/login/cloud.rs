#![allow(clippy::missing_errors_doc)]

use std::sync::Arc;

use base64::Engine;
use md5::{Digest, Md5};
use serde::Deserialize;
use sha1::Sha1;
use url::Url;

use super::MiotError;

const ACCOUNT_BASE: &str = "https://account.xiaomi.com";

/// Input for a Xiaomi account-password login. It is intentionally consumed per attempt.
#[derive(Clone, Debug)]
pub struct CloudLoginRequest {
    pub account: String,
    pub password: String,
}

/// A verification ticket obtained by the user outside this SDK.
#[derive(Clone, Debug)]
pub struct VerificationProof {
    ticket: String,
}

impl VerificationProof {
    #[must_use]
    pub fn new(ticket: String) -> Self {
        Self { ticket }
    }
}

/// Captcha material which may be shown by a caller without being persisted by the SDK.
#[derive(Clone, Debug)]
pub struct CaptchaChallenge {
    pub id: String,
    pub url: Url,
    pub image: Vec<u8>,
}

/// Credentials for the Xiaomi cloud service-login protocol.
#[derive(Clone, Debug)]
pub struct CloudCredential {
    user_id: String,
    service_token: String,
    ssecurity: String,
}

impl CloudCredential {
    #[must_use]
    pub fn user_id(&self) -> &str {
        &self.user_id
    }
    #[must_use]
    pub fn service_token(&self) -> &str {
        &self.service_token
    }
    #[must_use]
    pub fn ssecurity(&self) -> &str {
        &self.ssecurity
    }
}

/// A cloud login result. Challenges contain no account password, cookies, or raw response data.
#[derive(Clone, Debug)]
pub enum CloudLoginOutcome {
    Success(CloudCredential),
    VerificationRequired { url: Url },
    CaptchaRequired(CaptchaChallenge),
}

/// Stateful Xiaomi cloud account login client. A client serializes one login challenge at a time.
#[derive(Debug)]
pub struct CloudLoginClient {
    client: reqwest::Client,
    region: String,
    sid: String,
    pending: Option<PendingLogin>,
}

#[derive(Debug)]
struct PendingLogin {
    account: String,
    password: String,
    context: LoginContext,
    captcha_ick: Option<String>,
    verification_url: Option<Url>,
}

#[derive(Clone, Debug, Default)]
struct LoginContext {
    callback: String,
    sid: String,
    qs: String,
    sign: String,
    ssecurity: Option<String>,
    user_id: Option<String>,
    location: Option<String>,
}

impl CloudLoginClient {
    pub fn new(
        region: impl Into<String>,
        sid: impl Into<String>,
        device_id: impl Into<String>,
    ) -> Result<Self, MiotError> {
        let region = region.into();
        let sid = sid.into();
        let device_id = device_id.into();
        if region.is_empty() || sid.is_empty() || device_id.is_empty() {
            return Err(MiotError::InvalidInput(
                "cloud configuration must not be empty",
            ));
        }
        let jar = Arc::new(reqwest::cookie::Jar::default());
        let account_url = Self::account_url("/")?;
        jar.add_cookie_str("sdkVersion=3.8.6", &account_url);
        jar.add_cookie_str(&format!("deviceId={device_id}"), &account_url);
        let user_agent = format!(
            "Android-7.1.1-1.0.0-ONEPLUS A3010-136-{device_id} APP/xiaomi.smarthome APPV/62830"
        );
        let client = reqwest::Client::builder()
            .cookie_provider(jar)
            .user_agent(user_agent)
            .build()?;
        Ok(Self {
            client,
            region,
            sid,
            pending: None,
        })
    }

    /// Returns the configured Xiaomi cloud region.
    #[must_use]
    pub fn region(&self) -> &str {
        &self.region
    }

    pub async fn login(
        &mut self,
        request: CloudLoginRequest,
    ) -> Result<CloudLoginOutcome, MiotError> {
        if request.account.is_empty() || request.password.is_empty() {
            return Err(MiotError::InvalidInput(
                "account and password must not be empty",
            ));
        }
        self.pending = None;
        let context = self.fetch_context().await?;
        self.pending = Some(PendingLogin {
            account: request.account,
            password: request.password,
            context,
            captcha_ick: None,
            verification_url: None,
        });
        self.authenticate(None).await
    }

    pub async fn continue_verification(
        &mut self,
        proof: VerificationProof,
    ) -> Result<CloudLoginOutcome, MiotError> {
        let verify_url = self
            .pending
            .as_ref()
            .and_then(|pending| pending.verification_url.clone())
            .ok_or(MiotError::VerificationRequired)?;
        let identity_url = verify_url
            .as_str()
            .replace("fe/service/identity/authStart", "identity/list");
        let identity_response = self.client.get(identity_url).send().await?;
        if !identity_response.status().is_success()
            || cookie_value(identity_response.headers(), "identity_session").is_none()
        {
            self.pending = None;
            return Err(MiotError::Authentication);
        }
        let identity: IdentityListResponse = decode_json(&identity_response.text().await?)?;
        let options = identity
            .options
            .unwrap_or_else(|| vec![identity.flag.unwrap_or(4)]);
        for flag in options {
            let path = match flag {
                4 => "/identity/auth/verifyPhone",
                8 => "/identity/auth/verifyEmail",
                _ => continue,
            };
            let response = self
                .client
                .post(Self::account_url(path)?)
                .query(&[("_dc", now_millis().as_str())])
                .form(&[
                    ("_flag", flag.to_string()),
                    ("ticket", proof.ticket.clone()),
                    ("trust", "false".to_owned()),
                    ("_json", "true".to_owned()),
                ])
                .send()
                .await?;
            if !response.status().is_success() {
                continue;
            }
            let verified: VerificationResponse = decode_json(&response.text().await?)?;
            if verified.code != 0 {
                continue;
            }
            let location = verified.location.ok_or(MiotError::Authentication)?;
            let initial = self.client.get(location).send().await?;
            let initial_token = cookie_value(initial.headers(), "serviceToken");
            let context = self.fetch_context().await?;
            let location = context.location.ok_or(MiotError::Authentication)?;
            return self
                .finish_location(location, context.ssecurity, context.user_id, initial_token)
                .await;
        }
        self.pending = None;
        Err(MiotError::Authentication)
    }

    pub async fn submit_captcha(
        &mut self,
        captcha: String,
    ) -> Result<CloudLoginOutcome, MiotError> {
        if captcha.is_empty() {
            return Err(MiotError::InvalidInput("captcha must not be empty"));
        }
        self.authenticate(Some(&captcha)).await
    }

    async fn fetch_context(&self) -> Result<LoginContext, MiotError> {
        let response = self
            .client
            .get(Self::account_url("/pass/serviceLogin")?)
            .query(&[("sid", self.sid.as_str()), ("_json", "true")])
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(MiotError::Protocol("cloud login context request failed"));
        }
        let body = response.text().await?;
        let value: ServiceLogin = decode_json(&body)?;
        if value.code != 0 {
            return Err(MiotError::Authentication);
        }
        Ok(LoginContext {
            callback: value.callback.unwrap_or_default(),
            sid: value.sid.unwrap_or_else(|| self.sid.clone()),
            qs: value.qs.unwrap_or_default(),
            sign: value.sign.unwrap_or_default(),
            ssecurity: value.ssecurity,
            user_id: value.user_id,
            location: value.location,
        })
    }

    async fn authenticate(
        &mut self,
        captcha: Option<&str>,
    ) -> Result<CloudLoginOutcome, MiotError> {
        let pending = self
            .pending
            .as_ref()
            .ok_or(MiotError::Protocol("no active cloud login"))?;
        let password_hash = format!("{:X}", Md5::digest(pending.password.as_bytes()));
        let mut form = vec![
            ("user", pending.account.as_str()),
            ("hash", password_hash.as_str()),
            ("callback", pending.context.callback.as_str()),
            ("sid", pending.context.sid.as_str()),
            ("qs", pending.context.qs.as_str()),
            ("_sign", pending.context.sign.as_str()),
        ];
        if let Some(code) = captcha {
            form.push(("captCode", code));
        }
        let mut request = self
            .client
            .post(Self::account_url("/pass/serviceLoginAuth2")?)
            .query(&[("_json", "true")])
            .form(&form);
        if captcha.is_some() {
            request = request.query(&[("_dc", now_millis().as_str())]);
            if let Some(ick) = &pending.captcha_ick {
                request = request.header(reqwest::header::COOKIE, format!("ick={ick}"));
            }
        }
        self.finish_auth_response(request.send().await?).await
    }

    async fn finish_auth_response(
        &mut self,
        response: reqwest::Response,
    ) -> Result<CloudLoginOutcome, MiotError> {
        if !response.status().is_success() {
            self.pending = None;
            return Err(MiotError::Authentication);
        }
        let body = response.text().await?;
        let auth: AuthResponse = decode_json(&body)?;
        if let Some(location) = auth.location {
            return self
                .finish_location(location, auth.ssecurity, auth.user_id, None)
                .await;
        }
        if let Some(notification) = auth.notification_url {
            let url = Self::absolute_url(&notification)?;
            if let Some(pending) = &mut self.pending {
                pending.verification_url = Some(url.clone());
            }
            return Ok(CloudLoginOutcome::VerificationRequired { url });
        }
        if let Some(captcha) = auth.captcha_url {
            let url = Self::absolute_url(&captcha)?;
            let image_response = self.client.get(url.clone()).send().await?;
            let ick = cookie_value(image_response.headers(), "ick");
            let image = image_response.bytes().await?.to_vec();
            let id = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(getrandom_bytes()?);
            if let Some(pending) = &mut self.pending {
                pending.captcha_ick = ick;
            }
            return Ok(CloudLoginOutcome::CaptchaRequired(CaptchaChallenge {
                id,
                url,
                image,
            }));
        }
        self.pending = None;
        Err(MiotError::Authentication)
    }

    fn account_url(path: &str) -> Result<Url, MiotError> {
        Url::parse(&format!("{ACCOUNT_BASE}{path}"))
            .map_err(|_| MiotError::Protocol("invalid account endpoint"))
    }
    fn absolute_url(value: &str) -> Result<Url, MiotError> {
        Url::parse(value)
            .or_else(|_| Url::parse(ACCOUNT_BASE)?.join(value))
            .map_err(|_| MiotError::Protocol("invalid cloud challenge URL"))
    }
    async fn finish_location(
        &mut self,
        location: String,
        ssecurity: Option<String>,
        user_id: Option<String>,
        initial_token: Option<String>,
    ) -> Result<CloudLoginOutcome, MiotError> {
        let pending = self
            .pending
            .take()
            .ok_or(MiotError::Protocol("cloud login state disappeared"))?;
        let location = self.add_client_sign(location, ssecurity.as_deref())?;
        let final_response = self.client.get(location).send().await?;
        let token = cookie_value(final_response.headers(), "serviceToken")
            .or(initial_token)
            .ok_or(MiotError::Authentication)?;
        let user_id = cookie_value(final_response.headers(), "userId")
            .or(user_id)
            .unwrap_or(pending.account);
        let ssecurity = ssecurity.ok_or(MiotError::Protocol(
            "cloud login response did not contain ssecurity",
        ))?;
        Ok(CloudLoginOutcome::Success(CloudCredential {
            user_id,
            service_token: token,
            ssecurity,
        }))
    }
    fn add_client_sign(
        &self,
        mut location: String,
        ssecurity: Option<&str>,
    ) -> Result<String, MiotError> {
        if self.sid == "xiaomiio" {
            return Ok(location);
        }
        let secret = ssecurity.ok_or(MiotError::Protocol(
            "cloud login response did not contain ssecurity",
        ))?;
        let nonce = Url::parse(&location)
            .ok()
            .and_then(|url| {
                url.query_pairs()
                    .find(|(key, _)| key == "nonce")
                    .map(|(_, value)| value.into_owned())
            })
            .ok_or(MiotError::Protocol(
                "cloud login response did not contain nonce",
            ))?;
        let mut digest = Sha1::new();
        digest.update(format!("nonce={nonce}&{secret}"));
        let signature = base64::engine::general_purpose::STANDARD.encode(digest.finalize());
        let mut url =
            Url::parse(&location).map_err(|_| MiotError::Protocol("invalid cloud redirect"))?;
        url.query_pairs_mut().append_pair("clientSign", &signature);
        location = url.into();
        Ok(location)
    }
}

#[derive(Deserialize)]
struct ServiceLogin {
    code: i64,
    callback: Option<String>,
    sid: Option<String>,
    qs: Option<String>,
    #[serde(rename = "_sign")]
    sign: Option<String>,
    ssecurity: Option<String>,
    #[serde(rename = "userId")]
    user_id: Option<String>,
    location: Option<String>,
}
#[derive(Deserialize)]
struct AuthResponse {
    location: Option<String>,
    #[serde(rename = "notificationUrl")]
    notification_url: Option<String>,
    #[serde(rename = "captchaUrl")]
    captcha_url: Option<String>,
    #[serde(rename = "userId")]
    user_id: Option<String>,
    ssecurity: Option<String>,
}
#[derive(Deserialize)]
struct IdentityListResponse {
    flag: Option<i64>,
    options: Option<Vec<i64>>,
}
#[derive(Deserialize)]
struct VerificationResponse {
    code: i64,
    location: Option<String>,
}

fn decode_json<T: serde::de::DeserializeOwned>(text: &str) -> Result<T, MiotError> {
    serde_json::from_str(text.trim_start_matches("&&&START&&&"))
        .map_err(|_| MiotError::Protocol("cloud login returned an invalid response"))
}
fn cookie_value(headers: &reqwest::header::HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .find_map(|value| {
            let text = value.to_str().ok()?;
            text.strip_prefix(&format!("{name}="))?
                .split(';')
                .next()
                .map(ToOwned::to_owned)
        })
}
fn getrandom_bytes() -> Result<[u8; 16], MiotError> {
    let mut bytes = [0; 16];
    getrandom::fill(&mut bytes)
        .map_err(|_| MiotError::Protocol("secure random generation failed"))?;
    Ok(bytes)
}
fn now_millis() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .to_string()
}
