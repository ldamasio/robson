//! Spot REST reads over the shared Binance client (ADR-0056 step 8a).
//!
//! Every read is a GET. A signed one goes through the client's signing
//! (`build_signed_query`) and its one request bound, on the host its path
//! rule picks for `/api/v3/` (the spot host; `Endpoints::base_url` is not a
//! host table, ADR §4). None passes the mutation gate: none is a mutation.
//!
//! Errors stay raw and typed. `-2013`, `-2026`, `-1007`, a 5XX and a
//! client-side timeout are each their own variant, with the HTTP status kept,
//! because what each proves about an order is the trading surface's finality
//! rule (8c), not this module's.

use std::sync::Arc;

use robson_domain::Symbol;
use rust_decimal::Decimal;
use serde::{de::DeserializeOwned, Deserialize};
use thiserror::Error;

use super::{
    records::{self, SpotAccount, SpotCommission, SpotOrderRecord},
    rules::{self, SpotSymbolRules},
    trades::{
        self, SpotTradePage, SpotTradeSet, SpotTradesQuery, SpotTradesRead, MY_TRADES_MAX_PAGES,
    },
};
use crate::binance_rest::{BinanceRestClient, BinanceRestError};

/// One spot REST read: its path and Binance's documented request weight.
///
/// The weight is the figure the request limiter reserves before signing
/// (design note v8 §3.14), never a lower one read back from a response.
/// Line numbers are in `rest-api.md` at `995f6eb3`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpotRead {
    /// Path on the spot host.
    pub path: &'static str,
    /// Documented request weight.
    pub weight: u32,
    /// Whether the request is signed (`USER_DATA`).
    pub signed: bool,
}

impl SpotRead {
    /// `GET /api/v3/exchangeInfo`, weight 20, public (L884-L889).
    pub const EXCHANGE_INFO: Self = Self::public("/api/v3/exchangeInfo", 20);
    /// `GET /api/v3/account`, weight 20 (F23; L4278-L4283).
    pub const ACCOUNT: Self = Self::signed("/api/v3/account", 20);
    /// `GET /api/v3/openOrders` for one symbol, weight 6 (F23; L4411-L4416).
    pub const OPEN_ORDERS_SYMBOL: Self = Self::signed("/api/v3/openOrders", 6);
    /// `GET /api/v3/openOrders` for every symbol, weight 80 (F23;
    /// L4411-L4416).
    pub const OPEN_ORDERS_ALL: Self = Self::signed("/api/v3/openOrders", 80);
    /// `GET /api/v3/openOrderList`, weight 6 (F23; L4680-L4684).
    pub const OPEN_ORDER_LISTS: Self = Self::signed("/api/v3/openOrderList", 6);
    /// `GET /api/v3/order`, weight 4 (F13; L4341-L4346).
    pub const ORDER: Self = Self::signed("/api/v3/order", 4);
    /// `GET /api/v3/myTrades` without `orderId`, weight 20 (F16;
    /// L4728-L4736).
    pub const MY_TRADES: Self = Self::signed("/api/v3/myTrades", 20);
    /// `GET /api/v3/myTrades` with `orderId`, weight 5 (F16; L4728-L4736).
    pub const MY_TRADES_BY_ORDER: Self = Self::signed("/api/v3/myTrades", 5);
    /// `GET /api/v3/account/commission`, weight 20 (F17; L4954-L4959).
    pub const COMMISSION: Self = Self::signed("/api/v3/account/commission", 20);
    /// `GET /api/v3/klines`, weight 2, public (F25; L1273-L1281). Sent by
    /// `BinanceSpotOhlcv` (8b), not by this reader.
    pub const KLINES: Self = Self::public("/api/v3/klines", 2);
    /// `GET /api/v3/ping`, weight 1, public (L836-L852).
    pub const PING: Self = Self::public("/api/v3/ping", 1);
    /// `GET /api/v3/ticker/price` for one symbol, weight 2, public
    /// (L1750-L1782).
    pub const TICKER_PRICE: Self = Self::public("/api/v3/ticker/price", 2);

    const fn public(path: &'static str, weight: u32) -> Self {
        Self { path, weight, signed: false }
    }

    const fn signed(path: &'static str, weight: u32) -> Self {
        Self { path, weight, signed: true }
    }
}

/// Which order a lookup asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpotOrderRef {
    /// The venue's `orderId`.
    OrderId(u64),
    /// `origClientOrderId`. A cancel's own client id replaces the canceled
    /// order's (F12), so the record may carry a different `clientOrderId`
    /// than the one asked for; it is returned as the venue gives it.
    ClientOrderId(String),
}

