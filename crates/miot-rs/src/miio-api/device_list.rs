use serde_json::{Value, json};

use crate::{ApiClient, MiotError};

impl ApiClient {
    /// Returns the account-wide Xiaomi Home device list.
    ///
    /// # Errors
    ///
    /// Returns an error when Xiaomi rejects the request, the network fails, or the response is malformed.
    pub async fn device_list(&self) -> Result<Vec<Value>, MiotError> {
        let response = self
            .post(
                "home/device_list",
                json!({
                    "getVirtualModel": true,
                    "getHuamiDevices": 1,
                    "get_split_device": false,
                    "support_smart_home": true,
                }),
            )
            .await?;
        response
            .pointer("/result/list")
            .and_then(Value::as_array)
            .cloned()
            .ok_or(MiotError::Protocol(
                "cloud device list response did not contain result.list",
            ))
    }
}
