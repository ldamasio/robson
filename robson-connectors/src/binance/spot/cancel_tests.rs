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
const EXCHANGE_INFO: &str = include_str!("testdata/exchange_info_btcusdt.json");
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
            ("GET", "/api/v3/exchangeInfo") => (200, EXCHANGE_INFO.to_string()),
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
    let result = terminal_result(&record, twice, None).unwrap();
    assert_eq!(result.fills().len(), 2);
    let conflicting = vec![
        fill(1, dec!(0.001)),
        fill(1, dec!(0.002)),
        fill(2, dec!(0.001)),
    ];
    assert!(terminal_result(&record, conflicting, None).is_none());
    // §5's evidence rules, trade by trade and against the record's quote: a
    // zero or negative quantity (summing out), a zero price, a negative
    // commission, no asset, a zero record quote with execution, a quote the
    // trades disagree with. Each gives no result; two valid halves give one.
    assert!(
        terminal_result(&record, vec![fill(1, dec!(0.001)), fill(2, dec!(0.001))], None).is_some()
    );
    let invalid: [fn(&mut TradeFill); 5] = [
        |trade| trade.qty = Decimal::ZERO,
        |trade| trade.qty = dec!(-0.5),
        |trade| trade.price = Decimal::ZERO,
        |trade| trade.commission = dec!(-1),
        |trade| trade.commission_asset = String::new(),
    ];
    for edit in invalid {
        let mut bad = fill(1, dec!(0.001));
        edit(&mut bad);
        let rest = TradeFill {
            qty: dec!(1) - bad.qty,
            ..fill(2, dec!(0.001))
        };
        assert!(terminal_result(&record, vec![bad, rest], None).is_none());
    }
    let with_quote = |quote: Option<Decimal>| VenueOrderRecord {
        cumulative_quote_qty: quote,
        ..record.clone()
    };
    let halves = || vec![fill(1, dec!(0.001)), fill(2, dec!(0.001))];
    assert!(terminal_result(&with_quote(Some(Decimal::ZERO)), halves(), None).is_none());
    assert!(terminal_result(&with_quote(Some(dec!(0.11))), halves(), None).is_none());
    // Within R3's tolerance from the cached precision, one unit per trade;
    // with none cached, exactly.
    let near = with_quote(Some(dec!(0.10000002)));
    assert!(terminal_result(&near, halves(), Some(8)).is_some());
    assert!(terminal_result(&near, halves(), None).is_none());
    let priced = terminal_result(&with_quote(None), halves(), None).unwrap();
    assert_eq!(priced.fill_price.as_decimal(), dec!(0.1), "the trades' price");
    // Quote amounts that offset, -0.05 and 0.15 against the record's 0.1: a
    // negative quote is no evidence, whatever the sum. A zero one stays
    // valid (a dust trade's rounding), and R3 reconciles the sum.
    let offsetting = vec![
        TradeFill {
            quote_qty: dec!(-0.05),
            ..fill(1, dec!(0.001))
        },
        TradeFill {
            quote_qty: dec!(0.15),
            ..fill(2, dec!(0.001))
        },
    ];
    assert!(terminal_result(&record, offsetting, None).is_none());
    let dust = vec![fill(1, dec!(0.001)), TradeFill {
        quote_qty: Decimal::ZERO,
        ..fill(2, dec!(0.001))
    }];
    let priced = terminal_result(&with_quote(None), dust, None).unwrap();
    assert_eq!(priced.fill_price.as_decimal(), dec!(0.05));
    // Not terminal, or nothing executed: no result, whatever the trades.
    let live = VenueOrderRecord {
        phase: OrderPhase::Live,
        ..record.clone()
    };
    assert!(
        terminal_result(&live, vec![fill(1, dec!(0.001)), fill(2, dec!(0.001))], None).is_none()
    );
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
        .lookup(200, looked_up("FILLED", "1", "0.1"))
        .trades(trades(1, 4, dec!(0.25)));
    let (venue, seen) = scripted(answers).await;
    let result: OrderResult = venue.order_by_exchange_id(ltc(), "4").await.unwrap().unwrap();
    assert_eq!(result.filled_quantity.as_decimal(), dec!(1));
    assert_eq!(result.fill_price.as_decimal(), dec!(0.1));
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

