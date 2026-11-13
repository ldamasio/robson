//! `FakeSpotVenue` (8d plan §2.3, §7.1, §7.4): authentication, routes and
//! weights, answers, per-symbol limits, and the shutdown that leaves nothing
//! running, on one current-thread runtime (the quiescence protocol), once
//! and over 200 cycles. The spot adapter parses each route's shape in run S
//! (`robson-connectors`).

use std::{
    future::Future,
    sync::Arc,
    time::{Duration, Instant},
};

use axum::http::Method;
use hmac::{Hmac, Mac};
use rust_decimal::Decimal;
use serde_json::Value;
use sha2::Sha256;
use tokio::runtime::{Builder, Handle};

use crate::{
    fake_spot_venue::{FakeSpotVenue, SignatureVerdict, TestCredentials, SHUTDOWN_BOUND},
    spot_book::{BookRules, Script, SpotBook},
};

const KEY: &str = TestCredentials::FIXED.api_key;

fn d(s: &str) -> Decimal {
    s.parse().unwrap()
}

/// BTCUSDT and ETHUSDT, each limited to 200 orders and 5 stops, at 100 and
/// 10, with BTC 10, ETH 10 and USDT 1000.
fn book() -> SpotBook {
    let rules = vec![
        BookRules::btcusdt(),
        BookRules::new("ETHUSDT", "ETH", "USDT").with_limits(200, 5),
    ];
    let book = SpotBook::new(rules, d("0.001"));
    book.set_price("BTCUSDT", d("100"));
    book.set_price("ETHUSDT", d("10"));
    book.set_balance("BTC", d("10"));
    book.set_balance("ETH", d("10"));
    book.set_balance("USDT", d("1000"));
    book
}

async fn venue() -> FakeSpotVenue {
    FakeSpotVenue::start(Arc::new(book()), TestCredentials::FIXED).await.unwrap()
}

fn hmac_hex(secret: &str, payload: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(payload.as_bytes());
    mac.finalize().into_bytes().iter().map(|b| format!("{b:02x}")).collect()
}

/// `query` with its signature appended, as the REST client signs.
fn signed(query: &str) -> String {
    format!("{query}&signature={}", hmac_hex(TestCredentials::FIXED.api_secret, query))
}

fn client() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().build().unwrap()
}

/// One request; the answer's status and JSON body.
async fn call(
    venue: &FakeSpotVenue,
    method: reqwest::Method,
    target: &str,
    key: Option<&str>,
) -> (u16, Value) {
    let mut request = client().request(method, format!("{}{target}", venue.url()));
    if let Some(key) = key {
        request = request.header("X-MBX-APIKEY", key);
    }
    let answer = request.send().await.unwrap();
    let status = answer.status().as_u16();
    (status, serde_json::from_str(&answer.text().await.unwrap()).unwrap())
}

async fn get(venue: &FakeSpotVenue, path: &str, query: &str) -> (u16, Value) {
    call(venue, reqwest::Method::GET, &format!("{path}?{}", signed(query)), Some(KEY)).await
}

async fn post(venue: &FakeSpotVenue, query: &str) -> (u16, Value) {
    let target = format!("/api/v3/order?{}", signed(query));
    call(venue, reqwest::Method::POST, &target, Some(KEY)).await
}

async fn delete(venue: &FakeSpotVenue, query: &str) -> (u16, Value) {
    let target = format!("/api/v3/order?{}", signed(query));
    call(venue, reqwest::Method::DELETE, &target, Some(KEY)).await
}

fn code(body: &Value) -> i64 {
    body["code"].as_i64().unwrap_or(0)
}

fn stop_query(symbol: &str, qty: &str, trigger: &str, id: &str) -> String {
    format!(
        "newClientOrderId={id}&newOrderRespType=RESULT&quantity={qty}&selfTradePreventionMode=\
         EXPIRE_MAKER&side=SELL&stopPrice={trigger}&symbol={symbol}&timestamp=1&type=STOP_LOSS"
    )
}

