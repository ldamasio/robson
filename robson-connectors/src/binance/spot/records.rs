//! Account, order and commission records of the spot read side (ADR-0056
//! step 8a), parsed as Binance publishes them. Line numbers are in
//! `rest-api.md` at `995f6eb3`.
//!
//! A field Binance always shows is required, so a changed shape fails the
//! read instead of reading as zero; a conditional one is an `Option`.

use std::{collections::HashSet, fmt, marker::PhantomData};

use robson_exec::balance_costs::exact;
use rust_decimal::Decimal;
use serde::{de, Deserialize, Deserializer};

use super::rest::{parse_json, SpotReadError};

/// One asset of the account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpotAccountBalance {
    pub asset: String,
    pub free: Decimal,
    pub locked: Decimal,
}

impl<'de> Deserialize<'de> for SpotAccountBalance {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Raw {
            asset: String,
            free: exact::ExactDecimal,
            locked: exact::ExactDecimal,
        }
        let raw = Raw::deserialize(deserializer)?;
        let free = raw.free.value();
        let locked = raw.locked.value();
        if free < Decimal::ZERO || locked < Decimal::ZERO {
            return Err(serde::de::Error::custom(format!(
                "negative spot balance for {}",
                raw.asset
            )));
        }
        Ok(Self { asset: raw.asset, free, locked })
    }
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

/// `cummulativeQuoteQty`, required, with a negative value read as unknown
/// (here and in an order's placement answer, 8c-3).
pub(super) fn known_quote_qty<'de, D>(deserializer: D) -> Result<Option<Decimal>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = <Decimal as Deserialize>::deserialize(deserializer)?;
    Ok((value >= Decimal::ZERO).then_some(value))
}

/// The commission rates of one kind (F17).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SpotCommissionRates {
    #[serde(deserialize_with = "exact::nonnegative_exact")]
    pub maker: exact::ExactDecimal,
    #[serde(deserialize_with = "exact::nonnegative_exact")]
    pub taker: exact::ExactDecimal,
    #[serde(deserialize_with = "exact::nonnegative_exact")]
    pub buyer: exact::ExactDecimal,
    #[serde(deserialize_with = "exact::nonnegative_exact")]
    pub seller: exact::ExactDecimal,
}

/// The BNB discount block of the commission answer (F17).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpotCommissionDiscount {
    pub enabled_for_account: bool,
    pub enabled_for_symbol: bool,
    pub discount_asset: String,
    /// Multiplier on the standard commission when BNB pays it.
    #[serde(deserialize_with = "exact::nonnegative_exact")]
    pub discount: exact::ExactDecimal,
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

/// One member of an open order list, its identity as the venue gives it
/// (`GET /api/v3/openOrderList`, F23; L4680-L4716).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpotOrderListMember {
    /// Raw, case kept.
    pub symbol: String,
    /// The native order ID, as [`SpotOrderRecord::order_id`].
    pub order_id: u64,
    /// Raw, case kept.
    pub client_order_id: String,
}

/// One open order list: its native ID, raw symbol and member references.
/// Descriptive only: no account coherence, eligibility or scope is implied
/// (8k-1). A symbol Robson does not trade is kept for later classification.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpotOpenOrderList {
    /// The native list ID. The validated decoder refuses the `-1` sentinel.
    pub order_list_id: i64,
    /// Raw, case kept.
    pub symbol: String,
    /// The list's own client ID, raw and case kept. Optional so earlier
    /// bodies without it still decode; absence is never an identity.
    #[serde(default)]
    pub list_client_order_id: Option<String>,
    /// Members in the venue's order. The validated decoder requires a member.
    #[serde(deserialize_with = "object_entries")]
    pub orders: Vec<SpotOrderListMember>,
}

/// Why open order lists were refused. No variant carries input text beyond
/// a validated ID, and nothing of the input is kept on refusal.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SpotOrderListError {
    /// The input exceeds the caller's byte envelope; nothing was parsed.
    #[error("open order lists: {len} bytes exceed the {limit}-byte envelope")]
    Oversize { len: usize, limit: usize },
    /// Not the documented shape: a required field is missing, repeated or of
    /// another type, or the JSON is incomplete.
    #[error("open order lists: malformed ({0})")]
    Malformed(String),
    /// A negative list ID, the "no list" sentinel among them.
    #[error("open order lists: negative orderListId {0}")]
    NegativeListId(i64),
    #[error("open order lists: orderListId {0} is listed twice")]
    DuplicateList(i64),
    #[error("open order lists: list {0} has no member orders")]
    NoMembers(i64),
    /// An empty, whitespace or control-character symbol or client ID.
    #[error("open order lists: list {list_id} has an invalid {field}")]
    InvalidIdentity { list_id: i64, field: &'static str },
    /// A member on another symbol than its list.
    #[error("open order lists: list {0} has a member on another symbol")]
    SymbolDisagreement(i64),
    /// The same `(symbol, orderId)` or `(symbol, clientOrderId)` twice,
    /// whether repeated or conflicting.
    #[error("open order lists: list {list_id} repeats or conflicts on order {order_id}")]
    DuplicateMember { list_id: i64, order_id: u64 },
    /// Validation memory could not be reserved.
    #[error("open order lists: allocation refused")]
    Allocation,
}

