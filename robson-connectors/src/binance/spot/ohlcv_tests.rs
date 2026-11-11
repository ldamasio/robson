//! ADR-0056 step 8b: the spot klines observed on the wire.
//!
//! Requests go through the real REST client to a local listener standing in
//! for Binance, as in `read_tests.rs`; the host is the one the client's path
//! rule picks, checked on a client with no listener. The USD-M klines, on the
//! same answers, are the reference for how each error maps. Nothing reaches
//! the network.

use std::{
    str::FromStr,
    sync::{Arc, Mutex},
    time::Duration,
};

use chrono::TimeZone;
use robson_domain::Symbol;
use robson_exec::{CandleInterval, ExecError, OhlcvPort};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde_json::{json, Value};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

use super::{BinanceSpotOhlcv, SpotRead};
use crate::{binance::usdm::BinanceOhlcvAdapter, binance_rest::BinanceRestClient};

const KLINES: &str = include_str!("testdata/klines.json");
const KLINES_LIVE: &str = include_str!("testdata/klines_btcusdt_1m.json");
const ERRORS: &str = include_str!("testdata/errors.json");

/// One request as the stand-in venue saw it.
#[derive(Debug, Clone)]
struct Seen {
    method: String,
    target: String,
    api_key: bool,
}

impl Seen {
    fn param(&self, name: &str) -> Option<&str> {
        let (_, query) = self.target.split_once('?')?;
        query.split('&').find_map(|part| part.strip_prefix(name)?.strip_prefix('='))
    }
}

type Seens = Arc<Mutex<Vec<Seen>>>;

/// A stand-in venue answering every request with `status` and `body`, or,
/// when `status` is 0, never answering a client bounded at 300 ms.
async fn venue(status: u16, body: &str) -> (Arc<BinanceRestClient>, Seens) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&seen);
    let body = body.to_string();
    tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut head = Vec::new();
            let mut buf = [0u8; 4096];
            while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                match stream.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => head.extend_from_slice(&buf[..n]),
                }
            }
            let head = String::from_utf8_lossy(&head).into_owned();
            let mut lines = head.lines();
            let mut first = lines.next().unwrap_or_default().split_whitespace();
            recorded.lock().unwrap().push(Seen {
                method: first.next().unwrap_or_default().to_string(),
                target: first.next().unwrap_or_default().to_string(),
                api_key: lines.any(|line| line.to_ascii_lowercase().starts_with("x-mbx-apikey:")),
            });
            if status == 0 {
                // Hold the connection open, unanswered.
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_secs(30)).await;
                    drop(stream);
                });
                continue;
            }
            let response = format!(
                "HTTP/1.1 {status} Answer\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes()).await;
        }
    });
    let mut client = BinanceRestClient::with_local_base_url(url);
    if status == 0 {
        client = client.with_request_timeout(Duration::from_millis(300));
    }
    (Arc::new(client), seen)
}

fn btcusdt() -> Symbol {
    Symbol::from_pair("BTCUSDT").unwrap()
}

fn adapter(client: Arc<BinanceRestClient>) -> BinanceSpotOhlcv {
    BinanceSpotOhlcv::new(client).unwrap()
}

fn error_body(name: &str) -> String {
    let errors: Value = serde_json::from_str(ERRORS).unwrap();
    errors[name].as_str().unwrap().to_string()
}

fn decimal(value: &Value) -> Decimal {
    Decimal::from_str(value.as_str().unwrap()).unwrap()
}

/// F25: one public GET of `/api/v3/klines`, weight 2, with no API key,
/// timestamp or signature (`rest-api.md` L1273-L1292); the live BTCUSDT
/// answer parses into candles, oldest first.
#[tokio::test]
async fn klines_are_one_public_get_parsed_oldest_first() {
    assert_eq!(SpotRead::KLINES, SpotRead {
        path: "/api/v3/klines",
        weight: 2,
        signed: false,
    });

    let (client, seen) = venue(200, KLINES_LIVE).await;
    let candles = adapter(client)
        .fetch_candles(&btcusdt(), CandleInterval::OneMinute, 3)
        .await
        .unwrap();

    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].method, "GET");
    assert_eq!(seen[0].target, "/api/v3/klines?symbol=BTCUSDT&interval=1m&limit=3");
    assert!(!seen[0].api_key, "a public read sends no API key");

    let rows: Vec<Vec<Value>> = serde_json::from_str(KLINES_LIVE).unwrap();
    assert_eq!((rows.len(), candles.len()), (3, 3));
    for (candle, row) in candles.iter().zip(&rows) {
        assert_eq!(row.len(), 12, "the documented layout");
        assert_eq!(candle.symbol, btcusdt());
        assert_eq!(candle.open_time.timestamp_millis(), row[0].as_i64().unwrap());
        assert_eq!(
            (candle.open, candle.high, candle.low, candle.close, candle.volume),
            (
                decimal(&row[1]),
                decimal(&row[2]),
                decimal(&row[3]),
                decimal(&row[4]),
                decimal(&row[5])
            )
        );
        assert_eq!(candle.close_time.timestamp_millis(), row[6].as_i64().unwrap());
        assert_eq!(candle.trades, row[8].as_u64().unwrap());
        assert_eq!(candle.close_time - candle.open_time, chrono::Duration::milliseconds(59_999));
    }
    assert!(candles
        .windows(2)
        .all(|pair| { pair[1].open_time - pair[0].open_time == chrono::Duration::minutes(1) }));
    assert_eq!(
        (candles[0].open, candles[0].close, candles[0].trades),
        (dec!(74387.63), dec!(74404.01), 987)
    );
}