#[test]
fn the_test_credentials_are_a_keys_format() {
    for value in [
        TestCredentials::FIXED.api_key,
        TestCredentials::FIXED.api_secret,
    ] {
        assert_eq!(value.len(), 64);
        assert!(value.bytes().all(|b| b.is_ascii_alphanumeric()));
    }
}

#[tokio::test]
async fn signed_routes_authenticate_the_key_then_the_signature() {
    let venue = venue().await;
    let account = "/api/v3/account";
    let query = "omitZeroBalances=true&timestamp=1";
    let get = reqwest::Method::GET;
    let target = format!("{account}?{}", signed(query));
    let corrupt = format!("{account}?{query}&signature={}", "0".repeat(64));
    let upper = format!(
        "{account}?{query}&signature={}",
        hmac_hex(TestCredentials::FIXED.api_secret, query).to_uppercase()
    );
    let foreign = "Z".repeat(64);
    let unsigned = format!("{account}?{query}");
    let cases = [
        (&target, None, 401, -2014),
        (&target, Some("key"), 401, -2014),
        (&target, Some(foreign.as_str()), 401, -2015),
        (&unsigned, Some(KEY), 400, -1102),
        (&corrupt, Some(KEY), 400, -1022),
        (&upper, Some(KEY), 200, 0),
        (&target, Some(KEY), 200, 0),
    ];
    for (target, key, status, expected) in cases {
        let (got, body) = call(&venue, get.clone(), target, key).await;
        assert_eq!((got, code(&body)), (status, expected), "{target} with key {key:?}: {body}");
    }
    // Signed over the decoded bytes instead of the query as sent: refused.
    let sent = "origClientOrderId=a%2Fb&symbol=BTCUSDT&timestamp=1";
    let decoded = hmac_hex(
        TestCredentials::FIXED.api_secret,
        "origClientOrderId=a/b&symbol=BTCUSDT&timestamp=1",
    );
    let target = format!("/api/v3/order?{sent}&signature={decoded}");
    let (status, body) = call(&venue, get.clone(), &target, Some(KEY)).await;
    assert_eq!((status, code(&body)), (400, -1022));
    // The same query signed as sent passes authentication: the order is unknown.
    let (status, body) =
        call(&venue, get, &format!("/api/v3/order?{}", signed(sent)), Some(KEY)).await;
    assert_eq!((status, code(&body)), (400, -2013));

    let verdicts: Vec<SignatureVerdict> = venue.requests().iter().map(|r| r.signature).collect();
    use SignatureVerdict::*;
    assert_eq!(verdicts, [
        NotChecked, NotChecked, NotChecked, Missing, Invalid, Valid, Valid, Invalid, Valid
    ]);
    assert!(!venue.requests()[0].api_key && venue.requests()[1].api_key);
    // Only what was served weighs: two accounts and one order lookup.
    assert_eq!(venue.weight(), 20 + 20 + 4);
    venue.shutdown().await.unwrap();
}

#[tokio::test]
async fn public_routes_need_neither_key_nor_signature() {
    let venue = venue().await;
    let get = reqwest::Method::GET;
    let (status, _) = call(&venue, get.clone(), "/api/v3/ping", None).await;
    assert_eq!(status, 200);
    let (status, info) =
        call(&venue, get.clone(), "/api/v3/exchangeInfo?symbol=ETHUSDT", None).await;
    assert_eq!(status, 200);
    let symbol = &info["symbols"][0];
    assert_eq!(
        (symbol["symbol"].as_str(), symbol["baseAsset"].as_str()),
        (Some("ETHUSDT"), Some("ETH"))
    );
    let (status, price) = call(&venue, get, "/api/v3/ticker/price?symbol=BTCUSDT", None).await;
    assert_eq!((status, price["price"].as_str()), (200, Some("100")));
    assert!(venue
        .requests()
        .iter()
        .all(|r| r.signature == SignatureVerdict::Unsigned && !r.api_key));
    assert_eq!(venue.weight(), 1 + 20 + 2);
    venue.shutdown().await.unwrap();
}

