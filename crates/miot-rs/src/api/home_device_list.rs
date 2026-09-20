use serde_json::{Value, json};

use crate::{ApiClient, MiotError};

/// Parameters for one page of a home's device list.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HomeDeviceListQuery {
    pub home_owner: i64,
    pub home_id: i64,
    pub start_did: String,
    pub limit: u16,
}

impl HomeDeviceListQuery {
    #[must_use]
    pub fn new(home_owner: i64, home_id: i64) -> Self {
        Self {
            home_owner,
            home_id,
            start_did: String::new(),
            limit: 300,
        }
    }
}

impl ApiClient {
    /// Returns one paginated device-info page for a Xiaomi Home household.
    ///
    /// # Errors
    ///
    /// Returns an error when Xiaomi rejects the request, the network fails, or the response is malformed.
    pub async fn home_device_list(&self, query: &HomeDeviceListQuery) -> Result<Value, MiotError> {
        let response = self
            .post(
                "v2/home/home_device_list",
                json!({
                    "home_owner": query.home_owner,
                    "home_id": query.home_id,
                    "limit": query.limit,
                    "start_did": query.start_did,
                    "get_split_device": false,
                    "support_smart_home": true,
                    "get_cariot_device": true,
                    "get_third_device": true,
                }),
            )
            .await?;
        response.get("result").cloned().ok_or(MiotError::Protocol(
            "cloud home device response did not contain result",
        ))
    }
}
