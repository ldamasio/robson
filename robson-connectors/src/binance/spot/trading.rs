//! Binance spot order placement (ADR-0056 step 8, PR 8c-3a): the one place a
//! spot order is sent, its parameters, the typing of the venue's answer and
//! the evidence a placement answer carries (8c plan §3–§5).
//!
//! **One way out.** [`send_new_order`] is the only caller of the client's
//! `post_signed_answer`, and calls it inside [`SubmitAttemptLedger::submit`].
//! The adapter's ledger is a one-attempt ledger: its send start takes the
//! grant the attached journal gave this order (`begin_authorized_send`,
//! 8c-1), so nothing goes out without a grant and nothing goes out twice
//! under one client id. The client passes the operation gate before it signs.
//! One request per order, never a retry.
//!
//! **Parameters.** Every order sends `newClientOrderId`, an explicit
//! `newOrderRespType` and `selfTradePreventionMode=EXPIRE_MAKER`, and never
//! `reduceOnly` (spot has none) or `timeInForce` (rest-api.md L2151-L2172,
//! L2183-L2184; enums.md L174-L183).
//!
//! **Answers.** Only HTTP 200 is a success. HTTP 400 with -2010 is
//! `OrderRejected` and with -1013 `RulesRejected`: the venue's final refusal
//! of this attempt, and the only answers the ledger records as rejections
//! (errors.md L47-L51, L427-L429). Everything else that may have reached the
//! venue is uncertain and leaves the attempt unresolved: another 2xx, those
//! codes at another status, -1006 and -1007 (errors.md L35-L41), a 5XX
//! (rest-api.md L141-L143), a 403, 418 or 429, a timeout, an unreadable
//! answer. A gate refusal is a proven no-send.
//!
//! Line numbers are in `binance-spot-api-docs` at `995f6eb3`.

use chrono::DateTime;
use robson_domain::{OrderSide, Price, Quantity, Symbol};
use robson_exec::{
    ports::{FeeEvidence, FeeGap, OrderEvidence, TradeFill},
    venue::{PlacedStop, ProtectionKind, StopRef, SubmitAttemptLedger, VenueOrderRecord},
    ExecError, OrderResult,
};
use rust_decimal::Decimal;
use serde::Deserialize;
use tracing::warn;

use super::{
    exchange::order_phase,
    records::known_quote_qty,
    rest::{parse_json, SpotReadError},
};
use crate::binance_rest::{BinanceRestClient, BinanceRestError, RestAnswer};

/// The self-trade prevention mode every new order sends, never downgraded
/// (8c plan §4): a self-match expires the resting order's remaining quantity
/// (STP FAQ L30-L31).
pub const SPOT_STP_MODE: &str = "EXPIRE_MAKER";

/// One spot order mutation: its path, Binance's documented request weight and
/// the orders it adds to the unfilled-order count, the figures the request
/// limiter reserves before signing (8h).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpotWrite {
    pub path: &'static str,
    pub weight: u32,
    pub orders: u32,
}

impl SpotWrite {
    /// `POST /api/v3/order`: weight 1, one order (rest-api.md L2135-L2148).
    pub const NEW_ORDER: Self = Self {
        path: "/api/v3/order",
        weight: 1,
        orders: 1,
    };
}

/// What a new order asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum OrderKind {
    /// `MARKET`, answered `FULL` (rest-api.md L2166, L2183).
    Market,
    /// `STOP_LOSS` at `trigger`, answered `RESULT`: the default for it is
    /// `ACK`, which carries no record (rest-api.md L2163, L2166, L2184).
    StopLoss { trigger: Decimal },
}

/// One new order, as sent.
#[derive(Debug, Clone, Copy)]
pub(super) struct NewOrder<'a> {
    pub symbol: &'a Symbol,
    pub side: OrderSide,
    pub quantity: Decimal,
    pub client_order_id: &'a str,
    pub kind: OrderKind,
}

