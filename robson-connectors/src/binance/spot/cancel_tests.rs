//! ADR-0056 step 8c-3b: the spot cancel and the composite wrappers on the
//! wire, against a local stand-in for Binance (8c plan C1, C3, §4 fills rule,
//! §11). Nothing reaches the network.

use std::{
    sync::{Arc, OnceLock},
    time::Duration,
};

use chrono::{TimeZone, Utc};
use robson_domain::{OrderSide, Price, Quantity, Symbol};
use robson_exec::{
    order_journal::{IntentKind, OrderJournal},
    ports::{FeeEvidence, TradeFill},
    venue::{
        CancelAnswer, InventoryPort, OrderLookup, OrderPhase, ReplaceOutcome, StopCancelOutcome,
        StopParams, StopRef, TradingPort, Venue, VenueOrderRecord,
    },
    ExecError, OrderResult,
};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde_json::{json, Value};
use tokio::net::TcpListener;

use super::{
    stand_in::{self, Seen, Seens},
    trading::terminal_result,
    trading_tests::{error_body, granted, requests, result, spot_journal},
    BinanceSpotAdapter, SpotRead, SpotWrite,
};
use crate::binance_rest::BinanceRestClient;

const CANCEL_ORDER: &str = include_str!("testdata/cancel_order.json");
const ORDER: &str = include_str!("testdata/order.json");
const MY_TRADES: &str = include_str!("testdata/my_trades.json");

const BASE_TIME: i64 = 1_700_000_000_000;

fn ltc() -> &'static Symbol {
    static LTC: OnceLock<Symbol> = OnceLock::new();
    LTC.get_or_init(|| Symbol::from_pair("LTCBTC").unwrap())
}

fn stop_4() -> StopRef {
    StopRef::new("4")
}

/// The documented cancel answer, for order 4 as a `STOP_LOSS` SELL of 1
/// cancelled with nothing executed, then `edit`ed.
fn cancelled(edit: impl FnOnce(&mut Value)) -> String {
    let mut answer: Value = serde_json::from_str(CANCEL_ORDER).unwrap();
    answer["type"] = json!("STOP_LOSS");
    answer["side"] = json!("SELL");
    edit(&mut answer);
    answer.to_string()
}

/// The documented order record as order 4, a `STOP_LOSS` SELL of 1 (or of
/// `executed`, when more), with `status`, `executed` and `quote`.
fn looked_up(status: &str, executed: &str, quote: &str) -> String {
    let mut order: Value = serde_json::from_str(ORDER).unwrap();
    order["orderId"] = json!(4);
    if executed.parse::<Decimal>().unwrap() > Decimal::ONE {
        order["origQty"] = json!(executed);
    }
    order["type"] = json!("STOP_LOSS");
    order["side"] = json!("SELL");
    order["status"] = json!(status);
    order["executedQty"] = json!(executed);
    order["cummulativeQuoteQty"] = json!(quote);
    order.to_string()
}

/// `count` trades of order 4 on LTCBTC, ids from `first`, each of `qty` at
/// 0.1 with a 0.001 BNB commission, one second apart.
fn trades(first: u64, count: u64, qty: Decimal) -> Vec<Value> {
    (first..first + count)
        .map(|id| {
            let mut trade = serde_json::from_str::<Value>(MY_TRADES).unwrap()[0].clone();
            trade["symbol"] = json!("LTCBTC");
            trade["id"] = json!(id);
            trade["orderId"] = json!(4);
            trade["price"] = json!("0.1");
            trade["qty"] = json!(qty.to_string());
            trade["quoteQty"] = json!((dec!(0.1) * qty).to_string());
            trade["commission"] = json!("0.001");
            trade["time"] = json!(BASE_TIME + id as i64 * 1000);
            trade
        })
        .collect()
}

/// Binance's `myTrades` page over `trades`: ids from `fromId`, at most 1000.
fn page(trades: &[Value], request: &Seen) -> String {
    let from: u64 = request.param("fromId").unwrap().parse().unwrap();
    let page: Vec<&Value> = trades
        .iter()
        .filter(|trade| trade["id"].as_u64().unwrap() >= from)
        .take(1000)
        .collect();
    serde_json::to_string(&page).unwrap()
}

