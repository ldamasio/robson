use proptest::prelude::*;
use robson_connectors::binance::spot::{SpotAccount, SpotAccountBalance};
use robson_exec::{clock_records::RawClockCapture, venue::VenueId, venue_clock::ClockDomain};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use uuid::Uuid;

use super::*;

const TENANT: Uuid = Uuid::from_u128(0x0001);
const SCOPE: &str = "pilot-scope";

fn venue() -> VenueId {
    VenueId::new(VenueId::BINANCE_SPOT)
}

fn scope(venue: &VenueId) -> DetectorScope<'_> {
    DetectorScope {
        expected: ExpectedGeneration {
            tenant_id: TENANT,
            venue,
            account_scope: SCOPE,
            generation_id: Uuid::from_u128(2),
            process_epoch: Uuid::from_u128(3),
            source_audit_id: Uuid::from_u128(4),
        },
        uid: "1",
        operated: "BTCUSDT",
        base_asset: "BTC",
    }
}

fn account(balances: &[(&str, Decimal, Decimal)]) -> SpotAccount {
    SpotAccount {
        uid: 1,
        can_trade: true,
        account_type: "SPOT".into(),
        permissions: vec!["SPOT".into()],
        balances: balances
            .iter()
            .map(|(asset, free, locked)| SpotAccountBalance {
                asset: (*asset).into(),
                free: *free,
                locked: *locked,
            })
            .collect(),
    }
}

fn btc(free: Decimal, locked: Decimal) -> SpotAccount {
    account(&[("BTC", free, locked)])
}

/// A leg with a recorded partition whose entry is ledger-backed and terminal.
struct Built {
    pre: PreSnapshot,
    listing: Listing,
}

impl Built {
    fn new() -> Self {
        Self {
            pre: PreSnapshot {
                version: 7,
                closed_legs: 0,
                legs: Vec::new(),
                orders: Vec::new(),
                intents: Vec::new(),
            },
            listing: Listing::default(),
        }
    }

    fn linked(
        kind: &str,
        position: Uuid,
        client: &str,
        phase: &str,
        executed: Decimal,
        id: &str,
    ) -> LinkedOrder {
        LinkedOrder {
            owner: "leg".into(),
            kind: Some(kind.into()),
            position_id: Some(position),
            tenant_id: TENANT,
            venue_id: VenueId::BINANCE_SPOT.into(),
            account_scope: SCOPE.into(),
            client_order_id: client.into(),
            phase: phase.into(),
            executed,
            exchange_order_id: id.into(),
        }
    }

    fn intent(kind: &str, position: Uuid, client: &str, linked: Option<LinkedOrder>) -> IntentRow {
        IntentRow {
            intent_id: Uuid::now_v7(),
            kind: kind.into(),
            position_id: Some(position),
            not_sent: false,
            venue_outcome: None,
            disposition_kind: None,
            uncertain_at: None,
            client_order_id: client.into(),
            admission_of: None,
            target_terminal_proof: None,
            linked,
        }
    }

    /// Q, B (provisional) and X with `consumed`; entry terminal at Q.
    fn partitioned(&mut self, q: Decimal, b: Decimal, x: Decimal, consumed: Decimal) -> Uuid {
        let position = Uuid::now_v7();
        let client = format!("entry-{position}");
        let mut admission = Self::intent(
            "entry",
            position,
            &client,
            Some(Self::linked("entry", position, &client, "terminal", q, "1")),
        );
        admission.admission_of = Some(position);
        self.pre.legs.push(LegRow {
            position_id: position,
            admission_intent_id: admission.intent_id,
            gross: Some(q),
            base_fee: Some(b),
            base_fee_final: false,
            executable: Some(x),
            consumed,
            recovery_state: None,
            entry_orders: 1,
            entry_linked: true,
            requested: Some(q),
        });
        self.pre.intents.push(admission);
        position
    }

    /// An open leg whose entry has no partition yet and no order row: the
    /// admission intent is sent with no definite outcome (case b).
    fn claimed(&mut self, requested: Decimal) -> Uuid {
        let position = Uuid::now_v7();
        let mut admission = Self::intent("entry", position, &format!("entry-{position}"), None);
        admission.admission_of = Some(position);
        self.pre.legs.push(LegRow {
            position_id: position,
            admission_intent_id: admission.intent_id,
            gross: None,
            base_fee: None,
            base_fee_final: false,
            executable: None,
            consumed: Decimal::ZERO,
            recovery_state: None,
            entry_orders: 0,
            entry_linked: true,
            requested: Some(requested),
        });
        self.pre.intents.push(admission);
        position
    }

