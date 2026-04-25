//! Binance USD-M Futures adapter implementing ExchangePort.
//!
//! Wraps `BinanceRestClient` and translates between domain types
//! and Binance-specific API types.
//!
//! # Fill Extraction
//!
//! When processing order responses, fills are extracted with priority:
//! 1. `fills[]` array (most accurate) — VWAP price, actual commission
//! 2. Fallback to `executedQty + cummulativeQuoteQty` — average price,
//!    estimated fee

use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use robson_connectors::{BinanceRestClient, BinanceRestError};
use robson_domain::{OrderSide, Price, Quantity, Side, Symbol};
use robson_exec::{
    ports::{ExchangePosition, FuturesSettings},
    ExchangePort, ExecError, OrderResult,
};
use rust_decimal::Decimal;
use tracing::info;

/// Truncate a quantity to a given number of decimal places (round down).
fn trunc_to_scale(value: Decimal, scale: u32) -> Decimal {
    let factor = Decimal::from(10i64.pow(scale));
    (value * factor).floor() / factor
}

fn configured_step_size(symbol: &Symbol) -> Decimal {
    match symbol.as_pair().as_str() {
        "BTCUSDT" => Decimal::new(1, 3),
        _ => Decimal::new(1, 3),
    }
}

fn configured_step_scale(symbol: &Symbol) -> u32 {
    configured_step_size(symbol).scale()
}

pub(crate) fn normalize_market_quantity(
    symbol: &Symbol,
    quantity: Quantity,
) -> Result<Decimal, ExecError> {
    let raw_qty = quantity.as_decimal();
    let step_size = configured_step_size(symbol);
    let qty = trunc_to_scale(raw_qty, configured_step_scale(symbol));

    if qty < step_size {
        return Err(ExecError::OrderRejected(format!(
            "Quantity {} for {} rounds down to {} at Binance step size {} and falls below the minimum quantity {}. Increase capital or widen the stop distance; Robson will not round up.",
            raw_qty,
            symbol.as_pair(),
            qty,
            step_size,
            step_size
        )));
    }

    Ok(qty)
}

// =============================================================================
// Adapter
// =============================================================================

/// Binance USD-M Futures adapter implementing ExchangePort.
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
    fn map_error(error: BinanceRestError) -> ExecError {
        match &error {
            BinanceRestError::ApiError { code, msg } => match code {
                -2010 | -2011 | -2013 => ExecError::OrderRejected(msg.clone()),
                _ => ExecError::Exchange(error.to_string()),
            },
            BinanceRestError::Timeout => {
                ExecError::Timeout("Binance request timed out".to_string())
            },
            BinanceRestError::InvalidParameter(msg) => ExecError::OrderRejected(msg.clone()),
            _ => ExecError::Exchange(error.to_string()),
        }
    }
}

