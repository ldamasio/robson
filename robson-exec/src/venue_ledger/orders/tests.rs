use proptest::prelude::*;
use rust_decimal_macros::dec;

use super::*;
use crate::venue_ledger::{ClockEvidence, ClockVerdict};

fn at(second: i64) -> SeenAt {
    SeenAt {
        at: DateTime::from_timestamp(1_794_614_400 + second, 0).unwrap(),
        clock: None,
        capture: None,
    }
}

fn record() -> VenueOrderRecord {
    VenueOrderRecord {
        exchange_order_id: "7".into(),
        client_order_id: "original".into(),
        orig_client_order_id: None,
        symbol: "BTCUSDT".into(),
        side: OrderSide::Sell,
        order_type: "MARKET".into(),
        protection_kind: None,
        status: "PARTIALLY_FILLED".into(),
        phase: OrderPhase::Live,
        orig_qty: dec!(10),
        executed_qty: dec!(2),
        cumulative_quote_qty: Some(dec!(200)),
        stop_price: None,
        expiry_reason: None,
        transact_time_ms: None,
        time_ms: None,
        update_time_ms: None,
        working_time_ms: None,
    }
}

fn link(owner: OrderOwner) -> OrderLink {
    OrderLink {
        venue: VenueId::new(VenueId::BINANCE_SPOT),
        account_scope: "fixture-scope".into(),
        owner,
        client_order_id: "original".into(),
    }
}

fn leg() -> OrderLeg {
    OrderLeg {
        position_id: Uuid::now_v7(),
        venue: VenueId::new(VenueId::BINANCE_SPOT),
        account_scope: "fixture-scope".into(),
        symbol: "BTCUSDT".into(),
        admission_intent_id: Uuid::now_v7(),
        decided_month: NaiveDate::from_ymd_opt(2026, 11, 1).unwrap(),
        cohort_month: None,
        consumed_cum: Decimal::ZERO,
        closed_at: None,
    }
}

fn owned(leg: &OrderLeg, kind: LegOrderKind) -> OrderLink {
    link(OrderOwner::Leg {
        intent_id: if kind == LegOrderKind::Entry {
            leg.admission_intent_id
        } else {
            Uuid::now_v7()
        },
        position_id: leg.position_id,
        kind,
    })
}

fn unowned(record: &VenueOrderRecord) -> StoredOrder {
    merge_order(None, record, &link(OrderOwner::Discovered), &at(1), None)
        .unwrap()
        .order
}

#[test]
fn non_trigger_zero_stop_price_is_the_omitted_placeholder() {
    for order_type in ["MARKET", "LIMIT", "LIMIT_MAKER"] {
        for phase in [OrderPhase::Live, OrderPhase::Terminal] {
            for zero_first in [false, true] {
                let mut a = record();
                a.order_type = order_type.into();
                a.phase = phase;
                if phase == OrderPhase::Terminal {
                    a.status = "CANCELED".into();
                }
                a.stop_price = zero_first.then_some(Decimal::ZERO);
                let first = unowned(&a);
                assert_eq!(first.stop_price, None);
                a.stop_price = (!zero_first).then_some(Decimal::ZERO);
                let replay =
                    merge_order(Some(&first), &a, &link(OrderOwner::Discovered), &at(99), None)
                        .unwrap();
                assert!(!replay.changed);
                assert_eq!(replay.order, first);
                a.stop_price = Some(dec!(90));
                assert_eq!(
                    merge_order(Some(&first), &a, &link(OrderOwner::Discovered), &at(99), None),
                    Err(LedgerError::IdentityConflict)
                );
            }
        }
    }
    for order_type in ["STOP_LOSS", "STOP_LOSS_LIMIT", "UNKNOWN_TYPE"] {
        let mut a = record();
        a.order_type = order_type.into();
        a.stop_price = Some(Decimal::ZERO);
        assert!(matches!(
            merge_order(None, &a, &link(OrderOwner::Discovered), &at(1), None),
            Err(LedgerError::InvalidEvidence(_))
        ));
    }
    let mut a = record();
    a.protection_kind = Some(ProtectionKind::MarketOnTrigger);
    a.stop_price = Some(Decimal::ZERO);
    assert!(matches!(
        merge_order(None, &a, &link(OrderOwner::Discovered), &at(1), None),
        Err(LedgerError::InvalidEvidence(_))
    ));
}

