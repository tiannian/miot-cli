//! Error types returned by `MIoT` SDK clients.

use thiserror::Error;

/// An error returned by an `MIoT` SDK client. Authentication failures retain a bounded, redacted
/// server response for command-line debugging.
#[derive(Debug, Error)]
pub enum MiotError {
    #[error("authentication failed")]
    Authentication,
    #[error(
        "authentication failed{details}",
        details = authentication_response_details(*status, *code, response)
    )]
    AuthenticationResponse {
        status: u16,
        code: Option<i64>,
        response: String,
    },
    #[error("authorization failed")]
    Authorization,
    #[error("authorization failed (server code {code}): {message}")]
    AuthorizationResponse { code: i64, message: String },
    #[error(
        "cloud API request failed{details}",
        details = cloud_api_response_details(*status, *code, message)
    )]
    CloudApiResponse {
        status: u16,
        code: Option<i64>,
        message: String,
    },
    #[error("verification is required")]
    VerificationRequired,
    #[error("captcha is required")]
    CaptchaRequired,
    #[error("network request failed: {0}")]
    Network(#[from] reqwest::Error),
    #[error("{0}")]
    Protocol(&'static str),
    #[error("{0}")]
    InvalidInput(&'static str),
}

fn authentication_response_details(status: u16, code: Option<i64>, response: &str) -> String {
    response_details(status, code, response)
}

fn cloud_api_response_details(status: u16, code: Option<i64>, message: &str) -> String {
    response_details(status, code, message)
}

fn response_details(status: u16, code: Option<i64>, message: &str) -> String {
    match code {
        Some(code) => format!(" (HTTP {status}, server code {code}): {message}"),
        None => format!(" (HTTP {status}): {message}"),
    }
}
