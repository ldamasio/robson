//! `BalanceStub` against the spot venue's port contracts (8c; 8d plan §2.2,
//! §7.1): capabilities, the one-attempt ledger and the gate, every answer's
//! type and evidence, and the composites over the book.

use std::{
    future::Future,
    sync::{Arc, OnceLock},
};

use robson_domain::{OrderSide, Price, Quantity, Symbol, SymbolTradingRules};
use robson_exec::{
    order_journal::{IntentKind, IntentSpec, OrderJournal, StubJournalBackend, WriteClass},
    ports::FeeEvidence,
    venue::{
        capability_mismatches, AccountKind, AccountPort, AttemptEvidence, CancelAnswer,
        EntryOrReduce, InventoryPort, MarketKind, OrderLookup, OrderObservation, OrderPhase,
        OrderRef, ProtectionKind, ReplaceOutcome, ReplacementProtocol, StopCancelOutcome,
        StopParams, StopRef, TradingPort, Venue, VenueCapabilities, VenueFinalityRule, VenueId,
    },
    ExecError,
};
use rust_decimal::Decimal;
use serde_json::json;

use crate::{
    balance_stub::{spot_capabilities, BalanceStub, BALANCE_STUB_VENUE},
    spot_book::{BookRules, Script, SpotBook},
};

fn d(s: &str) -> Decimal {
    s.parse().unwrap()
}

fn btc() -> &'static Symbol {
    static BTC: OnceLock<Symbol> = OnceLock::new();
    BTC.get_or_init(|| Symbol::from_pair("BTCUSDT").unwrap())
}

fn qty(s: &str) -> Quantity {
    Quantity::new(d(s)).unwrap()
}

fn price(s: &str) -> Price {
    Price::new(d(s)).unwrap()
}

/// BTCUSDT at 100 with BTC 1 and USDT 1000, fee 0.001, the spot rules, and a
/// one-attempt journal attached.
fn stub() -> (BalanceStub, Arc<OrderJournal>) {
    let book = SpotBook::new(vec![BookRules::btcusdt()], d("0.001"));
    book.set_price("BTCUSDT", d("100"));
    book.set_balance("BTC", d("1"));
    book.set_balance("USDT", d("1000"));
    let stub = BalanceStub::new(Arc::new(book));
    let rules = SymbolTradingRules::new(
        btc().clone(),
        d("0.01"),
        d("0.01"),
        d("0.00001"),
        d("0.00001"),
        d("9000"),
        d("5"),
        2,
        5,
    )
    .unwrap();
    stub.set_trading_rules(rules);
    let journal = OrderJournal::new(
        Arc::new(StubJournalBackend::default()),
        &VenueId::new(BALANCE_STUB_VENUE),
    );
    let journal = Arc::new(journal.with_one_attempt_per_client_id(true));
    stub.attach_order_journal(Arc::clone(&journal));
    (stub, journal)
}

/// Run `place` as the owner will: inside one operation, under a fresh
/// Strict grant for `id`, then settle the intent: a success when `accepted`
/// says so, otherwise failed (rejected when every attempt was, uncertain
/// when one may have reached the book, which fences the journal).
async fn granted_with<T>(
    journal: &Arc<OrderJournal>,
    id: &str,
    place: impl Future<Output = T>,
    accepted: impl FnOnce(&T) -> bool,
) -> T {
    let operation = journal.begin_operation();
    operation
        .scope(async {
            let intent = journal.open_intent(IntentSpec {
                position_id: None,
                kind: IntentKind::Entry,
                client_order_id: id.to_string(),
                cancels_order_id: None,
                params: json!({}),
            });
            let guard = journal.authorize_send(&intent, WriteClass::Strict).await.unwrap();
            let answer = place.await;
            drop(guard);
            if accepted(&answer) {
                journal.finish_success(&intent, None).await;
            } else {
                journal.finish_failed(&intent).await;
            }
            answer
        })
        .await
}

