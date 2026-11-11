//! ADR-0056 step 8a: the spot reads observed on the wire.
//!
//! Every read goes through the real REST client to a local listener standing
//! in for Binance, so these tests see the requests Robson sends and the
//! errors it types. Nothing reaches the network.

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use robson_domain::Symbol;
use rust_decimal_macros::dec;
use serde_json::{json, Value};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

use super::{
    SpotOrderRef, SpotRead, SpotReadError, SpotRestReader, SpotTradesQuery, MY_TRADES_MAX_PAGES,
    MY_TRADES_MAX_WINDOW_MS,
};
use crate::binance_rest::BinanceRestClient;

const EXCHANGE_INFO: &str = include_str!("testdata/exchange_info_btcusdt.json");
const ACCOUNT: &str = include_str!("testdata/account.json");
const ORDER: &str = include_str!("testdata/order.json");
const OPEN_ORDERS: &str = include_str!("testdata/open_orders.json");
const OPEN_ORDER_LIST: &str = include_str!("testdata/open_order_list.json");
const MY_TRADES: &str = include_str!("testdata/my_trades.json");
const COMMISSION: &str = include_str!("testdata/commission.json");
const ERRORS: &str = include_str!("testdata/errors.json");
const READS: &str = include_str!("testdata/reads.json");

/// One request as the stand-in venue saw it.
#[derive(Debug, Clone)]
struct Seen {
    method: String,
    path: String,
    params: Vec<(String, String)>,
    api_key: bool,
}

impl Seen {
    fn param(&self, name: &str) -> Option<&str> {
        self.params.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    /// Method, path and parameters without the signature's, sorted.
    fn identity(&self) -> String {
        let mut params: Vec<String> = self
            .params
            .iter()
            .filter(|(k, _)| k != "timestamp" && k != "signature")
            .map(|(k, v)| format!("{k}={v}"))
            .collect();
        params.sort();
        if params.is_empty() {
            format!("{} {}", self.method, self.path)
        } else {
            format!("{} {}?{}", self.method, self.path, params.join("&"))
        }
    }
}

type Seens = Arc<Mutex<Vec<Seen>>>;

/// A stand-in venue answering each request with `reply`'s status and body.
async fn venue(
    reply: impl Fn(&Seen) -> (u16, String) + Send + Sync + 'static,
) -> (SpotRestReader, Seens) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&seen);
    let reply = Arc::new(reply);
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
            let (method, target) =
                (first.next().unwrap_or_default(), first.next().unwrap_or_default());
            let (path, query) = target.split_once('?').unwrap_or((target, ""));
            let request = Seen {
                method: method.to_string(),
                path: path.to_string(),
                params: query
                    .split('&')
                    .filter(|part| !part.is_empty())
                    .map(|part| {
                        let (k, v) = part.split_once('=').unwrap_or((part, ""));
                        (k.to_string(), v.to_string())
                    })
                    .collect(),
                api_key: lines.any(|line| line.to_ascii_lowercase().starts_with("x-mbx-apikey:")),
            };
            recorded.lock().unwrap().push(request.clone());

            let (status, body) = reply(&request);
            let response = format!(
                "HTTP/1.1 {status} Answer\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes()).await;
        }
    });
    let client = Arc::new(BinanceRestClient::with_local_base_url(url));
    (SpotRestReader::new(client), seen)
}

/// A venue answering every request with the same status and body.
async fn answering(status: u16, body: &str) -> (SpotRestReader, Seens) {
    let body = body.to_string();
    venue(move |_| (status, body.clone())).await
}

fn symbol(pair: &str) -> Symbol {
    Symbol::from_pair(pair).unwrap()
}

fn error_body(name: &str) -> String {
    let errors: Value = serde_json::from_str(ERRORS).unwrap();
    errors[name].as_str().unwrap().to_string()
}

