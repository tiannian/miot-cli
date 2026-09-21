#![allow(clippy::missing_errors_doc)]

use std::sync::Arc;

use base64::Engine;
use md5::{Digest, Md5};
use reqwest::cookie::CookieStore;
use serde::Deserialize;
use sha1::Sha1;
use url::Url;

use super::super::MiotError;
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
    pub(super) user_id: String,
    pub(super) service_token: String,
    pub(super) ssecurity: String,
}

impl CloudCredential {
    /// Rebuilds a cloud credential from values previously stored by an application.
    #[must_use]
    pub fn new(
        user_id: impl Into<String>,
        service_token: impl Into<String>,
        ssecurity: impl Into<String>,
    ) -> Self {
        Self {
            user_id: user_id.into(),
            service_token: service_token.into(),
            ssecurity: ssecurity.into(),
        }
    }

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
    pub(super) client: reqwest::Client,
    jar: Arc<reqwest::cookie::Jar>,
    region: String,
    sid: String,
    pub(super) pending: Option<PendingLogin>,
}

#[derive(Debug)]
pub(super) struct PendingLogin {
    account: String,
    password: String,
    context: LoginContext,
    captcha_ick: Option<String>,
    verification_url: Option<Url>,
}

#[derive(Clone, Debug, Default)]
pub(super) struct LoginContext {
    pub(super) callback: String,
    pub(super) sid: String,
    pub(super) qs: String,
    pub(super) sign: String,
    pub(super) ssecurity: Option<String>,
    pub(super) nonce: Option<String>,
    pub(super) user_id: Option<String>,
    pub(super) cuser_id: Option<String>,
    pub(super) pass_token: Option<String>,
    pub(super) location: Option<String>,
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
        let account_url = account_url("/")?;
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
                &account_url("/")?,
            );
        }
        let identity: IdentityListResponse = decode_json(&identity_body)?;
        let options = identity
            .options
            .unwrap_or_else(|| vec![identity.flag.unwrap_or(4)]);
        let verification_cookie = self.verification_cookie_header()?;
        let mut last_failure = None;
        for flag in options {
            let Some((path, form)) = verification_request(flag, &proof.ticket) else {
                continue;
            };
            let mut request = self
                .client
                .post(account_url(path)?)
                .query(&[("_dc", now_millis().as_str())])
                .form(&form);
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

    pub(super) async fn fetch_context_for_sid(&self, sid: &str) -> Result<LoginContext, MiotError> {
        trace_entry("CloudLoginClient::fetch_context_for_sid");
        fetch_service_login_context(&self.client, sid, &self.sid).await
    }

    fn verification_cookie_header(&self) -> Result<Option<String>, MiotError> {
        trace_entry("CloudLoginClient::verification_cookie_header");
        let account_url = account_url("/")?;
        let cookie_header = self
            .jar
            .cookies(&account_url)
            .and_then(|header| header.to_str().ok().map(ToOwned::to_owned));
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
            .post(account_url("/pass/serviceLoginAuth2")?)
            .query(&[("_json", "true")])
            .form(&form);
        if captcha.is_some() {
            request = request.query(&[("_dc", now_millis().as_str())]);
            if let Some(ick) = &pending.captcha_ick {
                // Put the CAPTCHA cookie into the jar instead of setting Cookie directly:
                // a direct header would suppress the device and login-session cookies.
                self.jar
                    .add_cookie_str(&format!("ick={ick}"), &account_url("/")?);
            }
        }
        self.finish_auth_response(request.send().await?).await
    }

    async fn finish_auth_response(
        &mut self,
        response: reqwest::Response,
    ) -> Result<CloudLoginOutcome, MiotError> {
        trace_entry("CloudLoginClient::finish_auth_response");
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            self.pending = None;
            return Err(authentication_response(status, &body));
        }
        let auth: AuthResponse = decode_json(&body)?;
        let user_id = json_string_or_number(auth.user_id);
        if let Some(location) = auth.location.filter(|location| !location.is_empty()) {
            return self
                .finish_location(
                    location,
                    auth.ssecurity,
                    user_id,
                    json_string_or_number(auth.nonce),
                )
                .await;
        }
        if let Some(notification) = auth.notification_url.filter(|url| !url.is_empty()) {
            let url = absolute_url(&notification)?;
            if let Some(pending) = &mut self.pending {
                pending.verification_url = Some(url.clone());
            }
            return Ok(CloudLoginOutcome::VerificationRequired { url });
        }
        if let Some(captcha) = auth.captcha_url.filter(|url| !url.is_empty()) {
            let url = absolute_url(&captcha)?;
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
        Err(authentication_response(status, &body))
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
        let location =
            add_client_sign(&self.sid, location, ssecurity.as_deref(), nonce.as_deref())?;
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
}

pub(super) fn account_url(path: &str) -> Result<Url, MiotError> {
    trace_entry("account_url");
    Url::parse(&format!("{ACCOUNT_BASE}{path}"))
        .map_err(|_| MiotError::Protocol("invalid account endpoint"))
}

pub(super) fn absolute_url(value: &str) -> Result<Url, MiotError> {
    trace_entry("absolute_url");
    Url::parse(value)
        .or_else(|_| Url::parse(ACCOUNT_BASE)?.join(value))
        .map_err(|_| MiotError::Protocol("invalid cloud challenge URL"))
}

pub(super) fn add_client_sign(
    sid: &str,
    location: String,
    ssecurity: Option<&str>,
    response_nonce: Option<&str>,
) -> Result<String, MiotError> {
    trace_entry("add_client_sign");
    if sid == "xiaomiio" {
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
    Ok(url.into())
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

pub(super) async fn fetch_service_login_context(
    client: &reqwest::Client,
    sid: &str,
    default_sid: &str,
) -> Result<LoginContext, MiotError> {
    let response = client
        .get(account_url("/pass/serviceLogin")?)
        .query(&[("sid", sid), ("_json", "true")])
        .send()
        .await?;
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        return Err(authentication_response(status, &body));
    }
    let value: ServiceLogin = decode_json(&body)?;
    Ok(LoginContext {
        callback: value.callback.unwrap_or_default(),
        sid: value.sid.unwrap_or_else(|| default_sid.to_owned()),
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
#[derive(Deserialize)]
pub(super) struct AuthResponse {
    pub(super) code: Option<i64>,
    pub(super) location: Option<String>,
    #[serde(rename = "notificationUrl")]
    pub(super) notification_url: Option<String>,
    #[serde(rename = "captchaUrl")]
    pub(super) captcha_url: Option<String>,
    #[serde(rename = "userId")]
    pub(super) user_id: Option<serde_json::Value>,
    pub(super) ssecurity: Option<String>,
    pub(super) nonce: Option<serde_json::Value>,
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

fn verification_request(
    flag: i64,
    ticket: &str,
) -> Option<(&'static str, Vec<(&'static str, String)>)> {
    let common = vec![("_flag", flag.to_string()), ("ticket", ticket.to_owned())];
    match flag {
        4 => Some((
            "/identity/auth/verifyPhone",
            common
                .into_iter()
                .chain([("trust", "false".to_owned()), ("_json", "true".to_owned())])
                .collect(),
        )),
        8 => Some((
            "/identity/auth/verifyEmail",
            common
                .into_iter()
                .chain([("trust", "false".to_owned()), ("_json", "true".to_owned())])
                .collect(),
        )),
        33_554_432 => Some((
            "/identity/auth/deviceAuth",
            common
                .into_iter()
                .chain([("type", "vcode".to_owned()), ("_json", "true".to_owned())])
                .collect(),
        )),
        _ => None,
    }
}

pub(super) fn decode_json<T: serde::de::DeserializeOwned>(text: &str) -> Result<T, MiotError> {
    trace_entry("decode_json");
    serde_json::from_str(text.trim_start_matches("&&&START&&&"))
        .map_err(|_| MiotError::Protocol("cloud login returned an invalid response"))
}

pub(super) fn json_string_or_number(value: Option<serde_json::Value>) -> Option<String> {
    trace_entry("json_string_or_number");
    match value? {
        serde_json::Value::String(value) => Some(value),
        serde_json::Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}
pub(super) fn cookie_value(headers: &reqwest::header::HeaderMap, name: &str) -> Option<String> {
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
pub(super) fn now_millis() -> String {
    trace_entry("now_millis");
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .to_string()
}

pub(super) fn authentication_response(status: reqwest::StatusCode, body: &str) -> MiotError {
    trace_entry("authentication_response");
    let body = body.trim_start_matches("&&&START&&&");
    let value = serde_json::from_str::<serde_json::Value>(body).ok();
    let code = value
        .as_ref()
        .and_then(|value| value.get("code"))
        .and_then(serde_json::Value::as_i64);
    let response = value.map_or_else(|| body.to_owned(), redact_sensitive_values);
    MiotError::AuthenticationResponse {
        status: status.as_u16(),
        code,
        response: response.chars().take(4096).collect(),
    }
}

fn redact_sensitive_values(mut value: serde_json::Value) -> String {
    redact_sensitive_value(&mut value);
    value.to_string()
}

fn redact_sensitive_value(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(object) => {
            for (key, value) in object {
                if is_sensitive_key(key) {
                    *value = serde_json::Value::String("[REDACTED]".to_owned());
                } else {
                    redact_sensitive_value(value);
                }
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                redact_sensitive_value(value);
            }
        }
        _ => {}
    }
}

fn is_sensitive_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    [
        "token",
        "password",
        "secret",
        "security",
        "cookie",
        "authorization",
        "ticket",
        "captcha",
        "nonce",
        "sign",
    ]
    .iter()
    .any(|needle| key.contains(needle))
}

fn confirm_phone_skip_url(url: &Url) -> Result<Option<Url>, MiotError> {
    trace_entry("confirm_phone_skip_url");
    if !url.path().starts_with("/fe/") {
        return Ok(None);
    }
    let Some((_, value)) = url.query_pairs().find(|(key, _)| key == "skipUrl") else {
        return Ok(None);
    };
    absolute_url(&value)
        .map(Some)
        .map_err(|_| MiotError::Protocol("invalid cloud verification skip URL"))
}

pub(super) fn trace_entry(function: &str) {
    let _ = function;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_confirmation_uses_device_auth_payload() {
        let (path, form) =
            verification_request(33_554_432, "638922").expect("device confirmation is supported");
        assert_eq!(path, "/identity/auth/deviceAuth");
        assert_eq!(
            form,
            vec![
                ("_flag", "33554432".to_owned()),
                ("ticket", "638922".to_owned()),
                ("type", "vcode".to_owned()),
                ("_json", "true".to_owned()),
            ]
        );
    }

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
        let signed = add_client_sign(
            "micoapi",
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
    fn authentication_error_redacts_sensitive_response_fields() {
        let error = authentication_response(
            reqwest::StatusCode::UNAUTHORIZED,
            r#"{"code":70016,"serviceToken":"secret"}"#,
        );
        assert_eq!(
            error.to_string(),
            "authentication failed (HTTP 401, server code 70016): {\"code\":70016,\"serviceToken\":\"[REDACTED]\"}"
        );
    }

    #[test]
    fn authentication_error_preserves_non_sensitive_response_fields() {
        let error = authentication_response(
            reqwest::StatusCode::BAD_REQUEST,
            r#"{"code":42,"message":"invalid account","details":{"retry_after":60}}"#,
        );
        assert_eq!(
            error.to_string(),
            "authentication failed (HTTP 400, server code 42): {\"code\":42,\"details\":{\"retry_after\":60},\"message\":\"invalid account\"}"
        );
    }
}
