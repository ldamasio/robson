//! EP-1: the one-time first set of a leg's entry partition at its terminal
//! entry. Amounts are fixture exchange metadata, never policy.
use super::*;

fn entry(executed: Decimal, quote: Option<Decimal>, terminal: bool) -> VenueOrderRecord {
    VenueOrderRecord {
        side: OrderSide::Buy,
        orig_qty: dec!(1),
        executed_qty: executed,
        cumulative_quote_qty: quote,
        phase: if terminal {
            OrderPhase::Terminal
        } else {
            OrderPhase::Live
        },
        status: if terminal {
            "EXPIRED"
        } else {
            "PARTIALLY_FILLED"
        }
        .into(),
        ..record()
    }
}

fn expected(gross: Decimal, cost: Decimal, base_fee: Decimal, executable: Decimal) -> LegPartition {
    LegPartition {
        gross,
        cost,
        base_fee,
        base_fee_final: false,
        executable,
    }
}

#[test]
fn taker_bps_become_an_exact_fraction_by_scale_shift() {
    for (bps, fraction) in [
        (dec!(10), dec!(0.001)),
        (dec!(7.5), dec!(0.00075)),
        (dec!(0), dec!(0)),
        (dec!(10000), dec!(1)),
        (dec!(9999.99), dec!(0.999999)),
    ] {
        let shifted = taker_fraction(bps).unwrap();
        assert_eq!(shifted, fraction);
        assert_eq!((shifted.mantissa(), shifted.scale()), (bps.mantissa(), bps.scale() + 4));
    }
    let deep = Decimal::from_i128_with_scale(1, 25);
    for refused in [
        dec!(-0.0001),
        dec!(-10),
        dec!(10000.0001),
        dec!(10001),
        deep,
    ] {
        assert_eq!(taker_fraction(refused), None, "{refused}");
    }
}

#[test]
fn a_terminal_entry_first_sets_its_partition_and_cohort_together() {
    let leg = leg();
    let link = owned(&leg, LegOrderKind::Entry);
    let terminal = entry(dec!(0.30000000), Some(dec!(30000.00)), true);
    let merged = merge_order(None, &terminal, &link, &at(1), Some(&leg)).unwrap();
    assert!(merged.changed && merged.set_cohort);
    assert_eq!(merged.consumed_delta, Decimal::ZERO);
    // B̂ = 0.3 × 10/10⁴ = 0.0003; X = floor_step(0.2997, 0.00001).
    let set = expected(dec!(0.3), dec!(30000), dec!(0.0003), dec!(0.2997));
    assert_eq!(merged.set_partition, Some(set));

    let mut recorded = leg.clone();
    recorded.cohort_month = Some(recorded.decided_month);
    recorded.partition = Some(set);
    let replay = merge_order(Some(&merged.order), &terminal, &link, &at(9), Some(&recorded));
    let replay = replay.unwrap();
    assert!(!replay.changed && !replay.set_cohort);
    assert_eq!(replay.set_partition, None);
    // A stale live read of a recorded terminal entry keeps the terminal tuple.
    let stale = entry(dec!(0.1), Some(dec!(10000)), false);
    let stale = merge_order(Some(&merged.order), &stale, &link, &at(10), Some(&recorded));
    assert_eq!(stale.unwrap().set_partition, None);
}

#[test]
fn a_live_entry_sets_only_the_cohort_until_its_terminal_record() {
    let mut leg = leg();
    let link = owned(&leg, LegOrderKind::Entry);
    let live = entry(dec!(0.1), Some(dec!(10000)), false);
    let first = merge_order(None, &live, &link, &at(1), Some(&leg)).unwrap();
    assert!(first.changed && first.set_cohort);
    assert_eq!(first.set_partition, None);
    leg.cohort_month = Some(leg.decided_month);
    let terminal = entry(dec!(0.25), Some(dec!(25000)), true);
    let done = merge_order(Some(&first.order), &terminal, &link, &at(2), Some(&leg)).unwrap();
    assert!(done.changed && !done.set_cohort);
    assert_eq!(
        done.set_partition,
        Some(expected(dec!(0.25), dec!(25000), dec!(0.00025), dec!(0.24975)))
    );
    // A terminal entry stored before any partition writer sets it on replay.
    let replay = merge_order(Some(&done.order), &terminal, &link, &at(3), Some(&leg)).unwrap();
    assert!(replay.changed);
    assert_eq!(replay.set_partition, done.set_partition);
}

