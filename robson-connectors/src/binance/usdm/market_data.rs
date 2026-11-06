//! Binance USDⓈ-M trade feed implementing `TradeFeedPort`.
//!
//! The futures WebSocket wiring relocated from `robsond` (ADR-0056 §3): stream
//! bases, the `aggTrade` subscription and its wire decoding. The reconnect
//! loop, the read-idle watchdog and the REST fallback are venue neutral and
//! stay in `robsond::market_data`.

use std::str::FromStr;

use async_trait::async_trait;
use robson_domain::{Price, Symbol};
use robson_exec::{TradeFeedMessage, TradeFeedPort, TradeStream};
use rust_decimal::Decimal;

use crate::binance_ws::{BinanceWebSocketClient, BinanceWsStream, WsMessage};

/// Default mainnet base for regular futures market streams. Binance routes
/// `aggTrade` under `/market`; the legacy unrouted path no longer delivers it.
pub const MAINNET_MARKET_WS_ENDPOINT: &str = "wss://fstream.binance.com/market";

/// Testnet retains the legacy unrouted stream base.
pub const TESTNET_MARKET_WS_ENDPOINT: &str = "wss://stream.binancefuture.com";

/// Futures `aggTrade` feed over an ordered list of stream bases.
pub struct BinanceUsdmTradeFeed {
    endpoints: Vec<String>,
}

impl BinanceUsdmTradeFeed {
    /// `configured_endpoints` wins when not empty; otherwise the built-in base
    /// for the environment.
    pub fn new(use_testnet: bool, configured_endpoints: Vec<String>) -> Self {
        let endpoints = if configured_endpoints.is_empty() {
            vec![if use_testnet {
                TESTNET_MARKET_WS_ENDPOINT.to_string()
            } else {
                MAINNET_MARKET_WS_ENDPOINT.to_string()
            }]
        } else {
            configured_endpoints
        };

        Self { endpoints }
    }
}

struct BinanceUsdmTradeStream {
    inner: BinanceWsStream,
}

fn decode_message(message: WsMessage) -> TradeFeedMessage {
    let WsMessage::AggTrade(trade) = message else {
        // Other message types are not used by this dedicated aggTrade
        // subscription.
        return TradeFeedMessage::NonTrade;
    };

    let price_decimal = match Decimal::from_str(&trade.price) {
        Ok(d) => d,
        Err(e) => {
            return TradeFeedMessage::Malformed {
                reason: "Failed to parse price",
                value: trade.price,
                error: e.to_string(),
            };
        },
    };

    let price = match Price::new(price_decimal) {
        Ok(p) => p,
        Err(e) => {
            return TradeFeedMessage::Malformed {
                reason: "Invalid price value",
                value: trade.price,
                error: e.to_string(),
            };
        },
    };

    match Symbol::from_pair(&trade.symbol) {
        Ok(symbol) => TradeFeedMessage::Trade { symbol, price },
        Err(e) => TradeFeedMessage::Malformed {
            reason: "Failed to parse symbol",
            value: trade.symbol,
            error: e.to_string(),
        },
    }
}

#[async_trait]
impl TradeStream for BinanceUsdmTradeStream {
    async fn next_message(&mut self) -> Option<Result<TradeFeedMessage, String>> {
        self.inner
            .next()
            .await
            .map(|message| message.map(decode_message).map_err(|error| error.to_string()))
    }
}

#[async_trait]
impl TradeFeedPort for BinanceUsdmTradeFeed {
    fn endpoints(&self) -> &[String] {
        &self.endpoints
    }

    async fn connect(
        &self,
        endpoint: &str,
        symbol: &Symbol,
    ) -> Result<Box<dyn TradeStream>, String> {
        let client = BinanceWebSocketClient::from_base_url(endpoint.to_string());
        let inner = client
            .subscribe_agg_trade(&symbol.as_pair())
            .await
            .map_err(|error| error.to_string())?;
        Ok(Box::new(BinanceUsdmTradeStream { inner }))
    }
}

#[cfg(test)]
mod tests {
    use rust_decimal_macros::dec;

    use super::*;

    fn agg_trade(symbol: &str, price: &str) -> WsMessage {
        serde_json::from_value(serde_json::json!({
            "e": "aggTrade",
            "E": 1_700_000_000_000u64,
            "s": symbol,
            "a": 1,
            "p": price,
            "q": "0.001",
            "f": 1,
            "l": 1,
            "T": 1_700_000_000_000u64,
            "m": false,
        }))
        .expect("aggTrade fixture")
    }

    #[test]
    fn endpoints_default_per_environment_and_configured_ones_win() {
        let mainnet = BinanceUsdmTradeFeed::new(false, vec![]);
        assert_eq!(mainnet.endpoints(), [MAINNET_MARKET_WS_ENDPOINT]);

        let testnet = BinanceUsdmTradeFeed::new(true, vec![]);
        assert_eq!(testnet.endpoints(), [TESTNET_MARKET_WS_ENDPOINT]);

        let configured = vec![
            "wss://primary.example/market".to_string(),
            "wss://fallback.example/market".to_string(),
        ];
        let feed = BinanceUsdmTradeFeed::new(true, configured.clone());
        assert_eq!(feed.endpoints(), configured.as_slice());
    }

    #[test]
    fn a_valid_agg_trade_is_a_tick_for_the_symbol_on_the_message() {
        assert_eq!(decode_message(agg_trade("ETHUSDT", "2500.5")), TradeFeedMessage::Trade {
            symbol: Symbol::from_pair("ETHUSDT").unwrap(),
            price: Price::new(dec!(2500.5)).unwrap(),
        });
    }

    #[test]
    fn undecodable_agg_trades_are_delivered_but_never_ticks() {
        for (trade, expected_reason, expected_value) in [
            (agg_trade("BTCUSDT", "not-a-number"), "Failed to parse price", "not-a-number"),
            (agg_trade("BTCUSDT", "-1"), "Invalid price value", "-1"),
            (agg_trade("???", "95000"), "Failed to parse symbol", "???"),
        ] {
            match decode_message(trade) {
                TradeFeedMessage::Malformed { reason, value, .. } => {
                    assert_eq!(reason, expected_reason);
                    assert_eq!(value, expected_value);
                },
                other => panic!("expected Malformed, got {other:?}"),
            }
        }
    }

    #[test]
    fn other_stream_messages_are_delivered_but_never_ticks() {
        let ticker: WsMessage = serde_json::from_value(serde_json::json!({
            "e": "24hrTicker",
            "E": 1_700_000_000_000u64,
            "s": "BTCUSDT",
            "p": "10",
            "P": "0.01",
            "c": "95000",
            "Q": "0.001",
            "o": "94990",
            "h": "96000",
            "l": "94000",
            "v": "100",
            "q": "9500000",
        }))
        .expect("ticker fixture");
        assert_eq!(decode_message(ticker), TradeFeedMessage::NonTrade);
    }
}