impl From<SpotOrderListError> for SpotReadError {
    fn from(error: SpotOrderListError) -> Self {
        match error {
            SpotOrderListError::Oversize { .. } | SpotOrderListError::Malformed(_) => {
                Self::Parse(error.to_string())
            },
            _ => Self::Inconsistent(error.to_string()),
        }
    }
}

struct Object<T>(T);

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Object<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct OnlyMap<T>(PhantomData<T>);

        impl<'de, T: Deserialize<'de>> de::Visitor<'de> for OnlyMap<T> {
            type Value = Object<T>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an order-list object")
            }

            fn visit_map<M: de::MapAccess<'de>>(self, map: M) -> Result<Self::Value, M::Error> {
                T::deserialize(de::value::MapAccessDeserializer::new(map)).map(Object)
            }
        }

        deserializer.deserialize_map(OnlyMap(PhantomData))
    }
}

fn object_entries<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct Entries<T>(PhantomData<T>);

    impl<'de, T: Deserialize<'de>> de::Visitor<'de> for Entries<T> {
        type Value = Vec<T>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("an array of order-list objects")
        }

        fn visit_seq<S: de::SeqAccess<'de>>(self, mut seq: S) -> Result<Self::Value, S::Error> {
            let mut entries = Vec::new();
            while let Some(Object(entry)) = seq.next_element()? {
                entries.try_reserve(1).map_err(|_| de::Error::custom("allocation refused"))?;
                entries.push(entry);
            }
            Ok(entries)
        }
    }

    deserializer.deserialize_seq(Entries(PhantomData))
}