    /// A ledger-backed live protection stop, listed or not.
    fn stop(&mut self, position: Uuid, id: u64, orig: Decimal, executed: Decimal, listed: bool) {
        let client = format!("stop-{id}");
        let intent = Self::intent(
            "protection",
            position,
            &client,
            Some(Self::linked("stop", position, &client, "live", executed, &id.to_string())),
        );
        self.pre.orders.push(OrderRow {
            position_id: position,
            exchange_order_id: id.to_string(),
            kind: "stop".into(),
            phase: "live".into(),
            orig_qty: orig,
            executed,
            intent_id: Some(intent.intent_id),
        });
        if listed {
            self.listing.orders.push(OperatedOrder {
                symbol: "BTCUSDT".into(),
                order_id: id,
                attribution: Attribution::Ledger { intent_id: intent.intent_id },
            });
        }
        self.pre.intents.push(intent);
    }

    fn classify(&self, account: &SpotAccount) -> Classification {
        let venue = venue();
        classify(scope(&venue), &self.pre, account, &self.listing)
    }
}

#[test]
fn open_dust_is_inside_the_upper_end() {
    let mut built = Built::new();
    // Q 1, B̂ 0.001, X 0.99: the wallet holds Q − B = 0.999, dust 0.009 open.
    built.partitioned(dec!(1), dec!(0.001), dec!(0.99), Decimal::ZERO);
    assert_eq!(built.classify(&btc(dec!(0.999), Decimal::ZERO)), Classification::Clean);
    assert_eq!(built.classify(&btc(dec!(1), Decimal::ZERO)), Classification::Clean);
    assert_eq!(
        built.classify(&btc(dec!(1.00000001), Decimal::ZERO)),
        Classification::Discrepancy(vec![Finding::BaseExcess {
            holdings: dec!(1.00000001),
            upper: dec!(1),
        }])
    );
}

#[test]
fn consumed_above_x_is_incomplete() {
    let mut built = Built::new();
    built.partitioned(dec!(1), dec!(0.001), dec!(0.99), dec!(0.995));
    assert_eq!(
        built.classify(&btc(dec!(0.004), Decimal::ZERO)),
        Classification::Incomplete(IncompleteReason::Deficit)
    );
}

#[test]
fn partially_executed_stop_locks_its_remainder() {
    let mut built = Built::new();
    let position = built.partitioned(dec!(1), dec!(0.001), dec!(0.99), dec!(0.2));
    built.stop(position, 41, dec!(0.5), dec!(0.2), true);
    // Holdings 0.799 inside [0.79, 0.8]; locked equals 0.5 − 0.2.
    assert_eq!(built.classify(&btc(dec!(0.499), dec!(0.3))), Classification::Clean);
    assert_eq!(
        built.classify(&btc(dec!(0.299), dec!(0.5))),
        Classification::Discrepancy(vec![Finding::LockedForeign {
            locked: dec!(0.5),
            robson: dec!(0.3),
        }])
    );
}

#[test]
fn other_asset_presence_is_ignored() {
    let mut built = Built::new();
    built.partitioned(dec!(1), dec!(0.001), dec!(0.99), Decimal::ZERO);
    let busy = account(&[
        ("USDT", dec!(1805), dec!(17)),
        ("BTC", dec!(0.995), Decimal::ZERO),
        ("ETH", dec!(3), dec!(1)),
    ]);
    assert_eq!(built.classify(&busy), Classification::Clean);
    built.listing.unrelated_orders = 2;
    built.listing.unrelated_lists = 1;
    assert_eq!(built.classify(&busy), Classification::Clean);
}

#[test]
fn unlisted_robson_stop_needs_exact_evidence() {
    let mut built = Built::new();
    let position = built.partitioned(dec!(1), dec!(0.001), dec!(0.99), Decimal::ZERO);
    built.stop(position, 77, dec!(0.99), Decimal::ZERO, false);
    assert_eq!(
        built.classify(&btc(dec!(0.005), dec!(0.99))),
        Classification::Discrepancy(vec![Finding::NeedsExactEvidence {
            exchange_order_id: "77".into(),
        }])
    );
}

#[test]
fn the_btc_balance_must_be_listed_exactly_once() {
    let built = Built::new();
    for balances in [vec![], vec![
        ("BTC", dec!(1), dec!(0)),
        ("BTC", dec!(0), dec!(0)),
    ]] {
        assert_eq!(
            built.classify(&account(&balances)),
            Classification::Incomplete(IncompleteReason::MalformedAccount)
        );
    }
    assert_eq!(built.classify(&btc(Decimal::ZERO, Decimal::ZERO)), Classification::Clean);
}

#[test]
fn a_closed_leg_without_a_write_off_record_is_incomplete() {
    let mut built = Built::new();
    built.pre.closed_legs = 1;
    assert_eq!(
        built.classify(&btc(Decimal::ZERO, Decimal::ZERO)),
        Classification::Incomplete(IncompleteReason::ClosedLegWithoutWriteOff)
    );
}

