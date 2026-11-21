//! One unsigned TIME request prepared by the application's bounded job owner.
//! Neither preparation nor a response establishes qualified clock evidence.

use reqwest::{RequestBuilder, Response};

use crate::binance_rest::{single_shot, BinanceRestClient, BinanceRestError};

const ENDPOINT: &str = "/api/v3/time";

/// Opaque, single-use request. Its builder retains the fresh transport; there
/// is no clone, public constructor, credential, body decode or timeout reset.
pub struct PreparedSpotTime {
    request: RequestBuilder,
}

impl BinanceRestClient {
    /// Synchronously prepare one unsigned request with the ordinary endpoint
    /// configuration (including the explicit loopback harness). The caller
    /// owns the bounded preparation job and its absolute deadline.
    pub fn prepare_spot_time(&self) -> Result<PreparedSpotTime, BinanceRestError> {
        let transport = single_shot()?;
        let url = format!("{}{}", self.endpoint_base_url(ENDPOINT), ENDPOINT);
        Ok(PreparedSpotTime { request: transport.get(url) })
    }
}

impl PreparedSpotTime {
    /// Send once. Return the original response at its head; the owner marks
    /// and reads its body under the same deadline before ordinary decoding.
    pub async fn send(self) -> reqwest::Result<Response> {
        self.request.send().await
    }
}

#[cfg(test)]
#[path = "time_tests.rs"]
mod tests;
