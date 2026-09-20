//! Xiaomi Home cloud API queries.
//!
//! `ApiClient` uses the `xiaomiio` service-login credentials returned by
//! [`CloudLoginClient`](crate::CloudLoginClient). Requests are RC4-encrypted
//! and signed as required by the Xiaomi Home `/app` API.

mod client;
mod device_list;
mod home_device_list;
mod home_merged;

pub use client::ApiClient;
pub use home_device_list::HomeDeviceListQuery;