#[test]
fn entry_evidence_decides_a_leg_without_partition() {
    // (b) sent, no outcome, no order row: a claim, never Clean or Incomplete.
    let mut built = Built::new();
    built.claimed(dec!(0.002));
    assert_eq!(
        built.classify(&btc(dec!(0.001), Decimal::ZERO)),
        Classification::Discrepancy(vec![Finding::WithinClaims {
            holdings: dec!(0.001),
            lower: Decimal::ZERO,
            upper: dec!(0.002),
        }])
    );
    // (c) success without an order row is still a claim.
    built.pre.intents[0].venue_outcome = Some("success".into());
    assert!(matches!(
        built.classify(&btc(Decimal::ZERO, Decimal::ZERO)),
        Classification::Discrepancy(f) if matches!(f[..], [Finding::WithinClaims { .. }])
    ));
    // (f) proven unsent: [0, 0].
    built.pre.intents[0].venue_outcome = None;
    built.pre.intents[0].not_sent = true;
    assert_eq!(built.classify(&btc(Decimal::ZERO, Decimal::ZERO)), Classification::Clean);
    // (a) a ledger-backed live entry; (d) unfilled terminal; (e) filled terminal.
    built.pre.intents[0].not_sent = false;
    let position = built.pre.legs[0].position_id;
    let client = built.pre.intents[0].client_order_id.clone();
    built.pre.legs[0].entry_orders = 1;
    for (phase, executed, expected) in [
        ("live", dec!(0.001), None),
        ("terminal", Decimal::ZERO, Some(Classification::Clean)),
        (
            "terminal",
            dec!(0.001),
            Some(Classification::Incomplete(IncompleteReason::UndefinedPartition)),
        ),
    ] {
        built.pre.intents[0].linked =
            Some(Built::linked("entry", position, &client, phase, executed, "9"));
        let judged = built.classify(&btc(Decimal::ZERO, Decimal::ZERO));
        match expected {
            Some(expected) => assert_eq!(judged, expected, "{phase} {executed}"),
            None => assert!(
                matches!(judged, Classification::Discrepancy(ref f)
                if matches!(f[..], [Finding::WithinClaims { .. }])),
                "{judged:?}"
            ),
        }
    }
    // A linked order that fails the exact linkage is a contradiction.
    built.pre.intents[0].linked =
        Some(Built::linked("entry", position, "other-client", "live", dec!(0.001), "9"));
    assert_eq!(
        built.classify(&btc(Decimal::ZERO, Decimal::ZERO)),
        Classification::Incomplete(IncompleteReason::AttributionContradiction)
    );
}

#[test]
fn a_recorded_entry_still_needs_its_exact_linkage() {
    let mut built = Built::new();
    built.partitioned(dec!(1), dec!(0.001), dec!(0.99), Decimal::ZERO);
    let wallet = btc(dec!(0.995), Decimal::ZERO);
    assert_eq!(built.classify(&wallet), Classification::Clean);
    let linked = built.pre.intents[0].linked.as_mut().unwrap();
    linked.account_scope = "another-scope".into();
    assert_eq!(
        built.classify(&wallet),
        Classification::Incomplete(IncompleteReason::AttributionContradiction)
    );
}

#[test]
fn uncertain_and_settled_intents() {
    let mut built = Built::new();
    let position = built.partitioned(dec!(1), dec!(0.001), dec!(0.99), Decimal::ZERO);
    let wallet = btc(dec!(0.995), Decimal::ZERO);
    // An uncertain protection intent with no order row.
    built
        .pre
        .intents
        .push(Built::intent("protection", position, "p-uncertain", None));
    assert_eq!(
        built.classify(&wallet),
        Classification::Incomplete(IncompleteReason::UncertainIntent)
    );
    // A 'success' protection with no recorded order is still unrecorded work.
    built.pre.intents.last_mut().unwrap().venue_outcome = Some("success".into());
    assert_eq!(
        built.classify(&wallet),
        Classification::Incomplete(IncompleteReason::UncertainIntent)
    );
    built.pre.intents.pop();
    // An uncertain cancel: settled only by its exact terminal-target proof.
    let mut cancel = Built::intent("cancel", position, "c-1", None);
    cancel.target_terminal_proof = Some(false);
    built.pre.intents.push(cancel);
    assert_eq!(
        built.classify(&wallet),
        Classification::Incomplete(IncompleteReason::UncertainCancel)
    );
    built.pre.intents.last_mut().unwrap().target_terminal_proof = Some(true);
    assert_eq!(built.classify(&wallet), Classification::Clean);
    // A ledger-backed terminal protection, NULL outcome, is settled.
    built.pre.intents.push(Built::intent(
        "protection",
        position,
        "p-done",
        Some(Built::linked("stop", position, "p-done", "terminal", Decimal::ZERO, "5")),
    ));
    assert_eq!(built.classify(&wallet), Classification::Clean);
    // A live Robson sell is in flight.
    built.pre.orders.push(OrderRow {
        position_id: position,
        exchange_order_id: "6".into(),
        kind: "sell".into(),
        phase: "live".into(),
        orig_qty: dec!(0.1),
        executed: Decimal::ZERO,
        intent_id: None,
    });
    assert_eq!(
        built.classify(&wallet),
        Classification::Incomplete(IncompleteReason::InFlightOrder)
    );
}

