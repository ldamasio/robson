//! Binance REST API Client for Isolated Margin Trading
//!
//! Provides REST API integration for:
//! - Querying isolated margin account positions
//! - Placing market orders for exit
//! - Authentication via HMAC SHA256 signatures
//!
//! # Authentication
//!
//! Binance uses API key + secret with HMAC SHA256 signatures.
//! All signed requests require:
//! - `X-MBX-APIKEY` header
//! - `signature` query parameter (HMAC SHA256 of query string)
//! - `timestamp` query parameter

use std::time::Duration;

use chrono::Utc;
use reqwest::Client;
use robson_domain::{Price, Quantity, Side};
use rust_decimal::Decimal;
use serde::Deserialize;
use serde_json::Value;
use thiserror::Error;
use tokio::time::timeout;

// =============================================================================
// Constants
// =============================================================================

/// Binance REST API base URL (Spot/Margin)
const BINANCE_API_URL: &str = "https://api.binance.com";

/// Binance USD-M futures REST API base URL.
const BINANCE_FUTURES_API_URL: &str = "https://fapi.binance.com";

/// Binance USD-M futures testnet REST API base URL.
const BINANCE_FUTURES_TESTNET_API_URL: &str = "https://testnet.binancefuture.com";

/// Request timeout in seconds
const REQUEST_TIMEOUT_SECS: u64 = 10;

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

    /// API returned error
    #[error("Binance API error: {code} - {msg}")]
    ApiError { code: i64, msg: String },

    /// Failed to parse response
    #[error("Failed to parse response: {0}")]
    ParseError(String),

    /// Request timed out
    #[error("Request timed out")]
    Timeout,

    /// Invalid parameter
    #[error("Invalid parameter: {0}")]
    InvalidParameter(String),

    /// Position not found
    #[error("Position not found for symbol: {0}")]
    PositionNotFound(String),
}

// =============================================================================
// Binance REST Client
// =============================================================================

/// Binance REST API client for isolated margin trading.
pub struct BinanceRestClient {
    /// HTTP client
    client: Client,
    /// API key
    api_key: String,
    /// API secret
    api_secret: String,
    /// Use testnet (for testing)
    testnet: bool,
}

impl BinanceRestClient {
    /// Create a new Binance REST client.
    ///
    /// # Arguments
    ///
    /// * `api_key` - Binance API key
    /// * `api_secret` - Binance API secret
    pub fn new(api_key: String, api_secret: String) -> Self {
        Self {
            client: Client::new(),
            api_key,
            api_secret,
            testnet: false,
        }
    }

    /// Create a client for testnet (for testing).
    pub fn testnet(api_key: String, api_secret: String) -> Self {
        Self {
            client: Client::new(),
            api_key,
            api_secret,
            testnet: true,
        }
    }

    /// Get the base URL for API requests.
    fn base_url(&self) -> &str {
        if self.testnet {
            "https://testnet.binance.vision"
        } else {
            BINANCE_API_URL
        }
    }

    /// Get the base URL for USD-M futures public API requests.
    fn futures_base_url(&self) -> &str {
        if self.testnet {
            BINANCE_FUTURES_TESTNET_API_URL
        } else {
            BINANCE_FUTURES_API_URL
        }
    }

    /// Build query string with signature for signed requests.
    ///
    /// Binance requires:
    /// 1. All parameters in query string
    /// 2. HMAC SHA256 signature of query string
    /// 3. signature and timestamp as query parameters
    fn build_signed_query(
        &self,
        mut params: Vec<(&str, String)>,
    ) -> Result<String, BinanceRestError> {
        // Add timestamp
        let timestamp = Utc::now().timestamp_millis().to_string();
        params.push(("timestamp", timestamp));

        // Sort parameters (required by Binance)
        params.sort_by(|a, b| a.0.cmp(b.0));

        // Build query string
        let query_string: String =
            params.iter().map(|(k, v)| format!("{}={}", k, v)).collect::<Vec<_>>().join("&");

        // Create signature
        use hmac::{Hmac, Mac};
        use sha2::Sha256;

        type HmacSha256 = Hmac<Sha256>;

        let mut mac = HmacSha256::new_from_slice(self.api_secret.as_bytes())
            .map_err(|e| BinanceRestError::SignatureError(format!("HMAC error: {}", e)))?;

        mac.update(query_string.as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes());

        // Add signature to query string
        Ok(format!("{}&signature={}", query_string, signature))
    }

