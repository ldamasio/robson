//! ADR-0056 step 8c-3a: spot order placement on the wire, against a local
//! stand-in for Binance: the one-attempt ledger and the gate, the request's
//! parameters, every answer's outcome and the evidence it carries (8c plan
//! §4, §5, §11). Nothing reaches the network.

use std::{
    future::Future,
    sync::{Arc, OnceLock},
    time::Duration,
};

use robson_domain::{OrderSide, Price, Quantity, Symbol};
use robson_exec::{
    order_journal::{IntentKind, IntentSpec, OrderJournal, StubJournalBackend, WriteClass},
    ports::{FeeEvidence, FeeGap},
    venue::{
        AttemptEvidence, EntryOrReduce, OrderPhase, PlacedStop, ProtectionKind, TradingPort, Venue,
        VenueId,
    },
    ExecError, OrderResult,
};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde_json::{json, Value};
use tokio::net::TcpListener;

use super::{
    stand_in::{self, Seen, Seens},
    BinanceSpotAdapter, SpotWrite, SPOT_STP_MODE,
};
use crate::binance_rest::BinanceRestClient;

const EXCHANGE_INFO: &str = include_str!("testdata/exchange_info_btcusdt.json");
const ORDER_FULL: &str = include_str!("testdata/order_full.json");
const ORDER_RESULT: &str = include_str!("testdata/order_result.json");
const ERRORS: &str = include_str!("testdata/errors.json");

/// The fills' quote in the documented `FULL` answer: Σ price × qty.
const FILLS_QUOTE: Decimal = dec!(39983);

fn btc() -> &'static Symbol {
    static BTC: OnceLock<Symbol> = OnceLock::new();
    BTC.get_or_init(|| Symbol::from_pair("BTCUSDT").unwrap())
}

fn qty(value: Decimal) -> Quantity {
    Quantity::new(value).unwrap()
}

fn error_body(name: &str) -> String {
    serde_json::from_str::<Value>(ERRORS).unwrap()[name]
        .as_str()
        .unwrap()
        .to_string()
}

fn requests(seen: &Seens) -> Vec<String> {
    seen.lock().unwrap().iter().map(Seen::identity).collect()
}

fn posts(seen: &Seens) -> usize {
    seen.lock().unwrap().iter().filter(|request| request.method == "POST").count()
}

/// A one-attempt journal for the spot venue, over the stub backend.
fn spot_journal() -> Arc<OrderJournal> {
    let journal = OrderJournal::new(
        Arc::new(StubJournalBackend::default()),
        &VenueId::new(VenueId::BINANCE_SPOT),
    );
    Arc::new(journal.with_one_attempt_per_client_id(true))
}

/// The spot venue on a stand-in whose rules are BTCUSDT's and whose order
/// endpoint answers with `order`, with a one-attempt journal attached.
async fn venue(
    order: impl Fn(&Seen) -> (u16, String) + Send + Sync + 'static,
) -> (BinanceSpotAdapter, Seens, Arc<OrderJournal>) {
    let (client, seen) = stand_in::serve(move |request| match request.path.as_str() {
        "/api/v3/exchangeInfo" => (200, EXCHANGE_INFO.to_string()),
        "/api/v3/order" => order(request),
        other => panic!("unexpected request {other}"),
    })
    .await;
    let venue = BinanceSpotAdapter::new(client).unwrap();
    let journal = spot_journal();
    venue.attach_order_journal(Arc::clone(&journal));
    (venue, seen, journal)
}

/// Run `place` as the owner will (8c plan §7): inside one operation, under a
/// fresh Strict grant for `id`, the guard held across the one request.
async fn granted<T>(
    journal: &Arc<OrderJournal>,
    id: &str,
    kind: IntentKind,
    place: impl Future<Output = T>,
) -> T {
    let operation = journal.begin_operation();
    operation
        .scope(async {
            let intent = journal.open_intent(IntentSpec {
                position_id: None,
                kind,
                client_order_id: id.to_string(),
                cancels_order_id: None,
                params: json!({}),
            });
            let _guard = journal.authorize_send(&intent, WriteClass::Strict).await.unwrap();
            place.await
        })
        .await
}

/// The parameters an answer echoes from its request.
fn echo(answer: &mut Value, request: &Seen) {
    for (field, param) in [
        ("symbol", "symbol"),
        ("side", "side"),
        ("type", "type"),
        ("origQty", "quantity"),
        ("clientOrderId", "newClientOrderId"),
        ("selfTradePreventionMode", "selfTradePreventionMode"),
    ] {
        answer[field] = json!(request.param(param).unwrap());
    }
    if let Some(stop) = request.param("stopPrice") {
        answer["stopPrice"] = json!(stop);
    }
}

/// The documented `FULL` answer to `request` (10 executed in five fills),
/// made coherent (its quote the fills' 39983), then `edit`ed.
fn full(request: &Seen, edit: impl FnOnce(&mut Value)) -> String {
    let mut answer: Value = serde_json::from_str(ORDER_FULL).unwrap();
    echo(&mut answer, request);
    answer["cummulativeQuoteQty"] = json!("39983.00000000");
    edit(&mut answer);
    answer.to_string()
}

/// The documented `RESULT` answer to a stop `request`, resting with nothing
/// executed, then `edit`ed.
fn result(request: &Seen, edit: impl FnOnce(&mut Value)) -> String {
    let mut answer: Value = serde_json::from_str(ORDER_RESULT).unwrap();
    echo(&mut answer, request);
    answer["status"] = json!("NEW");
    answer["executedQty"] = json!("0.00000000");
    answer["cummulativeQuoteQty"] = json!("0.00000000");
    edit(&mut answer);
    answer.to_string()
}