#[test]
fn governed_kinds_and_the_current_entry_admission_are_required() {
    let leg = leg();
    let mut a = record();
    for stop in [None, Some(Decimal::ZERO), Some(dec!(-1))] {
        a.order_type = "STOP_LOSS".into();
        a.protection_kind = Some(ProtectionKind::MarketOnTrigger);
        a.stop_price = stop;
        assert!(
            merge_order(None, &a, &owned(&leg, LegOrderKind::Stop), &at(1), Some(&leg)).is_err()
        );
    }
    // Synthetic chart fixture: 90 is the second support identified by analysis.
    a.stop_price = Some(dec!(90));
    assert!(merge_order(None, &a, &owned(&leg, LegOrderKind::Stop), &at(1), Some(&leg)).is_ok());
    a.protection_kind = None;
    assert_eq!(
        merge_order(None, &a, &owned(&leg, LegOrderKind::Stop), &at(1), Some(&leg)),
        Err(LedgerError::IdentityConflict)
    );
    a = record();
    a.side = OrderSide::Buy;
    let entry = owned(&leg, LegOrderKind::Entry);
    let merged = merge_order(None, &a, &entry, &at(1), Some(&leg)).unwrap();
    assert!(merged.set_cohort);
    assert_eq!(merged.consumed_delta, Decimal::ZERO);
    let mut wrong = leg.clone();
    wrong.admission_intent_id = Uuid::now_v7();
    assert_eq!(
        merge_order(None, &a, &entry, &at(1), Some(&wrong)),
        Err(LedgerError::IdentityConflict)
    );
    for (side, order_type, kind) in [
        (OrderSide::Sell, "MARKET", LegOrderKind::Entry),
        (OrderSide::Buy, "MARKET", LegOrderKind::Sell),
        (OrderSide::Sell, "LIMIT", LegOrderKind::Sell),
        (OrderSide::Sell, "STOP_LOSS_LIMIT", LegOrderKind::Stop),
    ] {
        a.side = side;
        a.order_type = order_type.into();
        assert_eq!(
            merge_order(None, &a, &owned(&leg, kind), &at(1), Some(&leg)),
            Err(LedgerError::IdentityConflict)
        );
    }
}

#[test]
fn every_identity_component_and_leg_link_is_kept() {
    let a = record();
    let stored = unowned(&a);
    for field in 0..7 {
        let mut bad = a.clone();
        match field {
            0 => bad.exchange_order_id = "8".into(),
            1 => bad.symbol = "ETHUSDT".into(),
            2 => bad.side = OrderSide::Buy,
            3 => bad.order_type = "LIMIT".into(),
            4 => bad.orig_qty = dec!(11),
            5 => bad.stop_price = Some(dec!(90)),
            _ => {
                bad.exchange_order_id = "-1".into();
            },
        }
        assert!(
            merge_order(Some(&stored), &bad, &link(OrderOwner::Discovered), &at(2), None).is_err()
        );
    }
    for scope in [true, false] {
        let mut bad = link(OrderOwner::Discovered);
        if scope {
            bad.account_scope = "other".into();
        } else {
            bad.venue = VenueId::new("other");
        }
        assert_eq!(
            merge_order(Some(&stored), &a, &bad, &at(2), None),
            Err(LedgerError::IdentityConflict)
        );
    }
    assert_eq!(
        merge_order(Some(&stored), &a, &link(OrderOwner::Unauthored), &at(2), None),
        Err(LedgerError::IdentityConflict)
    );
    let mut bad = link(OrderOwner::Discovered);
    bad.client_order_id = "missing".into();
    assert_eq!(merge_order(None, &a, &bad, &at(1), None), Err(LedgerError::IdentityConflict));
    let mut renamed = a.clone();
    renamed.orig_client_order_id = Some("original".into());
    renamed.client_order_id = "cancel-id".into();
    let merged = merge_order(None, &renamed, &link(OrderOwner::Discovered), &at(1), None).unwrap();
    assert_eq!(merged.order.client_order_id, "original");
    assert_eq!(merged.order.last_client_order_id, "cancel-id");
    let leg = leg();
    let owner = owned(&leg, LegOrderKind::Sell);
    let owned_row = merge_order(Some(&stored), &a, &owner, &at(2), Some(&leg)).unwrap().order;
    let another = owned(&leg, LegOrderKind::Sell);
    assert_eq!(
        merge_order(Some(&owned_row), &a, &another, &at(3), Some(&leg)),
        Err(LedgerError::IdentityConflict)
    );
    assert_eq!(
        merge_order(Some(&owned_row), &a, &link(OrderOwner::Discovered), &at(3), Some(&leg))
            .unwrap()
            .order
            .owner,
        owner.owner
    );
    let mut canonical = a.clone();
    canonical.exchange_order_id = "0007".into();
    assert!(
        !merge_order(Some(&stored), &canonical, &link(OrderOwner::Discovered), &at(3), None)
            .unwrap()
            .changed
    );
}