    /// Send a GET request to a public endpoint.
    async fn get_public(
        &self,
        endpoint: &str,
        params: Vec<(&str, String)>,
    ) -> Result<String, BinanceRestError> {
        self.get_public_from_base(self.base_url(), endpoint, params).await
    }

    /// Send a GET request to a public endpoint on an explicit base URL.
    async fn get_public_from_base(
        &self,
        base_url: &str,
        endpoint: &str,
        params: Vec<(&str, String)>,
    ) -> Result<String, BinanceRestError> {
        let url = if params.is_empty() {
            format!("{}{}", base_url, endpoint)
        } else {
            let query =
                params.iter().map(|(k, v)| format!("{}={}", k, v)).collect::<Vec<_>>().join("&");
            format!("{}{}?{}", base_url, endpoint, query)
        };

        let response =
            timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS), self.client.get(&url).send())
                .await
                .map_err(|_| BinanceRestError::Timeout)?
                .map_err(|e| BinanceRestError::RequestFailed(e.to_string()))?;

        let status = response.status();
        let body =
            response.text().await.map_err(|e| BinanceRestError::ParseError(e.to_string()))?;

        if !status.is_success() {
            // Try to parse Binance error response
            if let Ok(err) = serde_json::from_str::<BinanceErrorResponse>(&body) {
                return Err(BinanceRestError::ApiError { code: err.code, msg: err.msg });
            }
            return Err(BinanceRestError::RequestFailed(format!("HTTP {}: {}", status, body)));
        }

        Ok(body)
    }

    /// Send a GET request to a signed endpoint.
    async fn get_signed(
        &self,
        endpoint: &str,
        params: Vec<(&str, String)>,
    ) -> Result<String, BinanceRestError> {
        let query = self.build_signed_query(params)?;
        let url = format!("{}{}?{}", self.base_url(), endpoint, query);

        let response = timeout(
            Duration::from_secs(REQUEST_TIMEOUT_SECS),
            self.client.get(&url).header("X-MBX-APIKEY", &self.api_key).send(),
        )
        .await
        .map_err(|_| BinanceRestError::Timeout)?
        .map_err(|e| BinanceRestError::RequestFailed(e.to_string()))?;

        let status = response.status();
        let body =
            response.text().await.map_err(|e| BinanceRestError::ParseError(e.to_string()))?;

        if !status.is_success() {
            // Try to parse Binance error response
            if let Ok(err) = serde_json::from_str::<BinanceErrorResponse>(&body) {
                return Err(BinanceRestError::ApiError { code: err.code, msg: err.msg });
            }
            return Err(BinanceRestError::RequestFailed(format!("HTTP {}: {}", status, body)));
        }

        Ok(body)
    }

    /// Send a POST request to a signed endpoint.
    async fn post_signed(
        &self,
        endpoint: &str,
        params: Vec<(&str, String)>,
    ) -> Result<String, BinanceRestError> {
        let query = self.build_signed_query(params)?;
        let url = format!("{}{}?{}", self.base_url(), endpoint, query);

        let response = timeout(
            Duration::from_secs(REQUEST_TIMEOUT_SECS),
            self.client.post(&url).header("X-MBX-APIKEY", &self.api_key).send(),
        )
        .await
        .map_err(|_| BinanceRestError::Timeout)?
        .map_err(|e| BinanceRestError::RequestFailed(e.to_string()))?;

        let status = response.status();
        let body =
            response.text().await.map_err(|e| BinanceRestError::ParseError(e.to_string()))?;

        if !status.is_success() {
            // Try to parse Binance error response
            if let Ok(err) = serde_json::from_str::<BinanceErrorResponse>(&body) {
                return Err(BinanceRestError::ApiError { code: err.code, msg: err.msg });
            }
            return Err(BinanceRestError::RequestFailed(format!("HTTP {}: {}", status, body)));
        }

        Ok(body)
    }

    /// Send a DELETE request to a signed endpoint.
    async fn delete_signed(
        &self,
        endpoint: &str,
        params: Vec<(&str, String)>,
    ) -> Result<String, BinanceRestError> {
        let query = self.build_signed_query(params)?;
        let url = format!("{}{}?{}", self.base_url(), endpoint, query);

        let response = timeout(
            Duration::from_secs(REQUEST_TIMEOUT_SECS),
            self.client.delete(&url).header("X-MBX-APIKEY", &self.api_key).send(),
        )
        .await
        .map_err(|_| BinanceRestError::Timeout)?
        .map_err(|e| BinanceRestError::RequestFailed(e.to_string()))?;

        let status = response.status();
        let body =
            response.text().await.map_err(|e| BinanceRestError::ParseError(e.to_string()))?;

        if !status.is_success() {
            if let Ok(err) = serde_json::from_str::<BinanceErrorResponse>(&body) {
                return Err(BinanceRestError::ApiError { code: err.code, msg: err.msg });
            }
            return Err(BinanceRestError::RequestFailed(format!("HTTP {}: {}", status, body)));
        }

        Ok(body)
    }

    /// Get isolated margin account information for a symbol.
    ///
    /// Returns details about open positions, assets, and liabilities.
    ///
    /// # Endpoint
    ///
    /// `GET /sapi/v1/margin/isolated/account`
    ///
    /// # Example
    ///
    /// ```ignore
    /// let account = client.get_isolated_margin_account("BTCUSDT").await?;
    /// for asset in account.assets {
    ///     println!("{}: {}", asset.asset, asset.net_asset);
    /// }
    /// ```
    pub async fn get_isolated_margin_account(
        &self,
        symbol: &str,
    ) -> Result<IsolatedMarginAccount, BinanceRestError> {
        let params = vec![("symbols", symbol.to_string())];

        let body = self.get_signed("/sapi/v1/margin/isolated/account", params).await?;

        serde_json::from_str(&body).map_err(|e| BinanceRestError::ParseError(e.to_string()))
    }

    /// Get isolated margin account information for all symbols.
    pub async fn get_all_isolated_margin_accounts(
        &self,
    ) -> Result<Vec<IsolatedMarginAccount>, BinanceRestError> {
        let body = self.get_signed("/sapi/v1/margin/isolated/account", vec![]).await?;

        // Response is an array of accounts
        serde_json::from_str(&body).map_err(|e| BinanceRestError::ParseError(e.to_string()))
    }

    /// Get current open positions for a symbol.
    ///
    /// Returns positions that have non-zero quantity.
    pub async fn get_open_positions(
        &self,
        symbol: &str,
    ) -> Result<Vec<IsolatedMarginPosition>, BinanceRestError> {
        let account = self.get_isolated_margin_account(symbol).await?;

        let mut positions = Vec::new();

        for asset in account.assets {
            // Check if we have a long position (base asset)
            if asset.net_asset_value_btc != Decimal::ZERO {
                // Determine side based on borrowed amount
                let borrowed = asset.borrowed.clone().unwrap_or_default();
                let side = if borrowed > Decimal::ZERO {
                    Side::Short
                } else {
                    Side::Long
                };

                // Get current price from account
                let price = account
                    .total_asset_of_btc
                    .checked_div(account.total_liability_of_btc)
                    .unwrap_or_else(|| Decimal::ONE);

                positions.push(IsolatedMarginPosition {
                    symbol: symbol.to_string(),
                    side,
                    quantity: Quantity::new(asset.net_asset.abs()).map_err(|e| {
                        BinanceRestError::ParseError(format!("Invalid quantity: {}", e))
                    })?,
                    entry_price: Price::new(price).map_err(|e| {
                        BinanceRestError::ParseError(format!("Invalid price: {}", e))
                    })?,
                    // Use asset quote as asset name
                    asset: asset.asset.clone(),
                });
            }
        }

        Ok(positions)
    }

    // =========================================================================
    // Order API
    // =========================================================================

    /// Place a market order on isolated margin.
    ///
    /// # Arguments
    ///
    /// * `symbol` - Trading pair symbol (e.g., "BTCUSDT")
    /// * `side` - Order side (BUY or SELL)
    /// * `quantity` - Order quantity
    /// * `client_order_id` - Optional client order ID for idempotency
    ///
    /// # Endpoint
    ///
    /// `POST /sapi/v1/margin/order`
    ///
    /// # Example
    ///
    /// ```ignore
    /// let order = client.place_market_order("BTCUSDT", Side::Sell, dec!(0.1), None).await?;
    /// println!("Order ID: {}", order.order_id);
    /// ```
    pub async fn place_market_order(
        &self,
        symbol: &str,
        side: Side,
        quantity: Decimal,
        client_order_id: Option<&str>,
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
            ("isIsolated", "true".to_string()),
        ];

        if let Some(coid) = client_order_id {
            params.push(("newClientOrderId", coid.to_string()));
        }

        let body = self.post_signed("/sapi/v1/margin/order", params).await?;

        serde_json::from_str(&body).map_err(|e| BinanceRestError::ParseError(e.to_string()))
    }

    /// Cancel an open order.
    pub async fn cancel_order(
        &self,
        symbol: &str,
        order_id: u64,
    ) -> Result<BinanceOrderResponse, BinanceRestError> {
        let params = vec![
            ("symbol", symbol.to_string()),
            ("orderId", order_id.to_string()),
            ("isIsolated", "true".to_string()),
        ];

        let body = self.delete_signed("/sapi/v1/margin/order", params).await?;

        serde_json::from_str(&body).map_err(|e| BinanceRestError::ParseError(e.to_string()))
    }

    /// Get current price for a symbol.
    ///
    /// Uses public endpoint, no signature required.
    pub async fn get_price(&self, symbol: &str) -> Result<Price, BinanceRestError> {
        let params = vec![("symbol", symbol.to_string())];

        let body = self.get_public("/api/v3/ticker/price", params).await?;

        let response: PriceResponse =
            serde_json::from_str(&body).map_err(|e| BinanceRestError::ParseError(e.to_string()))?;

        Ok(Price::new(response.price).map_err(|e| {
            BinanceRestError::ParseError(format!("Invalid price in response: {}", e))
        })?)
    }

    /// Get USD-M futures klines for a symbol.
    ///
    /// Uses the public futures endpoint:
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

        let body = self
            .get_public_from_base(self.futures_base_url(), "/fapi/v1/klines", params)
            .await?;

        parse_futures_klines(&body)
    }

    /// Query order status.
    ///
    /// # Arguments
    ///
    /// * `symbol` - Trading pair (e.g., "BTCUSDT")
    /// * `order_id` - Exchange order ID
    pub async fn get_order_status(
        &self,
        symbol: &str,
        order_id: u64,
    ) -> Result<BinanceOrderResponse, BinanceRestError> {
        let params = vec![
            ("symbol", symbol.to_string()),
            ("orderId", order_id.to_string()),
        ];

        let body = self.get_signed("/sapi/v1/margin/order", params).await?;

        serde_json::from_str(&body).map_err(|e| BinanceRestError::ParseError(e.to_string()))
    }

    /// Ping Binance API to check connectivity.
    ///
    /// Uses public endpoint, no authentication required.
    /// Returns Ok(()) if API is reachable, Err otherwise.
    pub async fn ping(&self) -> Result<(), BinanceRestError> {
        let body = self.get_public("/api/v3/ping", vec![]).await?;

        // Ping returns empty JSON object {}
        if body.trim() == "{}" {
            Ok(())
        } else {
            Err(BinanceRestError::ParseError(format!("Unexpected ping response: {}", body)))
        }
    }
}