#[tokio::test]
async fn each_route_weighs_what_binance_documents_and_the_rest_is_unserved() {
    let venue = venue().await;
    let routes = [
        ("/api/v3/account", "timestamp=1", 20),
        ("/api/v3/openOrders", "symbol=BTCUSDT&timestamp=1", 6),
        ("/api/v3/openOrders", "timestamp=1", 80),
        ("/api/v3/openOrderList", "timestamp=1", 6),
        ("/api/v3/order", "orderId=1&symbol=BTCUSDT&timestamp=1", 4),
        ("/api/v3/myTrades", "symbol=BTCUSDT&timestamp=1", 20),
        ("/api/v3/myTrades", "orderId=1&symbol=BTCUSDT&timestamp=1", 5),
    ];
    for (path, query, weight) in routes {
        let before = venue.weight();
        get(&venue, path, query).await;
        assert_eq!(venue.weight() - before, weight, "{path}?{query}");
    }
    let before = venue.weight();
    post(&venue, &stop_query("BTCUSDT", "0.5", "90", "w1")).await;
    delete(&venue, "orderId=1&symbol=BTCUSDT&timestamp=1").await;
    assert_eq!(venue.weight() - before, 2, "a placement and a cancel weigh 1 each");

    assert!(venue.unserved_requests().is_empty());
    let (status, _) =
        call(&venue, reqwest::Method::GET, "/api/v3/avgPrice?symbol=BTCUSDT", None).await;
    assert_eq!(status, 400);
    let (status, _) = get(&venue, "/api/v3/account/commission", "symbol=BTCUSDT&timestamp=1").await;
    assert_eq!(status, 400);
    let unserved = venue.unserved_requests();
    assert_eq!(unserved.len(), 2);
    assert!(
        unserved[0].starts_with("GET /api/v3/avgPrice?")
            && unserved[1].starts_with("GET /api/v3/account/commission?")
    );
    venue.shutdown().await.unwrap();
}

