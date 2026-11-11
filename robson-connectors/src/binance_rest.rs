//! Binance REST API Client for USD-M Futures Trading
//!
//! Provides REST API integration for:
//! - Querying futures account positions via `/fapi/v2/positionRisk`
//! - Placing market orders via `/fapi/v1/order`
//! - Setting leverage via `/fapi/v1/leverage`
//! - Authentication via HMAC SHA256 signatures
//!
//! # Authentication
//!
//! Binance uses API key + secret with HMAC SHA256 signatures.
//! All signed requests require:
//! - `X-MBX-APIKEY` header
//! - `signature` query parameter (HMAC SHA256 of query string)
//! - `timestamp` query parameter

use std::{
    sync::{Arc, OnceLock},
    time::Duration,
};

use chrono::Utc;
use reqwest::{Client, RequestBuilder, StatusCode};
use robson_domain::{Price, Quantity, Side};
use robson_exec::{gate::OperationGate, order_journal::SendRefusal};
use rust_decimal::Decimal;
use serde::Deserialize;
use serde_json::Value;
use thiserror::Error;
use tokio::time::timeout;

// =============================================================================
// Constants
// =============================================================================

/// Binance USD-M futures REST API base URL.
const BINANCE_FUTURES_API_URL: &str = "https://fapi.binance.com";

/// Binance USD-M futures testnet REST API base URL.
const BINANCE_FUTURES_TESTNET_API_URL: &str = "https://testnet.binancefuture.com";

const BINANCE_SPOT_API_URL: &str = "https://api.binance.com";
const BINANCE_SPOT_TESTNET_API_URL: &str = "https://testnet.binance.vision";

/// One bound per request, sending and reading the body alike (ADR-0056 step 7,
/// D19). It replaced a bound on each, which let one request take 20 s.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

// =============================================================================
// Errors
// =============================================================================

/// Errors that can occur in the Binance REST client.
#[derive(Debug, Clone, Error)]
pub enum BinanceRestError {
    /// Failed to build request signature
    #[error("Failed to build signature: {0}")]
    SignatureError(String),

    /// HTTP request failed
    #[error("HTTP request failed: {0}")]
    RequestFailed(String),

    /// API returned error. Produced for any non-success HTTP status with a
    /// JSON error body, so `http_status` is what tells a 4xx from a 5xx; it
    /// stays out of the message and of the error mapping.
    #[error("Binance API error: {code} - {msg}")]
    ApiError {
        code: i64,
        msg: String,
        http_status: u16,
    },

    /// Failed to parse response
    #[error("Failed to parse response: {0}")]
    ParseError(String),

    /// Request timed out. Uncertain: it may have reached the venue.
    #[error("Request timed out")]
    Timeout,

    /// Refused before anything was sent (ADR-0056 step 7): a mutation without
    /// an operation permit while the daemon is stopping, or an entry the gate
    /// refused at its send.
    #[error("Refused before sending: {0}")]
    Refused(String),

    /// Invalid parameter
    #[error("Invalid parameter: {0}")]
    InvalidParameter(String),

    /// Position not found
    #[error("Position not found for symbol: {0}")]
    PositionNotFound(String),
}

impl From<SendRefusal> for BinanceRestError {
    fn from(refusal: SendRefusal) -> Self {
        Self::Refused(refusal.to_string())
    }
}

// =============================================================================
// Binance REST Client
// =============================================================================

/// Binance REST API client for USD-M futures trading.
pub struct BinanceRestClient {
    /// HTTP client
    client: Client,
    /// API key
    api_key: String,
    /// API secret
    api_secret: String,
    /// Use testnet
    testnet: bool,
    /// The permit gate every signed mutation passes; none checks nothing.
    gate: OnceLock<Arc<OperationGate>>,
    request_timeout: Duration,
    /// Test seam: serve every endpoint from a local listener.
    #[cfg(any(test, feature = "crash-points"))]
    local_base_url: Option<String>,
}

impl BinanceRestClient {
    /// Create a new Binance REST client for production.
    pub fn new(api_key: String, api_secret: String) -> Self {
        Self {
            client: Client::new(),
            api_key,
            api_secret,
            testnet: false,
            gate: OnceLock::new(),
            request_timeout: REQUEST_TIMEOUT,
            #[cfg(any(test, feature = "crash-points"))]
            local_base_url: None,
        }
    }

    /// Create a client for the futures testnet.
    pub fn testnet(api_key: String, api_secret: String) -> Self {
        Self {
            client: Client::new(),
            api_key,
            api_secret,
            testnet: true,
            gate: OnceLock::new(),
            request_timeout: REQUEST_TIMEOUT,
            #[cfg(any(test, feature = "crash-points"))]
            local_base_url: None,
        }
    }

    /// Pass every signed mutation through `gate` from now on (ADR-0056 step
    /// 7). The first gate stays.
    pub fn attach_gate(&self, gate: Arc<OperationGate>) {
        let _ = self.gate.set(gate);
    }

    /// Shorten the per-request bound, for tests that wait it out.
    #[cfg(test)]
    pub(crate) fn with_request_timeout(mut self, bound: Duration) -> Self {
        self.request_timeout = bound;
        self
    }

    /// Client whose every request goes to a local test listener.
    #[cfg(test)]
    pub(crate) fn with_local_base_url(base_url: String) -> Self {
        Self {
            local_base_url: Some(base_url),
            ..Self::new("key".to_string(), "secret".to_string())
        }
    }

    /// Crash harness: every request goes to the fake venue at `base_url`.
    /// Exists only with the `crash-points` feature.
    ///
    /// # Panics
    /// Unless `base_url` is an `http://<loopback>:<port>` origin.
    #[cfg(feature = "crash-points")]
    pub fn with_harness_base_url(base_url: String, api_key: String, api_secret: String) -> Self {
        let origin = harness_origin(&base_url)
            .unwrap_or_else(|refused| panic!("crash harness venue refused: {refused}"));
        Self {
            local_base_url: Some(origin),
            ..Self::new(api_key, api_secret)
        }
    }

    /// Get the base URL for USD-M futures API requests.
    fn base_url(&self) -> &str {
        if self.testnet {
            BINANCE_FUTURES_TESTNET_API_URL
        } else {
            BINANCE_FUTURES_API_URL
        }
    }

    fn spot_base_url(&self) -> &str {
        if self.testnet {
            BINANCE_SPOT_TESTNET_API_URL
        } else {
            BINANCE_SPOT_API_URL
        }
    }

    fn endpoint_base_url(&self, endpoint: &str) -> &str {
        #[cfg(any(test, feature = "crash-points"))]
        if let Some(local) = &self.local_base_url {
            return local;
        }
        if endpoint.starts_with("/fapi/") {
            self.base_url()
        } else {
            self.spot_base_url()
        }
    }

    /// Build query string with signature for signed requests.
    fn build_signed_query(
        &self,
        mut params: Vec<(&str, String)>,
    ) -> Result<String, BinanceRestError> {
        let timestamp = Utc::now().timestamp_millis().to_string();
        params.push(("timestamp", timestamp));

        params.sort_by(|a, b| a.0.cmp(b.0));

        let query_string: String =
            params.iter().map(|(k, v)| format!("{}={}", k, v)).collect::<Vec<_>>().join("&");

        use hmac::{Hmac, Mac};
        use sha2::Sha256;

        type HmacSha256 = Hmac<Sha256>;

        let mut mac = HmacSha256::new_from_slice(self.api_secret.as_bytes())
            .map_err(|e| BinanceRestError::SignatureError(format!("HMAC error: {}", e)))?;

        mac.update(query_string.as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes());

        Ok(format!("{}&signature={}", query_string, signature))
    }

    /// Send one request and read its body under one bound for both
    /// (ADR-0056 step 7, D19). A timeout is uncertain: the request may have
    /// reached the venue. Every HTTP status is an answer here.
    async fn answer(&self, request: RequestBuilder) -> Result<RestAnswer, BinanceRestError> {
        let answered = async {
            let response = request
                .send()
                .await
                .map_err(|e| BinanceRestError::RequestFailed(e.to_string()))?;
            let status = response.status();
            let body =
                response.text().await.map_err(|e| BinanceRestError::ParseError(e.to_string()))?;
            Ok::<_, BinanceRestError>(RestAnswer { status, body })
        };
        timeout(self.request_timeout, answered)
            .await
            .map_err(|_| BinanceRestError::Timeout)?
    }

    /// [`Self::answer`], with every non-success status an error.
    async fn exchange(&self, request: RequestBuilder) -> Result<String, BinanceRestError> {
        self.answer(request).await?.into_body()
    }

    /// The choke point (ADR-0056 step 7): every signed mutation passes the
    /// gate before anything is built or sent.
    fn pass_gate(&self, method: &str, endpoint: &str) -> Result<(), BinanceRestError> {
        match self.gate.get() {
            Some(gate) => gate
                .mutation(&format!("{method} {endpoint}"))
                .map_err(|refused| BinanceRestError::Refused(refused.to_string())),
            None => Ok(()),
        }
    }

