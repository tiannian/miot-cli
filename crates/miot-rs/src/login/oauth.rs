#![allow(clippy::missing_errors_doc, clippy::needless_pass_by_value)]

use std::time::{Duration, SystemTime};

use serde::Deserialize;
use sha1::Sha1;
use sha2::{Digest, Sha256};
use url::Url;

use crate::MiotError;

const OAUTH_API_DOMAIN: &str = "ha.api.io.mi.com";
const AUTHORIZATION_URL: &str = "https://account.xiaomi.com/oauth2/authorize";
const TOKEN_PATH: &str = "/app/v2/ha/oauth/get_token";
const STATE_TTL: Duration = Duration::from_secs(10 * 60);

/// Parameters that vary for each OAuth authorization request.
#[derive(Clone, Debug, Default)]
pub struct OAuthAuthorizationRequest {
    pub scopes: Vec<String>,
    pub skip_confirm: bool,
}

/// OAuth tokens returned by Xiaomi.
#[derive(Clone, Debug)]
pub struct OAuthCredential {
    access_token: String,
    refresh_token: String,
    expires_at: SystemTime,
}

impl OAuthCredential {
    /// Reconstructs an OAuth credential loaded from persistent storage.
    ///
    /// # Errors
    ///
    /// Returns an error when either token is empty.
    pub fn from_parts(
        access_token: impl Into<String>,
        refresh_token: impl Into<String>,
        expires_at: SystemTime,
    ) -> Result<Self, MiotError> {
        let access_token = access_token.into();
        let refresh_token = refresh_token.into();
        if access_token.is_empty() || refresh_token.is_empty() {
            return Err(MiotError::InvalidInput("OAuth tokens must not be empty"));
        }
        Ok(Self {
            access_token,
            refresh_token,
            expires_at,
        })
    }

    #[must_use]
    pub fn access_token(&self) -> &str {
        &self.access_token
    }
    #[must_use]
    pub fn refresh_token(&self) -> &str {
        &self.refresh_token
    }
    #[must_use]
    pub fn expires_at(&self) -> SystemTime {
        self.expires_at
    }
}

/// OAuth authorization-code client. Pending callback state is held only in memory.
#[derive(Debug)]
pub struct OAuthLoginClient {
    client: reqwest::Client,
    client_id: u64,
    redirect_url: Url,
    region: String,
    device_id: String,
    pending: Option<PendingAuthorization>,
}

#[derive(Debug)]
struct PendingAuthorization {
    state: String,
    redirect_url: Url,
    created_at: SystemTime,
}

impl OAuthLoginClient {
    pub fn new(
        client_id: impl Into<String>,
        redirect_url: Url,
        region: impl Into<String>,
        device_id: impl Into<String>,
    ) -> Result<Self, MiotError> {
        let client_id = client_id
            .into()
            .parse()
            .map_err(|_| MiotError::InvalidInput("OAuth client ID must be numeric"))?;
        let region = region.into();
        let device_id = device_id.into();
        if region.is_empty() || device_id.is_empty() {
            return Err(MiotError::InvalidInput(
                "OAuth configuration must not be empty",
            ));
        }
        Ok(Self {
            client: reqwest::Client::new(),
            client_id,
            redirect_url,
            region,
            device_id: home_assistant_device_id(&device_id),
            pending: None,
        })
    }

