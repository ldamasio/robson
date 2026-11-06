//! ADR-0056 step 4: Binance wire encodings observed on the request itself.
//!
//! `testdata/pre_adr0056_step4.json` was captured from commit 93a3bb76, before
//! the encodings moved out of the port layer. The requests go through the real
//! adapters and REST client to a local listener standing in for Binance, so
//! these tests do not depend on where the encodings live.

use std::sync::{Arc, Mutex};

use chrono::{TimeZone, Utc};
use robson_domain::{Exchange, Symbol};
use robson_exec::{venue::FundingPort, CandleInterval, OhlcvPort, UniversalTransferType};
use rust_decimal_macros::dec;
use serde_json::Value;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

use crate::{
    binance::usdm::{BinanceExchangeAdapter, BinanceOhlcvAdapter},
    binance_rest::BinanceRestClient,
};

fn golden() -> Value {
    serde_json::from_str(include_str!("testdata/pre_adr0056_step4.json")).unwrap()
}

/// Answer every request with `body`, recording its `"METHOD target"`.
async fn recording_client(body: &'static str) -> (Arc<BinanceRestClient>, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(Vec::new()));

    let recorded = requests.clone();
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
            let mut line = head.split_whitespace();
            let (method, target) =
                (line.next().unwrap_or_default(), line.next().unwrap_or_default());
            recorded.lock().unwrap().push(format!("{method} {target}"));

            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes()).await;
        }
    });

    (Arc::new(BinanceRestClient::with_local_base_url(url)), requests)
}

/// The `type` query parameter of a recorded request.
fn transfer_type_param(request: &str) -> &str {
    request
        .split(['?', '&'])
        .find_map(|part| part.strip_prefix("type="))
        .expect("request carries a type parameter")
}

#[tokio::test]
async fn candle_intervals_on_the_wire_are_stable() {
    let golden = golden();
    let symbol = Symbol::from_pair("BTCUSDT").unwrap();

    for (interval, variant) in [
        (CandleInterval::OneMinute, "OneMinute"),
        (CandleInterval::FiveMinutes, "FiveMinutes"),
        (CandleInterval::FifteenMinutes, "FifteenMinutes"),
        (CandleInterval::OneHour, "OneHour"),
        (CandleInterval::FourHours, "FourHours"),
        (CandleInterval::OneDay, "OneDay"),
    ] {
        let (client, requests) = recording_client("[]").await;
        BinanceOhlcvAdapter::new(client)
            .fetch_candles(&symbol, interval, 1)
            .await
            .unwrap();

        let wire = golden["candle_interval"][variant].as_str().unwrap();
        assert_eq!(requests.lock().unwrap().as_slice(), [format!(
            "GET /fapi/v1/klines?symbol=BTCUSDT&interval={wire}&limit=1"
        )]);
    }
}

#[tokio::test]
async fn universal_transfer_type_on_the_wire_is_stable() {
    let golden = golden();
    let wire = golden["universal_transfer_type"]["MainUmfuture"].as_str().unwrap();

    let (client, requests) = recording_client(r#"{"tranId":1}"#).await;
    BinanceExchangeAdapter::new(client)
        .transfer("USDT", dec!(5), UniversalTransferType::MainUmfuture, "fixture-transfer")
        .await
        .unwrap();
    let recorded = requests.lock().unwrap().clone();
    assert_eq!(recorded.len(), 1);
    assert!(recorded[0].starts_with("POST /sapi/v1/asset/transfer?"), "{}", recorded[0]);
    assert_eq!(transfer_type_param(&recorded[0]), wire);

    let (client, requests) = recording_client(r#"{"rows":[]}"#).await;
    BinanceExchangeAdapter::new(client)
        .transfer_history(
            UniversalTransferType::MainUmfuture,
            Utc.timestamp_millis_opt(1_700_000_000_000).unwrap(),
        )
        .await
        .unwrap();
    let recorded = requests.lock().unwrap().clone();
    assert_eq!(recorded.len(), 1);
    assert!(recorded[0].starts_with("GET /sapi/v1/asset/transfer?"), "{}", recorded[0]);
    assert_eq!(transfer_type_param(&recorded[0]), wire);
}

#[test]
fn rest_base_urls_are_stable() {
    let golden = golden();
    assert_eq!(Exchange::Binance.base_url(), golden["rest_base_url"]["binance"]);
    assert_eq!(Exchange::BinanceTestnet.base_url(), golden["rest_base_url"]["binance_testnet"]);
}
