//! Binance venue adapters (ADR-0056 §3).

mod endpoints;
pub mod usdm;

pub use endpoints::Endpoints;

#[cfg(test)]
mod wire_golden_tests;