// =============================================================================
// Binance Types (from API responses)
// =============================================================================

/// Binance error response.
#[derive(Debug, Deserialize)]
struct BinanceErrorResponse {
    code: i64,
    msg: String,
}

/// Isolated margin account information.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IsolatedMarginAccount {
    /// Symbol for this account
    pub symbol: String,
    /// Assets in this isolated margin account
    pub assets: Vec<IsolatedMarginAsset>,
    /// Total asset of BTC (for calculating total value)
    pub total_asset_of_btc: Decimal,
    /// Total liability of BTC
    pub total_liability_of_btc: Decimal,
    /// Total asset of USDT
    pub total_asset_of_usdt: Decimal,
    /// Total liability of USDT
    pub total_liability_of_usdt: Decimal,
}

/// Asset in isolated margin account.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IsolatedMarginAsset {
    /// Asset symbol (e.g., "BTC", "USDT")
    pub asset: String,
    /// Borrowed amount
    pub borrowed: Option<Decimal>,
    /// Free amount
    pub free: Decimal,
    /// Locked amount
    pub locked: Decimal,
    /// Interest
    pub interest: Decimal,
    /// Net asset (free - borrowed - interest)
    pub net_asset: Decimal,
    /// Net asset value in BTC
    pub net_asset_value_btc: Decimal,
}

