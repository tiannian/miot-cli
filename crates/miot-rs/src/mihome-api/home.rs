use serde_json::{Value, json};

use crate::{MiHomeApiClient, MiotError};

/// Parameters for one page of home and room device relationships.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DevRoomPageQuery {
    pub start_id: Option<String>,
    pub limit: u16,
}

impl Default for DevRoomPageQuery {
    fn default() -> Self {
        Self {
            start_id: None,
            limit: 150,
        }
    }
}

impl MiHomeApiClient {
    /// Returns owned and shared homes, rooms, and their device identifiers.
    ///
    /// The response's `homelist` and `share_home_list` together identify the
    /// device identifiers that must be queried for their complete metadata.
    ///
    /// # Errors
    ///
    /// Returns an error when Xiaomi rejects the request, the network fails, or
    /// the response is malformed.
    pub async fn home_info(&self) -> Result<Value, MiotError> {
        let response = self
            .post(
                "v2/homeroom/gethome",
                json!({
                    "limit": 150,
                    "fetch_share": true,
                    "fetch_share_dev": true,
                    "plat_form": 0,
                    "app_ver": 9,
                }),
            )
            .await?;
        response.get("result").cloned().ok_or(MiotError::Protocol(
            "cloud home response did not contain result",
        ))
    }

    /// Returns one page of device-to-home and device-to-room relationships.
    ///
    /// Call this when `home_info` reports `has_more`; use the returned
    /// `max_id` as `query.start_id` for the following page.
    ///
    /// # Errors
    ///
    /// Returns an error when Xiaomi rejects the request, the network fails, or
    /// the response is malformed.
    pub async fn dev_room_page(&self, query: &DevRoomPageQuery) -> Result<Value, MiotError> {
        let response = self
            .post(
                "v2/homeroom/get_dev_room_page",
                json!({
                    "start_id": query.start_id,
                    "limit": query.limit,
                }),
            )
            .await?;
        response.get("result").cloned().ok_or(MiotError::Protocol(
            "cloud device-room response did not contain result",
        ))
    }
}
