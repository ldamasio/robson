//! Conformance run U (ADR-0056 §10, step 8, 8d plan §4): every clause of
//! the inventory on the USD-M adapter, with the real REST client, against
//! `FakeVenue` over loopback, accepted only by `check` against its `const`
//! table. It pins USD-M's existing outcomes and changes no USD-M code; the
//! clauses it cannot execute are pending, copied into the design directory's
//! `usdm-deferred-notes.md`.

use std::{sync::Arc, time::Duration};

use robson_domain::{OrderSide, Quantity, Symbol};
use robson_exec::{
    venue::{AccountPort, EntryOrReduce, StopRef, TradingPort},
    ExecError,
};
use robson_testkit::{
    conformance::{
        check, pending_list, run_table,
        scenarios::{self, PositionHarness, TriggersStop},
        Boxed, Coverage,
        Coverage::{Characterized, NotApplicable, Pending, Verified},
        Event, Expected, Harness, Outcome, Property,
        Property::*,
        Verdict,
    },
    fake_venue::{FakeVenue, FAKE_SYMBOL},
};
use rust_decimal::Decimal;

use super::BinanceExchangeAdapter;
use crate::binance_rest::BinanceRestClient;

const fn e(property: Property, coverage: Coverage) -> Expected {
    Expected { property, coverage }
}

/// Run U: the USD-M adapter over `FakeVenue` (8d plan §4, notes ¹, ³, ⁴, ⁵,
/// ⁷).
const U_TABLE: [Expected; 22] = [
    e(F1, Verified),
    e(F2, Verified),
    e(F3, Verified),
    e(N1, Verified),
    e(N2, Verified),
    e(N3a, Verified),
    e(
        N3b,
        Pending("USD-M fake: no accepted-then-visible control that survives a cancelled request"),
    ),
    e(R1, Verified),
    e(R2, NotApplicable("PlaceThenCancel venue")),
    e(R3, NotApplicable("PlaceThenCancel venue")),
    e(A1a, Characterized),
    e(A1b, Pending("trigger validation not modeled; the real venue answers -2021")),
    e(A2, Characterized),
    e(A3, Characterized),
    e(A4, Characterized),
    e(A5b, NotApplicable("position account")),
    e(A5p, Pending("reduce-only execution bound: the real venue's, not modeled")),
    e(B1, NotApplicable("position account: no one-attempt ledger")),
    e(B2, NotApplicable("position account: no InventoryPort")),
    e(B3, NotApplicable("position account: no InventoryPort")),
    e(B4, NotApplicable("position account: no InventoryPort")),
    e(B5, NotApplicable("position account: no InventoryPort")),
];

/// The client's request bound: a held order's send ends in it (note ³).
const REQUEST_BOUND: Duration = Duration::from_millis(300);

fn d(s: &str) -> Decimal {
    s.parse().unwrap()
}

fn btc() -> Symbol {
    Symbol::from_pair(FAKE_SYMBOL).unwrap()
}

/// U: the adapter against its own `FakeVenue`, at the fake's mark of 100,
/// with a long of 1 bought through the adapter.
struct U {
    venue: FakeVenue,
    adapter: BinanceExchangeAdapter,
}

async fn make_u() -> U {
    let venue = FakeVenue::start().await.expect("a loopback port");
    let client = BinanceRestClient::with_local_base_url(venue.url());
    let adapter = BinanceExchangeAdapter::new(Arc::new(client.with_request_timeout(REQUEST_BOUND)));
    let one = Quantity::new(d("1")).unwrap();
    let seeded = adapter
        .place_market_order(&btc(), OrderSide::Buy, one, "u-seed", EntryOrReduce::Entry)
        .await;
    seeded.expect("a long of 1");
    U { venue, adapter }
}

impl Harness for U {
    type V = BinanceExchangeAdapter;

    fn venue(&self) -> &BinanceExchangeAdapter {
        &self.adapter
    }

    fn symbol(&self) -> Symbol {
        btc()
    }

    /// USD-M sends without grants: its ledger allows retries
    /// (`one_attempt_per_client_id == false`).
    fn sending<'a, T: Send + 'a>(
        &'a self,
        _client_id: &'a str,
        send: Boxed<'a, Result<T, ExecError>>,
        _accepted: fn(&Result<T, ExecError>) -> bool,
    ) -> Boxed<'a, Result<T, ExecError>> {
        send
    }

    /// HTTP 400 `-2010` on the next conditional order (note ¹).
    fn reject_next_placement(&self) {
        self.venue.reject_next_algo();
    }

    /// The next market order is read and held unanswered; the client's own
    /// bound ends the send (note ³). Nothing releases it.
    fn lose_next_placement(&self) {
        self.venue.hold_orders();
    }

    /// The held order is never applied: nothing to hide.
    fn conceal(&self, _client_id: &str) {}

    fn reveal(&self, _client_id: &str) {}

    fn placements(&self) -> u64 {
        self.venue.placement_requests()
    }

    fn open_stops(&self) -> Vec<String> {
        self.venue.open_algo_ids()
    }
}