/// A venue answering every order with a `FULL` answer edited by `edit`.
async fn answering_full(
    edit: impl Fn(&mut Value) + Send + Sync + 'static,
) -> (BinanceSpotAdapter, Seens, Arc<OrderJournal>) {
    venue(move |request| (200, full(request, &edit))).await
}

/// An entry: BUY 10 BTCUSDT under `id`, granted.
async fn entry(
    venue: &BinanceSpotAdapter,
    journal: &Arc<OrderJournal>,
    id: &str,
) -> Result<OrderResult, ExecError> {
    let place =
        venue.place_market_order(btc(), OrderSide::Buy, qty(dec!(10)), id, EntryOrReduce::Entry);
    granted(journal, id, IntentKind::Entry, place).await
}

/// A reduction: SELL `quantity` BTCUSDT under `id`, granted.
async fn reduction(
    venue: &BinanceSpotAdapter,
    journal: &Arc<OrderJournal>,
    id: &str,
    quantity: Decimal,
) -> Result<OrderResult, ExecError> {
    let place =
        venue.place_market_order(btc(), OrderSide::Sell, qty(quantity), id, EntryOrReduce::Reduce);
    granted(journal, id, IntentKind::Exit, place).await
}

/// A stop: SELL 0.5 BTCUSDT at 80000 under `id`, granted.
async fn stop(
    venue: &BinanceSpotAdapter,
    journal: &Arc<OrderJournal>,
    id: &str,
) -> Result<PlacedStop, ExecError> {
    let trigger = Price::new(dec!(80000)).unwrap();
    let place = venue.place_protection(btc(), OrderSide::Sell, qty(dec!(0.5)), trigger, id);
    granted(journal, id, IntentKind::Protection, place).await
}

fn unresolved(venue: &BinanceSpotAdapter, id: &str) -> bool {
    matches!(venue.ledger().evidence(id), AttemptEvidence::Unresolved {
        unresolved: 1,
        rejected: 0
    })
}

fn rejected(venue: &BinanceSpotAdapter, id: &str) -> bool {
    venue.ledger().evidence(id) == AttemptEvidence::AllRejected { rejected: 1 }
}

// =============================================================================
// The wire: one request, its documented parameters, through ledger and gate
// =============================================================================

/// Wire goldens: each order is one POST with `newClientOrderId`, an explicit
/// response type and `selfTradePreventionMode=EXPIRE_MAKER`, never
/// `reduceOnly` or `timeInForce`, signed and keyed. An entry reads fresh rules
/// first; a reduction and a stop read nothing.
#[tokio::test]
async fn every_order_is_one_request_with_its_documented_parameters() {
    let (venue, seen, journal) = venue(|request| match request.param("type") {
        Some("STOP_LOSS") => (200, result(request, |_| {})),
        _ => (200, full(request, |_| {})),
    })
    .await;
    entry(&venue, &journal, "e-1").await.unwrap();
    reduction(&venue, &journal, "x-1", dec!(10)).await.unwrap();
    stop(&venue, &journal, "ins-1").await.unwrap();
    assert_eq!(requests(&seen), [
        "GET /api/v3/exchangeInfo?symbol=BTCUSDT",
        "POST /api/v3/order?newClientOrderId=e-1&newOrderRespType=FULL&quantity=10&\
         selfTradePreventionMode=EXPIRE_MAKER&side=BUY&symbol=BTCUSDT&type=MARKET",
        "POST /api/v3/order?newClientOrderId=x-1&newOrderRespType=FULL&quantity=10&\
         selfTradePreventionMode=EXPIRE_MAKER&side=SELL&symbol=BTCUSDT&type=MARKET",
        "POST /api/v3/order?newClientOrderId=ins-1&newOrderRespType=RESULT&quantity=0.5&\
         selfTradePreventionMode=EXPIRE_MAKER&side=SELL&stopPrice=80000&symbol=BTCUSDT&\
         type=STOP_LOSS",
    ]);
    for request in seen.lock().unwrap().iter().filter(|request| request.method == "POST") {
        assert!(request.api_key, "{request:?}");
        assert!(request.param("timestamp").is_some() && request.param("signature").is_some());
        for never in ["reduceOnly", "timeInForce", "quoteOrderQty", "price"] {
            assert!(request.param(never).is_none(), "{never} in {request:?}");
        }
    }
    assert_eq!(SPOT_STP_MODE, "EXPIRE_MAKER");
    assert_eq!(SpotWrite::NEW_ORDER, SpotWrite {
        path: "/api/v3/order",
        weight: 1,
        orders: 1
    });
}