async fn granted<T>(
    journal: &Arc<OrderJournal>,
    id: &str,
    place: impl Future<Output = Result<T, ExecError>>,
) -> Result<T, ExecError> {
    granted_with(journal, id, place, Result::is_ok).await
}

async fn buy(
    stub: &BalanceStub,
    journal: &Arc<OrderJournal>,
    id: &str,
    q: &str,
) -> Result<robson_exec::OrderResult, ExecError> {
    granted(
        journal,
        id,
        stub.place_market_order(btc(), OrderSide::Buy, qty(q), id, EntryOrReduce::Entry),
    )
    .await
}

async fn protect(
    stub: &BalanceStub,
    journal: &Arc<OrderJournal>,
    id: &str,
    q: &str,
    trigger: &str,
) -> Result<robson_exec::venue::PlacedStop, ExecError> {
    granted(
        journal,
        id,
        stub.place_protection(btc(), OrderSide::Sell, qty(q), price(trigger), id),
    )
    .await
}

#[test]
fn its_capabilities_are_the_spot_adapters() {
    let (stub, _) = stub();
    let spot = VenueCapabilities {
        market_kind: MarketKind::Spot,
        account_kind: AccountKind::BalanceAccount,
        supports_short: false,
        reduce_only_wire_flag: false,
        protection: ProtectionKind::MarketOnTrigger,
        replacement: ReplacementProtocol::CancelThenPlace,
        has_funding: false,
        order_finality: VenueFinalityRule::EveryAttemptRejected,
        one_attempt_per_client_id: true,
    };
    assert_eq!(stub.capabilities(), spot);
    assert_eq!(spot_capabilities(), spot);
    assert!(capability_mismatches(&stub).is_empty());
    assert!(stub.inventory().is_some());
    assert!(stub.position_account().is_none() && stub.funding().is_none());
    assert_eq!(stub.id().as_str(), BALANCE_STUB_VENUE);
}

#[tokio::test]
async fn a_granted_market_buy_fills_with_its_record_and_a_base_fee() {
    let (stub, journal) = stub();
    let result = buy(&stub, &journal, "e1", "1").await.unwrap();
    assert_eq!(
        (result.fill_price.as_decimal(), result.filled_quantity.as_decimal()),
        (d("100"), d("1"))
    );
    assert_eq!((result.fee, result.fee_asset.as_str()), (d("0.001"), "BTC"));
    assert_eq!(result.fee_evidence(), FeeEvidence::Total);
    let record = result.record().unwrap();
    assert_eq!(
        (record.status.as_str(), record.phase, record.executed_qty),
        ("FILLED", OrderPhase::Terminal, d("1"))
    );
    assert_eq!(result.fills().len(), 1);
    assert_eq!(result.fills()[0].time, None, "a placement answer carries no trade time");
    assert_eq!(stub.sends(), 1);
    assert_eq!(stub.book().balance("BTC").free, d("1.999"));
}

/// B1: nothing reaches the book without a grant, and one grant sends once.
#[tokio::test]
async fn nothing_reaches_the_book_without_a_grant() {
    let unattached =
        BalanceStub::new(Arc::new(SpotBook::new(vec![BookRules::btcusdt()], d("0.001"))));
    let refused = unattached
        .place_market_order(btc(), OrderSide::Sell, qty("1"), "r1", EntryOrReduce::Reduce)
        .await;
    assert!(matches!(refused, Err(ExecError::SendRefused(_))), "{refused:?}");
    assert_eq!(unattached.sends(), 0);

    let (stub, journal) = stub();
    let ungranted = stub
        .place_market_order(btc(), OrderSide::Sell, qty("0.1"), "r2", EntryOrReduce::Reduce)
        .await;
    assert!(matches!(ungranted, Err(ExecError::SendRefused(_))), "{ungranted:?}");
    buy(&stub, &journal, "e1", "0.1").await.unwrap();
    let again = stub
        .place_market_order(btc(), OrderSide::Buy, qty("0.1"), "e1", EntryOrReduce::Entry)
        .await;
    assert!(matches!(again, Err(ExecError::SendRefused(_))), "{again:?}");
    assert_eq!(stub.sends(), 1, "one grant, one placement");
    assert_eq!(stub.book().all_orders().len(), 1);
}