#[tokio::test]
async fn orders_and_cancels_answer_as_binance_does() {
    let venue = venue().await;
    let market = "newClientOrderId=m1&newOrderRespType=FULL&quantity=0.1&selfTradePreventionMode=\
                  EXPIRE_MAKER&side=BUY&symbol=BTCUSDT&timestamp=1&type=MARKET";
    let (status, full) = post(&venue, market).await;
    assert_eq!(
        (status, full["status"].as_str(), full["clientOrderId"].as_str()),
        (200, Some("FILLED"), Some("m1"))
    );
    let fill = &full["fills"][0];
    assert_eq!(
        (fill["qty"].as_str(), fill["commissionAsset"].as_str()),
        (Some("0.1"), Some("BTC"))
    );

    let (status, stop) = post(&venue, &stop_query("BTCUSDT", "0.5", "90", "s1")).await;
    assert_eq!(
        (status, stop["type"].as_str(), stop["status"].as_str()),
        (200, Some("STOP_LOSS"), Some("NEW"))
    );
    assert!(stop.get("fills").is_none(), "RESULT carries no fills");
    let (status, refused) = post(&venue, &stop_query("BTCUSDT", "0.5", "100", "s2")).await;
    assert_eq!((status, code(&refused)), (400, -2010), "a trigger at the price");

    // A lost answer: the book acts, the client gets HTTP 503 -1007.
    venue.book().script_next(Script::Lost);
    let (status, lost) = post(&venue, &stop_query("BTCUSDT", "0.5", "80", "s3")).await;
    assert_eq!((status, code(&lost)), (503, -1007));
    let (status, found) =
        get(&venue, "/api/v3/order", "origClientOrderId=s3&symbol=BTCUSDT&timestamp=1").await;
    assert_eq!((status, found["status"].as_str()), (200, Some("NEW")));

    // A delayed cancel: queued, HTTP 503 -1007.
    let id = stop["orderId"].as_u64().unwrap();
    venue.book().delay_cancels(true);
    let (status, delayed) =
        delete(&venue, &format!("orderId={id}&symbol=BTCUSDT&timestamp=1")).await;
    assert_eq!((status, code(&delayed)), (503, -1007));
    assert_eq!(venue.book().queued_cancels(), [id]);
    venue.book().delay_cancels(false);
    let (status, cancelled) =
        delete(&venue, &format!("orderId={id}&symbol=BTCUSDT&timestamp=1")).await;
    assert_eq!(
        (status, cancelled["status"].as_str(), cancelled["origClientOrderId"].as_str()),
        (200, Some("CANCELED"), Some("s1"))
    );
    assert_ne!(cancelled["clientOrderId"], cancelled["origClientOrderId"]);
    let (status, gone) = delete(&venue, &format!("orderId={id}&symbol=BTCUSDT&timestamp=1")).await;
    assert_eq!((status, code(&gone)), (400, -2011));
    let (status, unknown) =
        get(&venue, "/api/v3/order", "orderId=999&symbol=BTCUSDT&timestamp=1").await;
    assert_eq!((status, code(&unknown)), (400, -2013));

    // A scripted wire answer, once; the hook runs before the book acts.
    venue.script_wire_next(
        Method::GET,
        "/api/v3/account",
        418,
        "{\"code\":-1003,\"msg\":\"banned\"}",
    );
    let (status, banned) = get(&venue, "/api/v3/account", "timestamp=1").await;
    assert_eq!((status, code(&banned)), (418, -1003));
    let (status, _) = get(&venue, "/api/v3/account", "timestamp=1").await;
    assert_eq!(status, 200);
    venue.on_next_placement(Box::new(|book: &SpotBook| book.set_price("BTCUSDT", d("70"))));
    let (status, refused) = post(&venue, &stop_query("BTCUSDT", "0.5", "75", "s4")).await;
    assert_eq!((status, code(&refused)), (400, -2010), "the hook moved the price first");
    venue.shutdown().await.unwrap();
}

#[tokio::test]
async fn order_limits_are_per_symbol_over_the_wire() {
    let venue = venue().await;
    for n in 0..5 {
        let (status, _) = post(&venue, &stop_query("BTCUSDT", "0.1", "90", &format!("b{n}"))).await;
        assert_eq!(status, 200);
    }
    let (status, sixth) = post(&venue, &stop_query("BTCUSDT", "0.1", "90", "b5")).await;
    assert_eq!(
        (status, code(&sixth), sixth["msg"].as_str()),
        (400, -1013, Some("Filter failure: MAX_NUM_ALGO_ORDERS"))
    );
    let (status, _) = post(&venue, &stop_query("ETHUSDT", "0.1", "9", "e0")).await;
    assert_eq!(status, 200, "the other symbol still accepts a stop");
    let (_, info) =
        call(&venue, reqwest::Method::GET, "/api/v3/exchangeInfo?symbol=BTCUSDT", None).await;
    let filters = info["symbols"][0]["filters"].as_array().unwrap();
    let algo = filters.iter().find(|f| f["filterType"] == "MAX_NUM_ALGO_ORDERS").unwrap();
    assert_eq!(algo["maxNumAlgoOrders"], 5);
    venue.shutdown().await.unwrap();
}