#[test]
fn adoption_counts_the_whole_execution_and_requires_an_open_matching_leg() {
    let a = record();
    let mut stored = unowned(&a);
    // Trades raised an unowned row before an operator-authorized adoption.
    stored.executed_cum = dec!(3);
    stored.fills_qty = dec!(3);
    let mut leg = leg();
    let owner = owned(&leg, LegOrderKind::Sell);
    let first = merge_order(Some(&stored), &a, &owner, &at(2), Some(&leg)).unwrap();
    assert_eq!(first.consumed_delta, dec!(3));
    assert!(first.anomalies.contains(&OrderAnomaly::ExecutionRegressed));
    assert_eq!(first.order.fills_qty, dec!(3));
    leg.consumed_cum = dec!(3);
    let replay = merge_order(Some(&first.order), &a, &owner, &at(9), Some(&leg)).unwrap();
    assert!(!replay.changed);
    assert_eq!(replay.consumed_delta, Decimal::ZERO);
    leg.closed_at = Some(at(3).at);
    assert_eq!(
        merge_order(Some(&stored), &a, &owner, &at(4), Some(&leg)),
        Err(LedgerError::IdentityConflict)
    );
    leg.closed_at = None;
    leg.symbol = "ETHUSDT".into();
    assert_eq!(
        merge_order(Some(&stored), &a, &owner, &at(4), Some(&leg)),
        Err(LedgerError::IdentityConflict)
    );
    leg.symbol = "BTCUSDT".into();
    let mut buy = a.clone();
    buy.side = OrderSide::Buy;
    let discovered_buy = unowned(&buy);
    assert_eq!(
        merge_order(
            Some(&discovered_buy),
            &buy,
            &owned(&leg, LegOrderKind::Entry),
            &at(4),
            Some(&leg)
        ),
        Err(LedgerError::IdentityConflict)
    );
}

#[test]
fn adoption_requires_the_venue_or_first_proven_client_lineage() {
    let a = record();
    let stored = unowned(&a);
    let leg = leg();
    let mut owner = owned(&leg, LegOrderKind::Sell);
    owner.client_order_id = "unrelated".into();
    assert_eq!(
        merge_order(Some(&stored), &a, &owner, &at(2), Some(&leg)),
        Err(LedgerError::IdentityConflict)
    );
    // Current, original and first-proven IDs each remain accepted lineage.
    let mut renamed = a.clone();
    renamed.client_order_id = "unrelated".into();
    assert_eq!(
        merge_order(Some(&stored), &renamed, &owner, &at(2), Some(&leg))
            .unwrap()
            .consumed_delta,
        dec!(2)
    );
    renamed.client_order_id = "cancel-id".into();
    renamed.orig_client_order_id = Some("unrelated".into());
    assert!(merge_order(Some(&stored), &renamed, &owner, &at(2), Some(&leg)).is_ok());
    owner.client_order_id = "original".into();
    renamed.orig_client_order_id = None;
    assert!(merge_order(Some(&stored), &renamed, &owner, &at(2), Some(&leg)).is_ok());
}

#[test]
fn a_known_quote_is_never_replaced_or_regressed() {
    let a = record();
    let stored = unowned(&a);
    for (executed, quote) in [
        (dec!(2), dec!(150)),
        (dec!(2), dec!(201)),
        (dec!(3), dec!(150)),
    ] {
        let mut x = a.clone();
        x.executed_qty = executed;
        x.cumulative_quote_qty = Some(quote);
        assert_eq!(
            merge_order(Some(&stored), &x, &link(OrderOwner::Discovered), &at(2), None),
            Err(LedgerError::TradeConflict)
        );
        x.phase = OrderPhase::Terminal;
        x.status = "CANCELED".into();
        assert_eq!(
            merge_order(Some(&stored), &x, &link(OrderOwner::Discovered), &at(2), None),
            Err(LedgerError::TradeConflict)
        );
    }
    let mut x = a.clone();
    x.executed_qty = dec!(3);
    x.cumulative_quote_qty = Some(dec!(200));
    assert_eq!(
        merge_order(Some(&stored), &x, &link(OrderOwner::Discovered), &at(2), None)
            .unwrap()
            .order
            .cumulative_quote,
        Some(dec!(200))
    );
    x.cumulative_quote_qty = Some(dec!(300));
    assert_eq!(
        merge_order(Some(&stored), &x, &link(OrderOwner::Discovered), &at(2), None)
            .unwrap()
            .order
            .cumulative_quote,
        Some(dec!(300))
    );
}

