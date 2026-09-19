//! Authentication clients for Xiaomi OAuth and cloud accounts.

mod cloud;
mod oauth;

pub use cloud::{
    CaptchaChallenge, CloudCredential, CloudLoginClient, CloudLoginOutcome, CloudLoginRequest,
    VerificationProof,
};
pub use oauth::{OAuthAuthorizationRequest, OAuthCredential, OAuthLoginClient};

use std::{error::Error, fmt};

/// An error returned by a login client. Authentication failures retain a bounded server response
/// for command-line debugging and can contain secrets.
#[derive(Debug)]
pub enum MiotError {
    Authentication,
    AuthenticationResponse {
        status: u16,
        code: Option<i64>,
        response: String,
        password_md5: Option<String>,
    },
    Authorization,
    AuthorizationResponse {
        code: i64,
        message: String,
    },
    VerificationRequired,
    CaptchaRequired,
    Network(reqwest::Error),
    Protocol(&'static str),
    InvalidInput(&'static str),
}

impl fmt::Display for MiotError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::Authentication => "authentication failed",
            Self::AuthenticationResponse {
                status,
                code,
                response,
                password_md5,
            } => {
                let digest = password_md5
                    .as_deref()
                    .map(|digest| format!("; password_md5: {digest}"))
                    .unwrap_or_default();
                if let Some(code) = code {
                    return write!(
                        formatter,
                        "authentication failed (HTTP {status}, server code {code}): {response}{digest}"
                    );
                }
                return write!(
                    formatter,
                    "authentication failed (HTTP {status}): {response}{digest}"
                );
            }
            Self::Authorization => "authorization failed",
            Self::AuthorizationResponse { code, message } => {
                return write!(
                    formatter,
                    "authorization failed (server code {code}): {message}"
                );
            }
            Self::VerificationRequired => "verification is required",
            Self::CaptchaRequired => "captcha is required",
            Self::Network(_) => "network request failed",
            Self::Protocol(message) | Self::InvalidInput(message) => message,
        };
        formatter.write_str(message)
    }
}

impl Error for MiotError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Network(error) => Some(error),
            _ => None,
        }
    }
}

impl From<reqwest::Error> for MiotError {
    fn from(error: reqwest::Error) -> Self {
        Self::Network(error)
    }
}