#[test]
fn a_recorded_partition_must_equal_the_terminal_entry() {
    let mut leg = leg();
    let link = owned(&leg, LegOrderKind::Entry);
    leg.cohort_month = Some(leg.decided_month);
    leg.partition = Some(expected(dec!(0.2), dec!(20000), dec!(0.0002), dec!(0.1998)));
    for record in [
        entry(dec!(0.3), Some(dec!(30000)), true),
        entry(dec!(0.2), Some(dec!(20001)), true),
        entry(dec!(0.2), None, true),
        entry(dec!(0.2), Some(dec!(20000)), false),
    ] {
        assert_eq!(
            merge_order(None, &record, &link, &at(1), Some(&leg)),
            Err(LedgerError::IdentityConflict)
        );
    }
    let same = entry(dec!(0.2), Some(dec!(20000.00)), true);
    let merged = merge_order(None, &same, &link, &at(1), Some(&leg)).unwrap();
    assert!(merged.changed);
    assert_eq!(merged.set_partition, None);
    // Existing order checks keep their errors before the partition check.
    let regressed = entry(dec!(0.1), Some(dec!(10000)), true);
    assert_eq!(
        merge_order(Some(&merged.order), &regressed, &link, &at(2), Some(&leg)),
        Err(LedgerError::TradeConflict)
    );
}

#[test]
fn no_partition_without_execution_quote_or_current_terms() {
    let leg = leg();
    let link = owned(&leg, LegOrderKind::Entry);
    for record in [
        entry(dec!(0.1), Some(dec!(10000)), false),
        entry(Decimal::ZERO, Some(Decimal::ZERO), true),
        entry(dec!(0.1), None, true),
    ] {
        let merged = merge_order(None, &record, &link, &at(1), Some(&leg)).unwrap();
        assert!(merged.set_cohort);
        assert_eq!(merged.set_partition, None);
    }
    let terminal = entry(dec!(0.1), Some(dec!(10000)), true);
    for terms in [
        None,
        Some(AdmissionTerms {
            taker_bps: dec!(10001),
            lot_step: dec!(0.00001),
        }),
        Some(AdmissionTerms {
            taker_bps: dec!(10),
            lot_step: Decimal::ZERO,
        }),
    ] {
        let refused = OrderLeg { terms, ..leg.clone() };
        assert!(matches!(
            merge_order(None, &terminal, &link, &at(1), Some(&refused)),
            Err(LedgerError::Invariant(_))
        ));
    }
    // B̂ = Q × f_L beyond 28 decimal places is refused, never rounded.
    let deep = entry(Decimal::from_i128_with_scale(11, 28), Some(dec!(1)), true);
    assert_eq!(
        merge_order(None, &deep, &link, &at(1), Some(&leg)),
        Err(LedgerError::Unrepresentable)
    );
    for kind in [LegOrderKind::Stop, LegOrderKind::Sell] {
        let mut sell = entry(dec!(0.1), Some(dec!(10000)), true);
        sell.side = OrderSide::Sell;
        if kind == LegOrderKind::Stop {
            sell.order_type = "STOP_LOSS".into();
            sell.stop_price = Some(dec!(90));
            sell.protection_kind = Some(ProtectionKind::MarketOnTrigger);
        }
        let merged = merge_order(None, &sell, &owned(&leg, kind), &at(1), Some(&leg)).unwrap();
        assert!(!merged.set_cohort);
        assert_eq!(merged.set_partition, None);
    }
}

