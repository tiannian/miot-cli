use serde_json::{Value, json};

use crate::{ApiClient, MiotError};

/// Parameters for one paginated Xiaomi Home device-details request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceListPageQuery {
    pub dids: Vec<String>,
    pub start_did: Option<String>,
    pub limit: u16,
}

impl DeviceListPageQuery {
    /// Creates a query for the supplied device identifiers.
    #[must_use]
    pub fn new(dids: Vec<String>) -> Self {
        Self {
            dids,
            start_did: None,
            limit: 200,
        }
    }

    /// Creates a query without a DID filter.
    ///
    /// Xiaomi uses this form to return devices shared directly with the
    /// current account; callers can identify them from each record's `owner`.
    #[must_use]
    pub fn all_visible() -> Self {
        Self::new(Vec::new())
    }
}

impl ApiClient {
    /// Returns one page of complete device metadata.
    ///
    /// The response can include `has_more` and `next_start_did`; callers must
    /// continue with the latter until `has_more` is false.
    ///
    /// # Errors
    ///
    /// Returns an error when Xiaomi rejects the request, the network fails, or
    /// the response is malformed.
    pub async fn device_list_page(&self, query: &DeviceListPageQuery) -> Result<Value, MiotError> {
        let mut request = json!({
            "limit": query.limit,
            "get_split_device": true,
            "get_third_device": true,
            "dids": query.dids,
        });
        if let Some(start_did) = &query.start_did {
            request["start_did"] = Value::String(start_did.clone());
        }
        let response = self.post("v2/home/device_list_page", request).await?;
        response.get("result").cloned().ok_or(MiotError::Protocol(
            "cloud device-list response did not contain result",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::DeviceListPageQuery;

    #[test]
    fn defaults_to_the_reference_page_size() {
        let query = DeviceListPageQuery::new(vec!["123".to_owned()]);
        assert_eq!(query.limit, 200);
        assert_eq!(query.start_did, None);
    }

    #[test]
    fn unfiltered_query_has_no_dids() {
        assert!(DeviceListPageQuery::all_visible().dids.is_empty());
    }
}