/// What the stand-in answers: the cancel, the order lookup, the trades and,
/// for a replacement, the placement.
#[derive(Clone)]
struct Answers {
    cancel: (u16, String),
    lookup: (u16, String),
    trades: Vec<Value>,
}

impl Answers {
    fn cancel(status: u16, body: String) -> Self {
        Self {
            cancel: (status, body),
            lookup: (500, error_body("unknown")),
            trades: Vec::new(),
        }
    }

    fn lookup(mut self, status: u16, body: String) -> Self {
        self.lookup = (status, body);
        self
    }

    fn trades(mut self, trades: Vec<Value>) -> Self {
        self.trades = trades;
        self
    }
}

async fn scripted(answers: Answers) -> (BinanceSpotAdapter, Seens) {
    let (client, seen) =
        stand_in::serve(move |request| match (request.method.as_str(), request.path.as_str()) {
            ("DELETE", "/api/v3/order") => answers.cancel.clone(),
            ("GET", "/api/v3/order") => answers.lookup.clone(),
            ("GET", "/api/v3/myTrades") => (200, page(&answers.trades, request)),
            ("POST", "/api/v3/order") => (200, result(request, |_| {})),
            other => panic!("unexpected request {other:?}"),
        })
        .await;
    (BinanceSpotAdapter::new(client).unwrap(), seen)
}

/// The documented weight of the requests `seen` holds.
fn weight(seen: &Seens) -> u32 {
    seen.lock()
        .unwrap()
        .iter()
        .map(|request| match (request.method.as_str(), request.path.as_str()) {
            ("DELETE", "/api/v3/order") => SpotWrite::CANCEL_ORDER.weight,
            ("POST", "/api/v3/order") => SpotWrite::NEW_ORDER.weight,
            ("GET", "/api/v3/order") => SpotRead::ORDER.weight,
            ("GET", "/api/v3/myTrades") => SpotRead::MY_TRADES_BY_ORDER.weight,
            other => panic!("unexpected request {other:?}"),
        })
        .sum()
}

const DELETE_4: &str = "DELETE /api/v3/order?orderId=4&symbol=LTCBTC";
const GET_4: &str = "GET /api/v3/order?orderId=4&symbol=LTCBTC";

fn trades_from(from: u64) -> String {
    format!("GET /api/v3/myTrades?fromId={from}&limit=1000&orderId=4&symbol=LTCBTC")
}

// =============================================================================
// cancel_order_once
// =============================================================================

/// One DELETE by `orderId` alone, signed and keyed, with no journal, no
/// grant and nothing renamed by Robson: the cancel's own id is the venue's
/// (F12), and the answer's record keeps both.
#[tokio::test]
async fn a_cancel_is_one_delete_by_order_id_with_no_grant() {
    let (venue, seen) = scripted(Answers::cancel(200, cancelled(|_| {}))).await;
    let answer = venue.cancel_order_once(ltc(), "4").await.unwrap();
    let CancelAnswer::Cancelled(record) = answer else {
        panic!("{answer:?}");
    };
    assert_eq!(requests(&seen), [DELETE_4]);
    let request = seen.lock().unwrap()[0].clone();
    assert!(request.api_key && request.param("signature").is_some());
    for never in [
        "newClientOrderId",
        "origClientOrderId",
        "cancelRestrictions",
    ] {
        assert!(request.param(never).is_none(), "{never}");
    }
    assert_eq!(record.exchange_order_id, "4");
    assert_eq!(record.client_order_id, "cancelMyOrder1");
    assert_eq!(record.orig_client_order_id.as_deref(), Some("myOrder1"));
    assert_eq!((record.status.as_str(), record.phase), ("CANCELED", OrderPhase::Terminal));
    assert_eq!((record.orig_qty, record.executed_qty), (dec!(1), dec!(0)));
    assert_eq!(record.transact_time_ms, Some(1684804350068));
    assert_eq!(record.stop_price, Some(dec!(23500)));
    assert!(record.protection_kind.is_some());
    assert_eq!(SpotWrite::CANCEL_ORDER, SpotWrite {
        path: "/api/v3/order",
        weight: 1,
        orders: 0
    });
}