/// Position detected in isolated margin.
#[derive(Debug, Clone)]
pub struct IsolatedMarginPosition {
    /// Trading symbol (e.g., "BTCUSDT")
    pub symbol: String,
    /// Position side (Long or Short)
    pub side: Side,
    /// Position quantity
    pub quantity: Quantity,
    /// Approximate entry price
    pub entry_price: Price,
    /// Asset being held (e.g., "BTC" for long, "USDT" for short)
    pub asset: String,
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

/// Binance order response.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BinanceOrderResponse {
    /// Symbol
    pub symbol: String,
    /// Order ID
    pub order_id: u64,
    /// Client order ID
    pub client_order_id: String,
    /// Transaction time
    pub transact_time: i64,
    /// Price
    pub price: Decimal,
    /// Original quantity
    pub orig_qty: Decimal,
    /// Executed quantity
    pub executed_qty: Decimal,
    /// Cummulative quoted quantity
    pub cummulative_quote_qty: Decimal,
    /// Status
    pub status: String,
    /// Side
    pub side: String,
    /// Type
    #[serde(rename = "type")]
    pub order_type: String,
    /// Individual fills (present in margin market order responses)
    #[serde(default)]
    pub fills: Vec<BinanceFill>,
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

/// Price ticker response.
#[derive(Debug, Deserialize)]
struct PriceResponse {
    symbol: String,
    price: Decimal,
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

#[cfg(test)]
mod tests {
    use rust_decimal_macros::dec;