/// The documented weight, path and signing of every read, and that each is
/// a GET (`testdata/reads.json`).
#[tokio::test]
async fn every_read_is_a_get_with_its_documented_weight() {
    let (reader, seen) = venue(|request| {
        let body = match request.path.as_str() {
            "/api/v3/exchangeInfo" => EXCHANGE_INFO,
            "/api/v3/account" => ACCOUNT,
            "/api/v3/openOrders" => OPEN_ORDERS,
            "/api/v3/openOrderList" => OPEN_ORDER_LIST,
            "/api/v3/order" => ORDER,
            "/api/v3/myTrades" => MY_TRADES,
            "/api/v3/account/commission" => COMMISSION,
            other => panic!("unexpected read {other}"),
        };
        (200, body.to_string())
    })
    .await;
    let (btc, ltc, bnb) = (symbol("BTCUSDT"), symbol("LTCBTC"), symbol("BNBBTC"));
    let window = SpotTradesQuery::Window {
        start_ms: 1_499_865_540_000,
        end_ms: 1_499_865_560_000,
        from_id: None,
    };

    let mut weights = Vec::new();
    reader.symbol_rules(&btc).await.unwrap();
    reader.account().await.unwrap();
    reader.open_orders(&ltc).await.unwrap();
    reader.all_open_orders().await.unwrap();
    reader.open_order_list_count().await.unwrap();
    reader.order(&ltc, &SpotOrderRef::OrderId(1)).await.unwrap();
    reader
        .order(&ltc, &SpotOrderRef::ClientOrderId("myOrder1".to_string()))
        .await
        .unwrap();
    for query in [
        SpotTradesQuery::Order { order_id: 100234, from_id: 0 },
        SpotTradesQuery::Order { order_id: 100234, from_id: 28457 },
        window,
        SpotTradesQuery::Window {
            start_ms: 1_499_865_540_000,
            end_ms: 1_499_865_560_000,
            from_id: Some(28457),
        },
    ] {
        let read = reader.trades(&bnb, query).await.unwrap();
        assert_eq!((read.pages, read.trades.len(), read.next), (1, 1, None));
        weights.push(read.weight);
    }
    reader.commission(&btc).await.unwrap();

    let golden: Value = serde_json::from_str(READS).unwrap();
    let golden = golden["reads"].as_array().unwrap();
    let seen = seen.lock().unwrap().clone();
    let sent = reader.sent();
    assert_eq!((seen.len(), sent.len()), (golden.len(), golden.len()));
    for ((row, request), read) in golden.iter().zip(&seen).zip(&sent) {
        let name = row["read"].as_str().unwrap();
        assert_eq!(request.identity(), row["request"].as_str().unwrap(), "{name}");
        assert_eq!(request.method, "GET", "{name}: a read never mutates");
        assert_eq!(read.path, request.path, "{name}");
        assert_eq!(u64::from(read.weight), row["weight"].as_u64().unwrap(), "{name}");
        let signed = row["signed"].as_bool().unwrap();
        assert_eq!(read.signed, signed, "{name}");
        assert_eq!(request.api_key, signed, "{name}: API key header");
        assert_eq!(request.param("timestamp").is_some(), signed, "{name}: timestamp");
        assert_eq!(request.param("signature").is_some(), signed, "{name}: signature");
    }
    // A trades read reports the weight of the pages it sent.
    assert_eq!(weights, [5, 5, 20, 20]);
}

/// Design note v8 §3.14: one observation (account, every open order, open
/// order lists) weighs 106.
#[test]
fn an_observation_weighs_one_hundred_and_six() {
    let observation = [
        SpotRead::ACCOUNT,
        SpotRead::OPEN_ORDERS_ALL,
        SpotRead::OPEN_ORDER_LISTS,
    ];
    assert_eq!(observation.iter().map(|read| read.weight).sum::<u32>(), 106);
    assert_eq!(SpotRead::OPEN_ORDERS_SYMBOL.path, SpotRead::OPEN_ORDERS_ALL.path);
    assert_eq!(SpotRead::MY_TRADES.path, SpotRead::MY_TRADES_BY_ORDER.path);
}

async fn lookup_error(status: u16, body: &str) -> SpotReadError {
    let (reader, seen) = answering(status, body).await;
    let error = reader.order(&symbol("BTCUSDT"), &SpotOrderRef::OrderId(7)).await.unwrap_err();
    assert_eq!(seen.lock().unwrap().len(), 1, "one request, no retry");
    error
}

