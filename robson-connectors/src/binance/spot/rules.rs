//! A spot symbol's `exchangeInfo` rules (ADR-0056 step 8a).
//!
//! Filter semantics are Binance's, `filters.md` at `995f6eb3`: PRICE_FILTER
//! L38, LOT_SIZE L129, NOTIONAL L184, MARKET_LOT_SIZE L229,
//! MAX_NUM_ALGO_ORDERS L269. "All symbol filters are optional" (`rest-api.md`
//! L986), so each is an `Option` here. Every value is runtime metadata, never
//! a constant (AGENTS.md invariant 9).
//!
//! [`SpotSymbolRules::trading_rules`] projects them onto the domain's
//! `SymbolTradingRules` (8c). It refuses a `minPrice`/`tickSize` pair whose
//! grid differs from Binance's: spot's price rule is `price % tickSize == 0`
//! (filters.md L52), while the domain anchors its grid at `min_price`, so
//! `minPrice` 0.015 with `tickSize` 0.01 would round a trigger onto a price
//! Binance refuses.

use robson_domain::{Symbol, SymbolTradingRules};
use rust_decimal::Decimal;
use serde::{de::DeserializeOwned, Deserialize};
use serde_json::Value;

use super::rest::{parse_json, SpotReadError};

/// PRICE_FILTER, for `price` and `stopPrice` alike. A zero in any field
/// disables that rule.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpotPriceFilter {
    pub min_price: Decimal,
    pub max_price: Decimal,
    pub tick_size: Decimal,
}

/// LOT_SIZE (every order) or MARKET_LOT_SIZE (MARKET orders).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpotLotFilter {
    pub min_qty: Decimal,
    pub max_qty: Decimal,
    pub step_size: Decimal,
}

/// NOTIONAL. A MARKET order's notional is priced at the reference price when
/// one exists, otherwise at the average over `avg_price_mins` (F7).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpotNotionalFilter {
    pub min_notional: Decimal,
    /// Whether `min_notional` binds MARKET orders.
    pub apply_min_to_market: bool,
    pub max_notional: Decimal,
    /// Whether `max_notional` binds MARKET orders.
    pub apply_max_to_market: bool,
    pub avg_price_mins: u32,
}

/// One symbol's entry of `GET /api/v3/exchangeInfo`, as published.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpotSymbolRules {
    pub symbol: String,
    /// `TRADING`, `END_OF_DAY`, `HALT`, `BREAK` or `CANCEL_ONLY` (enums.md
    /// L5-L11), kept raw.
    pub status: String,
    pub base_asset: String,
    pub quote_asset: String,
    pub base_asset_precision: u32,
    pub quote_asset_precision: u32,
    /// What the symbol accepts (F5): symbol availability, not account
    /// authorisation.
    pub order_types: Vec<String>,
    /// The account needs a permission from every set (`rest-api.md`
    /// L914-L916).
    pub permission_sets: Vec<Vec<String>>,
    pub price_filter: Option<SpotPriceFilter>,
    pub lot_size: Option<SpotLotFilter>,
    pub market_lot_size: Option<SpotLotFilter>,
    pub notional: Option<SpotNotionalFilter>,
    /// Open `STOP_LOSS`, `STOP_LOSS_LIMIT`, `TAKE_PROFIT` and
    /// `TAKE_PROFIT_LIMIT` orders allowed on the symbol (F8).
    pub max_num_algo_orders: Option<u32>,
    /// The self-trade prevention modes an order on the symbol may name
    /// (`allowedSelfTradePreventionModes`, `rest-api.md` L996; enums.md
    /// L172-L183), kept raw.
    pub allowed_stp_modes: Vec<String>,
}

impl SpotSymbolRules {
    /// Quantity bounds of a MARKET order, and of the MARKET order a triggered
    /// `STOP_LOSS` executes (F1): LOT_SIZE and MARKET_LOT_SIZE together.
    ///
    /// A zero `min_qty` or `step_size` adds no constraint. BTCUSDT's
    /// MARKET_LOT_SIZE has both at zero (F6), and Binance does not document
    /// what a zero there means (design note §16). Two non-zero steps must
    /// nest, and the coarser one applies. The maximum is the smaller one: no
    /// rule says a zero maximum disables it, so it is taken as written.
    pub fn market_lot(&self) -> Result<SpotLotFilter, SpotReadError> {
        let lot = self.lot_size.as_ref().ok_or_else(|| self.missing("LOT_SIZE"))?;
        let Some(market) = &self.market_lot_size else {
            return Ok(lot.clone());
        };
        let step_size = coarser_step(lot.step_size, market.step_size).ok_or_else(|| {
            SpotReadError::InvalidRules(format!(
                "{}: LOT_SIZE step {} and MARKET_LOT_SIZE step {} do not nest",
                self.symbol, lot.step_size, market.step_size
            ))
        })?;
        Ok(SpotLotFilter {
            min_qty: lot.min_qty.max(market.min_qty),
            max_qty: lot.max_qty.min(market.max_qty),
            step_size,
        })
    }

