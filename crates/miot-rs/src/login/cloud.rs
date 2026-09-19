#![allow(clippy::missing_errors_doc)]

use std::sync::Arc;

use base64::Engine;
use md5::{Digest, Md5};
use reqwest::cookie::CookieStore;
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

/// A Xiaomi QR login challenge. Scan the image URL with the Xiaomi Home app, then wait for the
/// login result on the same client instance.
#[derive(Clone, Debug)]
pub struct QrLoginChallenge {
    pub login_url: Url,
    pub image_url: Option<Url>,
}

impl VerificationProof {
    #[must_use]
    pub fn new(ticket: String) -> Self {
        trace_entry("VerificationProof::new");
        Self { ticket }
    }
}

/// Captcha material which may be shown by a caller without being persisted by the SDK.
#[derive(Clone, Debug)]
pub struct CaptchaChallenge {
    pub id: String,
    pub url: Url,
    pub image: Vec<u8>,
    /// Whether the preceding CAPTCHA answer was rejected and this is a replacement challenge.
    pub rejected_previous_answer: bool,
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
        trace_entry("CloudCredential::user_id");
        &self.user_id
    }
    #[must_use]
    pub fn service_token(&self) -> &str {
        trace_entry("CloudCredential::service_token");
        &self.service_token
    }
    #[must_use]
    pub fn ssecurity(&self) -> &str {
        trace_entry("CloudCredential::ssecurity");
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
    jar: Arc<reqwest::cookie::Jar>,
    region: String,
    sid: String,
    pending: Option<PendingLogin>,
    qr_pending: Option<QrLoginPending>,
}

#[derive(Debug)]
struct PendingLogin {
    account: String,
    password: String,
    context: LoginContext,
    captcha_ick: Option<String>,
    verification_url: Option<Url>,
}

#[derive(Debug)]
struct QrLoginPending {
    poll_url: Url,
}

#[derive(Clone, Debug, Default)]
struct LoginContext {
    callback: String,
    sid: String,
    qs: String,
    sign: String,
    ssecurity: Option<String>,
    nonce: Option<String>,
    user_id: Option<String>,
    cuser_id: Option<String>,
    pass_token: Option<String>,
    location: Option<String>,
}

impl CloudLoginClient {
    pub fn new(
        region: impl Into<String>,
        sid: impl Into<String>,
        device_id: impl Into<String>,
    ) -> Result<Self, MiotError> {
        trace_entry("CloudLoginClient::new");
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
            .cookie_provider(Arc::clone(&jar))
            .user_agent(user_agent)
            .build()?;
        Ok(Self {
            client,
            jar,
            region,
            sid,
            pending: None,
            qr_pending: None,
        })
    }

    /// Returns the configured Xiaomi cloud region.
    #[must_use]
    pub fn region(&self) -> &str {
        trace_entry("CloudLoginClient::region");
        &self.region
    }

    pub async fn login(
        &mut self,
        request: CloudLoginRequest,
    ) -> Result<CloudLoginOutcome, MiotError> {
        trace_entry("CloudLoginClient::login");
        if request.account.is_empty() || request.password.is_empty() {
            return Err(MiotError::InvalidInput(
                "account and password must not be empty",
            ));
        }
        self.pending = None;
        self.qr_pending = None;
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

    /// Starts a Xiaomi Home QR login transaction.
    pub async fn begin_qr_login(&mut self) -> Result<QrLoginChallenge, MiotError> {
        trace_entry("CloudLoginClient::begin_qr_login");
        self.pending = None;
        self.qr_pending = None;
        let context = self.fetch_context_for_sid("mijia").await?;
        let mut query = vec![
            ("theme", String::new()),
            ("bizDeviceType", String::new()),
            ("_hasLogo", "false".to_owned()),
            ("_qrsize", "240".to_owned()),
            ("_dc", now_millis()),
            ("sid", context.sid),
            ("qs", context.qs),
            ("_sign", context.sign),
            ("callback", context.callback),
        ];
        if let Some(ssecurity) = context.ssecurity {
            query.push(("ssecurity", ssecurity));
        }
        if let Some(nonce) = context.nonce {
            query.push(("nonce", nonce));
        }
        if let Some(user_id) = context.user_id {
            query.push(("userId", user_id));
        }
        if let Some(cuser_id) = context.cuser_id {
            query.push(("cUserId", cuser_id));
        }
        if let Some(pass_token) = context.pass_token {
            query.push(("passToken", pass_token));
        }
        if let Some(location) = context.location {
            query.push(("location", location));
        }
        let response = self
            .client
            .get(Self::account_url("/longPolling/loginUrl")?)
            .query(&query)
            .send()
            .await?;
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            return Err(authentication_response(status, &body));
        }
        let challenge: QrLoginStartResponse = decode_json(&body)?;
        if challenge.code != 0 {
            return Err(authentication_response(status, &body));
        }
        let login_url = Self::absolute_url(&challenge.login_url)?;
        let poll_url = Self::absolute_url(&challenge.poll_url)?;
        let image_url = challenge
            .image_url
            .as_deref()
            .map(Self::absolute_url)
            .transpose()?;
        self.qr_pending = Some(QrLoginPending { poll_url });
        Ok(QrLoginChallenge {
            login_url,
            image_url,
        })
    }