    /// Send a GET request to a public endpoint.
    async fn get_public(
        &self,
        endpoint: &str,
        params: Vec<(&str, String)>,
    ) -> Result<String, BinanceRestError> {
        self.get_public_answer(endpoint, params).await?.into_body()
    }

    /// A public GET whose answer comes back whatever its HTTP status, for a
    /// caller that types the venue's errors itself (the spot read side,
    /// ADR-0056 step 8a).
    pub(crate) async fn get_public_answer(
        &self,
        endpoint: &str,
        params: Vec<(&str, String)>,
    ) -> Result<RestAnswer, BinanceRestError> {
        let url = if params.is_empty() {
            format!("{}{}", self.endpoint_base_url(endpoint), endpoint)
        } else {
            let query =
                params.iter().map(|(k, v)| format!("{}={}", k, v)).collect::<Vec<_>>().join("&");
            format!("{}{}?{}", self.endpoint_base_url(endpoint), endpoint, query)
        };

        self.answer(self.client.get(&url)).await
    }

    /// Send a GET request to a signed endpoint.
    async fn get_signed(
        &self,
        endpoint: &str,
        params: Vec<(&str, String)>,
    ) -> Result<String, BinanceRestError> {
        self.get_signed_answer(endpoint, params).await?.into_body()
    }

    /// A signed GET whose answer comes back whatever its HTTP status, for a
    /// caller that types the venue's errors itself (the spot read side,
    /// ADR-0056 step 8a). A read: it does not pass the mutation gate.
    pub(crate) async fn get_signed_answer(
        &self,
        endpoint: &str,
        params: Vec<(&str, String)>,
    ) -> Result<RestAnswer, BinanceRestError> {
        let query = self.build_signed_query(params)?;
        let url = format!("{}{}?{}", self.endpoint_base_url(endpoint), endpoint, query);

        self.answer(self.client.get(&url).header("X-MBX-APIKEY", &self.api_key)).await
    }

    /// Send a POST request to a signed endpoint: a venue mutation.
    async fn post_signed(
        &self,
        endpoint: &str,
        params: Vec<(&str, String)>,
    ) -> Result<String, BinanceRestError> {
        self.pass_gate("POST", endpoint)?;
        let query = self.build_signed_query(params)?;
        let url = format!("{}{}?{}", self.endpoint_base_url(endpoint), endpoint, query);

        self.exchange(self.client.post(&url).header("X-MBX-APIKEY", &self.api_key))
            .await
    }

    /// Send a DELETE request to a signed endpoint: a venue mutation.
    async fn delete_signed(
        &self,
        endpoint: &str,
        params: Vec<(&str, String)>,
    ) -> Result<String, BinanceRestError> {
        self.pass_gate("DELETE", endpoint)?;
        let query = self.build_signed_query(params)?;
        let url = format!("{}{}?{}", self.endpoint_base_url(endpoint), endpoint, query);

        self.exchange(self.client.delete(&url).header("X-MBX-APIKEY", &self.api_key))
            .await
    }

    // =========================================================================
    // Futures Position API
    // =========================================================================

    /// Get current open futures positions for a symbol.
    ///
    /// Queries `GET /fapi/v2/positionRisk` (signed) and returns positions
    /// with non-zero `positionAmt`.
    pub async fn get_open_positions(
        &self,
        symbol: &str,
    ) -> Result<Vec<FuturesPosition>, BinanceRestError> {
        let params = vec![("symbol", symbol.to_string())];

        let body = self.get_signed("/fapi/v2/positionRisk", params).await?;
        Self::parse_open_positions(&body)
    }

    /// Get every open futures position across the account.
    pub async fn get_all_open_positions(&self) -> Result<Vec<FuturesPosition>, BinanceRestError> {
        let body = self.get_signed("/fapi/v2/positionRisk", vec![]).await?;
        Self::parse_open_positions(&body)
    }

    // =========================================================================
    // Order API
    // =========================================================================

    /// Place a market order on USD-M futures.
    ///
    /// # Arguments
    ///
    /// * `symbol` - Trading pair symbol (e.g., "BTCUSDT")
    /// * `side` - Order side (`BUY` or `SELL`)
    /// * `quantity` - Order quantity
    /// * `client_order_id` - Optional client order ID for idempotency
    /// * `reduce_only` - If true, only reduces existing position (for exits)
    ///
    /// # Endpoint
    ///
    /// `POST /fapi/v1/order`
    ///
    /// # Side mapping (One-way mode)
    ///
    /// | Position | Action | Side | reduceOnly |
    /// |----------|--------|------|------------|
    /// | Long     | Open   | BUY  | false      |
    /// | Long     | Close  | SELL | true       |
    /// | Short    | Open   | SELL | false      |
    /// | Short    | Close  | BUY  | true       |
    pub async fn place_market_order(
        &self,
        symbol: &str,
        side: Side,
        quantity: Decimal,
        client_order_id: Option<&str>,
        reduce_only: bool,
    ) -> Result<BinanceOrderResponse, BinanceRestError> {
        let side_str = match side {
            Side::Long => "BUY",
            Side::Short => "SELL",
        };

        let mut params = vec![
            ("symbol", symbol.to_string()),
            ("side", side_str.to_string()),
            ("type", "MARKET".to_string()),
            ("quantity", quantity.to_string()),
            ("newOrderRespType", "RESULT".to_string()),
        ];

        if reduce_only {
            params.push(("reduceOnly", "true".to_string()));
        }

        if let Some(coid) = client_order_id {
            params.push(("newClientOrderId", coid.to_string()));
        }

        let body = self.post_signed("/fapi/v1/order", params).await?;

        serde_json::from_str(&body).map_err(|e| BinanceRestError::ParseError(e.to_string()))
    }

    /// Place a reduce-only protective `STOP_MARKET` order (ADR-0039).
    ///
    /// Mirrors `place_market_order` for signing and response parsing, but posts
    /// a `STOP_MARKET` that is accepted (status `NEW`) and triggers when price
    /// crosses `stop_price`. Always reduce-only: it can only close the open
    /// position it protects.
    ///
    /// # Endpoint
    ///
    /// `POST /fapi/v1/algoOrder`
    pub async fn place_stop_market_order(
        &self,
        symbol: &str,
        side: Side,
        quantity: Decimal,
        stop_price: Decimal,
        client_order_id: &str,
    ) -> Result<BinanceAlgoOrderResponse, BinanceRestError> {
        let side_str = match side {
            Side::Long => "BUY",
            Side::Short => "SELL",
        };

        let params = vec![
            ("algoType", "CONDITIONAL".to_string()),
            ("symbol", symbol.to_string()),
            ("side", side_str.to_string()),
            ("type", "STOP_MARKET".to_string()),
            ("triggerPrice", stop_price.to_string()),
            ("quantity", quantity.to_string()),
            ("reduceOnly", "true".to_string()),
            ("clientAlgoId", client_order_id.to_string()),
            ("newOrderRespType", "RESULT".to_string()),
        ];

        let body = self.post_signed("/fapi/v1/algoOrder", params).await?;

        serde_json::from_str(&body).map_err(|e| BinanceRestError::ParseError(e.to_string()))
    }

    /// Cancel an open algo order.
    ///
    /// `DELETE /fapi/v1/algoOrder`
    pub async fn cancel_algo_order(
        &self,
        algo_id: i64,
    ) -> Result<BinanceAlgoCancelResponse, BinanceRestError> {
        let params = vec![("algoId", algo_id.to_string())];

        let body = self.delete_signed("/fapi/v1/algoOrder", params).await?;

        serde_json::from_str(&body).map_err(|e| BinanceRestError::ParseError(e.to_string()))
    }

    /// Query an algo order by algo id.
    ///
    /// `GET /fapi/v1/algoOrder`
    pub async fn query_algo_order(
        &self,
        algo_id: i64,
    ) -> Result<BinanceAlgoOrderDetail, BinanceRestError> {
        let params = vec![("algoId", algo_id.to_string())];

        let body = self.get_signed("/fapi/v1/algoOrder", params).await?;

        serde_json::from_str(&body).map_err(|e| BinanceRestError::ParseError(e.to_string()))
    }

    /// Query an algo order by client algo id.
    ///
    /// `GET /fapi/v1/algoOrder` (signed) with `clientAlgoId`. A not-found is
    /// returned as the raw API error; the caller applies the venue's finality
    /// rule.
    pub async fn query_algo_order_by_client_id(
        &self,
        client_algo_id: &str,
    ) -> Result<BinanceAlgoOrderDetail, BinanceRestError> {
        let params = vec![("clientAlgoId", client_algo_id.to_string())];

        let body = self.get_signed("/fapi/v1/algoOrder", params).await?;

        serde_json::from_str(&body).map_err(|e| BinanceRestError::ParseError(e.to_string()))
    }