    /// The domain's trading rules for `symbol` (8c). Refused, as
    /// [`SpotReadError::InvalidRules`]: no PRICE_FILTER or a zero tick; a
    /// `minPrice` off the `tickSize` grid (filters.md L52); no LOT_SIZE, or
    /// steps that do not nest ([`Self::market_lot`]); no NOTIONAL; anything
    /// `SymbolTradingRules::new` refuses. Carried: the tick and `minPrice`,
    /// the market lot's step and bounds, `minNotional` whatever
    /// `applyMinToMarket` says, and the asset precisions as the domain's
    /// informational precisions. Not carried: `maxPrice`, `maxNotional`, the
    /// order counts and the symbol status.
    pub fn trading_rules(&self, symbol: &Symbol) -> Result<SymbolTradingRules, SpotReadError> {
        if self.symbol != symbol.as_pair()
            || self.base_asset != symbol.base()
            || self.quote_asset != symbol.quote()
        {
            return Err(SpotReadError::Inconsistent(format!(
                "rules of {} ({}/{}) asked for {}/{}",
                self.symbol,
                self.base_asset,
                self.quote_asset,
                symbol.base(),
                symbol.quote()
            )));
        }
        let price = self.price_filter.as_ref().ok_or_else(|| self.missing("PRICE_FILTER"))?;
        if price.tick_size <= Decimal::ZERO {
            return Err(SpotReadError::InvalidRules(format!(
                "{}: tick size {} gives no price grid",
                self.symbol, price.tick_size
            )));
        }
        if !(price.min_price % price.tick_size).is_zero() {
            return Err(SpotReadError::InvalidRules(format!(
                "{}: minPrice {} is off the tickSize {} grid",
                self.symbol, price.min_price, price.tick_size
            )));
        }
        let lot = self.market_lot()?;
        let notional = self.notional.as_ref().ok_or_else(|| self.missing("NOTIONAL"))?;
        SymbolTradingRules::new(
            symbol.clone(),
            price.tick_size,
            price.min_price,
            lot.step_size,
            lot.min_qty,
            lot.max_qty,
            notional.min_notional,
            self.quote_asset_precision,
            self.base_asset_precision,
        )
        .map_err(|e| SpotReadError::InvalidRules(format!("{}: {e}", self.symbol)))
    }

    fn missing(&self, filter: &str) -> SpotReadError {
        SpotReadError::InvalidRules(format!("{} has no {filter} filter", self.symbol))
    }
}

/// The step both grids allow: one step when the other is zero, the coarser
/// when it is a whole multiple of the finer, otherwise none.
fn coarser_step(a: Decimal, b: Decimal) -> Option<Decimal> {
    if a.is_zero() {
        return Some(b);
    }
    if b.is_zero() {
        return Some(a);
    }
    let (fine, coarse) = if a <= b { (a, b) } else { (b, a) };
    (coarse % fine).is_zero().then_some(coarse)
}

#[derive(Deserialize)]
struct ExchangeInfoWire {
    symbols: Vec<SymbolWire>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SymbolWire {
    symbol: String,
    status: String,
    base_asset: String,
    quote_asset: String,
    base_asset_precision: u32,
    quote_asset_precision: u32,
    order_types: Vec<String>,
    permission_sets: Vec<Vec<String>>,
    filters: Vec<Value>,
    allowed_self_trade_prevention_modes: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MaxNumAlgoOrdersWire {
    max_num_algo_orders: u32,
}

/// The rules of `pair` from an `exchangeInfo` body, which must list it once.
pub(super) fn parse_symbol_rules(body: &str, pair: &str) -> Result<SpotSymbolRules, SpotReadError> {
    let info: ExchangeInfoWire = parse_json("exchangeInfo", body)?;
    let mut entries = info.symbols.into_iter().filter(|entry| entry.symbol == pair);
    let entry = entries.next().ok_or_else(|| {
        SpotReadError::Inconsistent(format!("exchangeInfo has no entry for {pair}"))
    })?;
    if entries.next().is_some() {
        return Err(SpotReadError::Inconsistent(format!("exchangeInfo lists {pair} twice")));
    }

    let mut rules = SpotSymbolRules {
        symbol: entry.symbol,
        status: entry.status,
        base_asset: entry.base_asset,
        quote_asset: entry.quote_asset,
        base_asset_precision: entry.base_asset_precision,
        quote_asset_precision: entry.quote_asset_precision,
        order_types: entry.order_types,
        permission_sets: entry.permission_sets,
        price_filter: None,
        lot_size: None,
        market_lot_size: None,
        notional: None,
        max_num_algo_orders: None,
        allowed_stp_modes: entry.allowed_self_trade_prevention_modes,
    };
    for filter in entry.filters {
        let kind = filter.get("filterType").and_then(Value::as_str).map(str::to_owned);
        let Some(kind) = kind else {
            return Err(SpotReadError::Parse(format!("{pair}: a filter has no filterType")));
        };
        match kind.as_str() {
            "PRICE_FILTER" => {
                once(&mut rules.price_filter, decode(pair, &kind, filter)?, pair, &kind)?
            },
            "LOT_SIZE" => once(&mut rules.lot_size, decode(pair, &kind, filter)?, pair, &kind)?,
            "MARKET_LOT_SIZE" => {
                once(&mut rules.market_lot_size, decode(pair, &kind, filter)?, pair, &kind)?
            },
            "NOTIONAL" => once(&mut rules.notional, decode(pair, &kind, filter)?, pair, &kind)?,
            "MAX_NUM_ALGO_ORDERS" => {
                let wire: MaxNumAlgoOrdersWire = decode(pair, &kind, filter)?;
                once(&mut rules.max_num_algo_orders, wire.max_num_algo_orders, pair, &kind)?
            },
            // Filters this side does not read.
            _ => {},
        }
    }
    Ok(rules)
}

fn decode<T: DeserializeOwned>(pair: &str, kind: &str, filter: Value) -> Result<T, SpotReadError> {
    serde_json::from_value(filter).map_err(|e| SpotReadError::Parse(format!("{pair} {kind}: {e}")))
}

/// Fill `slot`, which a filter listed twice would make ambiguous.
fn once<T>(slot: &mut Option<T>, value: T, pair: &str, kind: &str) -> Result<(), SpotReadError> {
    if slot.replace(value).is_some() {
        return Err(SpotReadError::Inconsistent(format!(
            "exchangeInfo lists {kind} twice for {pair}"
        )));
    }
    Ok(())
}