/// The documented example parses field by field (L1319-L1337).
#[tokio::test]
async fn the_documented_kline_parses() {
    let (client, _) = venue(200, KLINES).await;
    let candles = adapter(client)
        .fetch_candles(&btcusdt(), CandleInterval::OneDay, 1)
        .await
        .unwrap();
    assert_eq!(candles.len(), 1);
    let candle = &candles[0];
    assert_eq!(
        (candle.open, candle.high, candle.low, candle.close, candle.volume, candle.trades),
        (
            dec!(0.01634790),
            dec!(0.8),
            dec!(0.015758),
            dec!(0.015771),
            dec!(148976.11427815),
            308
        )
    );
    assert_eq!(candle.open_time, chrono::Utc.timestamp_millis_opt(1_499_040_000_000).unwrap());
    assert_eq!(candle.close_time, chrono::Utc.timestamp_millis_opt(1_499_644_799_999).unwrap());
}

/// The shared client's path rule sends `/api/v3/klines` to the spot host,
/// and USD-M's `/fapi/v1/klines` to the futures one. A testnet client, whose
/// rule would pick the spot testnet, is refused.
#[test]
fn the_path_rule_sends_klines_to_the_spot_host_and_testnet_is_refused() {
    let mainnet = BinanceRestClient::new(String::new(), String::new());
    assert_eq!(mainnet.endpoint_base_url(SpotRead::KLINES.path), "https://api.binance.com");
    assert_eq!(mainnet.endpoint_base_url("/fapi/v1/klines"), "https://fapi.binance.com");
    assert!(BinanceSpotOhlcv::new(Arc::new(mainnet)).is_ok());

    let testnet = Arc::new(BinanceRestClient::testnet(String::new(), String::new()));
    let refused = BinanceSpotOhlcv::new(testnet).err().expect("a testnet client is refused");
    assert!(
        matches!(&refused, ExecError::Config(msg) if msg.contains("testnet")),
        "{refused:?}"
    );
}

/// Every `CandleInterval` goes out as a documented spot interval
/// (L1298-L1307), each its own.
#[tokio::test]
async fn every_candle_interval_is_a_documented_spot_interval() {
    use CandleInterval::*;
    const DOCUMENTED: [&str; 16] = [
        "1s", "1m", "3m", "5m", "15m", "30m", "1h", "2h", "4h", "6h", "8h", "12h", "1d", "3d",
        "1w", "1M",
    ];
    let mut sent = Vec::new();
    for interval in [
        OneMinute,
        FiveMinutes,
        FifteenMinutes,
        OneHour,
        FourHours,
        OneDay,
    ] {
        // A new interval fails to compile here, so this test is revisited.
        let expected = match interval {
            OneMinute => "1m",
            FiveMinutes => "5m",
            FifteenMinutes => "15m",
            OneHour => "1h",
            FourHours => "4h",
            OneDay => "1d",
        };
        let (client, seen) = venue(200, "[]").await;
        adapter(client).fetch_candles(&btcusdt(), interval, 1).await.unwrap();
        let seen = seen.lock().unwrap().clone();
        let wire = seen[0].param("interval").unwrap().to_string();
        assert_eq!(wire, expected, "{interval:?}");
        assert!(DOCUMENTED.contains(&wire.as_str()), "{interval:?}: {wire}");
        sent.push(wire);
    }
    let mut distinct = sent.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(distinct.len(), sent.len(), "{sent:?}");
}

