//! Binance venue adapters (ADR-0056 §3).

mod endpoints;
pub mod spot;
pub mod usdm;
mod wire;

pub use endpoints::Endpoints;

#[cfg(test)]
mod quantize_golden_tests;
#[cfg(test)]
mod wire_golden_tests;
