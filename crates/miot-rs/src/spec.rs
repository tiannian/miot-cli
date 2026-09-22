use reqwest::Client;
use serde_json::Value;

use crate::MiotError;

const SPEC_BASE_URL: &str = "https://miot-spec.org/miot-spec-v2";

/// Client for the public `MIoT` Spec v2 service.
#[derive(Clone, Debug)]
pub struct MiotSpecClient {
    client: Client,
}

impl MiotSpecClient {
    /// Creates a client for the public `MIoT` Spec v2 service.
    ///
    /// # Errors
    ///
    /// Returns an error when the HTTP client cannot be built.
    pub fn new() -> Result<Self, MiotError> {
        Ok(Self {
            client: Client::builder().build()?,
        })
    }

    /// Downloads the complete specification for a device model.
    ///
    /// The service publishes an instance index separately from each instance
    /// document, so this first resolves `model` to its specification URN.
    ///
    /// # Errors
    ///
    /// Returns an error if the service cannot be reached, returns invalid JSON,
    /// or does not list a specification for the model.
    pub async fn instance_for_model(&self, model: &str) -> Result<Value, MiotError> {
        let instances = self
            .client
            .get(format!("{SPEC_BASE_URL}/instances?status=all"))
            .send()
            .await?
            .error_for_status()?
            .json::<Value>()
            .await?;
        let instance_type = instance_type(&instances, model).ok_or(MiotError::Protocol(
            "MIoT Spec did not contain the device model",
        ))?;
        self.client
            .get(format!("{SPEC_BASE_URL}/instance"))
            .query(&[("type", instance_type)])
            .send()
            .await?
            .error_for_status()?
            .json::<Value>()
            .await
            .map_err(MiotError::from)
    }
}

fn instance_type<'a>(instances: &'a Value, model: &str) -> Option<&'a str> {
    instances
        .get("instances")
        .and_then(Value::as_array)?
        .iter()
        .filter(|entry| entry.get("model").and_then(Value::as_str) == Some(model))
        .max_by_key(|entry| {
            (
                entry.get("status").and_then(Value::as_str) == Some("released"),
                entry
                    .get("version")
                    .and_then(Value::as_i64)
                    .unwrap_or_default(),
            )
        })
        .and_then(|entry| entry.get("type").and_then(Value::as_str))
}

#[cfg(test)]
mod tests {
    use super::{SPEC_BASE_URL, instance_type};
    use serde_json::json;

    #[test]
    fn uses_the_public_spec_v2_endpoint() {
        assert_eq!(SPEC_BASE_URL, "https://miot-spec.org/miot-spec-v2");
    }

    #[test]
    fn prefers_the_newest_released_model_specification() {
        let instances = json!({ "instances": [
            { "model": "example.device", "status": "released", "version": 1, "type": "first" },
            { "model": "example.device", "status": "debug", "version": 8, "type": "debug" },
            { "model": "example.device", "status": "released", "version": 2, "type": "latest" }
        ] });
        assert_eq!(instance_type(&instances, "example.device"), Some("latest"));
    }
}
