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
use chrono::{DateTime, Utc};
use robson_connectors::{BinanceRestClient, BinanceRestError};
use robson_domain::{OrderSide, Price, Quantity, Side, Symbol};
use robson_exec::{
    ports::{
        ExchangePosition, FuturesBalance, FuturesSettings, OpenOrderRecord, SpotBalance, SpotOrder,
        SpotOrderQuantity, SpotOrderRequest, SpotOrderSide, Transfer, TransferId,
        UniversalTransferType, UserTradeRecord,
    },
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
fn spot_order_from_response(
    response: robson_connectors::BinanceSpotOrderResponse,
) -> Result<SpotOrder, ExecError> {
    let fee: Decimal = response.fills.iter().map(|f| f.commission).sum();
    let fee_asset = response
        .fills
        .first()
        .map(|f| f.commission_asset.clone())
        .unwrap_or_else(|| "USDT".to_string());
    let millis = if response.transact_time > 0 {
        response.transact_time
    } else {
        response.update_time
    };
    let transact_time = DateTime::from_timestamp_millis(millis).unwrap_or_else(Utc::now);

    Ok(SpotOrder {
        symbol: response.symbol,
        exchange_order_id: response.order_id.to_string(),
        client_order_id: response.client_order_id,
        status: response.status,
        executed_qty: response.executed_qty,
        cummulative_quote_qty: response.cummulative_quote_qty,
        fee,
        fee_asset,
        transact_time,
    })
}

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
                -2010 | -2011 | -2013 | -2019 => ExecError::OrderRejected(msg.clone()),
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
            // Derive cumulative quote and fill price from the immediate RESULT response.
            // Binance Futures MARKET orders occasionally return avgPrice=0 / cumQuote=0
            // even on a FILLED status — re-query by orderId before treating it as an error.
            let (actual_cum_quote, fill_price) = if response.cum_quote > Decimal::ZERO {
                let price = response.cum_quote / executed_qty;
                (response.cum_quote, price)
            } else if response.avg_price > Decimal::ZERO {
                let cum_quote = response.avg_price * executed_qty;
                (cum_quote, response.avg_price)
            } else {
                tracing::warn!(
                    order_id = %response.order_id,
                    symbol = %symbol.as_pair(),
                    executed_qty = %executed_qty,
                    "MARKET order FILLED but price data absent in RESULT response — re-querying by orderId"
                );
                let settled = self
                    .client
                    .get_order_status(&symbol.as_pair(), response.order_id)
                    .await
                    .map_err(Self::map_error)?;

                if settled.cum_quote > Decimal::ZERO {
                    let price = settled.cum_quote / executed_qty;
                    (settled.cum_quote, price)
                } else if settled.avg_price > Decimal::ZERO {
                    let cum_quote = settled.avg_price * executed_qty;
                    (cum_quote, settled.avg_price)
                } else {
                    return Err(ExecError::Exchange(format!(
                        "Cannot determine fill price for order {} \
                         (no fills, cumQuote=0, avgPrice=0 after re-query)",
                        response.order_id
                    )));
                }
            };

            let estimated_fee = actual_cum_quote * Decimal::new(1, 3);

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

    async fn place_stop_market_order(
        &self,
        symbol: &Symbol,
        side: OrderSide,
        quantity: Quantity,
        stop_price: Price,
        client_order_id: &str,
    ) -> Result<OrderResult, ExecError> {
        let binance_side = match side {
            OrderSide::Buy => Side::Long,
            OrderSide::Sell => Side::Short,
        };

        let qty = normalize_market_quantity(symbol, quantity)?;

        let response = self
            .client
            .place_stop_market_order(
                &symbol.as_pair(),
                binance_side,
                qty,
                stop_price.as_decimal(),
                client_order_id,
            )
            .await
            .map_err(Self::map_error)?;

        // A protective STOP_MARKET is accepted, not filled: only `NEW` is the
        // clean success. Any other status (triggered, expired, rejected) is
        // surfaced so the executor records an `InsuranceStopFailed` audit
        // event and the software stop remains the primary exit path.
        match response.status.as_str() {
            "NEW" => {},
            "EXPIRED" | "CANCELED" | "REJECTED" => {
                return Err(ExecError::Exchange(format!(
                    "Insurance stop {} returned status '{}' (order_id={})",
                    client_order_id, response.status, response.order_id
                )));
            },
            other => {
                return Err(ExecError::Exchange(format!(
                    "Insurance stop {} returned unexpected status '{}' (order_id={})",
                    client_order_id, other, response.order_id
                )));
            },
        }

        let placed_at = chrono::DateTime::from_timestamp_millis(response.update_time)
            .unwrap_or_else(|| {
                tracing::warn!(
                    order_id = %response.order_id,
                    update_time = response.update_time,
                    "Invalid update_time from Binance — using local clock as fallback"
                );
                Utc::now()
            });

        // Accepted but unfilled: protective price recorded, no fill data.
        Ok(OrderResult {
            exchange_order_id: response.order_id.to_string(),
            client_order_id: response.client_order_id,
            fill_price: stop_price,
            filled_quantity: Quantity::zero(),
            fee: Decimal::ZERO,
            fee_asset: "USDT".to_string(),
            filled_at: placed_at,
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

    async fn get_open_orders(&self, symbol: &Symbol) -> Result<Vec<OpenOrderRecord>, ExecError> {
        let orders =
            self.client.get_open_orders(&symbol.as_pair()).await.map_err(Self::map_error)?;

        orders
            .into_iter()
            .map(|order| {
                let side = match order.side.as_str() {
                    "BUY" => OrderSide::Buy,
                    "SELL" => OrderSide::Sell,
                    other => {
                        return Err(ExecError::Exchange(format!(
                            "Open order {} has unexpected side '{}'",
                            order.order_id, other
                        )));
                    },
                };

                // A stop price of 0 means the order is not conditional.
                let stop_price =
                    if order.stop_price != Decimal::ZERO {
                        Some(Price::new(order.stop_price).map_err(|e| {
                            ExecError::Exchange(format!("Invalid stop price: {}", e))
                        })?)
                    } else {
                        None
                    };

                Ok(OpenOrderRecord {
                    exchange_order_id: order.order_id.to_string(),
                    client_order_id: order.client_order_id,
                    order_type: order.order_type,
                    reduce_only: order.reduce_only,
                    stop_price,
                    side,
                })
            })
            .collect()
    }

    async fn get_price(&self, symbol: &Symbol) -> Result<Price, ExecError> {
        self.client.get_price(&symbol.as_pair()).await.map_err(Self::map_error)
    }

    async fn health_check(&self) -> Result<(), ExecError> {
        self.client.ping().await.map_err(Self::map_error)
    }

    async fn get_futures_balance(&self) -> Result<FuturesBalance, ExecError> {
        let balance = self.client.get_futures_balance().await.map_err(Self::map_error)?;
        Ok(FuturesBalance {
            wallet_balance: balance.wallet_balance,
            available_balance: balance.available_balance,
        })
    }

    async fn get_spot_account_balances(&self) -> Result<Vec<SpotBalance>, ExecError> {
        let balances = self.client.get_spot_account_balances().await.map_err(Self::map_error)?;
        Ok(balances
            .into_iter()
            .map(|b| SpotBalance {
                asset: b.asset,
                free: b.free,
                locked: b.locked,
            })
            .collect())
    }

    async fn get_spot_price(&self, symbol: &str) -> Result<Price, ExecError> {
        self.client.get_spot_price(symbol).await.map_err(Self::map_error)
    }

    async fn spot_symbol_is_trading(&self, symbol: &str) -> Result<bool, ExecError> {
        self.client.spot_symbol_is_trading(symbol).await.map_err(Self::map_error)
    }

    async fn place_spot_market_order(
        &self,
        request: SpotOrderRequest,
    ) -> Result<SpotOrder, ExecError> {
        let side = match request.side {
            SpotOrderSide::Buy => "BUY",
            SpotOrderSide::Sell => "SELL",
        };
        let quantity_key = match request.quantity_kind {
            SpotOrderQuantity::Base => "quantity",
            SpotOrderQuantity::Quote => "quoteOrderQty",
        };

        let response = self
            .client
            .place_spot_market_order(
                &request.symbol,
                side,
                (quantity_key, request.quantity),
                &request.client_order_id,
            )
            .await
            .map_err(Self::map_error)?;

        spot_order_from_response(response)
    }

    async fn get_spot_order(
        &self,
        symbol: &str,
        client_order_id: &str,
    ) -> Result<Option<SpotOrder>, ExecError> {
        match self.client.get_spot_order(symbol, client_order_id).await {
            Ok(Some(response)) => spot_order_from_response(response).map(Some),
            Ok(None) => Ok(None),
            Err(error) => Err(Self::map_error(error)),
        }
    }

    async fn universal_transfer(
        &self,
        asset: &str,
        amount: Decimal,
        transfer_type: UniversalTransferType,
        client_tran_key: &str,
    ) -> Result<TransferId, ExecError> {
        let response = self
            .client
            .universal_transfer(asset, amount, transfer_type.as_binance_str(), client_tran_key)
            .await
            .map_err(Self::map_error)?;
        Ok(TransferId(response.tran_id.to_string()))
    }

    async fn get_transfer_history(
        &self,
        transfer_type: UniversalTransferType,
        start_time: DateTime<Utc>,
    ) -> Result<Vec<Transfer>, ExecError> {
        let transfers = self
            .client
            .get_transfer_history(transfer_type.as_binance_str(), start_time.timestamp_millis())
            .await
            .map_err(Self::map_error)?;

        transfers
            .into_iter()
            .map(|t| {
                let timestamp =
                    DateTime::from_timestamp_millis(t.timestamp).unwrap_or_else(Utc::now);
                Ok(Transfer {
                    transfer_id: TransferId(t.tran_id.to_string()),
                    client_tran_key: t.client_tran_id,
                    asset: t.asset,
                    amount: t.amount,
                    transfer_type,
                    status: t.status,
                    timestamp,
                })
            })
            .collect()
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

    async fn get_order_by_exchange_id(
        &self,
        symbol: &Symbol,
        order_id: &str,
    ) -> Result<Option<OrderResult>, ExecError> {
        let order_id_num: u64 = order_id.parse().map_err(|_| {
            ExecError::Exchange(format!("Invalid order_id for query: {}", order_id))
        })?;

        let response = match self.client.get_order_status(&symbol.as_pair(), order_id_num).await {
            Ok(response) => response,
            Err(BinanceRestError::ApiError { code: -2013, .. }) => return Ok(None),
            Err(error) => return Err(Self::map_error(error)),
        };

        // Only return fill data when the order is definitively filled.
        if response.status != "FILLED" {
            return Ok(None);
        }

        let executed_qty = response.executed_qty;
        if executed_qty <= Decimal::ZERO {
            return Ok(None);
        }

        if response.fills.is_empty() {
            // `/fapi/v1/order` usually omits fills for USD-M futures. Without
            // actual fill records, the fee would be an estimate; let callers
            // fall back to user trades for high-fidelity evidence instead.
            return Ok(None);
        }

        let total_quote: Decimal = response.fills.iter().map(|f| f.price * f.qty).sum();
        let total_qty: Decimal = response.fills.iter().map(|f| f.qty).sum();
        let fee: Decimal = response.fills.iter().map(|f| f.commission).sum();
        let fee_asset = response
            .fills
            .first()
            .map(|f| f.commission_asset.clone())
            .unwrap_or_else(|| "USDT".to_string());

        let fill_price = if total_qty > Decimal::ZERO {
            total_quote / total_qty
        } else {
            return Ok(None);
        };

        let filled_at =
            DateTime::from_timestamp_millis(response.update_time).unwrap_or_else(Utc::now);

        Ok(Some(OrderResult {
            exchange_order_id: response.order_id.to_string(),
            client_order_id: response.client_order_id,
            fill_price: Price::new(fill_price)
                .map_err(|e| ExecError::Exchange(format!("Invalid fill price: {}", e)))?,
            filled_quantity: Quantity::new(executed_qty)
                .map_err(|e| ExecError::Exchange(format!("Invalid filled quantity: {}", e)))?,
            fee,
            fee_asset,
            filled_at,
        }))
    }

    async fn get_user_trades_since(
        &self,
        symbol: &Symbol,
        since: DateTime<Utc>,
        limit: u16,
    ) -> Result<Vec<UserTradeRecord>, ExecError> {
        let start_time_ms = since.timestamp_millis();

        let trades = self
            .client
            .get_user_trades(&symbol.as_pair(), start_time_ms, limit)
            .await
            .map_err(Self::map_error)?;

        let mut records = trades
            .into_iter()
            .map(|t| {
                let filled_at = DateTime::from_timestamp_millis(t.time).unwrap_or_else(Utc::now);

                Ok(UserTradeRecord {
                    exchange_order_id: t.order_id.to_string(),
                    exchange_trade_id: t.id.to_string(),
                    fill_price: Price::new(t.price)
                        .map_err(|e| ExecError::Exchange(format!("Invalid trade price: {}", e)))?,
                    filled_quantity: Quantity::new(t.qty)
                        .map_err(|e| ExecError::Exchange(format!("Invalid trade qty: {}", e)))?,
                    fee: t.commission,
                    fee_asset: t.commission_asset,
                    filled_at,
                })
            })
            .collect::<Result<Vec<_>, ExecError>>()?;

        records.sort_by(|a, b| a.filled_at.cmp(&b.filled_at));

        Ok(records)
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
