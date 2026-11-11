//! ADR-0056 step 8b: the spot trade feed.
//!
//! The URL is the one the shared WebSocket client composes for the spot base.
//! Stream behaviour is observed against a local WebSocket server standing in
//! for Binance, through the real client; the USD-M feed, on the same frames,
//! is the reference for how each message maps. Nothing reaches the network.

use futures_util::{SinkExt, StreamExt};
use robson_domain::{Price, Symbol};
use robson_exec::{TradeFeedMessage, TradeFeedPort};
use rust_decimal_macros::dec;
use serde_json::{json, Value};
use tokio::{
    net::TcpListener,
    task::JoinHandle,
    time::{timeout, Duration},
};
use tokio_tungstenite::{
    accept_hdr_async,
    tungstenite::{
        handshake::server::{ErrorResponse, Request, Response},
        Message,
    },
};

use super::{market_data::decode_message, BinanceSpotTradeFeed, SPOT_MARKET_WS_ENDPOINT};
use crate::{
    binance::usdm::BinanceUsdmTradeFeed,
    binance_ws::{BinanceWebSocketClient, WsMessage},
};

const AGG_TRADE: &str = include_str!("testdata/agg_trade.json");
const SERVER_SHUTDOWN: &str = include_str!("testdata/server_shutdown.json");

fn symbol(pair: &str) -> Symbol {
    Symbol::from_pair(pair).unwrap()
}

/// The documented payload with another symbol and price, `M` kept.
fn agg_trade(pair: &str, price: &str) -> String {
    let mut trade: Value = serde_json::from_str(AGG_TRADE).unwrap();
    trade["s"] = json!(pair);
    trade["p"] = json!(price);
    trade.to_string()
}

/// A message that decodes and is not a trade, as in the USD-M feed's tests.
fn ticker() -> String {
    json!({
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
    })
    .to_string()
}

fn decoded(frame: &str) -> TradeFeedMessage {
    decode_message(serde_json::from_str::<WsMessage>(frame).unwrap())
}

/// A stand-in stream server for one connection: it sends `frames`, closes,
/// and returns the request target the client asked for.
async fn playing(frames: Vec<String>) -> (String, JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("ws://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        let mut target = String::new();
        let mut ws = accept_hdr_async(
            tcp,
            |request: &Request, response: Response| -> Result<Response, ErrorResponse> {
                target = request.uri().to_string();
                Ok(response)
            },
        )
        .await
        .unwrap();
        for frame in frames {
            ws.send(Message::Text(frame)).await.unwrap();
        }
        let _ = ws.close(None).await;
        target
    });
    (endpoint, server)
}

/// Every message the stream delivers on `endpoint` until it ends.
async fn delivered(
    feed: &dyn TradeFeedPort,
    endpoint: &str,
) -> Vec<Result<TradeFeedMessage, String>> {
    let mut stream = feed.connect(endpoint, &symbol("BTCUSDT")).await.unwrap();
    let mut messages = Vec::new();
    while let Some(message) = timeout(Duration::from_secs(5), stream.next_message()).await.unwrap()
    {
        messages.push(message);
    }
    messages
}

/// F25: one base, the spot one, and no other to fall back to; the shared
/// client composes a valid raw-stream URL on it (`web-socket-streams.md` L41,
/// L43, L48, L274).
#[test]
fn the_feed_opens_the_raw_agg_trade_stream_on_the_spot_base() {
    assert_eq!(SPOT_MARKET_WS_ENDPOINT, "wss://stream.binance.com:9443");
    let feed = BinanceSpotTradeFeed::new();
    assert_eq!(feed.endpoints(), [SPOT_MARKET_WS_ENDPOINT]);
    assert_eq!(BinanceSpotTradeFeed::default().endpoints(), feed.endpoints());

    let url = BinanceWebSocketClient::from_base_url(SPOT_MARKET_WS_ENDPOINT)
        .symbol_stream_url(&symbol("BTCUSDT").as_pair(), "aggTrade");
    assert_eq!(url, "wss://stream.binance.com:9443/ws/btcusdt@aggTrade");

    // A raw stream, `/ws/<streamName>`, not a combined `/stream?streams=`.
    let url = reqwest::Url::parse(&url).unwrap();
    assert_eq!(url.scheme(), "wss");
    assert_eq!(url.host_str(), Some("stream.binance.com"));
    assert_eq!(url.port(), Some(9443));
    assert_eq!(url.path(), "/ws/btcusdt@aggTrade");
    assert_eq!(url.query(), None);
}

/// `connect` asks the server for that path, and the stream answers the
/// server's ping with its payload while the caller reads (L54-L57).
#[tokio::test]
async fn connect_requests_the_raw_stream_and_answers_the_server_ping() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("ws://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        let mut target = String::new();
        let mut ws = accept_hdr_async(
            tcp,
            |request: &Request, response: Response| -> Result<Response, ErrorResponse> {
                target = request.uri().to_string();
                Ok(response)
            },
        )
        .await
        .unwrap();
        ws.send(Message::Ping(b"spot-ping".to_vec())).await.unwrap();
        let pong = loop {
            let frame = timeout(Duration::from_secs(5), ws.next()).await.unwrap().unwrap();
            if let Message::Pong(payload) = frame.unwrap() {
                break payload;
            }
        };
        ws.send(Message::Text(AGG_TRADE.to_string())).await.unwrap();
        let _ = ws.close(None).await;
        (target, pong)
    });

    let messages = delivered(&BinanceSpotTradeFeed::new(), &endpoint).await;
    let (target, pong) = server.await.unwrap();
    assert_eq!(target, "/ws/btcusdt@aggTrade");
    assert_eq!(pong, b"spot-ping");
    assert_eq!(messages, [Ok(TradeFeedMessage::Trade {
        symbol: symbol("BNBBTC"),
        price: Price::new(dec!(0.001)).unwrap(),
    })]);
}