    /// Cancel an open order.
    ///
    /// `DELETE /fapi/v1/order`
    pub async fn cancel_order(
        &self,
        symbol: &str,
        order_id: u64,
    ) -> Result<BinanceOrderResponse, BinanceRestError> {
        let params = vec![
            ("symbol", symbol.to_string()),
            ("orderId", order_id.to_string()),
        ];

        let body = self.delete_signed("/fapi/v1/order", params).await?;

        serde_json::from_str(&body).map_err(|e| BinanceRestError::ParseError(e.to_string()))
    }

    /// Query currently open (unfilled) algo orders for a symbol.
    ///
    /// `GET /fapi/v1/openAlgoOrders` (signed). Used by the reconciliation
    /// worker's orphan insurance-order sweep (ADR-0039).
    pub async fn get_open_algo_orders(
        &self,
        symbol: &str,
    ) -> Result<Vec<BinanceOpenAlgoOrder>, BinanceRestError> {
        let params = vec![("symbol", symbol.to_string())];

        let body = self.get_signed("/fapi/v1/openAlgoOrders", params).await?;

        serde_json::from_str(&body).map_err(|e| BinanceRestError::ParseError(e.to_string()))
    }

    /// Get current price for a symbol.
    ///
    /// `GET /fapi/v1/ticker/price` (public, no signature required).
    pub async fn get_price(&self, symbol: &str) -> Result<Price, BinanceRestError> {
        let params = vec![("symbol", symbol.to_string())];

        let body = self.get_public("/fapi/v1/ticker/price", params).await?;

        let response: PriceResponse =
            serde_json::from_str(&body).map_err(|e| BinanceRestError::ParseError(e.to_string()))?;

        Ok(Price::new(response.price).map_err(|e| {
            BinanceRestError::ParseError(format!("Invalid price in response: {}", e))
        })?)
    }

    /// Get USD-M futures klines for a symbol.
    ///
    /// `GET /fapi/v1/klines?symbol={symbol}&interval={interval}&limit={limit}`
    ///
    /// Returned klines preserve Binance's oldest-first ordering.
    pub async fn get_futures_klines(
        &self,
        symbol: &str,
        interval: &str,
        limit: u16,
    ) -> Result<Vec<BinanceKline>, BinanceRestError> {
        if limit == 0 || limit > 1000 {
            return Err(BinanceRestError::InvalidParameter(format!(
                "kline limit must be between 1 and 1000, got {}",
                limit
            )));
        }

        let params = vec![
            ("symbol", symbol.to_string()),
            ("interval", interval.to_string()),
            ("limit", limit.to_string()),
        ];

        let body = self.get_public("/fapi/v1/klines", params).await?;

        parse_futures_klines(&body)
    }

    /// Query order status.
    ///
    /// `GET /fapi/v1/order` (signed).
    pub async fn get_order_status(
        &self,
        symbol: &str,
        order_id: u64,
    ) -> Result<BinanceOrderResponse, BinanceRestError> {
        let params = vec![
            ("symbol", symbol.to_string()),
            ("orderId", order_id.to_string()),
        ];

        let body = self.get_signed("/fapi/v1/order", params).await?;

        serde_json::from_str(&body).map_err(|e| BinanceRestError::ParseError(e.to_string()))
    }

    /// Query order status by client order id.
    ///
    /// `GET /fapi/v1/order` (signed) with `origClientOrderId`. A not-found is
    /// returned as the raw `-2013` API error; the caller applies the venue's
    /// finality rule.
    pub async fn get_order_status_by_client_id(
        &self,
        symbol: &str,
        client_order_id: &str,
    ) -> Result<BinanceOrderResponse, BinanceRestError> {
        let params = vec![
            ("symbol", symbol.to_string()),
            ("origClientOrderId", client_order_id.to_string()),
        ];

        let body = self.get_signed("/fapi/v1/order", params).await?;

        serde_json::from_str(&body).map_err(|e| BinanceRestError::ParseError(e.to_string()))
    }

    /// Set leverage for a symbol.
    ///
    /// `POST /fapi/v1/leverage` (signed).
    ///
    /// Should be called at startup for each symbol before any trading.
    pub async fn set_leverage(&self, symbol: &str, leverage: u8) -> Result<(), BinanceRestError> {
        let params = vec![
            ("symbol", symbol.to_string()),
            ("leverage", leverage.to_string()),
        ];

        self.post_signed("/fapi/v1/leverage", params).await?;

        Ok(())
    }

    /// Get current position mode (Hedge vs One-way).
    ///
    /// `GET /fapi/v1/positionSide/dual` (signed).
    ///
    /// Returns `true` for Hedge mode (dualSidePosition=true), `false` for
    /// One-way mode.
    pub async fn get_position_mode(&self) -> Result<bool, BinanceRestError> {
        let body = self.get_signed("/fapi/v1/positionSide/dual", vec![]).await?;

        let response: serde_json::Value = serde_json::from_str(&body).map_err(|e| {
            BinanceRestError::ParseError(format!("Failed to parse position mode response: {}", e))
        })?;

        let dual_side = response["dualSidePosition"].as_bool().unwrap_or(false);

        Ok(dual_side)
    }

    fn parse_open_positions(body: &str) -> Result<Vec<FuturesPosition>, BinanceRestError> {
        let positions: Vec<PositionRiskResponse> =
            serde_json::from_str(body).map_err(|e| BinanceRestError::ParseError(e.to_string()))?;

        positions
            .into_iter()
            .filter(|p| p.position_amt != Decimal::ZERO)
            .map(|p| {
                let side = if p.position_amt > Decimal::ZERO {
                    Side::Long
                } else {
                    Side::Short
                };

                let quantity = p.position_amt.abs();
                let leverage: u8 = p.leverage.parse().map_err(|e: std::num::ParseIntError| {
                    BinanceRestError::ParseError(format!(
                        "Invalid leverage '{}': {}",
                        p.leverage, e
                    ))
                })?;

                Ok(FuturesPosition {
                    symbol: p.symbol,
                    side,
                    quantity: Quantity::new(quantity).map_err(|e| {
                        BinanceRestError::ParseError(format!("Invalid quantity: {}", e))
                    })?,
                    entry_price: Price::new(p.entry_price).map_err(|e| {
                        BinanceRestError::ParseError(format!("Invalid entry price: {}", e))
                    })?,
                    unrealized_pnl: p.unrealized_profit.unwrap_or(Decimal::ZERO),
                    leverage,
                })
            })
            .collect()
    }

    /// Ping Binance futures API to check connectivity.
    ///
    /// `GET /fapi/v1/ping` (public, no authentication required).
    pub async fn ping(&self) -> Result<(), BinanceRestError> {
        let body = self.get_public("/fapi/v1/ping", vec![]).await?;

        if body.trim() == "{}" {
            Ok(())
        } else {
            Err(BinanceRestError::ParseError(format!("Unexpected ping response: {}", body)))
        }
    }

    /// Query user trade history for a symbol.
    ///
    /// `GET /fapi/v1/userTrades` (signed).
    ///
    /// Returns trades ordered oldest-first (Binance default).
    pub async fn get_user_trades(
        &self,
        symbol: &str,
        start_time_ms: i64,
        limit: u16,
    ) -> Result<Vec<BinanceUserTrade>, BinanceRestError> {
        if limit == 0 || limit > 1000 {
            return Err(BinanceRestError::InvalidParameter(format!(
                "userTrades limit must be between 1 and 1000, got {}",
                limit
            )));
        }

        let params = vec![
            ("symbol", symbol.to_string()),
            ("startTime", start_time_ms.to_string()),
            ("limit", limit.to_string()),
        ];

        let body = self.get_signed("/fapi/v1/userTrades", params).await?;

        serde_json::from_str(&body).map_err(|e| BinanceRestError::ParseError(e.to_string()))
    }

    /// Get typed income history (ADR-0045 §1).
    ///
    /// `GET /fapi/v1/income` (signed). Deliberately no `symbol` filter: it
    /// would silently drop account-level items (e.g. `TRANSFER`) that carry
    /// no symbol. Returns items across the whole account since `start_time_ms`.
    pub async fn get_income(
        &self,
        start_time_ms: i64,
        limit: u16,
    ) -> Result<Vec<BinanceIncome>, BinanceRestError> {
        if limit == 0 || limit > 1000 {
            return Err(BinanceRestError::InvalidParameter(format!(
                "income limit must be between 1 and 1000, got {}",
                limit
            )));
        }

        let params = vec![
            ("startTime", start_time_ms.to_string()),
            ("limit", limit.to_string()),
        ];

        let body = self.get_signed("/fapi/v1/income", params).await?;

        serde_json::from_str(&body).map_err(|e| BinanceRestError::ParseError(e.to_string()))
    }

    /// Get USDT-M futures account balance.
    ///
    /// `GET /fapi/v2/balance` (signed).
    ///
    /// Returns per-asset balances. This method extracts the USDT entry.
    pub async fn get_futures_balance(&self) -> Result<BinanceFuturesBalance, BinanceRestError> {
        let body = self.get_signed("/fapi/v2/balance", vec![]).await?;
        parse_futures_balance(&body)
    }