/// Only an exact 200 whose own record is this order's, terminal, outside any
/// order list and consistent is `Cancelled`; a live or unknown status, other
/// ids, impossible quantities, an unreadable body or another success is
/// `Unknown`.
#[tokio::test]
async fn only_a_terminal_consistent_200_is_cancelled() {
    let cancels: [(&str, Value); 4] = [
        ("status", json!("FILLED")),
        ("status", json!("EXPIRED")),
        ("status", json!("EXPIRED_IN_MATCH")),
        ("executedQty", json!("0.4")),
    ];
    for (field, value) in cancels {
        let (venue, _) =
            scripted(Answers::cancel(200, cancelled(|answer| answer[field] = value.clone()))).await;
        let answer = venue.cancel_order_once(ltc(), "4").await.unwrap();
        assert!(matches!(answer, CancelAnswer::Cancelled(_)), "{field}: {answer:?}");
    }
    let unknowns: [(&str, Value); 9] = [
        ("status", json!("NEW")),
        ("status", json!("PARTIALLY_FILLED")),
        ("status", json!("PENDING_CANCEL")),
        ("status", json!("SOMETHING_NEW")),
        ("orderId", json!(5)),
        ("symbol", json!("BTCUSDT")),
        ("orderListId", json!(3)),
        ("executedQty", json!("1.5")),
        ("side", json!("HOLD")),
    ];
    for (field, value) in unknowns {
        let (venue, seen) =
            scripted(Answers::cancel(200, cancelled(|answer| answer[field] = value.clone()))).await;
        let answer = venue.cancel_order_once(ltc(), "4").await.unwrap();
        assert!(matches!(answer, CancelAnswer::Unknown { .. }), "{field}: {answer:?}");
        assert_eq!(requests(&seen), [DELETE_4]);
    }
    for (status, body) in [
        (200, "{}".to_string()),
        (200, "not json".to_string()),
        (201, cancelled(|_| {})),
        (202, cancelled(|_| {})),
        (204, String::new()),
    ] {
        let (venue, _) = scripted(Answers::cancel(status, body)).await;
        let answer = venue.cancel_order_once(ltc(), "4").await.unwrap();
        assert!(matches!(answer, CancelAnswer::Unknown { .. }), "{status}: {answer:?}");
    }
}

/// Not open is HTTP 400 with -2011 or -2013 only (errors.md L431-L437); the
/// same codes at another status, -2026, -1006, -1007, a 5XX with or without
/// JSON and any other code are `Unknown`.
#[tokio::test]
async fn not_open_is_400_with_2011_or_2013_and_everything_else_unknown() {
    let mut cases: Vec<(u16, String, Option<i64>)> = vec![
        (400, error_body("cancel_rejected"), Some(-2011)),
        (400, error_body("no_such_order"), Some(-2013)),
        (400, error_body("order_archived"), None),
        (400, error_body("unexpected_resp"), None),
        (400, error_body("backend_timeout"), None),
        (400, error_body("bad_symbol"), None),
        (500, error_body("unknown"), None),
        (502, error_body("html_page"), None),
        (503, error_body("backend_timeout"), None),
    ];
    for status in [403, 418, 429, 500, 503] {
        for name in ["cancel_rejected", "no_such_order"] {
            cases.push((status, error_body(name), None));
        }
    }
    for (status, body, not_open) in cases {
        let (venue, seen) = scripted(Answers::cancel(status, body)).await;
        let answer = venue.cancel_order_once(ltc(), "4").await.unwrap();
        match not_open {
            Some(code) => assert_eq!(answer, CancelAnswer::NotFound { code }, "{status}"),
            None => assert!(matches!(answer, CancelAnswer::Unknown { .. }), "{status}: {answer:?}"),
        }
        assert_eq!(requests(&seen), [DELETE_4]);
    }
}

