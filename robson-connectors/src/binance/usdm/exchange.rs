//! Binance USD-M Futures adapter implementing the venue ports (ADR-0056 §2)
//! as `binance:usdm`; the legacy `ExchangePort`/`IncomePort` reach it through
//! the forwarding layer in `robson-exec`.
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

use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use robson_domain::{OrderSide, Price, Quantity, Side, Symbol, SymbolTradingRules};
use robson_exec::{
    ports::{
        ExchangePosition, FuturesBalance, FuturesSettings, IncomeRecord, IncomeType,
        OpenOrderRecord, SpotBalance, SpotOrder, SpotOrderQuantity, SpotOrderRequest,
        SpotOrderSide, Transfer, TransferId, UniversalTransferType, UserTradeRecord,
    },
    venue::{
        self, AccountKind, AccountPort, Cash, EntryOrReduce, ExecutionCostEnvelope, FundingPort,
        InventoryPort, Mark, MarketKind, OrderLookup, PlacedStop, PositionAccountPort,
        ProtectionKind, ReplaceOutcome, ReplacementProtocol, SettlementPort, StopCancelOutcome,
        StopParams, StopRef, SubmitAttemptLedger, TradingPort, Venue, VenueCapabilities,
        VenueFinalityRule, VenueId,
    },
    ExecError, OrderResult,
};
use rust_decimal::Decimal;
use tracing::{info, warn};

use crate::{
    binance::wire,
    binance_rest::{
        BinanceAlgoOrderDetail, BinanceFuturesBalance, BinanceOrderResponse, BinanceRestClient,
        BinanceRestError,
    },
};

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

/// FALLBACK price scale for when exchange metadata is unavailable (issue
/// #154: the primary path validates against `tickSize` from exchangeInfo;
/// precision-based rounding does NOT produce tick-aligned prices on symbols
/// where `tickSize != 10^-pricePrecision`, e.g. BTCUSDT futures with tick
/// 0.10 and precision 2).
fn configured_price_scale(symbol: &Symbol) -> u32 {
    match symbol.as_pair().as_str() {
        "BTCUSDT" => 2,
        _ => 2,
    }
}

/// Normalize a protective stop trigger price to the symbol's price precision,
/// rounding AWAY from the position — up for a Buy stop (closing a short),
/// down for a Sell stop (closing a long) — so the stop never lands tighter
/// than the derived executable level. The sub-precision widening (< one unit
/// of the last decimal) sits well inside the sizing gap allowance (ADR-0039).
///
/// Derived stop prices are not precision-aligned in general: the ADR-0041
/// buffer is a bps offset (2026-07-04 prod incident: 62811.05 + 10 bps =
/// 62873.86105 → Binance -1111 "Precision is over the maximum").
pub(crate) fn normalize_stop_trigger_price(
    symbol: &Symbol,
    side: OrderSide,
    stop_price: Price,
) -> Decimal {
    let scale = configured_price_scale(symbol);
    let factor = Decimal::from(10i64.pow(scale));
    let scaled = stop_price.as_decimal() * factor;
    let rounded = match side {
        OrderSide::Buy => scaled.ceil(),
        OrderSide::Sell => scaled.floor(),
    };
    rounded / factor
}

