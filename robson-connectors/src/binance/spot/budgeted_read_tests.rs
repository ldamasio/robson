//! PS-4 (production non-trading Spot start): the closed set of budgeted
//! reads on the wire, their production body caps, and the closed client.
//! Every request goes to a loopback listener standing in for Binance;
//! nothing reaches the network.

use std::{
    future::Future,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use robson_domain::Symbol;
use robson_exec::{
    gate::OperationKind,
    order_journal::OrderJournal,
    venue::{AccountPort, InventoryPort, TradingPort},
    CandleInterval, ExecError, OhlcvPort,
};
use rust_decimal_macros::dec;
use serde_json::{json, Value};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::oneshot,
    time::timeout,
};

use super::{
    BinanceSpotAdapter, BinanceSpotOhlcv, SpotInventoryError, SpotOrderRef, SpotPreparedRead,
    SpotReadBody, SpotReadError, SpotReadRequest, SpotRestReader, SPOT_COMMISSION_BODY_CAP,
    SPOT_OPERATED_SYMBOL,
};
use crate::binance_rest::{BinanceRestClient, BinanceRestError};

const ACCOUNT: &str = include_str!("testdata/account.json");
const OPEN_ORDERS: &str = include_str!("testdata/open_orders.json");
const OPEN_ORDER_LIST: &str = include_str!("testdata/open_order_list.json");
const EXCHANGE_INFO: &str = include_str!("testdata/exchange_info_btcusdt.json");
const KLINES: &str = include_str!("testdata/klines_btcusdt_1m.json");
const COMMISSION: &str = include_str!("testdata/commission.json");

/// The fixed key and secret of the C8 loopback fixtures, so the known
/// answers below are the inventory wire tests' own.
const KEY: &str = "c8-synthetic-key";
const SECRET: &str = "c8-synthetic-secret";
/// An owner timestamp far from the test host's clock (2027-01-15).
const STAMP: i64 = 1_800_000_000_000;
/// HMAC-SHA256 of `timestamp=<STAMP>` under `SECRET` (a known answer).
const SIGNATURE: &str = "5240291be8a1f9092fc18756b920e57afd279112d735c2e1100014969556c238";
/// HMAC-SHA256 of `symbol=BTCUSDT&timestamp=<STAMP>` under `SECRET`.
const SCOPED_SIGNATURE: &str = "b0b44268eb1b217beeddf7bb2bed259bee1f314cc685a60b40e0e78a262d7edf";
/// The first kline's open time in `klines_btcusdt_1m.json`.
const KLINE_START: u64 = 1_794_435_960_000;
const REFUSED: &str = "budgeted Spot client: unbudgeted request refused, nothing sent";

const SIGNED: [SpotPreparedRead; 3] = [
    SpotPreparedRead::Account,
    SpotPreparedRead::OperatedOpenOrders,
    SpotPreparedRead::OpenOrderLists,
];
const PUBLIC: [SpotPreparedRead; 2] = [SpotPreparedRead::ExchangeInfo, SpotPreparedRead::Kline {
    start_ms: KLINE_START,
}];

const STEP: Duration = Duration::from_secs(5);
const QUIET: Duration = Duration::from_millis(250);

async fn bounded<F: Future>(test: F) -> F::Output {
    timeout(Duration::from_secs(60), test).await.expect("outer test deadline")
}

fn client(url: String) -> Arc<BinanceRestClient> {
    Arc::new(BinanceRestClient::with_local_base_url_and_credentials(url, KEY, SECRET))
}

async fn local() -> (TcpListener, String) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    (listener, url)
}

async fn read_head(stream: &mut TcpStream) -> String {
    let mut head = Vec::new();
    let mut buf = [0_u8; 1024];
    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
        let n = stream.read(&mut buf).await.unwrap();
        assert!(n > 0 && head.len() + n <= 8192, "incomplete test header");
        head.extend_from_slice(&buf[..n]);
    }
    String::from_utf8(head).unwrap()
}

async fn assert_quiet(listener: &TcpListener) {
    assert!(timeout(QUIET, listener.accept()).await.is_err(), "no further connection");
}

/// A venue counting every connection it accepts and keeping each request's
/// head; every request is answered `200 {}` and closed.
async fn counting() -> (String, Arc<AtomicUsize>, Arc<Mutex<Vec<String>>>) {
    let (listener, url) = local().await;
    let connections = Arc::new(AtomicUsize::new(0));
    let heads = Arc::new(Mutex::new(Vec::new()));
    let (counted, kept) = (Arc::clone(&connections), Arc::clone(&heads));
    tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            counted.fetch_add(1, Ordering::SeqCst);
            let head = read_head(&mut stream).await;
            kept.lock().unwrap().push(head.lines().next().unwrap().to_string());
            let answer = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}";
            stream.write_all(answer.as_bytes()).await.unwrap();
        }
    });
    (url, connections, heads)
}

