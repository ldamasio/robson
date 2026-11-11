//! Binance spot trade feed implementing `TradeFeedPort` (ADR-0056 step 8b).
//!
//! The spot counterpart of `usdm::market_data`: the stream base, the
//! `aggTrade` subscription and its wire decoding, through the shared
//! WebSocket client. The reconnect loop, the read-idle watchdog and the REST
//! fallback are venue neutral and stay in `robsond::market_data`; feed resync
//! is out of scope (design note v16 §0). No production path constructs
//! [`BinanceSpotTradeFeed`] yet (SD12; `inertness_tests.rs`).
//!
//! Venue facts, `web-socket-streams.md` at `995f6eb3` (F25):
//! - the base endpoint, port 9443 or 443 (L41); one base here, and no testnet
//!   (the spot testnet is out of scope, §0);
//! - raw streams at `/ws/<streamName>` (L43), symbols lowercase (L48), the
//!   stream name `<symbol>@aggTrade` (L274), so one connection carries one
//!   symbol's trades and no combined-stream wrapper (L44-L47);
//! - the payload (L279-L293) carries `M`, which the shared wire type ignores;
//! - a connection is valid for 24 hours (L49-L50) and a `serverShutdown` event
//!   precedes a shutdown (L51-L53, L85-L110). Both end the attempt, and the
//!   caller's reconnect loop opens the next: the close ends the stream, and the
//!   event, which the shared wire type does not decode, is a stream error;
//! - the server pings every 20 seconds and disconnects without a pong within a
//!   minute (L54-L57): the shared stream answers each ping with its payload
//!   while the caller reads.

use std::str::FromStr;

use async_trait::async_trait;
use robson_domain::{Price, Symbol};
use robson_exec::{TradeFeedMessage, TradeFeedPort, TradeStream};
use rust_decimal::Decimal;

use crate::binance_ws::{BinanceWebSocketClient, BinanceWsStream, WsMessage};

/// The spot market stream base (`web-socket-streams.md` L41).
pub const SPOT_MARKET_WS_ENDPOINT: &str = "wss://stream.binance.com:9443";

/// Spot `aggTrade` feed on [`SPOT_MARKET_WS_ENDPOINT`].
pub struct BinanceSpotTradeFeed {
    endpoints: Vec<String>,
}

impl BinanceSpotTradeFeed {
    /// The feed on the spot stream base. It takes no environment and no
    /// configured bases: there is no spot testnet here to select, and nothing
    /// to fall back to.
    pub fn new() -> Self {
        Self {
            endpoints: vec![SPOT_MARKET_WS_ENDPOINT.to_string()],
        }
    }
}

impl Default for BinanceSpotTradeFeed {
    fn default() -> Self {
        Self::new()
    }
}

struct BinanceSpotTradeStream {
    inner: BinanceWsStream,
}

/// One stream message as a feed message, mapped as the USD-M feed maps its
/// own: an `aggTrade` is a tick for the symbol it names, an undecodable one
/// is `Malformed`, anything else is `NonTrade`.
pub(super) fn decode_message(message: WsMessage) -> TradeFeedMessage {
    let WsMessage::AggTrade(trade) = message else {
        // The raw `aggTrade` stream carries nothing else this decodes.
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
impl TradeStream for BinanceSpotTradeStream {
    async fn next_message(&mut self) -> Option<Result<TradeFeedMessage, String>> {
        self.inner
            .next()
            .await
            .map(|message| message.map(decode_message).map_err(|error| error.to_string()))
    }
}

#[async_trait]
impl TradeFeedPort for BinanceSpotTradeFeed {
    fn endpoints(&self) -> &[String] {
        &self.endpoints
    }

    /// Opens `<endpoint>/ws/<symbol>@aggTrade`, the symbol lowercased.
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
        Ok(Box::new(BinanceSpotTradeStream { inner }))
    }
}
