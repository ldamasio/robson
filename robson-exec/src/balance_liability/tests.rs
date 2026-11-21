use std::cmp::Ordering;

use chrono::{TimeZone, Utc};
use proptest::prelude::*;
use robson_domain::OrderSide;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use uuid::Uuid;

use super::*;
use crate::{
    venue::{OrderPhase, VenueId},
    venue_ledger::{LegOrderKind, OrderKey, OrderOwner, SeenAt, StoredOrder},
};

fn recorded() -> RecordedPartition {
    RecordedPartition {
        gross: dec!(0.00011),
        base_fee: dec!(0.00000011),
        owned: dec!(0.00010),
        consumed: dec!(0.00003),
        fee_evidence: FeeEvidence::ProvisionalBound,
    }
}

#[test]
fn partition_preserves_inventory_and_explicit_deficit() {
    let input = recorded();
    let facts = partition_facts(input).unwrap();
    assert_eq!(facts.net(), dec!(0.00010989));
    assert_eq!(facts.dust(), dec!(0.00000989));
    assert_eq!(facts.remainder(), Remainder::Quantity(dec!(0.00007)));
    for fee_evidence in [FeeEvidence::ProvisionalBound, FeeEvidence::FinalCommission] {
        let tagged = partition_facts(RecordedPartition { fee_evidence, ..input }).unwrap();
        assert_eq!(tagged.fee_evidence(), fee_evidence);
        assert_eq!(tagged.dust(), facts.dust());
        assert_eq!(tagged.remainder(), facts.remainder());
    }
    for consumed in [dec!(0.000101), dec!(0.00020)] {
        let deficit = partition_facts(RecordedPartition { consumed, ..input }).unwrap();
        assert_eq!(deficit.remainder(), Remainder::Deficit { excess: consumed - input.owned });
        assert_eq!(deficit.dust(), facts.dust());
    }
    for consumed in [Decimal::ZERO, input.owned] {
        let end = partition_facts(RecordedPartition { consumed, ..input }).unwrap();
        assert_eq!(end.remainder(), Remainder::Quantity(input.owned - consumed));
    }
    let no_executable = partition_facts(RecordedPartition {
        owned: Decimal::ZERO,
        consumed: Decimal::ZERO,
        ..input
    })
    .unwrap();
    assert_eq!(no_executable.dust(), facts.net());
    assert_eq!(no_executable.remainder(), Remainder::Quantity(Decimal::ZERO));
}

#[test]
fn invalid_partition_inputs_never_become_balanced_inventory() {
    let input = recorded();
    for bad in [
        RecordedPartition { gross: dec!(-1), ..input },
        RecordedPartition { base_fee: dec!(-1), ..input },
        RecordedPartition { owned: dec!(-1), ..input },
        RecordedPartition { consumed: dec!(-1), ..input },
        RecordedPartition { base_fee: dec!(1), ..input },
        RecordedPartition { owned: input.gross, ..input },
    ] {
        assert_eq!(partition_facts(bad), Err(FactError::InvalidQuantity), "{bad:?}");
    }
}

#[test]
fn exactness_refuses_precision_loss_in_net_and_remainder() {
    let tiny = Decimal::from_i128_with_scale(1, 28);
    let input = RecordedPartition {
        gross: Decimal::MAX,
        base_fee: tiny,
        owned: Decimal::ZERO,
        consumed: Decimal::ZERO,
        fee_evidence: FeeEvidence::FinalCommission,
    };
    assert_eq!(partition_facts(input), Err(FactError::Unrepresentable));
    assert_eq!(
        partition_facts(RecordedPartition {
            base_fee: Decimal::ZERO,
            owned: Decimal::MAX,
            consumed: tiny,
            ..input
        }),
        Err(FactError::Unrepresentable)
    );
    let smallest = partition_facts(RecordedPartition {
        gross: tiny,
        base_fee: Decimal::ZERO,
        owned: tiny,
        consumed: tiny,
        ..input
    })
    .unwrap();
    assert_eq!(smallest.dust(), Decimal::ZERO);
    assert_eq!(smallest.remainder(), Remainder::Quantity(Decimal::ZERO));
}