/// The documented spot payload carries `M`, which the futures wire type has
/// no field for; the shared type decodes it anyway, and the trade is a tick
/// for the symbol on the message.
#[test]
fn the_documented_agg_trade_decodes_to_a_tick_for_its_symbol() {
    let payload: Value = serde_json::from_str(AGG_TRADE).unwrap();
    assert_eq!(payload["M"], json!(true), "the fixture carries spot's extra field");

    let message: WsMessage = serde_json::from_str(AGG_TRADE).unwrap();
    let WsMessage::AggTrade(trade) = &message else {
        panic!("expected an aggTrade, got {message:?}");
    };
    assert_eq!(trade.event_time, 1_672_515_782_136);
    assert_eq!(trade.symbol, "BNBBTC");
    assert_eq!(trade.agg_trade_id, 12345);
    assert_eq!((trade.price.as_str(), trade.quantity.as_str()), ("0.001", "100"));
    assert_eq!((trade.first_trade_id, trade.last_trade_id), (100, 105));
    assert_eq!(trade.trade_time, 1_672_515_782_136);
    assert!(trade.is_buyer_maker);

    assert_eq!(decode_message(message), TradeFeedMessage::Trade {
        symbol: symbol("BNBBTC"),
        price: Price::new(dec!(0.001)).unwrap(),
    });
    // The symbol comes from the message, whatever the subscription.
    assert_eq!(decoded(&agg_trade("ETHUSDT", "2500.5")), TradeFeedMessage::Trade {
        symbol: symbol("ETHUSDT"),
        price: Price::new(dec!(2500.5)).unwrap(),
    });
}

/// As on USD-M: an undecodable price or symbol is delivered but never a
/// tick, and a message that is not a trade is `NonTrade`.
#[test]
fn undecodable_trades_and_other_messages_are_never_ticks() {
    for (frame, expected_reason, expected_value) in [
        (agg_trade("BTCUSDT", "not-a-number"), "Failed to parse price", "not-a-number"),
        (agg_trade("BTCUSDT", "-1"), "Invalid price value", "-1"),
        (agg_trade("BTCEUR", "95000"), "Failed to parse symbol", "BTCEUR"),
        (agg_trade("???", "95000"), "Failed to parse symbol", "???"),
    ] {
        match decoded(&frame) {
            TradeFeedMessage::Malformed { reason, value, error } => {
                assert_eq!((reason, value.as_str()), (expected_reason, expected_value));
                assert!(!error.is_empty());
            },
            other => panic!("expected Malformed for {frame}, got {other:?}"),
        }
    }
    assert_eq!(decoded(&ticker()), TradeFeedMessage::NonTrade);
}

/// The spot and USD-M feeds deliver the same messages for the same frames,
/// stream errors included: a `serverShutdown` event (L85-L110), which the
/// shared wire type does not decode, is a stream error, and the caller's
/// reconnect loop ends the attempt on it.
#[tokio::test]
async fn the_spot_feed_delivers_what_the_usdm_feed_delivers() {
    let frames = vec![
        AGG_TRADE.to_string(),
        agg_trade("BTCUSDT", "95000.01"),
        agg_trade("BTCUSDT", "not-a-number"),
        agg_trade("BTCUSDT", "-1"),
        agg_trade("BTCEUR", "95000"),
        ticker(),
        SERVER_SHUTDOWN.to_string(),
        "{\"result\":null,\"id\":1}".to_string(),
    ];

    let (endpoint, spot_server) = playing(frames.clone()).await;
    let spot = delivered(&BinanceSpotTradeFeed::new(), &endpoint).await;
    assert_eq!(spot_server.await.unwrap(), "/ws/btcusdt@aggTrade");

    let (endpoint, usdm_server) = playing(frames).await;
    let usdm =
        delivered(&BinanceUsdmTradeFeed::new(false, vec![endpoint.clone()]), &endpoint).await;
    assert_eq!(usdm_server.await.unwrap(), "/ws/btcusdt@aggTrade");

    assert_eq!(spot, usdm);
    let kinds: Vec<&str> = spot
        .iter()
        .map(|message| match message {
            Ok(TradeFeedMessage::Trade { .. }) => "trade",
            Ok(TradeFeedMessage::NonTrade) => "non-trade",
            Ok(TradeFeedMessage::Malformed { .. }) => "malformed",
            Err(_) => "error",
        })
        .collect();
    assert_eq!(kinds, [
        "trade",
        "trade",
        "malformed",
        "malformed",
        "malformed",
        "non-trade",
        "error",
        "error"
    ]);
    assert_eq!(
        spot[1],
        Ok(TradeFeedMessage::Trade {
            symbol: symbol("BTCUSDT"),
            price: Price::new(dec!(95000.01)).unwrap(),
        })
    );
    let Err(shutdown) = &spot[6] else {
        unreachable!()
    };
    assert!(shutdown.contains("serverShutdown"), "{shutdown}");
}