    use super::*;

    #[test]
    fn test_build_signed_query() {
        let client = BinanceRestClient::new("test_key".to_string(), "test_secret".to_string());

        let params = vec![("symbol", "BTCUSDT".to_string())];
        let query = client.build_signed_query(params).unwrap();

        // Query should contain timestamp and signature
        assert!(query.contains("timestamp="));
        assert!(query.contains("signature="));
        assert!(query.contains("symbol=BTCUSDT"));
    }

    #[test]
    fn test_build_signed_query_sorts_params() {
        let client = BinanceRestClient::new("test_key".to_string(), "test_secret".to_string());

        // Add params in reverse alphabetical order
        let params = vec![
            ("symbol", "BTCUSDT".to_string()),
            ("side", "SELL".to_string()),
        ];
        let query = client.build_signed_query(params).unwrap();

        // Params should be sorted (side comes before symbol)
        let side_idx = query.find("side=").unwrap();
        let symbol_idx = query.find("symbol=").unwrap();
        assert!(side_idx < symbol_idx);
    }

    #[test]
    fn test_isolated_margin_position_creation() {
        let position = IsolatedMarginPosition {
            symbol: "BTCUSDT".to_string(),
            side: Side::Long,
            quantity: Quantity::new(dec!(0.1)).unwrap(),
            entry_price: Price::new(dec!(95000)).unwrap(),
            asset: "BTC".to_string(),
        };

        assert_eq!(position.symbol, "BTCUSDT");
        assert_eq!(position.side, Side::Long);
        assert_eq!(position.quantity.as_decimal(), dec!(0.1));
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

    #[tokio::test]
    async fn test_get_price_requires_no_signature() {
        // This test just verifies the method compiles and has correct signature
        // Actual API call would require credentials
        let client = BinanceRestClient::new("key".to_string(), "secret".to_string());

        // Method should exist and return correct type
        let _ = client.get_price("BTCUSDT");
    }
}