#[test]
fn listing_attribution_and_lists() {
    let mut built = Built::new();
    built.partitioned(dec!(1), dec!(0.001), dec!(0.99), Decimal::ZERO);
    let wallet = btc(dec!(0.995), Decimal::ZERO);
    for attribution in [
        Attribution::Absent,
        Attribution::Discovered,
        Attribution::Unauthored,
    ] {
        built.listing.orders = vec![OperatedOrder {
            symbol: "BTCUSDT".into(),
            order_id: 900,
            attribution,
        }];
        assert_eq!(
            built.classify(&wallet),
            Classification::Discrepancy(vec![Finding::UnlinkedOperatedOrder { order_id: 900 }])
        );
    }
    built.listing.orders[0].attribution = Attribution::Contradiction;
    assert_eq!(
        built.classify(&wallet),
        Classification::Incomplete(IncompleteReason::AttributionContradiction)
    );
    built.listing.orders.clear();
    built.listing.lists.push(OperatedList { order_list_id: 9 });
    assert_eq!(
        built.classify(&wallet),
        Classification::Discrepancy(vec![Finding::OperatedOrderList { order_list_id: 9 }])
    );
}

#[test]
fn consequences_never_sell() {
    use Consequence as C;
    let d = Decimal::ZERO;
    for (finding, expected) in [
        (Finding::BaseExcess { holdings: d, upper: d }, C::FenceAndPage),
        (Finding::LockedForeign { locked: d, robson: d }, C::FenceAndPage),
        (Finding::BaseDeficit { holdings: d, lower: d }, C::FenceAndPage),
        (Finding::OperatedOrderList { order_list_id: 1 }, C::FenceAndPage),
        (Finding::LockedShort { locked: d, robson: d, stops: vec![] }, C::ExactLookup),
        (Finding::NeedsExactEvidence { exchange_order_id: "1".into() }, C::ExactLookup),
        (Finding::UnlinkedOperatedOrder { order_id: 1 }, C::StrictRecovery),
        (Finding::WithinClaims { holdings: d, lower: d, upper: d }, C::Hold),
    ] {
        assert_eq!(consequence(&finding), expected, "{finding:?}");
    }
}

fn interval() -> ObservationInterval {
    ObservationInterval { start_ns: 1, end_ns: 2, cut_ns: 1 }
}

fn discrepancy(findings: Vec<Finding>) -> BaseVerdict {
    BaseVerdict::Discrepancy { findings, interval: interval() }
}

fn fence_and_page() -> [Finding; 4] {
    [
        Finding::LockedForeign { locked: dec!(1), robson: dec!(0.999) },
        Finding::BaseExcess { holdings: dec!(1.5), upper: dec!(1) },
        Finding::BaseDeficit { holdings: dec!(0.5), lower: dec!(0.999) },
        Finding::OperatedOrderList { order_list_id: 9 },
    ]
}

fn other_consequences() -> [Finding; 4] {
    [
        Finding::LockedShort {
            locked: dec!(0.5),
            robson: dec!(0.999),
            stops: vec!["41".into()],
        },
        Finding::NeedsExactEvidence { exchange_order_id: "41".into() },
        Finding::UnlinkedOperatedOrder { order_id: 900 },
        Finding::WithinClaims {
            holdings: dec!(0),
            lower: dec!(0),
            upper: dec!(1),
        },
    ]
}

#[test]
fn apply_fences_on_each_fence_and_page_finding() {
    for finding in fence_and_page() {
        let journal = robson_exec::order_journal::OrderJournal::unpersisted();
        let gate = journal.gate();
        let occurrence = apply(&discrepancy(vec![finding.clone()]), gate).expect("recorded");
        assert_eq!(occurrence.reason.as_str(), "venue_discrepancy", "{finding:?}");
        assert!(occurrence.opened && gate.fenced() && !journal.latch_set(), "{finding:?}");
        assert_eq!(gate.open_incidents().len(), 1, "{finding:?}");
    }
}