/// `limit` is at least 1 and at most the documented 1000 (L1292); outside
/// that, nothing is sent.
#[tokio::test]
async fn limit_is_bounded_by_the_documented_maximum() {
    for limit in [0, 1001, u16::MAX] {
        let (client, seen) = venue(200, "[]").await;
        let refused = adapter(client)
            .fetch_candles(&btcusdt(), CandleInterval::FifteenMinutes, limit)
            .await
            .unwrap_err();
        assert!(matches!(refused, ExecError::Config(_)), "{limit}: {refused:?}");
        assert!(seen.lock().unwrap().is_empty(), "{limit}: nothing is sent");
    }
    for limit in [1, 500, 1000] {
        let (client, seen) = venue(200, "[]").await;
        adapter(client)
            .fetch_candles(&btcusdt(), CandleInterval::FifteenMinutes, limit)
            .await
            .unwrap();
        let seen = seen.lock().unwrap().clone();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].param("limit"), Some(limit.to_string().as_str()));
    }
}

/// Spot's klines against USD-M's on the same answer: the same error.
async fn both(status: u16, body: &str, limit: u16) -> (String, String) {
    let (client, _) = venue(status, body).await;
    let spot = adapter(client).fetch_candles(&btcusdt(), CandleInterval::OneHour, limit).await;
    let (client, _) = venue(status, body).await;
    let usdm = BinanceOhlcvAdapter::new(client)
        .fetch_candles(&btcusdt(), CandleInterval::OneHour, limit)
        .await;
    (format!("{spot:?}"), format!("{usdm:?}"))
}

/// Errors map as on USD-M: a venue error, a non-JSON failure, a timeout, a
/// refused connection and an out-of-range limit each give the same
/// `ExecError`.
#[tokio::test]
async fn errors_map_as_the_usdm_klines_map_them() {
    for (status, body, variant) in [
        (400, error_body("bad_symbol"), "Exchange"),
        (429, error_body("too_many_requests"), "Exchange"),
        (418, error_body("too_many_requests"), "Exchange"),
        (500, error_body("unknown"), "Exchange"),
        (503, error_body("html_page"), "Exchange"),
        (403, error_body("html_page"), "Exchange"),
        (0, String::new(), "Timeout"),
    ] {
        let (spot, usdm) = both(status, &body, 10).await;
        assert_eq!(spot, usdm, "HTTP {status}");
        assert!(spot.starts_with(&format!("Err({variant}(")), "HTTP {status}: {spot}");
    }

    let (spot, usdm) = both(200, "[]", 0).await;
    assert_eq!(spot, usdm);
    assert!(spot.starts_with("Err(Config("), "{spot}");

    // Nothing listens: the request fails before any answer. The messages name
    // each adapter's own URL, so only the mapping is compared.
    let closed = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", closed.local_addr().unwrap());
    drop(closed);
    let client = || Arc::new(BinanceRestClient::with_local_base_url(url.clone()));
    let interval = CandleInterval::OneHour;
    let spot_error = adapter(client()).fetch_candles(&btcusdt(), interval, 10).await.unwrap_err();
    let usdm_error = BinanceOhlcvAdapter::new(client())
        .fetch_candles(&btcusdt(), interval, 10)
        .await
        .unwrap_err();
    for error in [&spot_error, &usdm_error] {
        assert!(
            matches!(error, ExecError::Exchange(msg) if msg.starts_with("HTTP request failed")),
            "{error:?}"
        );
    }
}

/// A kline off the documented shape fails the whole read, as on USD-M:
/// too few fields, a number where a decimal string belongs, a time as a
/// string, a negative trade count, a word for a price, an open time no
/// timestamp holds, a row that is not an array, a body that is not a list.
#[tokio::test]
async fn a_wrong_shaped_kline_is_refused() {
    let rows: Vec<Vec<Value>> = serde_json::from_str(KLINES).unwrap();
    let row = rows[0].clone();
    let with = |index: usize, value: Value| {
        let mut row = row.clone();
        row[index] = value;
        json!([row]).to_string()
    };
    let bodies = [
        json!([row[..8].to_vec()]).to_string(),
        with(1, json!(0.0163479)),
        with(0, json!("1499040000000")),
        with(8, json!(-1)),
        with(4, json!("abc")),
        with(0, json!(i64::MAX)),
        json!([{ "openTime": 1_499_040_000_000u64 }]).to_string(),
        json!({ "code": 0, "msg": "" }).to_string(),
        "not json".to_string(),
    ];
    for body in bodies {
        let (spot, usdm) = both(200, &body, 1).await;
        assert_eq!(spot, usdm, "{body}");
        assert!(spot.starts_with("Err(Exchange("), "{body}: {spot}");
    }
}