/// Why a spot read failed, raw and typed.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SpotReadError {
    /// `-2013 NO_SUCH_ORDER`, "Order does not exist." (errors.md L435). A raw
    /// not-found, never `Absent` by itself.
    #[error("Binance -2013 (HTTP {http_status}): {msg}")]
    NoSuchOrder { http_status: u16, msg: String },
    /// `-2026 ORDER_ARCHIVED`: canceled or expired with no executed quantity
    /// over 90 days ago (F14; errors.md L451).
    #[error("Binance -2026 (HTTP {http_status}): {msg}")]
    OrderArchived { http_status: u16, msg: String },
    /// `-1007 TIMEOUT`, the venue's backend timed out: "Send status unknown;
    /// execution status unknown." (F22; errors.md L39).
    #[error("Binance -1007 (HTTP {http_status}): {msg}")]
    BackendTimeout { http_status: u16, msg: String },
    /// HTTP 5XX: "the execution status is **UNKNOWN**" (F22). `code` when the
    /// body was Binance's JSON error, the raw body as `msg` otherwise.
    #[error("Binance HTTP {http_status} (code {code:?}): {msg}")]
    ServerError {
        http_status: u16,
        code: Option<i64>,
        msg: String,
    },
    /// Any other Binance JSON error, with the status that carried it (a 429
    /// or 418 among them; the limiter reads those in 8h).
    #[error("Binance {code} (HTTP {http_status}): {msg}")]
    Api {
        http_status: u16,
        code: i64,
        msg: String,
    },
    /// A non-success status below 500 whose body is not Binance's JSON error
    /// (a WAF 403, for one).
    #[error("Binance HTTP {http_status}: {body}")]
    Http { http_status: u16, body: String },
    /// No complete answer within the client's request bound. Uncertain: the
    /// request may have reached the venue.
    #[error("Binance request timed out")]
    Timeout,
    /// No answer: sending failed, or the answer's body could not be read.
    #[error("Binance request failed: {0}")]
    Transport(String),
    /// The request could not be signed; nothing was sent.
    #[error("Binance request not signed: {0}")]
    Signature(String),
    /// A success body that is not the documented response.
    #[error("Unparseable spot answer: {0}")]
    Parse(String),
    /// A well-formed answer that contradicts the request or itself.
    #[error("Inconsistent spot answer: {0}")]
    Inconsistent(String),
    /// Rules that parsed but give no usable market lot.
    #[error("Unusable spot rules: {0}")]
    InvalidRules(String),
}

/// `GET /api/v3/ticker/price` for one symbol (L1776-L1782).
#[derive(Deserialize)]
struct TickerPrice {
    symbol: String,
    price: Decimal,
}

/// Binance's error body: `{"code": -1121, "msg": "Invalid symbol."}`.
#[derive(Deserialize)]
struct ErrorBody {
    code: i64,
    msg: String,
}

impl SpotReadError {
    /// Type a Binance JSON error. The code decides first, so `-2013`, `-2026`
    /// and `-1007` stay themselves whatever status carried them; then a 5XX.
    fn from_api(http_status: u16, code: i64, msg: String) -> Self {
        match code {
            -2013 => Self::NoSuchOrder { http_status, msg },
            -2026 => Self::OrderArchived { http_status, msg },
            -1007 => Self::BackendTimeout { http_status, msg },
            _ if http_status >= 500 => Self::ServerError { http_status, code: Some(code), msg },
            _ => Self::Api { http_status, code, msg },
        }
    }

    /// Type a non-success answer (a read's, or an order's: 8c-3).
    pub(super) fn from_answer(http_status: u16, body: String) -> Self {
        match serde_json::from_str::<ErrorBody>(&body) {
            Ok(ErrorBody { code, msg }) => Self::from_api(http_status, code, msg),
            Err(_) if http_status >= 500 => {
                Self::ServerError { http_status, code: None, msg: body }
            },
            Err(_) => Self::Http { http_status, body },
        }
    }
}