    pub async fn get_spot_account_balances(
        &self,
    ) -> Result<Vec<BinanceSpotBalance>, BinanceRestError> {
        let body = self.get_signed("/api/v3/account", vec![]).await?;
        let account: BinanceSpotAccountResponse =
            serde_json::from_str(&body).map_err(|e| BinanceRestError::ParseError(e.to_string()))?;

        Ok(account
            .balances
            .into_iter()
            .filter(|b| b.free != Decimal::ZERO || b.locked != Decimal::ZERO)
            .collect())
    }

    pub async fn get_spot_price(&self, symbol: &str) -> Result<Price, BinanceRestError> {
        let body = self
            .get_public("/api/v3/ticker/price", vec![("symbol", symbol.to_string())])
            .await?;
        let response: PriceResponse =
            serde_json::from_str(&body).map_err(|e| BinanceRestError::ParseError(e.to_string()))?;

        Price::new(response.price)
            .map_err(|e| BinanceRestError::ParseError(format!("Invalid spot price: {}", e)))
    }

    pub async fn spot_symbol_is_trading(&self, symbol: &str) -> Result<bool, BinanceRestError> {
        let body = match self
            .get_public("/api/v3/exchangeInfo", vec![("symbol", symbol.to_string())])
            .await
        {
            Ok(body) => body,
            Err(BinanceRestError::ApiError { code: -1121, .. }) => return Ok(false),
            Err(error) => return Err(error),
        };
        let response: BinanceSpotExchangeInfoResponse =
            serde_json::from_str(&body).map_err(|e| BinanceRestError::ParseError(e.to_string()))?;

        Ok(response.symbols.into_iter().any(|info| info.status == "TRADING"))
    }

    /// Fetch the USD-M futures trading filters for one symbol from
    /// `/fapi/v1/exchangeInfo` (issue #154 deliverable 1).
    ///
    /// Quantization must be driven by `tickSize` (grid anchored at
    /// `minPrice`), never by `pricePrecision`: BTCUSDT futures has tickSize
    /// 0.10 with pricePrecision 2, so the two genuinely diverge.
    pub async fn get_futures_symbol_filters(
        &self,
        symbol: &str,
    ) -> Result<BinanceFuturesSymbolFilters, BinanceRestError> {
        let body = self
            .get_public("/fapi/v1/exchangeInfo", vec![("symbol", symbol.to_string())])
            .await?;
        let response: serde_json::Value =
            serde_json::from_str(&body).map_err(|e| BinanceRestError::ParseError(e.to_string()))?;

        let info = response["symbols"]
            .as_array()
            .and_then(|symbols| symbols.iter().find(|s| s["symbol"].as_str() == Some(symbol)))
            .ok_or_else(|| {
                BinanceRestError::ParseError(format!(
                    "exchangeInfo response has no symbol entry for {symbol}"
                ))
            })?;

        let decimal_field =
            |value: &serde_json::Value, name: &str| -> Result<Decimal, BinanceRestError> {
                value.as_str().and_then(|s| s.parse::<Decimal>().ok()).ok_or_else(|| {
                    BinanceRestError::ParseError(format!(
                        "exchangeInfo filter field {name} missing or not a decimal for {symbol}"
                    ))
                })
            };
        let filter = |filter_type: &str| -> Result<&serde_json::Value, BinanceRestError> {
            info["filters"]
                .as_array()
                .and_then(|filters| {
                    filters.iter().find(|f| f["filterType"].as_str() == Some(filter_type))
                })
                .ok_or_else(|| {
                    BinanceRestError::ParseError(format!(
                        "exchangeInfo response has no {filter_type} filter for {symbol}"
                    ))
                })
        };

        let price_filter = filter("PRICE_FILTER")?;
        let lot_size = filter("LOT_SIZE")?;
        // MARKET_LOT_SIZE bounds market orders (the only order kind Robson
        // sends); combine with LOT_SIZE taking the more restrictive value.
        let market_lot = filter("MARKET_LOT_SIZE").ok();
        let min_notional = filter("MIN_NOTIONAL")?;

        let mut step_size = decimal_field(&lot_size["stepSize"], "LOT_SIZE.stepSize")?;
        let mut min_qty = decimal_field(&lot_size["minQty"], "LOT_SIZE.minQty")?;
        let mut max_qty = decimal_field(&lot_size["maxQty"], "LOT_SIZE.maxQty")?;
        if let Some(market_lot) = market_lot {
            step_size =
                step_size.max(decimal_field(&market_lot["stepSize"], "MARKET_LOT_SIZE.stepSize")?);
            min_qty = min_qty.max(decimal_field(&market_lot["minQty"], "MARKET_LOT_SIZE.minQty")?);
            max_qty = max_qty.min(decimal_field(&market_lot["maxQty"], "MARKET_LOT_SIZE.maxQty")?);
        }

        Ok(BinanceFuturesSymbolFilters {
            symbol: symbol.to_string(),
            price_precision: info["pricePrecision"].as_u64().unwrap_or(0) as u32,
            quantity_precision: info["quantityPrecision"].as_u64().unwrap_or(0) as u32,
            tick_size: decimal_field(&price_filter["tickSize"], "PRICE_FILTER.tickSize")?,
            min_price: decimal_field(&price_filter["minPrice"], "PRICE_FILTER.minPrice")?,
            step_size,
            min_qty,
            max_qty,
            min_notional: decimal_field(&min_notional["notional"], "MIN_NOTIONAL.notional")?,
        })
    }

    pub async fn place_spot_market_order(
        &self,
        symbol: &str,
        side: &str,
        quantity_param: (&str, Decimal),
        client_order_id: &str,
    ) -> Result<BinanceSpotOrderResponse, BinanceRestError> {
        let params = vec![
            ("symbol", symbol.to_string()),
            ("side", side.to_string()),
            ("type", "MARKET".to_string()),
            (quantity_param.0, quantity_param.1.to_string()),
            ("newClientOrderId", client_order_id.to_string()),
            ("newOrderRespType", "FULL".to_string()),
        ];

        let body = self.post_signed("/api/v3/order", params).await?;
        serde_json::from_str(&body).map_err(|e| BinanceRestError::ParseError(e.to_string()))
    }

    pub async fn get_spot_order(
        &self,
        symbol: &str,
        client_order_id: &str,
    ) -> Result<Option<BinanceSpotOrderResponse>, BinanceRestError> {
        let params = vec![
            ("symbol", symbol.to_string()),
            ("origClientOrderId", client_order_id.to_string()),
        ];

        match self.get_signed("/api/v3/order", params).await {
            Ok(body) => serde_json::from_str(&body)
                .map(Some)
                .map_err(|e| BinanceRestError::ParseError(e.to_string())),
            Err(BinanceRestError::ApiError { code: -2013, .. }) => Ok(None),
            Err(error) => Err(error),
        }
    }

    pub async fn universal_transfer(
        &self,
        asset: &str,
        amount: Decimal,
        transfer_type: &str,
        client_tran_key: &str,
    ) -> Result<BinanceUniversalTransferResponse, BinanceRestError> {
        let params = vec![
            ("type", transfer_type.to_string()),
            ("asset", asset.to_string()),
            ("amount", amount.to_string()),
            ("clientTranId", client_tran_key.to_string()),
        ];

        let body = self.post_signed("/sapi/v1/asset/transfer", params).await?;
        serde_json::from_str(&body).map_err(|e| BinanceRestError::ParseError(e.to_string()))
    }

    pub async fn get_transfer_history(
        &self,
        transfer_type: &str,
        start_time_ms: i64,
    ) -> Result<Vec<BinanceUniversalTransfer>, BinanceRestError> {
        let params = vec![
            ("type", transfer_type.to_string()),
            ("startTime", start_time_ms.to_string()),
        ];

        let body = self.get_signed("/sapi/v1/asset/transfer", params).await?;
        let response: BinanceUniversalTransferHistoryResponse =
            serde_json::from_str(&body).map_err(|e| BinanceRestError::ParseError(e.to_string()))?;
        Ok(response.rows)
    }
}

// =============================================================================
// Binance Types (from API responses)
// =============================================================================

/// One answer from the venue, of any HTTP status.
pub(crate) struct RestAnswer {
    pub(crate) status: StatusCode,
    pub(crate) body: String,
}

impl RestAnswer {
    /// The body of a success. Any other status is an error: `ApiError` when
    /// the body is Binance's JSON error, `RequestFailed` otherwise.
    fn into_body(self) -> Result<String, BinanceRestError> {
        let RestAnswer { status, body } = self;
        if !status.is_success() {
            if let Ok(err) = serde_json::from_str::<BinanceErrorResponse>(&body) {
                return Err(BinanceRestError::ApiError {
                    code: err.code,
                    msg: err.msg,
                    http_status: status.as_u16(),
                });
            }
            return Err(BinanceRestError::RequestFailed(format!("HTTP {}: {}", status, body)));
        }

        Ok(body)
    }
}

/// Binance error response.
#[derive(Debug, Deserialize)]
struct BinanceErrorResponse {
    code: i64,
    msg: String,
}