#[test]
fn terminal_facts_survive_stale_live_audit_and_adoption() {
    let mut a = record();
    a.phase = OrderPhase::Terminal;
    a.status = "CANCELED".into();
    a.expiry_reason = Some("original-expiry".into());
    let stored = unowned(&a);
    let leg = leg();
    let owner = owned(&leg, LegOrderKind::Sell);
    let mut later = a.clone();
    later.phase = OrderPhase::Live;
    later.status = "PARTIALLY_FILLED".into();
    later.executed_qty = dec!(9);
    later.cumulative_quote_qty = Some(dec!(999));
    later.expiry_reason = Some("other-expiry".into());
    later.client_order_id = "renamed".into();
    later.time_ms = Some(99);
    later.update_time_ms = Some(100);
    let merged = merge_order(Some(&stored), &later, &owner, &at(9), Some(&leg)).unwrap();
    assert_eq!(merged.anomalies, [OrderAnomaly::TerminalReadAsLive]);
    assert_eq!(merged.consumed_delta, dec!(2));
    assert_eq!(merged.order.executed_cum, dec!(2));
    assert_eq!(merged.order.status, "CANCELED");
    assert_eq!(merged.order.cumulative_quote, Some(dec!(200)));
    assert_eq!(merged.order.expiry_reason, stored.expiry_reason);
    assert_eq!(merged.order.terminal_seen, stored.terminal_seen);
    assert_eq!(merged.order.live_seen, stored.live_seen);
    assert_eq!(merged.order.last_client_order_id, "renamed");
    assert_eq!(merged.order.venue_time_ms, Some(99));
    assert_eq!(merged.order.last_checked_at, at(9).at);
    let replay = merge_order(Some(&merged.order), &later, &owner, &at(10), Some(&leg)).unwrap();
    assert!(!replay.changed);
    later = a.clone();
    later.status = "FILLED".into();
    assert_eq!(
        merge_order(Some(&stored), &later, &link(OrderOwner::Discovered), &at(9), None),
        Err(LedgerError::IdentityConflict)
    );
    later = a.clone();
    later.executed_qty = dec!(3);
    assert_eq!(
        merge_order(Some(&stored), &later, &link(OrderOwner::Discovered), &at(9), None),
        Err(LedgerError::TradeConflict)
    );
    later = a.clone();
    later.cumulative_quote_qty = Some(dec!(201));
    assert_eq!(
        merge_order(Some(&stored), &later, &link(OrderOwner::Discovered), &at(9), None),
        Err(LedgerError::TradeConflict)
    );
    a.cumulative_quote_qty = None;
    let unknown = unowned(&a);
    a.cumulative_quote_qty = Some(dec!(200));
    assert_eq!(
        merge_order(Some(&unknown), &a, &link(OrderOwner::Discovered), &at(9), None)
            .unwrap()
            .order
            .cumulative_quote,
        None
    );
}

#[test]
fn lower_live_values_are_anomalies_but_lower_terminal_values_conflict() {
    let mut a = record();
    let stored = unowned(&a);
    a.executed_qty = dec!(1);
    a.cumulative_quote_qty = Some(dec!(100));
    a.status = "NEW".into();
    let same = merge_order(Some(&stored), &a, &link(OrderOwner::Discovered), &at(2), None).unwrap();
    assert!(!same.changed);
    assert_eq!(same.order, stored);
    assert_eq!(same.anomalies, [OrderAnomaly::ExecutionRegressed]);
    a.update_time_ms = Some(77);
    let audit =
        merge_order(Some(&stored), &a, &link(OrderOwner::Discovered), &at(2), None).unwrap();
    assert!(audit.changed);
    assert_eq!(audit.order.executed_cum, stored.executed_cum);
    assert_eq!(audit.order.venue_update_time_ms, Some(77));
    a.phase = OrderPhase::Terminal;
    a.status = "CANCELED".into();
    assert_eq!(
        merge_order(Some(&stored), &a, &link(OrderOwner::Discovered), &at(3), None),
        Err(LedgerError::TradeConflict)
    );
}