impl PositionHarness for U {
    fn position(&self) -> Decimal {
        self.venue.position(FAKE_SYMBOL)
    }

    fn on_next_stop_accepted(&self, hook: Box<dyn FnOnce(Vec<String>) + Send>) {
        self.venue.on_next_algo_accepted(hook);
    }
}

impl TriggersStop for U {
    fn trigger(&self, stop: &StopRef, executed: Decimal) -> bool {
        self.venue.trigger_algo(&stop.exchange_order_id, executed)
    }
}

/// N3a's own evidence on U (note ³): the order was held, never applied.
fn held_evidence(u: &U, outcome: &mut Outcome) {
    let held = u.venue.held_orders();
    let applied = u.venue.orders().iter().any(|order| order.client_id == "n3");
    let what = format!("held orders {held}, n3 applied {applied}");
    outcome.trace.0.push(Event::State { what });
    let mut problems = Vec::new();
    if held != 1 {
        problems.push(format!("one held order, not {held}"));
    }
    if applied {
        problems.push("the held order was applied".to_string());
    }
    if !u.venue.unserved_requests().is_empty() {
        problems.push(format!("unserved requests {:?}", u.venue.unserved_requests()));
    }
    fail_with(outcome, problems);
}

fn fail_with(outcome: &mut Outcome, problems: Vec<String>) {
    if problems.is_empty() {
        return;
    }
    outcome.verdict = match std::mem::replace(&mut outcome.verdict, Verdict::NotRun) {
        Verdict::Fail(mut why) => {
            why.extend(problems);
            Verdict::Fail(why)
        },
        _ => Verdict::Fail(problems),
    };
}

/// One clause on a fresh venue; a request the fake does not serve fails it.
fn u_scenario(property: Property) -> Boxed<'static, Outcome> {
    Box::pin(async move {
        let u = make_u().await;
        let mut outcome = match property {
            F1 => scenarios::f1(&u).await,
            F2 => scenarios::f2(&u).await,
            F3 => scenarios::f3(&u).await,
            N1 => scenarios::n1(&u).await,
            N2 => scenarios::n2(&u).await,
            N3a => scenarios::n3a(&u).await,
            R1 => scenarios::r1(&u).await,
            A1a => scenarios::a1a_triggered(&u).await,
            A2 => scenarios::a2_triggered(&u).await,
            A3 => scenarios::a3_triggered(&u).await,
            A4 => scenarios::a4_characterized(&u).await,
            other => Outcome::no_scenario(other),
        };
        if property == N3a {
            held_evidence(&u, &mut outcome);
        }
        let unserved = u.venue.unserved_requests();
        if !unserved.is_empty() {
            fail_with(&mut outcome, vec![format!(
                "requests the fake does not serve: {unserved:?}"
            )]);
        }
        outcome
    })
}

#[tokio::test]
async fn run_u_usdm_adapter_over_the_wire() {
    let outcomes = run_table(&U_TABLE, u_scenario).await;
    println!("run U, not run: {:#?}", pending_list(&U_TABLE));
    for failed in outcomes.iter().filter(|o| matches!(o.verdict, Verdict::Fail(_))) {
        println!(
            "{:?} failed: {:?}; its trace: {:#?}",
            failed.property, failed.verdict, failed.trace
        );
    }
    let capabilities = make_u().await.adapter.capabilities();
    if let Err(problems) = check(&capabilities, &U_TABLE, &outcomes) {
        panic!("run U is not accepted:\n{}", problems.join("\n"));
    }
}

fn pending(table: &[Expected]) -> Vec<Property> {
    table
        .iter()
        .filter(|e| matches!(e.coverage, Pending(_)))
        .map(|e| e.property)
        .collect()
}

/// U's pending list is the plan's (8d plan §4): N3b, A1b, A5p.
#[test]
fn run_u_pending_list_is_the_plans() {
    assert_eq!(pending(&U_TABLE), [N3b, A1b, A5p]);
}

/// The characterized outcome at an old stop's partial execution, pinned
/// from the trace: the helper places the successor first, the old stop's
/// cancel is refused `-2011`, and the outcome does not report the
/// execution (`usdm-deferred-notes.md`).
#[tokio::test]
async fn a_triggered_stops_execution_is_unreported() {
    let u = make_u().await;
    let outcome = scenarios::a2_triggered(&u).await;
    assert_eq!(outcome.verdict, Verdict::Pass, "{:#?}", outcome.trace);
    let replaced = outcome.trace.0.iter().find_map(|event| match event {
        Event::Answered { call, answer } if call == "replace_protection" => Some(answer.clone()),
        _ => None,
    });
    let replaced = replaced.expect("the replacement's answer");
    assert!(replaced.starts_with("Ok(Unknown { new: Some("), "{replaced}");
    assert!(!replaced.contains("old_filled"), "{replaced}");
    assert_eq!(u.venue.counts().cancels, 0, "the old stop's cancel was refused");
}