/// Response from `GET /fapi/v2/positionRisk`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PositionRiskResponse {
    symbol: String,
    position_amt: Decimal,
    entry_price: Decimal,
    /// Unrealized profit. Optional for Binance testnet compatibility.
    /// Testnet API may omit this field; production API always includes it.
    #[serde(default)]
    unrealized_profit: Option<Decimal>,
    leverage: String,
}

/// Position detected in USD-M futures.
#[derive(Debug, Clone)]
pub struct FuturesPosition {
    /// Trading symbol (e.g., "BTCUSDT")
    pub symbol: String,
    /// Position side (Long if positionAmt > 0, Short if < 0)
    pub side: Side,
    /// Position quantity (absolute value of positionAmt)
    pub quantity: Quantity,
    /// Entry price
    pub entry_price: Price,
    /// Unrealized PnL
    pub unrealized_pnl: Decimal,
    /// Current leverage
    pub leverage: u8,
}

/// Parsed Binance futures kline.
#[derive(Debug, Clone, PartialEq)]
pub struct BinanceKline {
    /// Candle open time in milliseconds since epoch
    pub open_time_ms: i64,
    /// Open price
    pub open: Decimal,
    /// High price
    pub high: Decimal,
    /// Low price
    pub low: Decimal,
    /// Close price
    pub close: Decimal,
    /// Base asset volume
    pub volume: Decimal,
    /// Candle close time in milliseconds since epoch
    pub close_time_ms: i64,
    /// Number of trades
    pub trades: u64,
}

/// Binance USD-M Futures order response (FAPI).
///
/// Field names match `/fapi/v1/order` response format:
/// - `updateTime` (not `transactTime` which is spot)
/// - `cumQuote` (not `cummulativeQuoteQty` which is spot)
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BinanceOrderResponse {
    /// Symbol
    pub symbol: String,
    /// Order ID
    pub order_id: u64,
    /// Client order ID
    pub client_order_id: String,
    /// Update time (ms since epoch); FAPI uses `updateTime`
    pub update_time: i64,
    /// Price
    pub price: Decimal,
    /// Average fill price (futures RESULT response)
    #[serde(default)]
    pub avg_price: Decimal,
    /// Original quantity
    pub orig_qty: Decimal,
    /// Executed quantity
    pub executed_qty: Decimal,
    /// Cumulative quote quantity — futures uses `cumQuote`
    #[serde(default)]
    pub cum_quote: Decimal,
    /// Status
    pub status: String,
    /// Side
    pub side: String,
    /// Type
    #[serde(rename = "type")]
    pub order_type: String,
    /// Individual fills (present in spot, usually empty in futures)
    #[serde(default)]
    pub fills: Vec<BinanceFill>,
}

/// Response from `POST /fapi/v1/algoOrder`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BinanceAlgoOrderResponse {
    /// Exchange-assigned algo order id
    pub algo_id: i64,
    /// Client-provided algo id
    pub client_algo_id: String,
    /// Algo type (`CONDITIONAL`)
    pub algo_type: String,
    /// Underlying conditional order type (`STOP_MARKET`)
    pub order_type: String,
    /// Symbol
    pub symbol: String,
    /// Side (`BUY` / `SELL`)
    pub side: String,
    /// Original quantity
    pub quantity: Decimal,
    /// Algo status (`NEW`, `CANCELED`, `TRIGGERED`, ...)
    pub algo_status: String,
    /// Trigger price
    pub trigger_price: Decimal,
    /// Whether the triggered order can only reduce a position
    pub reduce_only: bool,
    /// Create time (ms since epoch)
    pub create_time: i64,
    /// Update time (ms since epoch)
    pub update_time: i64,
}

/// Response from `DELETE /fapi/v1/algoOrder`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BinanceAlgoCancelResponse {
    pub algo_id: i64,
    pub client_algo_id: String,
    /// Binance returns this as the STRING "200" on success (2026-07-04 prod
    /// incident: parsing it as i64 aborted cancel-replace AFTER the exchange
    /// had already cancelled the stop, leaving the position unprotected).
    /// Accept both encodings.
    #[serde(deserialize_with = "de_i64_from_int_or_string")]
    pub code: i64,
    pub msg: String,
}

/// Deserialize an i64 that Binance may encode as a JSON number or string.
fn de_i64_from_int_or_string<'de, D>(deserializer: D) -> Result<i64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum IntOrString {
        Int(i64),
        Str(String),
    }
    match IntOrString::deserialize(deserializer)? {
        IntOrString::Int(v) => Ok(v),
        IntOrString::Str(s) => s.parse::<i64>().map_err(serde::de::Error::custom),
    }
}

/// Response from `GET /fapi/v1/algoOrder`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BinanceAlgoOrderDetail {
    pub algo_id: i64,
    pub client_algo_id: String,
    pub algo_type: String,
    pub order_type: String,
    pub symbol: String,
    pub side: String,
    pub quantity: Decimal,
    pub algo_status: String,
    pub trigger_price: Decimal,
    pub reduce_only: bool,
    pub create_time: i64,
    pub update_time: i64,
    #[serde(default)]
    pub actual_order_id: String,
    #[serde(default)]
    pub actual_price: Decimal,
    #[serde(default)]
    pub actual_qty: Decimal,
    #[serde(default)]
    pub actual_type: String,
    #[serde(default)]
    pub trigger_time: i64,
}

/// Individual fill from a Binance order response.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BinanceFill {
    /// Fill price
    pub price: Decimal,
    /// Fill quantity
    pub qty: Decimal,
    /// Commission amount
    pub commission: Decimal,
    /// Commission asset
    pub commission_asset: String,
}

/// An open algo order returned by `GET /fapi/v1/openAlgoOrders`.
///
/// Only the fields the orphan insurance-order sweep needs (ADR-0039).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BinanceOpenAlgoOrder {
    /// Exchange-assigned algo id
    pub algo_id: i64,
    /// Client-provided algo id (robsond insurance stops carry the `ins-`
    /// prefix)
    pub client_algo_id: String,
    /// Order side (`BUY` / `SELL`)
    pub side: String,
    /// Order type (e.g. `STOP_MARKET`, `MARKET`)
    pub order_type: String,
    /// Algo type (`CONDITIONAL`)
    pub algo_type: String,
    /// Algo status (e.g. `NEW`)
    pub algo_status: String,
    /// Whether the order can only reduce an existing position
    #[serde(default)]
    pub reduce_only: bool,
    /// Trigger price for conditional orders; `0` when not applicable
    #[serde(default)]
    pub trigger_price: Decimal,
    /// Original quantity
    pub quantity: Decimal,
    /// Create time (ms since epoch)
    pub create_time: i64,
    /// Update time (ms since epoch)
    pub update_time: i64,
}

/// Price ticker response.
#[derive(Debug, Deserialize)]
struct PriceResponse {
    symbol: String,
    price: Decimal,
}

/// Response from `GET /fapi/v2/balance`.
///
/// Binance returns `balance` (not `walletBalance`) for the total wallet field,
/// but `availableBalance` (camelCase) for the available field.
#[derive(Debug, Clone, Deserialize)]
#[allow(non_snake_case)]
struct BinanceBalanceResponse {
    asset: String,
    balance: Decimal,
    availableBalance: Decimal,
    /// Unrealized P&L of cross-margin positions. Defaulted so a response
    /// without it never fails the balance fetch.
    #[serde(default)]
    crossUnPnl: Decimal,
}

/// Individual user trade from Binance USD-M futures.
///
/// Response from `GET /fapi/v1/userTrades`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BinanceUserTrade {
    /// Trade ID
    pub id: u64,
    /// Order ID
    pub order_id: u64,
    /// Symbol
    pub symbol: String,
    /// Fill price
    pub price: Decimal,
    /// Filled quantity
    pub qty: Decimal,
    /// Quote quantity
    pub quote_qty: Decimal,
    /// Commission (fee)
    pub commission: Decimal,
    /// Commission asset (fee asset)
    pub commission_asset: String,
    /// Trade time (ms since epoch)
    pub time: i64,
}

/// Single item from `GET /fapi/v1/income` (ADR-0045 §1).
///
/// Field shapes verified live against the production account 2026-07-07
/// (read-only): `symbol` is `""` for account-level items with no symbol
/// (not omitted, not null — this was NOT assumed, it is Binance's actual
/// wire behavior per public docs and is defended here rather than assumed).
/// `tradeId` is an empty string for `FUNDING_FEE` (funding never links to a
/// trade) and a numeric string for `COMMISSION`/`REALIZED_PNL`. `tranId` is
/// always present.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BinanceIncome {
    /// Trading symbol, or `""` for account-level items (e.g. `TRANSFER`).
    pub symbol: String,
    pub income_type: String,
    pub income: Decimal,
    pub asset: String,
    pub time: i64,
    #[serde(default)]
    pub info: String,
    pub tran_id: i64,
    /// Empty string when the exchange provides no trade linkage.
    #[serde(default)]
    pub trade_id: String,
}