impl NewOrder<'_> {
    fn order_type(&self) -> &'static str {
        match self.kind {
            OrderKind::Market => "MARKET",
            OrderKind::StopLoss { .. } => "STOP_LOSS",
        }
    }

    fn side(&self) -> &'static str {
        match self.side {
            OrderSide::Buy => "BUY",
            OrderSide::Sell => "SELL",
        }
    }

    /// The request's parameters, before the timestamp and signature.
    pub(super) fn params(&self) -> Vec<(&'static str, String)> {
        let mut params = vec![
            ("symbol", self.symbol.as_pair()),
            ("side", self.side().to_string()),
            ("type", self.order_type().to_string()),
            ("quantity", self.quantity.normalize().to_string()),
            ("newClientOrderId", self.client_order_id.to_string()),
            ("selfTradePreventionMode", SPOT_STP_MODE.to_string()),
        ];
        match self.kind {
            OrderKind::Market => params.push(("newOrderRespType", "FULL".to_string())),
            OrderKind::StopLoss { trigger } => {
                params.push(("stopPrice", trigger.normalize().to_string()));
                params.push(("newOrderRespType", "RESULT".to_string()));
            },
        }
        params
    }
}

/// One fill of a `FULL` answer (rest-api.md L2265-L2301).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SpotFill {
    pub price: Decimal,
    pub qty: Decimal,
    pub commission: Decimal,
    pub commission_asset: String,
    pub trade_id: u64,
}

/// A `RESULT` or `FULL` answer to `POST /api/v3/order` (rest-api.md
/// L2222-L2302). A field both always show is required; `stopPrice` and
/// `expiryReason` are conditional (L170-L197), `fills` is `FULL`'s.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SpotOrderAnswer {
    pub symbol: String,
    pub order_id: u64,
    pub order_list_id: i64,
    pub client_order_id: String,
    pub transact_time: i64,
    pub orig_qty: Decimal,
    pub executed_qty: Decimal,
    /// `None` where Binance reports a negative value: unavailable, never
    /// zero (8c plan §5 rule 2).
    #[serde(deserialize_with = "known_quote_qty")]
    pub cummulative_quote_qty: Option<Decimal>,
    pub status: String,
    #[serde(rename = "type")]
    pub order_type: String,
    pub side: String,
    #[serde(default)]
    pub working_time: Option<i64>,
    pub self_trade_prevention_mode: String,
    #[serde(default)]
    pub stop_price: Option<Decimal>,
    #[serde(default)]
    pub expiry_reason: Option<String>,
    #[serde(default)]
    pub fills: Option<Vec<SpotFill>>,
}

/// The ledger's rejections: the venue's final refusal of this attempt. An
/// expiry with nothing executed is not one (C4).
fn is_final_rejection(error: &ExecError) -> bool {
    matches!(error, ExecError::OrderRejected(_) | ExecError::RulesRejected(_))
}

/// An answer that established nothing: the order may exist.
fn uncertain(why: impl std::fmt::Display) -> ExecError {
    ExecError::Exchange(format!("binance:spot order uncertain, it may exist: {why}"))
}

/// An order's answer, typed before its body is read: the body of an exact
/// HTTP 200, or what the failure proves.
fn order_answer(answer: Result<RestAnswer, BinanceRestError>) -> Result<String, ExecError> {
    let answer = match answer {
        Ok(answer) => answer,
        // Refused by the gate before anything was built or sent.
        Err(BinanceRestError::Refused(why)) => return Err(ExecError::SendRefused(why)),
        Err(BinanceRestError::SignatureError(why)) => {
            return Err(ExecError::SendRefused(format!(
                "binance:spot order not signed, nothing sent: {why}"
            )))
        },
        Err(BinanceRestError::Timeout) => {
            return Err(ExecError::Timeout(
                "binance:spot order timed out; it may exist".to_string(),
            ))
        },
        Err(other) => return Err(uncertain(other)),
    };
    let status = answer.status.as_u16();
    if status == 200 {
        return Ok(answer.body);
    }
    if answer.status.is_success() {
        return Err(uncertain(format!("HTTP {status}, not 200")));
    }
    Err(match SpotReadError::from_answer(status, answer.body) {
        SpotReadError::Api { http_status: 400, code: -2010, msg } => {
            ExecError::OrderRejected(format!("Binance -2010 (HTTP 400): {msg}"))
        },
        SpotReadError::Api { http_status: 400, code: -1013, msg } => {
            ExecError::RulesRejected(format!("Binance -1013 (HTTP 400): {msg}"))
        },
        other => uncertain(other),
    })
}

