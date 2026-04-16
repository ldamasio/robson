//! Binance OHLCV adapter implementing `OhlcvPort`.
//!
//! The detector uses this adapter to fetch chart history before invoking the
//! pure `TechnicalStopAnalyzer`.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use robson_connectors::{BinanceRestClient, BinanceRestError};
use robson_domain::{Candle, Symbol};
use robson_exec::{CandleInterval, ExecError, OhlcvPort};

/// Binance adapter for historical candlestick data.
pub struct BinanceOhlcvAdapter {
    client: Arc<BinanceRestClient>,
}

impl BinanceOhlcvAdapter {
    /// Create a new adapter wrapping a Binance REST client.
    pub fn new(client: Arc<BinanceRestClient>) -> Self {
        Self { client }
    }

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
impl OhlcvPort for BinanceOhlcvAdapter {
    async fn fetch_candles(
        &self,
        symbol: &Symbol,
        interval: CandleInterval,
        limit: u16,
    ) -> Result<Vec<Candle>, ExecError> {
        let klines = self
            .client
            .get_futures_klines(&symbol.as_pair(), interval.as_binance_str(), limit)
            .await
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