/// A timeout and a failed transport may have reached the venue: `Unknown`. A
/// refusal is a proven no-send: the gate stopping, or an id that is not a
/// number.
#[tokio::test]
async fn a_lost_answer_is_unknown_and_a_refusal_is_err() {
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
    let venue = BinanceSpotAdapter::new(Arc::new(client)).unwrap();
    let answer = venue.cancel_order_once(ltc(), "4").await.unwrap();
    assert!(matches!(answer, CancelAnswer::Unknown { .. }), "{answer:?}");

    let url = stand_in::closing().await;
    let venue =
        BinanceSpotAdapter::new(Arc::new(BinanceRestClient::with_local_base_url(url))).unwrap();
    let answer = venue.cancel_order_once(ltc(), "4").await.unwrap();
    assert!(matches!(answer, CancelAnswer::Unknown { .. }), "{answer:?}");

    let (venue, seen) = scripted(Answers::cancel(200, cancelled(|_| {}))).await;
    let stopping = OrderJournal::unpersisted();
    venue.attach_gate(Arc::clone(stopping.gate()));
    stopping.gate().begin_draining();
    let refused = venue.cancel_order_once(ltc(), "4").await;
    assert!(matches!(refused, Err(ExecError::SendRefused(_))), "{refused:?}");
    let refused = venue.cancel_order_once(ltc(), "four").await;
    assert!(matches!(refused, Err(ExecError::Config(_))), "{refused:?}");
    assert_eq!(requests(&seen), Vec::<String>::new());
}

// =============================================================================
// cancel_protection (C3: at most 7 requests, weight 30)
// =============================================================================

/// A cancelled stop: its own answer's execution, and fills only when it
/// executed, read from its trades. Nothing executed: no trade read.
#[tokio::test]
async fn a_cancelled_stop_carries_its_execution_and_fills_only_when_it_executed() {
    let (venue, seen) = scripted(Answers::cancel(200, cancelled(|_| {}))).await;
    let outcome = venue.cancel_protection(ltc(), &stop_4()).await.unwrap();
    assert!(matches!(outcome, StopCancelOutcome::Cancelled {
        filled_before_cancel,
        fills: None
    } if filled_before_cancel.is_zero()));
    assert_eq!(requests(&seen), [DELETE_4]);

    let partly = cancelled(|answer| {
        answer["executedQty"] = json!("0.4");
        answer["cummulativeQuoteQty"] = json!("0.04");
    });
    let answers = Answers::cancel(200, partly).trades(trades(10, 2, dec!(0.2)));
    let (venue, seen) = scripted(answers).await;
    let outcome = venue.cancel_protection(ltc(), &stop_4()).await.unwrap();
    let StopCancelOutcome::Cancelled { filled_before_cancel, fills: Some(fills) } = outcome else {
        panic!("{outcome:?}");
    };
    assert_eq!(filled_before_cancel, dec!(0.4));
    assert_eq!(fills.filled_quantity.as_decimal(), dec!(0.4));
    assert_eq!(fills.fill_price.as_decimal(), dec!(0.1));
    assert_eq!(fills.client_order_id, "myOrder1", "the order's own id, not the cancel's");
    assert_eq!(fills.fee_total(), Some((dec!(0.002), "BNB")));
    assert_eq!(fills.fills().len(), 2);
    assert_eq!(fills.filled_at.timestamp_millis(), BASE_TIME + 11_000);
    assert_eq!(requests(&seen), [DELETE_4.to_string(), trades_from(0)]);
}