#[async_trait]
impl ExchangePort for BinanceExchangeAdapter {
    async fn validate_futures_settings(
        &self,
        symbol: &Symbol,
        expected_leverage: u8,
    ) -> Result<FuturesSettings, ExecError> {
        // 1. Check position mode via API
        let dual_side: bool =
            self.client.get_position_mode().await.map_err(|e| {
                ExecError::Exchange(format!("Failed to check position mode: {}", e))
            })?;

        if dual_side {
            return Err(ExecError::FuturesSafetyViolation {
                expected: "One-way position mode".to_string(),
                actual: "Hedge mode".to_string(),
                advice: "Switch to One-way position mode before trading".to_string(),
            });
        }

        // 2. Set leverage on the symbol — idempotent, safe to call on every check.
        self.client
            .set_leverage(&symbol.as_pair(), expected_leverage)
            .await
            .map_err(Self::map_error)?;

        info!(
            symbol = %symbol.as_pair(),
            leverage = expected_leverage,
            position_mode = "One-way",
            "Futures settings verified: One-way mode confirmed, leverage set"
        );

        Ok(FuturesSettings {
            position_mode: "One-way".to_string(),
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
        reduce_only: bool,
    ) -> Result<OrderResult, ExecError> {
        let binance_side = match side {
            OrderSide::Buy => Side::Long,
            OrderSide::Sell => Side::Short,
        };

        // BTCUSDT futures currently uses hardcoded 0.001 step size / minimum
        // quantity. TODO: query exchangeInfo per symbol for dynamic filters.
        let qty = normalize_market_quantity(symbol, quantity)?;

        let response = self
            .client
            .place_market_order(
                &symbol.as_pair(),
                binance_side,
                qty,
                Some(client_order_id),
                reduce_only,
            )
            .await
            .map_err(Self::map_error)?;

        match response.status.as_str() {
            "FILLED" => {},
            "PARTIALLY_FILLED" | "EXPIRED" | "CANCELED" | "REJECTED" => {
                return Err(ExecError::Exchange(format!(
                    "Order {} returned status '{}' — not a clean fill (order_id={})",
                    client_order_id, response.status, response.order_id
                )));
            },
            _ => {
                return Err(ExecError::Exchange(format!(
                    "Order {} returned unexpected status '{}' (order_id={})",
                    client_order_id, response.status, response.order_id
                )));
            },
        }

        let executed_qty = response.executed_qty;
        if executed_qty <= Decimal::ZERO {
            return Err(ExecError::Exchange(format!(
                "Order {} reports FILLED but executed_qty={} — inconsistent response (order_id={})",
                client_order_id, executed_qty, response.order_id
            )));
        }

        let (fill_price, fee, fee_asset) = if !response.fills.is_empty() {
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
                if response.cum_quote > Decimal::ZERO {
                    response.cum_quote / executed_qty
                } else {
                    tracing::warn!(
                        order_id = %response.order_id,
                        executed_qty = %executed_qty,
                        "FILLS present but no usable price source"
                    );
                    return Err(ExecError::Exchange(format!(
                        "Cannot determine fill price for order {} (fills empty, cumQuote=0)",
                        response.order_id
                    )));
                }
            };

            (vwap_price, total_fee, fee_asset)
        } else {
            let fill_price = if response.cum_quote > Decimal::ZERO {
                response.cum_quote / executed_qty
            } else if response.avg_price > Decimal::ZERO {
                response.avg_price
            } else {
                return Err(ExecError::Exchange(format!(
                    "Cannot determine fill price for order {} (no fills, cumQuote=0, avgPrice=0)",
                    response.order_id
                )));
            };

            let estimated_fee = response.cum_quote * Decimal::new(1, 3);

            (fill_price, estimated_fee, "USDT".to_string())
        };

        let filled_at = chrono::DateTime::from_timestamp_millis(response.update_time)
            .unwrap_or_else(|| {
                tracing::warn!(
                    order_id = %response.order_id,
                    update_time = response.update_time,
                    "Invalid update_time from Binance — using local clock as fallback"
                );
                Utc::now()
            });

        Ok(OrderResult {
            exchange_order_id: response.order_id.to_string(),
            client_order_id: response.client_order_id,
            fill_price: Price::new(fill_price)
                .map_err(|e| ExecError::Exchange(format!("Invalid fill price: {}", e)))?,
            filled_quantity: Quantity::new(executed_qty)
                .map_err(|e| ExecError::Exchange(format!("Invalid filled quantity: {}", e)))?,
            fee,
            fee_asset,
            filled_at,
        })
    }

    async fn cancel_order(&self, symbol: &Symbol, order_id: &str) -> Result<(), ExecError> {
        let order_id_num: u64 = order_id.parse().map_err(|_| {
            ExecError::Exchange(format!("Invalid order_id for cancel: {}", order_id))
        })?;

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

    async fn get_all_open_positions(&self) -> Result<Vec<ExchangePosition>, ExecError> {
        let positions = self.client.get_all_open_positions().await.map_err(Self::map_error)?;

        positions
            .into_iter()
            .map(|position| {
                let symbol = Symbol::from_pair(&position.symbol).map_err(|e| {
                    ExecError::Exchange(format!(
                        "Invalid symbol '{}' returned by exchange: {}",
                        position.symbol, e
                    ))
                })?;

                Ok(ExchangePosition {
                    symbol,
                    side: position.side,
                    quantity: position.quantity,
                    entry_price: position.entry_price,
                })
            })
            .collect()
    }

    async fn close_position_market(
        &self,
        symbol: &Symbol,
        side: Side,
        quantity: Quantity,
        client_order_id: &str,
    ) -> Result<OrderResult, ExecError> {
        let close_side = match side {
            Side::Long => OrderSide::Sell,
            Side::Short => OrderSide::Buy,
        };

        self.place_market_order(symbol, close_side, quantity, client_order_id, true)
            .await
    }
}

#[cfg(test)]
mod tests {
    use rust_decimal_macros::dec;

    use super::*;

    #[test]
    fn normalize_market_quantity_rejects_below_btc_step_size() {
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let quantity = Quantity::new(dec!(0.0004211005040573033565921178)).unwrap();

        let err = normalize_market_quantity(&symbol, quantity).unwrap_err();

        match err {
            ExecError::OrderRejected(message) => {
                assert!(message.contains("BTCUSDT"), "message: {message}");
                assert!(message.contains("step size 0.001"), "message: {message}");
                assert!(message.contains("minimum quantity 0.001"), "message: {message}");
                assert!(message.contains("will not round up"), "message: {message}");
            },
            other => panic!("expected OrderRejected, got {other:?}"),
        }
    }
}
