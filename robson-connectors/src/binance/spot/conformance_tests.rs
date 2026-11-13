//! Conformance run S (ADR-0056 §10, step 8, 8d plan §4 and §7.3): every
//! clause of the inventory on the spot adapter, with the real REST client,
//! against `FakeSpotVenue` over loopback, accepted only by `check` against
//! its `const` table; and the composites' request and weight budgets from the
//! fake's own count. Each scenario starts its own venues and shuts each down;
//! a request the fake does not serve, or a venue that does not drain, fails
//! the clause.

use std::{
    future::Future,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use reqwest::Method;
use robson_domain::{OrderSide, Price, Quantity, Symbol};
use robson_exec::{
    order_journal::{IntentKind, IntentSpec, OrderJournal, StubJournalBackend, WriteClass},
    venue::{AccountPort, EntryOrReduce, StopRef, TradingPort, Venue, VenueId},
    ExecError,
};
use robson_testkit::{
    balance_stub::PlacementHook,
    conformance::{
        check, pending_list, run_table,
        scenarios::{self, BalanceHarness},
        Boxed, Coverage,
        Coverage::{NotApplicable, Verified},
        Expected, Harness, Outcome, Property,
        Property::*,
        Verdict,
    },
    fake_spot_venue::{FakeSpotVenue, TestCredentials},
    spot_book::{BookRules, BookStatus, OrderKind, Script, SpotBook},
};
use rust_decimal::Decimal;
use serde_json::json;

use super::{BinanceSpotAdapter, SpotReadError, SpotRestReader};
use crate::binance_rest::BinanceRestClient;

const fn e(property: Property, coverage: Coverage) -> Expected {
    Expected { property, coverage }
}

/// Run S: the spot adapter over the fake spot venue (8d plan §4).
const S_TABLE: [Expected; 22] = [
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

/// BTCUSDT at 100 with BTC 1 and USDT 1000 (fee 0.001), BTCUSDT's rules.
fn seeded_book() -> SpotBook {
    let book = SpotBook::new(vec![BookRules::btcusdt()], d("0.001"));
    book.set_price("BTCUSDT", d("100"));
    book.set_balance("BTC", d("1"));
    book.set_balance("USDT", d("1000"));
    book
}

fn client(venue: &FakeSpotVenue) -> BinanceRestClient {
    let TestCredentials { api_key, api_secret } = TestCredentials::FIXED;
    BinanceRestClient::with_local_base_url_and_credentials(venue.url(), api_key, api_secret)
}

/// The venues a scenario started, shut down after its harnesses are gone.
#[derive(Clone, Default)]
struct Venues(Arc<Mutex<Vec<Arc<FakeSpotVenue>>>>);

impl Venues {
    /// Shut each venue down; what went wrong, if anything.
    async fn close(self) -> Vec<String> {
        let all = std::mem::take(&mut *self.0.lock().unwrap());
        let mut problems = Vec::new();
        for venue in all {
            let Ok(venue) = Arc::try_unwrap(venue) else {
                problems.push("a harness outlived its scenario".to_string());
                continue;
            };
            let unserved = venue.unserved_requests();
            if !unserved.is_empty() {
                problems.push(format!("requests the fake does not serve: {unserved:?}"));
            }
            if let Err(error) = venue.shutdown().await {
                problems.push(format!("shutdown: {error}"));
            }
        }
        problems
    }
}

/// S: the adapter, a one-attempt journal attached, against its own venue.
struct S {
    venue: Arc<FakeSpotVenue>,
    adapter: BinanceSpotAdapter,
    journal: Arc<OrderJournal>,
}

async fn make_s(venues: &Venues) -> S {
    let book = Arc::new(seeded_book());
    let venue = FakeSpotVenue::start(book, TestCredentials::FIXED)
        .await
        .expect("a loopback port");
    let venue = Arc::new(venue);
    venues.0.lock().unwrap().push(Arc::clone(&venue));
    let adapter = BinanceSpotAdapter::new(Arc::new(client(&venue))).unwrap();
    let journal = OrderJournal::new(
        Arc::new(StubJournalBackend::default()),
        &VenueId::new(VenueId::BINANCE_SPOT),
    );
    let journal = Arc::new(journal.with_one_attempt_per_client_id(true));
    adapter.attach_order_journal(Arc::clone(&journal));
    S { venue, adapter, journal }
}

impl S {
    fn order_with(&self, client_id: &str) -> Option<u64> {
        let orders = self.venue.book().all_orders();
        orders.into_iter().rev().find(|o| o.client_id == client_id).map(|o| o.id)
    }
}

impl Harness for S {
    type V = BinanceSpotAdapter;

    fn venue(&self) -> &BinanceSpotAdapter {
        &self.adapter
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

    /// The venue refuses it: HTTP 400 `-2010`.
    fn reject_next_placement(&self) {
        let msg = "Account has insufficient balance for requested action.".to_string();
        self.venue.book().script_next(Script::Reject { code: -2010, msg });
    }

    /// The book acts, and the client gets HTTP 503 `-1007`.
    fn lose_next_placement(&self) {
        self.venue.book().script_next(Script::Lost);
    }

    fn conceal(&self, client_id: &str) {
        if let Some(id) = self.order_with(client_id) {
            self.venue.book().hide_order(id);
        }
    }

    fn reveal(&self, client_id: &str) {
        if let Some(id) = self.order_with(client_id) {
            self.venue.book().show_order(id);
        }
    }

    /// `POST /api/v3/order` requests the fake received: the wire's count.
    fn placements(&self) -> u64 {
        let requests = self.venue.requests();
        let placements =
            requests.iter().filter(|r| r.method == "POST" && r.path == "/api/v3/order");
        placements.count() as u64
    }

    fn open_stops(&self) -> Vec<String> {
        let orders = self.venue.book().all_orders().into_iter();
        let live = orders.filter(|o| {
            matches!(o.kind, OrderKind::StopLoss { .. })
                && matches!(o.status, BookStatus::New | BookStatus::PartiallyFilled)
        });
        live.map(|o| o.id.to_string()).collect()
    }
}

impl BalanceHarness for S {
    fn book(&self) -> &SpotBook {
        self.venue.book()
    }

    fn on_next_placement(&self, hook: PlacementHook) {
        self.venue.on_next_placement(hook);
    }
}

async fn on_s(s: &S, property: Property) -> Outcome {
    match property {
        F1 => scenarios::f1(s).await,
        F2 => scenarios::f2(s).await,
        F3 => scenarios::f3(s).await,
        N1 => scenarios::n1(s).await,
        N2 => scenarios::n2(s).await,
        N3a => scenarios::n3a(s).await,
        N3b => scenarios::n3b(s).await,
        R2 => scenarios::r2(s).await,
        R3 => scenarios::r3(s).await,
        A1a => scenarios::a1a_balance(s).await,
        A1b => scenarios::a1b(s).await,
        A2 => scenarios::a2_balance(s).await,
        A3 => scenarios::a3_balance(s).await,
        A4 => scenarios::a4_balance(s).await,
        B1 => scenarios::b1(s).await,
        B2 => scenarios::b2(s).await,
        B3 => scenarios::b3(s).await,
        B4 => scenarios::b4(s).await,
        B5 => scenarios::b5(s).await,
        other => Outcome::no_scenario(other),
    }
}

/// One clause on fresh venues, each shut down after it; a cleanup failure
/// fails the clause.
fn s_scenario(property: Property) -> Boxed<'static, Outcome> {
    Box::pin(async move {
        let venues = Venues::default();
        let mut outcome = if property == A5b {
            scenarios::a5b(|| make_s(&venues)).await
        } else {
            let s = make_s(&venues).await;
            on_s(&s, property).await
        };
        let problems = venues.close().await;
        if !problems.is_empty() {
            outcome.verdict = match outcome.verdict {
                Verdict::Fail(mut why) => {
                    why.extend(problems);
                    Verdict::Fail(why)
                },
                _ => Verdict::Fail(problems),
            };
        }
        outcome
    })
}

#[tokio::test]
async fn run_s_spot_adapter_over_the_wire() {
    let outcomes = run_table(&S_TABLE, s_scenario).await;
    println!("run S, not run: {:#?}", pending_list(&S_TABLE));
    for failed in outcomes.iter().filter(|o| matches!(o.verdict, Verdict::Fail(_))) {
        println!(
            "{:?} failed: {:?}; its trace: {:#?}",
            failed.property, failed.verdict, failed.trace
        );
    }
    let venues = Venues::default();
    let capabilities = make_s(&venues).await.adapter.capabilities();
    assert!(venues.close().await.is_empty());
    if let Err(problems) = check(&capabilities, &S_TABLE, &outcomes) {
        panic!("run S is not accepted:\n{}", problems.join("\n"));
    }
}

/// S has no pending clause (8d plan §4).
#[test]
fn run_s_has_no_pending_clause() {
    assert!(!S_TABLE.iter().any(|e| matches!(e.coverage, Coverage::Pending(_))));
}

/// What `call` cost on the wire: requests and documented weight.
async fn cost<T>(s: &S, what: &str, call: impl Future<Output = T>) -> (T, u64, u32) {
    let (requests, weight) = (s.venue.requests_total(), s.venue.weight());
    let answer = call.await;
    let (requests, weight) = (s.venue.requests_total() - requests, s.venue.weight() - weight);
    println!("{what}: {requests} requests, weight {weight}");
    (answer, requests, weight)
}

fn ok<T>(result: &Result<T, ExecError>) -> bool {
    result.is_ok()
}

/// §7.3: the composites' budgets, from the fake's own count: an entry 2
/// requests and weight 21 with a rules refresh, 1 and 1 without; a
/// placement 1 and 1; `cancel_protection` at most 7 and 30;
/// `order_by_exchange_id` at most 6 and 29.
#[tokio::test]
async fn run_s_request_and_weight_budgets() {
    let venues = Venues::default();
    let s = make_s(&venues).await;
    let symbol = btc();
    let q = |v: &str| Quantity::new(d(v)).unwrap();
    let entry = |id: &'static str| {
        let send = s.adapter.place_market_order(
            &symbol,
            OrderSide::Buy,
            q("0.1"),
            id,
            EntryOrReduce::Entry,
        );
        s.sending(id, Box::pin(send), ok)
    };
    let (first, requests, weight) = cost(&s, "entry, rules refreshed", entry("w-e1")).await;
    assert!(first.is_ok(), "{first:?}");
    assert_eq!((requests, weight), (2, 21), "an entry with its rules refresh");
    let (second, requests, weight) = cost(&s, "entry within the TTL", entry("w-e2")).await;
    let filled = second.unwrap();
    assert_eq!((requests, weight), (1, 1), "an entry within the rules' TTL");

    let stop = |id: &'static str| {
        let send = s.adapter.place_protection(
            &symbol,
            OrderSide::Sell,
            q("0.5"),
            Price::new(d("90")).unwrap(),
            id,
        );
        s.sending(id, Box::pin(send), ok)
    };
    let (placed, requests, weight) = cost(&s, "placement", stop("w-s1")).await;
    let placed = placed.unwrap();
    assert_eq!((requests, weight), (1, 1), "a placement");

    // A stop that executed in part before its cancel: the cancel and the
    // fills rule's trade pages.
    let old: StopRef = placed.stop.clone();
    s.venue.book().set_price("BTCUSDT", d("85"));
    s.venue.book().cross(old.exchange_order_id.parse().unwrap(), d("0.2")).unwrap();
    let (cancelled, requests, weight) = cost(
        &s,
        "cancel_protection, partly executed",
        s.adapter.cancel_protection(&symbol, &old),
    )
    .await;
    assert!(cancelled.is_ok(), "{cancelled:?}");
    assert!(
        requests <= 7 && weight <= 30,
        "cancel_protection: {requests} requests, weight {weight}"
    );
    // A cancel of a stop that already ended: the cancel, the lookup and the
    // trade pages, the composite's longest path.
    s.venue.book().set_price("BTCUSDT", d("100"));
    let (gone, _, _) = cost(&s, "placement", stop("w-s2")).await;
    let gone = gone.unwrap().stop;
    s.venue.book().set_price("BTCUSDT", d("85"));
    s.venue.book().cross(gone.exchange_order_id.parse().unwrap(), d("0.5")).unwrap();
    let (ended, requests, weight) = cost(
        &s,
        "cancel_protection, already filled",
        s.adapter.cancel_protection(&symbol, &gone),
    )
    .await;
    assert!(ended.is_ok(), "{ended:?}");
    assert!(
        requests <= 7 && weight <= 30,
        "cancel_protection after the end: {requests}, {weight}"
    );
    assert!(requests >= 3, "the cancel, the lookup and a trade page");

    let id = filled.exchange_order_id.clone();
    let (found, requests, weight) =
        cost(&s, "order_by_exchange_id", s.adapter.order_by_exchange_id(&symbol, &id)).await;
    assert!(matches!(found, Ok(Some(_))), "{found:?}");
    assert!(
        requests <= 6 && weight <= 29,
        "order_by_exchange_id: {requests} requests, weight {weight}"
    );
    assert!(requests >= 2, "the lookup and a trade page");

    drop(s);
    let problems = venues.close().await;
    assert!(problems.is_empty(), "{problems:?}");
}