/// The one request `bind` makes of `read` on a fresh one-shot listener,
/// answered `200 {}`: its head, after which nothing else connects.
async fn one_request(
    read: SpotPreparedRead,
    bind: impl FnOnce(super::PreparedSpotRead) -> Result<SpotReadRequest, BinanceRestError>,
) -> String {
    let (listener, url) = local().await;
    let request = bind(client(url).prepare_spot_read(read).unwrap()).unwrap();
    let venue = async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let head = read_head(&mut stream).await;
        let answer = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}";
        stream.write_all(answer.as_bytes()).await.unwrap();
        drop(stream);
        assert_quiet(&listener).await;
        head
    };
    let receive = async {
        let response = request.send().await.unwrap();
        assert_eq!(response.status().as_u16(), 200);
        assert_eq!(response.bytes().await.unwrap().as_ref(), b"{}");
    };
    let (head, ()) = timeout(STEP, async { tokio::join!(venue, receive) }).await.unwrap();
    head
}

fn hmac(query: &str) -> String {
    use hmac::{Hmac, Mac};
    let mut mac = Hmac::<sha2::Sha256>::new_from_slice(SECRET.as_bytes()).unwrap();
    mac.update(query.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

/// A signed read's head: one GET of its fixed parameters and `stamp`, the
/// key once, no body, never the secret. Returns the request line.
fn checked_signed_head(head: &str) -> &str {
    let lower = head.to_ascii_lowercase();
    assert_eq!(lower.matches("\r\nx-mbx-apikey: ").count(), 1, "{head}");
    assert!(lower.contains(&format!("\r\nx-mbx-apikey: {KEY}\r\n")), "{head}");
    assert!(!lower.contains("content-length"), "a GET carries no body: {head}");
    assert!(!lower.contains(SECRET), "the secret never goes on the wire");
    head.lines().next().unwrap()
}

/// The documented open order moved onto the operated pair.
fn operated_open_orders() -> String {
    let mut orders: Value = serde_json::from_str(OPEN_ORDERS).unwrap();
    for order in orders.as_array_mut().unwrap() {
        order["symbol"] = json!(SPOT_OPERATED_SYMBOL);
    }
    orders.to_string()
}

/// `body` padded with JSON whitespace to exactly `len` bytes.
fn padded(body: &str, len: usize) -> Vec<u8> {
    let mut bytes = body.as_bytes().to_vec();
    assert!(bytes.len() <= len, "{} > {len}", bytes.len());
    bytes.resize(len, b' ');
    bytes
}

fn btc() -> Symbol {
    Symbol::from_pair("BTCUSDT").unwrap()
}

#[test]
fn the_closed_read_set_has_fixed_paths_queries_weights_and_caps() {
    let reads = [SIGNED[0], SIGNED[1], SIGNED[2], PUBLIC[0], PUBLIC[1]];
    assert_eq!(reads.map(|read| read.request().path), [
        "/api/v3/account",
        "/api/v3/openOrders",
        "/api/v3/openOrderList",
        "/api/v3/exchangeInfo",
        "/api/v3/klines",
    ]);
    assert_eq!(reads.map(SpotPreparedRead::query), [
        None,
        Some("symbol=BTCUSDT".to_string()),
        None,
        Some("symbol=BTCUSDT".to_string()),
        Some(format!("symbol=BTCUSDT&interval=1m&startTime={KLINE_START}&limit=1")),
    ]);
    assert_eq!(reads.map(|read| read.request().weight), [20, 6, 6, 20, 2]);
    assert_eq!(reads.map(|read| read.request().signed), [true, true, true, false, false]);
    assert_eq!(reads.map(SpotPreparedRead::body_cap), [
        1024 * 1024,
        256 * 1024,
        1024 * 1024,
        256 * 1024,
        16 * 1024,
    ]);
    assert_eq!(SPOT_COMMISSION_BODY_CAP, 16 * 1024);
    // 8k-1 transport B: 32 weight per attempt, at most 96 per 3-attempt scan.
    let triple: u32 = SIGNED.iter().map(|read| read.request().weight).sum();
    assert_eq!((triple, 3 * triple), (32, 96));
}

/// Each signed read carries the owner's timestamp, never the host clock's:
/// the exact URI with the known-answer signature, and for another stamp the
/// signature of exactly the query sent. A non-positive stamp is refused with
/// nothing sent.
#[tokio::test]
async fn signed_reads_carry_the_owner_timestamp() {
    bounded(async {
        for read in SIGNED {
            let head = one_request(read, |prepared| prepared.sign(STAMP)).await;
            let line = checked_signed_head(&head);
            let expected = match read.query() {
                None => {
                    format!(
                        "GET {}?timestamp={STAMP}&signature={SIGNATURE} HTTP/1.1",
                        read.request().path
                    )
                },
                Some(fixed) => format!(
                    "GET {}?{fixed}&timestamp={STAMP}&signature={SCOPED_SIGNATURE} HTTP/1.1",
                    read.request().path
                ),
            };
            assert_eq!(line, expected, "{read:?}");

            let other = 1_234_567_i64;
            let head = one_request(read, |prepared| prepared.sign(other)).await;
            let query = match read.query() {
                None => format!("timestamp={other}"),
                Some(fixed) => format!("{fixed}&timestamp={other}"),
            };
            let expected =
                format!("GET {}?{query}&signature={} HTTP/1.1", read.request().path, hmac(&query));
            assert_eq!(checked_signed_head(&head), expected, "{read:?}");
        }

        let (listener, url) = local().await;
        let client = client(url);
        for read in SIGNED {
            for stamp in [0, -1, i64::MIN] {
                match client.prepare_spot_read(read).unwrap().sign(stamp) {
                    Err(BinanceRestError::Refused(detail)) => assert_eq!(
                        detail,
                        "a signed read at a positive owner timestamp only, nothing sent"
                    ),
                    _ => panic!("{read:?} at {stamp} must be refused"),
                }
            }
        }
        assert_quiet(&listener).await;
    })
    .await;
}

/// 8k-1 transport B: the open orders are read for BTCUSDT only (weight 6),
/// and an answer holding another pair's order is refused whole.
#[tokio::test]
async fn operated_open_orders_is_scoped_to_btcusdt() {
    bounded(async {
        let read = SpotPreparedRead::OperatedOpenOrders;
        let head = one_request(read, |prepared| prepared.sign(STAMP)).await;
        assert_eq!(
            checked_signed_head(&head),
            format!(
                "GET /api/v3/openOrders?symbol=BTCUSDT&timestamp={STAMP}&signature={SCOPED_SIGNATURE} HTTP/1.1"
            )
        );
        assert_eq!(read.request().weight, 6);

        let operated = operated_open_orders();
        let Ok(SpotReadBody::OpenOrders(orders)) = read.decode(operated.as_bytes()) else {
            panic!("the operated pair's orders decode");
        };
        assert_eq!(orders.len(), 1);
        assert_eq!(orders[0].symbol, "BTCUSDT");
        let foreign: Value = serde_json::from_str(OPEN_ORDERS).unwrap();
        let ours: Value = serde_json::from_str(&operated).unwrap();
        for body in [foreign.to_string(), json!([ours[0], foreign[0]]).to_string()] {
            assert_eq!(read.decode(body.as_bytes()).unwrap_err(), SpotInventoryError::OutsideScope);
        }
    })
    .await;
}

/// The public reads carry their fixed parameters only: no key, no
/// timestamp, no signature. A signed read cannot go unsigned, a public one
/// takes no timestamp.
#[tokio::test]
async fn public_reads_carry_no_key_and_no_timestamp() {
    bounded(async {
        let mut lines = Vec::new();
        for read in PUBLIC {
            let head = one_request(read, |prepared| prepared.unsigned()).await;
            let lower = head.to_ascii_lowercase();
            for absent in ["x-mbx-apikey", "timestamp", "signature", KEY, SECRET] {
                assert!(!lower.contains(absent), "{read:?} carries {absent}: {head}");
            }
            lines.push(head.lines().next().unwrap().to_string());
        }
        assert_eq!(lines, [
            "GET /api/v3/exchangeInfo?symbol=BTCUSDT HTTP/1.1".to_string(),
            format!(
                "GET /api/v3/klines?symbol=BTCUSDT&interval=1m&startTime={KLINE_START}&limit=1 HTTP/1.1"
            ),
        ]);

        let (listener, url) = local().await;
        let client = client(url);
        let refused = |result: Result<SpotReadRequest, BinanceRestError>| {
            matches!(result, Err(BinanceRestError::Refused(detail)) if detail.ends_with("nothing sent"))
        };
        for read in PUBLIC {
            assert!(refused(client.prepare_spot_read(read).unwrap().sign(STAMP)), "{read:?}");
        }
        for read in SIGNED {
            assert!(refused(client.prepare_spot_read(read).unwrap().unsigned()), "{read:?}");
        }
        assert_quiet(&listener).await;
    })
    .await;
}

/// ACCOUNT's cap is exactly 1 MiB: a valid body of that size decodes, one
/// byte more is refused before any parse (never read as an empty account).
/// Every other read keeps its own cap the same way.
#[test]
fn an_account_body_over_its_cap_is_refused() {
    let account = SpotPreparedRead::Account;
    let cap = account.body_cap();
    assert_eq!(cap, 1024 * 1024);
    let Ok(SpotReadBody::Account(decoded)) = account.decode(&padded(ACCOUNT, cap)) else {
        panic!("a body at the cap decodes");
    };
    assert_eq!(decoded.uid, 354937868);
    assert_eq!(
        account.decode(&padded(ACCOUNT, cap + 1)).unwrap_err(),
        SpotInventoryError::Oversize { len: cap + 1, limit: cap }
    );

    let operated = operated_open_orders();
    let bodies = [
        (SpotPreparedRead::OperatedOpenOrders, operated.as_str()),
        (SpotPreparedRead::OpenOrderLists, OPEN_ORDER_LIST),
        (SpotPreparedRead::ExchangeInfo, EXCHANGE_INFO),
        (SpotPreparedRead::Kline { start_ms: KLINE_START }, KLINES),
    ];
    for (read, body) in bodies {
        let cap = read.body_cap();
        assert!(read.decode(&padded(body, cap)).is_ok(), "{read:?} at its cap");
        assert_eq!(
            read.decode(&padded(body, cap + 1)).unwrap_err(),
            SpotInventoryError::Oversize { len: cap + 1, limit: cap },
            "{read:?}"
        );
    }
}

/// Each read decodes its documented body into its own typed record; a body
/// of another shape, or not UTF-8, is refused whole.
#[test]
fn each_read_decodes_its_own_body_only() {
    let operated = operated_open_orders();
    match SpotPreparedRead::Account.decode(ACCOUNT.as_bytes()).unwrap() {
        SpotReadBody::Account(account) => assert_eq!(account.balances.len(), 2),
        other => panic!("{other:?}"),
    }
    match SpotPreparedRead::OpenOrderLists.decode(OPEN_ORDER_LIST.as_bytes()).unwrap() {
        SpotReadBody::OpenOrderLists(lists) => assert_eq!(lists[0].order_list_id, 31),
        other => panic!("{other:?}"),
    }
    match SpotPreparedRead::ExchangeInfo.decode(EXCHANGE_INFO.as_bytes()).unwrap() {
        SpotReadBody::Rules(rules) => assert_eq!(rules.symbol, "BTCUSDT"),
        other => panic!("{other:?}"),
    }
    let kline = SpotPreparedRead::Kline { start_ms: KLINE_START };
    match kline.decode(KLINES.as_bytes()).unwrap() {
        SpotReadBody::Kline(rows) => assert_eq!(rows, KLINES),
        other => panic!("{other:?}"),
    }
    let other_pair = EXCHANGE_INFO.replace("BTCUSDT", "ETHUSDT");
    for (read, body) in [
        (SpotPreparedRead::Account, operated.as_str()),
        (SpotPreparedRead::OperatedOpenOrders, ACCOUNT),
        (SpotPreparedRead::OpenOrderLists, ACCOUNT),
        (SpotPreparedRead::ExchangeInfo, other_pair.as_str()),
        (SpotPreparedRead::ExchangeInfo, ACCOUNT),
    ] {
        assert!(read.decode(body.as_bytes()).is_err(), "{read:?} admits another body");
    }
    for read in [SIGNED[0], SIGNED[1], SIGNED[2], PUBLIC[0], PUBLIC[1]] {
        assert_eq!(
            read.decode(&[b'[', 0xff, b']']).unwrap_err(),
            SpotInventoryError::Malformed("not UTF-8".into())
        );
    }
}

/// Acceptance evidence for the production caps (PLAN §6.12.6): each holds a
/// plausible full answer at its size, measured here and printed. ACCOUNT
/// with 3,000 assets (Binance lists every asset, a few hundred); 200 open
/// orders of the operated pair (the venue's `MAX_NUM_ORDERS`) in the
/// documented shape with every optional field, and in a plain LIMIT shape;
/// 500 open order lists of three members; the operated pair's rules; one
/// kline; the commission answer.
#[test]
fn each_cap_holds_a_plausible_full_answer() {
    let assets: Vec<Value> = (0..3_000)
        .map(|i| json!({"asset": format!("A{i:07}"), "free": "1234567.12345678", "locked": "1234567.12345678"}))
        .collect();
    let mut account: Value = serde_json::from_str(ACCOUNT).unwrap();
    account["balances"] = json!(assets);
    let full: Value = serde_json::from_str(&operated_open_orders()).unwrap();
    let full_orders: Vec<Value> = (0..200)
        .map(|i| {
            let mut order = full[0].clone();
            order["orderId"] = json!(10_000_000_000_u64 + i);
            order["clientOrderId"] = json!(format!("web_{i:024}"));
            order
        })
        .collect();
    let plain_orders: Vec<Value> = (0..200)
        .map(|i| {
            json!({"symbol": "BTCUSDT", "orderId": 10_000_000_000_u64 + i, "orderListId": -1,
                "clientOrderId": format!("web_{i:024}"), "price": "74000.00000000",
                "origQty": "0.00100000", "executedQty": "0.00000000",
                "origQuoteOrderQty": "0.00000000", "cummulativeQuoteQty": "0.00000000",
                "status": "NEW", "timeInForce": "GTC", "type": "LIMIT", "side": "BUY",
                "stopPrice": "0.00000000", "icebergQty": "0.00000000", "time": KLINE_START,
                "updateTime": KLINE_START, "isWorking": true, "workingTime": KLINE_START,
                "selfTradePreventionMode": "EXPIRE_MAKER"})
        })
        .collect();
    let lists: Vec<Value> = (0..500)
        .map(|i| {
            let symbol = format!("S{i:05}USDT");
            let members: Vec<Value> = (0..3)
                .map(|m| json!({"symbol": symbol, "orderId": 3 * i + m, "clientOrderId": format!("c{i}m{m}")}))
                .collect();
            json!({"orderListId": i, "contingencyType": "OCO", "listStatusType": "EXEC_STARTED",
                "listOrderStatus": "EXECUTING", "listClientOrderId": format!("list{i:018}"),
                "transactionTime": KLINE_START, "symbol": symbol, "orders": members})
        })
        .collect();
    let rows: Value = serde_json::from_str(KLINES).unwrap();
    let kline = SpotPreparedRead::Kline { start_ms: KLINE_START };
    let answers = [
        ("account_3000_assets", SpotPreparedRead::Account, account.to_string()),
        (
            "open_orders_200_full",
            SpotPreparedRead::OperatedOpenOrders,
            json!(full_orders).to_string(),
        ),
        (
            "open_orders_200_limit",
            SpotPreparedRead::OperatedOpenOrders,
            json!(plain_orders).to_string(),
        ),
        ("order_lists_500x3", SpotPreparedRead::OpenOrderLists, json!(lists).to_string()),
        (
            "exchange_info_btcusdt",
            SpotPreparedRead::ExchangeInfo,
            EXCHANGE_INFO.to_string(),
        ),
        ("kline_1", kline, json!([rows[0]]).to_string()),
    ];
    for (name, read, body) in answers {
        assert!(read.decode(body.as_bytes()).is_ok(), "{name} decodes");
        let (len, cap) = (body.len(), read.body_cap());
        println!("PS4_CAP {name} bytes={len} cap={cap} headroom_pct={}", cap * 100 / len);
        assert!(len * 3 / 2 <= cap, "{name}: {len} bytes leave under 1.5x headroom in {cap}");
    }
    let commission = COMMISSION.len();
    println!(
        "PS4_CAP commission bytes={commission} cap={SPOT_COMMISSION_BODY_CAP} headroom_pct={}",
        SPOT_COMMISSION_BODY_CAP * 100 / commission
    );
    assert!(commission * 3 <= SPOT_COMMISSION_BODY_CAP);
}

/// The closed client: every legacy sender, USD-M's included, and every
/// legacy Spot consumer over it (reader, klines, venue) refuses with nothing
/// sent, not even a connection, measured or not; the prepared, budgeted
/// reads (the closed set, TIME, commission) still go, one request each.
#[tokio::test]
async fn a_closed_client_sends_nothing_unbudgeted() {
    bounded(async {
        let (url, connections, heads) = counting().await;
        let client = client(url);
        assert!(!client.is_closed());
        client.close_unbudgeted_sends();
        client.close_unbudgeted_sends();
        assert!(client.is_closed());

        let refused = |name: &str, result: Result<(), BinanceRestError>| match result {
            Err(BinanceRestError::Refused(detail)) => assert_eq!(detail, REFUSED, "{name}"),
            other => panic!("{name}: {other:?}"),
        };
        refused("ping", client.ping().await);
        refused("futures balance", client.get_futures_balance().await.map(drop));
        refused("set leverage", client.set_leverage("BTCUSDT", 1).await);
        refused("cancel", client.cancel_order("BTCUSDT", 1).await.map(drop));
        refused("spot balances", client.get_spot_account_balances().await.map(drop));
        refused(
            "spot market order",
            client
                .place_spot_market_order("BTCUSDT", "BUY", ("quantity", dec!(1)), "ps4")
                .await
                .map(drop),
        );
        refused(
            "transfer",
            client
                .universal_transfer("USDT", dec!(1), "MAIN_UMFUTURE", "ps4")
                .await
                .map(drop),
        );
        refused(
            "public answer",
            client.get_public_answer("/api/v3/ping", vec![]).await.map(drop),
        );
        refused(
            "signed answer",
            client.get_signed_answer("/api/v3/account", vec![]).await.map(drop),
        );
        let measured = robson_exec::observation::reconciliation(
            client.get_signed_answer("/fapi/v2/positionRisk", vec![]),
        );
        refused("measured signed answer", measured.await.map(drop));
        refused(
            "spot public",
            client.get_public_spot_answer("/api/v3/ping", vec![]).await.map(drop),
        );
        refused(
            "spot signed",
            client.get_signed_spot_answer("/api/v3/account", vec![]).await.map(drop),
        );
        refused("spot post", client.post_signed_answer("/api/v3/order", vec![]).await.map(drop));
        refused(
            "spot delete",
            client.delete_signed_answer("/api/v3/order", vec![]).await.map(drop),
        );

        let reader = SpotRestReader::new(Arc::clone(&client));
        let read_refused = |name: &str, error: SpotReadError| {
            assert_eq!(
                error,
                SpotReadError::Transport(format!("Refused before sending: {REFUSED}")),
                "{name}"
            )
        };
        read_refused("reader ping", reader.ping().await.unwrap_err());
        read_refused("reader account", reader.account().await.unwrap_err());
        read_refused("reader open orders", reader.open_orders(&btc()).await.unwrap_err());
        read_refused("reader lists", reader.open_order_lists().await.unwrap_err());
        read_refused("reader rules", reader.symbol_rules(&btc()).await.unwrap_err());
        read_refused("reader commission", reader.commission(&btc()).await.unwrap_err());
        let order = reader.order(&btc(), &SpotOrderRef::OrderId(1)).await;
        read_refused("reader order", order.unwrap_err());
        let ohlcv = BinanceSpotOhlcv::new(Arc::clone(&client)).unwrap();
        let candles = ohlcv.fetch_candles(&btc(), CandleInterval::OneMinute, 1).await;
        let venue = BinanceSpotAdapter::non_trading(Arc::clone(&client)).unwrap();
        let errors: Vec<ExecError> = vec![
            candles.unwrap_err(),
            venue.health().await.unwrap_err(),
            venue.price(&btc()).await.unwrap_err(),
            venue.trading_rules(&btc()).await.unwrap_err(),
            venue.cash("BTC").await.unwrap_err(),
            venue.account_observation().await.unwrap_err(),
            venue.cancel_order(&btc(), "1").await.unwrap_err(),
            venue.cancel_order_once(&btc(), "1").await.unwrap_err(),
        ];
        for error in errors {
            assert!(error.to_string().contains(REFUSED), "{error}");
        }
        assert!(matches!(
            venue.cancel_order_once(&btc(), "1").await,
            Err(ExecError::SendRefused(detail)) if detail == REFUSED
        ));
        assert_eq!(connections.load(Ordering::SeqCst), 0, "nothing unbudgeted connected");

        for read in SIGNED {
            let request = client.prepare_spot_read(read).unwrap().sign(STAMP).unwrap();
            assert_eq!(request.send().await.unwrap().status().as_u16(), 200, "{read:?}");
        }
        for read in PUBLIC {
            let request = client.prepare_spot_read(read).unwrap().unsigned().unwrap();
            assert_eq!(request.send().await.unwrap().status().as_u16(), 200, "{read:?}");
        }
        let time = client.prepare_spot_time().unwrap().send().await.unwrap();
        assert_eq!(time.status().as_u16(), 200);
        let commission = client.prepare_spot_commission(&btc()).unwrap().sign(STAMP).unwrap();
        assert_eq!(commission.send().await.unwrap().status().as_u16(), 200);
        assert_eq!(connections.load(Ordering::SeqCst), 7, "one connection per budgeted read");
        let paths: Vec<String> = heads
            .lock()
            .unwrap()
            .iter()
            .map(|line| line.split(['?', ' ']).take(2).collect::<Vec<_>>().join(" "))
            .collect();
        assert_eq!(paths, [
            "GET /api/v3/account",
            "GET /api/v3/openOrders",
            "GET /api/v3/openOrderList",
            "GET /api/v3/exchangeInfo",
            "GET /api/v3/klines",
            "GET /api/v3/time",
            "GET /api/v3/account/commission",
        ]);
    })
    .await;
}

/// `non_trading` closes the very client it wraps, before anything can use
/// it: the venue's reads and orders are refused with nothing sent. The
/// ordinary constructor leaves its client open, and a testnet client is
/// refused, closed all the same.
#[tokio::test]
async fn the_non_trading_venue_closes_its_client_before_wrapping_it() {
    bounded(async {
        let (url, connections, _) = counting().await;
        let open = client(url.clone());
        let ordinary = BinanceSpotAdapter::new(Arc::clone(&open)).unwrap();
        assert!(!open.is_closed());
        ordinary.health().await.unwrap();
        assert_eq!(connections.load(Ordering::SeqCst), 1);

        let shared = client(url);
        let venue = BinanceSpotAdapter::non_trading(Arc::clone(&shared)).unwrap();
        assert!(shared.is_closed());
        for error in [
            venue.health().await.unwrap_err(),
            venue.cash("USDT").await.unwrap_err(),
            venue.cancel_order(&btc(), "7").await.unwrap_err(),
        ] {
            assert!(error.to_string().contains(REFUSED), "{error}");
        }
        assert_eq!(connections.load(Ordering::SeqCst), 1, "the non-trading venue sent nothing");
        assert!(!open.is_closed(), "closing one client closes no other");

        let testnet = Arc::new(BinanceRestClient::testnet("key".into(), "secret".into()));
        assert!(matches!(
            BinanceSpotAdapter::non_trading(Arc::clone(&testnet)),
            Err(ExecError::Config(_))
        ));
        assert!(testnet.is_closed());
    })
    .await;
}

/// One physical request whatever the venue does: a redirect is the answer
/// and is not followed; a connection closed early, reset, or closed before
/// the request was read is a failed read, never sent again; a send dropped
/// unpolled sends nothing.
#[tokio::test]
async fn a_prepared_read_is_one_physical_request() {
    bounded(async {
        for status in [301_u16, 302, 303, 307, 308] {
            let (elsewhere, target) = local().await;
            let (listener, url) = local().await;
            let request = client(url)
                .prepare_spot_read(SpotPreparedRead::Account)
                .unwrap()
                .sign(STAMP)
                .unwrap();
            let venue = async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                read_head(&mut stream).await;
                let answer = format!(
                    "HTTP/1.1 {status} Moved\r\nLocation: {target}/api/v3/account\r\n\
                     Content-Length: 0\r\nConnection: close\r\n\r\n"
                );
                stream.write_all(answer.as_bytes()).await.unwrap();
                drop(stream);
                assert_quiet(&listener).await;
            };
            let receive = async {
                assert_eq!(request.send().await.unwrap().status().as_u16(), status);
            };
            timeout(STEP, async { tokio::join!(venue, receive) }).await.unwrap();
            assert_quiet(&elsewhere).await;
        }

        // 0: closed before the request is read; 1: closed after its head,
        // unanswered; 2: reset after its head (an abortive close).
        for ending in 0..3 {
            let (listener, url) = local().await;
            let request = client(url)
                .prepare_spot_read(SpotPreparedRead::ExchangeInfo)
                .unwrap()
                .unsigned()
                .unwrap();
            let venue = async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                if ending > 0 {
                    read_head(&mut stream).await;
                }
                if ending == 2 {
                    stream.set_zero_linger().unwrap();
                }
                drop(stream);
                assert_quiet(&listener).await;
            };
            let receive = async {
                assert!(request.send().await.is_err(), "ending {ending} is a failed read");
            };
            timeout(STEP, async { tokio::join!(venue, receive) }).await.unwrap();
        }

        let (listener, url) = local().await;
        let client = client(url);
        for read in SIGNED {
            drop(client.prepare_spot_read(read).unwrap().sign(STAMP).unwrap().send());
        }
        for read in PUBLIC {
            drop(client.prepare_spot_read(read).unwrap().unsigned().unwrap().send());
        }
        assert_quiet(&listener).await;
    })
    .await;
}