#[test]
fn apply_ignores_verdicts_without_a_fence_and_page_finding() {
    let mut verdicts = vec![
        BaseVerdict::Clean { interval: interval() },
        BaseVerdict::Incomplete {
            reason: IncompleteReason::Deficit,
            interval: interval(),
        },
        BaseVerdict::Unqualified(UnqualifiedReason::Venue),
    ];
    verdicts.extend(other_consequences().into_iter().map(|finding| discrepancy(vec![finding])));
    for verdict in verdicts {
        let journal = robson_exec::order_journal::OrderJournal::unpersisted();
        let gate = journal.gate();
        let generation = gate.generation();
        assert_eq!(apply(&verdict, gate), None, "{verdict:?}");
        assert!(
            !gate.fenced() && gate.generation() == generation && gate.open_incidents().is_empty(),
            "{verdict:?}"
        );
    }
}

#[test]
fn apply_records_once_for_several_findings() {
    let [locked, excess, ..] = fence_and_page();
    let [short, ..] = other_consequences();
    let journal = robson_exec::order_journal::OrderJournal::unpersisted();
    let gate = journal.gate();
    let generation = gate.generation();
    apply(&discrepancy(vec![locked, short, excess]), gate).expect("recorded");
    let open = gate.open_incidents();
    assert_eq!((open.len(), open[0].count, gate.generation()), (1, 1, generation + 1));
}

#[test]
fn a_clean_verdict_never_clears_an_open_discrepancy() {
    let journal = robson_exec::order_journal::OrderJournal::unpersisted();
    let gate = journal.gate();
    let [locked, ..] = fence_and_page();
    apply(&discrepancy(vec![locked]), gate).expect("recorded");
    for later in [
        BaseVerdict::Clean { interval: interval() },
        BaseVerdict::Unqualified(UnqualifiedReason::Clock),
    ] {
        assert_eq!(apply(&later, gate), None, "{later:?}");
        let open: Vec<_> = gate.open_incidents().iter().map(|i| i.reason.as_str()).collect();
        assert!(gate.fenced() && open == ["venue_discrepancy"], "{later:?}: {open:?}");
    }
}

fn capture(seconds: i64, nanos: u32, before: i128, after: i128) -> RawClockCapture {
    RawClockCapture {
        epoch: Uuid::from_u128(3),
        wall_seconds: seconds,
        wall_nanoseconds: nanos,
        domain: ClockDomain::new(Uuid::from_u128(1), -1, 7).unwrap(),
        before_ns: before,
        after_ns: after,
    }
}

#[test]
fn account_interval_is_the_cut_source() {
    let send = capture(1_794_614_400, 100, 10, 20);
    let receive = capture(1_794_614_400, 900_000_000, 30, 40);
    let interval = ObservationInterval::from_account(&send, &receive, 202_000_001).unwrap();
    let wall = |c: &RawClockCapture| {
        i128::from(c.wall_seconds) * 1_000_000_000 + i128::from(c.wall_nanoseconds)
    };
    // u = ⌈u2 / 2⌉ widens both ends; the cut is the send end, never a later
    // receipt.
    assert_eq!(interval.start_ns, wall(&send) - 101_000_001);
    assert_eq!(interval.end_ns, wall(&receive) + 101_000_001);
    assert_eq!(interval.cut_ns, interval.start_ns);
    let later = capture(1_794_614_401, 0, 50, 60);
    assert_ne!(ObservationInterval::from_account(&send, &later, 202_000_001), Some(interval));
    assert_eq!(ObservationInterval::from_account(&receive, &send, 0), None);
}

fn full() -> Progress {
    Progress { pre: true, responses: 3, post: true }
}

fn qualified() -> AttemptOutcome {
    let send = capture(1_794_614_400, 100, 10, 20);
    let receive = capture(1_794_614_400, 200, 30, 40);
    AttemptOutcome::Qualified(ObservationInterval::from_account(&send, &receive, 2).unwrap())
}

