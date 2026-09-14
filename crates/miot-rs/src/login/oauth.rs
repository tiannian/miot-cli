#![allow(clippy::missing_errors_doc, clippy::needless_pass_by_value)]

use std::time::{Duration, SystemTime};

use serde::Deserialize;
use url::Url;

use super::MiotError;

const OAUTH_DOMAIN: &str = "oauth2.xiaomi.com";
const AUTHORIZATION_PATH: &str = "/app/v2/ha/oauth/authorize";
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
    client_id: String,
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
        let client_id = client_id.into();
        let region = region.into();
        let device_id = device_id.into();
        if client_id.is_empty() || region.is_empty() || device_id.is_empty() {
            return Err(MiotError::InvalidInput(
                "OAuth configuration must not be empty",
            ));
        }
        Ok(Self {
            client: reqwest::Client::new(),
            client_id,
            redirect_url,
            region,
            device_id,
            pending: None,
        })
    }

    pub fn authorization_url(
        &mut self,
        request: OAuthAuthorizationRequest,
    ) -> Result<Url, MiotError> {
        let state = random_state()?;
        let mut url = self.oauth_url(AUTHORIZATION_PATH)?;
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("redirect_uri", self.redirect_url.as_str());
            query.append_pair("client_id", &self.client_id);
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
        let data = serde_json::json!({ "client_id": self.client_id, "redirect_uri": self.redirect_url.as_str(), "device_id": self.device_id, "code": extra.iter().find(|(key, _)| *key == "code").map(|(_, value)| *value), "refresh_token": extra.iter().find(|(key, _)| *key == "refresh_token").map(|(_, value)| *value) });
        let response = self
            .client
            .get(self.oauth_url(TOKEN_PATH)?)
            .query(&[("data", data.to_string())])
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
        let payload: TokenEnvelope = response.json().await?;
        if payload.code != 0 {
            return Err(MiotError::Authorization);
        }
        let token = payload
            .result
            .ok_or(MiotError::Protocol("OAuth token response is incomplete"))?;
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
                OAUTH_DOMAIN.to_owned()
            } else {
                format!("{}.{}", self.region, OAUTH_DOMAIN)
            }
        ))
        .map_err(|_| MiotError::InvalidInput("invalid OAuth endpoint"))
    }
}

#[derive(Deserialize)]
struct TokenEnvelope {
    code: i64,
    result: Option<TokenResult>,
}
#[derive(Deserialize)]
struct TokenResult {
    access_token: String,
    refresh_token: String,
    expires_in: u64,
}

fn random_state() -> Result<String, MiotError> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|_| MiotError::Protocol("secure random generation failed"))?;
    Ok(base64::Engine::encode(
        &base64::engine::general_purpose::URL_SAFE_NO_PAD,
        bytes,
    ))
}