#[test]
fn first_live_and_terminal_clock_evidence_is_preserved_and_null_audit_does_not_erase() {
    let mut a = record();
    a.time_ms = Some(10);
    a.transact_time_ms = Some(11);
    a.update_time_ms = Some(12);
    a.working_time_ms = Some(13);
    let mut live = at(1);
    live.clock = Some(ClockEvidence {
        sample_id: Uuid::now_v7(),
        wall_delta_ms: 4,
        mono_delta_ms: 5,
        verdict: ClockVerdict::Invalid,
    });
    let first = merge_order(None, &a, &link(OrderOwner::Discovered), &live, None).unwrap().order;
    let mut terminal = at(2);
    terminal.clock = Some(ClockEvidence {
        sample_id: Uuid::now_v7(),
        wall_delta_ms: 6,
        mono_delta_ms: 7,
        verdict: ClockVerdict::Valid,
    });
    a.phase = OrderPhase::Terminal;
    a.status = "CANCELED".into();
    a.time_ms = None;
    a.transact_time_ms = None;
    a.update_time_ms = None;
    a.working_time_ms = None;
    let done = merge_order(Some(&first), &a, &link(OrderOwner::Discovered), &terminal, None)
        .unwrap()
        .order;
    assert_eq!(done.live_seen, live);
    assert_eq!(done.terminal_seen, Some(terminal));
    assert_eq!(
        (
            done.venue_time_ms,
            done.venue_transact_time_ms,
            done.venue_update_time_ms,
            done.venue_working_time_ms
        ),
        (Some(10), Some(11), Some(12), Some(13))
    );
    let replay =
        merge_order(Some(&done), &a, &link(OrderOwner::Discovered), &at(99), None).unwrap();
    assert!(!replay.changed);
    assert_eq!(replay.order, done);
}

#[test]
fn expired_zero_entry_is_terminal_settled_and_sets_only_its_cohort() {
    let leg = leg();
    let mut a = record();
    a.side = OrderSide::Buy;
    a.phase = OrderPhase::Terminal;
    a.status = "EXPIRED".into();
    a.executed_qty = Decimal::ZERO;
    a.cumulative_quote_qty = Some(Decimal::ZERO);
    let merged =
        merge_order(None, &a, &owned(&leg, LegOrderKind::Entry), &at(1), Some(&leg)).unwrap();
    assert!(merged.order.terminal());
    assert!(merged.order.fills_complete());
    assert!(merged.set_cohort);
    assert_eq!(merged.consumed_delta, Decimal::ZERO);
}

#[test]
fn invalid_quantities_and_unrepresentable_contributions_are_refused() {
    for case in 0..4 {
        let mut a = record();
        match case {
            0 => a.orig_qty = Decimal::ZERO,
            1 => a.executed_qty = dec!(-1),
            2 => a.executed_qty = dec!(11),
            _ => a.cumulative_quote_qty = Some(dec!(-1)),
        }
        assert!(matches!(
            merge_order(None, &a, &link(OrderOwner::Discovered), &at(1), None),
            Err(LedgerError::InvalidEvidence(_))
        ));
    }
    let mut leg = leg();
    leg.consumed_cum = Decimal::MAX;
    assert_eq!(
        merge_order(None, &record(), &owned(&leg, LegOrderKind::Sell), &at(1), Some(&leg)),
        Err(LedgerError::Unrepresentable)
    );
    let mut a = record();
    a.orig_qty = Decimal::MAX;
    a.executed_qty = dec!(0.0000000000000000000000000001);
    a.cumulative_quote_qty = None;
    leg.consumed_cum = Decimal::ZERO;
    let owner = owned(&leg, LegOrderKind::Sell);
    let stored = merge_order(None, &a, &owner, &at(1), Some(&leg)).unwrap().order;
    a.executed_qty = Decimal::MAX;
    assert_eq!(
        merge_order(Some(&stored), &a, &owner, &at(2), Some(&leg)),
        Err(LedgerError::Unrepresentable)
    );
}

