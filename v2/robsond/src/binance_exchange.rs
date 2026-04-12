//! Binance isolated margin adapter implementing ExchangePort.
//!
//! Wraps `BinanceRestClient` and translates between domain types
//! and Binance-specific API types.
//!
//! # Fill Extraction
//!
//! When processing order responses, fills are extracted with priority:
//! 1. `fills[]` array (most accurate) — VWAP price, actual commission
//! 2. Fallback to `executedQty + cummulativeQuoteQty` — average price, estimated fee

use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use rust_decimal::Decimal;
use tracing::info;

use robson_connectors::{BinanceRestClient, BinanceRestError};
use robson_domain::{OrderSide, Price, Quantity, Side, Symbol};
use robson_exec::{ExecError, ExchangePort, OrderResult};
use robson_exec::ports::MarginSettings;

// =============================================================================
// Adapter
// =============================================================================

/// Binance isolated margin adapter implementing ExchangePort.
///
/// Wraps `BinanceRestClient` and translates between domain types
/// and Binance-specific API types.
pub struct BinanceExchangeAdapter {
    client: Arc<BinanceRestClient>,
}

impl BinanceExchangeAdapter {
    /// Create a new adapter wrapping a BinanceRestClient.
    pub fn new(client: Arc<BinanceRestClient>) -> Self {
        Self { client }
    }

    /// Map BinanceRestError to ExecError.
    ///
    /// Known Binance error codes are mapped to specific variants.
    /// All other errors become generic Exchange errors.
    /// No auto-retry is performed — the caller decides.
    fn map_error(error: BinanceRestError) -> ExecError {
        match &error {
            BinanceRestError::ApiError { code, msg } => match code {
                -2010 | -2011 | -2013 => ExecError::OrderRejected(msg.clone()),
                _ => ExecError::Exchange(error.to_string()),
            },
            BinanceRestError::Timeout => ExecError::Timeout("Binance request timed out".to_string()),
            BinanceRestError::InvalidParameter(msg) => ExecError::OrderRejected(msg.clone()),
            _ => ExecError::Exchange(error.to_string()),
        }
    }
}

#[async_trait]
impl ExchangePort for BinanceExchangeAdapter {
    async fn validate_margin_settings(
        &self,
        symbol: &Symbol,
        expected_leverage: u8,
    ) -> Result<MarginSettings, ExecError> {
        // Verify the isolated margin account exists and is accessible.
        // Binance isolated margin leverage is implicit (collateral/borrow ratio),
        // not a per-symbol configurable setting like futures.
        let _account = self
            .client
            .get_isolated_margin_account(&symbol.as_pair())
            .await
            .map_err(Self::map_error)?;

        info!(symbol = %symbol.as_pair(), "Isolated margin account verified");

        Ok(MarginSettings {
            is_isolated: true,
            leverage: expected_leverage,
            symbol: symbol.as_pair(),
        })
    }

    async fn place_market_order(
        &self,
        symbol: &Symbol,
        side: OrderSide,
        quantity: Quantity,
        client_order_id: &str,
    ) -> Result<OrderResult, ExecError> {
        // Convert OrderSide (Buy/Sell) to Side (Long/Short) for BinanceRestClient.
        let binance_side = match side {
            OrderSide::Buy => Side::Long,
            OrderSide::Sell => Side::Short,
        };

        let response = self
            .client
            .place_market_order(
                &symbol.as_pair(),
                binance_side,
                quantity.as_decimal(),
                Some(client_order_id),
            )
            .await
            .map_err(Self::map_error)?;

        // Extract fill information.
        // Priority 1: fills[] (most accurate — VWAP price, actual commission)
        // Priority 2: fallback to executedQty + cummulativeQuoteQty
        let (fill_price, filled_quantity, fee, fee_asset) = if !response.fills.is_empty() {
            let total_quote: Decimal = response.fills.iter().map(|f| f.price * f.qty).sum();
            let total_qty: Decimal = response.fills.iter().map(|f| f.qty).sum();
            let total_fee: Decimal = response.fills.iter().map(|f| f.commission).sum();
            let fee_asset = response
                .fills
                .first()
                .map(|f| f.commission_asset.clone())
                .unwrap_or_else(|| "USDT".to_string());

            let vwap_price = if total_qty > Decimal::ZERO {
                total_quote / total_qty
            } else {
                response.price
            };

            (vwap_price, total_qty, total_fee, fee_asset)
        } else {
            // Fallback: derive price from executed quantities
            let fill_price = if response.executed_qty > Decimal::ZERO {
                response.cummulative_quote_qty / response.executed_qty
            } else {
                response.price
            };

            // No commission data available — estimate 0.1% (conservative upper bound)
            let estimated_fee = response.cummulative_quote_qty * Decimal::new(1, 3);

            (fill_price, response.executed_qty, estimated_fee, "USDT".to_string())
        };

        let filled_at = chrono::DateTime::from_timestamp_millis(response.transact_time)
            .unwrap_or_else(Utc::now);

        Ok(OrderResult {
            exchange_order_id: response.order_id.to_string(),
            client_order_id: response.client_order_id,
            fill_price: Price::new(fill_price)
                .map_err(|e| ExecError::Exchange(format!("Invalid fill price: {}", e)))?,
            filled_quantity: Quantity::new(filled_quantity)
                .map_err(|e| ExecError::Exchange(format!("Invalid filled quantity: {}", e)))?,
            fee,
            fee_asset,
            filled_at,
        })
    }

    async fn cancel_order(&self, symbol: &Symbol, order_id: &str) -> Result<(), ExecError> {
        let order_id_num: u64 = order_id
            .parse()
            .map_err(|_| ExecError::Exchange(format!("Invalid order_id for cancel: {}", order_id)))?;

        self.client
            .cancel_order(&symbol.as_pair(), order_id_num)
            .await
            .map_err(Self::map_error)?;

        Ok(())
    }

    async fn get_price(&self, symbol: &Symbol) -> Result<Price, ExecError> {
        self.client.get_price(&symbol.as_pair()).await.map_err(Self::map_error)
    }

    async fn health_check(&self) -> Result<(), ExecError> {
        self.client.ping().await.map_err(Self::map_error)
    }
}