pub fn normalize_market_quantity(
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
    response: crate::binance_rest::BinanceSpotOrderResponse,
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

/// Fee-grade fill evidence from a `/fapi/v1/order` response: `None` when
/// nothing executed or the response carries no per-fill records (callers fall
/// back to user trades, Policy 11).
fn fill_evidence(response: BinanceOrderResponse) -> Result<Option<OrderResult>, ExecError> {
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

    let filled_at = DateTime::from_timestamp_millis(response.update_time).unwrap_or_else(Utc::now);

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

/// Map the connector balance onto the port type: `wallet_balance` unchanged,
/// `unrealized_pnl` separate (ADR-0056 §2.4).
fn futures_balance_from(balance: BinanceFuturesBalance) -> FuturesBalance {
    FuturesBalance {
        wallet_balance: balance.wallet_balance,
        available_balance: balance.available_balance,
        unrealized_pnl: balance.unrealized_pnl,
    }
}

/// Map a `/fapi/v1/order` response onto the order lifecycle. Unrecognized
/// statuses are `Unknown`, never guessed.
fn order_lookup_from_response(response: BinanceOrderResponse) -> Result<OrderLookup, ExecError> {
    let filled_qty = response.executed_qty;
    match response.status.as_str() {
        "NEW" => Ok(OrderLookup::Open { filled_qty }),
        "PARTIALLY_FILLED" => Ok(OrderLookup::PartiallyFilled { filled_qty }),
        "FILLED" | "CANCELED" | "EXPIRED" | "EXPIRED_IN_MATCH" | "REJECTED" => {
            let cancelled_qty = (response.orig_qty - filled_qty).max(Decimal::ZERO);
            Ok(OrderLookup::Terminal {
                filled_qty,
                cancelled_qty,
                fills: fill_evidence(response)?,
            })
        },
        _ => Ok(OrderLookup::Unknown),
    }
}

/// Order finality rule of `binance:usdm`: a not-found on both lookups is final
/// only when every attempt with the client id was authoritatively rejected
/// (`is_authoritative_rejection`). Binance documents no bound on when an
/// accepted order becomes visible to Query Order or Query Algo Order, and
/// `recvWindow` only bounds request acceptance, so time proves nothing.
pub const USDM_FINALITY: VenueFinalityRule = VenueFinalityRule::EveryAttemptRejected;

/// Bound on the in-memory submit-attempt evidence.
const SUBMIT_ATTEMPTS_CAPACITY: usize = 4096;

/// Whether a failed placement proves Binance refused the order: HTTP 400 with
/// an allow-listed code. Anything else is indeterminate, and `Unknown` is
/// always the safe answer.
///
/// The codes, from the USDⓈ-M error code table
/// (<https://developers.binance.com/docs/derivatives/usds-margined-futures/error-code>):
/// `-2010` `NEW_ORDER_REJECTED`; `-2019` `MARGIN_NOT_SUFFICIEN` "Margin is
/// insufficient."; `-2021` `ORDER_WOULD_IMMEDIATELY_TRIGGER` "Order would
/// immediately trigger."; `-2022` `REDUCE_ONLY_REJECT` "ReduceOnly Order is
/// rejected.". The table does not say which codes guarantee non-acceptance;
/// these are the ones whose documented meaning is a refusal of the new order.
/// Deliberately out: `-1000`, `-1001`, `-1006`, `-1007` (execution status
/// unknown or unspecified), rate limits (`-1003`, `-1008`, `-1015`), and every
/// filter or validation code.
///
/// The status is checked because `ApiError` covers any non-success status with
/// a JSON body. Per General Info
/// (<https://developers.binance.com/docs/derivatives/usds-margined-futures/general-info>)
/// 5XX is an internal error and a 503 "could have been a success"; 403, 408,
/// 418 and 429 are WAF, backend timeout and rate limits, not order rejections.
fn is_authoritative_rejection(error: &BinanceRestError) -> bool {
    matches!(error, BinanceRestError::ApiError {
        http_status: 400,
        code: -2010 | -2019 | -2021 | -2022,
        ..
    })
}

// Adapter
// =============================================================================

/// TTL for cached symbol trading rules (issue #154). Exchange filters change
/// rarely; six hours keeps the metadata fresh without any per-tick calls.
const TRADING_RULES_TTL: Duration = Duration::from_secs(6 * 60 * 60);

/// Bound on the trading-rules cache (symbols actually traded; a runaway
/// symbol set is a defect, not a workload).
const TRADING_RULES_CACHE_CAPACITY: usize = 64;

struct CachedTradingRules {
    rules: SymbolTradingRules,
    fetched_at: Instant,
    /// Last cache hit; eviction is least-recently-used so an actively traded
    /// symbol is never displaced by a burst of one-off lookups.
    last_used_at: Instant,
}

/// Drop the least recently USED entry so a new symbol can be cached. Active
/// positions touch their symbol's rules on every tick, so they are never the
/// LRU candidate; only idle symbols are displaced.
fn evict_lru_trading_rules(cache: &mut HashMap<String, CachedTradingRules>) {
    let lru = cache
        .iter()
        .min_by_key(|(_, cached)| cached.last_used_at)
        .map(|(pair, _)| pair.clone());
    if let Some(pair) = lru {
        warn!(target: "robsond::binance_exchange", symbol = %pair, capacity = TRADING_RULES_CACHE_CAPACITY, "Trading-rules cache full; evicting least recently used entry");
        cache.remove(&pair);
    }
}

/// Serve the stale snapshot for `pair` after a failed refresh and mark it as
/// used now (keeping `fetched_at`, so the next lookup still tries to refresh).
/// An actively traded symbol living on its stale fallback must stay out of the
/// LRU eviction candidates exactly like one served fresh.
fn touch_stale_trading_rules(
    cache: &mut HashMap<String, CachedTradingRules>,
    pair: &str,
) -> Option<SymbolTradingRules> {
    let stale = cache.get_mut(pair)?;
    stale.last_used_at = Instant::now();
    Some(stale.rules.clone())
}

/// Binance USD-M Futures adapter: the `binance:usdm` venue.
///
/// Wraps `BinanceRestClient` and translates between domain types
/// and Binance-specific API types.
pub struct BinanceExchangeAdapter {
    venue_id: VenueId,
    client: Arc<BinanceRestClient>,
    /// Bounded TTL cache of per-symbol trading rules (issue #154). The
    /// tokio Mutex held across the refresh gives single-flight semantics:
    /// concurrent callers wait for one fetch instead of stampeding the
    /// exchange. Market ticks read the cached entry; zero HTTP per tick.
    trading_rules: tokio::sync::Mutex<HashMap<String, CachedTradingRules>>,
    /// Placement attempts by client id (finality evidence for
    /// `order_by_client_id`). In memory: see its restart limitation.
    submit_attempts: SubmitAttemptLedger,
}

impl BinanceExchangeAdapter {
    /// Create a new adapter wrapping a BinanceRestClient.
    pub fn new(client: Arc<BinanceRestClient>) -> Self {
        Self {
            venue_id: VenueId::new(VenueId::BINANCE_USDM),
            client,
            trading_rules: tokio::sync::Mutex::new(HashMap::new()),
            submit_attempts: SubmitAttemptLedger::new(SUBMIT_ATTEMPTS_CAPACITY),
        }
    }

    async fn fetch_trading_rules(&self, symbol: &Symbol) -> Result<SymbolTradingRules, ExecError> {
        let filters = self
            .client
            .get_futures_symbol_filters(&symbol.as_pair())
            .await
            .map_err(Self::map_error)?;
        SymbolTradingRules::new(
            symbol.clone(),
            filters.tick_size,
            filters.min_price,
            filters.step_size,
            filters.min_qty,
            filters.max_qty,
            filters.min_notional,
            filters.price_precision,
            filters.quantity_precision,
        )
        .map_err(|e| ExecError::Exchange(format!("Invalid exchangeInfo filters: {e}")))
    }

    /// Serve trading rules from the bounded TTL cache. On a refresh failure
    /// the STALE snapshot is served with a warning (policy for exchange-side
    /// filter changes: an existing order remains valid until a replacement
    /// exists, so stale metadata beats no metadata).
    async fn trading_rules_cached(&self, symbol: &Symbol) -> Result<SymbolTradingRules, ExecError> {
        let pair = symbol.as_pair();
        let mut cache = self.trading_rules.lock().await;
        if let Some(cached) = cache.get_mut(&pair) {
            if cached.fetched_at.elapsed() < TRADING_RULES_TTL {
                cached.last_used_at = Instant::now();
                return Ok(cached.rules.clone());
            }
        }
        match self.fetch_trading_rules(symbol).await {
            Ok(rules) => {
                if cache.len() >= TRADING_RULES_CACHE_CAPACITY && !cache.contains_key(&pair) {
                    // Bounded, never fail-closed: evicting the LRU entry
                    // costs one exchangeInfo call on that symbol's next use,
                    // whereas refusing would make a real arm on a new symbol
                    // fail once 64 distinct symbols have been seen.
                    evict_lru_trading_rules(&mut cache);
                }
                let now = Instant::now();
                cache.insert(pair, CachedTradingRules {
                    rules: rules.clone(),
                    fetched_at: now,
                    last_used_at: now,
                });
                Ok(rules)
            },
            Err(error) => match touch_stale_trading_rules(&mut cache, &pair) {
                Some(stale) => {
                    warn!(target: "robsond::binance_exchange",
                        symbol = %pair,
                        %error,
                        "Trading-rules refresh failed; serving stale snapshot"
                    );
                    Ok(stale)
                },
                None => Err(error),
            },
        }
    }

    /// Read-only lookup for the arm preview: a fresh cached entry is served
    /// from a momentary lock (no network under it); otherwise exchangeInfo is
    /// fetched WITHOUT touching the cache, so a preview can neither populate,
    /// refresh nor evict production metadata, nor hold the adapter mutex
    /// across HTTP while a close or panic waits for it.
    async fn trading_rules_peek_or_fetch(
        &self,
        symbol: &Symbol,
    ) -> Result<SymbolTradingRules, ExecError> {
        let pair = symbol.as_pair();
        let cached = {
            let cache = self.trading_rules.lock().await;
            cache
                .get(&pair)
                .filter(|cached| cached.fetched_at.elapsed() < TRADING_RULES_TTL)
                .map(|cached| cached.rules.clone())
        };
        match cached {
            Some(rules) => Ok(rules),
            None => self.fetch_trading_rules(symbol).await,
        }
    }

    /// Map BinanceRestError to ExecError.
    fn map_error(error: BinanceRestError) -> ExecError {
        match &error {
            BinanceRestError::ApiError { code, msg, .. } => match code {
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

    fn is_tolerated_algo_cancel_error(error: &BinanceRestError) -> bool {
        match error {
            BinanceRestError::ApiError { code, msg, .. } => {
                matches!(*code, -2011 | -2013)
                    || msg.to_ascii_lowercase().contains("unknown")
                    || msg.to_ascii_lowercase().contains("not exist")
                    || msg.to_ascii_lowercase().contains("already")
            },
            _ => false,
        }
    }

    /// Lifecycle of a protective stop found by client id. A triggered stop
    /// resolves to its resulting order; its fills come from the order
    /// response or, when absent there, from user trades.
    async fn algo_order_lookup(
        &self,
        symbol: &Symbol,
        detail: &BinanceAlgoOrderDetail,
    ) -> Result<OrderLookup, ExecError> {
        let actual_order_id = detail.actual_order_id.trim();
        if actual_order_id.is_empty() {
            return Ok(match detail.algo_status.as_str() {
                "NEW" => OrderLookup::Open { filled_qty: Decimal::ZERO },
                "CANCELED" | "EXPIRED" | "REJECTED" => OrderLookup::Terminal {
                    filled_qty: Decimal::ZERO,
                    cancelled_qty: detail.quantity,
                    fills: None,
                },
                // TRIGGERING, or TRIGGERED/FINISHED without the resulting
                // order id: not established.
                _ => OrderLookup::Unknown,
            });
        }
        let Ok(order_id) = actual_order_id.parse::<u64>() else {
            return Ok(OrderLookup::Unknown);
        };
        let response = match self.client.get_order_status(&symbol.as_pair(), order_id).await {
            Ok(response) => response,
            Err(BinanceRestError::ApiError { code: -2013, .. }) => return Ok(OrderLookup::Unknown),
            Err(error) => return Err(Self::map_error(error)),
        };
        match order_lookup_from_response(response)? {
            OrderLookup::Terminal { filled_qty, cancelled_qty, fills: None }
                if filled_qty > Decimal::ZERO =>
            {
                let fills = self.triggered_trade_fill(symbol, detail).await?;
                Ok(OrderLookup::Terminal { filled_qty, cancelled_qty, fills })
            },
            other => Ok(other),
        }
    }

    /// Fee-grade fill of a triggered protective stop from user trades keyed
    /// by the triggered order id (`/fapi/v1/order` omits per-fill records on
    /// USD-M futures).
    async fn triggered_trade_fill(
        &self,
        symbol: &Symbol,
        detail: &BinanceAlgoOrderDetail,
    ) -> Result<Option<OrderResult>, ExecError> {
        // /fapi/v1/order omits per-fill records on USD-M futures, so the
        // direct order query yields no fee-grade evidence for filled market
        // orders. Resolve through user trades keyed by the triggered order id
        // (UserTradeRecord-grade evidence, Policy 11). Found in production on
        // 2026-07-03: the first real insurance-stop fill left its position
        // unreconciled because this fallback was missing.
        let since = DateTime::from_timestamp_millis(if detail.trigger_time > 0 {
            detail.trigger_time - 5 * 60 * 1000
        } else {
            detail.create_time
        })
        .unwrap_or_else(|| Utc::now() - chrono::Duration::hours(24));

        let trades = self.user_trades_since(symbol, since, 1000).await?;
        let fills: Vec<_> = trades
            .into_iter()
            .filter(|t| t.exchange_order_id == detail.actual_order_id)
            .collect();
        if fills.is_empty() {
            return Ok(None);
        }

        let total_qty: Decimal = fills.iter().map(|t| t.filled_quantity.as_decimal()).sum();
        if total_qty <= Decimal::ZERO {
            return Ok(None);
        }
        let total_quote: Decimal = fills
            .iter()
            .map(|t| t.fill_price.as_decimal() * t.filled_quantity.as_decimal())
            .sum();
        let fee: Decimal = fills.iter().map(|t| t.fee).sum();
        let fee_asset =
            fills.first().map(|t| t.fee_asset.clone()).unwrap_or_else(|| "USDT".to_string());
        let filled_at = fills.iter().map(|t| t.filled_at).max().unwrap_or_else(Utc::now);

        Ok(Some(OrderResult {
            exchange_order_id: detail.actual_order_id.clone(),
            client_order_id: detail.client_algo_id.clone(),
            fill_price: Price::new(total_quote / total_qty)
                .map_err(|e| ExecError::Exchange(format!("Invalid fill price: {}", e)))?,
            filled_quantity: Quantity::new(total_qty)
                .map_err(|e| ExecError::Exchange(format!("Invalid fill quantity: {}", e)))?,
            fee,
            fee_asset,
            filled_at,
        }))
    }
}

#[async_trait]
impl PositionAccountPort for BinanceExchangeAdapter {
    async fn validate_position_settings(
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

        info!(target: "robsond::binance_exchange",
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

    async fn balance(&self) -> Result<FuturesBalance, ExecError> {
        let balance = self.client.get_futures_balance().await.map_err(Self::map_error)?;
        Ok(futures_balance_from(balance))
    }

    async fn open_positions(&self) -> Result<Vec<ExchangePosition>, ExecError> {
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

        TradingPort::place_market_order(
            self,
            symbol,
            close_side,
            quantity,
            client_order_id,
            EntryOrReduce::Reduce,
        )
        .await
    }
}

#[async_trait]
impl TradingPort for BinanceExchangeAdapter {
    async fn trading_rules(&self, symbol: &Symbol) -> Result<SymbolTradingRules, ExecError> {
        self.trading_rules_cached(symbol).await
    }

    async fn trading_rules_preview(
        &self,
        symbol: &Symbol,
    ) -> Result<SymbolTradingRules, ExecError> {
        self.trading_rules_peek_or_fetch(symbol).await
    }

    async fn place_market_order(
        &self,
        symbol: &Symbol,
        side: OrderSide,
        quantity: Quantity,
        client_order_id: &str,
        intent: EntryOrReduce,
    ) -> Result<OrderResult, ExecError> {
        // USDⓈ-M carries the reduce intent as the `reduceOnly` wire flag.
        let reduce_only = intent.is_reduce();
        let binance_side = match side {
            OrderSide::Buy => Side::Long,
            OrderSide::Sell => Side::Short,
        };

        // Lot filters from exchange metadata when available (issue #154);
        // the sizing already quantized down by stepSize, so this is a
        // validation in the expected case. Falls back to the configured
        // table when metadata is unavailable.
        let qty = match self.trading_rules_cached(symbol).await {
            Ok(rules) => {
                let qty = rules.quantize_qty_down(quantity.as_decimal());
                rules.validate_order_qty(qty).map_err(|e| {
                    ExecError::OrderRejected(format!(
                        "{e}. Increase capital or widen the stop distance; Robson will not \
                         round up."
                    ))
                })?;
                qty
            },
            Err(error) => {
                warn!(target: "robsond::binance_exchange",
                    symbol = %symbol.as_pair(),
                    %error,
                    "Trading rules unavailable; falling back to configured lot filters"
                );
                normalize_market_quantity(symbol, quantity)?
            },
        };

        let response = self
            .submit_attempts
            .submit(
                client_order_id,
                is_authoritative_rejection,
                self.client.place_market_order(
                    &symbol.as_pair(),
                    binance_side,
                    qty,
                    Some(client_order_id),
                    reduce_only,
                ),
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
                    tracing::warn!(target: "robsond::binance_exchange",
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
                tracing::warn!(target: "robsond::binance_exchange",
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
                tracing::warn!(target: "robsond::binance_exchange",
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

    async fn place_protection(
        &self,
        symbol: &Symbol,
        side: OrderSide,
        quantity: Quantity,
        stop_price: Price,
        client_order_id: &str,
    ) -> Result<PlacedStop, ExecError> {
        let binance_side = match side {
            OrderSide::Buy => Side::Long,
            OrderSide::Sell => Side::Short,
        };

        let qty = normalize_market_quantity(symbol, quantity)?;
        // Metadata-driven trigger validation (issue #154 deliverable 1): the
        // domain quantizes ExecutableSpan triggers on the tickSize grid, so the
        // expected case here is a pure pass-through validation. A misaligned
        // trigger can only come from a legacy-policy derivation (unquantized
        // by design); it is aligned adversely per tickSize and logged,
        // because a slightly wider stop beats an order the exchange may
        // reject under PRICE_FILTER (the empirical Binance leniency for
        // off-grid algo triggers is undocumented and must not be relied on).
        // Only when exchange metadata is unavailable does the legacy
        // precision-table fallback apply.
        let trigger_price = match self.trading_rules_cached(symbol).await {
            Ok(rules) => {
                if rules.is_tick_aligned(stop_price.as_decimal()) {
                    stop_price.as_decimal()
                } else {
                    let aligned = rules
                        .quantize_stop_trigger(side, stop_price)
                        .map_err(|e| ExecError::Exchange(format!("Invalid stop trigger: {e}")))?;
                    warn!(target: "robsond::binance_exchange",
                        symbol = %symbol.as_pair(),
                        requested = %stop_price.as_decimal(),
                        aligned = %aligned.as_decimal(),
                        tick_size = %rules.tick_size(),
                        "Stop trigger not tick-aligned (legacy derivation); aligned adversely"
                    );
                    aligned.as_decimal()
                }
            },
            Err(error) => {
                warn!(target: "robsond::binance_exchange",
                    symbol = %symbol.as_pair(),
                    %error,
                    "Trading rules unavailable; falling back to precision-based trigger rounding"
                );
                normalize_stop_trigger_price(symbol, side, stop_price)
            },
        };

        let response = self
            .submit_attempts
            .submit(
                client_order_id,
                is_authoritative_rejection,
                self.client.place_stop_market_order(
                    &symbol.as_pair(),
                    binance_side,
                    qty,
                    trigger_price,
                    client_order_id,
                ),
            )
            .await
            .map_err(Self::map_error)?;

        // A protective STOP_MARKET is accepted, not filled: only `NEW` is the
        // clean success. Any other status (triggered, expired, rejected) is
        // surfaced so the executor records an `InsuranceStopFailed` audit
        // event and the software stop remains the primary exit path.
        match response.algo_status.as_str() {
            "NEW" => {},
            "EXPIRED" | "CANCELED" | "REJECTED" => {
                return Err(ExecError::Exchange(format!(
                    "Insurance stop {} returned algoStatus '{}' (algo_id={})",
                    client_order_id, response.algo_status, response.algo_id
                )));
            },
            other => {
                return Err(ExecError::Exchange(format!(
                    "Insurance stop {} returned unexpected algoStatus '{}' (algo_id={})",
                    client_order_id, other, response.algo_id
                )));
            },
        }

        let placed_at = chrono::DateTime::from_timestamp_millis(response.update_time)
            .unwrap_or_else(|| {
                tracing::warn!(target: "robsond::binance_exchange",
                    algo_id = %response.algo_id,
                    update_time = response.update_time,
                    "Invalid update_time from Binance — using local clock as fallback"
                );
                Utc::now()
            });

        // Accepted but unfilled: protective price recorded, no fill data.
        Ok(PlacedStop {
            stop: StopRef::new(response.algo_id.to_string()),
            acceptance: OrderResult {
                exchange_order_id: response.algo_id.to_string(),
                client_order_id: response.client_algo_id,
                fill_price: stop_price,
                filled_quantity: Quantity::zero(),
                fee: Decimal::ZERO,
                fee_asset: "USDT".to_string(),
                filled_at: placed_at,
            },
        })
    }

    async fn cancel_protection(
        &self,
        _symbol: &Symbol,
        stop: &StopRef,
    ) -> Result<StopCancelOutcome, ExecError> {
        let algo_id = stop.exchange_order_id.as_str();
        let algo_id_num: i64 = algo_id.parse().map_err(|_| {
            ExecError::Exchange(format!("Invalid algo_id for insurance stop cancel: {}", algo_id))
        })?;

        match self.client.cancel_algo_order(algo_id_num).await {
            // An accepted algo cancel means the conditional never triggered,
            // so nothing executed.
            Ok(_) => Ok(StopCancelOutcome::Cancelled {
                filled_before_cancel: Decimal::ZERO,
                fills: None,
            }),
            Err(error) if Self::is_tolerated_algo_cancel_error(&error) => {
                warn!(target: "robsond::binance_exchange",
                    %algo_id,
                    %error,
                    "Insurance stop algo order already gone; most likely triggered"
                );
                // The final state is not looked up here (no extra request);
                // `protection_fill` resolves it.
                Ok(StopCancelOutcome::AlreadyGone { final_state: OrderLookup::Unknown })
            },
            Err(error) => Err(Self::map_error(error)),
        }
    }

    async fn replace_protection(
        &self,
        symbol: &Symbol,
        old: &StopRef,
        new: StopParams,
    ) -> Result<ReplaceOutcome, ExecError> {
        venue::replace_place_then_cancel(self, &self.submit_attempts, symbol, old, new).await
    }

    /// Lookup in both order namespaces this adapter places under: `GET
    /// /fapi/v1/order` with `origClientOrderId` (market orders), then `GET
    /// /fapi/v1/algoOrder` with `clientAlgoId` (protective stops). A
    /// triggered algo order resolves to its resulting order and fills (the
    /// same chain as `protection_fill`). Only a `-2013` on **both** lookups
    /// is a not-found, classified by [`USDM_FINALITY`] against the submit
    /// attempts this adapter recorded, as they stand after both responses
    /// returned (a retry started mid-lookup is seen); at most four requests.
    async fn order_by_client_id(
        &self,
        symbol: &Symbol,
        client_order_id: &str,
    ) -> Result<OrderLookup, ExecError> {
        let pair = symbol.as_pair();
        match self.client.get_order_status_by_client_id(&pair, client_order_id).await {
            Ok(response) => return order_lookup_from_response(response),
            Err(BinanceRestError::ApiError { code: -2013, .. }) => {},
            Err(error) => return Err(Self::map_error(error)),
        }
        match self.client.query_algo_order_by_client_id(client_order_id).await {
            Ok(detail) => self.algo_order_lookup(symbol, &detail).await,
            Err(BinanceRestError::ApiError { code: -2013, .. }) => {
                Ok(USDM_FINALITY.classify_not_found(self.submit_attempts.evidence(client_order_id)))
            },
            Err(error) => Err(Self::map_error(error)),
        }
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

    async fn open_protective_orders(
        &self,
        symbol: &Symbol,
    ) -> Result<Vec<OpenOrderRecord>, ExecError> {
        let orders = self
            .client
            .get_open_algo_orders(&symbol.as_pair())
            .await
            .map_err(Self::map_error)?;

        orders
            .into_iter()
            .map(|order| {
                let side = match order.side.as_str() {
                    "BUY" => OrderSide::Buy,
                    "SELL" => OrderSide::Sell,
                    other => {
                        return Err(ExecError::Exchange(format!(
                            "Open algo order {} has unexpected side '{}'",
                            order.algo_id, other
                        )));
                    },
                };

                // A stop price of 0 means the order is not conditional.
                let stop_price =
                    if order.trigger_price != Decimal::ZERO {
                        Some(Price::new(order.trigger_price).map_err(|e| {
                            ExecError::Exchange(format!("Invalid stop price: {}", e))
                        })?)
                    } else {
                        None
                    };

                Ok(OpenOrderRecord {
                    exchange_order_id: order.algo_id.to_string(),
                    client_order_id: order.client_algo_id,
                    order_type: order.order_type,
                    reduce_only: order.reduce_only,
                    stop_price,
                    side,
                })
            })
            .collect()
    }

    async fn price(&self, symbol: &Symbol) -> Result<Price, ExecError> {
        self.client.get_price(&symbol.as_pair()).await.map_err(Self::map_error)
    }

    async fn health(&self) -> Result<(), ExecError> {
        self.client.ping().await.map_err(Self::map_error)
    }

    async fn order_by_exchange_id(
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

        fill_evidence(response)
    }

    async fn protection_fill(
        &self,
        symbol: &Symbol,
        stop: &StopRef,
    ) -> Result<Option<OrderResult>, ExecError> {
        let algo_id = stop.exchange_order_id.as_str();
        let algo_id_num: i64 = algo_id.parse().map_err(|_| {
            ExecError::Exchange(format!("Invalid algo_id for insurance stop query: {}", algo_id))
        })?;

        let detail = match self.client.query_algo_order(algo_id_num).await {
            Ok(detail) => detail,
            Err(BinanceRestError::ApiError { code: -2013, .. }) => return Ok(None),
            Err(error) => return Err(Self::map_error(error)),
        };

        if detail.actual_order_id.trim().is_empty() {
            return Ok(None);
        }

        if let Some(result) = self.order_by_exchange_id(symbol, &detail.actual_order_id).await? {
            return Ok(Some(result));
        }

        self.triggered_trade_fill(symbol, &detail).await
    }

    async fn user_trades_since(
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

#[async_trait]
impl FundingPort for BinanceExchangeAdapter {
    async fn spot_balances(&self) -> Result<Vec<SpotBalance>, ExecError> {
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

    async fn spot_price(&self, symbol: &str) -> Result<Price, ExecError> {
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

        // Never resolves as rejected: the allow-list covers USDⓈ-M codes, and
        // `order_by_client_id` does not query the spot namespace.
        let response = self
            .submit_attempts
            .submit(
                &request.client_order_id,
                |_: &BinanceRestError| false,
                self.client.place_spot_market_order(
                    &request.symbol,
                    side,
                    (quantity_key, request.quantity),
                    &request.client_order_id,
                ),
            )
            .await
            .map_err(Self::map_error)?;

        spot_order_from_response(response)
    }

    async fn spot_order(
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

    async fn transfer(
        &self,
        asset: &str,
        amount: Decimal,
        transfer_type: UniversalTransferType,
        client_tran_key: &str,
    ) -> Result<TransferId, ExecError> {
        let response = self
            .client
            .universal_transfer(
                asset,
                amount,
                wire::universal_transfer_type(transfer_type),
                client_tran_key,
            )
            .await
            .map_err(Self::map_error)?;
        Ok(TransferId(response.tran_id.to_string()))
    }

    async fn transfer_history(
        &self,
        transfer_type: UniversalTransferType,
        start_time: DateTime<Utc>,
    ) -> Result<Vec<Transfer>, ExecError> {
        let transfers = self
            .client
            .get_transfer_history(
                wire::universal_transfer_type(transfer_type),
                start_time.timestamp_millis(),
            )
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
}

#[async_trait]
impl SettlementPort for BinanceExchangeAdapter {
    async fn settlement_since(
        &self,
        since: DateTime<Utc>,
        limit: u16,
    ) -> Result<Vec<IncomeRecord>, ExecError> {
        let start_time_ms = since.timestamp_millis();

        let items = self.client.get_income(start_time_ms, limit).await.map_err(Self::map_error)?;

        let records = items
            .into_iter()
            .map(|item| {
                let income_time =
                    DateTime::from_timestamp_millis(item.time).unwrap_or_else(Utc::now);
                let symbol = if item.symbol.is_empty() {
                    None
                } else {
                    Symbol::from_pair(&item.symbol).ok()
                };
                let exchange_trade_id = if item.trade_id.is_empty() {
                    None
                } else {
                    Some(item.trade_id)
                };

                IncomeRecord {
                    exchange_income_id: item.tran_id.to_string(),
                    symbol,
                    income_type: IncomeType::from_exchange_str(&item.income_type),
                    amount: item.income,
                    asset: item.asset,
                    exchange_trade_id,
                    income_time,
                }
            })
            .collect();

        Ok(records)
    }
}

#[async_trait]
impl AccountPort for BinanceExchangeAdapter {
    /// USDT only (the futures balance endpoint mapping extracts the USDT
    /// entry): `free` is `availableBalance`, `locked` the rest of the wallet
    /// balance.
    async fn cash(&self, asset: &str) -> Result<Cash, ExecError> {
        if asset != "USDT" {
            return Err(ExecError::Exchange(format!(
                "binance:usdm cash is only mapped for USDT, not {asset}"
            )));
        }
        let balance = self.client.get_futures_balance().await.map_err(Self::map_error)?;
        Ok(Cash {
            free: balance.available_balance,
            locked: (balance.wallet_balance - balance.available_balance).max(Decimal::ZERO),
            as_of: Utc::now(),
        })
    }

    /// Last price from the same ticker endpoint as `price`, stamped on
    /// receipt.
    async fn valuation(&self, symbol: &Symbol) -> Result<Mark, ExecError> {
        let price = self.client.get_price(&symbol.as_pair()).await.map_err(Self::map_error)?;
        Ok(Mark { price, as_of: Utc::now() })
    }

    /// The cost envelope source is undecided (ADR-0056 N5, required before
    /// step 8); admission still reads `RiskConfig`. Until then this fails
    /// closed rather than inventing numbers.
    async fn execution_costs(&self, symbol: &Symbol) -> Result<ExecutionCostEnvelope, ExecError> {
        Err(ExecError::Config(format!(
            "binance:usdm execution cost envelope for {} has no source yet (ADR-0056 N5)",
            symbol.as_pair()
        )))
    }

    fn capabilities(&self) -> VenueCapabilities {
        VenueCapabilities {
            market_kind: MarketKind::PerpUsdM,
            account_kind: AccountKind::PositionAccount,
            supports_short: true,
            reduce_only_wire_flag: true,
            protection: ProtectionKind::MarketOnTrigger,
            replacement: ReplacementProtocol::PlaceThenCancel,
            has_funding: true,
            order_finality: USDM_FINALITY,
        }
    }
}

impl Venue for BinanceExchangeAdapter {
    fn id(&self) -> &VenueId {
        &self.venue_id
    }

    fn position_account(&self) -> Option<&dyn PositionAccountPort> {
        Some(self)
    }

    fn inventory(&self) -> Option<&dyn InventoryPort> {
        None
    }

    fn settlement(&self) -> &dyn SettlementPort {
        self
    }

    fn funding(&self) -> Option<&dyn FundingPort> {
        Some(self)
    }
}

#[cfg(test)]
mod log_target_guard {
    /// The adapter was relocated from `robsond::binance_exchange` (ADR-0056
    /// step 1). Production `RUST_LOG` filters list `robsond=info` and do
    /// not include `robson_connectors`, so every log site keeps the legacy
    /// target explicitly.
    #[test]
    fn every_log_site_keeps_the_legacy_target() {
        let src = include_str!("exchange.rs");
        for (i, line) in src.lines().enumerate() {
            let is_site = ["warn!(", "info!(", "error!(", "debug!(", "trace!("].iter().any(|m| {
                line.trim_start().starts_with(m) || line.contains(&format!("tracing::{m}"))
            });
            if is_site && !line.contains("include_str!") {
                assert!(
                    line.contains("target: \"robsond::binance_exchange\""),
                    "log site without legacy target at exchange.rs:{}: {}",
                    i + 1,
                    line.trim()
                );
            }
        }
    }
}

#[cfg(test)]
mod submit_attempt_evidence {
    //! Placement evidence through the real adapter and REST client, against a
    //! local listener standing in for Binance.

    use std::sync::atomic::{AtomicUsize, Ordering};

    use robson_exec::venue::AttemptEvidence;
    use rust_decimal_macros::dec;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
        sync::Notify,
    };

    use super::*;

    type Reply = Option<(u16, String)>;

    const IN_FLIGHT: AttemptEvidence = AttemptEvidence::Unresolved { unresolved: 1, rejected: 0 };
    const FILLED: &str = r#"{"symbol":"BTCUSDT","orderId":7,"clientOrderId":"c1",
        "updateTime":1700000000000,"price":"0","origQty":"0.01","executedQty":"0.01",
        "cumQuote":"900","status":"FILLED","side":"BUY","type":"MARKET","fills":[]}"#;

    async fn bound_venue() -> (TcpListener, Arc<BinanceExchangeAdapter>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let client = BinanceRestClient::with_local_base_url(url);
        (listener, Arc::new(BinanceExchangeAdapter::new(Arc::new(client))))
    }

    /// Answer every request per `reply`, given `"METHOD /path"`; `None` reads
    /// the request and never answers.
    fn serve(listener: TcpListener, reply: impl Fn(&str) -> Reply + Send + Sync + 'static) {
        let reply = Arc::new(reply);
        tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let reply = reply.clone();
                tokio::spawn(async move {
                    let mut head = Vec::new();
                    let mut buf = [0u8; 4096];
                    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                        match stream.read(&mut buf).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => head.extend_from_slice(&buf[..n]),
                        }
                    }
                    let request = {
                        let head = String::from_utf8_lossy(&head);
                        let mut line = head.split_whitespace();
                        let method = line.next().unwrap_or_default();
                        let target = line.next().unwrap_or_default();
                        format!("{method} {}", target.split('?').next().unwrap_or_default())
                    };
                    match reply(&request) {
                        Some((status, body)) => {
                            let response = format!(
                                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\n\
                                 Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                                body.len()
                            );
                            let _ = stream.write_all(response.as_bytes()).await;
                        },
                        None => while matches!(stream.read(&mut buf).await, Ok(n) if n > 0) {},
                    }
                });
            }
        });
    }

    fn api_error(code: i64) -> String {
        format!(r#"{{"code":{code},"msg":"refused"}}"#)
    }

    /// Both lookups answer not-found; anything else (exchangeInfo) is a 404,
    /// so placements fall back to the configured lot filters.
    fn not_found(request: &str) -> Reply {
        match request {
            "GET /fapi/v1/order" | "GET /fapi/v1/algoOrder" => Some((400, api_error(-2013))),
            _ => Some((404, String::new())),
        }
    }

    #[derive(Debug, Clone, Copy)]
    enum Placement {
        Market,
        Protection,
        Spot,
    }

    impl Placement {
        fn route(self) -> &'static str {
            match self {
                Self::Market => "POST /fapi/v1/order",
                Self::Protection => "POST /fapi/v1/algoOrder",
                Self::Spot => "POST /api/v3/order",
            }
        }

        async fn place(self, venue: &BinanceExchangeAdapter, id: &str) -> Result<(), ExecError> {
            let symbol = Symbol::from_pair("BTCUSDT").unwrap();
            let qty = Quantity::new(dec!(0.01)).unwrap();
            match self {
                Self::Market => TradingPort::place_market_order(
                    venue,
                    &symbol,
                    OrderSide::Buy,
                    qty,
                    id,
                    EntryOrReduce::Entry,
                )
                .await
                .map(|_| ()),
                Self::Protection => {
                    let trigger = Price::new(dec!(90000)).unwrap();
                    venue
                        .place_protection(&symbol, OrderSide::Sell, qty, trigger, id)
                        .await
                        .map(|_| ())
                },
                Self::Spot => venue
                    .place_spot_market_order(SpotOrderRequest {
                        symbol: "BTCUSDT".to_string(),
                        side: SpotOrderSide::Buy,
                        quantity_kind: SpotOrderQuantity::Quote,
                        quantity: dec!(10),
                        client_order_id: id.to_string(),
                    })
                    .await
                    .map(|_| ()),
            }
        }
    }

    async fn lookup(venue: &BinanceExchangeAdapter, id: &str) -> OrderLookup {
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        venue.order_by_client_id(&symbol, id).await.unwrap()
    }

    async fn is_unknown(venue: &BinanceExchangeAdapter, id: &str) -> bool {
        matches!(lookup(venue, id).await, OrderLookup::Unknown)
    }

    /// A venue answering the first `Placement::route` request per `first` and
    /// later ones per `then`, with not-found lookups.
    async fn scripted_venue(
        placement: Placement,
        first: Reply,
        then: Reply,
    ) -> (Arc<BinanceExchangeAdapter>, Arc<AtomicUsize>) {
        let (listener, venue) = bound_venue().await;
        let posts = Arc::new(AtomicUsize::new(0));
        let counter = posts.clone();
        serve(listener, move |request| {
            if request != placement.route() {
                return not_found(request);
            }
            match counter.fetch_add(1, Ordering::SeqCst) {
                0 => first.clone(),
                _ => then.clone(),
            }
        });
        (venue, posts)
    }

    #[test]
    fn only_allow_listed_codes_on_http_400_are_rejections() {
        let api = |code, http_status| BinanceRestError::ApiError {
            code,
            msg: "x".to_string(),
            http_status,
        };
        for code in [-2010, -2019, -2021, -2022] {
            assert!(is_authoritative_rejection(&api(code, 400)), "{code}");
            for status in [200, 401, 403, 408, 418, 429, 500, 502, 503, 504] {
                assert!(!is_authoritative_rejection(&api(code, status)), "{code} on {status}");
            }
        }
        for code in [
            -1000, -1001, -1003, -1006, -1007, -1008, -1015, -1016, -1021, -1022, -1111, -2013,
            -2015, -2018, -2020, -2024, -2025, -4164,
        ] {
            assert!(!is_authoritative_rejection(&api(code, 400)), "{code}");
        }
        for error in [
            BinanceRestError::Timeout,
            BinanceRestError::RequestFailed("x".to_string()),
            BinanceRestError::ParseError("x".to_string()),
            BinanceRestError::InvalidParameter("x".to_string()),
            BinanceRestError::SignatureError("x".to_string()),
            BinanceRestError::PositionNotFound("x".to_string()),
        ] {
            assert!(!is_authoritative_rejection(&error), "{error}");
        }
    }

    #[tokio::test]
    async fn unanswered_placement_is_on_record_and_an_abort_keeps_blocking_absent() {
        for placement in [Placement::Market, Placement::Protection, Placement::Spot] {
            let arrived = Arc::new(Notify::new());
            let signal = arrived.clone();
            let (listener, venue) = bound_venue().await;
            serve(listener, move |request| {
                if request == placement.route() {
                    signal.notify_one();
                    return None;
                }
                not_found(request)
            });

            assert_eq!(venue.submit_attempts.evidence("c1"), AttemptEvidence::NoRecord);
            let task = tokio::spawn({
                let venue = venue.clone();
                async move { placement.place(&venue, "c1").await }
            });
            arrived.notified().await;
            assert_eq!(venue.submit_attempts.evidence("c1"), IN_FLIGHT, "{placement:?}");
            assert!(is_unknown(&venue, "c1").await, "{placement:?}");

            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
            assert_eq!(venue.submit_attempts.evidence("c1"), IN_FLIGHT, "{placement:?}");
            assert!(is_unknown(&venue, "c1").await, "{placement:?}");
        }
    }

    #[tokio::test]
    async fn rejected_retry_does_not_clear_an_aborted_attempt() {
        let arrived = Arc::new(Notify::new());
        let signal = arrived.clone();
        let posts = Arc::new(AtomicUsize::new(0));
        let counter = posts.clone();
        let (listener, venue) = bound_venue().await;
        serve(listener, move |request| {
            if request != Placement::Market.route() {
                return not_found(request);
            }
            if counter.fetch_add(1, Ordering::SeqCst) == 0 {
                signal.notify_one();
                return None;
            }
            Some((400, api_error(-2010)))
        });

        let task = tokio::spawn({
            let venue = venue.clone();
            async move { Placement::Market.place(&venue, "c1").await }
        });
        arrived.notified().await;
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());

        assert!(Placement::Market.place(&venue, "c1").await.is_err());
        assert_eq!(venue.submit_attempts.evidence("c1"), AttemptEvidence::Unresolved {
            unresolved: 1,
            rejected: 1
        });
        assert!(is_unknown(&venue, "c1").await);
        assert_eq!(posts.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn rejected_placement_is_absent_with_its_error_mapping_unchanged() {
        for (code, mapped_to_rejected) in
            [(-2010, true), (-2019, true), (-2021, false), (-2022, false)]
        {
            for placement in [Placement::Market, Placement::Protection] {
                let rejected = Some((400, api_error(code)));
                let (venue, posts) = scripted_venue(placement, rejected.clone(), rejected).await;

                let error = placement.place(&venue, "c1").await.unwrap_err();
                assert_eq!(
                    matches!(error, ExecError::OrderRejected(_)),
                    mapped_to_rejected,
                    "{code}: {error}"
                );
                assert_eq!(posts.load(Ordering::SeqCst), 1);
                assert_eq!(venue.submit_attempts.evidence("c1"), AttemptEvidence::AllRejected {
                    rejected: 1
                });
                assert!(matches!(
                    lookup(&venue, "c1").await,
                    OrderLookup::Absent { finality } if finality == USDM_FINALITY
                ));
            }
        }
    }

    #[tokio::test]
    async fn non_400_response_with_a_json_body_is_never_a_rejection() {
        let error = BinanceRestError::ApiError {
            code: -2010,
            msg: "x".to_string(),
            http_status: 503,
        };
        assert_eq!(error.to_string(), "Binance API error: -2010 - x");

        for status in [500, 502, 503, 504, 401, 403, 408, 418, 429] {
            let reply = Some((status, api_error(-2010)));
            let (venue, _) = scripted_venue(Placement::Market, reply.clone(), reply).await;

            // The mapping other callers see does not depend on the status.
            let error = Placement::Market.place(&venue, "c1").await.unwrap_err();
            assert!(matches!(error, ExecError::OrderRejected(_)), "{status}: {error}");
            assert_eq!(venue.submit_attempts.evidence("c1"), IN_FLIGHT, "{status}");
            assert!(is_unknown(&venue, "c1").await, "{status}");
        }
    }

    const STOP_ACCEPTED: &str = r#"{"algoId":998877,"clientAlgoId":"ins-2",
        "algoType":"CONDITIONAL","orderType":"STOP_MARKET","symbol":"BTCUSDT","side":"SELL",
        "quantity":"0.01","algoStatus":"NEW","triggerPrice":"91000","reduceOnly":true,
        "createTime":1,"updateTime":2}"#;
    const STOP_CANCELLED: &str = r#"{"algoId":77,"clientAlgoId":"ins-1","code":"200","msg":"OK"}"#;

    /// A venue scripting the successor's placements as [`scripted_venue`] does,
    /// counting the cancel requests that reach it.
    async fn replacement_venue(
        first: Reply,
        then: Reply,
    ) -> (Arc<BinanceExchangeAdapter>, Arc<AtomicUsize>) {
        let (listener, venue) = bound_venue().await;
        let posts = AtomicUsize::new(0);
        let cancels = Arc::new(AtomicUsize::new(0));
        let counter = cancels.clone();
        serve(listener, move |request| match request {
            "POST /fapi/v1/algoOrder" => match posts.fetch_add(1, Ordering::SeqCst) {
                0 => first.clone(),
                _ => then.clone(),
            },
            "DELETE /fapi/v1/algoOrder" => {
                counter.fetch_add(1, Ordering::SeqCst);
                Some((200, STOP_CANCELLED.to_string()))
            },
            _ => not_found(request),
        });
        (venue, cancels)
    }

    async fn replace_with(
        venue: &BinanceExchangeAdapter,
        quantity: Decimal,
        client_order_id: &str,
    ) -> ReplaceOutcome {
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let params = StopParams {
            close_side: OrderSide::Sell,
            quantity: Quantity::new(quantity).unwrap(),
            trigger: Price::new(dec!(91000)).unwrap(),
            client_order_id: client_order_id.to_string(),
        };
        venue.replace_protection(&symbol, &StopRef::new("77"), params).await.unwrap()
    }

    async fn replace(venue: &BinanceExchangeAdapter) -> ReplaceOutcome {
        replace_with(venue, dec!(0.01), "ins-2").await
    }

    #[tokio::test]
    async fn accepted_successor_cancels_the_old_stop() {
        // Control: the cancel counter of the tests below does count.
        let accepted = Some((200, STOP_ACCEPTED.to_string()));
        let (venue, cancels) = replacement_venue(accepted.clone(), accepted).await;

        let outcome = replace(&venue).await;
        assert!(matches!(outcome, ReplaceOutcome::Replaced { .. }), "{outcome:?}");
        assert_eq!(cancels.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn rejected_successor_keeps_the_old_stop() {
        // -2021 maps to `ExecError::Exchange`: the variant does not decide.
        for code in [-2010, -2019, -2021, -2022] {
            let rejected = Some((400, api_error(code)));
            let (venue, cancels) = replacement_venue(rejected.clone(), rejected).await;

            let outcome = replace(&venue).await;
            assert!(
                matches!(&outcome, ReplaceOutcome::OldKeptNewRejected { reason }
                    if reason.contains("refused")),
                "{code}: {outcome:?}"
            );
            assert_eq!(cancels.load(Ordering::SeqCst), 0, "{code}");
        }
    }

    #[tokio::test]
    async fn replacement_stays_unknown_without_proof_of_rejection() {
        for (case, reply) in [
            ("503 with a JSON rejection code", (503, api_error(-2010))),
            ("500 with a JSON rejection code", (500, api_error(-2019))),
            ("parse failure after a 2xx", (200, "not json".to_string())),
        ] {
            let reply = Some(reply);
            let (venue, cancels) = replacement_venue(reply.clone(), reply).await;

            let outcome = replace(&venue).await;
            assert!(
                matches!(outcome, ReplaceOutcome::Unknown { new: None }),
                "{case}: {outcome:?}"
            );
            assert_eq!(cancels.load(Ordering::SeqCst), 0, "{case}");
        }
    }

    #[tokio::test]
    async fn rejected_successor_retry_after_an_unresolved_attempt_stays_unknown() {
        let (venue, cancels) =
            replacement_venue(Some((503, api_error(-1007))), Some((400, api_error(-2010)))).await;

        let outcome = replace(&venue).await;
        assert!(matches!(outcome, ReplaceOutcome::Unknown { new: None }), "{outcome:?}");
        let outcome = replace(&venue).await;
        assert!(matches!(outcome, ReplaceOutcome::Unknown { new: None }), "{outcome:?}");
        assert_eq!(venue.submit_attempts.evidence("ins-2"), AttemptEvidence::Unresolved {
            unresolved: 1,
            rejected: 1
        });
        assert_eq!(cancels.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn rejected_successor_with_truncated_evidence_stays_unknown() {
        let rejected = Some((400, api_error(-2010)));
        let (venue, cancels) = replacement_venue(rejected.clone(), rejected).await;
        for i in 0..=SUBMIT_ATTEMPTS_CAPACITY {
            venue.submit_attempts.begin(&format!("fill-{i}"));
        }

        let outcome = replace(&venue).await;
        assert!(matches!(outcome, ReplaceOutcome::Unknown { new: None }), "{outcome:?}");
        assert_eq!(cancels.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn successor_refused_before_sending_stays_unknown() {
        // Below the lot step: `OrderRejected` with nothing sent, so no evidence.
        let accepted = Some((200, STOP_ACCEPTED.to_string()));
        let (venue, cancels) = replacement_venue(accepted.clone(), accepted).await;

        let outcome = replace_with(&venue, dec!(0.0001), "ins-2").await;
        assert!(matches!(outcome, ReplaceOutcome::Unknown { new: None }), "{outcome:?}");
        assert_eq!(venue.submit_attempts.evidence("ins-2"), AttemptEvidence::NoRecord);
        assert_eq!(cancels.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn parse_failure_after_2xx_stays_indeterminate() {
        for placement in [Placement::Market, Placement::Protection, Placement::Spot] {
            let reply = Some((200, "not json".to_string()));
            let (venue, _) = scripted_venue(placement, reply.clone(), reply).await;

            assert!(placement.place(&venue, "c1").await.is_err());
            assert_eq!(venue.submit_attempts.evidence("c1"), IN_FLIGHT, "{placement:?}");
            assert!(is_unknown(&venue, "c1").await, "{placement:?}");
        }
    }

    #[tokio::test]
    async fn spot_rejection_never_resolves_the_attempt() {
        let reply = Some((400, api_error(-2010)));
        let (venue, _) = scripted_venue(Placement::Spot, reply.clone(), reply).await;

        assert!(Placement::Spot.place(&venue, "c1").await.is_err());
        assert_eq!(venue.submit_attempts.evidence("c1"), IN_FLIGHT);
        assert!(is_unknown(&venue, "c1").await);
    }

    #[tokio::test]
    async fn accepted_attempt_that_lookups_cannot_see_stays_unknown() {
        // Accepted while both lookups answer not-found (a delayed record), and
        // after a rejected attempt with the same client id.
        let (venue, _) = scripted_venue(
            Placement::Market,
            Some((400, api_error(-2010))),
            Some((200, FILLED.to_string())),
        )
        .await;

        assert!(Placement::Market.place(&venue, "c1").await.is_err());
        assert!(matches!(lookup(&venue, "c1").await, OrderLookup::Absent { .. }));
        Placement::Market.place(&venue, "c1").await.unwrap();
        assert_eq!(venue.submit_attempts.evidence("c1"), AttemptEvidence::Unresolved {
            unresolved: 1,
            rejected: 1
        });
        assert!(is_unknown(&venue, "c1").await);
    }

    #[tokio::test]
    async fn not_found_without_evidence_is_unknown() {
        let (venue, _) = scripted_venue(Placement::Market, None, None).await;
        assert!(is_unknown(&venue, "never").await);
    }

    #[tokio::test]
    async fn retry_started_during_a_lookup_is_seen() {
        let (listener, venue) = bound_venue().await;
        let during_lookup = venue.clone();
        serve(listener, move |request| {
            if request == "GET /fapi/v1/algoOrder" {
                // After the first not-found, before the second one returns.
                during_lookup.submit_attempts.begin("c1");
            }
            not_found(request)
        });
        venue.submit_attempts.begin("c1").rejected();
        assert_eq!(venue.submit_attempts.evidence("c1"), AttemptEvidence::AllRejected {
            rejected: 1
        });

        assert!(is_unknown(&venue, "c1").await);
    }

    #[tokio::test]
    async fn truncated_evidence_keeps_a_rejected_retry_unknown() {
        let (venue, _) = scripted_venue(
            Placement::Market,
            Some((503, api_error(-1007))),
            Some((400, api_error(-2010))),
        )
        .await;
        assert!(Placement::Market.place(&venue, "c1").await.is_err());
        assert_eq!(venue.submit_attempts.evidence("c1"), IN_FLIGHT);

        for i in 0..SUBMIT_ATTEMPTS_CAPACITY {
            venue.submit_attempts.begin(&format!("fill-{i}"));
        }
        assert_eq!(venue.submit_attempts.evidence("c1"), AttemptEvidence::Truncated);

        // A fresh record for this retry would read `AllRejected`.
        assert!(Placement::Market.place(&venue, "c1").await.is_err());
        assert_eq!(venue.submit_attempts.evidence("c1"), AttemptEvidence::Truncated);
        assert!(is_unknown(&venue, "c1").await);
    }
}

#[cfg(test)]
mod tests {
    use robson_exec::venue::{
        capability_mismatches, AccountKind, MarketKind, ProtectionKind, ReplacementProtocol,
    };

    fn order(status: &str, executed: &str, fills: &str) -> BinanceOrderResponse {
        serde_json::from_str(&format!(
            r#"{{"symbol":"BTCUSDT","orderId":7,"clientOrderId":"c7","updateTime":1700000000000,
               "price":"0","origQty":"1","executedQty":"{executed}","status":"{status}",
               "side":"BUY","type":"MARKET","fills":[{fills}]}}"#
        ))
        .unwrap()
    }

    #[test]
    fn usdm_capabilities() {
        let venue = adapter();
        let caps = venue.capabilities();
        assert_eq!(venue.id().as_str(), "binance:usdm");
        assert_eq!(caps.market_kind, MarketKind::PerpUsdM);
        assert_eq!(caps.account_kind, AccountKind::PositionAccount);
        assert!(caps.supports_short && caps.reduce_only_wire_flag && caps.has_funding);
        assert_eq!(caps.protection, ProtectionKind::MarketOnTrigger);
        assert_eq!(caps.replacement, ReplacementProtocol::PlaceThenCancel);
        assert_eq!(caps.order_finality, VenueFinalityRule::EveryAttemptRejected);
        assert!(capability_mismatches(&venue).is_empty());
    }

    fn adapter() -> BinanceExchangeAdapter {
        BinanceExchangeAdapter::new(Arc::new(BinanceRestClient::new(
            "key".to_string(),
            "secret".to_string(),
        )))
    }

    #[tokio::test]
    async fn untriggered_algo_orders_map_without_further_requests() {
        let venue = adapter();
        let detail = |status: &str| -> BinanceAlgoOrderDetail {
            serde_json::from_str(&format!(
                r#"{{"algoId":1,"clientAlgoId":"ins-1","algoType":"CONDITIONAL",
                   "orderType":"STOP_MARKET","symbol":"BTCUSDT","side":"SELL",
                   "quantity":"0.004","algoStatus":"{status}","triggerPrice":"58888",
                   "reduceOnly":true,"createTime":1,"updateTime":2}}"#
            ))
            .unwrap()
        };
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let lookup = |status: &'static str| {
            let (venue, symbol, detail) = (&venue, &symbol, detail(status));
            async move { venue.algo_order_lookup(symbol, &detail).await.unwrap() }
        };
        assert!(
            matches!(lookup("NEW").await, OrderLookup::Open { filled_qty } if filled_qty.is_zero())
        );
        assert!(matches!(
            lookup("CANCELED").await,
            OrderLookup::Terminal { filled_qty, cancelled_qty, fills: None }
                if filled_qty.is_zero() && cancelled_qty == dec!(0.004)
        ));
        assert!(matches!(lookup("TRIGGERING").await, OrderLookup::Unknown));
        assert!(matches!(lookup("TRIGGERED").await, OrderLookup::Unknown));
    }

    #[test]
    fn balance_wire_response_maps_to_legacy_balance() {
        let body = r#"[{"asset":"BNB","balance":"1","availableBalance":"1","crossUnPnl":"0"},
                       {"asset":"USDT","balance":"1000.5","crossWalletBalance":"1000.5",
                        "crossUnPnl":"-12.25","availableBalance":"900"}]"#;
        let balance =
            futures_balance_from(crate::binance_rest::parse_futures_balance(body).unwrap());
        assert_eq!(balance.wallet_balance, dec!(1000.5));
        assert_eq!(balance.available_balance, dec!(900));
        assert_eq!(balance.unrealized_pnl, dec!(-12.25));
    }

    #[test]
    fn order_lookup_maps_binance_statuses() {
        let lookup = |status, executed, fills| {
            order_lookup_from_response(order(status, executed, fills)).unwrap()
        };
        assert!(
            matches!(lookup("NEW", "0", ""), OrderLookup::Open { filled_qty } if filled_qty.is_zero())
        );
        assert!(matches!(
            lookup("PARTIALLY_FILLED", "0.4", ""),
            OrderLookup::PartiallyFilled { filled_qty } if filled_qty == dec!(0.4)
        ));
        // No per-fill records on /fapi/v1/order: callers use user trades.
        assert!(matches!(
            lookup("CANCELED", "0.4", ""),
            OrderLookup::Terminal { filled_qty, cancelled_qty, fills: None }
                if filled_qty == dec!(0.4) && cancelled_qty == dec!(0.6)
        ));
        let fill = r#"{"price":"95000","qty":"1","commission":"0.5","commissionAsset":"USDT"}"#;
        assert!(matches!(
            lookup("FILLED", "1", fill),
            OrderLookup::Terminal { cancelled_qty, fills: Some(f), .. }
                if cancelled_qty.is_zero() && f.fee == dec!(0.5) && f.client_order_id == "c7"
        ));
        assert!(matches!(lookup("SOMETHING_NEW", "0", ""), OrderLookup::Unknown));
    }

    #[test]
    fn trading_rules_cache_evicts_the_least_recently_used_entry_when_full() {
        use std::collections::HashMap;

        use rust_decimal_macros::dec;

        let rules = |pair: &str| {
            robson_domain::SymbolTradingRules::new(
                robson_domain::Symbol::from_pair(pair).unwrap(),
                dec!(0.1),
                dec!(0.1),
                dec!(0.001),
                dec!(0.001),
                dec!(1000),
                dec!(5),
                1,
                3,
            )
            .unwrap()
        };
        // Fixtures live in the past so that "used now" is strictly newer.
        let base = std::time::Instant::now() - std::time::Duration::from_secs(100_000);
        let mut cache: HashMap<String, super::CachedTradingRules> = HashMap::new();
        for i in 0..super::TRADING_RULES_CACHE_CAPACITY {
            let pair = format!("S{i:02}USDT");
            let fetched_at = base + std::time::Duration::from_secs(i as u64);
            cache.insert(pair.clone(), super::CachedTradingRules {
                rules: rules(&pair),
                fetched_at,
                last_used_at: fetched_at,
            });
        }
        // The oldest FETCH is the symbol with an active position: it is hit
        // on every tick, so it is the most recently USED and must survive.
        cache.get_mut("S00USDT").unwrap().last_used_at = std::time::Instant::now();

        super::evict_lru_trading_rules(&mut cache);

        assert_eq!(cache.len(), super::TRADING_RULES_CACHE_CAPACITY - 1);
        assert!(cache.contains_key("S00USDT"), "the active symbol is never the LRU candidate");
        assert!(!cache.contains_key("S01USDT"), "the idle oldest-used entry is evicted");

        // An expired entry served as the stale fallback after a failed
        // refresh counts as used too: S02 is now the LRU candidate, S00 is
        // not, and S00's fetched_at is untouched so the next lookup retries.
        let before = cache.get("S02USDT").unwrap().fetched_at;
        let served = super::touch_stale_trading_rules(&mut cache, "S02USDT");
        assert!(served.is_some());
        assert_eq!(cache.get("S02USDT").unwrap().fetched_at, before);
        assert!(super::touch_stale_trading_rules(&mut cache, "NOPEUSDT").is_none());
        super::evict_lru_trading_rules(&mut cache);
        assert!(cache.contains_key("S02USDT"), "a stale fallback in use is not evicted");
        assert!(!cache.contains_key("S03USDT"));
    }

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

    #[test]
    fn stop_trigger_price_rounds_away_from_the_position() {
        // Regression: 2026-07-04 prod. The ADR-0041 buffered stop
        // 62811.05 + 10 bps = 62873.86105 exceeded BTCUSDT's 2-decimal price
        // precision and Binance rejected the insurance stop (-1111).
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let buffered = Price::new(dec!(62873.86105)).unwrap();

        // Short protection (Buy stop above): never round DOWN toward entry.
        assert_eq!(normalize_stop_trigger_price(&symbol, OrderSide::Buy, buffered), dec!(62873.87));
        // Long protection (Sell stop below): never round UP toward entry.
        assert_eq!(
            normalize_stop_trigger_price(&symbol, OrderSide::Sell, buffered),
            dec!(62873.86)
        );
    }

    #[test]
    fn stop_trigger_price_already_aligned_is_unchanged() {
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let aligned = Price::new(dec!(62811.05)).unwrap();

        assert_eq!(normalize_stop_trigger_price(&symbol, OrderSide::Buy, aligned), dec!(62811.05));
        assert_eq!(normalize_stop_trigger_price(&symbol, OrderSide::Sell, aligned), dec!(62811.05));
    }
}