#[test]
fn three_attempts_count_every_prefix() {
    let venue_at_open_orders = Attempt {
        progress: Progress { pre: true, responses: 2, post: false },
        outcome: AttemptOutcome::Unqualified(UnqualifiedReason::Venue),
    };
    let moved = Attempt {
        progress: full(),
        outcome: AttemptOutcome::Unqualified(UnqualifiedReason::VersionChanged),
    };
    let clock_at_account = Attempt {
        progress: Progress { pre: true, responses: 1, post: false },
        outcome: AttemptOutcome::Unqualified(UnqualifiedReason::Clock),
    };
    let never = Attempt { progress: full(), outcome: qualified() };
    let outcome = scan([venue_at_open_orders, moved, clock_at_account, never]).unwrap();
    assert_eq!(outcome.attempts, 3);
    assert_eq!(outcome.outcome, AttemptOutcome::Unqualified(UnqualifiedReason::Clock));
    // 1+2+1 SQL, 2+3+1 HTTP, 26+32+20 weight.
    assert_eq!(outcome.cost, ScanCost { sql: 4, http: 6, weight: 78 });
    let retried = scan([moved, Attempt { progress: full(), outcome: qualified() }]).unwrap();
    assert_eq!((retried.attempts, retried.cost), (2, ScanCost { sql: 4, http: 6, weight: 64 }));
    // A permanent refusal at pre stops the scan.
    let scope = Attempt {
        progress: Progress { pre: true, responses: 0, post: false },
        outcome: AttemptOutcome::Unqualified(UnqualifiedReason::Scope),
    };
    let refused = scan([scope, never]).unwrap();
    assert_eq!((refused.attempts, refused.cost), (1, ScanCost { sql: 1, http: 0, weight: 0 }));
}

#[test]
fn qualification_reads_every_mark() {
    let interval = match qualified() {
        AttemptOutcome::Qualified(i) => i,
        _ => unreachable!(),
    };
    let base = QualificationMarks {
        generation_before: 5,
        generation_after: 5,
        pending_before: false,
        pending_after: false,
        post: PostOutcome::Matched,
        clock: ClockOutcome::Valid(interval),
    };
    use UnqualifiedReason as U;
    assert_eq!(qualify(&base), AttemptOutcome::Qualified(interval));
    for (marks, reason) in [
        (QualificationMarks { post: PostOutcome::ScopeMoved, ..base }, U::VersionChanged),
        (QualificationMarks { post: PostOutcome::Sql, ..base }, U::Sql),
        (QualificationMarks { post: PostOutcome::Timeout, ..base }, U::Timeout),
        (QualificationMarks { generation_after: 6, ..base }, U::GenerationChanged),
        (QualificationMarks { pending_before: true, ..base }, U::PendingEvidence),
        (QualificationMarks { pending_after: true, ..base }, U::PendingEvidence),
        (QualificationMarks { clock: ClockOutcome::Invalid, ..base }, U::Clock),
    ] {
        assert_eq!(qualify(&marks), AttemptOutcome::Unqualified(reason));
    }
}

// Qualification oracle over 6 points × 8 perturbations × attempts 1..=3.
// Points: 0 before pre and the first gate marks; 1 after pre, before ACCOUNT;
// 2 between ACCOUNT and OPEN_ORDERS; 3 between OPEN_ORDERS and
// OPEN_ORDER_LIST; 4 after the triple's final check, before post; 5 after post
// and the second gate marks.
#[derive(Debug, Clone, Copy)]
enum Perturbation {
    LedgerWrite,
    PendingRise,
    OlderAckAfterNewer,
    GenerationChange,
    ClockInvalidAccount,
    ClockInvalidLater,
    PostSqlError,
    VenueError,
}

const PERTURBATIONS: [Perturbation; 8] = [
    Perturbation::LedgerWrite,
    Perturbation::PendingRise,
    Perturbation::OlderAckAfterNewer,
    Perturbation::GenerationChange,
    Perturbation::ClockInvalidAccount,
    Perturbation::ClockInvalidLater,
    Perturbation::PostSqlError,
    Perturbation::VenueError,
];

/// What the outside world holds while one attempt runs.
#[derive(Default)]
struct World {
    version: u32,
    generation: i64,
    evidence: u32,
    acknowledged: u32,
    account_clock_invalid: bool,
    later_clock_invalid: bool,
    venue_down: bool,
    post_fails: bool,
}

impl World {
    fn apply(&mut self, perturbation: Perturbation) {
        use Perturbation as P;
        match perturbation {
            P::LedgerWrite => self.version += 1,
            P::PendingRise => self.evidence += 1,
            // Newer evidence, then the older one's ACK: still pending.
            P::OlderAckAfterNewer => {
                self.evidence += 2;
                self.acknowledged = self.acknowledged.max(self.evidence - 1);
            },
            P::GenerationChange => self.generation += 1,
            P::ClockInvalidAccount => self.account_clock_invalid = true,
            P::ClockInvalidLater => self.later_clock_invalid = true,
            P::PostSqlError => self.post_fails = true,
            P::VenueError => self.venue_down = true,
        }
    }

    fn pending(&self) -> bool {
        self.acknowledged < self.evidence
    }
}