/// A closed client still shows each mutation to its attached gate first, as
/// an open one does, so the gate's refusals and their accounting see it; it
/// then refuses, nothing sent. A permit holder passes the gate uncounted, an
/// unpermitted mutation is counted by it, and a closing gate refuses first
/// with its own reason.
#[tokio::test]
async fn a_closed_client_shows_each_mutation_to_its_gate_first() {
    bounded(async {
        let (url, connections, _) = counting().await;
        let client = client(url);
        let journal = Arc::new(OrderJournal::unpersisted());
        client.attach_gate(Arc::clone(journal.gate()));
        client.close_unbudgeted_sends();
        let refused = |result: Result<(), BinanceRestError>| match result {
            Err(BinanceRestError::Refused(detail)) => detail,
            other => panic!("{other:?}"),
        };

        let permit = journal.acquire(OperationKind::Panic).unwrap();
        permit
            .scope(async {
                let post = client.post_signed_answer("/api/v3/order", vec![]).await;
                assert_eq!(refused(post.map(drop)), REFUSED);
            })
            .await;
        permit.complete();
        assert_eq!(journal.gate().snapshot().unpermitted, 0);

        let post = client.post_signed_answer("/api/v3/order", vec![]).await;
        assert_eq!(refused(post.map(drop)), REFUSED);
        let delete = client.delete_signed_answer("/api/v3/order", vec![]).await;
        assert_eq!(refused(delete.map(drop)), REFUSED);
        assert_eq!(refused(client.set_leverage("BTCUSDT", 1).await), REFUSED);
        assert_eq!(journal.gate().snapshot().unpermitted, 3, "the gate saw each attempt");

        journal.gate().begin_closing();
        let post = client.post_signed_answer("/api/v3/order", vec![]).await;
        let delete = client.delete_signed_answer("/api/v3/order", vec![]).await;
        for detail in [refused(post.map(drop)), refused(delete.map(drop))] {
            assert!(detail.starts_with("venue mutation without a permit refused"), "{detail}");
        }
        assert_eq!(journal.gate().snapshot().unpermitted, 3);
        assert_eq!(connections.load(Ordering::SeqCst), 0, "nothing connected");
    })
    .await;
}

