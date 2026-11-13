//! Account, order and commission records of the spot read side (ADR-0056
//! step 8a), parsed as Binance publishes them. Line numbers are in
//! `rest-api.md` at `995f6eb3`.
//!
//! A field Binance always shows is required, so a changed shape fails the
//! read instead of reading as zero; a conditional one is an `Option`.

use std::collections::HashSet;

use rust_decimal::Decimal;
use serde::{Deserialize, Deserializer};

use super::rest::{parse_json, SpotReadError};

/// One asset of the account.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SpotAccountBalance {
    pub asset: String,
    pub free: Decimal,
    pub locked: Decimal,
}

/// `GET /api/v3/account` (F23; L4278-L4332): the account's holdings and what
/// the venue says about its identity and authorisation. Parsed only;
/// eligibility reads these in a later step.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpotAccount {
    pub uid: u64,
    pub can_trade: bool,
    /// `"SPOT"` in Binance's example; kept raw.
    pub account_type: String,
    pub permissions: Vec<String>,
    /// Every asset as listed, zero balances included. One entry per asset.
    pub balances: Vec<SpotAccountBalance>,
}

impl SpotAccount {
    /// The balance of `asset`, when the account lists it.
    pub fn balance(&self, asset: &str) -> Option<&SpotAccountBalance> {
        self.balances.iter().find(|balance| balance.asset == asset)
    }
}

/// A spot order as `GET /api/v3/order` and `GET /api/v3/openOrders` return it
/// (F13, F23; L4364-L4400, L4430-L4468). A `STOP_LOSS` is one record from
/// acceptance to execution: its status and cumulative `executedQty` are its
/// own (S2).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpotOrderRecord {
    pub symbol: String,
    pub order_id: u64,
    /// `-1` unless the order belongs to an order list.
    pub order_list_id: i64,
    pub client_order_id: String,
    pub price: Decimal,
    pub orig_qty: Decimal,
    /// Cumulative executed quantity.
    pub executed_qty: Decimal,
    /// Cumulative quote quantity. `None` where Binance reports a negative
    /// value, which for some historical orders means "the data is not
    /// available at this time" (L4362).
    #[serde(deserialize_with = "known_quote_qty")]
    pub cummulative_quote_qty: Option<Decimal>,
    /// Raw status (`NEW`, `PARTIALLY_FILLED`, `FILLED`, `CANCELED`,
    /// `REJECTED`, `EXPIRED`, `EXPIRED_IN_MATCH`, ...; enums.md L45-L57).
    pub status: String,
    pub time_in_force: String,
    #[serde(rename = "type")]
    pub order_type: String,
    pub side: String,
    /// Shown for `STOP_LOSS`, `STOP_LOSS_LIMIT`, `TAKE_PROFIT` and
    /// `TAKE_PROFIT_LIMIT` orders (L186).
    #[serde(default)]
    pub stop_price: Option<Decimal>,
    pub time: i64,
    pub update_time: i64,
    /// When the order went onto the book; documented on lookups and listings
    /// (L4385), kept when given.
    #[serde(default)]
    pub working_time: Option<i64>,
    pub self_trade_prevention_mode: String,
    /// Shown once the order has expired (F3a).
    #[serde(default)]
    pub expiry_reason: Option<String>,
}

/// `cummulativeQuoteQty`, required, with a negative value read as unknown.
fn known_quote_qty<'de, D>(deserializer: D) -> Result<Option<Decimal>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = <Decimal as Deserialize>::deserialize(deserializer)?;
    Ok((value >= Decimal::ZERO).then_some(value))
}

/// The commission rates of one kind (F17).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SpotCommissionRates {
    pub maker: Decimal,
    pub taker: Decimal,
    pub buyer: Decimal,
    pub seller: Decimal,
}

/// The BNB discount block of the commission answer (F17).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpotCommissionDiscount {
    pub enabled_for_account: bool,
    pub enabled_for_symbol: bool,
    pub discount_asset: String,
    /// Multiplier on the standard commission when BNB pays it.
    pub discount: Decimal,
}

/// `GET /api/v3/account/commission` for one symbol (F17; L4954-L5002).
/// Parsed only: the N5 envelope that combines them is 8g's.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpotCommission {
    pub symbol: String,
    #[serde(rename = "standardCommission")]
    pub standard: SpotCommissionRates,
    #[serde(rename = "specialCommission")]
    pub special: SpotCommissionRates,
    #[serde(rename = "taxCommission")]
    pub tax: SpotCommissionRates,
    pub discount: SpotCommissionDiscount,
}

/// What is read of an open order list: that it exists.
#[derive(Deserialize)]
struct OrderListHead {
    #[serde(rename = "orderListId")]
    _order_list_id: i64,
}

pub(super) fn parse_account(body: &str) -> Result<SpotAccount, SpotReadError> {
    let account: SpotAccount = parse_json("account", body)?;
    let twice = {
        let mut assets = HashSet::new();
        account
            .balances
            .iter()
            .find(|b| !assets.insert(b.asset.as_str()))
            .map(|b| b.asset.clone())
    };
    if let Some(asset) = twice {
        return Err(SpotReadError::Inconsistent(format!("account lists {asset} twice")));
    }
    Ok(account)
}

pub(super) fn parse_orders(body: &str) -> Result<Vec<SpotOrderRecord>, SpotReadError> {
    parse_json("open orders", body)
}

pub(super) fn parse_order(body: &str) -> Result<SpotOrderRecord, SpotReadError> {
    parse_json("order", body)
}

pub(super) fn parse_order_list_count(body: &str) -> Result<usize, SpotReadError> {
    let lists: Vec<OrderListHead> = parse_json("open order lists", body)?;
    Ok(lists.len())
}

pub(super) fn parse_commission(body: &str, pair: &str) -> Result<SpotCommission, SpotReadError> {
    let commission: SpotCommission = parse_json("commission", body)?;
    if commission.symbol != pair {
        return Err(SpotReadError::Inconsistent(format!(
            "commission for {pair} answered {}",
            commission.symbol
        )));
    }
    Ok(commission)
}