/// §2.3: a held request ends in the client's own shortened timeout, never
/// in an answer; the venue still drains.
#[tokio::test]
async fn a_held_request_ends_in_the_clients_timeout() {
    let venue = FakeSpotVenue::start(Arc::new(seeded_book()), TestCredentials::FIXED)
        .await
        .unwrap();
    venue.hold_next(Method::GET, "/api/v3/account");
    let bound = Duration::from_millis(300);
    let reader = SpotRestReader::new(Arc::new(client(&venue).with_request_timeout(bound)));
    let started = Instant::now();
    let read = reader.account().await;
    assert!(matches!(read, Err(SpotReadError::Timeout)), "{read:?}");
    assert!(started.elapsed() < Duration::from_secs(5));
    let held = venue.requests();
    assert_eq!(held.len(), 1, "the held request reached the venue");
    assert!(venue.unserved_requests().is_empty());
    venue.shutdown().await.unwrap();
}

/// The client signs as the fake checks: an authenticated read passes, and
/// the fixed `with_local_base_url` key is refused for its format.
#[tokio::test]
async fn the_fakes_authentication_accepts_the_clients_signature() {
    let venue = FakeSpotVenue::start(Arc::new(seeded_book()), TestCredentials::FIXED)
        .await
        .unwrap();
    let reader = SpotRestReader::new(Arc::new(client(&venue)));
    let account = reader.account().await.unwrap();
    assert!(account.balances.iter().any(|b| b.asset == "BTC" && b.free == d("1")));
    let fixed = SpotRestReader::new(Arc::new(BinanceRestClient::with_local_base_url(venue.url())));
    let refused = fixed.account().await;
    assert!(
        matches!(refused, Err(SpotReadError::Api { code: -2014, http_status: 401, .. })),
        "{refused:?}"
    );
    venue.shutdown().await.unwrap();
}