/// `-2013`, `-2026`, `-1007`, 5XX, the status and a timeout each stay
/// distinguishable. The statuses are this test's choice: Binance documents
/// 4XX as the sender's issue and 5XX as unknown (rest-api.md L132-L143), not
/// which code arrives with which status.
#[tokio::test]
async fn errors_stay_raw_and_typed_with_their_status() {
    let not_found = lookup_error(400, &error_body("no_such_order")).await;
    assert_eq!(not_found, SpotReadError::NoSuchOrder {
        http_status: 400,
        msg: "Order does not exist.".to_string(),
    });
    let archived = lookup_error(400, &error_body("order_archived")).await;
    assert!(matches!(archived, SpotReadError::OrderArchived { http_status: 400, .. }));
    // The code decides before the status.
    for status in [400, 503] {
        let backend = lookup_error(status, &error_body("backend_timeout")).await;
        assert!(
            matches!(backend, SpotReadError::BackendTimeout { http_status, .. } if http_status == status),
            "{backend:?}"
        );
    }
    assert_eq!(lookup_error(500, &error_body("unknown")).await, SpotReadError::ServerError {
        http_status: 500,
        code: Some(-1000),
        msg: "An unknown error occurred while processing the request.".to_string(),
    });
    assert_eq!(lookup_error(502, &error_body("html_page")).await, SpotReadError::ServerError {
        http_status: 502,
        code: None,
        msg: error_body("html_page"),
    });
    assert_eq!(lookup_error(403, &error_body("html_page")).await, SpotReadError::Http {
        http_status: 403,
        body: error_body("html_page"),
    });
    assert!(matches!(
        lookup_error(429, &error_body("too_many_requests")).await,
        SpotReadError::Api { http_status: 429, code: -1003, .. }
    ));
    // The same not-found code under another status keeps that status.
    assert!(matches!(
        lookup_error(404, &error_body("no_such_order")).await,
        SpotReadError::NoSuchOrder { http_status: 404, .. }
    ));

    let (reader, _) = answering(400, &error_body("bad_symbol")).await;
    assert!(matches!(
        reader.symbol_rules(&symbol("BTCUSDT")).await,
        Err(SpotReadError::Api { http_status: 400, code: -1121, .. })
    ));
}

#[tokio::test]
async fn an_unanswered_read_is_a_timeout_and_a_refused_connection_is_transport() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let mut held = Vec::new();
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            held.push(stream);
        }
    });
    let client = BinanceRestClient::with_local_base_url(url)
        .with_request_timeout(Duration::from_millis(200));
    let reader = SpotRestReader::new(Arc::new(client));
    assert_eq!(
        reader.order(&symbol("BTCUSDT"), &SpotOrderRef::OrderId(7)).await,
        Err(SpotReadError::Timeout)
    );

    let closed = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", closed.local_addr().unwrap());
    drop(closed);
    let reader = SpotRestReader::new(Arc::new(BinanceRestClient::with_local_base_url(url)));
    assert!(matches!(reader.account().await, Err(SpotReadError::Transport(_))));
}

#[tokio::test]
async fn a_success_body_off_the_documented_shape_is_a_parse_error() {
    let (reader, _) = answering(200, "{\"balances\": 7}").await;
    assert!(matches!(reader.account().await, Err(SpotReadError::Parse(_))));
    let (reader, _) = answering(200, "not json").await;
    assert!(matches!(
        reader.open_orders(&symbol("BTCUSDT")).await,
        Err(SpotReadError::Parse(_))
    ));
}

#[tokio::test]
async fn an_order_lookup_returns_the_record_it_was_asked_for() {
    let (reader, _) = answering(200, ORDER).await;
    let order = reader.order(&symbol("LTCBTC"), &SpotOrderRef::OrderId(1)).await.unwrap();
    assert_eq!((order.order_id, order.status.as_str()), (1, "NEW"));

    // Another orderId, or another symbol, is not the order asked for.
    assert!(matches!(
        reader.order(&symbol("LTCBTC"), &SpotOrderRef::OrderId(2)).await,
        Err(SpotReadError::Inconsistent(_))
    ));
    assert!(matches!(
        reader.order(&symbol("BTCUSDT"), &SpotOrderRef::OrderId(1)).await,
        Err(SpotReadError::Inconsistent(_))
    ));

    // By client id the record comes back as the venue gives it, even under
    // another client id (F12): what that means is 8c's to decide.
    let order = reader
        .order(&symbol("LTCBTC"), &SpotOrderRef::ClientOrderId("renamed".to_string()))
        .await
        .unwrap();
    assert_eq!(order.client_order_id, "myOrder1");
}

