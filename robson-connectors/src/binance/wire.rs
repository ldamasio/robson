//! Binance wire encodings of port-level enums: adapter knowledge, not port
//! contract (ADR-0056 §4).

use robson_exec::{CandleInterval, UniversalTransferType};

/// Binance REST API interval string for this interval.
pub(crate) fn candle_interval(interval: CandleInterval) -> &'static str {
    match interval {
        CandleInterval::OneMinute => "1m",
        CandleInterval::FiveMinutes => "5m",
        CandleInterval::FifteenMinutes => "15m",
        CandleInterval::OneHour => "1h",
        CandleInterval::FourHours => "4h",
        CandleInterval::OneDay => "1d",
    }
}

/// Binance universal transfer `type` parameter for this transfer.
pub(crate) fn universal_transfer_type(transfer_type: UniversalTransferType) -> &'static str {
    match transfer_type {
        UniversalTransferType::MainUmfuture => "MAIN_UMFUTURE",
    }
}
