use reqwest::{Client, Url};
use serde_json::Value;
use tracing::{debug, trace};

use crate::{MiotError, OAuthCredential};

const DEFAULT_CLIENT_ID: &str = "2882303761520251711";

/// Xiaomi Home API client authenticated with an OAuth access token.
#[derive(Clone, Debug)]
pub struct MiHomeApiClient {
    client: Client,
    region: String,
    credential: OAuthCredential,
}

impl MiHomeApiClient {
    /// Creates an OAuth-authenticated Xiaomi Home API client.
    ///
    /// # Errors
    ///
    /// Returns an error when the region is empty or the HTTP client cannot be built.
    pub fn new(region: impl Into<String>, credential: OAuthCredential) -> Result<Self, MiotError> {
        let region = region.into();
        if region.is_empty() {
            return Err(MiotError::InvalidInput("cloud region must not be empty"));
        }
        Ok(Self {
            client: Client::new(),
            region,
            credential,
        })
    }

    pub(crate) async fn post(&self, path: &str, data: Value) -> Result<Value, MiotError> {
        let url = self.api_url(path)?;
        debug!(region = %self.region, path, "sending Xiaomi Home API request");
        let response = self
            .client
            .post(url)
            .header("X-Client-BizId", "haapi")
            .header("X-Client-AppId", DEFAULT_CLIENT_ID)
            .header(
                "Authorization",
                format!("Bearer{}", self.credential.access_token()),
            )
            .json(&data)
            .send()
            .await?;
        let status = response.status();
        let body = response.text().await?;
        trace!(path, status = %status, response_bytes = body.len(), "received Xiaomi Home API response");
        let value: Value = serde_json::from_str(&body)
            .map_err(|_| MiotError::Protocol("could not decode Xiaomi Home API response"))?;
        if !status.is_success()
            || value
                .get("code")
                .and_then(Value::as_i64)
                .is_some_and(|code| code != 0)
        {
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
            "ha.api.io.mi.com".to_owned()
        } else {
            format!("{}.ha.api.io.mi.com", self.region)
        };
        Url::parse(&format!(
            "https://{host}/app/{}",
            path.trim_start_matches('/')
        ))
        .map_err(|_| MiotError::InvalidInput("invalid Xiaomi Home API path"))
    }
}