/// Parsed USDT-M futures balance for the USDT asset.
#[derive(Debug, Clone)]
pub struct BinanceFuturesBalance {
    /// Binance `balance` (wallet balance). Mapping unchanged (ADR-0056 §2.4);
    /// the separate `unrealized_pnl` is reported next to it.
    pub wallet_balance: Decimal,
    /// Balance available for new positions.
    pub available_balance: Decimal,
    /// Binance `crossUnPnl`: unrealized P&L of cross-margin positions
    /// (isolated positions report theirs in `positionRisk`).
    pub unrealized_pnl: Decimal,
}

#[derive(Debug, Clone, Deserialize)]
struct BinanceSpotAccountResponse {
    balances: Vec<BinanceSpotBalance>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BinanceSpotBalance {
    pub asset: String,
    pub free: Decimal,
    pub locked: Decimal,
}

#[derive(Debug, Clone, Deserialize)]
struct BinanceSpotExchangeInfoResponse {
    symbols: Vec<BinanceSpotSymbolInfo>,
}

/// USD-M futures trading filters for one symbol (issue #154 deliverable 1).
///
/// `step_size`/`min_qty`/`max_qty` already combine LOT_SIZE and
/// MARKET_LOT_SIZE taking the more restrictive value, because Robson only
/// sends market and stop-market orders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinanceFuturesSymbolFilters {
    /// Trading pair (e.g. "BTCUSDT").
    pub symbol: String,
    /// Exchange-reported price precision (informational; quantization uses
    /// `tick_size`).
    pub price_precision: u32,
    /// Exchange-reported quantity precision (informational).
    pub quantity_precision: u32,
    /// PRICE_FILTER tickSize.
    pub tick_size: Decimal,
    /// PRICE_FILTER minPrice (tick grid anchor).
    pub min_price: Decimal,
    /// Combined LOT_SIZE / MARKET_LOT_SIZE stepSize.
    pub step_size: Decimal,
    /// Combined LOT_SIZE / MARKET_LOT_SIZE minQty.
    pub min_qty: Decimal,
    /// Combined LOT_SIZE / MARKET_LOT_SIZE maxQty.
    pub max_qty: Decimal,
    /// MIN_NOTIONAL notional threshold.
    pub min_notional: Decimal,
}