/// One attempt in the composition's order, with `event` applied at its point.
/// It builds the marks and calls `qualify` as the composition does.
fn simulate(event: Option<(Perturbation, u8)>) -> Attempt {
    let mut world = World::default();
    let at = |point: u8, world: &mut World| {
        if let Some((perturbation, _)) = event.filter(|(_, at)| *at == point) {
            world.apply(perturbation);
        }
    };
    let stop = |progress, reason| Attempt {
        progress,
        outcome: AttemptOutcome::Unqualified(reason),
    };
    at(0, &mut world);
    let (generation_before, pending_before) = (world.generation, world.pending());
    let read = world.version;
    let mut progress = Progress { pre: true, responses: 0, post: false };
    for response in 1..=3u8 {
        at(response, &mut world);
        progress.responses = response;
        if world.venue_down {
            return stop(progress, UnqualifiedReason::Venue);
        }
        let receipt_invalid = if response == 1 {
            world.account_clock_invalid
        } else {
            world.later_clock_invalid
        };
        if receipt_invalid {
            return stop(progress, UnqualifiedReason::Clock);
        }
    }
    // The triple revalidates its ACCOUNT anchor before it returns.
    if world.account_clock_invalid {
        return stop(progress, UnqualifiedReason::Clock);
    }
    at(4, &mut world);
    progress.post = true;
    let post = if world.post_fails {
        PostOutcome::Sql
    } else if world.version != read {
        PostOutcome::ScopeMoved
    } else {
        PostOutcome::Matched
    };
    let AttemptOutcome::Qualified(interval) = qualified() else {
        unreachable!()
    };
    let marks = QualificationMarks {
        generation_before,
        generation_after: world.generation,
        pending_before,
        pending_after: world.pending(),
        post,
        // After post the ACCOUNT anchor is revalidated, never retroactively.
        clock: if world.account_clock_invalid {
            ClockOutcome::Invalid
        } else {
            ClockOutcome::Valid(interval)
        },
    };
    at(5, &mut world);
    Attempt { progress, outcome: qualify(&marks) }
}

/// Independent expectation from the plan's rules, not from `qualify`: the
/// reason a discarded attempt carries and its (SQL, HTTP, weight).
fn oracle(perturbation: Perturbation, at: u8) -> Option<(UnqualifiedReason, u32, u32, u32)> {
    use Perturbation as P;
    use UnqualifiedReason as U;
    const FULL: (u32, u32, u32) = (2, 3, 32);
    let (reason, (sql, http, weight)) = match (perturbation, at) {
        // A write before pre is seen by pre; one after post is late evidence.
        (P::LedgerWrite, 1..=4) => (U::VersionChanged, FULL),
        // Pending is read before pre and after post.
        (P::PendingRise | P::OlderAckAfterNewer, 0..=4) => (U::PendingEvidence, FULL),
        (P::GenerationChange, 1..=4) => (U::GenerationChanged, FULL),
        // The ACCOUNT receipt itself, or its revalidation ending the triple.
        (P::ClockInvalidAccount, 0 | 1) => (U::Clock, (1, 1, 20)),
        (P::ClockInvalidAccount, 2 | 3) => (U::Clock, (1, 3, 32)),
        // Past the triple, the anchor's revalidation after post refuses it.
        (P::ClockInvalidAccount, 4) => (U::Clock, FULL),
        (P::ClockInvalidLater, 0..=2) => (U::Clock, (1, 2, 26)),
        (P::ClockInvalidLater, 3) => (U::Clock, (1, 3, 32)),
        (P::PostSqlError, 0..=4) => (U::Sql, FULL),
        (P::VenueError, 0 | 1) => (U::Venue, (1, 1, 20)),
        (P::VenueError, 2) => (U::Venue, (1, 2, 26)),
        (P::VenueError, 3) => (U::Venue, (1, 3, 32)),
        _ => return None,
    };
    Some((reason, sql, http, weight))
}

#[test]
fn qualification_oracle_covers_144_schedules() {
    let mut schedules = 0;
    let mut mid_triple = 0;
    for point in 0..=5u8 {
        for perturbation in PERTURBATIONS {
            for hit in 1..=3u8 {
                schedules += 1;
                mid_triple += usize::from((2..=3).contains(&point));
                // The event recurs in attempts 1..=hit; later ones are clean.
                let run = (1..=3u8).map(|n| simulate((n <= hit).then_some((perturbation, point))));
                let outcome = scan(run).unwrap();
                let n = u32::from(hit);
                let (attempts, sql, http, weight, last) = match oracle(perturbation, point) {
                    None => (1, 2, 3, 32, None),
                    Some((reason, s, h, w)) if hit == 3 => (3, 3 * s, 3 * h, 3 * w, Some(reason)),
                    Some((_, s, h, w)) => (hit + 1, n * s + 2, n * h + 3, n * w + 32, None),
                };
                let context = format!("{perturbation:?} at {point}, hit {hit}");
                assert_eq!(outcome.attempts, attempts, "{context}");
                assert_eq!(outcome.cost, ScanCost { sql, http, weight }, "{context}");
                match last {
                    Some(reason) => {
                        assert_eq!(
                            outcome.outcome,
                            AttemptOutcome::Unqualified(reason),
                            "{context}"
                        )
                    },
                    // Qualified with the ACCOUNT receipt's own interval.
                    None => assert_eq!(outcome.outcome, qualified(), "{context}"),
                }
            }
        }
    }
    assert_eq!((schedules, mid_triple), (144, 48));
}