    pub fn authorization_url(
        &mut self,
        request: OAuthAuthorizationRequest,
    ) -> Result<Url, MiotError> {
        let state = home_assistant_state(&self.device_id);
        let mut url = Url::parse(AUTHORIZATION_URL)
            .map_err(|_| MiotError::Protocol("invalid OAuth authorization endpoint"))?;
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("redirect_uri", self.redirect_url.as_str());
            query.append_pair("client_id", &self.client_id.to_string());
            query.append_pair("response_type", "code");
            query.append_pair("device_id", &self.device_id);
            query.append_pair("state", &state);
            query.append_pair(
                "skip_confirm",
                if request.skip_confirm {
                    "true"
                } else {
                    "false"
                },
            );
            if !request.scopes.is_empty() {
                query.append_pair("scope", &request.scopes.join(" "));
            }
        }
        self.pending = Some(PendingAuthorization {
            state,
            redirect_url: self.redirect_url.clone(),
            created_at: SystemTime::now(),
        });
        Ok(url)
    }

    pub async fn complete_callback(&mut self, callback: Url) -> Result<OAuthCredential, MiotError> {
        let pending = self.pending.take().ok_or(MiotError::Authorization)?;
        if pending.created_at.elapsed().unwrap_or(STATE_TTL) > STATE_TTL
            || callback.scheme() != pending.redirect_url.scheme()
            || callback.host_str() != pending.redirect_url.host_str()
            || callback.path() != pending.redirect_url.path()
        {
            return Err(MiotError::Authorization);
        }
        let parameters: std::collections::HashMap<_, _> =
            callback.query_pairs().into_owned().collect();
        if parameters.get("state") != Some(&pending.state) {
            return Err(MiotError::Authorization);
        }
        let code = parameters.get("code").ok_or(MiotError::Authorization)?;
        self.token([
            ("code", code.as_str()),
            ("device_id", self.device_id.as_str()),
        ])
        .await
    }

    pub async fn refresh(
        &self,
        credential: &OAuthCredential,
    ) -> Result<OAuthCredential, MiotError> {
        self.token([("refresh_token", &credential.refresh_token)])
            .await
    }

    async fn token<const N: usize>(
        &self,
        extra: [(&str, &str); N],
    ) -> Result<OAuthCredential, MiotError> {
        let mut data = serde_json::Map::new();
        data.insert("client_id".to_owned(), serde_json::json!(self.client_id));
        data.insert(
            "redirect_uri".to_owned(),
            serde_json::json!(self.redirect_url.as_str()),
        );
        for (key, value) in extra {
            data.insert(key.to_owned(), serde_json::json!(value));
        }
        let response = self
            .client
            .get(self.oauth_url(TOKEN_PATH)?)
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            .query(&[("data", serde_json::Value::Object(data).to_string())])
            .send()
            .await?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(MiotError::Authorization);
        }
        if !response.status().is_success() {
            return Err(MiotError::Protocol(
                "OAuth token endpoint returned an unexpected status",
            ));
        }
        let body = response.text().await?;
        let payload: TokenEnvelope = serde_json::from_str(&body).map_err(|_| {
            MiotError::Protocol("OAuth token endpoint returned an invalid response")
        })?;
        if payload.code != 0 {
            return Err(MiotError::AuthorizationResponse {
                code: payload.code,
                message: payload
                    .message
                    .unwrap_or_else(|| "no error message".to_owned()),
            });
        }
        let token: TokenResult = serde_json::from_value(
            payload
                .result
                .ok_or(MiotError::Protocol("OAuth token response is incomplete"))?,
        )
        .map_err(|_| MiotError::Protocol("OAuth token response is incomplete"))?;
        if token.access_token.is_empty() || token.refresh_token.is_empty() {
            return Err(MiotError::Protocol("OAuth token response is incomplete"));
        }
        Ok(OAuthCredential {
            access_token: token.access_token,
            refresh_token: token.refresh_token,
            expires_at: SystemTime::now() + Duration::from_secs(token.expires_in),
        })
    }

    fn oauth_url(&self, path: &str) -> Result<Url, MiotError> {
        Url::parse(&format!(
            "https://{}{path}",
            if self.region == "cn" {
                OAUTH_API_DOMAIN.to_owned()
            } else {
                format!("{}.{}", self.region, OAUTH_API_DOMAIN)
            }
        ))
        .map_err(|_| MiotError::InvalidInput("invalid OAuth endpoint"))
    }
}

#[derive(Deserialize)]
struct TokenEnvelope {
    code: i64,
    message: Option<String>,
    result: Option<serde_json::Value>,
}
#[derive(Deserialize)]
struct TokenResult {
    access_token: String,
    refresh_token: String,
    expires_in: u64,
}

fn home_assistant_device_id(identifier: &str) -> String {
    let digest = Sha256::digest(identifier.as_bytes());
    format!("ha.{digest:x}")[..35].to_owned()
}

fn home_assistant_state(device_id: &str) -> String {
    format!("{:x}", Sha1::digest(format!("d={device_id}").as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_endpoint_uses_the_home_assistant_api_domain() {
        let cn = OAuthLoginClient::new(
            "1",
            Url::parse("http://homeassistant.local:8123/").expect("valid URL"),
            "cn",
            "device",
        )
        .expect("valid client");
        let us = OAuthLoginClient::new(
            "1",
            Url::parse("http://homeassistant.local:8123/").expect("valid URL"),
            "us",
            "device",
        )
        .expect("valid client");

        assert_eq!(
            cn.oauth_url(TOKEN_PATH).expect("valid endpoint").as_str(),
            "https://ha.api.io.mi.com/app/v2/ha/oauth/get_token"
        );
        assert_eq!(
            us.oauth_url(TOKEN_PATH).expect("valid endpoint").as_str(),
            "https://us.ha.api.io.mi.com/app/v2/ha/oauth/get_token"
        );
    }

    #[test]
    fn authorization_request_uses_the_home_assistant_device_id_format() {
        let mut client = OAuthLoginClient::new(
            "2882303761520251711",
            Url::parse("http://homeassistant.local:8123/").expect("valid URL"),
            "cn",
            "device-id",
        )
        .expect("valid client");

        let query: std::collections::HashMap<_, _> = client
            .authorization_url(OAuthAuthorizationRequest::default())
            .expect("valid authorization URL")
            .query_pairs()
            .into_owned()
            .collect();

        assert_eq!(
            query.get("device_id"),
            Some(&"ha.bd732105ef89cf8edd2606a5309c8a26".to_owned())
        );
        assert_eq!(
            query.get("client_id"),
            Some(&"2882303761520251711".to_owned())
        );
        assert_eq!(
            query.get("state"),
            Some(&"c4f205a6ba60b608c936d241ffdf3b9c373ea918".to_owned())
        );
    }

    #[test]
    fn error_response_does_not_require_token_fields() {
        let payload: TokenEnvelope =
            serde_json::from_str(r#"{"code":-1,"message":"request rejected","result":{}}"#)
                .expect("error response is valid");

        assert_eq!(payload.code, -1);
        assert_eq!(payload.message.as_deref(), Some("request rejected"));
    }
}
