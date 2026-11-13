//! The spot adapter's fill-evidence rules (8c plan §4, §5), mirrored for
//! [`crate::balance_stub`]: the stub certifies exactly what the adapter
//! certifies. Each item restates the function of the same name in
//! `robson-connectors/src/binance/spot/trading.rs`, which is `pub(super)`
//! production code 8d does not change, so it cannot be called from here.
//!
//! - Rule 3, per fill or trade: `price > 0`, `qty > 0`, `commission ≥ 0` and an
//!   asset; a trade's quote amount is never negative.
//! - Fills are distinct by trade id: an identical duplicate counts once, a
//!   different one under the same id is a conflict.
//! - A MARKET answer's fills ([`market_evidence`]) are complete when they sum
//!   to the execution, incomplete when less, unusable when invalid, conflicting
//!   or more; the order's price is its record's quote over its execution, and
//!   its fee a `Total` only over complete fills in one asset.
//! - An order's trades ([`terminal_result`]) give a result only for a terminal
//!   order whose valid, distinct trades sum exactly to its execution and whose
//!   quote agrees with its record's (R3); anything else withholds the fills,
//!   never the record's terminality or execution.
//!
//! R3's tolerance is `fills × 10^-quoteAssetPrecision`; with no precision,
//! as the stub has none, it is zero, as on the adapter with none cached.

use chrono::{DateTime, Utc};
use robson_domain::{Price, Quantity};
use robson_exec::{
    ports::{FeeEvidence, FeeGap, OrderEvidence, TradeFill},
    venue::{OrderPhase, VenueOrderRecord},
    OrderResult,
};
use rust_decimal::Decimal;

/// One fill of a `FULL` answer (`SpotFill`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MarketFill {
    pub price: Decimal,
    pub qty: Decimal,
    pub commission: Decimal,
    pub commission_asset: String,
    pub trade_id: u64,
}

/// What a `FULL` answer's fills establish (`FillSet`).
enum FillSet {
    Complete {
        fills: Vec<TradeFill>,
        qty: Decimal,
        quote: Decimal,
    },
    Incomplete(Vec<TradeFill>),
    Unusable(FeeGap),
}

/// Rule 3 for one fill or trade (`valid_fill`).
fn valid_fill(price: Decimal, qty: Decimal, commission: Decimal, asset: &str) -> bool {
    qty > Decimal::ZERO && price > Decimal::ZERO && commission >= Decimal::ZERO && !asset.is_empty()
}

/// Rule 3 for a trade, and a quote amount never negative (`valid_trade`).
fn valid_trade(trade: &TradeFill) -> bool {
    valid_fill(trade.price, trade.qty, trade.commission, &trade.commission_asset)
        && trade.quote_qty >= Decimal::ZERO
}

/// The fills of an order that executed `executed` (`fill_set`).
fn fill_set(fills: Option<&[MarketFill]>, executed: Decimal) -> FillSet {
    let Some(fills) = fills.filter(|fills| !fills.is_empty()) else {
        return FillSet::Unusable(FeeGap::NoFills);
    };
    let mut distinct: Vec<&MarketFill> = Vec::with_capacity(fills.len());
    for fill in fills {
        if !valid_fill(fill.price, fill.qty, fill.commission, &fill.commission_asset) {
            return FillSet::Unusable(FeeGap::Conflicting);
        }
        match distinct.iter().find(|seen| seen.trade_id == fill.trade_id) {
            Some(seen) if *seen == fill => {},
            Some(_) => return FillSet::Unusable(FeeGap::Conflicting),
            None => distinct.push(fill),
        }
    }
    let (mut qty, mut quote) = (Decimal::ZERO, Decimal::ZERO);
    let mut trade_fills = Vec::with_capacity(distinct.len());
    for fill in distinct {
        let Some(fill_quote) = fill.price.checked_mul(fill.qty) else {
            return FillSet::Unusable(FeeGap::Overflow);
        };
        let (Some(new_qty), Some(new_quote)) =
            (qty.checked_add(fill.qty), quote.checked_add(fill_quote))
        else {
            return FillSet::Unusable(FeeGap::Overflow);
        };
        (qty, quote) = (new_qty, new_quote);
        trade_fills.push(TradeFill {
            trade_id: fill.trade_id.to_string(),
            price: fill.price,
            qty: fill.qty,
            quote_qty: fill_quote,
            commission: fill.commission,
            commission_asset: fill.commission_asset.clone(),
            time: None,
        });
    }
    match qty.cmp(&executed) {
        std::cmp::Ordering::Greater => FillSet::Unusable(FeeGap::Conflicting),
        std::cmp::Ordering::Equal => FillSet::Complete { fills: trade_fills, qty, quote },
        std::cmp::Ordering::Less => FillSet::Incomplete(trade_fills),
    }
}

/// R3 (`quotes_agree`).
fn quotes_agree(
    record_quote: Decimal,
    fills_quote: Decimal,
    fill_count: usize,
    quote_precision: Option<u32>,
) -> Result<(), FeeGap> {
    let unit = quote_precision
        .and_then(|precision| Decimal::try_new(1, precision).ok())
        .unwrap_or(Decimal::ZERO);
    let tolerance = Decimal::from(fill_count).checked_mul(unit).ok_or(FeeGap::Overflow)?;
    let gap = record_quote.checked_sub(fills_quote).ok_or(FeeGap::Overflow)?.abs();
    if gap <= tolerance {
        Ok(())
    } else {
        Err(FeeGap::Conflicting)
    }
}

