//! Xiaomi Home APIs for device discovery and central-gateway certificates.
//!
//! The API first returns homes, rooms, and their device identifiers. Device
//! details are then requested in batches through the paginated device-list API.

mod client;
mod device_list_page;
mod home;

pub use client::MiHomeApiClient;
pub use device_list_page::DeviceListPageQuery;
pub use home::DevRoomPageQuery;
