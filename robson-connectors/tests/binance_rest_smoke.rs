//! Read-only smoke test against live Binance public endpoints.
//!
//! `#[ignore]` by default: it needs network and it talks to a third party, so
//! it must never run in the normal test job. Run it deliberately:
//!
//! ```text
//! cargo test -p robson-connectors --test binance_rest_smoke -- --ignored --nocapture
//! ```
//!
//! Why it exists: nothing else in this workspace sends an HTTP request through
//! `BinanceRestClient`. Unit tests cover parsing and signing; the transport is
//! never exercised. That gap is invisible until a dependency bump changes the
//! transport underneath, which is exactly what the reqwest 0.11 -> 0.12 move
//! did (hyper 0.14 -> 1, new TCP keepalive and TCP_USER_TIMEOUT defaults).
//!
//! Every endpoint used here is public and unsigned. The client is constructed
//! with empty credentials on purpose: if any of these calls ever starts
//! requiring a signature, this test fails rather than silently authenticating.

use robson_connectors::binance_rest::BinanceRestClient;

fn client() -> BinanceRestClient {
    BinanceRestClient::new(String::new(), String::new())
}

#[tokio::test]
#[ignore = "hits live Binance endpoints; run explicitly with --ignored"]
async fn smoke_futures_ping() {
    client().ping().await.expect("futures ping failed");
}

#[tokio::test]
#[ignore = "hits live Binance endpoints; run explicitly with --ignored"]
async fn smoke_futures_price() {
    let price = client().get_price("BTCUSDT").await.expect("futures price failed");
    assert!(
        price.as_decimal() > rust_decimal::Decimal::ZERO,
        "price must be positive: {price}"
    );
    println!("futures BTCUSDT = {price}");
}

#[tokio::test]
#[ignore = "hits live Binance endpoints; run explicitly with --ignored"]
async fn smoke_spot_price() {
    let price = client().get_spot_price("BTCUSDT").await.expect("spot price failed");
    assert!(
        price.as_decimal() > rust_decimal::Decimal::ZERO,
        "price must be positive: {price}"
    );
    println!("spot BTCUSDT = {price}");
}

#[tokio::test]
#[ignore = "hits live Binance endpoints; run explicitly with --ignored"]
async fn smoke_spot_symbol_is_trading() {
    let trading = client()
        .spot_symbol_is_trading("BTCUSDT")
        .await
        .expect("exchangeInfo query failed");
    assert!(trading, "BTCUSDT should be trading on spot");
}