/// The commission total of complete fills, in their one asset
/// (`commission_total`).
fn commission_total(fills: &[TradeFill]) -> Result<(Decimal, String), FeeGap> {
    let Some(first) = fills.first() else {
        return Err(FeeGap::NoFills);
    };
    if fills.iter().any(|fill| fill.commission_asset != first.commission_asset) {
        return Err(FeeGap::MixedAssets);
    }
    let total = fills
        .iter()
        .try_fold(Decimal::ZERO, |total, fill| total.checked_add(fill.commission))
        .ok_or(FeeGap::Overflow)?;
    Ok((total, first.commission_asset.clone()))
}

/// The price, fills and fees of an executed MARKET answer
/// (`MarketEvidence`).
pub(crate) struct MarketEvidence {
    pub price: Decimal,
    pub fills: Vec<TradeFill>,
    pub fee: Decimal,
    pub fee_asset: String,
    pub fee_evidence: FeeEvidence,
}

/// Rules 2, 4 and 5 to 7 over an answer that executed `executed > 0`
/// (`market_evidence`). `Err` carries why the answer is uncertain.
pub(crate) fn market_evidence(
    record_quote: Option<Decimal>,
    fills: Option<&[MarketFill]>,
    executed: Decimal,
    quote_precision: Option<u32>,
) -> Result<MarketEvidence, String> {
    if record_quote == Some(Decimal::ZERO) {
        return Err(format!("executed {executed} for a zero cummulativeQuoteQty"));
    }
    let fills = fill_set(fills, executed);
    let price = match (record_quote, &fills) {
        (Some(quote), _) => quote.checked_div(executed),
        (None, FillSet::Complete { qty, quote, .. }) => quote.checked_div(*qty),
        (None, _) => None,
    };
    let Some(price) = price.filter(|price| *price > Decimal::ZERO) else {
        return Err(format!("executed {executed} with no available price"));
    };
    let fills = match (record_quote, fills) {
        (Some(record_quote), FillSet::Complete { fills, qty, quote }) => {
            match quotes_agree(record_quote, quote, fills.len(), quote_precision) {
                Ok(()) => FillSet::Complete { fills, qty, quote },
                Err(gap) => FillSet::Unusable(gap),
            }
        },
        (_, fills) => fills,
    };
    let (fills, fee_evidence, fee, fee_asset) = match fills {
        FillSet::Complete { fills, .. } => match commission_total(&fills) {
            Ok((total, asset)) => (fills, FeeEvidence::Total, total, asset),
            Err(gap) => (fills, FeeEvidence::Unavailable(gap), Decimal::ZERO, String::new()),
        },
        FillSet::Incomplete(fills) => (
            fills,
            FeeEvidence::Unavailable(FeeGap::Incomplete),
            Decimal::ZERO,
            String::new(),
        ),
        FillSet::Unusable(gap) => {
            (Vec::new(), FeeEvidence::Unavailable(gap), Decimal::ZERO, String::new())
        },
    };
    Ok(MarketEvidence {
        price,
        fills,
        fee,
        fee_asset,
        fee_evidence,
    })
}

/// The fills rule over an order's own record and the trades read for it
/// (`terminal_result`).
pub(crate) fn terminal_result(
    record: &VenueOrderRecord,
    trades: Vec<TradeFill>,
    quote_precision: Option<u32>,
) -> Option<OrderResult> {
    let executed = record.executed_qty;
    if record.phase != OrderPhase::Terminal || executed <= Decimal::ZERO {
        return None;
    }
    if record.cumulative_quote_qty == Some(Decimal::ZERO) {
        return None;
    }
    let mut distinct: Vec<TradeFill> = Vec::with_capacity(trades.len());
    for trade in trades {
        if !valid_trade(&trade) {
            return None;
        }
        match distinct.iter().find(|seen| seen.trade_id == trade.trade_id) {
            Some(seen) if *seen == trade => {},
            Some(_) => return None,
            None => distinct.push(trade),
        }
    }
    let (mut qty, mut quote) = (Decimal::ZERO, Decimal::ZERO);
    for trade in &distinct {
        qty = qty.checked_add(trade.qty)?;
        quote = quote.checked_add(trade.quote_qty)?;
    }
    if qty != executed {
        return None;
    }
    let price = match record.cumulative_quote_qty {
        Some(record_quote) => {
            if quotes_agree(record_quote, quote, distinct.len(), quote_precision).is_err() {
                return None;
            }
            record_quote.checked_div(executed)
        },
        None => quote.checked_div(qty),
    }?;
    let filled_at: DateTime<Utc> = distinct.iter().filter_map(|trade| trade.time).max()?;
    let (fee, fee_asset, fee_evidence) = match commission_total(&distinct) {
        Ok((total, asset)) => (total, asset, FeeEvidence::Total),
        Err(gap) => (Decimal::ZERO, String::new(), FeeEvidence::Unavailable(gap)),
    };
    Some(OrderResult {
        exchange_order_id: record.exchange_order_id.clone(),
        client_order_id: record
            .orig_client_order_id
            .clone()
            .unwrap_or_else(|| record.client_order_id.clone()),
        fill_price: Price::new(price).ok()?,
        filled_quantity: Quantity::new(executed).ok()?,
        fee,
        fee_asset,
        filled_at,
        evidence: Some(Box::new(OrderEvidence {
            fills: distinct,
            record: Some(record.clone()),
            fee_evidence,
        })),
    })
}
