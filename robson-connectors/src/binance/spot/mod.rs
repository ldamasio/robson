//! Binance spot (`binance:spot`), ADR-0056 step 8: the REST read side (8a),
//! the trade feed and the klines (8b), the rules projection, the rules cache
//! and the single trades page (8c-2a), the venue's read side (8c-2b), order
//! placement (8c-3a), and cancels and the composite wrappers (8c-3b).
//!
//! The REST read side reads a symbol's `exchangeInfo` rules, the account,
//! open orders and order lists, one order, `myTrades` pages and commission
//! rates, and parses each into a typed record; it implements no venue port.
//! The trade feed and the klines implement the existing `TradeFeedPort` and
//! `OhlcvPort`, as their USD-M counterparts do. [`BinanceSpotAdapter`] (8c-2b)
//! implements the venue ports over that reader and the rules cache. It places
//! MARKET and `STOP_LOSS` orders (8c-3a), each through `trading.rs` under the
//! one-attempt ledger and the operation gate, and cancels them there with no
//! grant (8c-3b). No production path constructs
//! [`SpotRestReader`], [`BinanceSpotTradeFeed`], [`BinanceSpotOhlcv`] or
//! [`BinanceSpotAdapter`] (SD12; `inertness_tests.rs`).
//!
//! For the production non-trading Spot start (PS-4): the closed set of
//! budgeted reads ([`SpotPreparedRead`], `reads.rs`) and its one-shot
//! transport (`read_send.rs`), and [`BinanceSpotAdapter::non_trading`], which
//! closes the shared client to every other request before wrapping it
//! (`BinanceRestClient::close_unbudgeted_sends`).
//!
//! Venue facts are from Binance's spot API documentation at commit
//! `995f6eb3` of `binance/binance-spot-api-docs`, read 2026-11-10,
//! 2026-11-11 and 2026-11-13 (design note v16 §2, F13–F17, F22, F23, F25).

mod cache;
mod commission_send;
mod costs;
mod exchange;
mod market_data;
mod ohlcv;
mod read_send;
mod reads;
mod records;
mod rest;
mod rules;
mod sizing_inputs;
mod time;
mod trades;
mod trading;

pub use commission_send::{PreparedSpotCommission, SignedSpotCommission};
#[cfg(any(test, feature = "crash-points"))]
pub(crate) use exchange::decode_fixture_order;
pub use exchange::{BinanceSpotAdapter, SPOT_FINALITY};
pub use market_data::{BinanceSpotTradeFeed, SPOT_MARKET_WS_ENDPOINT};
pub use ohlcv::BinanceSpotOhlcv;
pub use read_send::{PreparedSpotRead, SpotReadRequest};
pub use reads::{SpotPreparedRead, SpotReadBody, SPOT_COMMISSION_BODY_CAP, SPOT_OPERATED_SYMBOL};
#[cfg(any(test, feature = "crash-points"))]
pub use records::{decode_account, decode_open_orders};
pub use records::{
    decode_open_order_lists, SpotAccount, SpotAccountBalance, SpotCommission,
    SpotCommissionDiscount, SpotCommissionRates, SpotInventoryError, SpotOpenOrderList,
    SpotOrderListError, SpotOrderListMember, SpotOrderRecord,
};
pub use rest::{SpotOrderRef, SpotRead, SpotReadError, SpotRestReader};
pub use rules::{SpotLotFilter, SpotNotionalFilter, SpotPriceFilter, SpotSymbolRules};
pub use time::PreparedSpotTime;
pub use trades::{
    SpotTrade, SpotTradePage, SpotTradesQuery, SpotTradesRead, MY_TRADES_MAX_PAGES,
    MY_TRADES_PAGE_LIMIT,
};
pub use trading::{SpotWrite, SPOT_STP_MODE};

#[cfg(test)]
mod adapter_tests;
#[cfg(test)]
mod budgeted_read_tests;
#[cfg(test)]
mod cancel_tests;
#[cfg(test)]
mod conformance_tests;
#[cfg(test)]
mod exact_ingress_tests;
#[cfg(test)]
mod feed_tests;
#[cfg(test)]
mod inertness_tests;
#[cfg(test)]
mod ohlcv_tests;
#[cfg(test)]
mod parse_tests;
#[cfg(test)]
mod read_tests;
#[cfg(test)]
mod rules_tests;
#[cfg(test)]
mod stand_in;
#[cfg(test)]
mod trading_tests;
