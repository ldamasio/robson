//! Conformance runs P and B (ADR-0056 §10, step 8, 8d plan §4 and §7.3):
//! every clause of the inventory on the position stub (`StubExchange`) and
//! on the balance stub (`BalanceStub`), each against its `const` table,
//! accepted only by `check`. The tables are the source of the matrix's P and
//! B columns; review them with the scenarios they name.

use std::sync::{Arc, Weak};

use robson_domain::{Price, Quantity, Side, Symbol, SymbolTradingRules};
use robson_exec::{
    order_journal::{IntentKind, IntentSpec, OrderJournal, StubJournalBackend, WriteClass},
    stub::StubExchange,
    venue::{StopCancelOutcome, Venue, VenueId},
    ExecError,
};
use robson_testkit::{
    balance_stub::{BalanceStub, PlacementHook, BALANCE_STUB_VENUE},
    conformance::{
        check, pending_list, run_table,
        scenarios::{self, BalanceHarness, PositionHarness, ScriptsStopCancel},
        Boxed, Coverage,
        Coverage::{Characterized, NotApplicable, Pending, Scripted, Verified},
        Expected, Harness, Outcome, Property,
        Property::*,
        Verdict,
    },
    spot_book::{BookRules, BookStatus, OrderKind, Script, SpotBook},
};
use rust_decimal::Decimal;
use serde_json::json;

const fn e(property: Property, coverage: Coverage) -> Expected {
    Expected { property, coverage }
}

/// Run P: the position stub (8d plan §4, notes ², ⁵, ⁶, ⁷).
const P_TABLE: [Expected; 22] = [
    e(F1, Verified),
    e(F2, Verified),
    e(F3, Verified),
    e(N1, Verified),
    e(N2, Verified),
    e(N3a, Verified),
    e(N3b, Pending("position stub: no accepted-then-visible control")),
    e(R1, Verified),
    e(R2, NotApplicable("PlaceThenCancel venue")),
    e(R3, NotApplicable("PlaceThenCancel venue")),
    e(A1a, Verified),
    e(A1b, Pending("trigger validation not modeled; the real venue answers -2021")),
    e(A2, Scripted),
    e(A3, Scripted),
    e(A4, Characterized),
    e(A5b, NotApplicable("position account")),
    e(A5p, Pending("reduce-only execution bound: the real venue's, not modeled")),
    e(B1, NotApplicable("position account: no one-attempt ledger")),
    e(B2, NotApplicable("position account: no InventoryPort")),
    e(B3, NotApplicable("position account: no InventoryPort")),
    e(B4, NotApplicable("position account: no InventoryPort")),
    e(B5, NotApplicable("position account: no InventoryPort")),
];

/// Run B: the balance stub over a `SpotBook` (8d plan §4).
const B_TABLE: [Expected; 22] = [
    e(F1, Verified),
    e(F2, Verified),
    e(F3, Verified),
    e(N1, Verified),
    e(N2, Verified),
    e(N3a, Verified),
    e(N3b, Verified),
    e(R1, NotApplicable("CancelThenPlace venue")),
    e(R2, Verified),
    e(R3, Verified),
    e(A1a, Verified),
    e(A1b, Verified),
    e(A2, Verified),
    e(A3, Verified),
    e(A4, Verified),
    e(A5b, Verified),
    e(A5p, NotApplicable("balance account")),
    e(B1, Verified),
    e(B2, Verified),
    e(B3, Verified),
    e(B4, Verified),
    e(B5, Verified),
];

fn d(s: &str) -> Decimal {
    s.parse().unwrap()
}

fn btc() -> Symbol {
    Symbol::from_pair("BTCUSDT").unwrap()
}

// The position stub.

/// P: the stub at 100 with a long of 1 on BTCUSDT, tracking its positions.
struct P {
    stub: Arc<StubExchange>,
}

fn make_p() -> P {
    let stub = StubExchange::new(d("100"));
    stub.set_price("BTCUSDT", d("100"));
    stub.set_tracks_inventory(true);
    let one = Quantity::new(d("1")).unwrap();
    stub.set_open_position(btc(), Side::Long, one, Price::new(d("100")).unwrap());
    P { stub: Arc::new(stub) }
}