/// Nothing goes out without the journal's grant, and never twice under one
/// client id: no journal, no authorization, a second placement after an
/// answer, and a second one after an uncertain first attempt (the duplicate
/// Binance would answer with "Duplicate order sent." never leaves).
#[tokio::test]
async fn nothing_is_sent_without_a_grant_or_twice_under_one_id() {
    let (client, seen) = stand_in::serve(|request| (200, full(request, |_| {}))).await;
    let bare = BinanceSpotAdapter::new(client).unwrap();
    let refused = bare.place_market_order(
        btc(),
        OrderSide::Sell,
        qty(dec!(10)),
        "x-0",
        EntryOrReduce::Reduce,
    );
    assert!(matches!(refused.await, Err(ExecError::SendRefused(_))));
    let trigger = Price::new(dec!(80000)).unwrap();
    let refused = bare.place_protection(btc(), OrderSide::Sell, qty(dec!(1)), trigger, "s-0");
    assert!(matches!(refused.await, Err(ExecError::SendRefused(_))));
    assert_eq!(requests(&seen), Vec::<String>::new());

    let (venue, seen, journal) = venue(|request| match request.param("newClientOrderId") {
        Some("x-2") => (503, error_body("unknown")),
        _ => (200, full(request, |_| {})),
    })
    .await;
    let ungranted = venue.place_market_order(
        btc(),
        OrderSide::Sell,
        qty(dec!(10)),
        "x-1",
        EntryOrReduce::Reduce,
    );
    let operation = journal.begin_operation();
    assert!(matches!(operation.scope(ungranted).await, Err(ExecError::SendRefused(_))));
    assert_eq!(posts(&seen), 0);

    reduction(&venue, &journal, "x-2", dec!(10)).await.unwrap_err();
    assert_eq!(posts(&seen), 1);
    assert!(unresolved(&venue, "x-2"));
    reduction(&venue, &journal, "x-3", dec!(10)).await.unwrap();
    assert_eq!(posts(&seen), 2);
    for id in ["x-2", "x-3"] {
        // Again under the same id: the journal grants no second attempt, and
        // the ledger takes no grant.
        let again = venue.place_market_order(
            btc(),
            OrderSide::Sell,
            qty(dec!(10)),
            id,
            EntryOrReduce::Reduce,
        );
        assert!(matches!(operation.scope(again).await, Err(ExecError::SendRefused(_))));
    }
    assert_eq!(posts(&seen), 2, "one request per client id");
}

/// SD12's choke point: a placement with its grant still passes the gate, and
/// a stopping gate with no permit lets nothing reach the listener.
#[tokio::test]
async fn spot_mutations_pass_the_gate() {
    let (venue, seen, journal) = venue(|request| match request.param("type") {
        Some("STOP_LOSS") => (200, result(request, |_| {})),
        _ => (200, full(request, |_| {})),
    })
    .await;
    let stopping = OrderJournal::unpersisted();
    venue.attach_gate(Arc::clone(stopping.gate()));
    stopping.gate().begin_draining();
    let refused = reduction(&venue, &journal, "x-1", dec!(10)).await;
    assert!(matches!(refused, Err(ExecError::SendRefused(_))), "{refused:?}");
    let refused = stop(&venue, &journal, "ins-1").await;
    assert!(matches!(refused, Err(ExecError::SendRefused(_))), "{refused:?}");
    assert_eq!(posts(&seen), 0, "nothing refused reached the venue");
    // The attempt stays unresolved: its id is spent, and nothing is concluded.
    assert!(unresolved(&venue, "x-1") && unresolved(&venue, "ins-1"));
}