// Independent integer oracle in 1e-8 units; it does not call partition_facts.
#[derive(Debug, Clone)]
struct GenLeg {
    q: i64,
    b: i64,
    x: i64,
    consumed: i64,
    final_fee: bool,
    stops: Vec<(i64, i64)>,
}

fn units(value: i64) -> Decimal {
    Decimal::new(value, 8)
}

fn gen_leg() -> impl Strategy<Value = GenLeg> {
    (1i64..=1_000_000)
        .prop_flat_map(|q| (Just(q), 0..=q / 100))
        .prop_flat_map(|(q, b)| (Just(q), Just(b), 0..=q - b))
        .prop_flat_map(|(q, b, x)| {
            (
                Just(q),
                Just(b),
                Just(x),
                0..=x,
                any::<bool>(),
                prop::collection::vec((1i64..=100_000).prop_flat_map(|o| (Just(o), 0..=o)), 0..=2),
            )
        })
        .prop_map(|(q, b, x, consumed, final_fee, stops)| GenLeg {
            q,
            b,
            x,
            consumed,
            final_fee,
            stops,
        })
}

proptest! {
    #[test]
    fn classification_matches_an_integer_oracle(
        legs in prop::collection::vec(gen_leg(), 0..=3),
        claims in prop::collection::vec(1i64..=1_000_000, 0..=1),
        free in 0i64..=4_000_000,
        locked in 0i64..=400_000,
        unlinked in any::<bool>(),
    ) {
        let mut built = Built::new();
        let mut id = 100;
        for leg in &legs {
            let position = built.partitioned(units(leg.q), units(leg.b), units(leg.x), units(leg.consumed));
            // B̂ or a final B: the interval does not depend on which.
            built.pre.legs.last_mut().unwrap().base_fee_final = leg.final_fee;
            for (orig, executed) in &leg.stops {
                id += 1;
                built.stop(position, id, units(*orig), units(*executed), true);
            }
        }
        for requested in &claims {
            built.claimed(units(*requested));
        }
        if unlinked {
            built.listing.orders.push(OperatedOrder {
                symbol: "BTCUSDT".into(), order_id: 7, attribution: Attribution::Discovered,
            });
        }
        let lower: i64 = legs.iter().map(|l| l.x - l.consumed).sum();
        let upper: i64 = legs.iter().map(|l| l.q - l.consumed).sum::<i64>() + claims.iter().sum::<i64>();
        let robson: i64 = legs.iter().flat_map(|l| &l.stops).map(|(o, e)| o - e).sum();
        let holdings = free + locked;
        let mut expected = Vec::new();
        if unlinked {
            expected.push(Finding::UnlinkedOperatedOrder { order_id: 7 });
        }
        if holdings < lower {
            expected.push(Finding::BaseDeficit { holdings: units(holdings), lower: units(lower) });
        } else if holdings > upper {
            expected.push(Finding::BaseExcess { holdings: units(holdings), upper: units(upper) });
        } else if !claims.is_empty() {
            expected.push(Finding::WithinClaims {
                holdings: units(holdings), lower: units(lower), upper: units(upper),
            });
        }
        if locked > robson {
            expected.push(Finding::LockedForeign { locked: units(locked), robson: units(robson) });
        } else if locked < robson {
            let stops = (101..=id).map(|s| s.to_string()).collect();
            expected.push(Finding::LockedShort { locked: units(locked), robson: units(robson), stops });
        }
        let judged = built.classify(&btc(units(free), units(locked)));
        if expected.is_empty() {
            prop_assert_eq!(judged, Classification::Clean);
        } else {
            prop_assert_eq!(judged, Classification::Discrepancy(expected));
        }
    }

    #[test]
    fn unrepresentable_sums_are_refused(scale in 0u32..=4) {
        let mut built = Built::new();
        built.partitioned(Decimal::MAX, Decimal::ZERO, Decimal::MAX, Decimal::ZERO);
        built.partitioned(Decimal::MAX, Decimal::ZERO, Decimal::MAX, Decimal::ZERO);
        let judged = built.classify(&btc(Decimal::new(1, scale), Decimal::ZERO));
        prop_assert_eq!(judged, Classification::Incomplete(IncompleteReason::Unrepresentable));
    }
}