/// After HTTP 400 -2011 or -2013, one lookup by exchange id decides: a
/// terminal record with its fills, a live one as it is; a further
/// not-found, an archive or a failed lookup proves nothing.
#[tokio::test]
async fn a_stop_already_gone_is_what_one_lookup_establishes() {
    for code in ["cancel_rejected", "no_such_order"] {
        let answers = Answers::cancel(400, error_body(code))
            .lookup(200, looked_up("FILLED", "1", "0.1"))
            .trades(trades(1, 4, dec!(0.25)));
        let (venue, seen) = scripted(answers).await;
        let outcome = venue.cancel_protection(ltc(), &stop_4()).await.unwrap();
        let StopCancelOutcome::AlreadyGone {
            final_state:
                OrderLookup::Terminal {
                    filled_qty,
                    cancelled_qty,
                    fills: Some(fills),
                },
        } = outcome
        else {
            panic!("{code}: {outcome:?}");
        };
        assert_eq!((filled_qty, cancelled_qty), (dec!(1), dec!(0)));
        assert_eq!(fills.filled_quantity.as_decimal(), dec!(1));
        assert_eq!(fills.fills().len(), 4);
        assert_eq!(requests(&seen), [DELETE_4.to_string(), GET_4.to_string(), trades_from(0)]);
    }
    let undecided: [(u16, String); 3] = [
        (400, error_body("no_such_order")),
        (400, error_body("order_archived")),
        (503, error_body("unknown")),
    ];
    for (status, body) in undecided {
        let answers = Answers::cancel(400, error_body("cancel_rejected")).lookup(status, body);
        let (venue, seen) = scripted(answers).await;
        let outcome = venue.cancel_protection(ltc(), &stop_4()).await.unwrap();
        assert!(
            matches!(outcome, StopCancelOutcome::AlreadyGone { final_state: OrderLookup::Unknown }),
            "{status}: {outcome:?}"
        );
        assert_eq!(requests(&seen), [DELETE_4, GET_4]);
    }
    let answers =
        Answers::cancel(400, error_body("cancel_rejected")).lookup(200, looked_up("NEW", "0", "0"));
    let (venue, _) = scripted(answers).await;
    let outcome = venue.cancel_protection(ltc(), &stop_4()).await.unwrap();
    assert!(matches!(outcome, StopCancelOutcome::AlreadyGone {
        final_state: OrderLookup::Open { .. }
    }));
}

/// An uncertain cancel is `Unknown`, with nothing more read; a refused one is
/// the cancel's `Err`.
#[tokio::test]
async fn an_uncertain_cancel_is_unknown_and_reads_nothing_more() {
    let (venue, seen) = scripted(Answers::cancel(503, error_body("unknown"))).await;
    let outcome = venue.cancel_protection(ltc(), &stop_4()).await.unwrap();
    assert!(matches!(outcome, StopCancelOutcome::Unknown));
    assert_eq!(requests(&seen), [DELETE_4]);
    assert!(venue.cancel_protection(ltc(), &StopRef::new("four")).await.is_err());
}

/// The fills rule: fills only when distinct-id trades sum exactly to
/// `executedQty`. Short, conflicting or failed reads give none, and never
/// undo terminality or the executed quantity; more than five pages are
/// never read: 7 requests and weight 30 at most.
#[tokio::test]
async fn fills_need_trades_summing_to_the_execution_within_five_pages() {
    let gone = || {
        Answers::cancel(400, error_body("cancel_rejected"))
            .lookup(200, looked_up("FILLED", "1", "0.1"))
    };
    let mut conflicting = trades(1, 4, dec!(0.25));
    let mut changed = conflicting[0].clone();
    changed["commission"] = json!("0.002");
    conflicting.push(changed);
    for (case, trades) in [
        ("short", trades(1, 3, dec!(0.25))),
        ("over", trades(1, 5, dec!(0.25))),
        ("conflicting", conflicting),
    ] {
        let (venue, _) = scripted(gone().trades(trades)).await;
        let outcome = venue.cancel_protection(ltc(), &stop_4()).await.unwrap();
        assert!(
            matches!(outcome, StopCancelOutcome::AlreadyGone {
                final_state: OrderLookup::Terminal { filled_qty, fills: None, .. }
            } if filled_qty == dec!(1)),
            "{case}: {outcome:?}"
        );
    }
    // A trades read that fails.
    let (client, seen) = stand_in::serve(|request| match request.path.as_str() {
        "/api/v3/myTrades" => (503, error_body("unknown")),
        _ if request.method == "DELETE" => (400, error_body("cancel_rejected")),
        _ => (200, looked_up("FILLED", "1", "0.1")),
    })
    .await;
    let venue = BinanceSpotAdapter::new(client).unwrap();
    let outcome = venue.cancel_protection(ltc(), &stop_4()).await.unwrap();
    assert!(matches!(outcome, StopCancelOutcome::AlreadyGone {
        final_state: OrderLookup::Terminal { fills: None, .. }
    }));
    assert_eq!(requests(&seen).len(), 3);

    // 6000 fills: five full pages, then nothing more is read.
    let many =
        gone()
            .lookup(200, looked_up("FILLED", "6000", "600"))
            .trades(trades(1, 6000, dec!(1)));
    let (venue, seen) = scripted(many).await;
    let outcome = venue.cancel_protection(ltc(), &stop_4()).await.unwrap();
    assert!(matches!(outcome, StopCancelOutcome::AlreadyGone {
        final_state: OrderLookup::Terminal { fills: None, .. }
    }));
    let mut expected = vec![DELETE_4.to_string(), GET_4.to_string()];
    expected.extend([0, 1001, 2001, 3001, 4001].map(trades_from));
    assert_eq!(requests(&seen), expected);
    assert_eq!((requests(&seen).len(), weight(&seen)), (7, 30));
}