    /// Waits for the active QR login transaction to be confirmed in Xiaomi Home.
    pub async fn wait_for_qr_login(&mut self) -> Result<CloudLoginOutcome, MiotError> {
        trace_entry("CloudLoginClient::wait_for_qr_login");
        let pending = self
            .qr_pending
            .take()
            .ok_or(MiotError::Protocol("no active QR login"))?;
        let response = self
            .client
            .get(pending.poll_url)
            .timeout(std::time::Duration::from_secs(130))
            .send()
            .await?;
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            return Err(authentication_response(status, &body));
        }
        let auth: AuthResponse = decode_json(&body)?;
        if auth.code != Some(0) {
            return Err(authentication_response(status, &body));
        }
        let location = auth.location.ok_or(MiotError::Protocol(
            "QR login response did not contain location",
        ))?;
        self.finish_qr_location(
            location,
            auth.ssecurity,
            json_string_or_number(auth.user_id),
            auth.nonce,
        )
        .await
    }

    #[allow(clippy::too_many_lines)]
    pub async fn continue_verification(
        &mut self,
        proof: VerificationProof,
    ) -> Result<CloudLoginOutcome, MiotError> {
        trace_entry("CloudLoginClient::continue_verification");
        let verify_url = self
            .pending
            .as_ref()
            .and_then(|pending| pending.verification_url.clone())
            .ok_or(MiotError::VerificationRequired)?;
        let identity_url = verify_url
            .as_str()
            .replace("fe/service/identity/authStart", "identity/list");
        let identity_response = self.client.get(identity_url).send().await?;
        let identity_status = identity_response.status();
        let identity_session = cookie_value(identity_response.headers(), "identity_session");
        let identity_body = identity_response.text().await?;
        if !identity_status.is_success() || identity_session.is_none() {
            self.pending = None;
            return Err(authentication_response(identity_status, &identity_body));
        }
        if let Some(identity_session) = identity_session {
            self.jar.add_cookie_str(
                &format!("identity_session={identity_session}"),
                &Self::account_url("/")?,
            );
        }
        let identity: IdentityListResponse = decode_json(&identity_body)?;
        let options = identity
            .options
            .unwrap_or_else(|| vec![identity.flag.unwrap_or(4)]);
        println!("cloud_login verification_options: {options:?}");
        let verification_cookie = self.verification_cookie_header()?;
        let mut last_failure = None;
        for flag in options {
            let path = match flag {
                4 => "/identity/auth/verifyPhone",
                8 => "/identity/auth/verifyEmail",
                _ => continue,
            };
            println!("cloud_login verification_method: flag={flag}, path={path}");
            let mut request = self
                .client
                .post(Self::account_url(path)?)
                .query(&[("_dc", now_millis().as_str())])
                .form(&[
                    ("_flag", flag.to_string()),
                    ("ticket", proof.ticket.clone()),
                    ("trust", "false".to_owned()),
                    ("_json", "true".to_owned()),
                ]);
            if let Some(cookie) = &verification_cookie {
                request = request.header(reqwest::header::COOKIE, cookie);
            }
            let response = request.send().await?;
            let status = response.status();
            let body = response.text().await?;
            if !status.is_success() {
                self.pending = None;
                return Err(authentication_response(status, &body));
            }
            let verified: VerificationResponse = decode_json(&body)?;
            println!(
                "cloud_login verification_response: flag={flag}, code={}",
                verified.code
            );
            if verified.code != 0 {
                last_failure = Some((status, body));
                continue;
            }
            let location = verified
                .location
                .filter(|location| !location.is_empty())
                .ok_or(MiotError::Authentication)?;
            let initial = self.client.get(location).send().await?;
            let initial_status = initial.status();
            let initial_url = initial.url().clone();
            let initial_body = initial.text().await?;
            if !initial_status.is_success() {
                self.pending = None;
                return Err(authentication_response(initial_status, &initial_body));
            }
            if let Some(skip_url) = confirm_phone_skip_url(&initial_url)? {
                let skipped = self.client.get(skip_url).send().await?;
                let skipped_status = skipped.status();
                let skipped_body = skipped.text().await?;
                if !skipped_status.is_success() {
                    self.pending = None;
                    return Err(authentication_response(skipped_status, &skipped_body));
                }
            }
            let context = self.fetch_context().await?;
            let location = context
                .location
                .filter(|location| !location.is_empty())
                .ok_or(MiotError::Authentication)?;
            return self
                .finish_location(location, context.ssecurity, context.user_id, None)
                .await;
        }
        self.pending = None;
        if let Some((status, body)) = last_failure {
            return Err(authentication_response(status, &body));
        }
        Err(MiotError::Authentication)
    }

    pub async fn submit_captcha(
        &mut self,
        captcha: String,
    ) -> Result<CloudLoginOutcome, MiotError> {
        trace_entry("CloudLoginClient::submit_captcha");
        if captcha.is_empty() {
            return Err(MiotError::InvalidInput("captcha must not be empty"));
        }
        self.authenticate(Some(&captcha)).await
    }

    async fn fetch_context(&self) -> Result<LoginContext, MiotError> {
        trace_entry("CloudLoginClient::fetch_context");
        self.fetch_context_for_sid(&self.sid).await
    }

    async fn fetch_context_for_sid(&self, sid: &str) -> Result<LoginContext, MiotError> {
        trace_entry("CloudLoginClient::fetch_context_for_sid");
        let response = self
            .client
            .get(Self::account_url("/pass/serviceLogin")?)
            .query(&[("sid", sid), ("_json", "true")])
            .send()
            .await?;
        let status = response.status();
        let body = response.text().await?;
        println!("cloud_login service_login_status: {status}");
        println!("cloud_login service_login_response: {body}");
        if !status.is_success() {
            return Err(authentication_response(status, &body));
        }
        let value: ServiceLogin = decode_json(&body)?;
        Ok(LoginContext {
            callback: value.callback.unwrap_or_default(),
            sid: value.sid.unwrap_or_else(|| self.sid.clone()),
            qs: value.qs.unwrap_or_default(),
            sign: value.sign.unwrap_or_default(),
            ssecurity: value.ssecurity,
            nonce: json_string_or_number(value.nonce),
            user_id: json_string_or_number(value.user_id),
            cuser_id: json_string_or_number(value.cuser_id),
            pass_token: value.pass_token,
            location: value.location,
        })
    }

    fn verification_cookie_header(&self) -> Result<Option<String>, MiotError> {
        trace_entry("CloudLoginClient::verification_cookie_header");
        let account_url = Self::account_url("/")?;
        let cookie_header = self
            .jar
            .cookies(&account_url)
            .and_then(|header| header.to_str().ok().map(ToOwned::to_owned));
        let cookie_names = cookie_header.as_deref().map_or_else(Vec::new, |header| {
            header
                .split(';')
                .filter_map(|cookie| cookie.trim().split_once('=').map(|(name, _)| name))
                .collect::<Vec<_>>()
        });
        println!("cloud_login verification_cookie_names: {cookie_names:?}");
        Ok(cookie_header)
    }

    async fn authenticate(
        &mut self,
        captcha: Option<&str>,
    ) -> Result<CloudLoginOutcome, MiotError> {
        trace_entry("CloudLoginClient::authenticate");
        let pending = self
            .pending
            .as_ref()
            .ok_or(MiotError::Protocol("no active cloud login"))?;
        let password_hash = format!("{:X}", Md5::digest(pending.password.as_bytes()));
        println!("password_md5: {password_hash}");
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
                // Put the CAPTCHA cookie into the jar instead of setting Cookie directly:
                // a direct header would suppress the device and login-session cookies.
                self.jar
                    .add_cookie_str(&format!("ick={ick}"), &Self::account_url("/")?);
            }
        }
        self.finish_auth_response(request.send().await?).await
    }

    async fn finish_auth_response(
        &mut self,
        response: reqwest::Response,
    ) -> Result<CloudLoginOutcome, MiotError> {
        trace_entry("CloudLoginClient::finish_auth_response");
        let password_md5 = self
            .pending
            .as_ref()
            .map(|pending| format!("{:X}", Md5::digest(pending.password.as_bytes())));
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            self.pending = None;
            return Err(authentication_response_with_password_md5(
                status,
                &body,
                password_md5,
            ));
        }
        let auth: AuthResponse = decode_json(&body)?;
        let user_id = json_string_or_number(auth.user_id);
        if let Some(location) = auth.location.filter(|location| !location.is_empty()) {
            return self
                .finish_location(location, auth.ssecurity, user_id, auth.nonce)
                .await;
        }
        if let Some(notification) = auth.notification_url.filter(|url| !url.is_empty()) {
            let url = Self::absolute_url(&notification)?;
            if let Some(pending) = &mut self.pending {
                pending.verification_url = Some(url.clone());
            }
            return Ok(CloudLoginOutcome::VerificationRequired { url });
        }
        if let Some(captcha) = auth.captcha_url.filter(|url| !url.is_empty()) {
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
                rejected_previous_answer: auth.code == Some(87_001),
            }));
        }
        if auth.code == Some(81_003) {
            if let Some(url) = self
                .pending
                .as_ref()
                .and_then(|pending| pending.verification_url.clone())
            {
                return Ok(CloudLoginOutcome::VerificationRequired { url });
            }
        }
        self.pending = None;
        Err(authentication_response_with_password_md5(
            status,
            &body,
            password_md5,
        ))
    }

    fn account_url(path: &str) -> Result<Url, MiotError> {
        trace_entry("CloudLoginClient::account_url");
        Url::parse(&format!("{ACCOUNT_BASE}{path}"))
            .map_err(|_| MiotError::Protocol("invalid account endpoint"))
    }
    fn absolute_url(value: &str) -> Result<Url, MiotError> {
        trace_entry("CloudLoginClient::absolute_url");
        Url::parse(value)
            .or_else(|_| Url::parse(ACCOUNT_BASE)?.join(value))
            .map_err(|_| MiotError::Protocol("invalid cloud challenge URL"))
    }
    async fn finish_location(
        &mut self,
        location: String,
        ssecurity: Option<String>,
        user_id: Option<String>,
        nonce: Option<String>,
    ) -> Result<CloudLoginOutcome, MiotError> {
        trace_entry("CloudLoginClient::finish_location");
        let pending = self
            .pending
            .take()
            .ok_or(MiotError::Protocol("cloud login state disappeared"))?;
        let location = self.add_client_sign(location, ssecurity.as_deref(), nonce.as_deref())?;
        println!("cloud_login final_location: {location}");
        let final_response = self
            .client
            .get(location)
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            .send()
            .await?;
        let status = final_response.status();
        let token = cookie_value(final_response.headers(), "serviceToken");
        let response_user_id = cookie_value(final_response.headers(), "userId");
        let body = final_response.text().await?;
        if !status.is_success() {
            return Err(authentication_response(status, &body));
        }
        let token = token.ok_or_else(|| authentication_response(status, &body))?;
        let user_id = response_user_id.or(user_id).unwrap_or(pending.account);
        let ssecurity = ssecurity.ok_or(MiotError::Protocol(
            "cloud login response did not contain ssecurity",
        ))?;
        Ok(CloudLoginOutcome::Success(CloudCredential {
            user_id,
            service_token: token,
            ssecurity,
        }))
    }

    async fn finish_qr_location(
        &self,
        location: String,
        ssecurity: Option<String>,
        user_id: Option<String>,
        nonce: Option<String>,
    ) -> Result<CloudLoginOutcome, MiotError> {
        trace_entry("CloudLoginClient::finish_qr_location");
        let location = self.add_client_sign(location, ssecurity.as_deref(), nonce.as_deref())?;
        let final_response = self
            .client
            .get(location)
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            .send()
            .await?;
        let status = final_response.status();
        let token = cookie_value(final_response.headers(), "serviceToken");
        let response_user_id = cookie_value(final_response.headers(), "userId");
        let body = final_response.text().await?;
        if !status.is_success() {
            return Err(authentication_response(status, &body));
        }
        let service_token = token.ok_or_else(|| authentication_response(status, &body))?;
        let user_id = response_user_id.or(user_id).ok_or(MiotError::Protocol(
            "QR login response did not contain user ID",
        ))?;
        let ssecurity = ssecurity.ok_or(MiotError::Protocol(
            "QR login response did not contain ssecurity",
        ))?;
        Ok(CloudLoginOutcome::Success(CloudCredential {
            user_id,
            service_token,
            ssecurity,
        }))
    }
    fn add_client_sign(
        &self,
        mut location: String,
        ssecurity: Option<&str>,
        response_nonce: Option<&str>,
    ) -> Result<String, MiotError> {
        trace_entry("CloudLoginClient::add_client_sign");
        if self.sid == "xiaomiio" {
            return Ok(location);
        }
        let secret = ssecurity.ok_or(MiotError::Protocol(
            "cloud login response did not contain ssecurity",
        ))?;
        let nonce = response_nonce
            .map(ToOwned::to_owned)
            .or_else(|| {
                Url::parse(&location).ok().and_then(|url| {
                    url.query_pairs()
                        .find(|(key, _)| key == "nonce")
                        .map(|(_, value)| value.into_owned())
                })
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
    callback: Option<String>,
    sid: Option<String>,
    qs: Option<String>,
    #[serde(rename = "_sign")]
    sign: Option<String>,
    ssecurity: Option<String>,
    nonce: Option<serde_json::Value>,
    #[serde(rename = "userId")]
    user_id: Option<serde_json::Value>,
    #[serde(rename = "cUserId")]
    cuser_id: Option<serde_json::Value>,
    #[serde(rename = "passToken")]
    pass_token: Option<String>,
    location: Option<String>,
}
#[derive(Deserialize)]
struct AuthResponse {
    code: Option<i64>,
    location: Option<String>,
    #[serde(rename = "notificationUrl")]
    notification_url: Option<String>,
    #[serde(rename = "captchaUrl")]
    captcha_url: Option<String>,
    #[serde(rename = "userId")]
    user_id: Option<serde_json::Value>,
    ssecurity: Option<String>,
    nonce: Option<String>,
}
#[derive(Deserialize)]
struct QrLoginStartResponse {
    code: i64,
    #[serde(rename = "loginUrl")]
    login_url: String,
    #[serde(rename = "qr")]
    image_url: Option<String>,
    #[serde(rename = "lp")]
    poll_url: String,
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
    trace_entry("decode_json");
    serde_json::from_str(text.trim_start_matches("&&&START&&&"))
        .map_err(|_| MiotError::Protocol("cloud login returned an invalid response"))
}

fn json_string_or_number(value: Option<serde_json::Value>) -> Option<String> {
    trace_entry("json_string_or_number");
    match value? {
        serde_json::Value::String(value) => Some(value),
        serde_json::Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}
fn cookie_value(headers: &reqwest::header::HeaderMap, name: &str) -> Option<String> {
    trace_entry("cookie_value");
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
    trace_entry("getrandom_bytes");
    let mut bytes = [0; 16];
    getrandom::fill(&mut bytes)
        .map_err(|_| MiotError::Protocol("secure random generation failed"))?;
    Ok(bytes)
}
fn now_millis() -> String {
    trace_entry("now_millis");
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .to_string()
}

fn authentication_response(status: reqwest::StatusCode, body: &str) -> MiotError {
    trace_entry("authentication_response");
    authentication_response_with_password_md5(status, body, None)
}

fn authentication_response_with_password_md5(
    status: reqwest::StatusCode,
    body: &str,
    password_md5: Option<String>,
) -> MiotError {
    trace_entry("authentication_response_with_password_md5");
    let body = body.trim_start_matches("&&&START&&&");
    let value = serde_json::from_str::<serde_json::Value>(body).ok();
    let code = value
        .as_ref()
        .and_then(|value| value.get("code"))
        .and_then(serde_json::Value::as_i64);
    let response = value.map_or_else(|| body.to_owned(), |value| value.to_string());
    MiotError::AuthenticationResponse {
        status: status.as_u16(),
        code,
        response: response.chars().take(4096).collect(),
        password_md5,
    }
}

fn confirm_phone_skip_url(url: &Url) -> Result<Option<Url>, MiotError> {
    trace_entry("confirm_phone_skip_url");
    if !url.path().starts_with("/fe/") {
        return Ok(None);
    }
    let Some((_, value)) = url.query_pairs().find(|(key, _)| key == "skipUrl") else {
        return Ok(None);
    };
    CloudLoginClient::absolute_url(&value)
        .map(Some)
        .map_err(|_| MiotError::Protocol("invalid cloud verification skip URL"))
}

fn trace_entry(function: &str) {
    println!("cloud_login entered: {function}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_login_context_is_usable_when_server_returns_70016() {
        let response: ServiceLogin = decode_json(
            r#"{"code":70016,"callback":"https://sts.api.io.mi.com/sts","sid":"xiaomiio","qs":"%3Fsid%3Dxiaomiio","_sign":"sign"}"#,
        )
        .expect("service-login response parses");
        assert_eq!(
            response.callback.as_deref(),
            Some("https://sts.api.io.mi.com/sts")
        );
        assert_eq!(response.sid.as_deref(), Some("xiaomiio"));
        assert_eq!(response.qs.as_deref(), Some("%3Fsid%3Dxiaomiio"));
        assert_eq!(response.sign.as_deref(), Some("sign"));
    }

    #[test]
    fn extracts_confirm_phone_skip_url() {
        let url = Url::parse(
            "https://account.xiaomi.com/fe/identity/result/check?skipUrl=%2Fpass%2Fcontinue%3Fa%3Db",
        )
        .expect("valid URL");
        assert_eq!(
            confirm_phone_skip_url(&url)
                .expect("valid skip URL")
                .expect("skip URL present")
                .as_str(),
            "https://account.xiaomi.com/pass/continue?a=b"
        );
    }

    #[test]
    fn ignores_skip_url_outside_frontend_flow() {
        let url = Url::parse("https://account.xiaomi.com/pass/continue?skipUrl=%2Fnext")
            .expect("valid URL");
        assert!(confirm_phone_skip_url(&url).expect("valid URL").is_none());
    }

    #[test]
    fn client_sign_prefers_auth_response_nonce() {
        let client = CloudLoginClient::new("cn", "micoapi", "device-id").expect("valid client");
        let signed = client
            .add_client_sign(
                "https://example.test/callback?nonce=location-nonce".to_owned(),
                Some("c2VjdXJpdHk="),
                Some("response-nonce"),
            )
            .expect("client sign");
        let url = Url::parse(&signed).expect("valid signed URL");
        let signature = url
            .query_pairs()
            .find(|(key, _)| key == "clientSign")
            .map(|(_, value)| value.into_owned())
            .expect("client sign present");
        let expected = base64::engine::general_purpose::STANDARD
            .encode(Sha1::digest(b"nonce=response-nonce&c2VjdXJpdHk="));
        assert_eq!(signature, expected);
    }

    #[test]
    fn authentication_error_includes_response_body_for_debugging() {
        let error = authentication_response(
            reqwest::StatusCode::UNAUTHORIZED,
            r#"{"code":70016,"serviceToken":"secret"}"#,
        );
        assert_eq!(
            error.to_string(),
            "authentication failed (HTTP 401, server code 70016): {\"code\":70016,\"serviceToken\":\"secret\"}"
        );
    }
}