#[tokio::test]
async fn open_orders_may_be_empty_and_stay_on_their_symbol() {
    let (reader, _) = answering(200, "[]").await;
    assert!(reader.open_orders(&symbol("BTCUSDT")).await.unwrap().is_empty());
    assert!(reader.all_open_orders().await.unwrap().is_empty());
    assert_eq!(reader.open_order_list_count().await.unwrap(), 0);

    let (reader, _) = answering(200, OPEN_ORDERS).await;
    assert!(matches!(
        reader.open_orders(&symbol("BTCUSDT")).await,
        Err(SpotReadError::Inconsistent(_))
    ));
    // Every symbol: whatever the account holds.
    assert_eq!(reader.all_open_orders().await.unwrap()[0].symbol, "LTCBTC");
}

#[tokio::test]
async fn commission_is_read_for_its_symbol() {
    let (reader, _) = answering(200, COMMISSION).await;
    let commission = reader.commission(&symbol("BTCUSDT")).await.unwrap();
    assert_eq!(commission.standard.taker, dec!(0.0000002));
    assert!(matches!(
        reader.commission(&symbol("ETHUSDT")).await,
        Err(SpotReadError::Inconsistent(_))
    ));
}

// =============================================================================
// myTrades paging
// =============================================================================

const BASE_TIME: i64 = 1_700_000_000_000;

/// A trade from the documented record, changing only id, order and time.
fn trade(id: u64, order_id: u64, time: i64) -> Value {
    let mut trade = serde_json::from_str::<Value>(MY_TRADES).unwrap()[0].clone();
    trade["id"] = json!(id);
    trade["orderId"] = json!(order_id);
    trade["time"] = json!(time);
    trade
}

/// `count` trades of order 9 on BNBBTC, ids from 1, one second apart.
fn history(count: u64) -> Vec<Value> {
    (1..=count).map(|id| trade(id, 9, BASE_TIME + id as i64 * 1000)).collect()
}

/// Binance's `myTrades` over `trades`, held in id order, as documented
/// (rest-api.md L98-L102, L4754): filtered by `orderId`, and by
/// `startTime`/`endTime`; then the oldest `limit` from `fromId` (inclusive) or
/// from `startTime`, otherwise the most recent `limit`; chronological either
/// way. `overlap` serves each `fromId` page from one id earlier, as a venue
/// repeating a trade would.
fn my_trades(trades: Vec<Value>, overlap: bool) -> impl Fn(&Seen) -> (u16, String) {
    move |request| {
        let number = |name: &str| request.param(name).map(|v| v.parse::<i64>().unwrap());
        let from_id = number("fromId").map(|id| if overlap { id - 1 } else { id });
        let limit = number("limit").unwrap() as usize;
        let matching: Vec<&Value> = trades
            .iter()
            .filter(|t| number("orderId").is_none_or(|id| t["orderId"].as_i64() == Some(id)))
            .filter(|t| from_id.is_none_or(|id| t["id"].as_i64().unwrap() >= id))
            .filter(|t| number("startTime").is_none_or(|ms| t["time"].as_i64().unwrap() >= ms))
            .filter(|t| number("endTime").is_none_or(|ms| t["time"].as_i64().unwrap() <= ms))
            .collect();
        let page = if from_id.is_some() || number("startTime").is_some() {
            &matching[..limit.min(matching.len())]
        } else {
            &matching[matching.len().saturating_sub(limit)..]
        };
        (200, serde_json::to_string(page).unwrap())
    }
}

fn ids(read: &super::SpotTradesRead) -> Vec<u64> {
    read.trades.iter().map(|t| t.id).collect()
}

/// The stand-in answers as Binance documents: without `fromId` or a time, the
/// most recent trades, which is what hid a scan that did not start from 0.
#[test]
fn without_from_id_or_a_time_the_venue_answers_the_most_recent_trades() {
    let answer = my_trades(history(2500), false);
    let request = |params: &[(&str, &str)]| Seen {
        method: "GET".to_string(),
        path: "/api/v3/myTrades".to_string(),
        params: params.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        api_key: true,
    };
    let first_and_last = |params: &[(&str, &str)]| {
        let page: Vec<Value> = serde_json::from_str(&answer(&request(params)).1).unwrap();
        (page[0]["id"].as_u64().unwrap(), page[page.len() - 1]["id"].as_u64().unwrap())
    };
    let order = [("symbol", "BNBBTC"), ("orderId", "9"), ("limit", "1000")];
    assert_eq!(first_and_last(&order), (1501, 2500));
    let from_zero = [
        ("symbol", "BNBBTC"),
        ("orderId", "9"),
        ("fromId", "0"),
        ("limit", "1000"),
    ];
    assert_eq!(first_and_last(&from_zero), (1, 1000));
}