#[tokio::test]
async fn spot_holds_no_short_and_a_reduction_goes_as_given() {
    let (stub, journal) = stub();
    for (id, side, intent) in [
        ("x1", OrderSide::Sell, EntryOrReduce::Entry),
        ("x2", OrderSide::Buy, EntryOrReduce::Reduce),
    ] {
        let refused =
            granted(&journal, id, stub.place_market_order(btc(), side, qty("0.1"), id, intent))
                .await;
        assert!(matches!(refused, Err(ExecError::Config(_))), "{refused:?}");
    }
    let reduced = granted(
        &journal,
        "r",
        stub.place_market_order(
            btc(),
            OrderSide::Sell,
            qty("0.123456789"),
            "r",
            EntryOrReduce::Reduce,
        ),
    )
    .await;
    assert_eq!(
        reduced.unwrap().filled_quantity.as_decimal(),
        d("0.123456789"),
        "no rules applied"
    );
    let entry = buy(&stub, &journal, "e", "0.123456789").await.unwrap();
    assert_eq!(entry.filled_quantity.as_decimal(), d("0.12345"), "an entry is quantized down");
    assert_eq!(stub.sends(), 2);
}

/// A refused -2010 or -1013 is the ledger's rejection, so the not-found is
/// `Absent`; any other answer is uncertain and leaves the id `Unknown`.
#[tokio::test]
async fn rejections_are_final_and_everything_else_uncertain() {
    let (stub, journal) = stub();
    let short = buy(&stub, &journal, "poor", "11").await;
    assert!(matches!(short, Err(ExecError::OrderRejected(_))), "{short:?}");
    stub.book().script_next(Script::Reject {
        code: -1013,
        msg: "Filter failure: LOT_SIZE".into(),
    });
    let rules = buy(&stub, &journal, "lot", "0.1").await;
    assert!(matches!(rules, Err(ExecError::RulesRejected(_))), "{rules:?}");
    stub.book().script_next(Script::Reject { code: -1007, msg: "Timeout".into() });
    let other = buy(&stub, &journal, "odd", "0.1").await;
    assert!(matches!(other, Err(ExecError::Exchange(_))), "{other:?}");
    for id in ["poor", "lot"] {
        assert_eq!(stub.ledger().evidence(id), AttemptEvidence::AllRejected { rejected: 1 });
        let lookup = stub.order_by_client_id(btc(), id).await.unwrap();
        assert!(
            matches!(lookup, OrderLookup::Absent {
                finality: VenueFinalityRule::EveryAttemptRejected
            }),
            "{id}: {lookup:?}"
        );
    }
    assert!(matches!(stub.ledger().evidence("odd"), AttemptEvidence::Unresolved { .. }));
    assert!(matches!(
        stub.order_by_client_id(btc(), "odd").await.unwrap(),
        OrderLookup::Unknown
    ));
    assert!(matches!(
        stub.order_by_client_id(btc(), "never").await.unwrap(),
        OrderLookup::Unknown
    ));
}

/// A lost answer is uncertain: the attempt stays unresolved, and the order
/// the book accepted is found once visible, `Unknown` while hidden.
#[tokio::test]
async fn a_lost_answer_leaves_the_attempt_unresolved() {
    let (stub, journal) = stub();
    stub.book().script_next(Script::Lost);
    let lost = buy(&stub, &journal, "lost", "0.1").await;
    assert!(matches!(lost, Err(ExecError::Exchange(_))), "{lost:?}");
    assert!(matches!(stub.ledger().evidence("lost"), AttemptEvidence::Unresolved { .. }));
    let id = stub.book().all_orders()[0].id;
    stub.book().hide_order(id);
    assert!(matches!(
        stub.order_by_client_id(btc(), "lost").await.unwrap(),
        OrderLookup::Unknown
    ));
    stub.book().show_order(id);
    let found = stub.order_by_client_id(btc(), "lost").await.unwrap();
    assert!(
        matches!(found, OrderLookup::Terminal { filled_qty, .. } if filled_qty == d("0.1")),
        "{found:?}"
    );
}