/// Poll `done`, yielding to the runtime, for at most [`SHUTDOWN_BOUND`].
async fn settle(
    what: &str,
    mut done: impl FnMut() -> bool,
    show: impl Fn() -> String,
) -> Result<(), String> {
    let deadline = Instant::now() + SHUTDOWN_BOUND;
    while !done() {
        if Instant::now() >= deadline {
            return Err(format!("{what}: not within {SHUTDOWN_BOUND:?}: {}", show()));
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    Ok(())
}

async fn bounded<T>(what: &str, future: impl Future<Output = T>) -> Result<T, String> {
    tokio::time::timeout(SHUTDOWN_BOUND, future)
        .await
        .map_err(|_| format!("{what}: not within {SHUTDOWN_BOUND:?}"))
}

/// One cycle of the quiescence protocol (8d plan §7.4) on the current
/// runtime: a held request and an idle keep-alive connection present, then
/// shutdown, the test's side released, and everything back at `baseline`.
/// Returns the drain's duration.
async fn cycle(book: &Arc<SpotBook>, baseline: usize) -> Result<Duration, String> {
    let venue = FakeSpotVenue::start(Arc::clone(book), TestCredentials::FIXED)
        .await
        .map_err(|e| e.to_string())?;
    let counters = venue.counters();
    let url = venue.url();
    let idle = client();
    let served = bounded("the served ping", idle.get(format!("{url}/api/v3/ping")).send()).await?;
    let served = served.map_err(|e| e.to_string())?;
    if served.status() != 200 {
        return Err(format!("the ping answered {}", served.status()));
    }
    drop(served);
    venue.hold_next(Method::GET, "/api/v3/ping");
    let held = client();
    let target = format!("{url}/api/v3/ping");
    let sender = tokio::spawn(async move {
        held.get(target).send().await.map(|answer| answer.status().as_u16())
    });
    settle(
        "present before shutdown",
        || counters.held() == 1 && counters.live() == (2, 1),
        || format!("held {}, live {:?}", counters.held(), counters.live()),
    )
    .await?;
    let started = Instant::now();
    venue.shutdown().await.map_err(|e| e.to_string())?;
    let drain = started.elapsed();
    drop(idle);
    let answered = bounded("the held request's sender", sender).await?;
    if !matches!(answered, Ok(Ok(503))) {
        return Err(format!("the held request got {answered:?}, not 503"));
    }
    let metrics = Handle::current().metrics();
    settle(
        "quiescence",
        || {
            counters.live() == (0, 0)
                && Arc::strong_count(book) == 1
                && metrics.num_alive_tasks() == baseline
        },
        || {
            format!(
                "live {:?}, book references {}, tasks {} (baseline {baseline})",
                counters.live(),
                Arc::strong_count(book),
                metrics.num_alive_tasks()
            )
        },
    )
    .await?;
    let port = url.trim_start_matches("http://").to_string();
    if tokio::net::TcpStream::connect(&port).await.is_ok() {
        return Err(format!("{port} still accepts connections"));
    }
    Ok(drain)
}

fn on_one_thread<T>(test: impl Future<Output = T>) -> T {
    Builder::new_current_thread().enable_all().build().unwrap().block_on(test)
}

/// §7.1: one dedicated current-thread runtime, the baseline read in it.
#[test]
fn shutdown_drains_a_held_request_and_an_idle_connection() {
    on_one_thread(async {
        let book = Arc::new(book());
        let baseline = Handle::current().metrics().num_alive_tasks();
        let drain = cycle(&book, baseline).await.unwrap_or_else(|why| panic!("{why}"));
        assert!(drain < SHUTDOWN_BOUND);
    });
}

/// §8: the cleanup profile, 200 cycles on one current-thread runtime.
#[test]
fn cleanup_profile_200_cycles() {
    on_one_thread(async {
        let book = Arc::new(book());
        let baseline = Handle::current().metrics().num_alive_tasks();
        let started = Instant::now();
        let mut slowest = Duration::ZERO;
        for n in 0..200 {
            let drain =
                cycle(&book, baseline).await.unwrap_or_else(|why| panic!("cycle {n}: {why}"));
            slowest = slowest.max(drain);
        }
        println!(
            "cleanup profile: 200 cycles in {:?}, slowest drain {slowest:?}",
            started.elapsed()
        );
        assert_eq!(Handle::current().metrics().num_alive_tasks(), baseline);
        assert_eq!(Arc::strong_count(&book), 1);
    });
}