/// The fills rule itself, over trades from anywhere: an identical duplicate
/// counts once, a different one under the same id gives no fills. (The
/// reader already refuses a conflicting duplicate within a page, and pages
/// never overlap, so the pages above cannot show it.)
#[test]
fn the_fills_rule_refuses_trades_that_conflict() {
    let record: VenueOrderRecord = {
        let answer: Value = serde_json::from_str(&looked_up("FILLED", "1", "0.1")).unwrap();
        VenueOrderRecord {
            exchange_order_id: "4".to_string(),
            client_order_id: "myOrder1".to_string(),
            orig_client_order_id: None,
            symbol: answer["symbol"].as_str().unwrap().to_string(),
            side: OrderSide::Sell,
            order_type: "STOP_LOSS".to_string(),
            protection_kind: None,
            status: "FILLED".to_string(),
            phase: OrderPhase::Terminal,
            orig_qty: dec!(1),
            executed_qty: dec!(1),
            cumulative_quote_qty: Some(dec!(0.1)),
            stop_price: None,
            expiry_reason: None,
            transact_time_ms: None,
            time_ms: None,
            update_time_ms: None,
            working_time_ms: None,
        }
    };
    let fill = |id: u64, commission: Decimal| TradeFill {
        trade_id: id.to_string(),
        price: dec!(0.1),
        qty: dec!(0.5),
        quote_qty: dec!(0.05),
        commission,
        commission_asset: "BNB".to_string(),
        time: Some(Utc.timestamp_millis_opt(BASE_TIME + id as i64).unwrap()),
    };
    let twice = vec![
        fill(1, dec!(0.001)),
        fill(1, dec!(0.001)),
        fill(2, dec!(0.001)),
    ];
    let result = terminal_result(&record, twice).unwrap();
    assert_eq!(result.fills().len(), 2);
    let conflicting = vec![
        fill(1, dec!(0.001)),
        fill(1, dec!(0.002)),
        fill(2, dec!(0.001)),
    ];
    assert!(terminal_result(&record, conflicting).is_none());
    // Not terminal, or nothing executed: no result, whatever the trades.
    let live = VenueOrderRecord {
        phase: OrderPhase::Live,
        ..record.clone()
    };
    assert!(terminal_result(&live, vec![fill(1, dec!(0.001)), fill(2, dec!(0.001))]).is_none());
}

// =============================================================================
// order_by_exchange_id, protection_fill (C3: weight 29 at most)
// =============================================================================

