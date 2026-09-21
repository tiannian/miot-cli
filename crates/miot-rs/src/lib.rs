//! Async SDK primitives for `MIoT` devices.
//!
//! Protocol clients and services will be added incrementally. The CLI crate
//! depends on this crate instead of accessing protocol implementations directly.

pub mod error;
pub mod login;
#[path = "miio-api/mod.rs"]
pub mod miio_api;

pub use error::MiotError;
pub use login::{
    CaptchaChallenge, CloudCredential, CloudLoginClient, CloudLoginOutcome, CloudLoginRequest,
    OAuthAuthorizationRequest, OAuthCredential, OAuthLoginClient, QrLoginChallenge, QrLoginClient,
    VerificationProof,
};
pub use miio_api::{ApiClient, HomeDeviceListQuery};

/// Returns the SDK version compiled into the current binary.
#[must_use]
pub const fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    #[test]
    fn exposes_its_package_version() {
        assert_eq!(super::version(), env!("CARGO_PKG_VERSION"));
    }
}