fn live_stops(stub: &StubExchange) -> Vec<String> {
    let mut ids: Vec<String> =
        stub.stop_orders_snapshot().into_iter().map(|o| o.exchange_order_id).collect();
    ids.sort();
    ids
}

impl Harness for P {
    type V = StubExchange;

    fn venue(&self) -> &StubExchange {
        &self.stub
    }

    fn symbol(&self) -> Symbol {
        btc()
    }

    /// The position stub sends without grants.
    fn sending<'a, T: Send + 'a>(
        &'a self,
        _client_id: &'a str,
        send: Boxed<'a, Result<T, ExecError>>,
        _accepted: fn(&Result<T, ExecError>) -> bool,
    ) -> Boxed<'a, Result<T, ExecError>> {
        send
    }

    fn reject_next_placement(&self) {
        self.stub.set_reject_order_next(true);
    }

    /// The stub registers the attempt, then fails before recording the
    /// order (note ²).
    fn lose_next_placement(&self) {
        self.stub.set_order_fail_next(true);
    }

    /// The failed order was never recorded: nothing to hide.
    fn conceal(&self, _client_id: &str) {}

    fn reveal(&self, _client_id: &str) {}

    fn placements(&self) -> u64 {
        self.stub.market_order_call_count() + self.stub.protection_call_count()
    }

    fn open_stops(&self) -> Vec<String> {
        live_stops(&self.stub)
    }
}

impl PositionHarness for P {
    fn position(&self) -> Decimal {
        self.stub.held_quantity(&btc(), Side::Long)
    }

    fn on_next_stop_accepted(&self, hook: Box<dyn FnOnce(Vec<String>) + Send>) {
        let stub: Weak<StubExchange> = Arc::downgrade(&self.stub);
        self.stub.set_after_next_stop_accepted(move || {
            if let Some(stub) = stub.upgrade() {
                hook(live_stops(&stub));
            }
        });
    }
}

impl ScriptsStopCancel for P {
    fn script_next_stop_cancel(&self, outcome: StopCancelOutcome) {
        self.stub.set_stop_cancel_outcome_next(Ok(outcome));
    }
}

async fn p_scenario(property: Property) -> Outcome {
    let p = make_p();
    match property {
        F1 => scenarios::f1(&p).await,
        F2 => scenarios::f2(&p).await,
        F3 => scenarios::f3(&p).await,
        N1 => scenarios::n1(&p).await,
        N2 => scenarios::n2(&p).await,
        N3a => scenarios::n3a(&p).await,
        R1 => scenarios::r1(&p).await,
        A1a => scenarios::a1a_position(&p).await,
        A2 => scenarios::a2_scripted(&p).await,
        A3 => scenarios::a3_scripted(&p).await,
        A4 => scenarios::a4_characterized(&p).await,
        other => Outcome::no_scenario(other),
    }
}

// The balance stub.

/// B: the stub over BTCUSDT at 100, with BTC 1 and USDT 1000 (fee 0.001),
/// BTCUSDT's rules, and a one-attempt journal attached.
struct B {
    stub: BalanceStub,
    journal: Arc<OrderJournal>,
}