/// The result of a terminal order with execution: its record's quantity and
/// price, its trades as fills with their times, the fee in one asset. Not
/// executed, not terminal, unknown or archived: `None`, with no trade read.
#[tokio::test]
async fn a_lookup_by_exchange_id_is_the_fill_of_a_terminal_executed_order() {
    let answers = Answers::cancel(500, String::new())
        .lookup(200, looked_up("FILLED", "1", "0.11"))
        .trades(trades(1, 4, dec!(0.25)));
    let (venue, seen) = scripted(answers).await;
    let result: OrderResult = venue.order_by_exchange_id(ltc(), "4").await.unwrap().unwrap();
    assert_eq!(result.filled_quantity.as_decimal(), dec!(1));
    assert_eq!(result.fill_price.as_decimal(), dec!(0.11), "the record's price first");
    assert_eq!(result.fee_evidence(), FeeEvidence::Total);
    assert_eq!(result.fee_total(), Some((dec!(0.004), "BNB")));
    assert!(result.fills().iter().all(|fill| fill.time.is_some()));
    assert_eq!(result.filled_at.timestamp_millis(), BASE_TIME + 4_000);
    assert_eq!(result.record().unwrap().status, "FILLED");
    assert_eq!(requests(&seen), [GET_4.to_string(), trades_from(0)]);

    let nothing: [(u16, String); 6] = [
        (200, looked_up("NEW", "0", "0")),
        (200, looked_up("PARTIALLY_FILLED", "0.5", "0.05")),
        (200, looked_up("CANCELED", "0", "0")),
        (200, looked_up("EXPIRED", "0", "0")),
        (400, error_body("no_such_order")),
        (400, error_body("order_archived")),
    ];
    for (status, body) in nothing {
        let (venue, seen) =
            scripted(Answers::cancel(500, String::new()).lookup(status, body)).await;
        assert!(venue.order_by_exchange_id(ltc(), "4").await.unwrap().is_none(), "{status}");
        assert_eq!(requests(&seen), [GET_4]);
    }
}

/// A failed lookup, and an executed order whose trades cannot be
/// established within five pages, are `Err`, never `None`: weight 29 at
/// most.
#[tokio::test]
async fn an_executed_order_without_its_trades_is_an_error() {
    let (venue, _) = scripted(Answers::cancel(500, String::new())).await;
    assert!(venue.order_by_exchange_id(ltc(), "4").await.is_err());
    let short = Answers::cancel(500, String::new())
        .lookup(200, looked_up("FILLED", "1", "0.1"))
        .trades(trades(1, 3, dec!(0.25)));
    let (venue, _) = scripted(short).await;
    assert!(venue.order_by_exchange_id(ltc(), "4").await.is_err());
    let many = Answers::cancel(500, String::new())
        .lookup(200, looked_up("FILLED", "6000", "600"))
        .trades(trades(1, 6000, dec!(1)));
    let (venue, seen) = scripted(many).await;
    assert!(venue.order_by_exchange_id(ltc(), "4").await.is_err());
    assert_eq!((requests(&seen).len(), weight(&seen)), (6, 29));
}

/// A stop's fill is its lookup's, and an order that is not a stop is an
/// error.
#[tokio::test]
async fn a_protection_fill_is_a_stop_s_and_nothing_else() {
    let answers = Answers::cancel(500, String::new())
        .lookup(200, looked_up("FILLED", "1", "0.1"))
        .trades(trades(1, 4, dec!(0.25)));
    let (venue, _) = scripted(answers).await;
    let fill = venue.protection_fill(ltc(), &stop_4()).await.unwrap().unwrap();
    assert_eq!(fill.filled_quantity.as_decimal(), dec!(1));
    let resting = Answers::cancel(500, String::new()).lookup(200, looked_up("NEW", "0", "0"));
    let (venue, _) = scripted(resting).await;
    assert!(venue.protection_fill(ltc(), &stop_4()).await.unwrap().is_none());
    // A filled LIMIT with its trades: an order_by_exchange_id result, never a
    // stop's fill.
    let limit = looked_up("FILLED", "1", "0.1").replace("\"STOP_LOSS\"", "\"LIMIT\"");
    let answers =
        Answers::cancel(500, String::new())
            .lookup(200, limit)
            .trades(trades(1, 4, dec!(0.25)));
    let (venue, seen) = scripted(answers).await;
    assert!(venue.order_by_exchange_id(ltc(), "4").await.unwrap().is_some());
    assert!(venue.protection_fill(ltc(), &stop_4()).await.is_err());
    assert_eq!(requests(&seen), [GET_4.to_string(), trades_from(0), GET_4.to_string()]);
}