/// B3: an expiry with nothing executed is never a rejection.
#[tokio::test]
async fn an_unfilled_expiry_is_never_a_rejection() {
    let (stub, journal) = stub();
    stub.book().script_next(Script::ExpirePartial(d("0.04")));
    let partial = buy(&stub, &journal, "p", "0.1").await.unwrap();
    assert_eq!(partial.filled_quantity.as_decimal(), d("0.04"));
    assert_eq!(partial.record().unwrap().status, "EXPIRED");

    // Last: the journal takes an expiry it did not settle for uncertain.
    stub.book().script_next(Script::ExpireUnfilled);
    let expired = buy(&stub, &journal, "x", "0.1").await;
    let Err(ExecError::ExpiredUnfilled(record)) = expired else {
        panic!("{expired:?}");
    };
    assert_eq!((record.status.as_str(), record.executed_qty), ("EXPIRED", Decimal::ZERO));
    assert!(matches!(stub.ledger().evidence("x"), AttemptEvidence::Unresolved { .. }));
    let id = record.exchange_order_id.parse().unwrap();
    stub.book().hide_order(id);
    assert!(matches!(
        stub.order_by_client_id(btc(), "x").await.unwrap(),
        OrderLookup::Unknown
    ));
    stub.book().show_order(id);
    let visible = stub.order_by_client_id(btc(), "x").await.unwrap();
    assert!(
        matches!(visible, OrderLookup::Terminal { filled_qty, .. } if filled_qty.is_zero()),
        "{visible:?}"
    );
}

#[tokio::test]
async fn a_stop_rests_with_its_record_and_a_buy_stop_is_refused() {
    let (stub, journal) = stub();
    let placed = protect(&stub, &journal, "s", "0.5", "90").await.unwrap();
    let record = placed.acceptance.record().unwrap();
    assert_eq!(
        (record.status.as_str(), record.phase, record.order_type.as_str()),
        ("NEW", OrderPhase::Live, "STOP_LOSS")
    );
    assert_eq!(record.stop_price, Some(d("90")));
    assert_eq!(
        (placed.acceptance.fee_asset.as_str(), placed.acceptance.fee_evidence()),
        ("USDT", FeeEvidence::Total)
    );
    assert_eq!(stub.book().balance("BTC").locked, d("0.5"));
    let buy_stop = granted(
        &journal,
        "b",
        stub.place_protection(btc(), OrderSide::Buy, qty("0.1"), price("90"), "b"),
    )
    .await;
    assert!(matches!(buy_stop, Err(ExecError::Config(_))), "{buy_stop:?}");
    let above = protect(&stub, &journal, "a", "0.1", "100").await;
    assert!(matches!(above, Err(ExecError::OrderRejected(_))), "{above:?}");
    let open = stub.open_protective_orders(btc()).await.unwrap();
    assert_eq!(open.len(), 1);
    assert_eq!(
        (open[0].exchange_order_id.as_str(), open[0].stop_price),
        (placed.stop.exchange_order_id.as_str(), Some(price("90")))
    );
    assert_eq!(stub.sends(), 2, "the BUY stop never reached the book");
}

