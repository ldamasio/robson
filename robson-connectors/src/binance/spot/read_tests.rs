//! ADR-0056 step 8a: the spot reads observed on the wire.
//!
//! Every read goes through the real REST client to a local listener standing
//! in for Binance, so these tests see the requests Robson sends and the
//! errors it types. Nothing reaches the network.

use std::{sync::Arc, time::Duration};

use robson_domain::Symbol;
use rust_decimal_macros::dec;
use serde_json::{json, Value};
use tokio::net::TcpListener;

use super::{
    stand_in::{self, Seen, Seens},
    SpotOrderRef, SpotRead, SpotReadError, SpotRestReader, SpotTradesQuery, MY_TRADES_MAX_PAGES,
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

/// A stand-in venue answering each request with `reply`'s status and body.
async fn venue(
    reply: impl Fn(&Seen) -> (u16, String) + Send + Sync + 'static,
) -> (SpotRestReader, Seens) {
    let (client, seen) = stand_in::serve(reply).await;
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
        SpotTradesQuery::FromId { from_id: 28457 },
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
    assert_eq!(weights, [5, 5, 20]);
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
/// (rest-api.md L98-L102, L4754): filtered by `orderId`, then the oldest
/// `limit` from `fromId` (inclusive), otherwise the most recent `limit`;
/// chronological either way. The reader sends no time, so a time here is a
/// defect. `overlap` serves each `fromId` page from one id earlier, as a venue
/// repeating a trade would.
fn my_trades(trades: Vec<Value>, overlap: bool) -> impl Fn(&Seen) -> (u16, String) {
    move |request| {
        for time in ["startTime", "endTime"] {
            assert!(request.param(time).is_none(), "the reader sent {time}");
        }
        let number = |name: &str| request.param(name).map(|v| v.parse::<i64>().unwrap());
        let from_id = number("fromId").map(|id| if overlap { id - 1 } else { id });
        let limit = number("limit").unwrap() as usize;
        let matching: Vec<&Value> = trades
            .iter()
            .filter(|t| number("orderId").is_none_or(|id| t["orderId"].as_i64() == Some(id)))
            .filter(|t| from_id.is_none_or(|id| t["id"].as_i64().unwrap() >= id))
            .collect();
        let page = if from_id.is_some() {
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

/// Trades of two orders, alternating: ids from 1, one second apart.
fn two_orders(count: u64) -> Vec<Value> {
    (1..=count)
        .map(|id| trade(id, 8 + id % 2, BASE_TIME + id as i64 * 1000))
        .collect()
}

/// A scan from an id reads every order's trades from the caller's seed, by
/// `symbol + fromId` only (rest-api.md L4761).
#[tokio::test]
async fn a_scan_from_an_id_reads_every_order_from_its_seed() {
    let (reader, seen) = venue(my_trades(two_orders(2500), false)).await;
    let read = reader
        .trades(&symbol("BNBBTC"), SpotTradesQuery::FromId { from_id: 1201 })
        .await
        .unwrap();
    assert_eq!(ids(&read), (1201..=2500).collect::<Vec<_>>());
    assert!(
        read.trades.iter().any(|t| t.order_id == 8) && read.trades.iter().any(|t| t.order_id == 9)
    );
    assert_eq!((read.pages, read.weight, read.next), (2, 40, None));
    for request in seen.lock().unwrap().iter() {
        assert!(request.param("fromId").is_some() && request.param("orderId").is_none());
    }
}

#[tokio::test]
async fn a_scan_from_an_id_stops_at_five_pages_and_its_cursor_continues_it() {
    let (reader, _) = venue(my_trades(two_orders(6000), false)).await;
    let first = reader
        .trades(&symbol("BNBBTC"), SpotTradesQuery::FromId { from_id: 0 })
        .await
        .unwrap();
    assert_eq!(ids(&first), (1..=5000).collect::<Vec<_>>());
    assert_eq!((first.pages, first.weight), (MY_TRADES_MAX_PAGES, 100));
    assert_eq!(first.next, Some(SpotTradesQuery::FromId { from_id: 5001 }));

    let rest = reader.trades(&symbol("BNBBTC"), first.next.unwrap()).await.unwrap();
    assert_eq!(ids(&rest), (5001..=6000).collect::<Vec<_>>());
    // A full page, then an empty one: the venue had no more.
    assert_eq!((rest.pages, rest.next), (2, None));
}

/// A first page holding only the latest trades, which is what Binance
/// answers without `fromId` (L4754), cannot end a scan. No scan sends a
/// request without `fromId`, and only a short page ends one: a full page,
/// even one that ignored `fromId`, continues, and the page bound then returns
/// a cursor instead of claiming the end.
#[tokio::test]
async fn a_latest_subset_first_page_cannot_end_a_scan() {
    let (reader, seen) = venue(my_trades(history(2500), false)).await;
    for query in [
        SpotTradesQuery::Order { order_id: 9, from_id: 0 },
        SpotTradesQuery::FromId { from_id: 0 },
    ] {
        let read = reader.trades(&symbol("BNBBTC"), query).await.unwrap();
        assert_eq!(ids(&read), (1..=2500).collect::<Vec<_>>(), "{query:?}");
        assert_eq!(read.next, None, "{query:?}");
    }
    assert!(seen.lock().unwrap().iter().all(|r| r.param("fromId").is_some()));

    // A venue answering every page with the latest 1000 of 2500.
    let latest = serde_json::to_string(&history(2500)[1500..]).unwrap();
    let (reader, _) = answering(200, &latest).await;
    for query in [
        SpotTradesQuery::Order { order_id: 9, from_id: 0 },
        SpotTradesQuery::FromId { from_id: 0 },
    ] {
        let read = reader.trades(&symbol("BNBBTC"), query).await.unwrap();
        assert_eq!(read.pages, MY_TRADES_MAX_PAGES, "{query:?}");
        assert!(read.next.is_some(), "{query:?}: a full page never ends a scan");
    }
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