proptest! {
    // Independent scaled-i128 oracle: inputs are constructed from separate
    // inventory components, not the production subtraction sequence.
    #[test]
    fn scaled_integer_inventory_oracle(
        b in 0i64..1_000_000_000, d in 0i64..1_000_000_000,
        x in 0i64..1_000_000_000, consumed in 0i64..2_000_000_000,
        scale in 0u32..=28, final_fee in any::<bool>(),
    ) {
        let decimal = |n: i128| Decimal::from_i128_with_scale(n, scale);
        let (b,d,x,c) = (i128::from(b),i128::from(d),i128::from(x),i128::from(consumed));
        let input = RecordedPartition {
            gross: decimal(b+d+x), base_fee: decimal(b), owned: decimal(x), consumed: decimal(c),
            fee_evidence: if final_fee { FeeEvidence::FinalCommission } else { FeeEvidence::ProvisionalBound },
        };
        let facts = partition_facts(input).unwrap();
        prop_assert_eq!(facts.net(), decimal(d+x));
        prop_assert_eq!(facts.dust(), decimal(d));
        prop_assert_eq!(facts.fee_evidence(), input.fee_evidence);
        let expected = if c <= x { Remainder::Quantity(decimal(x-c)) }
            else { Remainder::Deficit { excess: decimal(c-x) } };
        prop_assert_eq!(facts.remainder(), expected);
        prop_assert_eq!(input.base_fee + facts.dust() + input.owned, input.gross);
    }
}

fn order(symbol: &str) -> StoredOrder {
    let now = Utc.timestamp_opt(1_700_000_000, 0).single().unwrap();
    StoredOrder {
        key: OrderKey {
            venue: VenueId::new(VenueId::BINANCE_SPOT),
            account_scope: "fixture-account".into(),
            symbol: symbol.into(),
            exchange_order_id: "123".parse().unwrap(),
        },
        owner: OrderOwner::Leg {
            intent_id: Uuid::from_u128(1),
            position_id: Uuid::from_u128(2),
            kind: LegOrderKind::Stop,
        },
        client_order_id: "original-stop".into(),
        last_client_order_id: "cancel-renamed".into(),
        side: OrderSide::Sell,
        order_type: "STOP_LOSS".into(),
        orig_qty: dec!(0.00010),
        stop_price: Some(dec!(65955.60)),
        status: "NEW".into(),
        phase: OrderPhase::Live,
        executed_cum: dec!(0.00003),
        cumulative_quote: None,
        quote_floor: None,
        fills_qty: Decimal::ZERO,
        trades_cursor: -1,
        last_checked_at: now,
        live_seen: SeenAt { at: now, clock: None, capture: None },
        terminal_seen: None,
        expiry_reason: None,
        venue_time_ms: None,
        venue_update_time_ms: None,
        venue_transact_time_ms: None,
        venue_working_time_ms: None,
    }
}

#[test]
fn governed_order_identity_checks_every_key_and_owner_field() {
    for symbol in ["BTCUSDT", "ETHUSDC"] {
        let order = order(symbol);
        let mut key = order.key.clone();
        key.exchange_order_id = "000123".parse().unwrap();
        assert!(order_facts(&order, &key, &order.owner).is_ok());
        for changed in 0..7 {
            let mut key = order.key.clone();
            match changed {
                0 => key.venue = VenueId::new("other:spot"),
                1 => key.account_scope = "other-account".into(),
                2 => key.symbol = "OTHERQUOTE".into(),
                3 => key.exchange_order_id = "124".parse().unwrap(),
                4 => key.venue = VenueId::new(""),
                5 => key.account_scope.clear(),
                _ => key.symbol.clear(),
            }
            assert_eq!(order_facts(&order, &key, &order.owner), Err(FactError::IdentityMismatch));
            if changed >= 4 {
                let mut missing = order.clone();
                missing.key = key.clone();
                assert_eq!(
                    order_facts(&missing, &key, &order.owner),
                    Err(FactError::IdentityMismatch)
                );
            }
        }
        for owner in [
            OrderOwner::Unauthored,
            OrderOwner::Discovered,
            OrderOwner::Leg {
                intent_id: Uuid::from_u128(3),
                position_id: Uuid::from_u128(2),
                kind: LegOrderKind::Stop,
            },
            OrderOwner::Leg {
                intent_id: Uuid::from_u128(1),
                position_id: Uuid::from_u128(3),
                kind: LegOrderKind::Stop,
            },
            OrderOwner::Leg {
                intent_id: Uuid::from_u128(1),
                position_id: Uuid::from_u128(2),
                kind: LegOrderKind::Sell,
            },
        ] {
            assert_eq!(order_facts(&order, &order.key, &owner), Err(FactError::IdentityMismatch));
            let mut unowned = order.clone();
            unowned.owner = owner.clone();
            if !matches!(owner, OrderOwner::Leg { .. }) {
                assert_eq!(
                    order_facts(&unowned, &order.key, &owner),
                    Err(FactError::IdentityMismatch)
                );
            }
        }
        let mut renamed = order.clone();
        renamed.client_order_id = "another-cancel-id".into();
        renamed.last_client_order_id = "new-current-id".into();
        assert_eq!(
            order_facts(&renamed, &order.key, &order.owner),
            order_facts(&order, &order.key, &order.owner)
        );
    }
}

