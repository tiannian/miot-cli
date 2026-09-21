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

impl MiotError {
    pub(crate) fn authentication_response(status: reqwest::StatusCode, body: &str) -> Self {
        tracing::trace!(
            function = "MiotError::authentication_response",
            "constructing authentication response error"
        );
        let body = body.trim_start_matches("&&&START&&&");
        let value = serde_json::from_str::<serde_json::Value>(body).ok();
        let code = value
            .as_ref()
            .and_then(|value| value.get("code"))
            .and_then(serde_json::Value::as_i64);
        let response = value.map_or_else(|| body.to_owned(), redact_sensitive_values);
        Self::AuthenticationResponse {
            status: status.as_u16(),
            code,
            response: response.chars().take(4096).collect(),
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authentication_response_redacts_sensitive_response_fields() {
        let error = MiotError::authentication_response(
            reqwest::StatusCode::UNAUTHORIZED,
            r#"{"code":70016,"serviceToken":"secret"}"#,
        );
        assert_eq!(
            error.to_string(),
            "authentication failed (HTTP 401, server code 70016): {\"code\":70016,\"serviceToken\":\"[REDACTED]\"}"
        );
    }

    #[test]
    fn authentication_response_preserves_non_sensitive_response_fields() {
        let error = MiotError::authentication_response(
            reqwest::StatusCode::BAD_REQUEST,
            r#"{"code":42,"message":"invalid account","details":{"retry_after":60}}"#,
        );
        assert_eq!(
            error.to_string(),
            "authentication failed (HTTP 400, server code 42): {\"code\":42,\"details\":{\"retry_after\":60},\"message\":\"invalid account\"}"
        );
    }
}
