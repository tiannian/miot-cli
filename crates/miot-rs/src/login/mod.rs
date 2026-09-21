//! Authentication clients for Xiaomi OAuth and cloud accounts.

mod cloud;
mod oauth;

pub use cloud::{
    CaptchaChallenge, CloudCredential, CloudLoginClient, CloudLoginOutcome, CloudLoginRequest,
    QrLoginChallenge, QrLoginClient, VerificationProof,
};
pub use oauth::{OAuthAuthorizationRequest, OAuthCredential, OAuthLoginClient};

use std::{error::Error, fmt};

/// An error returned by a login client. Authentication failures retain a bounded, redacted server
/// response for command-line debugging.
#[derive(Debug)]
pub enum MiotError {
    Authentication,
    AuthenticationResponse {
        status: u16,
        code: Option<i64>,
        response: String,
    },
    Authorization,
    AuthorizationResponse {
        code: i64,
        message: String,
    },
    CloudApiResponse {
        status: u16,
        code: Option<i64>,
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
            } => {
                if let Some(code) = code {
                    return write!(
                        formatter,
                        "authentication failed (HTTP {status}, server code {code}): {response}"
                    );
                }
                return write!(
                    formatter,
                    "authentication failed (HTTP {status}): {response}"
                );
            }
            Self::Authorization => "authorization failed",
            Self::AuthorizationResponse { code, message } => {
                return write!(
                    formatter,
                    "authorization failed (server code {code}): {message}"
                );
            }
            Self::CloudApiResponse {
                status,
                code,
                message,
            } => {
                if let Some(code) = code {
                    return write!(
                        formatter,
                        "cloud API request failed (HTTP {status}, server code {code}): {message}"
                    );
                }
                return write!(
                    formatter,
                    "cloud API request failed (HTTP {status}): {message}"
                );
            }
            Self::VerificationRequired => "verification is required",
            Self::CaptchaRequired => "captcha is required",
            Self::Network(error) => return write!(formatter, "network request failed: {error}"),
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
