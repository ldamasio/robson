//! Binance spot (`binance:spot`), ADR-0056 step 8: the REST read side (8a).
//!
//! Read-only. It reads a symbol's `exchangeInfo` rules, the account, open
//! orders and order lists, one order, `myTrades` pages and commission rates,
//! and parses each into a typed record. Nothing here places, cancels or
//! changes anything on the venue, and nothing implements a venue port: the
//! trading surface (8c) maps these records and errors onto the port
//! contracts, and decides what a not-found means. No production path
//! constructs [`SpotRestReader`] yet (SD12; `inertness_tests.rs`).
//!
//! Venue facts are from Binance's spot API documentation at commit
//! `995f6eb3` of `binance/binance-spot-api-docs`, read 2026-11-10 (design
//! note v8 §2, F13–F17, F22, F23).

mod records;
mod rest;
mod rules;
mod trades;

pub use records::{
    SpotAccount, SpotAccountBalance, SpotCommission, SpotCommissionDiscount, SpotCommissionRates,
    SpotOrderRecord,
};
pub use rest::{SpotOrderRef, SpotRead, SpotReadError, SpotRestReader};
pub use rules::{SpotLotFilter, SpotNotionalFilter, SpotPriceFilter, SpotSymbolRules};
pub use trades::{
    SpotTrade, SpotTradesQuery, SpotTradesRead, MY_TRADES_MAX_PAGES, MY_TRADES_PAGE_LIMIT,
};

#[cfg(test)]
mod inertness_tests;
#[cfg(test)]
mod parse_tests;
#[cfg(test)]
mod read_tests;