/// A cancel takes no grant; `-2011` is `NotFound`, a delayed cancel
/// `Unknown`, a malformed id refused with nothing sent.
#[tokio::test]
async fn a_cancel_is_one_typed_answer_without_a_grant() {
    let (stub, journal) = stub();
    let placed = protect(&stub, &journal, "s", "0.5", "90").await.unwrap();
    let id = placed.stop.exchange_order_id;
    let cancelled = stub.cancel_order_once(btc(), &id).await.unwrap();
    let CancelAnswer::Cancelled(record) = cancelled else {
        panic!("{cancelled:?}");
    };
    assert_eq!(
        (record.status.as_str(), record.orig_client_order_id.as_deref()),
        ("CANCELED", Some("s"))
    );
    assert_eq!(stub.cancel_order_once(btc(), &id).await.unwrap(), CancelAnswer::NotFound {
        code: -2011
    });
    assert_eq!(stub.cancel_order_once(btc(), "999").await.unwrap(), CancelAnswer::NotFound {
        code: -2011
    });
    assert!(matches!(stub.cancel_order_once(btc(), "x1").await, Err(ExecError::Config(_))));
    let delayed = protect(&stub, &journal, "t", "0.5", "90").await.unwrap();
    stub.book().delay_cancels(true);
    let unknown = stub.cancel_order_once(btc(), &delayed.stop.exchange_order_id).await.unwrap();
    assert!(matches!(unknown, CancelAnswer::Unknown { .. }), "{unknown:?}");
    assert!(matches!(stub.cancel_order(btc(), &id).await, Err(ExecError::OrderRejected(_))));
    assert_eq!(stub.sends(), 2, "cancels are not placements");
}

/// The gate refuses every mutation while the daemon stops: nothing reaches
/// the book.
#[tokio::test]
async fn the_gate_refuses_when_stopping() {
    let (stub, journal) = stub();
    let placed = protect(&stub, &journal, "s", "0.5", "90").await.unwrap();
    let stopping = OrderJournal::new(
        Arc::new(StubJournalBackend::default()),
        &VenueId::new(BALANCE_STUB_VENUE),
    );
    stub.attach_gate(Arc::clone(stopping.gate()));
    stopping.gate().begin_draining();
    let before = stub.book().all_orders();
    let order = buy(&stub, &journal, "e", "0.1").await;
    assert!(matches!(order, Err(ExecError::SendRefused(_))), "{order:?}");
    let cancel = stub.cancel_order_once(btc(), &placed.stop.exchange_order_id).await;
    assert!(matches!(cancel, Err(ExecError::SendRefused(_))), "{cancel:?}");
    assert_eq!(stub.book().all_orders(), before);
    assert_eq!(stub.sends(), 1);
}

/// The cancel composite: a definite cancel reports the execution before it
/// (with its fills); after a fill, `AlreadyGone` with the lookup's terminal
/// state; withheld trades give no fills; a delayed cancel is `Unknown`.
#[tokio::test]
async fn cancel_protection_reports_what_executed() {
    let (stub, journal) = stub();
    let partial = protect(&stub, &journal, "p", "0.5", "90").await.unwrap().stop;
    stub.book().set_price("BTCUSDT", d("85"));
    stub.book().cross(partial.exchange_order_id.parse().unwrap(), d("0.2")).unwrap();
    let outcome = stub.cancel_protection(btc(), &partial).await.unwrap();
    let StopCancelOutcome::Cancelled { filled_before_cancel, fills: Some(fills) } = outcome else {
        panic!("{outcome:?}");
    };
    assert_eq!((filled_before_cancel, fills.filled_quantity.as_decimal()), (d("0.2"), d("0.2")));
    assert_eq!((fills.fee_asset.as_str(), fills.fee_evidence()), ("USDT", FeeEvidence::Total));
    assert!(fills.fills().iter().all(|fill| fill.time.is_some()));

    stub.book().set_price("BTCUSDT", d("100"));
    let filled = protect(&stub, &journal, "f", "0.3", "90").await.unwrap().stop;
    let filled_id: u64 = filled.exchange_order_id.parse().unwrap();
    stub.book().cross(filled_id, d("0.3")).unwrap();
    let gone = stub.cancel_protection(btc(), &filled).await.unwrap();
    let StopCancelOutcome::AlreadyGone {
        final_state: OrderLookup::Terminal { filled_qty, fills, .. },
    } = gone
    else {
        panic!("{gone:?}");
    };
    assert_eq!(filled_qty, d("0.3"));
    assert_eq!(fills.unwrap().filled_quantity.as_decimal(), d("0.3"));
    stub.book().withhold_trades(filled_id);
    let withheld = stub.cancel_protection(btc(), &filled).await.unwrap();
    assert!(
        matches!(withheld, StopCancelOutcome::AlreadyGone {
            final_state: OrderLookup::Terminal { fills: None, .. }
        }),
        "{withheld:?}"
    );

    let delayed = protect(&stub, &journal, "d", "0.1", "90").await.unwrap().stop;
    stub.book().delay_cancels(true);
    assert!(matches!(
        stub.cancel_protection(btc(), &delayed).await.unwrap(),
        StopCancelOutcome::Unknown
    ));
}