#[derive(Debug, Clone, Deserialize)]
struct BinanceSpotSymbolInfo {
    status: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BinanceSpotOrderResponse {
    pub symbol: String,
    pub order_id: u64,
    pub client_order_id: String,
    #[serde(default)]
    pub transact_time: i64,
    #[serde(default)]
    pub update_time: i64,
    pub status: String,
    pub executed_qty: Decimal,
    pub cummulative_quote_qty: Decimal,
    #[serde(default)]
    pub fills: Vec<BinanceFill>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BinanceUniversalTransferResponse {
    pub tran_id: u64,
    #[serde(default)]
    pub client_tran_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BinanceUniversalTransferHistoryResponse {
    #[serde(default)]
    pub rows: Vec<BinanceUniversalTransfer>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BinanceUniversalTransfer {
    pub tran_id: u64,
    #[serde(default)]
    pub client_tran_id: Option<String>,
    pub asset: String,
    pub amount: Decimal,
    #[serde(rename = "type")]
    pub transfer_type: String,
    pub status: String,
    pub timestamp: i64,
}

/// Extract the USDT entry of a `/fapi/v2/balance` response.
pub(crate) fn parse_futures_balance(body: &str) -> Result<BinanceFuturesBalance, BinanceRestError> {
    let balances: Vec<BinanceBalanceResponse> =
        serde_json::from_str(body).map_err(|e| BinanceRestError::ParseError(e.to_string()))?;

    let usdt = balances.into_iter().find(|b| b.asset == "USDT").ok_or_else(|| {
        BinanceRestError::ParseError("No USDT entry in futures balance response".to_string())
    })?;

    Ok(BinanceFuturesBalance {
        wallet_balance: usdt.balance,
        available_balance: usdt.availableBalance,
        unrealized_pnl: usdt.crossUnPnl,
    })
}

fn parse_futures_klines(body: &str) -> Result<Vec<BinanceKline>, BinanceRestError> {
    let rows: Vec<Vec<Value>> =
        serde_json::from_str(body).map_err(|e| BinanceRestError::ParseError(e.to_string()))?;

    rows.into_iter()
        .enumerate()
        .map(|(idx, row)| parse_futures_kline(idx, row))
        .collect()
}

fn parse_futures_kline(idx: usize, row: Vec<Value>) -> Result<BinanceKline, BinanceRestError> {
    if row.len() < 9 {
        return Err(BinanceRestError::ParseError(format!(
            "kline row {} has {} fields, expected at least 9",
            idx,
            row.len()
        )));
    }

    Ok(BinanceKline {
        open_time_ms: parse_i64_field(&row, idx, 0, "open_time")?,
        open: parse_decimal_field(&row, idx, 1, "open")?,
        high: parse_decimal_field(&row, idx, 2, "high")?,
        low: parse_decimal_field(&row, idx, 3, "low")?,
        close: parse_decimal_field(&row, idx, 4, "close")?,
        volume: parse_decimal_field(&row, idx, 5, "volume")?,
        close_time_ms: parse_i64_field(&row, idx, 6, "close_time")?,
        trades: parse_u64_field(&row, idx, 8, "trades")?,
    })
}

fn parse_decimal_field(
    row: &[Value],
    row_idx: usize,
    field_idx: usize,
    field_name: &str,
) -> Result<Decimal, BinanceRestError> {
    let raw = row
        .get(field_idx)
        .and_then(Value::as_str)
        .ok_or_else(|| parse_field_error(row_idx, field_idx, field_name, "decimal string"))?;

    raw.parse::<Decimal>().map_err(|e| {
        BinanceRestError::ParseError(format!(
            "failed to parse kline row {} field {} ({}) as decimal: {}",
            row_idx, field_idx, field_name, e
        ))
    })
}

fn parse_i64_field(
    row: &[Value],
    row_idx: usize,
    field_idx: usize,
    field_name: &str,
) -> Result<i64, BinanceRestError> {
    row.get(field_idx)
        .and_then(Value::as_i64)
        .ok_or_else(|| parse_field_error(row_idx, field_idx, field_name, "integer"))
}

fn parse_u64_field(
    row: &[Value],
    row_idx: usize,
    field_idx: usize,
    field_name: &str,
) -> Result<u64, BinanceRestError> {
    row.get(field_idx)
        .and_then(Value::as_u64)
        .ok_or_else(|| parse_field_error(row_idx, field_idx, field_name, "unsigned integer"))
}

fn parse_field_error(
    row_idx: usize,
    field_idx: usize,
    field_name: &str,
    expected: &str,
) -> BinanceRestError {
    BinanceRestError::ParseError(format!(
        "kline row {} field {} ({}) is not a {}",
        row_idx, field_idx, field_name, expected
    ))
}

// =============================================================================
// Tests
// =============================================================================

/// The origin of a crash-harness venue URL, parsed and checked here so that a
/// lookalike such as `http://127.0.0.1:80@evil.example` cannot leave loopback.
/// The port must be explicit, and credentials, paths and queries are refused.
#[cfg(any(test, feature = "crash-points"))]
fn harness_origin(base_url: &str) -> Result<String, String> {
    let url = reqwest::Url::parse(base_url).map_err(|error| format!("{base_url}: {error}"))?;
    let host = url.host_str().unwrap_or_default();
    let loopback = host == "localhost"
        || host
            .trim_matches(['[', ']'])
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback());
    let bare_origin = url.path() == "/" && url.query().is_none() && url.fragment().is_none();
    let no_credentials = url.username().is_empty() && url.password().is_none();
    if url.scheme() != "http"
        || !loopback
        || url.port().is_none()
        || !bare_origin
        || !no_credentials
    {
        return Err(format!("{base_url} is not an http://<loopback>:<port> origin"));
    }
    Ok(url.origin().ascii_serialization())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use robson_exec::{gate::OperationKind, order_journal::OrderJournal};
    use rust_decimal_macros::dec;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    use super::*;

    /// A venue that answers every request `{}` with 200, its head after
    /// `head_after` and its body `body_after` later, and counts the requests.
    async fn slow_venue(head_after: Duration, body_after: Duration) -> (String, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&requests);
        tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let counted = Arc::clone(&counted);
                tokio::spawn(async move {
                    let mut head = Vec::new();
                    let mut buf = [0u8; 4096];
                    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                        match stream.read(&mut buf).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => head.extend_from_slice(&buf[..n]),
                        }
                    }
                    counted.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(head_after).await;
                    let head = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                                Content-Length: 2\r\nConnection: close\r\n\r\n";
                    let _ = stream.write_all(head.as_bytes()).await;
                    let _ = stream.flush().await;
                    tokio::time::sleep(body_after).await;
                    let _ = stream.write_all(b"{}").await;
                });
            }
        });
        (url, requests)
    }

    #[test]
    fn one_request_is_bounded_at_ten_seconds() {
        assert_eq!(REQUEST_TIMEOUT, Duration::from_secs(10));
    }

    /// D19: one bound from the send to the last byte of the body. A request
    /// whose head and body each arrive inside the bound, but not both, is a
    /// timeout, which callers treat as uncertain.
    #[tokio::test]
    async fn one_bound_covers_the_send_and_the_body() {
        let bound = Duration::from_millis(400);
        let (url, requests) = slow_venue(bound * 3 / 5, bound * 3 / 5).await;
        let client = BinanceRestClient::with_local_base_url(url).with_request_timeout(bound);
        let started = std::time::Instant::now();
        let answered = client.get_public("/fapi/v1/ping", vec![]).await;
        assert!(matches!(answered, Err(BinanceRestError::Timeout)), "{answered:?}");
        assert!(
            started.elapsed() < bound * 6 / 5,
            "it ends at the bound: {:?}",
            started.elapsed()
        );
        assert_eq!(requests.load(Ordering::SeqCst), 1);

        let (url, _) = slow_venue(bound / 5, bound / 5).await;
        let client = BinanceRestClient::with_local_base_url(url).with_request_timeout(bound);
        assert_eq!(client.get_public("/fapi/v1/ping", vec![]).await.unwrap(), "{}");
    }

    /// ADR-0056 step 8a split the bounded send from the error mapping; a
    /// non-success answer maps as it did before.
    #[test]
    fn a_non_success_answer_maps_as_before() {
        let answer = |status: StatusCode, body: &str| RestAnswer { status, body: body.to_string() };
        assert_eq!(answer(StatusCode::OK, "{}").into_body().unwrap(), "{}");
        let api =
            answer(StatusCode::BAD_REQUEST, r#"{"code":-2013,"msg":"Order does not exist."}"#);
        assert!(matches!(
            api.into_body(),
            Err(BinanceRestError::ApiError { code: -2013, http_status: 400, msg })
                if msg == "Order does not exist."
        ));
        match answer(StatusCode::BAD_GATEWAY, "<html></html>").into_body() {
            Err(BinanceRestError::RequestFailed(msg)) => {
                assert_eq!(msg, "HTTP 502 Bad Gateway: <html></html>")
            },
            other => panic!("expected RequestFailed, got {other:?}"),
        }
    }

    /// The choke point (ADR-0056 step 7) without a permit: counted and latched
    /// while open, refused before anything is sent once stopping. A read is
    /// not a mutation.
    #[tokio::test]
    async fn a_mutation_without_a_permit_is_counted_while_open_and_refused_once_stopping() {
        let (url, requests) = slow_venue(Duration::ZERO, Duration::ZERO).await;
        let client = BinanceRestClient::with_local_base_url(url);
        let journal = Arc::new(OrderJournal::unpersisted());
        client.attach_gate(Arc::clone(journal.gate()));

        client.set_leverage("BTCUSDT", 1).await.unwrap();
        assert_eq!(requests.load(Ordering::SeqCst), 1);
        assert_eq!(journal.gate().snapshot().unpermitted, 1);
        assert!(journal.latch_set());
        client.get_position_mode().await.unwrap();
        assert_eq!(journal.gate().snapshot().unpermitted, 1);

        journal.gate().begin_draining();
        let refused = client.set_leverage("BTCUSDT", 1).await;
        assert!(matches!(refused, Err(BinanceRestError::Refused(_))), "{refused:?}");
        journal.gate().begin_closing();
        let refused = client.cancel_algo_order(7).await;
        assert!(matches!(refused, Err(BinanceRestError::Refused(_))), "{refused:?}");

        assert_eq!(requests.load(Ordering::SeqCst), 2, "nothing refused reached the venue");
        assert_eq!(journal.gate().snapshot().unpermitted, 1);
    }

    /// With a permit a mutation goes out in every phase, uncounted.
    #[tokio::test]
    async fn a_mutation_under_a_permit_goes_out_in_every_phase() {
        let (url, requests) = slow_venue(Duration::ZERO, Duration::ZERO).await;
        let client = BinanceRestClient::with_local_base_url(url);
        let journal = Arc::new(OrderJournal::unpersisted());
        client.attach_gate(Arc::clone(journal.gate()));

        let permit = journal.acquire(OperationKind::Panic).unwrap();
        permit
            .scope(async {
                client.set_leverage("BTCUSDT", 1).await.unwrap();
                journal.gate().begin_draining();
                client.set_leverage("BTCUSDT", 1).await.unwrap();
                journal.gate().begin_closing();
                client.set_leverage("BTCUSDT", 1).await.unwrap();
            })
            .await;
        permit.complete();

        assert_eq!(requests.load(Ordering::SeqCst), 3);
        assert_eq!(journal.gate().snapshot().unpermitted, 0);
        assert!(!journal.latch_set());
    }

    #[test]
    fn test_algo_cancel_response_accepts_string_and_numeric_code() {
        // Regression: 2026-07-04 prod. Binance answered the algo cancel with
        // "code": "200" (string); parsing it as i64 failed AFTER the exchange
        // had cancelled the stop, so cancel-replace aborted before placing
        // the new one and the position was left without exchange-side
        // protection.
        let as_string =
            r#"{"algoId":2000001240350523,"clientAlgoId":"ins-x","code":"200","msg":"OK"}"#;
        let parsed: BinanceAlgoCancelResponse = serde_json::from_str(as_string).unwrap();
        assert_eq!(parsed.code, 200);
        assert_eq!(parsed.algo_id, 2000001240350523);

        let as_number = r#"{"algoId":1,"clientAlgoId":"ins-y","code":200,"msg":"OK"}"#;
        let parsed: BinanceAlgoCancelResponse = serde_json::from_str(as_number).unwrap();
        assert_eq!(parsed.code, 200);
    }

    #[test]
    fn test_build_signed_query() {
        let client = BinanceRestClient::new("test_key".to_string(), "test_secret".to_string());

        let params = vec![("symbol", "BTCUSDT".to_string())];
        let query = client.build_signed_query(params).unwrap();

        assert!(query.contains("timestamp="));
        assert!(query.contains("signature="));
        assert!(query.contains("symbol=BTCUSDT"));
    }

    #[test]
    fn test_build_signed_query_sorts_params() {
        let client = BinanceRestClient::new("test_key".to_string(), "test_secret".to_string());

        let params = vec![
            ("symbol", "BTCUSDT".to_string()),
            ("side", "SELL".to_string()),
        ];
        let query = client.build_signed_query(params).unwrap();

        let side_idx = query.find("side=").unwrap();
        let symbol_idx = query.find("symbol=").unwrap();
        assert!(side_idx < symbol_idx);
    }

    #[test]
    fn test_futures_position_creation() {
        let position = FuturesPosition {
            symbol: "BTCUSDT".to_string(),
            side: Side::Long,
            quantity: Quantity::new(dec!(0.1)).unwrap(),
            entry_price: Price::new(dec!(95000)).unwrap(),
            unrealized_pnl: dec!(5.0),
            leverage: 10,
        };

        assert_eq!(position.symbol, "BTCUSDT");
        assert_eq!(position.side, Side::Long);
        assert_eq!(position.quantity.as_decimal(), dec!(0.1));
        assert_eq!(position.leverage, 10);
    }

    #[test]
    fn test_parse_futures_klines() {
        let body = r#"[
            [
                1499040000000,
                "0.01634790",
                "0.80000000",
                "0.01575800",
                "0.01577100",
                "148976.11427815",
                1499644799999,
                "2434.19055334",
                308,
                "1756.87402397",
                "28.46694368",
                "17928899.62484339"
            ]
        ]"#;

        let klines = parse_futures_klines(body).unwrap();

        assert_eq!(klines.len(), 1);
        assert_eq!(klines[0].open_time_ms, 1_499_040_000_000);
        assert_eq!(klines[0].open, dec!(0.01634790));
        assert_eq!(klines[0].high, dec!(0.80000000));
        assert_eq!(klines[0].low, dec!(0.01575800));
        assert_eq!(klines[0].close, dec!(0.01577100));
        assert_eq!(klines[0].volume, dec!(148976.11427815));
        assert_eq!(klines[0].close_time_ms, 1_499_644_799_999);
        assert_eq!(klines[0].trades, 308);
    }

    #[test]
    fn the_harness_venue_url_must_be_a_loopback_origin() {
        for accepted in [
            "http://127.0.0.1:41000",
            "http://localhost:41000",
            "http://[::1]:41000",
        ] {
            assert_eq!(harness_origin(accepted).as_deref(), Ok(accepted));
        }
        for refused in [
            // `127.0.0.1:80` is user information; the host is evil.example.
            "http://127.0.0.1:80@evil.example",
            // Not a port at all.
            "http://localhost:80.evil",
            "http://127.0.0.1:41000@evil.example:41000",
            "http://user:secret@127.0.0.1:41000",
            "https://127.0.0.1:41000",
            "http://evil.example:41000",
            "http://localhost.evil:41000",
            "http://127.0.0.1.evil.example:41000",
            "http://127.0.0.1",
            "http://127.0.0.1:41000/fapi",
            "http://127.0.0.1:41000?a=1",
            "not a url",
        ] {
            assert!(harness_origin(refused).is_err(), "{refused} must be refused");
        }
    }

    #[tokio::test]
    async fn test_get_price_requires_no_signature() {
        let client = BinanceRestClient::new("key".to_string(), "secret".to_string());
        let _ = client.get_price("BTCUSDT");
    }

    #[test]
    fn test_binance_order_response_defaults_missing_cum_quote() {
        let body = r#"{
            "symbol": "BTCUSDT",
            "orderId": 123456,
            "clientOrderId": "signal-uuid",
            "updateTime": 1719372600000,
            "price": "0.00000000",
            "avgPrice": "59300.00000000",
            "origQty": "0.00257030",
            "executedQty": "0.00257030",
            "status": "FILLED",
            "side": "BUY",
            "type": "MARKET",
            "fills": []
        }"#;

        let response: BinanceOrderResponse = serde_json::from_str(body).unwrap();
        assert_eq!(response.cum_quote, Decimal::ZERO);
        assert_eq!(response.executed_qty, dec!(0.00257030));
        assert_eq!(response.avg_price, dec!(59300.00000000));
    }

    #[test]
    fn test_position_risk_response_parsing() {
        let body = r#"[
            {
                "symbol": "BTCUSDT",
                "initialMargin": "5.00000000",
                "maintMargin": "0.25000000",
                "unrealizedProfit": "0.10000000",
                "positionInitialMargin": "5.00000000",
                "openOrderInitialMargin": "0.00000000",
                "leverage": "10",
                "isolated": false,
                "entryPrice": "75000.00000000",
                "maxNotional": "1000000.00000000",
                "positionSide": "BOTH",
                "positionAmt": "0.00100000",
                "updatedTime": 1234567890000
            },
            {
                "symbol": "ETHUSDT",
                "initialMargin": "0",
                "maintMargin": "0",
                "unrealizedProfit": "0",
                "positionInitialMargin": "0",
                "openOrderInitialMargin": "0",
                "leverage": "10",
                "isolated": false,
                "entryPrice": "0.00000000",
                "maxNotional": "1000000.00000000",
                "positionSide": "BOTH",
                "positionAmt": "0",
                "updatedTime": 1234567890000
            }
        ]"#;

        let positions: Vec<PositionRiskResponse> = serde_json::from_str(body).unwrap();
        // ETHUSDT has zero positionAmt — should be filtered
        let active: Vec<_> =
            positions.into_iter().filter(|p| p.position_amt != Decimal::ZERO).collect();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].symbol, "BTCUSDT");
        assert_eq!(active[0].position_amt, dec!(0.001));
        assert_eq!(active[0].leverage, "10");
    }

    #[test]
    fn test_balance_response_parsing() {
        let body = r#"[
            {
                "accountAlias": "SgsR",
                "asset": "USDT",
                "balance": "122607.35137903",
                "crossWalletBalance": "23.72469206",
                "crossUnPnl": "0.00000000",
                "availableBalance": "23.72469206",
                "maxWithdrawAmount": "23.72469206",
                "marginAvailable": true,
                "updateTime": 1617939110373
            },
            {
                "accountAlias": "SgsR",
                "asset": "BNB",
                "balance": "0.00000000",
                "crossWalletBalance": "0.00000000",
                "crossUnPnl": "0.00000000",
                "availableBalance": "0.00000000",
                "maxWithdrawAmount": "0.00000000",
                "marginAvailable": true,
                "updateTime": 1617939110373
            }
        ]"#;

        let balances: Vec<BinanceBalanceResponse> = serde_json::from_str(body).unwrap();
        let usdt = balances.into_iter().find(|b| b.asset == "USDT").unwrap();

        assert_eq!(usdt.balance, dec!(122607.35137903));
        assert_eq!(usdt.availableBalance, dec!(23.72469206));
        assert_eq!(usdt.crossUnPnl, dec!(0));
    }

    #[test]
    fn test_balance_response_maps_unrealized_pnl_separately() {
        let body = r#"[{"asset": "USDT", "balance": "1000.5", "crossUnPnl": "-12.25",
                        "availableBalance": "900"}, {"asset": "BNB", "balance": "1",
                        "availableBalance": "1"}]"#;

        let balances: Vec<BinanceBalanceResponse> = serde_json::from_str(body).unwrap();

        // wallet_balance keeps mapping `balance`; unrealized P&L is separate,
        // and a response without `crossUnPnl` still parses (as zero).
        assert_eq!((balances[0].balance, balances[0].crossUnPnl), (dec!(1000.5), dec!(-12.25)));
        assert_eq!((balances[1].balance, balances[1].crossUnPnl), (dec!(1), dec!(0)));
    }

    #[test]
    fn test_algo_order_response_parsing() {
        let body = r#"{
            "algoId": 998877,
            "clientAlgoId": "ins-019f10a322b77093bb2fc66a51882bf5",
            "algoType": "CONDITIONAL",
            "orderType": "STOP_MARKET",
            "symbol": "BTCUSDT",
            "side": "SELL",
            "quantity": "0.004",
            "algoStatus": "NEW",
            "triggerPrice": "58888.00",
            "reduceOnly": true,
            "createTime": 1783045560000,
            "updateTime": 1783045560001
        }"#;

        let response: BinanceAlgoOrderResponse = serde_json::from_str(body).unwrap();
        assert_eq!(response.algo_id, 998877);
        assert_eq!(response.client_algo_id, "ins-019f10a322b77093bb2fc66a51882bf5");
        assert_eq!(response.algo_type, "CONDITIONAL");
        assert_eq!(response.order_type, "STOP_MARKET");
        assert_eq!(response.algo_status, "NEW");
        assert_eq!(response.trigger_price, dec!(58888.00));
        assert!(response.reduce_only);
    }

    #[test]
    fn test_algo_cancel_response_parsing() {
        let body = r#"{
            "algoId": 998877,
            "clientAlgoId": "ins-019f10a322b77093bb2fc66a51882bf5",
            "code": 200,
            "msg": "success"
        }"#;

        let response: BinanceAlgoCancelResponse = serde_json::from_str(body).unwrap();
        assert_eq!(response.algo_id, 998877);
        assert_eq!(response.client_algo_id, "ins-019f10a322b77093bb2fc66a51882bf5");
        assert_eq!(response.code, 200);
        assert_eq!(response.msg, "success");
    }

    #[test]
    fn test_algo_order_detail_response_parsing() {
        let body = r#"{
            "algoId": 998877,
            "clientAlgoId": "ins-019f10a322b77093bb2fc66a51882bf5",
            "algoType": "CONDITIONAL",
            "orderType": "STOP_MARKET",
            "symbol": "BTCUSDT",
            "side": "SELL",
            "quantity": "0.004",
            "algoStatus": "TRIGGERED",
            "triggerPrice": "58888.00",
            "reduceOnly": true,
            "createTime": 1783045560000,
            "updateTime": 1783045570000,
            "actualOrderId": "123456789",
            "actualPrice": "58880.50",
            "actualQty": "0.004",
            "actualType": "MARKET",
            "triggerTime": 1783045569000
        }"#;

        let detail: BinanceAlgoOrderDetail = serde_json::from_str(body).unwrap();
        assert_eq!(detail.algo_id, 998877);
        assert_eq!(detail.algo_status, "TRIGGERED");
        assert_eq!(detail.actual_order_id, "123456789");
        assert_eq!(detail.actual_price, dec!(58880.50));
        assert_eq!(detail.actual_qty, dec!(0.004));
        assert_eq!(detail.actual_type, "MARKET");
        assert_eq!(detail.trigger_time, 1783045569000);
    }

    #[test]
    fn test_open_algo_order_response_parsing() {
        let body = r#"[
            {
                "symbol": "BTCUSDT",
                "algoId": 998877,
                "clientAlgoId": "ins-019f10a322b77093bb2fc66a51882bf5",
                "side": "SELL",
                "orderType": "STOP_MARKET",
                "algoType": "CONDITIONAL",
                "algoStatus": "NEW",
                "reduceOnly": true,
                "triggerPrice": "58888.00",
                "quantity": "0.004",
                "createTime": 1783045560000,
                "updateTime": 1783045560001
            }
        ]"#;

        let orders: Vec<BinanceOpenAlgoOrder> = serde_json::from_str(body).unwrap();
        assert_eq!(orders.len(), 1);

        let order = &orders[0];
        assert_eq!(order.algo_id, 998877);
        assert_eq!(order.client_algo_id, "ins-019f10a322b77093bb2fc66a51882bf5");
        assert_eq!(order.side, "SELL");
        assert_eq!(order.order_type, "STOP_MARKET");
        assert_eq!(order.algo_type, "CONDITIONAL");
        assert_eq!(order.algo_status, "NEW");
        assert!(order.reduce_only);
        assert_eq!(order.trigger_price, dec!(58888.00));
        assert_eq!(order.quantity, dec!(0.004));
    }
}
