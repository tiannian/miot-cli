use serde::Deserialize;
use url::Url;

use super::super::MiotError;
use super::account::{
    AuthResponse, CloudCredential, CloudLoginClient, CloudLoginOutcome, authentication_response,
    cookie_value, decode_json, json_string_or_number, now_millis, trace_entry,
};

/// A Xiaomi QR login challenge. Scan the image URL with the Xiaomi Home app, then wait for the
/// login result on the same client instance.
#[derive(Clone, Debug)]
pub struct QrLoginChallenge {
    pub login_url: Url,
    pub image_url: Option<Url>,
}

#[derive(Debug)]
pub(super) struct QrLoginPending {
    poll_url: Url,
}

impl CloudLoginClient {
    /// Starts a Xiaomi Home QR login transaction.
    ///
    /// # Errors
    ///
    /// 返回认证服务不可用、响应无效，或无法构造登录链接时的错误。
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
    ///
    /// # Errors
    ///
    /// 返回不存在活动登录事务、认证失败、网络超时或服务响应无效时的错误。
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
            json_string_or_number(auth.nonce),
        )
        .await
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