/// Cancel-then-place: a definite cancel of an unexecuted stop, then the
/// successor; an uncertain cancel or an executed old stop places nothing.
#[tokio::test]
async fn replace_protection_cancels_then_places() {
    let (stub, journal) = stub();
    let placed = |outcome: &Result<ReplaceOutcome, ExecError>| {
        matches!(outcome, Ok(ReplaceOutcome::Replaced { .. }))
    };
    let params = |id: &str, q: &str| StopParams {
        close_side: OrderSide::Sell,
        quantity: qty(q),
        trigger: price("92"),
        client_order_id: id.to_string(),
    };
    let old = protect(&stub, &journal, "o1", "0.5", "90").await.unwrap().stop;
    let replaced = granted_with(
        &journal,
        "n1",
        stub.replace_protection(btc(), &old, params("n1", "0.5")),
        placed,
    )
    .await
    .unwrap();
    let ReplaceOutcome::Replaced { new, old_filled } = replaced else {
        panic!("{replaced:?}");
    };
    assert_eq!(old_filled, Decimal::ZERO);
    assert_eq!(stub.book().balance("BTC").locked, d("0.5"), "only the successor locks");

    let sends = stub.sends();
    stub.book().delay_cancels(true);
    let uncertain = granted_with(
        &journal,
        "n2",
        stub.replace_protection(btc(), &new.stop, params("n2", "0.5")),
        placed,
    )
    .await
    .unwrap();
    assert!(matches!(uncertain, ReplaceOutcome::Unknown { new: None }), "{uncertain:?}");
    stub.book().delay_cancels(false);
    stub.book().apply_delayed_cancels();

    let executed = protect(&stub, &journal, "o3", "0.5", "90").await.unwrap().stop;
    stub.book()
        .cross(executed.exchange_order_id.parse().unwrap(), d("0.1"))
        .unwrap();
    let outcome = granted_with(
        &journal,
        "n3",
        stub.replace_protection(btc(), &executed, params("n3", "0.4")),
        placed,
    )
    .await
    .unwrap();
    assert!(
        matches!(outcome, ReplaceOutcome::OldExecuted { old_filled } if old_filled == d("0.1")),
        "{outcome:?}"
    );
    assert_eq!(stub.sends(), sends + 1, "only o3 was placed after n1");
}