fn make_b() -> B {
    let book = SpotBook::new(vec![BookRules::btcusdt()], d("0.001"));
    book.set_price("BTCUSDT", d("100"));
    book.set_balance("BTC", d("1"));
    book.set_balance("USDT", d("1000"));
    let stub = BalanceStub::new(Arc::new(book));
    let rules = SymbolTradingRules::new(
        btc(),
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
    B { stub, journal }
}

impl B {
    fn order_with(&self, client_id: &str) -> Option<u64> {
        let orders = self.stub.book().all_orders();
        orders.into_iter().rev().find(|o| o.client_id == client_id).map(|o| o.id)
    }
}

impl Harness for B {
    type V = BalanceStub;

    fn venue(&self) -> &BalanceStub {
        &self.stub
    }

    fn symbol(&self) -> Symbol {
        btc()
    }

    /// As the owner sends: one operation, a fresh Strict grant, the send,
    /// then the intent settled.
    fn sending<'a, T: Send + 'a>(
        &'a self,
        client_id: &'a str,
        send: Boxed<'a, Result<T, ExecError>>,
        accepted: fn(&Result<T, ExecError>) -> bool,
    ) -> Boxed<'a, Result<T, ExecError>> {
        Box::pin(async move {
            let journal = &self.journal;
            let operation = journal.begin_operation();
            operation
                .scope(async {
                    let intent = journal.open_intent(IntentSpec {
                        position_id: None,
                        kind: IntentKind::Entry,
                        client_order_id: client_id.to_string(),
                        cancels_order_id: None,
                        params: json!({}),
                    });
                    let guard = journal.authorize_send(&intent, WriteClass::Strict).await;
                    let Ok(guard) = guard else {
                        return Err(ExecError::SendRefused(format!("no grant for {client_id}")));
                    };
                    let answer = send.await;
                    drop(guard);
                    if accepted(&answer) {
                        journal.finish_success(&intent, None).await;
                    } else {
                        journal.finish_failed(&intent).await;
                    }
                    answer
                })
                .await
        })
    }

    fn reject_next_placement(&self) {
        let msg = "Account has insufficient balance for requested action.".to_string();
        self.stub.book().script_next(Script::Reject { code: -2010, msg });
    }

    /// The book acts, and the answer is lost.
    fn lose_next_placement(&self) {
        self.stub.book().script_next(Script::Lost);
    }

    fn conceal(&self, client_id: &str) {
        if let Some(id) = self.order_with(client_id) {
            self.stub.book().hide_order(id);
        }
    }

    fn reveal(&self, client_id: &str) {
        if let Some(id) = self.order_with(client_id) {
            self.stub.book().show_order(id);
        }
    }

    fn placements(&self) -> u64 {
        self.stub.sends()
    }

    fn open_stops(&self) -> Vec<String> {
        let orders = self.stub.book().all_orders().into_iter();
        let live = orders.filter(|o| {
            matches!(o.kind, OrderKind::StopLoss { .. })
                && matches!(o.status, BookStatus::New | BookStatus::PartiallyFilled)
        });
        live.map(|o| o.id.to_string()).collect()
    }
}

impl BalanceHarness for B {
    fn book(&self) -> &SpotBook {
        self.stub.book()
    }

    fn on_next_placement(&self, hook: PlacementHook) {
        self.stub.on_next_placement(hook);
    }
}

async fn b_scenario(property: Property) -> Outcome {
    let b = make_b();
    match property {
        F1 => scenarios::f1(&b).await,
        F2 => scenarios::f2(&b).await,
        F3 => scenarios::f3(&b).await,
        N1 => scenarios::n1(&b).await,
        N2 => scenarios::n2(&b).await,
        N3a => scenarios::n3a(&b).await,
        N3b => scenarios::n3b(&b).await,
        R2 => scenarios::r2(&b).await,
        R3 => scenarios::r3(&b).await,
        A1a => scenarios::a1a_balance(&b).await,
        A1b => scenarios::a1b(&b).await,
        A2 => scenarios::a2_balance(&b).await,
        A3 => scenarios::a3_balance(&b).await,
        A4 => scenarios::a4_balance(&b).await,
        A5b => scenarios::a5b(make_b).await,
        B1 => scenarios::b1(&b).await,
        B2 => scenarios::b2(&b).await,
        B3 => scenarios::b3(&b).await,
        B4 => scenarios::b4(&b).await,
        B5 => scenarios::b5(&b).await,
        other => Outcome::no_scenario(other),
    }
}

/// Run a table and accept it only by `check`, printing what is not run.
fn accept(name: &str, venue: &dyn Venue, table: &[Expected], outcomes: &[Outcome]) {
    println!("run {name}, not run: {:#?}", pending_list(table));
    for failed in outcomes.iter().filter(|o| matches!(o.verdict, Verdict::Fail(_))) {
        println!("{:?} failed; its trace: {:#?}", failed.property, failed.trace);
    }
    if let Err(problems) = check(&venue.capabilities(), table, outcomes) {
        panic!("run {name} is not accepted:\n{}", problems.join("\n"));
    }
}

#[tokio::test]
async fn run_p_position_stub() {
    let outcomes = run_table(&P_TABLE, p_scenario).await;
    accept("P", make_p().venue(), &P_TABLE, &outcomes);
}

#[tokio::test]
async fn run_b_balance_stub() {
    let outcomes = run_table(&B_TABLE, b_scenario).await;
    accept("B", make_b().venue(), &B_TABLE, &outcomes);
}