/// Send `order` through `ledger`: the one call of `post_signed_answer`. The
/// ledger's send start takes the order's grant before the request can be
/// polled; `read` types an exact 200's body. Every error other than the two
/// final rejections leaves the attempt unresolved.
async fn send_new_order<T>(
    client: &BinanceRestClient,
    ledger: &SubmitAttemptLedger,
    order: &NewOrder<'_>,
    read: impl FnOnce(&str) -> Result<T, ExecError>,
) -> Result<T, ExecError> {
    let params = order.params();
    let result = ledger
        .submit(order.client_order_id, is_final_rejection, async {
            read(&order_answer(
                client.post_signed_answer(SpotWrite::NEW_ORDER.path, params).await,
            )?)
        })
        .await;
    if let Err(error) = &result {
        if !is_final_rejection(error) {
            warn!(
                client_order_id = order.client_order_id,
                symbol = %order.symbol.as_pair(),
                %error,
                "Binance spot order not established"
            );
        }
    }
    result
}

/// The answer, and the record it gives, when it echoes what was sent: the
/// symbol, client id, side, type, quantity, STP mode and trigger, outside any
/// order list, with `0 ≤ executedQty ≤ origQty`. Anything else established
/// nothing.
fn read_answer(
    body: &str,
    order: &NewOrder<'_>,
) -> Result<(SpotOrderAnswer, VenueOrderRecord), ExecError> {
    let answer: SpotOrderAnswer = parse_json("order answer", body).map_err(uncertain)?;
    let trigger = match order.kind {
        OrderKind::StopLoss { trigger } => Some(trigger),
        OrderKind::Market => None,
    };
    let checks = [
        (answer.symbol == order.symbol.as_pair(), "symbol"),
        (answer.client_order_id == order.client_order_id, "clientOrderId"),
        (answer.side == order.side(), "side"),
        (answer.order_type == order.order_type(), "type"),
        (answer.orig_qty == order.quantity, "origQty"),
        (answer.self_trade_prevention_mode == SPOT_STP_MODE, "selfTradePreventionMode"),
        (answer.order_list_id == -1, "orderListId"),
        (trigger.is_none() || answer.stop_price == trigger, "stopPrice"),
    ];
    if let Some((_, field)) = checks.iter().find(|(echoed, _)| !echoed) {
        return Err(uncertain(format!(
            "order {} answered with another {field} than {} sent",
            answer.order_id, order.client_order_id
        )));
    }
    let record = VenueOrderRecord {
        exchange_order_id: answer.order_id.to_string(),
        client_order_id: answer.client_order_id.clone(),
        orig_client_order_id: None,
        symbol: answer.symbol.clone(),
        side: order.side,
        order_type: answer.order_type.clone(),
        protection_kind: (answer.order_type == "STOP_LOSS")
            .then_some(ProtectionKind::MarketOnTrigger),
        status: answer.status.clone(),
        phase: order_phase(&answer.status),
        orig_qty: answer.orig_qty,
        executed_qty: answer.executed_qty,
        cumulative_quote_qty: answer.cummulative_quote_qty,
        stop_price: answer.stop_price,
        expiry_reason: answer.expiry_reason.clone(),
        transact_time_ms: Some(answer.transact_time),
        time_ms: None,
        update_time_ms: None,
        working_time_ms: answer.working_time,
    };
    if !record.quantities_consistent() {
        return Err(uncertain(format!(
            "order {} answered executedQty {} against origQty {}",
            answer.order_id, answer.executed_qty, answer.orig_qty
        )));
    }
    Ok((answer, record))
}

/// The answer's `transactTime`, as given.
fn transacted_at(answer: &SpotOrderAnswer) -> Result<DateTime<chrono::Utc>, ExecError> {
    DateTime::from_timestamp_millis(answer.transact_time).ok_or_else(|| {
        uncertain(format!(
            "order {} answered transactTime {}",
            answer.order_id, answer.transact_time
        ))
    })
}