impl From<BinanceRestError> for SpotReadError {
    fn from(error: BinanceRestError) -> Self {
        match error {
            BinanceRestError::Timeout => Self::Timeout,
            BinanceRestError::SignatureError(msg) => Self::Signature(msg),
            // Sending failed, or the body could not be read (`answer`).
            BinanceRestError::RequestFailed(msg) | BinanceRestError::ParseError(msg) => {
                Self::Transport(msg)
            },
            BinanceRestError::ApiError { code, msg, http_status } => {
                Self::from_api(http_status, code, msg)
            },
            // Not produced by a GET; kept apart from a venue answer.
            BinanceRestError::Refused(_)
            | BinanceRestError::InvalidParameter(_)
            | BinanceRestError::PositionNotFound(_) => Self::Transport(error.to_string()),
        }
    }
}

/// Parse a success body as `T`, naming the read when it does not fit.
pub(super) fn parse_json<T: DeserializeOwned>(what: &str, body: &str) -> Result<T, SpotReadError> {
    serde_json::from_str(body).map_err(|e| SpotReadError::Parse(format!("{what}: {e}")))
}

/// The spot REST read side: typed reads, no mutation, no port.
pub struct SpotRestReader {
    client: Arc<BinanceRestClient>,
    /// Every read sent, in order, for the weight golden test.
    #[cfg(test)]
    sent: std::sync::Mutex<Vec<SpotRead>>,
}

