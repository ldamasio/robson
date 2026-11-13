//! Binance spot klines implementing `OhlcvPort` (ADR-0056 step 8b).
//!
//! The spot counterpart of `usdm::ohlcv`: one public, unsigned
//! [`SpotRead::KLINES`] through the shared REST client, whose path rule sends
//! `/api/v3/` to the spot host; since 8c-3 on its spot read helper, values
//! encoded and one request with no redirect or retry. The spot response has the
//! futures layout at the indices the shared kline parser reads, so the rows go
//! through it, and errors map as `BinanceOhlcvAdapter` maps them. No production
//! path constructs [`BinanceSpotOhlcv`] yet (SD12; `inertness_tests.rs`).
//!
//! Venue facts, `rest-api.md` at `995f6eb3` (F25):
//! - `GET /api/v3/klines`, weight 2 (L1273-L1281), with no security type, so
//!   public and unsigned (L263-L268);
//! - `symbol` and `interval` mandatory, `limit` "Default: 500; Maximum: 1000."
//!   (L1285-L1292); the intervals (L1298-L1307);
//! - without `startTime` or `endTime`, the most recent klines (L1311), oldest
//!   first (L98-L99);
//! - each kline an array: open time, open, high, low, close, volume, close
//!   time, quote volume, trade count, taker volumes, ignore (L1319-L1337).
//!
//! Only the most recent `limit` klines: the kline by `startTime` at a month
//! boundary (design §C6) is 8p's.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use robson_domain::{Candle, Symbol};
use robson_exec::{CandleInterval, ExecError, OhlcvPort};

use super::rest::SpotRead;
use crate::{
    binance::wire,
    binance_rest::{parse_futures_klines, BinanceRestClient, BinanceRestError},
};

/// The most klines one request may ask for (L1292).
const KLINES_MAX_LIMIT: u16 = 1000;

/// Binance spot historical candles.
pub struct BinanceSpotOhlcv {
    client: Arc<BinanceRestClient>,
}

impl BinanceSpotOhlcv {
    /// Klines over `client`, the venue's shared REST client.
    ///
    /// A testnet client is refused, never served from mainnet: its path rule
    /// would send `/api/v3/` to the spot testnet, which step 8 does not
    /// support (design note v16 §0).
    pub fn new(client: Arc<BinanceRestClient>) -> Result<Self, ExecError> {
        if client.is_testnet() {
            return Err(ExecError::Config(
                "Binance spot klines need a mainnet client: the spot testnet is out of scope \
                 (ADR-0056 step 8)"
                    .to_string(),
            ));
        }
        Ok(Self { client })
    }

    /// As `BinanceOhlcvAdapter` maps its errors.
    fn map_error(error: BinanceRestError) -> ExecError {
        match &error {
            BinanceRestError::Timeout => {
                ExecError::Timeout("Binance OHLCV request timed out".to_string())
            },
            BinanceRestError::InvalidParameter(msg) => ExecError::Config(msg.clone()),
            _ => ExecError::Exchange(error.to_string()),
        }
    }

    fn timestamp_from_millis(ms: i64, field: &str) -> Result<DateTime<Utc>, ExecError> {
        DateTime::from_timestamp_millis(ms).ok_or_else(|| {
            ExecError::Exchange(format!("Invalid Binance kline {} timestamp: {}", field, ms))
        })
    }
}

#[async_trait]
impl OhlcvPort for BinanceSpotOhlcv {
    async fn fetch_candles(
        &self,
        symbol: &Symbol,
        interval: CandleInterval,
        limit: u16,
    ) -> Result<Vec<Candle>, ExecError> {
        // Refused before anything is sent, as the USD-M client refuses it.
        if limit == 0 || limit > KLINES_MAX_LIMIT {
            return Err(Self::map_error(BinanceRestError::InvalidParameter(format!(
                "kline limit must be between 1 and {KLINES_MAX_LIMIT}, got {limit}"
            ))));
        }

        let params = vec![
            ("symbol", symbol.as_pair()),
            ("interval", wire::candle_interval(interval).to_string()),
            ("limit", limit.to_string()),
        ];
        let klines = self
            .client
            .get_public_spot_answer(SpotRead::KLINES.path, params)
            .await
            .and_then(|answer| answer.into_body())
            .and_then(|body| parse_futures_klines(&body))
            .map_err(Self::map_error)?;

        klines
            .into_iter()
            .map(|kline| {
                let open_time = Self::timestamp_from_millis(kline.open_time_ms, "open")?;
                let close_time = Self::timestamp_from_millis(kline.close_time_ms, "close")?;

                Ok(Candle::new(
                    symbol.clone(),
                    kline.open,
                    kline.high,
                    kline.low,
                    kline.close,
                    kline.volume,
                    kline.trades,
                    open_time,
                    close_time,
                ))
            })
            .collect()
    }
}