/// The response returns at its head: the body is the owner's to read, under
/// its own deadline and cap, after the head has arrived.
#[tokio::test]
async fn a_prepared_read_returns_at_its_head_before_the_body() {
    bounded(async {
        let (listener, url) = local().await;
        let (release, released) = oneshot::channel();
        let venue = async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            read_head(&mut stream).await;
            let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", ACCOUNT.len());
            stream.write_all(head.as_bytes()).await.unwrap();
            released.await.unwrap();
            stream.write_all(ACCOUNT.as_bytes()).await.unwrap();
            drop(stream);
            assert_quiet(&listener).await;
        };
        let request = client(url)
            .prepare_spot_read(SpotPreparedRead::Account)
            .unwrap()
            .sign(STAMP)
            .unwrap();
        let receive = async {
            let response = request.send().await.unwrap();
            assert_eq!(response.status().as_u16(), 200);
            release.send(()).unwrap();
            let body = response.bytes().await.unwrap();
            let decoded = SpotPreparedRead::Account.decode(&body).unwrap();
            assert!(matches!(decoded, SpotReadBody::Account(account) if account.uid == 354937868));
        };
        timeout(STEP, async { tokio::join!(venue, receive) }).await.unwrap();
    })
    .await;
}

/// Cancelled once the request reached the venue, its answer withheld: the
/// one connection closes, and nothing is sent again or left behind. (An
/// HTTP/2 NACK cannot occur on this plain loopback, where reqwest speaks
/// HTTP/1.1 without ALPN; its retry is off in `single_shot`, which the
/// transport's pin and `the_answer_helpers_keep_their_single_shot_transport`
/// hold.)
#[tokio::test]
async fn a_prepared_read_cancelled_in_flight_closes_its_one_connection() {
    bounded(async {
        for read in [SpotPreparedRead::OperatedOpenOrders, SpotPreparedRead::ExchangeInfo] {
            let (listener, url) = local().await;
            let (arrived, arrival) = oneshot::channel();
            let venue = async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                let head = read_head(&mut stream).await;
                arrived.send(()).unwrap();
                let mut rest = [0_u8; 64];
                let closed = timeout(STEP, stream.read(&mut rest)).await.expect("closed");
                assert!(
                    matches!(&closed, Ok(0))
                        || matches!(&closed, Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset),
                    "connection did not retire: {closed:?}"
                );
                assert_quiet(&listener).await;
                head
            };
            let prepared = client(url).prepare_spot_read(read).unwrap();
            let request =
                if read.request().signed { prepared.sign(STAMP) } else { prepared.unsigned() };
            let cancel = async {
                let mut sending = Box::pin(request.unwrap().send());
                tokio::select! {
                    biased;
                    answered = sending.as_mut() => panic!("answered: {}", answered.is_ok()),
                    arrived = arrival => arrived.unwrap(),
                }
                drop(sending);
            };
            let (head, ()) = timeout(STEP, async { tokio::join!(venue, cancel) }).await.unwrap();
            assert!(head.starts_with(&format!("GET {}?", read.request().path)), "{head}");
        }
    })
    .await;
}