/// What a `FULL` answer's fills establish (8c plan §5 rule 3).
enum FillSet {
    /// Valid, distinct fills whose quantities sum to the executed quantity.
    Complete {
        fills: Vec<TradeFill>,
        qty: Decimal,
        quote: Decimal,
    },
    /// Valid, distinct fills covering less than the executed quantity.
    Incomplete(Vec<TradeFill>),
    /// Fills that establish nothing.
    Unusable(FeeGap),
}

/// The fills of an order that executed `executed`: each with a trade id,
/// `qty > 0`, `price > 0`, `commission ≥ 0` and an asset; an identical
/// duplicate counts once, a different one under the same id is a conflict;
/// more than `executed` in all is a conflict. Each fill's quote is
/// `price × qty`, checked; its time is unknown, never filled in.
fn fill_set(fills: Option<&[SpotFill]>, executed: Decimal) -> FillSet {
    let Some(fills) = fills.filter(|fills| !fills.is_empty()) else {
        return FillSet::Unusable(FeeGap::NoFills);
    };
    let mut distinct: Vec<&SpotFill> = Vec::with_capacity(fills.len());
    for fill in fills {
        let valid = fill.qty > Decimal::ZERO
            && fill.price > Decimal::ZERO
            && fill.commission >= Decimal::ZERO
            && !fill.commission_asset.is_empty();
        if !valid {
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

/// R3 (8c plan §5 rule 5): the record's quote and the complete fills' agree
/// when `|record − fills| ≤ fills × 10^-quoteAssetPrecision`, every step
/// checked. No precision cached: no tolerance.
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

/// The commission total of complete fills, in their one asset (C5).
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

/// The price, fills and fees of an executed MARKET answer (8c plan §5).
struct MarketEvidence {
    price: Decimal,
    fills: Vec<TradeFill>,
    fee: Decimal,
    fee_asset: String,
    fee_evidence: FeeEvidence,
}

/// Rules 2, 4 and 5 to 7 over an answer that executed `executed > 0`. The
/// price is the record's quote over `executedQty`, else the complete fills'
/// quote over their quantity; never estimated, and without one the answer is
/// uncertain (R4). A zero quote with execution is inconsistent.
fn market_evidence(
    answer: &SpotOrderAnswer,
    executed: Decimal,
    quote_precision: Option<u32>,
) -> Result<MarketEvidence, ExecError> {
    let id = answer.order_id;
    if answer.cummulative_quote_qty == Some(Decimal::ZERO) {
        return Err(uncertain(format!(
            "order {id} executed {executed} for a zero cummulativeQuoteQty"
        )));
    }
    let fills = fill_set(answer.fills.as_deref(), executed);
    let price = match (answer.cummulative_quote_qty, &fills) {
        (Some(quote), _) => quote.checked_div(executed),
        (None, FillSet::Complete { qty, quote, .. }) => quote.checked_div(*qty),
        (None, _) => None,
    };
    let Some(price) = price.filter(|price| *price > Decimal::ZERO) else {
        return Err(uncertain(format!("order {id} executed {executed} with no available price")));
    };
    let fills = match (answer.cummulative_quote_qty, fills) {
        (Some(record_quote), FillSet::Complete { fills, qty, quote }) => {
            match quotes_agree(record_quote, quote, fills.len(), quote_precision) {
                Ok(()) => FillSet::Complete { fills, qty, quote },
                Err(gap) => {
                    warn!(
                        order_id = id,
                        %record_quote,
                        fills_quote = %quote,
                        "Binance spot fills disagree with the order's quote; the record stands"
                    );
                    FillSet::Unusable(gap)
                },
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

/// A MARKET order's `FULL` answer (8c plan §4): `FILLED`, or `EXPIRED` with
/// execution (market orders FAQ L15-L19), is the result of `executedQty`;
/// `EXPIRED` with nothing executed is [`ExecError::ExpiredUnfilled`]; every
/// other status, `CANCELED`, `REJECTED` and `EXPIRED_IN_MATCH` included, is
/// uncertain, for the caller's later lookup to resolve.
fn read_market(
    body: &str,
    order: &NewOrder<'_>,
    quote_precision: Option<u32>,
) -> Result<OrderResult, ExecError> {
    let (answer, record) = read_answer(body, order)?;
    let executed = record.executed_qty;
    match record.status.as_str() {
        "FILLED" | "EXPIRED" if executed > Decimal::ZERO => {},
        "EXPIRED" => return Err(ExecError::ExpiredUnfilled(Box::new(record))),
        status => {
            return Err(uncertain(format!(
                "MARKET order {} answered {status} with executedQty {executed}",
                record.exchange_order_id
            )))
        },
    }
    let evidence = market_evidence(&answer, executed, quote_precision)?;
    Ok(OrderResult {
        exchange_order_id: record.exchange_order_id.clone(),
        client_order_id: record.client_order_id.clone(),
        fill_price: Price::new(evidence.price)?,
        filled_quantity: Quantity::new(executed)?,
        fee: evidence.fee,
        fee_asset: evidence.fee_asset,
        filled_at: transacted_at(&answer)?,
        evidence: Some(Box::new(OrderEvidence {
            fills: evidence.fills,
            record: Some(record),
            fee_evidence: evidence.fee_evidence,
        })),
    })
}

/// A `STOP_LOSS`'s `RESULT` answer (8c plan §4): whatever its status, the
/// stop with its record, which alone says whether it rests. The legacy fields
/// echo the trigger, carry `executedQty` and a zero fee; the fee is a `Total`
/// in the quote asset when nothing executed, and unavailable when something
/// did, as `RESULT` carries no fills. No price rule: a stop's execution price
/// is its record's and its trades'.
fn read_stop(body: &str, order: &NewOrder<'_>, trigger: Price) -> Result<PlacedStop, ExecError> {
    let (answer, record) = read_answer(body, order)?;
    let executed = record.executed_qty;
    let (filled_quantity, fee_asset, fee_evidence) = if executed.is_zero() {
        (Quantity::zero(), order.symbol.quote().to_string(), FeeEvidence::Total)
    } else {
        (
            Quantity::new(executed)?,
            String::new(),
            FeeEvidence::Unavailable(FeeGap::NoFills),
        )
    };
    Ok(PlacedStop {
        stop: StopRef::new(record.exchange_order_id.clone()),
        acceptance: OrderResult {
            exchange_order_id: record.exchange_order_id.clone(),
            client_order_id: record.client_order_id.clone(),
            fill_price: trigger,
            filled_quantity,
            fee: Decimal::ZERO,
            fee_asset,
            filled_at: transacted_at(&answer)?,
            evidence: Some(Box::new(OrderEvidence {
                fills: Vec::new(),
                record: Some(record),
                fee_evidence,
            })),
        },
    })
}

/// One MARKET order with `FULL`, through `ledger` (8c plan §4).
/// `quote_precision` is the cached rules' `quoteAssetPrecision`, for R3.
pub(super) async fn place_market(
    client: &BinanceRestClient,
    ledger: &SubmitAttemptLedger,
    order: NewOrder<'_>,
    quote_precision: Option<u32>,
) -> Result<OrderResult, ExecError> {
    send_new_order(client, ledger, &order, |body| read_market(body, &order, quote_precision)).await
}

/// One `STOP_LOSS` SELL with `stopPrice` and `RESULT`, through `ledger` (8c
/// plan §4).
pub(super) async fn place_stop(
    client: &BinanceRestClient,
    ledger: &SubmitAttemptLedger,
    symbol: &Symbol,
    quantity: Quantity,
    trigger: Price,
    client_order_id: &str,
) -> Result<PlacedStop, ExecError> {
    let order = NewOrder {
        symbol,
        side: OrderSide::Sell,
        quantity: quantity.as_decimal(),
        client_order_id,
        kind: OrderKind::StopLoss { trigger: trigger.as_decimal() },
    };
    send_new_order(client, ledger, &order, |body| read_stop(body, &order, trigger)).await
}