proptest! {
    #[test]
    fn merge_is_monotone_idempotent_and_conserves_contribution(mut events in proptest::collection::vec((any::<u16>(),0u8..=10,any::<bool>(),any::<bool>()),1..40)) {
        events.sort_by_key(|event|event.0);
        let mut leg=leg();let owner=owned(&leg,LegOrderKind::Sell);let discovered=link(OrderOwner::Discovered);
        let mut stored:Option<StoredOrder>=None;let mut adopted=false;let mut total=Decimal::ZERO;
        for (index,(_,quantity,terminal,adopt)) in events.iter().enumerate() {
            adopted |= *adopt;
            let mut record=record();record.executed_qty=Decimal::from(*quantity);record.cumulative_quote_qty=Some(Decimal::from(u32::from(*quantity)*100));
            record.phase=if *terminal {OrderPhase::Terminal}else{OrderPhase::Live};record.status=if *terminal {"CANCELED"}else{"PARTIALLY_FILLED"}.into();
            let link=if adopted {&owner}else{&discovered};let seen=at(index as i64+1);
            match merge_order(stored.as_ref(),&record,link,&seen,Some(&leg)) {
                Ok(merged)=>{
                    if let Some(old)=&stored {
                        prop_assert!(merged.order.executed_cum>=old.executed_cum);
                        prop_assert_eq!(&merged.order.live_seen,&old.live_seen);
                        if old.terminal() {
                            prop_assert!(merged.order.terminal());
                            prop_assert_eq!((&merged.order.status,merged.order.executed_cum,merged.order.cumulative_quote,&merged.order.terminal_seen),(&old.status,old.executed_cum,old.cumulative_quote,&old.terminal_seen));
                        }
                    }
                    prop_assert!(merged.consumed_delta>=Decimal::ZERO);
                    total=exact_add(total,merged.consumed_delta).unwrap();leg.consumed_cum=total;
                    prop_assert_eq!(total,contribution(&merged.order));
                    let duplicate=merge_order(Some(&merged.order),&record,link,&at(index as i64+100),Some(&leg)).unwrap();
                    prop_assert!(!duplicate.changed);prop_assert_eq!(duplicate.consumed_delta,Decimal::ZERO);prop_assert_eq!(&duplicate.order,&merged.order);
                    stored=Some(merged.order);
                },
                Err(error)=>prop_assert!(matches!(error,LedgerError::TradeConflict|LedgerError::IdentityConflict)),
            }
        }
        prop_assert_eq!(total,stored.as_ref().map_or(Decimal::ZERO,contribution));
    }
}

#[test]
fn the_quote_floor_survives_an_unknown_total_as_execution_grows() {
    let mut incoming = record();
    let known = unowned(&incoming);
    assert_eq!(known.quote_floor, Some(dec!(200)));
    incoming.executed_qty = dec!(3);
    incoming.cumulative_quote_qty = None;
    let unknown = merge_order(Some(&known), &incoming, &link(OrderOwner::Discovered), &at(2), None)
        .unwrap()
        .order;
    assert_eq!((unknown.cumulative_quote, unknown.quote_floor), (None, Some(dec!(200))));
    incoming.cumulative_quote_qty = Some(dec!(150));
    for phase in [OrderPhase::Live, OrderPhase::Terminal] {
        incoming.phase = phase;
        incoming.status = if phase == OrderPhase::Terminal {
            "CANCELED"
        } else {
            "PARTIALLY_FILLED"
        }
        .into();
        assert_eq!(
            merge_order(Some(&unknown), &incoming, &link(OrderOwner::Discovered), &at(3), None),
            Err(LedgerError::TradeConflict)
        );
    }
    incoming.phase = OrderPhase::Live;
    incoming.status = "PARTIALLY_FILLED".into();
    incoming.cumulative_quote_qty = Some(dec!(300));
    let exact = merge_order(Some(&unknown), &incoming, &link(OrderOwner::Discovered), &at(4), None)
        .unwrap()
        .order;
    assert_eq!((exact.cumulative_quote, exact.quote_floor), (Some(dec!(300)), Some(dec!(300))));
    // A pre-floor row with no exact evidence stays unknown; one with an exact
    // total uses it as the floor without a change on replay.
    let mut old = known.clone();
    old.quote_floor = None;
    incoming.executed_qty = dec!(2);
    incoming.cumulative_quote_qty = Some(dec!(200));
    assert!(
        !merge_order(Some(&old), &incoming, &link(OrderOwner::Discovered), &at(5), None)
            .unwrap()
            .changed
    );
    incoming.executed_qty = dec!(3);
    incoming.cumulative_quote_qty = Some(dec!(150));
    assert_eq!(
        merge_order(Some(&old), &incoming, &link(OrderOwner::Discovered), &at(5), None),
        Err(LedgerError::TradeConflict)
    );
    old.cumulative_quote = None;
    let blind = merge_order(Some(&old), &incoming, &link(OrderOwner::Discovered), &at(6), None)
        .unwrap()
        .order;
    assert_eq!(blind.quote_floor, Some(dec!(150)));
}

