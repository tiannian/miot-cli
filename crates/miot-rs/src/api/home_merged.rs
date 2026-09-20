use serde_json::{Value, json};

use crate::{ApiClient, MiotError};

impl ApiClient {
    /// Returns homes, rooms, and device-to-room relationships for the account.
    ///
    /// # Errors
    ///
    /// Returns an error when Xiaomi rejects the request, the network fails, or the response is malformed.
    pub async fn home_merged(&self) -> Result<Value, MiotError> {
        let response = self
            .post(
                "v2/homeroom/gethome_merged",
                json!({
                    "fg": true,
                    "fetch_share": true,
                    "fetch_share_dev": true,
                    "fetch_cariot": true,
                    "limit": 300,
                    "app_ver": 7,
                    "plat_form": 0,
                }),
            )
            .await?;
        response.get("result").cloned().ok_or(MiotError::Protocol(
            "cloud home response did not contain result",
        ))
    }
}