#[test]
fn order_quantities_and_shape_are_exact_and_separate_from_lifecycle() {
    let original = order("BTCUSDT");
    let facts = order_facts(&original, &original.key, &original.owner).unwrap();
    assert_eq!(facts.remaining(), dec!(0.00007));
    for target in [dec!(65955.59), dec!(65955.60)] {
        assert_eq!(facts.matches_protection_shape(dec!(0.00007), target), Ok(true));
    }
    assert_eq!(facts.matches_protection_shape(dec!(0.00007), dec!(65955.61)), Ok(false));
    for quantity in [dec!(0.00006), dec!(0.00008)] {
        assert_eq!(facts.matches_protection_shape(quantity, dec!(65955.60)), Ok(false));
    }
    assert_eq!(
        facts.matches_protection_shape(dec!(-1), dec!(1)),
        Err(FactError::InvalidQuantity)
    );
    for target in [Decimal::ZERO, dec!(-1)] {
        assert_eq!(
            facts.matches_protection_shape(dec!(0.00007), target),
            Err(FactError::InvalidTarget)
        );
    }
    for changed in 0..5 {
        let mut order = original.clone();
        match changed {
            0 => order.side = OrderSide::Buy,
            1 => order.order_type = "STOP_LOSS_LIMIT".into(),
            2 => order.stop_price = None,
            3 => order.stop_price = Some(Decimal::ZERO),
            _ => order.stop_price = Some(dec!(-1)),
        }
        assert_eq!(
            order_facts(&order, &order.key, &order.owner)
                .unwrap()
                .matches_protection_shape(dec!(0.00007), dec!(65955.60)),
            Ok(false)
        );
    }
    for (status, phase) in [
        ("PARTIALLY_FILLED", OrderPhase::Live),
        ("EXPIRED", OrderPhase::Terminal),
        ("PENDING_CANCEL", OrderPhase::Unknown),
        ("PENDING_NEW", OrderPhase::Live),
    ] {
        let mut order = original.clone();
        order.status = status.into();
        order.phase = phase;
        order.expiry_reason = Some("anything".into());
        // Same shape even for terminal/unknown/partial: this API cannot be an
        // R3/lifecycle decision and must never be used as one.
        assert_eq!(order_facts(&order, &order.key, &order.owner).unwrap(), facts);
    }
    for (orig_qty, executed_cum, expected) in [
        (dec!(1), dec!(-1), FactError::InvalidQuantity),
        (dec!(-1), Decimal::ZERO, FactError::InvalidQuantity),
        (dec!(1), dec!(2), FactError::InvalidQuantity),
        (Decimal::MAX, Decimal::from_i128_with_scale(1, 28), FactError::Unrepresentable),
    ] {
        let mut order = original.clone();
        order.orig_qty = orig_qty;
        order.executed_cum = executed_cum;
        assert_eq!(order_facts(&order, &order.key, &order.owner), Err(expected));
    }
}

#[test]
fn transition_strength_preserves_equal_class_and_target_monotonicity() {
    use TransitionKind::*;
    let kinds = [Replace, Reprotect, Flatten, Exit];
    let oracle = [
        [
            Ordering::Equal,
            Ordering::Equal,
            Ordering::Less,
            Ordering::Less,
        ],
        [
            Ordering::Equal,
            Ordering::Equal,
            Ordering::Less,
            Ordering::Less,
        ],
        [
            Ordering::Greater,
            Ordering::Greater,
            Ordering::Equal,
            Ordering::Less,
        ],
        [
            Ordering::Greater,
            Ordering::Greater,
            Ordering::Greater,
            Ordering::Equal,
        ],
    ];
    for (i, left) in kinds.into_iter().enumerate() {
        for (j, right) in kinds.into_iter().enumerate() {
            assert_eq!(left.compare_strength(right), oracle[i][j]);
        }
    }
    for invalid in [Decimal::ZERO, dec!(-1)] {
        assert_eq!(join_target(invalid, dec!(2)), Err(FactError::InvalidTarget));
        assert_eq!(join_target(dec!(2), invalid), Err(FactError::InvalidTarget));
    }
    assert_eq!(join_target(Decimal::MAX, dec!(1)), Ok(Decimal::MAX));
    assert_eq!(join_target(dec!(1), Decimal::MAX), Ok(Decimal::MAX));
}

proptest! {
    #[test]
    fn target_join_is_a_monotone_idempotent_semilattice(a in 1i64..1_000_000, b in 1i64..1_000_000, c in 1i64..1_000_000) {
        let (a,b,c) = (Decimal::new(a,2),Decimal::new(b,2),Decimal::new(c,2));
        let ab = join_target(a,b).unwrap();
        prop_assert!(ab >= a && ab >= b);
        prop_assert!(ab == a || ab == b);
        prop_assert_eq!(ab, join_target(b,a).unwrap());
        prop_assert_eq!(join_target(a,a).unwrap(), a);
        prop_assert_eq!(join_target(ab,c).unwrap(),join_target(a,join_target(b,c).unwrap()).unwrap());
    }
}