#[test]
fn a_quote_subtotal_never_becomes_an_unknown_larger_execution_total() {
    for terminal_first in [false, true] {
        let mut incoming = record();
        let initial = unowned(&incoming);
        // Missing quote at unchanged execution still has the same known total.
        incoming.cumulative_quote_qty = None;
        let unchanged =
            merge_order(Some(&initial), &incoming, &link(OrderOwner::Discovered), &at(2), None)
                .unwrap();
        assert!(!unchanged.changed);
        assert_eq!(unchanged.order.cumulative_quote, Some(dec!(200)));
        incoming.executed_qty = dec!(3);
        if terminal_first {
            incoming.phase = OrderPhase::Terminal;
            incoming.status = "CANCELED".into();
        }
        let advanced =
            merge_order(Some(&initial), &incoming, &link(OrderOwner::Discovered), &at(3), None)
                .unwrap();
        assert!(advanced.changed);
        assert_eq!(advanced.order.cumulative_quote, None);
        incoming.phase = OrderPhase::Terminal;
        incoming.status = "CANCELED".into();
        let terminal = merge_order(
            Some(&advanced.order),
            &incoming,
            &link(OrderOwner::Discovered),
            &at(4),
            None,
        )
        .unwrap()
        .order;
        assert_eq!(terminal.cumulative_quote, None);
        incoming.cumulative_quote_qty = Some(dec!(300));
        let replay =
            merge_order(Some(&terminal), &incoming, &link(OrderOwner::Discovered), &at(5), None)
                .unwrap();
        assert!(!replay.changed);
        assert_eq!(replay.order, terminal);
        let mut leg = leg();
        let owner = owned(&leg, LegOrderKind::Sell);
        let adopted = merge_order(Some(&terminal), &incoming, &owner, &at(6), Some(&leg)).unwrap();
        assert_eq!(adopted.consumed_delta, dec!(3));
        assert_eq!(adopted.order.cumulative_quote, None);
        leg.consumed_cum = dec!(3);
        assert!(
            !merge_order(Some(&adopted.order), &incoming, &owner, &at(7), Some(&leg))
                .unwrap()
                .changed
        );
    }
}

#[test]
fn original_exact_capture_and_legacy_absence_never_get_enriched() {
    use crate::{clock_records::RawClockCapture, venue_clock::ClockDomain};
    let mut observed = at(1);
    let raw = RawClockCapture {
        epoch: Uuid::from_u128(2),
        wall_seconds: 1,
        wall_nanoseconds: 7,
        domain: ClockDomain::new(Uuid::from_u128(3), -2, 4).unwrap(),
        before_ns: 5,
        after_ns: 9,
    };
    observed.capture = Some(raw);
    let r = record();
    let l = link(OrderOwner::Discovered);
    let first = merge_order(None, &r, &l, &observed, None).unwrap().order;
    let mut later = at(2);
    later.capture = Some(RawClockCapture {
        wall_nanoseconds: 8,
        ..raw
    });
    let replay = merge_order(Some(&first), &r, &l, &later, None).unwrap();
    assert!(!replay.changed);
    assert_eq!(replay.order.live_seen.capture, Some(raw));
    let legacy = merge_order(None, &r, &l, &at(0), None).unwrap().order;
    let replay = merge_order(Some(&legacy), &r, &l, &later, None).unwrap();
    assert!(!replay.changed);
    assert_eq!(replay.order.live_seen.capture, None);
    let mut terminal = r.clone();
    terminal.phase = OrderPhase::Terminal;
    terminal.status = "CANCELED".into();
    let first_terminal = merge_order(Some(&first), &terminal, &l, &observed, None)
        .unwrap()
        .order;
    let replay = merge_order(Some(&first_terminal), &terminal, &l, &later, None).unwrap();
    assert!(!replay.changed);
    assert_eq!(replay.order.terminal_seen.unwrap().capture, Some(raw));
}