/// The successor stop under `id`: each test grants exactly this id, so a
/// placement that suppression failed to stop would go out.
fn successor(id: &str) -> StopParams {
    StopParams {
        close_side: OrderSide::Sell,
        quantity: Quantity::new(dec!(1)).unwrap(),
        trigger: Price::new(dec!(0.09)).unwrap(),
        client_order_id: id.to_string(),
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
    let replace = venue.replace_protection(ltc(), &old, successor("ins-2"));
    let outcome = granted(&journal, "ins-2", IntentKind::Protection, replace).await.unwrap();
    assert!(matches!(outcome, ReplaceOutcome::Replaced { old_filled, .. } if old_filled.is_zero()));
    let identities = requests(&seen);
    assert_eq!(identities[0], DELETE_4);
    assert!(identities[1].starts_with("POST /api/v3/order?newClientOrderId=ins-2"));
    assert_eq!(identities.len(), 2);

    let (venue, seen) = scripted(Answers::cancel(503, error_body("unknown"))).await;
    venue.attach_order_journal(Arc::clone(&journal));
    let replace = venue.replace_protection(ltc(), &old, successor("ins-3"));
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
    let replace = venue.replace_protection(ltc(), &old, successor("ins-4"));
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

// =============================================================================
// Round 1 of the code review: evidence rules, completeness, one request
// =============================================================================

/// Completeness is the trades' sum reaching `executedQty`, a full page or
/// not (v16 §3.7): exactly 1,000 and exactly 5,000 trades are complete, with
/// no page read past them; 6,000 are not, within five pages.
#[tokio::test]
async fn trades_are_complete_when_they_cover_the_execution_within_five_pages() {
    for (count, pages) in [(1000, 1), (5000, 5)] {
        let quote = (Decimal::from(count) * dec!(0.1)).to_string();
        let answers = Answers::cancel(500, String::new())
            .lookup(200, looked_up("FILLED", &count.to_string(), &quote))
            .trades(trades(1, count, dec!(1)));
        let (venue, seen) = scripted(answers).await;
        let result = venue.order_by_exchange_id(ltc(), "4").await.unwrap().unwrap();
        assert_eq!(result.fills().len(), count as usize);
        assert_eq!(result.filled_quantity.as_decimal(), Decimal::from(count));
        let mut expected = vec![GET_4.to_string()];
        expected.extend([0, 1001, 2001, 3001, 4001].into_iter().take(pages).map(trades_from));
        assert_eq!(requests(&seen), expected, "{count}");
    }
}

/// The fills rule applies §5's evidence rules to trades: a trade with a
/// zero or negative quantity (even one that makes the sum come out), a zero
/// price, a negative commission or no asset, a zero record quote with
/// execution, or a record quote the trades disagree with gives no fills,
/// and terminality and the executed quantity stand.
#[tokio::test]
async fn trades_that_break_the_evidence_rules_give_no_fills() {
    let edited = |edit: fn(&mut Vec<Value>)| {
        let mut trades = trades(1, 4, dec!(0.25));
        edit(&mut trades);
        trades
    };
    let cases: [(&str, &str, Vec<Value>); 7] = [
        (
            "a zero quantity",
            "0.1",
            edited(|trades| {
                trades[0]["qty"] = json!("0");
                trades[1]["qty"] = json!("0.5");
            }),
        ),
        (
            "a negative quantity summing out",
            "0.1",
            edited(|trades| {
                trades[0]["qty"] = json!("-0.25");
                trades[1]["qty"] = json!("0.75");
            }),
        ),
        ("a zero price", "0.1", edited(|trades| trades[0]["price"] = json!("0"))),
        (
            "a negative commission",
            "0.1",
            edited(|trades| trades[0]["commission"] = json!("-1")),
        ),
        (
            "no commission asset",
            "0.1",
            edited(|trades| trades[0]["commissionAsset"] = json!("")),
        ),
        ("a zero record quote", "0", trades(1, 4, dec!(0.25))),
        ("a record quote the trades disagree with", "0.11", trades(1, 4, dec!(0.25))),
    ];
    for (case, quote, trades) in cases {
        let gone = Answers::cancel(400, error_body("cancel_rejected"))
            .lookup(200, looked_up("FILLED", "1", quote))
            .trades(trades.clone());
        let (venue, _) = scripted(gone).await;
        let outcome = venue.cancel_protection(ltc(), &stop_4()).await.unwrap();
        assert!(
            matches!(outcome, StopCancelOutcome::AlreadyGone {
                final_state: OrderLookup::Terminal { filled_qty, fills: None, .. }
            } if filled_qty == dec!(1)),
            "{case}: {outcome:?}"
        );
        let lookup = Answers::cancel(500, String::new())
            .lookup(200, looked_up("FILLED", "1", quote))
            .trades(trades);
        let (venue, _) = scripted(lookup).await;
        assert!(venue.order_by_exchange_id(ltc(), "4").await.is_err(), "{case}");
    }
}

/// A cancel answered with a redirect is `Unknown`: one DELETE, nothing sent
/// where `Location` points. A symbol that is not one value is refused with
/// nothing sent.
#[tokio::test]
async fn a_cancel_follows_no_redirect_and_refuses_a_malformed_symbol() {
    for status in [301, 302, 303, 307, 308] {
        let (sink, sink_seen) = stand_in::listening(|_| (200, cancelled(|_| {}))).await;
        let (client, seen) = stand_in::redirecting(status, format!("{sink}/api/v3/order")).await;
        let venue = BinanceSpotAdapter::new(client).unwrap();
        let answer = venue.cancel_order_once(ltc(), "4").await.unwrap();
        assert!(matches!(answer, CancelAnswer::Unknown { .. }), "{status}: {answer:?}");
        assert_eq!(requests(&seen), [DELETE_4], "{status}");
        assert_eq!(requests(&sink_seen), Vec::<String>::new(), "{status}: nothing followed");
    }
    let (venue, seen) = scripted(Answers::cancel(200, cancelled(|_| {}))).await;
    let odd = Symbol::new("LTC&orderId=5".to_string(), "BTC".to_string()).unwrap();
    let refused = venue.cancel_order_once(&odd, "4").await;
    assert!(matches!(refused, Err(ExecError::Config(_))), "{refused:?}");
    assert_eq!(requests(&seen), Vec::<String>::new());
}

/// R3's tolerance in the composites comes from the rules already cached
/// (BTCUSDT's quoteAssetPrecision 8: one unit per trade), and is 0 with
/// none cached; no request is made for it.
#[tokio::test]
async fn the_cached_precision_sets_the_composite_tolerance() {
    let btc = Symbol::from_pair("BTCUSDT").unwrap();
    let lookup = looked_up("FILLED", "1", "0.10000004").replace("LTCBTC", "BTCUSDT");
    let on_btc: Vec<Value> = trades(1, 4, dec!(0.25))
        .into_iter()
        .map(|mut trade| {
            trade["symbol"] = json!("BTCUSDT");
            trade
        })
        .collect();
    let answers = || {
        Answers::cancel(500, String::new())
            .lookup(200, lookup.clone())
            .trades(on_btc.clone())
    };
    let (venue, _) = scripted(answers()).await;
    assert!(venue.order_by_exchange_id(&btc, "4").await.is_err(), "no rules cached: exact");
    let (venue, seen) = scripted(answers()).await;
    venue.trading_rules(&btc).await.unwrap();
    let result = venue.order_by_exchange_id(&btc, "4").await.unwrap().unwrap();
    assert_eq!(result.fills().len(), 4);
    assert_eq!(result.fill_price.as_decimal(), dec!(0.10000004));
    let reads: Vec<String> = requests(&seen);
    assert_eq!(reads.iter().filter(|read| read.contains("exchangeInfo")).count(), 1);
    // One unit more than four trades allow: withheld.
    let over = looked_up("FILLED", "1", "0.10000005").replace("LTCBTC", "BTCUSDT");
    let (venue, _) =
        scripted(Answers::cancel(500, String::new()).lookup(200, over).trades(on_btc.clone()))
            .await;
    venue.trading_rules(&btc).await.unwrap();
    assert!(venue.order_by_exchange_id(&btc, "4").await.is_err());
}

/// A trade page whose quote amounts offset, -1 and 2 against an order that
/// executed 2 for a quote of 1, is no evidence: the page is an error, the
/// composites withhold the fills and keep terminality and the executed
/// quantity. A zero quote amount on its own is a valid page.
#[tokio::test]
async fn a_negative_trade_quote_is_no_evidence() {
    let mut offsetting = trades(1, 2, dec!(1));
    for (trade, quote) in offsetting.iter_mut().zip(["-1", "2"]) {
        trade["price"] = json!("0.5");
        trade["quoteQty"] = json!(quote);
    }
    let (venue, _) = scripted(Answers::cancel(500, String::new()).trades(offsetting.clone())).await;
    assert!(venue.order_trades_page(ltc(), "4", 0).await.is_err(), "the page itself");
    let gone = Answers::cancel(400, error_body("cancel_rejected"))
        .lookup(200, looked_up("FILLED", "2", "1"))
        .trades(offsetting.clone());
    let (venue, _) = scripted(gone).await;
    let outcome = venue.cancel_protection(ltc(), &stop_4()).await.unwrap();
    assert!(
        matches!(outcome, StopCancelOutcome::AlreadyGone {
            final_state: OrderLookup::Terminal { filled_qty, fills: None, .. }
        } if filled_qty == dec!(2)),
        "{outcome:?}"
    );
    let lookup = Answers::cancel(500, String::new())
        .lookup(200, looked_up("FILLED", "2", "1"))
        .trades(offsetting);
    let (venue, _) = scripted(lookup).await;
    assert!(venue.order_by_exchange_id(ltc(), "4").await.is_err());

    let mut dust = trades(1, 2, dec!(1));
    dust[1]["quoteQty"] = json!("0");
    let (venue, _) = scripted(Answers::cancel(500, String::new()).trades(dust)).await;
    let page = venue.order_trades_page(ltc(), "4", 0).await.unwrap();
    assert_eq!(page.trades[1].quote_qty, Decimal::ZERO);
}
