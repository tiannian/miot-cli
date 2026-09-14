//! Authentication clients for Xiaomi OAuth and cloud accounts.

mod cloud;
mod oauth;

pub use cloud::{
    CaptchaChallenge, CloudCredential, CloudLoginClient, CloudLoginOutcome, CloudLoginRequest,
    VerificationProof,
};
pub use oauth::{OAuthAuthorizationRequest, OAuthCredential, OAuthLoginClient};

use std::{error::Error, fmt};

/// An error returned by a login client. Its messages never include secrets.
#[derive(Debug)]
pub enum MiotError {
    Authentication,
    Authorization,
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
            Self::Authorization => "authorization failed",
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