#[derive(Deserialize)]
#[serde(transparent)]
struct OpenOrderListEntries(#[serde(deserialize_with = "object_entries")] Vec<SpotOpenOrderList>);

fn valid_identity(text: &str) -> bool {
    !text.is_empty() && !text.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// Decode a completed `GET /api/v3/openOrderList` body held within the
/// caller's `max_bytes` envelope: synchronous, no I/O, all or nothing.
///
/// The length is checked before any parse. Every list and member is then
/// validated in one pass; one hash entry per list ID and two per member
/// (its order ID and its client ID, each under its raw symbol), reserved
/// checked. Retained identity text is copied from the input, so it never
/// exceeds `body.len()`. The envelope is the caller's: it does not by itself
/// bound how the body was received (no C8 transport claim).
pub fn decode_open_order_lists(
    body: &str,
    max_bytes: usize,
) -> Result<Vec<SpotOpenOrderList>, SpotOrderListError> {
    if body.len() > max_bytes {
        return Err(SpotOrderListError::Oversize { len: body.len(), limit: max_bytes });
    }
    let OpenOrderListEntries(lists) = serde_json::from_str(body).map_err(|e| {
        // Category and position only: a serde message may quote the input.
        SpotOrderListError::Malformed(format!(
            "{:?} at line {} column {}",
            e.classify(),
            e.line(),
            e.column()
        ))
    })?;
    let members: usize = lists
        .iter()
        .try_fold(0usize, |sum, list| sum.checked_add(list.orders.len()))
        .ok_or(SpotOrderListError::Allocation)?;
    let mut list_ids = HashSet::new();
    list_ids.try_reserve(lists.len()).map_err(|_| SpotOrderListError::Allocation)?;
    let mut by_order_id: HashSet<(&str, u64)> = HashSet::new();
    by_order_id.try_reserve(members).map_err(|_| SpotOrderListError::Allocation)?;
    let mut by_client_id: HashSet<(&str, &str)> = HashSet::new();
    by_client_id.try_reserve(members).map_err(|_| SpotOrderListError::Allocation)?;
    for list in &lists {
        let list_id = list.order_list_id;
        if list_id < 0 {
            return Err(SpotOrderListError::NegativeListId(list_id));
        }
        if !list_ids.insert(list_id) {
            return Err(SpotOrderListError::DuplicateList(list_id));
        }
        if !valid_identity(&list.symbol) {
            return Err(SpotOrderListError::InvalidIdentity { list_id, field: "symbol" });
        }
        if list.list_client_order_id.as_deref().is_some_and(|id| !valid_identity(id)) {
            return Err(SpotOrderListError::InvalidIdentity { list_id, field: "listClientOrderId" });
        }
        if list.orders.is_empty() {
            return Err(SpotOrderListError::NoMembers(list_id));
        }
        for member in &list.orders {
            let field = if !valid_identity(&member.symbol) {
                Some("member symbol")
            } else if !valid_identity(&member.client_order_id) {
                Some("clientOrderId")
            } else {
                None
            };
            if let Some(field) = field {
                return Err(SpotOrderListError::InvalidIdentity { list_id, field });
            }
            if member.symbol != list.symbol {
                return Err(SpotOrderListError::SymbolDisagreement(list_id));
            }
            let order_id = member.order_id;
            let symbol = member.symbol.as_str();
            if !by_order_id.insert((symbol, order_id))
                || !by_client_id.insert((symbol, member.client_order_id.as_str()))
            {
                return Err(SpotOrderListError::DuplicateMember { list_id, order_id });
            }
        }
    }
    drop((by_order_id, by_client_id));
    Ok(lists)
}

#[cfg(any(test, feature = "crash-points"))]
#[derive(Deserialize)]
#[serde(transparent)]
struct OpenOrderEntries(#[serde(deserialize_with = "object_entries")] Vec<SpotOrderRecord>);

/// Why a strict inventory body was refused. No variant carries input text
/// beyond a validated ID.
#[cfg(any(test, feature = "crash-points"))]
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SpotInventoryError {
    #[error("inventory: {len} bytes exceed the {limit}-byte envelope")]
    Oversize { len: usize, limit: usize },
    /// Not one map of the documented shape (positional arrays included), a
    /// field missing, repeated or of another type, or trailing input.
    #[error("inventory: malformed ({0})")]
    Malformed(String),
    #[error("inventory: an asset is listed twice")]
    DuplicateAsset,
    #[error("inventory: order {0} has an invalid identity")]
    InvalidIdentity(u64),
    /// The same `(symbol, orderId)` or `(symbol, clientOrderId)` twice.
    #[error("inventory: order {0} repeats or conflicts")]
    DuplicateOrder(u64),
    #[error("inventory: allocation refused")]
    Allocation,
}

#[cfg(any(test, feature = "crash-points"))]
fn malformed(e: serde_json::Error) -> SpotInventoryError {
    // Category and position only: a serde message may quote the input.
    SpotInventoryError::Malformed(format!("{:?} at line {} column {}", e.classify(), e.line(), e.column()))
}

/// Decode a completed `GET /api/v3/account` body within `max_bytes`: one
/// map only (never a positional array), every asset once. Synchronous, no I/O.
#[cfg(any(test, feature = "crash-points"))]
pub fn decode_account(body: &str, max_bytes: usize) -> Result<SpotAccount, SpotInventoryError> {
    if body.len() > max_bytes {
        return Err(SpotInventoryError::Oversize { len: body.len(), limit: max_bytes });
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Account {
        uid: u64,
        can_trade: bool,
        account_type: String,
        permissions: Vec<String>,
        #[serde(deserialize_with = "object_entries")]
        balances: Vec<SpotAccountBalance>,
    }

    let Object(account): Object<Account> = serde_json::from_str(body).map_err(malformed)?;
    let mut assets = HashSet::new();
    assets.try_reserve(account.balances.len()).map_err(|_| SpotInventoryError::Allocation)?;
    if account.balances.iter().any(|b| !assets.insert(b.asset.as_str())) {
        return Err(SpotInventoryError::DuplicateAsset);
    }
    drop(assets);
    Ok(SpotAccount {
        uid: account.uid,
        can_trade: account.can_trade,
        account_type: account.account_type,
        permissions: account.permissions,
        balances: account.balances,
    })
}

/// Decode a completed `GET /api/v3/openOrders` body within `max_bytes`: an
/// array of maps, every row and raw symbol kept, all or nothing. Identity is
/// unique by `(symbol, orderId)` and by `(symbol, clientOrderId)`; list
/// references are not resolved here.
#[cfg(any(test, feature = "crash-points"))]
pub fn decode_open_orders(
    body: &str,
    max_bytes: usize,
) -> Result<Vec<SpotOrderRecord>, SpotInventoryError> {
    if body.len() > max_bytes {
        return Err(SpotInventoryError::Oversize { len: body.len(), limit: max_bytes });
    }
    let OpenOrderEntries(orders) = serde_json::from_str(body).map_err(malformed)?;
    let mut by_order_id: HashSet<(&str, u64)> = HashSet::new();
    by_order_id.try_reserve(orders.len()).map_err(|_| SpotInventoryError::Allocation)?;
    let mut by_client_id: HashSet<(&str, &str)> = HashSet::new();
    by_client_id.try_reserve(orders.len()).map_err(|_| SpotInventoryError::Allocation)?;
    for order in &orders {
        let symbol = order.symbol.as_str();
        if !valid_identity(symbol) || !valid_identity(&order.client_order_id) {
            return Err(SpotInventoryError::InvalidIdentity(order.order_id));
        }
        if !by_order_id.insert((symbol, order.order_id))
            || !by_client_id.insert((symbol, order.client_order_id.as_str()))
        {
            return Err(SpotInventoryError::DuplicateOrder(order.order_id));
        }
    }
    drop((by_order_id, by_client_id));
    Ok(orders)
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

/// The open order lists of a body the legacy transport already buffered
/// whole: its own length is the envelope, which bounds nothing it received.
pub(super) fn parse_open_order_lists(body: &str) -> Result<Vec<SpotOpenOrderList>, SpotReadError> {
    Ok(decode_open_order_lists(body, body.len())?)
}

#[cfg(test)]
pub(super) fn parse_order_list_count(body: &str) -> Result<usize, SpotReadError> {
    Ok(parse_open_order_lists(body)?.len())
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
