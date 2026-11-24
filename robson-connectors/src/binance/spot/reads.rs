//! The closed set of budgeted Spot reads (PS, production Spot start; PS-4).
//!
//! Each read has a fixed path, fixed parameters, Binance's documented weight
//! ([`SpotRead`], which the request limiter reserves before anything is
//! prepared) and a production body cap. A kline's start time is the only
//! value a caller chooses. Open orders, rules and klines are read for the
//! operated pair only (8k-1 transport B: ACCOUNT 20 + `openOrders` for
//! BTCUSDT 6 + `openOrderList` 6 = 32 per attempt). A signed read takes the
//! owner's timestamp; its transport is `read_send.rs`.
//!
//! The caps (design note v16 and 8h name none) hold a plausible full answer
//! with the headroom `each_cap_holds_a_plausible_full_answer` measures:
//! about 4.7x for an account of 3,000 assets, 5x for 500 order lists, 25x
//! and more for the rules, one kline and the commission, but only about 1.7x
//! for 200 open orders (the venue's per-pair maximum) of the maximal
//! documented shape (2.6x for plain LIMIT orders). A body over its cap fails
//! the read; it is never read as an empty account.

use super::{
    records::{self, SpotAccount, SpotInventoryError, SpotOpenOrderList, SpotOrderRecord},
    rest::SpotRead,
    rules::{self, SpotSymbolRules},
};

/// The pair a scoped read names: the operated pair, a fixed constant.
pub const SPOT_OPERATED_SYMBOL: &str = "BTCUSDT";

/// Body cap of the commission read for the operated pair, whose prepared
/// transport is the existing `PreparedSpotCommission`.
pub const SPOT_COMMISSION_BODY_CAP: usize = 16 << 10;

/// One read of the closed set. Nothing else can be prepared through it: no
/// caller path, symbol, parameter or weight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpotPreparedRead {
    /// `GET /api/v3/account`: signed, weight 20, cap 1 MiB.
    Account,
    /// `GET /api/v3/openOrders?symbol=BTCUSDT`: signed, weight 6, cap 256 KiB.
    OperatedOpenOrders,
    /// `GET /api/v3/openOrderList`: signed, weight 6, cap 1 MiB.
    OpenOrderLists,
    /// `GET /api/v3/exchangeInfo?symbol=BTCUSDT`: public, weight 20, cap
    /// 256 KiB.
    ExchangeInfo,
    /// `GET /api/v3/klines?symbol=BTCUSDT&interval=1m&startTime=<start_ms>&
    /// limit=1`: public, weight 2, cap 16 KiB. The start is milliseconds
    /// since the epoch, never negative.
    Kline { start_ms: u64 },
}

/// One complete body decoded by the read that fetched it, all or nothing.
/// Descriptive only: nothing here carries authority.
#[derive(Debug)]
pub enum SpotReadBody {
    Account(SpotAccount),
    /// Every open order of the operated pair; another pair's refuses the body.
    OpenOrders(Vec<SpotOrderRecord>),
    OpenOrderLists(Vec<SpotOpenOrderList>),
    /// The operated pair's `exchangeInfo` rules, as projected for the reader
    /// (boxed: the projection is large beside the other variants).
    Rules(Box<SpotSymbolRules>),
    /// The kline rows as received: their parse and validation are 8p-4's.
    Kline(String),
}

impl SpotPreparedRead {
    /// The read's request: its path, Binance's documented weight (never one
    /// read back from a response) and whether it is signed (`USER_DATA`,
    /// with the owner's timestamp).
    pub fn request(self) -> SpotRead {
        match self {
            Self::Account => SpotRead::ACCOUNT,
            Self::OperatedOpenOrders => SpotRead::OPEN_ORDERS_SYMBOL,
            Self::OpenOrderLists => SpotRead::OPEN_ORDER_LISTS,
            Self::ExchangeInfo => SpotRead::EXCHANGE_INFO,
            Self::Kline { .. } => SpotRead::KLINES,
        }
    }

    /// The most body bytes the read accepts.
    pub fn body_cap(self) -> usize {
        match self {
            Self::Account | Self::OpenOrderLists => 1 << 20,
            Self::OperatedOpenOrders | Self::ExchangeInfo => 256 << 10,
            Self::Kline { .. } => 16 << 10,
        }
    }

    /// The read's fixed parameters, before a signed read's timestamp.
    pub fn query(self) -> Option<String> {
        let pair = format!("symbol={SPOT_OPERATED_SYMBOL}");
        match self {
            Self::Account | Self::OpenOrderLists => None,
            Self::OperatedOpenOrders | Self::ExchangeInfo => Some(pair),
            Self::Kline { start_ms } => {
                Some(format!("{pair}&interval=1m&startTime={start_ms}&limit=1"))
            },
        }
    }

    /// Strict decode of one complete body within this read's cap, checked
    /// before any parse. Synchronous, no I/O; a refusal carries no input text.
    pub fn decode(self, body: &[u8]) -> Result<SpotReadBody, SpotInventoryError> {
        let limit = self.body_cap();
        if body.len() > limit {
            return Err(SpotInventoryError::Oversize { len: body.len(), limit });
        }
        let text = std::str::from_utf8(body)
            .map_err(|_| SpotInventoryError::Malformed("not UTF-8".into()))?;
        Ok(match self {
            Self::Account => SpotReadBody::Account(records::decode_account(text, limit)?),
            Self::OperatedOpenOrders => match records::decode_open_orders(text, limit)? {
                orders if orders.iter().all(|order| order.symbol == SPOT_OPERATED_SYMBOL) => {
                    SpotReadBody::OpenOrders(orders)
                },
                _ => return Err(SpotInventoryError::OutsideScope),
            },
            Self::OpenOrderLists => SpotReadBody::OpenOrderLists(
                records::decode_open_order_lists(text, limit)
                    .map_err(|error| SpotInventoryError::Malformed(error.to_string()))?,
            ),
            Self::ExchangeInfo => SpotReadBody::Rules(Box::new(
                rules::parse_symbol_rules(text, SPOT_OPERATED_SYMBOL)
                    .map_err(|_| SpotInventoryError::Malformed("exchangeInfo rules".into()))?,
            )),
            Self::Kline { .. } => SpotReadBody::Kline(text.to_owned()),
        })
    }
}