/// An order with more fills than a page is read whole, oldest first: every
/// page names `fromId`, the first 0 (rest-api.md L4754, L4763).
#[tokio::test]
async fn trades_by_order_page_by_from_id_until_a_short_page() {
    let (reader, seen) = venue(my_trades(history(2500), false)).await;
    let query = SpotTradesQuery::Order { order_id: 9, from_id: 0 };
    let read = reader.trades(&symbol("BNBBTC"), query).await.unwrap();
    assert_eq!(ids(&read), (1..=2500).collect::<Vec<_>>());
    assert_eq!((read.pages, read.weight, read.next), (3, 15, None));
    let from: Vec<_> = seen
        .lock()
        .unwrap()
        .iter()
        .map(|r| r.param("fromId").map(str::to_string))
        .collect();
    assert_eq!(from, [
        Some("0".to_string()),
        Some("1001".to_string()),
        Some("2001".to_string())
    ]);
}

#[tokio::test]
async fn an_order_read_stops_at_five_pages_and_its_cursor_continues_it() {
    // 6,000 fills of order 9, then another order's trades on the symbol.
    let mut trades = history(6000);
    trades.extend((6001..=6100).map(|id| trade(id, 8, BASE_TIME + id as i64 * 1000)));
    let (reader, seen) = venue(my_trades(trades, false)).await;
    let first = reader
        .trades(&symbol("BNBBTC"), SpotTradesQuery::Order { order_id: 9, from_id: 0 })
        .await
        .unwrap();
    assert_eq!(ids(&first), (1..=5000).collect::<Vec<_>>());
    assert_eq!((first.pages, first.weight), (MY_TRADES_MAX_PAGES, 25));
    assert_eq!(first.next, Some(SpotTradesQuery::Order { order_id: 9, from_id: 5001 }));
    assert!(seen.lock().unwrap().iter().all(|r| r.param("orderId") == Some("9")));

    let rest = reader.trades(&symbol("BNBBTC"), first.next.unwrap()).await.unwrap();
    assert_eq!(ids(&rest), (5001..=6000).collect::<Vec<_>>());
    // A full page, then an empty one: the order had no more.
    assert_eq!((rest.pages, rest.weight, rest.next), (2, 10, None));
}

#[tokio::test]
async fn a_read_stops_at_five_pages_and_its_cursor_continues_it() {
    let (reader, seen) = venue(my_trades(history(6000), false)).await;
    let window = SpotTradesQuery::Window {
        start_ms: BASE_TIME,
        end_ms: BASE_TIME + MY_TRADES_MAX_WINDOW_MS,
        from_id: None,
    };
    let first = reader.trades(&symbol("BNBBTC"), window).await.unwrap();
    assert_eq!(first.pages, MY_TRADES_MAX_PAGES);
    assert_eq!((first.trades.len(), first.weight), (5000, 100));
    let Some(cursor) = first.next else {
        panic!("a full fifth page leaves a cursor")
    };
    assert_eq!(cursor, SpotTradesQuery::Window {
        start_ms: BASE_TIME,
        end_ms: BASE_TIME + MY_TRADES_MAX_WINDOW_MS,
        from_id: Some(5001),
    });
    // The first page names the window; later ones only `fromId` (L4756-L4763).
    let requests = seen.lock().unwrap().clone();
    assert!(requests[0].param("startTime").is_some() && requests[0].param("fromId").is_none());
    for later in &requests[1..] {
        assert!(later.param("startTime").is_none() && later.param("endTime").is_none());
        assert!(later.param("fromId").is_some());
    }

    let rest = reader.trades(&symbol("BNBBTC"), cursor).await.unwrap();
    assert_eq!(ids(&rest), (5001..=6000).collect::<Vec<_>>());
    // A full page, then an empty one: the venue had no more.
    assert_eq!((rest.pages, rest.next), (2, None));
}