#[tokio::test]
async fn lookups_and_trade_pages_follow_the_book() {
    let (stub, journal) = stub();
    let stop = protect(&stub, &journal, "s", "0.9", "90").await.unwrap().stop;
    let id: u64 = stop.exchange_order_id.parse().unwrap();
    for _ in 0..3 {
        stub.book().cross(id, d("0.1")).unwrap();
    }
    let by_id = OrderRef::ExchangeId(stop.exchange_order_id.clone());
    let OrderObservation::Found(record) = stub.observe_order(btc(), &by_id).await.unwrap() else {
        panic!("not found");
    };
    assert_eq!((record.status.as_str(), record.executed_qty), ("PARTIALLY_FILLED", d("0.3")));
    assert!(record.time_ms.is_some() && record.transact_time_ms.is_none());
    let by_client = OrderRef::ClientId("s".into());
    assert!(matches!(
        stub.observe_order(btc(), &by_client).await.unwrap(),
        OrderObservation::Found(_)
    ));
    let unknown = OrderRef::ExchangeId("999".into());
    assert_eq!(stub.observe_order(btc(), &unknown).await.unwrap(), OrderObservation::NotFound);
    assert!(matches!(
        stub.observe_order(btc(), &OrderRef::ExchangeId("x".into())).await,
        Err(ExecError::Config(_))
    ));

    stub.set_trades_page_limit(2);
    let first = stub.order_trades_page(btc(), &stop.exchange_order_id, 0).await.unwrap();
    assert_eq!(first.trades.len(), 2);
    let next = first.next_from_id.unwrap();
    assert_eq!(
        next,
        first.trades[1].trade_id.parse::<u64>().unwrap() + 1,
        "past the highest id"
    );
    let second = stub.order_trades_page(btc(), &stop.exchange_order_id, next).await.unwrap();
    assert_eq!((second.trades.len(), second.next_from_id), (1, None));

    stub.book().cancel("BTCUSDT", id).unwrap();
    let fill = stub.protection_fill(btc(), &stop).await.unwrap().unwrap();
    assert_eq!(fill.filled_quantity.as_decimal(), d("0.3"), "across two pages");
    let market = buy(&stub, &journal, "m", "0.1").await.unwrap();
    let not_a_stop = StopRef::new(market.exchange_order_id.clone());
    assert!(stub.protection_fill(btc(), &not_a_stop).await.is_err());
    let filled = stub.order_by_exchange_id(btc(), &market.exchange_order_id).await.unwrap();
    assert_eq!(filled.unwrap().filled_quantity.as_decimal(), d("0.1"));

    let archived = protect(&stub, &journal, "z", "0.01", "90").await.unwrap().stop;
    let archived_id: u64 = archived.exchange_order_id.parse().unwrap();
    stub.book().cancel("BTCUSDT", archived_id).unwrap();
    stub.book().archive(archived_id);
    let by = OrderRef::ExchangeId(archived.exchange_order_id.clone());
    assert_eq!(stub.observe_order(btc(), &by).await.unwrap(), OrderObservation::Archived);
    assert!(matches!(
        stub.order_by_client_id(btc(), "z").await.unwrap(),
        OrderLookup::Unknown
    ));
}

#[tokio::test]
async fn account_reads_and_what_spot_refuses() {
    let (stub, journal) = stub();
    protect(&stub, &journal, "s", "0.4", "90").await.unwrap();
    let observation = stub.account_observation().await.unwrap();
    let holdings: Vec<(&str, Decimal, Decimal)> = observation
        .holdings
        .iter()
        .map(|(a, h)| (a.as_str(), h.free, h.locked))
        .collect();
    assert_eq!(holdings, [
        ("BTC", d("0.6"), d("0.4")),
        ("USDT", d("1000"), Decimal::ZERO)
    ]);
    assert_eq!(observation.open_orders.len(), 1);
    assert_eq!(observation.open_orders[0].record.as_ref().unwrap().client_order_id, "s");
    let cash = stub.cash("BTC").await.unwrap();
    assert_eq!((cash.free, cash.locked), (d("0.6"), d("0.4")));
    assert_eq!(stub.valuation(btc()).await.unwrap().price, price("100"));
    let since = chrono::Utc::now();
    assert!(matches!(stub.execution_costs(btc()).await, Err(ExecError::Config(_))));
    assert!(matches!(stub.governed_inventory(btc()).await, Err(ExecError::Config(_))));
    assert!(matches!(
        stub.settlement().settlement_since(since, 10).await,
        Err(ExecError::Config(_))
    ));
    assert!(matches!(
        stub.user_trades_since(btc(), since, 10).await,
        Err(ExecError::Config(_))
    ));
    assert!(stub.health().await.is_ok());
}