impl SpotRestReader {
    pub fn new(client: Arc<BinanceRestClient>) -> Self {
        Self {
            client,
            #[cfg(test)]
            sent: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// Reads sent so far, oldest first.
    #[cfg(test)]
    pub(super) fn sent(&self) -> Vec<SpotRead> {
        self.sent.lock().unwrap().clone()
    }

    /// Send one read and return its body when the venue answered with
    /// success.
    async fn read(
        &self,
        read: SpotRead,
        params: Vec<(&str, String)>,
    ) -> Result<String, SpotReadError> {
        #[cfg(test)]
        self.sent.lock().unwrap().push(read);
        let answer = if read.signed {
            self.client.get_signed_answer(read.path, params).await?
        } else {
            self.client.get_public_answer(read.path, params).await?
        };
        if answer.status.is_success() {
            Ok(answer.body)
        } else {
            Err(SpotReadError::from_answer(answer.status.as_u16(), answer.body))
        }
    }

    /// The `exchangeInfo` rules of `symbol` ([`SpotRead::EXCHANGE_INFO`]).
    pub async fn symbol_rules(&self, symbol: &Symbol) -> Result<SpotSymbolRules, SpotReadError> {
        let pair = symbol.as_pair();
        let body = self.read(SpotRead::EXCHANGE_INFO, vec![("symbol", pair.clone())]).await?;
        rules::parse_symbol_rules(&body, &pair)
    }

    /// The account: balances, `uid`, `canTrade`, `accountType` and
    /// `permissions` ([`SpotRead::ACCOUNT`]).
    pub async fn account(&self) -> Result<SpotAccount, SpotReadError> {
        let body = self.read(SpotRead::ACCOUNT, vec![]).await?;
        records::parse_account(&body)
    }

    /// Every open order on `symbol`, full records
    /// ([`SpotRead::OPEN_ORDERS_SYMBOL`]).
    pub async fn open_orders(
        &self,
        symbol: &Symbol,
    ) -> Result<Vec<SpotOrderRecord>, SpotReadError> {
        let pair = symbol.as_pair();
        let body = self.read(SpotRead::OPEN_ORDERS_SYMBOL, vec![("symbol", pair.clone())]).await?;
        let orders = records::parse_orders(&body)?;
        if let Some(other) = orders.iter().find(|order| order.symbol != pair) {
            return Err(SpotReadError::Inconsistent(format!(
                "open orders for {pair} include order {} on {}",
                other.order_id, other.symbol
            )));
        }
        Ok(orders)
    }

    /// Every open order on the account, every symbol, full records
    /// ([`SpotRead::OPEN_ORDERS_ALL`]).
    pub async fn all_open_orders(&self) -> Result<Vec<SpotOrderRecord>, SpotReadError> {
        let body = self.read(SpotRead::OPEN_ORDERS_ALL, vec![]).await?;
        records::parse_orders(&body)
    }

    /// How many order lists are open on the account
    /// ([`SpotRead::OPEN_ORDER_LISTS`]). Robson places none, so only the
    /// count is read.
    pub async fn open_order_list_count(&self) -> Result<usize, SpotReadError> {
        let body = self.read(SpotRead::OPEN_ORDER_LISTS, vec![]).await?;
        records::parse_order_list_count(&body)
    }

    /// One order's full record ([`SpotRead::ORDER`]). A record on another
    /// symbol, or with another `orderId` than the one asked for, is
    /// [`SpotReadError::Inconsistent`].
    pub async fn order(
        &self,
        symbol: &Symbol,
        by: &SpotOrderRef,
    ) -> Result<SpotOrderRecord, SpotReadError> {
        let pair = symbol.as_pair();
        let id = match by {
            SpotOrderRef::OrderId(order_id) => ("orderId", order_id.to_string()),
            SpotOrderRef::ClientOrderId(client_id) => ("origClientOrderId", client_id.clone()),
        };
        let body = self.read(SpotRead::ORDER, vec![("symbol", pair.clone()), id]).await?;
        let order = records::parse_order(&body)?;
        if order.symbol != pair {
            return Err(SpotReadError::Inconsistent(format!(
                "order lookup on {pair} answered order {} on {}",
                order.order_id, order.symbol
            )));
        }
        if let SpotOrderRef::OrderId(order_id) = by {
            if order.order_id != *order_id {
                return Err(SpotReadError::Inconsistent(format!(
                    "order lookup for {order_id} answered order {}",
                    order.order_id
                )));
            }
        }
        Ok(order)
    }

    /// Trades on `symbol` for `query`, at most [`MY_TRADES_MAX_PAGES`] pages
    /// ([`SpotRead::MY_TRADES_BY_ORDER`] or [`SpotRead::MY_TRADES`] each).
    /// Evidence only, never finality: see [`SpotTradesRead`].
    pub async fn trades(
        &self,
        symbol: &Symbol,
        query: SpotTradesQuery,
    ) -> Result<SpotTradesRead, SpotReadError> {
        let pair = symbol.as_pair();
        let mut collected = SpotTradeSet::new(pair.clone());
        let mut page = query;
        let (mut pages, mut weight) = (0, 0);
        loop {
            let (read, params) = page.page_request(&pair);
            let body = self.read(read, params).await?;
            pages += 1;
            weight += read.weight;
            match collected.absorb(page, &body)? {
                Some(next) if pages < MY_TRADES_MAX_PAGES => page = next,
                next => return Ok(collected.finish(next, pages, weight)),
            }
        }
    }

    /// One page of `order_id`'s trades on `symbol`, ids from `from_id` on
    /// ([`SpotRead::MY_TRADES_BY_ORDER`]): one request, never a retry. The
    /// page's checks are [`trades::parse_page`]'s.
    pub async fn trades_page(
        &self,
        symbol: &Symbol,
        order_id: u64,
        from_id: u64,
    ) -> Result<SpotTradePage, SpotReadError> {
        let pair = symbol.as_pair();
        let (read, params) = SpotTradesQuery::Order { order_id, from_id }.page_request(&pair);
        let body = self.read(read, params).await?;
        trades::parse_page(&pair, order_id, from_id, &body)
    }

    /// Whether the REST API answers ([`SpotRead::PING`]): `{}`.
    pub async fn ping(&self) -> Result<(), SpotReadError> {
        let body = self.read(SpotRead::PING, vec![]).await?;
        let _: serde_json::Map<String, serde_json::Value> = parse_json("ping", &body)?;
        Ok(())
    }

    /// The latest price of `symbol` ([`SpotRead::TICKER_PRICE`]): on the
    /// symbol asked for, and positive.
    pub async fn ticker_price(&self, symbol: &Symbol) -> Result<Decimal, SpotReadError> {
        let pair = symbol.as_pair();
        let body = self.read(SpotRead::TICKER_PRICE, vec![("symbol", pair.clone())]).await?;
        let ticker: TickerPrice = parse_json("ticker price", &body)?;
        if ticker.symbol != pair {
            return Err(SpotReadError::Inconsistent(format!(
                "ticker price for {pair} answered {}",
                ticker.symbol
            )));
        }
        if ticker.price <= Decimal::ZERO {
            return Err(SpotReadError::Inconsistent(format!(
                "ticker price {} for {pair} is not positive",
                ticker.price
            )));
        }
        Ok(ticker.price)
    }

    /// The account's commission rates on `symbol` ([`SpotRead::COMMISSION`]).
    /// Parsed only: the cost envelope that uses them is 8g's.
    pub async fn commission(&self, symbol: &Symbol) -> Result<SpotCommission, SpotReadError> {
        let pair = symbol.as_pair();
        let body = self.read(SpotRead::COMMISSION, vec![("symbol", pair.clone())]).await?;
        records::parse_commission(&body, &pair)
    }
}