#[tokio::test]
async fn a_window_read_stops_at_the_first_trade_past_its_end() {
    let (reader, seen) = venue(my_trades(history(3000), false)).await;
    let end_ms = BASE_TIME + 1500 * 1000;
    let window = SpotTradesQuery::Window {
        start_ms: BASE_TIME,
        end_ms,
        from_id: None,
    };
    let read = reader.trades(&symbol("BNBBTC"), window).await.unwrap();
    // The first page ends at 1000; the `fromId` page runs past the window.
    assert_eq!(ids(&read), (1..=1500).collect::<Vec<_>>());
    assert_eq!((read.pages, read.next), (2, None));
    assert_eq!(seen.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn trades_read_twice_count_once() {
    let (reader, _) = venue(my_trades(history(2500), true)).await;
    let query = SpotTradesQuery::Order { order_id: 9, from_id: 0 };
    let read = reader.trades(&symbol("BNBBTC"), query).await.unwrap();
    assert_eq!(ids(&read), (1..=2500).collect::<Vec<_>>());
    assert_eq!(read.pages, 3);
}

#[tokio::test]
async fn one_trade_id_with_two_records_is_inconsistent() {
    let mut trades = history(1000);
    let mut changed = trade(1000, 9, BASE_TIME + 1000 * 1000);
    changed["qty"] = json!("13.00000000");
    trades.push(changed);
    // The second page starts at 1000 again (overlap) and finds another record.
    let (reader, _) = venue(my_trades(trades, true)).await;
    let query = SpotTradesQuery::Order { order_id: 9, from_id: 0 };
    assert!(matches!(
        reader.trades(&symbol("BNBBTC"), query).await,
        Err(SpotReadError::Inconsistent(why)) if why.contains("trade 1000")
    ));
}

#[tokio::test]
async fn a_trade_of_another_order_or_symbol_is_inconsistent() {
    let foreign_order = serde_json::to_string(&[trade(1, 8, BASE_TIME)]).unwrap();
    let (reader, _) = answering(200, &foreign_order).await;
    let query = SpotTradesQuery::Order { order_id: 9, from_id: 0 };
    assert!(matches!(
        reader.trades(&symbol("BNBBTC"), query).await,
        Err(SpotReadError::Inconsistent(why)) if why.contains("of order 8")
    ));

    let (reader, _) = answering(200, MY_TRADES).await;
    let query = SpotTradesQuery::Order { order_id: 100234, from_id: 0 };
    assert!(matches!(
        reader.trades(&symbol("BTCUSDT"), query).await,
        Err(SpotReadError::Inconsistent(why)) if why.contains("on BNBBTC")
    ));
}

#[tokio::test]
async fn no_trades_is_one_empty_page() {
    let (reader, _) = answering(200, "[]").await;
    let query = SpotTradesQuery::Order { order_id: 9, from_id: 0 };
    let read = reader.trades(&symbol("BNBBTC"), query).await.unwrap();
    assert!(read.trades.is_empty());
    assert_eq!((read.pages, read.weight, read.next), (1, 5, None));
}

#[tokio::test]
async fn a_window_over_24_hours_is_refused_before_anything_is_sent() {
    let (reader, seen) = answering(200, "[]").await;
    for (start_ms, end_ms) in [
        (BASE_TIME, BASE_TIME + MY_TRADES_MAX_WINDOW_MS + 1),
        (BASE_TIME, BASE_TIME - 1),
        (-1, 0),
    ] {
        let window = SpotTradesQuery::Window { start_ms, end_ms, from_id: None };
        assert!(matches!(
            reader.trades(&symbol("BNBBTC"), window).await,
            Err(SpotReadError::InvalidQuery(_))
        ));
    }
    assert!(seen.lock().unwrap().is_empty());

    let day = SpotTradesQuery::Window {
        start_ms: BASE_TIME,
        end_ms: BASE_TIME + MY_TRADES_MAX_WINDOW_MS,
        from_id: None,
    };
    reader.trades(&symbol("BNBBTC"), day).await.unwrap();
}

#[tokio::test]
async fn a_failed_page_fails_the_read() {
    let (reader, _) = venue(|request| match request.param("fromId") {
        Some("0") => (200, serde_json::to_string(&history(1000)).unwrap()),
        _ => (503, error_body("unknown")),
    })
    .await;
    let query = SpotTradesQuery::Order { order_id: 9, from_id: 0 };
    assert!(matches!(
        reader.trades(&symbol("BNBBTC"), query).await,
        Err(SpotReadError::ServerError { http_status: 503, .. })
    ));
}