// =============================================================================
// replace_protection, cancel_order
// =============================================================================

fn successor() -> StopParams {
    StopParams {
        close_side: OrderSide::Sell,
        quantity: Quantity::new(dec!(1)).unwrap(),
        trigger: Price::new(dec!(0.09)).unwrap(),
        client_order_id: "ins-2".to_string(),
    }
}

/// Cancel first, then place: a definite cancel of an unexecuted stop places
/// the successor under its grant; an uncertain cancel or an executed old
/// stop places nothing.
#[tokio::test]
async fn a_replacement_cancels_first_and_places_only_after_a_definite_cancel() {
    let journal = spot_journal();
    let old = stop_4();
    let (venue, seen) = scripted(Answers::cancel(200, cancelled(|_| {}))).await;
    venue.attach_order_journal(Arc::clone(&journal));
    let replace = venue.replace_protection(ltc(), &old, successor());
    let outcome = granted(&journal, "ins-2", IntentKind::Protection, replace).await.unwrap();
    assert!(matches!(outcome, ReplaceOutcome::Replaced { old_filled, .. } if old_filled.is_zero()));
    let identities = requests(&seen);
    assert_eq!(identities[0], DELETE_4);
    assert!(identities[1].starts_with("POST /api/v3/order?newClientOrderId=ins-2"));
    assert_eq!(identities.len(), 2);

    let (venue, seen) = scripted(Answers::cancel(503, error_body("unknown"))).await;
    venue.attach_order_journal(Arc::clone(&journal));
    let replace = venue.replace_protection(ltc(), &old, successor());
    let outcome = granted(&journal, "ins-3", IntentKind::Protection, replace).await.unwrap();
    assert!(matches!(outcome, ReplaceOutcome::Unknown { new: None }));
    assert_eq!(requests(&seen), [DELETE_4]);

    let executed = cancelled(|answer| {
        answer["executedQty"] = json!("0.4");
        answer["cummulativeQuoteQty"] = json!("0.04");
    });
    let (venue, seen) =
        scripted(Answers::cancel(200, executed).trades(trades(10, 2, dec!(0.2)))).await;
    venue.attach_order_journal(Arc::clone(&journal));
    let replace = venue.replace_protection(ltc(), &old, successor());
    let outcome = granted(&journal, "ins-4", IntentKind::Protection, replace).await.unwrap();
    assert!(
        matches!(outcome, ReplaceOutcome::OldExecuted { old_filled } if old_filled == dec!(0.4))
    );
    assert!(requests(&seen).iter().all(|request| !request.starts_with("POST")));
}

/// The legacy cancel is `Ok` only when the venue's own answer shows the
/// order cancelled and terminal.
#[tokio::test]
async fn the_legacy_cancel_is_ok_only_for_a_definite_cancel() {
    let (venue, _) = scripted(Answers::cancel(200, cancelled(|_| {}))).await;
    venue.cancel_order(ltc(), "4").await.unwrap();
    let (venue, _) = scripted(Answers::cancel(400, error_body("cancel_rejected"))).await;
    assert!(matches!(venue.cancel_order(ltc(), "4").await, Err(ExecError::OrderRejected(_))));
    let (venue, _) = scripted(Answers::cancel(503, error_body("unknown"))).await;
    assert!(matches!(venue.cancel_order(ltc(), "4").await, Err(ExecError::Exchange(_))));
    let live = cancelled(|answer| answer["status"] = json!("NEW"));
    let (venue, _) = scripted(Answers::cancel(200, live)).await;
    assert!(matches!(venue.cancel_order(ltc(), "4").await, Err(ExecError::Exchange(_))));
}
