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
}

impl BinanceRestClient {
    /// Create a new Binance REST client for production.
    pub fn new(api_key: String, api_secret: String) -> Self {
        Self {
            client: Client::new(),
            api_key,
            api_secret,
            testnet: false,
        }
    }

    /// Create a client for the futures testnet.
    pub fn testnet(api_key: String, api_secret: String) -> Self {
        Self {
            client: Client::new(),
            api_key,
            api_secret,
            testnet: true,
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

    /// Send a GET request to a public endpoint.
    async fn get_public(
        &self,
        endpoint: &str,
        params: Vec<(&str, String)>,
    ) -> Result<String, BinanceRestError> {
        let url = if params.is_empty() {
            format!("{}{}", self.base_url(), endpoint)
        } else {
            let query =
                params.iter().map(|(k, v)| format!("{}={}", k, v)).collect::<Vec<_>>().join("&");
            format!("{}{}?{}", self.base_url(), endpoint, query)
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
    /// Returns `true` for Hedge mode (dualSidePosition=true), `false` for One-way mode.
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
                    unrealized_pnl: p.unrealized_profit,
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

/// Response from `GET /fapi/v2/positionRisk`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PositionRiskResponse {
    symbol: String,
    position_amt: Decimal,
    entry_price: Decimal,
    unrealized_profit: Decimal,
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

    #[tokio::test]
    async fn test_get_price_requires_no_signature() {
        let client = BinanceRestClient::new("key".to_string(), "secret".to_string());
        let _ = client.get_price("BTCUSDT");
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
}
