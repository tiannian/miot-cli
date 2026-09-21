use serde::Deserialize;
use url::Url;

use super::account::{
    AuthResponse, CloudCredential, CloudLoginOutcome, absolute_url, account_url, add_client_sign,
    authentication_response, cookie_value, decode_json, fetch_service_login_context,
    json_string_or_number, now_millis, trace_entry,
};
use crate::MiotError;

/// 小米扫码登录挑战。请用米家 App 扫描二维码，并在同一客户端实例上等待结果。
#[derive(Clone, Debug)]
pub struct QrLoginChallenge {
    pub login_url: Url,
    pub image_url: Option<Url>,
}

#[derive(Debug)]
struct QrLoginPending {
    poll_url: Url,
    sid: String,
}

/// 小米扫码登录客户端。一个实例只维护一条扫码登录会话。
#[derive(Debug)]
pub struct QrLoginClient {
    client: reqwest::Client,
    pending: Option<QrLoginPending>,
}

impl QrLoginClient {
    /// 创建扫码登录客户端。
    pub fn new(device_id: impl Into<String>) -> Result<Self, MiotError> {
        trace_entry("QrLoginClient::new");
        let device_id = device_id.into();
        if device_id.is_empty() {
            return Err(MiotError::InvalidInput("device ID must not be empty"));
        }
        let jar = std::sync::Arc::new(reqwest::cookie::Jar::default());
        let account_url = account_url("/")?;
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
            pending: None,
        })
    }

    /// 开始一条米家扫码登录事务。
    ///
    /// # Errors
    ///
    /// 当认证服务不可用、响应无效或无法构造登录链接时返回错误。
    pub async fn begin_qr_login(&mut self) -> Result<QrLoginChallenge, MiotError> {
        trace_entry("QrLoginClient::begin_qr_login");
        self.pending = None;
        let context = fetch_service_login_context(&self.client, "mijia", "mijia").await?;
        let mut query = vec![
            ("theme", String::new()),
            ("bizDeviceType", String::new()),
            ("_hasLogo", "false".to_owned()),
            ("_qrsize", "240".to_owned()),
            ("_dc", now_millis()),
            ("sid", context.sid.clone()),
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
            .get(account_url("/longPolling/loginUrl")?)
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
        let login_url = absolute_url(&challenge.login_url)?;
        let poll_url = absolute_url(&challenge.poll_url)?;
        let image_url = challenge
            .image_url
            .as_deref()
            .map(absolute_url)
            .transpose()?;
        self.pending = Some(QrLoginPending {
            poll_url,
            sid: context.sid,
        });
        Ok(QrLoginChallenge {
            login_url,
            image_url,
        })
    }

    /// 等待米家确认当前扫码登录事务。
    ///
    /// # Errors
    ///
    /// 当不存在活动事务、认证失败、网络超时或服务响应无效时返回错误。
    pub async fn wait_for_qr_login(&mut self) -> Result<CloudLoginOutcome, MiotError> {
        trace_entry("QrLoginClient::wait_for_qr_login");
        let pending = self
            .pending
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
            &pending.sid,
            location,
            auth.ssecurity,
            json_string_or_number(auth.user_id),
            json_string_or_number(auth.nonce),
        )
        .await
    }

    async fn finish_qr_location(
        &self,
        sid: &str,
        location: String,
        ssecurity: Option<String>,
        user_id: Option<String>,
        nonce: Option<String>,
    ) -> Result<CloudLoginOutcome, MiotError> {
        trace_entry("QrLoginClient::finish_qr_location");
        let location = add_client_sign(sid, location, ssecurity.as_deref(), nonce.as_deref())?;
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