fn scaled(units: u128, scale: u32) -> Decimal {
    Decimal::from_i128_with_scale(i128::try_from(units).unwrap(), scale)
}

proptest! {
    #[test]
    fn the_candidate_equals_an_independent_rational_oracle(
        q_units in 1u64..=1_000_000_000_000,
        q_scale in 0u32..=8,
        bps_scale in 0u32..=2,
        bps_seed in 0u32..=1_000_000,
        step_units in 1u64..=1_000,
        step_scale in 0u32..=8,
        venue_format: bool,
    ) {
        let bps_units = u128::from(bps_seed % (10_000 * 10u32.pow(bps_scale) + 1));
        // The venue prints eight decimals: the same Q with trailing zeros.
        let gross = if venue_format {
            scaled(u128::from(q_units) * 10u128.pow(8 - q_scale), 8)
        } else {
            scaled(u128::from(q_units), q_scale)
        };
        let mut leg = leg();
        leg.terms = Some(AdmissionTerms {
            taker_bps: scaled(bps_units, bps_scale),
            lot_step: scaled(u128::from(step_units), step_scale),
        });
        let mut record = entry(gross, Some(dec!(1)), true);
        record.orig_qty = gross;
        let merged = merge_order(None, &record, &owned(&leg, LegOrderKind::Entry), &at(1), Some(&leg)).unwrap();
        // B̂ = Q·bps/10⁴ and X = floor((Q − B̂)/step)·step over integers.
        let denominator = q_scale + bps_scale + 4;
        let whole = 10u128.pow(bps_scale + 4);
        let net = u128::from(q_units) * (whole - bps_units);
        let steps = net * 10u128.pow(step_scale) / (10u128.pow(denominator) * u128::from(step_units));
        let oracle = expected(
            gross,
            dec!(1),
            scaled(u128::from(q_units) * bps_units, denominator),
            scaled(steps * u128::from(step_units), step_scale),
        );
        prop_assert_eq!(merged.set_partition, Some(oracle));
    }

    #[test]
    fn the_partition_is_set_at_most_once_and_never_changes(
        events in proptest::collection::vec((0u8..=10, any::<bool>(), any::<bool>()), 1..30),
    ) {
        let mut leg = leg();
        let link = owned(&leg, LegOrderKind::Entry);
        let mut stored: Option<StoredOrder> = None;
        let mut sets = 0;
        for (index, (quantity, terminal, quoted)) in events.into_iter().enumerate() {
            let executed = Decimal::new(i64::from(quantity), 2);
            let record = entry(executed, quoted.then(|| executed * dec!(100000)), terminal);
            match merge_order(stored.as_ref(), &record, &link, &at(index as i64 + 1), Some(&leg)) {
                Ok(merged) => {
                    if let Some(partition) = merged.set_partition {
                        prop_assert!(leg.partition.is_none());
                        prop_assert!(merged.order.terminal());
                        sets += 1;
                        leg.partition = Some(partition);
                    }
                    if merged.set_cohort {
                        leg.cohort_month = Some(leg.decided_month);
                    }
                    if let Some(partition) = leg.partition {
                        // The recorded (Q, C) is the frozen first terminal tuple.
                        prop_assert!(merged.order.terminal());
                        prop_assert_eq!(
                            (Some(partition.gross), Some(partition.cost)),
                            (Some(merged.order.executed_cum), merged.order.cumulative_quote)
                        );
                        prop_assert!(partition.base_fee + partition.executable <= partition.gross);
                    }
                    stored = Some(merged.order);
                },
                Err(error) => prop_assert!(matches!(
                    error,
                    LedgerError::TradeConflict | LedgerError::IdentityConflict
                )),
            }
        }
        prop_assert!(sets <= 1);
    }
}
