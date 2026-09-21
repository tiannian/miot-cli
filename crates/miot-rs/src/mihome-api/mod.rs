//! Xiaomi Home APIs used to enumerate every device visible to an account.
//!
//! The API first returns homes, rooms, and their device identifiers. Device
//! details are then requested in batches through the paginated device-list API.

mod client;
mod device_list_page;
mod home;

pub use client::MiHomeApiClient;
pub use device_list_page::DeviceListPageQuery;
pub use home::DevRoomPageQuery;