/// Refused before anything is sent: spot has no short, so an entry never
/// sells and a reduction never buys; a stop never buys; an entry on a symbol
/// that does not allow `EXPIRE_MAKER`, or whose rules cannot be read, or
/// whose quantity the rules refuse.
#[tokio::test]
async fn what_spot_cannot_send_is_refused_before_sending() {
    let (venue, seen, journal) = answering_full(|_| {}).await;
    let short_entry =
        venue.place_market_order(btc(), OrderSide::Sell, qty(dec!(1)), "a", EntryOrReduce::Entry);
    assert!(matches!(short_entry.await, Err(ExecError::Config(_))));
    let buying_reduction =
        venue.place_market_order(btc(), OrderSide::Buy, qty(dec!(1)), "b", EntryOrReduce::Reduce);
    assert!(matches!(buying_reduction.await, Err(ExecError::Config(_))));
    let trigger = Price::new(dec!(80000)).unwrap();
    let buy_stop = venue.place_protection(btc(), OrderSide::Buy, qty(dec!(1)), trigger, "c");
    assert!(matches!(buy_stop.await, Err(ExecError::Config(_))));
    // Below the lot's minimum: the quantity rule refuses it, locally.
    let tiny = venue.place_market_order(
        btc(),
        OrderSide::Buy,
        qty(dec!(0.000001)),
        "d",
        EntryOrReduce::Entry,
    );
    let tiny = granted(&journal, "d", IntentKind::Entry, tiny).await;
    assert!(matches!(tiny, Err(ExecError::OrderRejected(_))), "{tiny:?}");
    assert_eq!(venue.ledger().evidence("d"), AttemptEvidence::NoRecord);
    assert_eq!(posts(&seen), 0);

    // A symbol whose STP modes lack EXPIRE_MAKER.
    let no_maker = EXCHANGE_INFO.replace(r#""EXPIRE_TAKER","EXPIRE_MAKER","#, r#""EXPIRE_TAKER","#);
    assert!(no_maker.len() < EXCHANGE_INFO.len(), "the fixture's STP modes moved");
    let (client, seen) = stand_in::serve(move |request| match request.path.as_str() {
        "/api/v3/exchangeInfo" => (200, no_maker.clone()),
        _ => (200, full(request, |_| {})),
    })
    .await;
    let venue = BinanceSpotAdapter::new(client).unwrap();
    let journal = spot_journal();
    venue.attach_order_journal(Arc::clone(&journal));
    let refused = entry(&venue, &journal, "e-1").await;
    assert!(matches!(refused, Err(ExecError::Config(_))), "{refused:?}");
    assert_eq!(requests(&seen), ["GET /api/v3/exchangeInfo?symbol=BTCUSDT"]);

    // Rules that cannot be read: admission fails closed.
    let (client, seen) = stand_in::serve(|request| match request.path.as_str() {
        "/api/v3/exchangeInfo" => (503, error_body("unknown")),
        _ => (200, full(request, |_| {})),
    })
    .await;
    let venue = BinanceSpotAdapter::new(client).unwrap();
    let journal = spot_journal();
    venue.attach_order_journal(Arc::clone(&journal));
    assert!(entry(&venue, &journal, "e-1").await.is_err());
    assert_eq!(requests(&seen), ["GET /api/v3/exchangeInfo?symbol=BTCUSDT"]);
}

// =============================================================================
// MARKET answers
// =============================================================================

/// `FILLED` is the result of `executedQty` at the record's price, with the
/// record, every fill (no time: a placement answer carries none) and the
/// commission total in its one asset.
#[tokio::test]
async fn a_filled_market_order_is_its_record_fills_and_fee_total() {
    let (venue, seen, journal) = answering_full(|_| {}).await;
    let result = entry(&venue, &journal, "e-1").await.unwrap();
    assert_eq!(result.exchange_order_id, "28");
    assert_eq!(result.client_order_id, "e-1");
    assert_eq!(result.filled_quantity.as_decimal(), dec!(10));
    assert_eq!(result.fill_price.as_decimal(), dec!(3998.3));
    assert_eq!(result.fee_evidence(), FeeEvidence::Total);
    assert_eq!(result.fee_total(), Some((dec!(39.983), "USDT")));
    assert_eq!(result.filled_at.timestamp_millis(), 1507725176595);
    let record = result.record().unwrap();
    assert_eq!((record.status.as_str(), record.phase), ("FILLED", OrderPhase::Terminal));
    assert_eq!((record.side, record.order_type.as_str()), (OrderSide::Buy, "MARKET"));
    assert_eq!(record.protection_kind, None);
    assert_eq!(record.cumulative_quote_qty, Some(FILLS_QUOTE));
    assert_eq!(record.transact_time_ms, Some(1507725176595));
    assert_eq!(record.working_time_ms, Some(1507725176595));
    let fills = result.fills();
    assert_eq!(fills.len(), 5);
    assert_eq!((fills[1].trade_id.as_str(), fills[1].quote_qty), ("57", dec!(19995)));
    assert!(fills.iter().all(|fill| fill.time.is_none()));
    assert!(unresolved(&venue, "e-1"), "an accepted order stays unresolved");
    assert_eq!(posts(&seen), 1);

    // It serializes with its evidence, and reads back the same.
    let json = serde_json::to_value(&result).unwrap();
    for field in ["fills", "record", "fee_evidence"] {
        assert!(json.get(field).is_some(), "{field} in {json}");
    }
    let back: OrderResult = serde_json::from_value(json).unwrap();
    assert_eq!(back.evidence, result.evidence);
}

/// Every MARKET status (enums.md L45-L57; market orders FAQ L15-L19): only
/// `FILLED` or `EXPIRED` with execution are results; `EXPIRED` with nothing
/// executed is `ExpiredUnfilled`, never a rejection; everything else is
/// uncertain. One request each, and the attempt stays unresolved.
#[tokio::test]
async fn every_market_status_has_its_outcome() {
    let cases: [(&str, &str, &str); 11] = [
        ("FILLED", "10", "Ok"),
        ("EXPIRED", "4", "Ok"),
        ("EXPIRED", "0", "ExpiredUnfilled"),
        ("FILLED", "0", "uncertain"),
        ("CANCELED", "4", "uncertain"),
        ("REJECTED", "0", "uncertain"),
        ("EXPIRED_IN_MATCH", "0", "uncertain"),
        ("NEW", "0", "uncertain"),
        ("PARTIALLY_FILLED", "4", "uncertain"),
        ("PENDING_CANCEL", "4", "uncertain"),
        ("SOMETHING_NEW", "4", "uncertain"),
    ];
    for (status, executed, expected) in cases {
        let (venue, seen, journal) = answering_full(move |answer| {
            answer["status"] = json!(status);
            answer["executedQty"] = json!(executed);
            if executed == "4" {
                // Three of the five fills: 1 + 2 + 1 at 4000, 3998 and 3997.
                let fills = answer["fills"].as_array_mut().unwrap();
                fills.remove(1);
                fills.remove(3);
                answer["cummulativeQuoteQty"] = json!("15993");
            }
            if executed == "0" {
                answer["fills"] = json!([]);
                answer["cummulativeQuoteQty"] = json!("0");
            }
        })
        .await;
        let outcome = entry(&venue, &journal, "e-1").await;
        let name = match &outcome {
            Ok(result) => {
                let executed: Decimal = executed.parse().unwrap();
                assert_eq!(result.filled_quantity.as_decimal(), executed);
                assert_eq!(result.record().unwrap().status, status);
                assert_eq!(result.fee_evidence(), FeeEvidence::Total, "{status}");
                "Ok"
            },
            Err(ExecError::ExpiredUnfilled(record)) => {
                assert_eq!((record.status.as_str(), record.executed_qty), ("EXPIRED", dec!(0)));
                assert_eq!(record.phase, OrderPhase::Terminal);
                "ExpiredUnfilled"
            },
            Err(ExecError::Exchange(_)) => "uncertain",
            Err(other) => panic!("{status} {executed}: {other:?}"),
        };
        assert_eq!(name, expected, "{status} {executed}");
        assert_eq!(posts(&seen), 1, "{status}");
        assert!(unresolved(&venue, "e-1"), "{status} {executed}: the attempt stays unresolved");
    }
}

/// A 200 that contradicts the request, or itself, established nothing:
/// another symbol, client id, side, type, quantity or STP mode, an order
/// list, more executed than ordered, an impossible time, or a body that is
/// not the answer.
#[tokio::test]
async fn a_contradicting_answer_is_uncertain() {
    let edits: [(&str, Value); 9] = [
        ("symbol", json!("ETHUSDT")),
        ("clientOrderId", json!("someone-else")),
        ("side", json!("SELL")),
        ("type", json!("LIMIT")),
        ("origQty", json!("11")),
        ("selfTradePreventionMode", json!("NONE")),
        ("orderListId", json!(7)),
        ("executedQty", json!("10.1")),
        ("transactTime", json!(i64::MAX)),
    ];
    for (field, value) in edits {
        let (venue, seen, journal) =
            answering_full(move |answer| answer[field] = value.clone()).await;
        let outcome = entry(&venue, &journal, "e-1").await;
        assert!(matches!(outcome, Err(ExecError::Exchange(_))), "{field}: {outcome:?}");
        assert_eq!(posts(&seen), 1);
        assert!(unresolved(&venue, "e-1"), "{field}");
    }
    for body in ["{}", "not json", "[]"] {
        let (venue, _, journal) = venue(move |_| (200, body.to_string())).await;
        let outcome = entry(&venue, &journal, "e-1").await;
        assert!(matches!(outcome, Err(ExecError::Exchange(_))), "{body}: {outcome:?}");
        assert!(unresolved(&venue, "e-1"));
    }
}

/// Only an exact 200 is a success; HTTP 400 with -2010 or -1013 is the only
/// rejection; the same codes at any other status, -1006, -1007, a 5XX with
/// or without JSON, and another success establish nothing.
#[tokio::test]
async fn only_400_with_the_finality_codes_rejects_and_only_200_succeeds() {
    let mut cases: Vec<(u16, String, &str)> = vec![
        (400, error_body("new_order_rejected"), "OrderRejected"),
        (400, error_body("duplicate_order"), "OrderRejected"),
        (400, error_body("filter_failure"), "RulesRejected"),
        (400, error_body("unexpected_resp"), "uncertain"),
        (400, error_body("backend_timeout"), "uncertain"),
        (400, error_body("bad_symbol"), "uncertain"),
        (500, error_body("unexpected_resp"), "uncertain"),
        (503, error_body("backend_timeout"), "uncertain"),
        (502, error_body("html_page"), "uncertain"),
        (503, error_body("unknown"), "uncertain"),
        (403, error_body("html_page"), "uncertain"),
    ];
    for status in [403, 418, 429, 500, 503] {
        for name in ["new_order_rejected", "filter_failure"] {
            cases.push((status, error_body(name), "uncertain"));
        }
    }
    for (status, body, expected) in cases {
        let (venue, seen, journal) = venue(move |_| (status, body.clone())).await;
        let outcome = reduction(&venue, &journal, "x-1", dec!(10)).await;
        let name = match &outcome {
            Err(ExecError::OrderRejected(_)) => "OrderRejected",
            Err(ExecError::RulesRejected(_)) => "RulesRejected",
            Err(ExecError::Exchange(_)) => "uncertain",
            other => panic!("{status}: {other:?}"),
        };
        assert_eq!(name, expected, "{status} {outcome:?}");
        assert_eq!(posts(&seen), 1);
        if expected == "uncertain" {
            assert!(unresolved(&venue, "x-1"), "{status} {expected}");
        } else {
            assert!(rejected(&venue, "x-1"), "{status} {expected}");
        }
    }
    // A success other than 200, with the very answer a 200 would carry.
    for status in [201, 202, 204] {
        let (venue, seen, journal) = venue(move |request| (status, full(request, |_| {}))).await;
        let outcome = reduction(&venue, &journal, "x-1", dec!(10)).await;
        assert!(matches!(outcome, Err(ExecError::Exchange(_))), "{status}: {outcome:?}");
        assert_eq!(posts(&seen), 1);
        assert!(unresolved(&venue, "x-1"), "{status}");
    }
}

/// No answer at all: a timeout stays a timeout, a failed transport is
/// uncertain, and both leave the attempt unresolved.
#[tokio::test]
async fn a_timeout_or_a_failed_transport_is_uncertain() {
    // A venue that takes the request and never answers.
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
    let journal = spot_journal();
    venue.attach_order_journal(Arc::clone(&journal));
    let outcome = reduction(&venue, &journal, "x-1", dec!(10)).await;
    assert!(matches!(outcome, Err(ExecError::Timeout(_))), "{outcome:?}");
    assert!(unresolved(&venue, "x-1"));

    let client = Arc::new(BinanceRestClient::with_local_base_url(stand_in::closing().await));
    let venue = BinanceSpotAdapter::new(client).unwrap();
    let journal = spot_journal();
    venue.attach_order_journal(Arc::clone(&journal));
    let outcome = reduction(&venue, &journal, "x-1", dec!(10)).await;
    assert!(matches!(outcome, Err(ExecError::Exchange(_))), "{outcome:?}");
    assert!(unresolved(&venue, "x-1"));
}

// =============================================================================
// MARKET evidence (8c plan §5)
// =============================================================================

/// What a MARKET answer's evidence came to.
fn evidence(result: &OrderResult) -> (Decimal, usize, FeeEvidence, Option<(Decimal, String)>) {
    (
        result.fill_price.as_decimal(),
        result.fills().len(),
        result.fee_evidence(),
        result.fee_total().map(|(fee, asset)| (fee, asset.to_string())),
    )
}

/// A reduction of 10 whose answer is the coherent `FULL` one, edited.
async fn reduced(
    edit: impl Fn(&mut Value) + Send + Sync + 'static,
) -> Result<OrderResult, ExecError> {
    let (venue, seen, journal) = answering_full(edit).await;
    let outcome = reduction(&venue, &journal, "x-1", dec!(10)).await;
    assert_eq!(requests(&seen).len(), 1, "no rules read for a reduction");
    outcome
}

fn total(fee: Decimal) -> Option<(Decimal, String)> {
    Some((fee, "USDT".to_string()))
}

/// Rule 4: the record's quote over `executedQty` first, the complete fills'
/// quote otherwise, never an estimate. A negative quote is unavailable, never
/// zero; a zero quote with execution is inconsistent.
#[tokio::test]
async fn the_price_is_the_record_s_then_the_complete_fills_never_estimated() {
    let unavailable = reduced(|answer| answer["cummulativeQuoteQty"] = json!("-1.00000000"));
    let result = unavailable.await.unwrap();
    assert_eq!(evidence(&result), (dec!(3998.3), 5, FeeEvidence::Total, total(dec!(39.983))));
    assert_eq!(result.record().unwrap().cumulative_quote_qty, None);

    // An unavailable quote with incomplete fills, or none: no price.
    let incomplete = reduced(|answer| {
        answer["cummulativeQuoteQty"] = json!("-1");
        answer["fills"].as_array_mut().unwrap().pop();
    });
    assert!(matches!(incomplete.await, Err(ExecError::Exchange(_))));
    let no_fills = reduced(|answer| {
        answer["cummulativeQuoteQty"] = json!("-1");
        answer.as_object_mut().unwrap().remove("fills");
    });
    assert!(matches!(no_fills.await, Err(ExecError::Exchange(_))));
    let zero = reduced(|answer| answer["cummulativeQuoteQty"] = json!("0.00000000"));
    assert!(matches!(zero.await, Err(ExecError::Exchange(_))));
}

/// Rule 3 and C5: fees are a total only over complete, consistent fills in
/// one asset; otherwise they are unavailable, with the reason, and the
/// quantity and price stand. Never `(0, "MIXED")`.
#[tokio::test]
async fn fees_are_a_total_only_over_complete_consistent_fills_in_one_asset() {
    let incomplete = reduced(|answer| {
        answer["fills"].as_array_mut().unwrap().pop();
    });
    let result = incomplete.await.unwrap();
    let gap = FeeEvidence::Unavailable(FeeGap::Incomplete);
    assert_eq!(evidence(&result), (dec!(3998.3), 4, gap, None));
    assert_eq!(result.filled_quantity.as_decimal(), dec!(10));

    for empty in [json!([]), Value::Null] {
        let result = reduced(move |answer| answer["fills"] = empty.clone()).await.unwrap();
        let gap = FeeEvidence::Unavailable(FeeGap::NoFills);
        assert_eq!(evidence(&result), (dec!(3998.3), 0, gap, None));
    }

    let mixed = reduced(|answer| answer["fills"][2]["commissionAsset"] = json!("BNB"));
    let result = mixed.await.unwrap();
    let gap = FeeEvidence::Unavailable(FeeGap::MixedAssets);
    assert_eq!(evidence(&result), (dec!(3998.3), 5, gap, None));
    assert_ne!(result.fee_asset, "MIXED");
    assert_eq!(result.fee, Decimal::ZERO);

    // An identical duplicate counts once; a different one under the same id,
    // more than executed, or an invalid fill conflicts.
    let duplicate = reduced(|answer| {
        let first = answer["fills"][0].clone();
        answer["fills"].as_array_mut().unwrap().push(first);
    });
    let result = duplicate.await.unwrap();
    assert_eq!(evidence(&result), (dec!(3998.3), 5, FeeEvidence::Total, total(dec!(39.983))));
    let conflicting = FeeEvidence::Unavailable(FeeGap::Conflicting);
    let edits: [fn(&mut Value); 6] = [
        |answer| {
            let mut other = answer["fills"][0].clone();
            other["commission"] = json!("4.5");
            answer["fills"].as_array_mut().unwrap().push(other);
        },
        |answer| answer["fills"][0]["qty"] = json!("1.5"),
        |answer| answer["fills"][0]["qty"] = json!("0"),
        |answer| answer["fills"][0]["price"] = json!("0"),
        |answer| answer["fills"][0]["commission"] = json!("-0.1"),
        |answer| answer["fills"][0]["commissionAsset"] = json!(""),
    ];
    for edit in edits {
        let result = reduced(edit).await.unwrap();
        assert_eq!(evidence(&result), (dec!(3998.3), 0, conflicting, None));
    }
}

/// The documented `FULL` example is incoherent: its quote is 10, its fills'
/// 39983. The record's price stands, the fills are dropped and the fees are
/// unavailable: the trade pages are the authority.
#[tokio::test]
async fn the_documented_incoherent_answer_keeps_the_record_s_price() {
    let result = reduced(|answer| answer["cummulativeQuoteQty"] = json!("10.00000000")).await;
    let conflicting = FeeEvidence::Unavailable(FeeGap::Conflicting);
    assert_eq!(evidence(&result.unwrap()), (dec!(1), 0, conflicting, None));
}

/// R3 (rule 5): the record's quote and the fills' agree within one unit of
/// `quoteAssetPrecision` per fill, from the rules already cached and with no
/// request for them; with none cached, exactly.
#[tokio::test]
async fn the_quotes_agree_within_one_precision_unit_per_fill() {
    let conflicting = FeeEvidence::Unavailable(FeeGap::Conflicting);
    // An entry caches the rules: quoteAssetPrecision 8, five fills.
    for (quote, fees) in [
        ("39983.00000005", FeeEvidence::Total),
        ("39982.99999995", FeeEvidence::Total),
        ("39983.00000006", conflicting),
        ("39982.99999994", conflicting),
    ] {
        let (venue, seen, journal) =
            answering_full(move |answer| answer["cummulativeQuoteQty"] = json!(quote)).await;
        let result = entry(&venue, &journal, "e-1").await.unwrap();
        assert_eq!(result.fee_evidence(), fees, "{quote}");
        let quote: Decimal = quote.parse().unwrap();
        assert_eq!(result.fill_price.as_decimal(), quote / dec!(10));
        assert_eq!(requests(&seen).len(), 2);
    }
    // A reduction with no rules cached: tolerance 0, and no request for them.
    for (quote, fees) in [
        ("39983", FeeEvidence::Total),
        ("39983.00000001", conflicting),
    ] {
        let result = reduced(move |answer| answer["cummulativeQuoteQty"] = json!(quote))
            .await
            .unwrap();
        assert_eq!(result.fee_evidence(), fees, "{quote}");
    }
}

/// Rule 6: every product, sum and quotient is checked; an overflow makes the
/// derived value unavailable, never a panic.
#[tokio::test]
async fn an_overflow_makes_the_derived_value_unavailable() {
    let max = Decimal::MAX.to_string();
    let price_max = max.clone();
    // One fill whose price × qty overflows.
    let result = reduced(move |answer| {
        answer["fills"] = json!([{
            "price": price_max, "qty": "10", "commission": "1",
            "commissionAsset": "USDT", "tradeId": 1
        }]);
    })
    .await
    .unwrap();
    assert_eq!(result.fee_evidence(), FeeEvidence::Unavailable(FeeGap::Overflow));
    assert!(result.fills().is_empty());
    // Commissions whose sum overflows: the fills stand, the total does not.
    let commission = max.clone();
    let result = reduced(move |answer| {
        for fill in answer["fills"].as_array_mut().unwrap() {
            fill["commission"] = json!(commission);
        }
    })
    .await
    .unwrap();
    assert_eq!(result.fee_evidence(), FeeEvidence::Unavailable(FeeGap::Overflow));
    assert_eq!(result.fills().len(), 5);
    // A price whose quotient overflows: no price, an uncertain answer.
    let tiny = dec!(0.0000000000000000000000000001);
    let (venue, _, journal) = venue(move |request| {
        let quote = max.clone();
        (
            200,
            full(request, |answer| {
                answer["executedQty"] = json!("0.0000000000000000000000000001");
                answer["cummulativeQuoteQty"] = json!(quote);
                answer["fills"] = json!([]);
            }),
        )
    })
    .await;
    let outcome = reduction(&venue, &journal, "x-1", tiny).await;
    assert!(matches!(outcome, Err(ExecError::Exchange(_))), "{outcome:?}");
}

// =============================================================================
// STOP_LOSS answers
// =============================================================================

/// Any 200 that echoes the request is `Ok` with the stop's record, whatever
/// its status: a resting stop with nothing executed echoes the trigger,
/// carries quantity 0 and a `Total` fee of 0 in the quote asset; an executing
/// or executed stop carries its `executedQty` and no fee total (`RESULT` has
/// no fills); a `REJECTED` record is never presented as resting. The price
/// rule is MARKET's only: a stop's quote proves nothing here.
#[tokio::test]
async fn a_stop_is_its_record_whatever_its_status() {
    let cases: [(&str, &str, &str, OrderPhase); 6] = [
        ("NEW", "0", "0", OrderPhase::Live),
        ("PARTIALLY_FILLED", "0.2", "0", OrderPhase::Live),
        ("FILLED", "0.5", "-1", OrderPhase::Terminal),
        ("EXPIRED", "0", "0", OrderPhase::Terminal),
        ("CANCELED", "0", "0", OrderPhase::Terminal),
        ("REJECTED", "0", "0", OrderPhase::Terminal),
    ];
    for (status, executed, quote, phase) in cases {
        let (venue, seen, journal) = venue(move |request| {
            (
                200,
                result(request, |answer| {
                    answer["status"] = json!(status);
                    answer["executedQty"] = json!(executed);
                    answer["cummulativeQuoteQty"] = json!(quote);
                }),
            )
        })
        .await;
        let placed = stop(&venue, &journal, "ins-1").await.unwrap();
        assert_eq!(placed.stop.exchange_order_id, "28");
        let acceptance = &placed.acceptance;
        assert_eq!(acceptance.fill_price.as_decimal(), dec!(80000), "{status}");
        assert_eq!(acceptance.fee, Decimal::ZERO);
        let executed: Decimal = executed.parse().unwrap();
        assert_eq!(acceptance.filled_quantity.as_decimal(), executed, "{status}");
        if executed.is_zero() {
            assert_eq!(acceptance.fee_total(), Some((Decimal::ZERO, "USDT")), "{status}");
        } else {
            assert_eq!(acceptance.fee_evidence(), FeeEvidence::Unavailable(FeeGap::NoFills));
        }
        assert!(acceptance.fills().is_empty());
        let record = acceptance.record().unwrap();
        assert_eq!((record.status.as_str(), record.phase), (status, phase));
        assert_eq!(record.protection_kind, Some(ProtectionKind::MarketOnTrigger));
        assert_eq!(record.stop_price, Some(dec!(80000)));
        assert_eq!((record.orig_qty, record.side), (dec!(0.5), OrderSide::Sell));
        assert_eq!(posts(&seen), 1);
        assert!(unresolved(&venue, "ins-1"), "{status}");
    }
}

/// A stop's answer that contradicts it is uncertain; HTTP 400 with -2010
/// ("Order would trigger immediately.", errors.md L500) or -1013 is its
/// rejection.
#[tokio::test]
async fn a_stop_s_contradiction_is_uncertain_and_its_rejection_final() {
    let edits: [(&str, Value); 3] = [
        ("stopPrice", json!("80000.01")),
        ("type", json!("STOP_LOSS_LIMIT")),
        ("side", json!("BUY")),
    ];
    for (field, value) in edits {
        let (venue, _, journal) =
            venue(move |request| (200, result(request, |answer| answer[field] = value.clone())))
                .await;
        let outcome = stop(&venue, &journal, "ins-1").await;
        assert!(matches!(outcome, Err(ExecError::Exchange(_))), "{field}");
        assert!(unresolved(&venue, "ins-1"));
    }
    for (name, rejection) in [
        ("would_trigger", "OrderRejected"),
        ("filter_failure", "RulesRejected"),
    ] {
        let (venue, _, journal) = venue(move |_| (400, error_body(name))).await;
        let outcome = stop(&venue, &journal, "ins-1").await;
        let found = match outcome {
            Err(ExecError::OrderRejected(_)) => "OrderRejected",
            Err(ExecError::RulesRejected(_)) => "RulesRejected",
            other => panic!("{name}: {other:?}"),
        };
        assert_eq!(found, rejection);
        assert!(rejected(&venue, "ins-1"));
    }
}

// =============================================================================
// One request on the wire, one identity (8c-3 code review round 1)
// =============================================================================

/// A redirect is the answer, never followed: the order goes to one place
/// once, the outcome is uncertain and the attempt stays unresolved.
#[tokio::test]
async fn a_redirect_is_never_followed_and_proves_nothing() {
    for status in [301, 302, 303, 307, 308] {
        let (sink, sink_seen) = stand_in::listening(|request| (200, full(request, |_| {}))).await;
        let (client, seen) = stand_in::redirecting(status, format!("{sink}/api/v3/order")).await;
        let venue = BinanceSpotAdapter::new(client).unwrap();
        let journal = spot_journal();
        venue.attach_order_journal(Arc::clone(&journal));
        let outcome = reduction(&venue, &journal, "x-1", dec!(10)).await;
        assert!(matches!(outcome, Err(ExecError::Exchange(_))), "{status}: {outcome:?}");
        assert!(unresolved(&venue, "x-1"), "{status}");
        assert_eq!(posts(&seen), 1, "{status}");
        assert_eq!(requests(&sink_seen), Vec::<String>::new(), "{status}: nothing followed");
    }
}

/// A client id outside Binance's pattern, or a symbol that is not one
/// value, is refused before the ledger records anything: no request, no
/// attempt, so no order under another identity. `&`, `=`, `%`, `+`, space,
/// `#`, non-ASCII, `.`, `/`, empty and 37 characters alike.
#[tokio::test]
async fn a_malformed_id_or_symbol_is_refused_before_the_ledger() {
    let (venue, seen, journal) = answering_full(|_| {}).await;
    let long = "a".repeat(37);
    for id in [
        "x&recvWindow=5000",
        "a=b",
        "50%",
        "a+b",
        "a b",
        "a#b",
        "ação",
        "a.b",
        "a/b",
        "",
        long.as_str(),
    ] {
        let outcome = reduction(&venue, &journal, id, dec!(10)).await;
        assert!(matches!(outcome, Err(ExecError::Config(_))), "{id:?}: {outcome:?}");
        assert_eq!(venue.ledger().evidence(id), AttemptEvidence::NoRecord, "{id:?}");
    }
    let trigger = Price::new(dec!(80000)).unwrap();
    let bases = ["X&recvWindow=5000", "A B", "A=B", "A%", "A+B", "A#B", "A.B"];
    for (n, base) in bases.into_iter().enumerate() {
        let odd = Symbol::new(base.to_string(), "USDT".to_string()).unwrap();
        let id = format!("x-s{n}");
        let place = venue.place_market_order(
            &odd,
            OrderSide::Sell,
            qty(dec!(1)),
            &id,
            EntryOrReduce::Reduce,
        );
        let outcome = granted(&journal, &id, IntentKind::Exit, place).await;
        assert!(matches!(outcome, Err(ExecError::Config(_))), "{base:?}: {outcome:?}");
        assert_eq!(venue.ledger().evidence(&id), AttemptEvidence::NoRecord, "{base:?}");
        let place = venue.place_protection(&odd, OrderSide::Sell, qty(dec!(1)), trigger, "s-2");
        assert!(matches!(place.await, Err(ExecError::Config(_))), "{base:?}");
    }
    assert_eq!(requests(&seen), Vec::<String>::new());
    // The longest valid id goes out as it is.
    let valid = "ins-0123456789abcdef0123456789abcdef";
    assert_eq!(valid.len(), 36);
    reduction(&venue, &journal, valid, dec!(10)).await.unwrap();
    assert_eq!(posts(&seen), 1);
}

/// A non-ASCII symbol, which Binance documents, goes percent-encoded as one
/// value (rest-api.md L375-L417): the request carries exactly its own
/// parameters.
#[tokio::test]
async fn a_non_ascii_symbol_goes_encoded_as_one_value() {
    let (venue, seen, journal) = answering_full(|_| {}).await;
    let named = Symbol::new("１２３".to_string(), "USDT".to_string()).unwrap();
    let place = venue.place_market_order(
        &named,
        OrderSide::Sell,
        qty(dec!(1)),
        "x-1",
        EntryOrReduce::Reduce,
    );
    let _ = granted(&journal, "x-1", IntentKind::Exit, place).await;
    let request = seen.lock().unwrap()[0].clone();
    assert_eq!(request.param("symbol"), Some("%EF%BC%91%EF%BC%92%EF%BC%93USDT"));
    let mut names: Vec<&str> = request.params.iter().map(|(name, _)| name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(names, [
        "newClientOrderId",
        "newOrderRespType",
        "quantity",
        "selfTradePreventionMode",
        "side",
        "signature",
        "symbol",
        "timestamp",
        "type"
    ]);
}
