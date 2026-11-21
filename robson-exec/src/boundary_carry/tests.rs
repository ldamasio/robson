//! 8p-3 fixed tests (PLAN-r3 §8): every §C4 branch, the exact-B fixtures,
//! the review's worked examples to the digit, rounding, holds, SD15, SD16 and
//! the multi-boundary record. Every order carries venue times chosen to
//! mislead (v16:1958); no result may depend on them.

use chrono::{DateTime, NaiveDate, TimeDelta, Utc};
use rust_decimal::{Decimal, RoundingStrategy};
use rust_decimal_macros::dec;
use uuid::Uuid;

use super::*;

const LEG: Uuid = Uuid::from_u128(0x0000_8003_0000_4000_8000_0000_0000_0001);
const OTHER_LEG: Uuid = Uuid::from_u128(0x0000_8003_0000_4000_8000_0000_0000_0002);
const SEC: i64 = 1_000;
const HOUR: i64 = 3_600_000;
const DAY: i64 = 86_400_000;

fn first(year: i32, month: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, 1).unwrap()
}

fn nov(mark: Decimal) -> Boundary {
    Boundary::new(first(2026, 11), mark).unwrap()
}

/// B = 2026-11-01T00:00:00Z.
fn b() -> DateTime<Utc> {
    nov(dec!(1)).instant()
}

fn at(offset_ms: i64) -> DateTime<Utc> {
    b() + TimeDelta::milliseconds(offset_ms)
}

fn stamp(offset_ms: i64, verdict: Option<ClockVerdict>) -> RobsonTime {
    RobsonTime { at: at(offset_ms), verdict }
}

fn valid(offset_ms: i64) -> RobsonTime {
    stamp(offset_ms, Some(ClockVerdict::Valid))
}

fn oid(n: u64) -> VenueIntId {
    n.to_string().parse().unwrap()
}

/// Venue times that would flip a decision if read: created and working from
/// after B, updated and transacted before it.
fn misleading() -> VenueRecordTimes {
    let ms = |offset: i64| Some(at(offset).timestamp_millis());
    VenueRecordTimes {
        time_ms: ms(HOUR),
        update_time_ms: ms(-HOUR),
        transact_time_ms: ms(-HOUR),
        working_time_ms: ms(HOUR),
    }
}

/// The review's common terms (§4): f = 10 bps, gap 10 bps, R̂ = 0.00008 and
/// a = X × 8,223.928 + 0.80 + R̂ × AF × 0.999 = 19.70677056.
fn terms() -> LegTerms {
    LegTerms {
        gap_bps: dec!(10),
        taker_bps: dec!(10),
        remainder_reserve: dec!(0.00008),
        admission_charge: dec!(19.70677056),
    }
}

/// A buy: its base fee is already out of X.
fn buy(offset_ms: i64, qty: Decimal, price: Decimal) -> Fill {
    Fill {
        trade_time: at(offset_ms),
        qty,
        price,
        commission: Decimal::ZERO,
    }
}

/// A sell paying the 0.1 % taker fee in quote.
fn sell(offset_ms: i64, qty: Decimal, price: Decimal) -> Fill {
    let commission = qty * price * dec!(0.001);
    Fill {
        trade_time: at(offset_ms),
        qty,
        price,
        commission,
    }
}

/// An order acknowledged on 28 October with valid evidence; a terminal one
/// was seen terminal after B, at the restart.
fn order(n: u64, kind: LegOrderKind, status: &str, qty: Decimal) -> LegOrder {
    let phase = OrderPhase::from_binance_spot_status(status);
    LegOrder {
        exchange_order_id: oid(n),
        kind,
        status: status.to_string(),
        phase,
        orig_qty: qty,
        stop_price: None,
        live_seen: valid(-4 * DAY + 2 * SEC),
        terminal_seen: (phase == OrderPhase::Terminal).then(|| valid(70 * SEC)),
        first_attempt: Some(valid(-4 * DAY + SEC)),
        fills: Vec::new(),
        lookup: None,
        venue_record_times: misleading(),
    }
}

/// The review's entry: 0.00161 BTC at 80,000 on 28 October (C = 128.80), X =
/// 0.0016.
fn entry() -> LegOrder {
    let mut entry = order(1, LegOrderKind::Entry, "FILLED", dec!(0.00161));
    entry.fills = vec![buy(-4 * DAY, dec!(0.00161), dec!(80000))];
    entry.terminal_seen = Some(valid(-4 * DAY + SEC));
    entry
}

/// A stop placed on 28 October and still `NEW` at completion: the owner's
/// lookup sent at B + 70 s answered `NEW`.
fn stop(n: u64, trigger: Decimal, qty: Decimal) -> LegOrder {
    let mut stop = order(n, LegOrderKind::Stop, "NEW", qty);
    stop.stop_price = Some(trigger);
    stop.lookup = Some(Lookup {
        sent: valid(70 * SEC),
        status: "NEW".to_string(),
    });
    stop
}

/// A stop of the review's leg that ended (FILLED, CANCELED or EXPIRED) after
/// B with no lookup, as found at the restart.
fn ended_stop(status: &str) -> LegOrder {
    let mut stop = stop(2, dec!(72000), dec!(0.0016));
    stop.status = status.to_string();
    stop.phase = OrderPhase::Terminal;
    stop.terminal_seen = Some(valid(70 * SEC));
    stop.lookup = None;
    stop
}

fn leg_of(orders: Vec<LegOrder>) -> CandidateLeg {
    CandidateLeg {
        position_id: LEG,
        cohort_month: Some(first(2026, 10)),
        executable: Some(dec!(0.0016)),
        terms: terms(),
        orders,
        summarized_sold_before_b: Decimal::ZERO,
        cancels: Vec::new(),
        dust: None,
    }
}

/// The review's leg with one sell-capable order.
fn with(order: LegOrder) -> CandidateLeg {
    leg_of(vec![entry(), order])
}

/// The review's leg with its 72,000 stop on 0.0016.
fn review_leg() -> CandidateLeg {
    with(stop(2, dec!(72000), dec!(0.0016)))
}

/// The review's leg shape for another position.
fn other_leg(order: LegOrder) -> CandidateLeg {
    CandidateLeg { position_id: OTHER_LEG, ..with(order) }
}

fn at_b(leg: &CandidateLeg, mark: Decimal) -> Result<Option<CarriedLeg>, Hold> {
    leg_at_b(leg, &nov(mark))
}

/// F_B of a carried leg at P_B = 81,000.
fn floor(leg: &CandidateLeg) -> Decimal {
    at_b(leg, dec!(81000)).unwrap().unwrap().floor
}

fn r4(value: Decimal) -> String {
    value
        .round_dp_with_strategy(4, RoundingStrategy::MidpointAwayFromZero)
        .to_string()
}

/// Robson's cancel of order `n`, answered 200 and classified Cancelled.
fn cancel(n: u64, attempt: RobsonTime) -> CancelIntent {
    CancelIntent {
        position_id: LEG,
        cancels_order_id: oid(n),
        venue_outcome: Some("success".to_string()),
        cancel_class: Some("cancelled".to_string()),
        first_attempt: Some(attempt),
    }
}

fn november() -> (DateTime<Utc>, DateTime<Utc>) {
    (b(), b() + TimeDelta::days(30))
}

#[test]
fn boundary_is_a_first_day_with_a_positive_mark() {
    assert_eq!(b(), "2026-11-01T00:00:00Z".parse::<DateTime<Utc>>().unwrap());
    assert!(Boundary::new(NaiveDate::from_ymd_opt(2026, 11, 2).unwrap(), dec!(1)).is_none());
    assert!(Boundary::new(first(2026, 11), Decimal::ZERO).is_none());
    assert!(Boundary::new(first(2026, 11), dec!(-1)).is_none());
}

#[test]
fn review_example_1_carries_the_leg_at_its_stop_floor() {
    let (carried, totals) = snapshot(dec!(1871.20), &[review_leg()], &nov(dec!(81000))).unwrap();
    assert_eq!(carried.len(), 1);
    let leg = &carried[0];
    let protection = leg.protection.as_ref().unwrap();
    assert_eq!((protection.stop, protection.trigger), (oid(2), dec!(72000)));
    assert_eq!(protection.open_after_b, OpenAfterB::NewAtCompletion);
    assert_eq!(
        (protection.floor.adverse_fill, protection.floor.price),
        (dec!(71928), dec!(71928))
    );
    assert_eq!(leg.g_b, dec!(0.0016));
    assert_eq!((leg.floor, r4(leg.floor)), (dec!(109.22122944), "109.2212".to_string()));
    assert_eq!((leg.mark_value, leg.downside), (dec!(129.6), dec!(20.37877056)));
    assert_eq!(r4(leg.downside), "20.3788");
    assert_eq!(totals.basis, dec!(1980.42122944));
    assert_eq!(r4(totals.basis), "1980.4212");
    // Equity at the mark, 2,000.80, less Σ D_B is the basis.
    assert_eq!(dec!(1871.20) + leg.mark_value - totals.downside, totals.basis);
    // Stopped on 3 November at 71,950: Δcash 115.0049 ≥ F_B, nothing consumed.
    let mut leg = review_leg();
    leg.orders[1].fills = vec![sell(2 * DAY, dec!(0.0016), dec!(71950))];
    let (from, to) = november();
    let cash = delta_cash(&leg.orders, from..to).unwrap();
    assert_eq!((cash, r4(cash)), (dec!(115.00488), "115.0049".to_string()));
    assert_eq!(realized_excess(dec!(109.22122944), cash), Ok(Decimal::ZERO));
}

#[test]
fn review_example_2_floor_at_a_trailing_stop_below_the_mark() {
    let leg = with(stop(2, dec!(88000), dec!(0.0016)));
    let (carried, totals) = snapshot(dec!(1871.20), &[leg], &nov(dec!(96000))).unwrap();
    let floor = carried[0].protection.as_ref().unwrap().floor;
    assert_eq!((floor.adverse_fill, floor.price), (dec!(87912), dec!(87912)));
    assert_eq!(
        (carried[0].floor, r4(carried[0].floor)),
        (dec!(133.49261376), "133.4926".to_string())
    );
    assert_eq!((totals.basis, r4(totals.basis)), (dec!(2004.69261376), "2004.6926".to_string()));
    assert_eq!(
        (totals.downside, r4(totals.downside)),
        (dec!(20.10738624), "20.1074".to_string())
    );
    assert_eq!(dec!(1871.20) + carried[0].mark_value, dec!(2024.80));
    // Stopped at 87,950 on 2 November: 140.5793, 7.0867 above the floor.
    let cash = dec!(0.0016) * dec!(87950) * dec!(0.999);
    assert_eq!(r4(cash), "140.5793");
    assert_eq!(r4(cash - carried[0].floor), "7.0867");
    assert_eq!(realized_excess(carried[0].floor, cash), Ok(Decimal::ZERO));
}

#[test]
fn carried_excess_examples_match_to_the_digit() {
    let f_b = floor(&review_leg());
    let (from, to) = november();
    // Example 3: the stop gaps through, its market order fills at 60,500.
    let mut gap = review_leg();
    gap.orders[1].fills = vec![sell(2 * DAY, dec!(0.0016), dec!(60500))];
    let cash = delta_cash(&gap.orders, from..to).unwrap();
    assert_eq!(cash, dec!(96.7032));
    let realized = realized_excess(f_b, cash).unwrap();
    assert_eq!((realized, r4(realized)), (dec!(12.51802944), "12.5180".to_string()));
    // The fall from the mark is D_B (absorbed in the basis) plus CE_real.
    let carried = at_b(&gap, dec!(81000)).unwrap().unwrap();
    assert_eq!(carried.mark_value - cash, carried.downside + realized);
    assert_eq!(carried.mark_value - cash, dec!(32.8968));
    // The partial-exit variant: 0.0010 at 71,950 on 5 November, the rest
    // re-protected at 72,000, then gapped and sold at 60,000 on 9 November.
    let mut partial = review_leg();
    partial.orders[1].fills = vec![sell(4 * DAY, dec!(0.0010), dec!(71950))];
    let first_cash = delta_cash(&partial.orders, from..to).unwrap();
    assert_eq!((first_cash, r4(first_cash)), (dec!(71.87805), "71.8781".to_string()));
    let f_now = floor_at(dec!(0.0006), dec!(72000), dec!(72500), &terms()).unwrap().quote;
    assert_eq!((f_now, r4(f_now)), (dec!(37.36515744), "37.3652".to_string()));
    assert_eq!(latent_excess(f_b, first_cash, f_now), Ok(Decimal::ZERO));
    // Unprotected instead: the whole shortfall stays reserved (the review
    // shows 109.2212 − 71.8781 = 37.3431 from rounded figures).
    assert_eq!(latent_excess(f_b, first_cash, Decimal::ZERO), Ok(dec!(37.34317944)));
    let mut rest = stop(3, dec!(72000), dec!(0.0006));
    rest.first_attempt = Some(valid(4 * DAY + SEC));
    rest.live_seen = valid(4 * DAY + 2 * SEC);
    rest.fills = vec![sell(8 * DAY, dec!(0.0006), dec!(60000))];
    partial.orders.push(rest);
    let all_cash = delta_cash(&partial.orders, from..to).unwrap();
    assert_eq!((all_cash, r4(all_cash)), (dec!(107.84205), "107.8421".to_string()));
    let realized = realized_excess(f_b, all_cash).unwrap();
    assert_eq!((realized, r4(realized)), (dec!(1.37917944), "1.3792".to_string()));
    // Entry in flight, stopped at 71,950: Δcash −13.7951 ≥ −a, nothing consumed.
    let flight = in_flight_leg(&[(20, dec!(0.00161))]);
    let in_flight = at_b(&flight, dec!(81000)).unwrap().unwrap();
    let cash = delta_cash(&flight.orders, from..to).unwrap();
    assert_eq!((cash, r4(cash)), (dec!(-13.79512), "-13.7951".to_string()));
    assert_eq!(realized_excess(in_flight.floor, cash), Ok(Decimal::ZERO));
}

/// An entry authorized at 23:59:59.98 (cohort October) with buys at the
/// given offsets from B, stopped at 71,950 on 3 November by a stop placed
/// after B.
fn in_flight_leg(buys: &[(i64, Decimal)]) -> CandidateLeg {
    let mut entry = entry();
    entry.first_attempt = Some(valid(-20));
    entry.live_seen = valid(5);
    entry.terminal_seen = Some(valid(70 * SEC));
    entry.fills = buys.iter().map(|(offset, qty)| buy(*offset, *qty, dec!(80000))).collect();
    let mut stop = ended_stop("FILLED");
    stop.first_attempt = Some(valid(80 * SEC));
    stop.live_seen = valid(81 * SEC);
    stop.fills = vec![sell(2 * DAY, dec!(0.0016), dec!(71950))];
    stop.terminal_seen = Some(valid(2 * DAY + SEC));
    leg_of(vec![entry, stop])
}

#[test]
fn entry_in_flight_floor_is_minus_the_charge_to_the_digit() {
    let a = terms().admission_charge;
    assert_eq!(r4(a), "19.7068");
    let flight = in_flight_leg(&[(20, dec!(0.00161))]);
    let (carried, totals) = snapshot(dec!(2000), &[flight], &nov(dec!(81000))).unwrap();
    let expected = CarriedLeg {
        position_id: LEG,
        g_b: Decimal::ZERO,
        protection: None,
        floor: -a,
        mark_value: Decimal::ZERO,
        downside: a,
    };
    assert_eq!(carried, vec![expected.clone()]);
    assert_eq!((totals.basis, r4(totals.basis)), (dec!(1980.29322944), "1980.2932".to_string()));
    // Half bought at B − 1 ms (64.40 already out of quote_B = 1,935.60): the
    // same −a, no inventory counted, 64.40 below the protected value.
    let half = dec!(0.000805);
    let straddle = in_flight_leg(&[(-1, half), (20, half)]);
    let (carried, totals) =
        snapshot(dec!(1935.60), std::slice::from_ref(&straddle), &nov(dec!(81000))).unwrap();
    assert_eq!(carried, vec![expected]);
    assert_eq!((totals.basis, r4(totals.basis)), (dec!(1915.89322944), "1915.8932".to_string()));
    // Its post-boundary cash is at least c₁ − a, so no carried excess.
    let (from, to) = november();
    let cash = delta_cash(&straddle.orders, from..to).unwrap();
    assert_eq!(cash, dec!(50.60488));
    assert!(cash >= dec!(64.40) - a);
}

#[test]
fn entry_with_a_fill_at_b_is_in_flight() {
    // FILLED, one trade at B − 1 ms and one at exactly B.
    let mut entry = entry();
    entry.fills = vec![
        buy(-1, dec!(0.0008), dec!(80000)),
        buy(0, dec!(0.00081), dec!(80000)),
    ];
    entry.terminal_seen = Some(valid(70 * SEC));
    let carried = at_b(&leg_of(vec![entry, stop(2, dec!(72000), dec!(0.0016))]), dec!(81000));
    let carried = carried.unwrap().unwrap();
    assert_eq!(
        (carried.g_b, carried.floor, carried.protection),
        (dec!(0), -terms().admission_charge, None)
    );
    // Every trade before B, but CANCELED and seen terminal only within 2 s of
    // B: in flight; seen at B − 2 s: complete, carried with inventory.
    for (seen, in_flight) in [(-1999, true), (-2000, false)] {
        let mut entry = order(1, LegOrderKind::Entry, "CANCELED", dec!(0.002));
        entry.fills = vec![buy(-HOUR, dec!(0.00161), dec!(80000))];
        entry.terminal_seen = Some(valid(seen));
        let carried = at_b(&with_entry(entry), dec!(81000)).unwrap().unwrap();
        assert_eq!(carried.g_b.is_zero(), in_flight, "{seen}");
    }
    // Still working at B, or no entry evidence at all: in flight.
    let mut working = order(1, LegOrderKind::Entry, "NEW", dec!(0.00161));
    working.fills = vec![buy(-HOUR, dec!(0.00161), dec!(80000))];
    assert!(at_b(&with_entry(working), dec!(81000)).unwrap().unwrap().g_b.is_zero());
    let missing = leg_of(vec![stop(2, dec!(72000), dec!(0.0016))]);
    assert_eq!(at_b(&missing, dec!(81000)).unwrap().unwrap().floor, -terms().admission_charge);
}

fn with_entry(entry: LegOrder) -> CandidateLeg {
    leg_of(vec![entry, stop(2, dec!(72000), dec!(0.0016))])
}

#[test]
fn sell_at_exactly_b_is_post_boundary() {
    let mut stopped = ended_stop("FILLED");
    // The stop fills at exactly B: post-boundary, the leg is carried.
    stopped.fills = vec![sell(0, dec!(0.0016), dec!(71950))];
    let at_b_leg = with(stopped.clone());
    let (carried, totals) =
        snapshot(dec!(1871.20), std::slice::from_ref(&at_b_leg), &nov(dec!(81000))).unwrap();
    assert_eq!(carried[0].g_b, dec!(0.0016));
    assert_eq!(carried[0].protection.as_ref().unwrap().open_after_b, OpenAfterB::TradeAfterB);
    assert_eq!((totals.basis, r4(totals.basis)), (dec!(1980.42122944), "1980.4212".to_string()));
    let (from, to) = november();
    let cash = delta_cash(&at_b_leg.orders, from..to).unwrap();
    assert_eq!(realized_excess(carried[0].floor, cash), Ok(Decimal::ZERO));
    // At B − 1 ms: pre-boundary, the leg is not carried and quote_B holds
    // the proceeds.
    stopped.fills = vec![sell(-1, dec!(0.0016), dec!(71950))];
    let before = with(stopped);
    let quote_b = dec!(1871.20) + dec!(115.00488);
    let (carried, totals) =
        snapshot(quote_b, std::slice::from_ref(&before), &nov(dec!(81000))).unwrap();
    assert!(carried.is_empty());
    assert_eq!((totals.basis, r4(totals.basis)), (dec!(1986.20488), "1986.2049".to_string()));
    // No unit counted twice: quote_B plus K's cash of the leg is the cash held.
    for (quote_b, leg) in [(dec!(1871.20), at_b_leg), (quote_b, before)] {
        assert_eq!(quote_b + delta_cash(&leg.orders, from..to).unwrap(), dec!(1986.20488));
    }
}

#[test]
fn a_sweep_sharing_one_timestamp_is_never_split() {
    for (offset, carried) in [(0, true), (-1, false)] {
        let mut stopped = ended_stop("FILLED");
        stopped.fills = [dec!(0.0005), dec!(0.0005), dec!(0.0006)]
            .into_iter()
            .map(|qty| sell(offset, qty, dec!(71950)))
            .collect();
        let result = at_b(&with(stopped), dec!(81000)).unwrap();
        assert_eq!(result.as_ref().map(|leg| leg.g_b), carried.then_some(dec!(0.0016)));
    }
}

#[test]
fn stop_branches_open_after_b() {
    // (b1) still NEW at completion.
    let carried = at_b(&review_leg(), dec!(81000)).unwrap().unwrap();
    assert_eq!(carried.protection.unwrap().open_after_b, OpenAfterB::NewAtCompletion);
    // (b2) the daemon was down; the stop filled at 00:10.
    let mut filled = ended_stop("FILLED");
    filled.fills = vec![sell(10 * 60 * SEC, dec!(0.0016), dec!(71990))];
    filled.terminal_seen = Some(valid(10 * 60 * SEC + SEC));
    let carried = at_b(&with(filled), dec!(81000)).unwrap().unwrap();
    assert_eq!(carried.protection.unwrap().open_after_b, OpenAfterB::TradeAfterB);
    assert_eq!(carried.floor, dec!(109.22122944));
    // (b3) Robson's own cancel after B + 2 s found it open.
    let mut cancelled = with(ended_stop("CANCELED"));
    cancelled.cancels = vec![cancel(2, valid(60 * SEC))];
    let carried = at_b(&cancelled, dec!(81000)).unwrap().unwrap();
    assert_eq!(carried.protection.unwrap().open_after_b, OpenAfterB::RobsonCancelAfterB);
    // Someone else's cancel, a cancel of another order or leg, a rejected or
    // gone-terminal answer: nothing proves the stop open after B.
    let others = [
        CancelIntent {
            position_id: OTHER_LEG,
            ..cancel(2, valid(60 * SEC))
        },
        cancel(9, valid(60 * SEC)),
        CancelIntent {
            venue_outcome: Some("rejected".into()),
            ..cancel(2, valid(60 * SEC))
        },
        CancelIntent {
            cancel_class: Some("gone_terminal".into()),
            ..cancel(2, valid(60 * SEC))
        },
        CancelIntent {
            venue_outcome: None,
            ..cancel(2, valid(60 * SEC))
        },
    ];
    for other in others {
        let mut leg = with(ended_stop("CANCELED"));
        leg.cancels = vec![other.clone()];
        assert_eq!(floor(&leg), Decimal::ZERO, "{other:?}");
    }
    // FILLED with every trade before B: finished before B, not carried.
    let mut done = ended_stop("FILLED");
    done.fills = vec![sell(-10 * 60 * SEC, dec!(0.0016), dec!(71990))];
    done.terminal_seen = Some(valid(-10 * 60 * SEC + SEC));
    assert_eq!(at_b(&with(done), dec!(81000)), Ok(None));
}

#[test]
fn expired_stop_without_trade_gives_floor_zero() {
    // It expired at 23:55; nothing proves it open after B, whatever its
    // updateTime says. The leg's value is outside the basis.
    for status in ["EXPIRED", "EXPIRED_IN_MATCH", "REJECTED"] {
        let carried = at_b(&with(ended_stop(status)), dec!(81000)).unwrap().unwrap();
        assert_eq!((carried.floor, carried.protection.clone()), (dec!(0), None), "{status}");
        assert_eq!(carried.downside, dec!(129.6));
    }
    // A post-B lookup that answers anything but NEW proves nothing.
    for answer in [
        "EXPIRED",
        "EXPIRED_IN_MATCH",
        "FILLED",
        "CANCELED",
        "PARTIALLY_FILLED",
    ] {
        let mut stop = stop(2, dec!(72000), dec!(0.0016));
        stop.lookup = Some(Lookup {
            sent: valid(70 * SEC),
            status: answer.to_string(),
        });
        assert_eq!(floor(&with(stop)), Decimal::ZERO, "{answer}");
    }
}

#[test]
fn expired_answer_classified_cancelled_proves_nothing() {
    // A 200 classified Cancelled whose terminal record is EXPIRED, and one
    // FILLED (no trade booked), prove nothing; only CANCELED does.
    for (status, protected) in [("EXPIRED", false), ("FILLED", false), ("CANCELED", true)] {
        let mut leg = with(ended_stop(status));
        leg.cancels = vec![cancel(2, valid(60 * SEC))];
        assert_eq!(floor(&leg) > Decimal::ZERO, protected, "{status}");
    }
    // The target's terminal record not booked yet: K waits for it.
    let mut live = stop(2, dec!(72000), dec!(0.0016));
    live.lookup = None;
    let mut leg = with(live);
    leg.cancels = vec![cancel(2, valid(60 * SEC))];
    assert_eq!(at_b(&leg, dec!(81000)), Err(Hold::TerminalRecordNeeded(oid(2))));
}

#[test]
fn acknowledgement_within_two_seconds_of_b_proves_nothing() {
    for (offset, protected) in [(-1999, false), (-2000, true)] {
        let mut stop = stop(2, dec!(72000), dec!(0.0016));
        stop.live_seen = valid(offset);
        assert_eq!(floor(&with(stop)) > Decimal::ZERO, protected, "ack {offset}");
    }
    // The lookup and Robson's cancel must be sent at or after B + 2 s.
    for (offset, protected) in [(1999, false), (2000, true)] {
        let mut looked = stop(2, dec!(72000), dec!(0.0016));
        looked.lookup = Some(Lookup {
            sent: valid(offset),
            status: "NEW".to_string(),
        });
        assert_eq!(floor(&with(looked)) > Decimal::ZERO, protected, "lookup {offset}");
        let mut leg = with(ended_stop("CANCELED"));
        leg.cancels = vec![cancel(2, valid(offset))];
        assert_eq!(floor(&leg) > Decimal::ZERO, protected, "cancel {offset}");
        // A second order excluded by its intent at B + 2 s, not before.
        let mut second = order(3, LegOrderKind::Sell, "NEW", dec!(0.0016));
        second.first_attempt = Some(valid(offset));
        second.live_seen = valid(offset + SEC);
        let leg = leg_of(vec![entry(), stop(2, dec!(72000), dec!(0.0016)), second]);
        assert_eq!(floor(&leg) > Decimal::ZERO, protected, "intent {offset}");
    }
    // Excluded by being seen terminal at B − 2 s, not later.
    for (offset, protected) in [(-1999, false), (-2000, true)] {
        let mut second = order(3, LegOrderKind::Sell, "EXPIRED", dec!(0.0016));
        second.terminal_seen = Some(valid(offset));
        let leg = leg_of(vec![entry(), stop(2, dec!(72000), dec!(0.0016)), second]);
        assert_eq!(floor(&leg) > Decimal::ZERO, protected, "terminal {offset}");
    }
}

#[test]
fn invalid_clock_evidence_gives_floor_zero() {
    for verdict in [Some(ClockVerdict::Invalid), None] {
        let mut ack = stop(2, dec!(72000), dec!(0.0016));
        ack.live_seen = stamp(-DAY, verdict);
        assert_eq!(floor(&with(ack)), Decimal::ZERO, "ack {verdict:?}");
        let mut looked = stop(2, dec!(72000), dec!(0.0016));
        looked.lookup = Some(Lookup {
            sent: stamp(70 * SEC, verdict),
            status: "NEW".into(),
        });
        assert_eq!(floor(&with(looked)), Decimal::ZERO, "lookup {verdict:?}");
        let mut cancelled = with(ended_stop("CANCELED"));
        cancelled.cancels = vec![cancel(2, stamp(60 * SEC, verdict))];
        assert_eq!(floor(&cancelled), Decimal::ZERO, "cancel {verdict:?}");
        let mut late = order(3, LegOrderKind::Sell, "NEW", dec!(0.0016));
        late.first_attempt = Some(stamp(HOUR, verdict));
        late.live_seen = valid(HOUR + SEC);
        let excluded_late = leg_of(vec![entry(), stop(2, dec!(72000), dec!(0.0016)), late]);
        assert_eq!(floor(&excluded_late), Decimal::ZERO, "intent {verdict:?}");
        let mut ended = order(3, LegOrderKind::Sell, "EXPIRED", dec!(0.0016));
        ended.terminal_seen = Some(stamp(-HOUR, verdict));
        let excluded_ended = leg_of(vec![entry(), stop(2, dec!(72000), dec!(0.0016)), ended]);
        assert_eq!(floor(&excluded_ended), Decimal::ZERO, "terminal {verdict:?}");
    }
}

#[test]
fn stop_quantity_must_equal_g_b() {
    for qty in [dec!(0.0015), dec!(0.0017)] {
        assert_eq!(floor(&with(stop(2, dec!(72000), qty))), Decimal::ZERO, "{qty}");
    }
    // Partly sold before B by this stop: its open quantity at B is G_B.
    let mut partial = stop(2, dec!(72000), dec!(0.0016));
    partial.fills = vec![sell(-HOUR, dec!(0.0006), dec!(72000))];
    partial.status = "PARTIALLY_FILLED".to_string();
    let carried = at_b(&with(partial), dec!(81000)).unwrap().unwrap();
    assert_eq!(carried.g_b, dec!(0.0010));
    assert_eq!(carried.floor, dec!(0.00092) * dec!(71928) * dec!(0.999));
}

#[test]
fn unexcluded_second_order_gives_floor_zero() {
    let second = |status: &str| order(3, LegOrderKind::Sell, status, dec!(0.0016));
    // Neither excluded: a sell working at B, a second stop, a sell that
    // traded at B (seen terminal only after it).
    let mut traded = second("FILLED");
    traded.fills = vec![sell(0, dec!(0.0016), dec!(80000))];
    let mut other_stop = stop(3, dec!(70000), dec!(0.0015));
    other_stop.lookup = None;
    for other in [second("NEW"), other_stop, traded] {
        let leg = leg_of(vec![entry(), stop(2, dec!(72000), dec!(0.0016)), other.clone()]);
        assert_eq!(floor(&leg), Decimal::ZERO, "{other:?}");
    }
    // Two stops that both satisfy (a), (b) and (c): neither is the one.
    let leg = leg_of(vec![
        entry(),
        stop(2, dec!(72000), dec!(0.0016)),
        stop(3, dec!(71000), dec!(0.0016)),
    ]);
    assert_eq!(floor(&leg), Decimal::ZERO);
    // Controls: shown not open at B by a post-B intent, or by being seen
    // terminal before B with every trade before B.
    let mut late = second("NEW");
    late.first_attempt = Some(valid(HOUR));
    late.live_seen = valid(HOUR + SEC);
    let mut ended = second("CANCELED");
    ended.terminal_seen = Some(valid(-HOUR));
    for other in [late, ended] {
        let leg = leg_of(vec![entry(), stop(2, dec!(72000), dec!(0.0016)), other.clone()]);
        assert_eq!(floor(&leg), dec!(109.22122944), "{other:?}");
    }
}

#[test]
fn floor_price_is_the_min_of_adverse_fill_and_marked_gap() {
    // The mark below the trigger caps p_B at P_B × (1 − gap).
    let carried = at_b(&review_leg(), dec!(70000)).unwrap().unwrap();
    let floor = carried.protection.unwrap().floor;
    assert_eq!((floor.adverse_fill, floor.price), (dec!(71928), dec!(69930)));
    assert_eq!(carried.floor, dec!(106.1873064));
    assert_eq!(carried.downside, dec!(0.0016) * dec!(70000) - dec!(106.1873064));
    // At or above it, p_B is the adverse fill.
    for mark in [dec!(72000), dec!(81000)] {
        let floor = at_b(&review_leg(), mark).unwrap().unwrap().protection.unwrap().floor;
        assert_eq!(floor.price, dec!(71928), "{mark}");
    }
}

#[test]
fn floor_excludes_the_remainder_reserve() {
    // (0.0016 − 0.00008) × 71,928 × 0.999, not 0.0016 × ….
    assert_eq!(floor(&review_leg()), dec!(109.22122944));
    // A remainder within the reserve is protected and floors at 0.
    let mut small = review_leg();
    small.executable = Some(dec!(0.00005));
    small.orders[1].orig_qty = dec!(0.00005);
    let carried = at_b(&small, dec!(81000)).unwrap().unwrap();
    assert!(carried.protection.is_some());
    assert_eq!((carried.floor, carried.downside), (dec!(0), dec!(4.05)));
}

#[test]
fn floors_round_down() {
    // AF = 71,927.99999999999999999999001 is exact; F's exact value needs
    // scale 33, so it rounds down at the finest scale that fits 96 bits.
    let trigger = dec!(71999.99999999999999999999);
    let leg = with(stop(2, trigger, dec!(0.0016)));
    let carried = at_b(&leg, dec!(81000)).unwrap().unwrap();
    let floor = carried.protection.as_ref().unwrap().floor;
    assert_eq!(floor.adverse_fill, dec!(71927.99999999999999999999001));
    // Exact F = 152 × AF coefficient × 9990 / 10³³ (integer oracle).
    let af = u128::try_from(floor.adverse_fill.mantissa()).unwrap();
    let exact = 152 * af * 10_u128.pow(24 - floor.adverse_fill.scale()) * 9990;
    let (scale, quotient) = (0..=28_u32)
        .rev()
        .map(|scale| (scale, exact / 10_u128.pow(33 - scale)))
        .find(|(_, quotient)| *quotient < 1 << 96)
        .unwrap();
    assert_eq!(scale, 26);
    assert_ne!(exact % 10_u128.pow(33 - scale), 0, "the fixture must need rounding");
    let expected = Decimal::from_i128_with_scale(i128::try_from(quotient).unwrap(), scale);
    assert_eq!(carried.floor, expected);
    assert_eq!(carried.floor, dec!(109.22122943999999999999998483));
    // D_B is its exact complement to G_B × P_B.
    assert_eq!(carried.downside, dec!(20.37877056000000000000001517));
    assert_eq!(carried.downside + carried.floor, carried.mark_value);
    // An adverse fill the ledger cannot hold exactly holds K.
    let leg = with(stop(2, dec!(7199.999999999999999999999999), dec!(0.0016)));
    assert_eq!(at_b(&leg, dec!(81000)), Err(Hold::Invalid));
}

#[test]
fn unproven_write_off_close_keeps_the_leg_carried() {
    // Everything but R = 0.00008 sold before B; the dust row writes R off.
    let written_off = |close: RobsonTime, last_fill: i64| {
        let mut sold = order(2, LegOrderKind::Sell, "FILLED", dec!(0.00152));
        sold.fills = vec![sell(last_fill, dec!(0.00152), dec!(81000))];
        let mut leg = leg_of(vec![entry(), sold]);
        leg.dust = Some(Dust {
            write_off: dec!(0.00008),
            close_at: close,
            last_fill_at: at(last_fill),
        });
        leg
    };
    // Close proven before B: finished, not carried; E4 is dated before B.
    let proven = written_off(valid(-3 * SEC), -HOUR);
    assert!(write_off_before(&proven.dust.unwrap(), b()));
    assert_eq!(at_b(&proven, dec!(81000)), Ok(None));
    // Close within 2 s of B, with invalid evidence, or after the last fill at
    // B: carried with its remainder, unprotected; E4 is dated in K.
    let unproven = [
        written_off(valid(-SEC), -HOUR),
        written_off(stamp(-3 * SEC, Some(ClockVerdict::Invalid)), -HOUR),
        written_off(stamp(-3 * SEC, None), -HOUR),
        written_off(valid(3 * SEC), -HOUR),
    ];
    for leg in unproven {
        assert!(!write_off_before(&leg.dust.unwrap(), b()));
        let carried = at_b(&leg, dec!(81000)).unwrap().unwrap();
        assert_eq!(
            (carried.g_b, carried.floor, carried.downside),
            (dec!(0.00008), dec!(0), dec!(6.48))
        );
    }
    // A last fill at B: that sale is post-boundary, G_B is X.
    let at_b_fill = written_off(valid(-3 * SEC), 0);
    assert!(!write_off_before(&at_b_fill.dust.unwrap(), b()));
    assert_eq!(at_b(&at_b_fill, dec!(81000)).unwrap().unwrap().g_b, dec!(0.0016));
}

#[test]
fn a_missing_lookup_keeps_k_pending() {
    let mut unanswered = stop(2, dec!(72000), dec!(0.0016));
    unanswered.lookup = None;
    assert_eq!(at_b(&with(unanswered.clone()), dec!(81000)), Err(Hold::LookupNeeded(oid(2))));
    let held =
        snapshot(dec!(1871.20), &[review_leg(), other_leg(unanswered.clone())], &nov(dec!(81000)));
    assert_eq!(held, Err(Hold::LookupNeeded(oid(2))));
    // Not needed when a trade after B proves it, nor when it could not
    // protect anyway (another order is not excluded, or (c) fails).
    let mut traded = unanswered.clone();
    traded.fills = vec![sell(HOUR, dec!(0.0001), dec!(80000))];
    traded.orig_qty = dec!(0.0016);
    assert!(floor(&with(traded)) > Decimal::ZERO);
    let working = order(3, LegOrderKind::Sell, "NEW", dec!(0.0016));
    assert_eq!(floor(&leg_of(vec![entry(), unanswered.clone(), working])), Decimal::ZERO);
    let mut short = unanswered;
    short.orig_qty = dec!(0.0015);
    assert_eq!(floor(&with(short)), Decimal::ZERO);
}

#[test]
fn summarized_orders_count_in_g_b_only() {
    // A stop of 0.0006 that filled on 30 October, summarized by the reader;
    // the replacement stop on the remaining 0.0010 protects at B.
    let mut leg = with(stop(3, dec!(72000), dec!(0.0010)));
    leg.summarized_sold_before_b = dec!(0.0006);
    let carried = at_b(&leg, dec!(81000)).unwrap().unwrap();
    assert_eq!(carried.g_b, dec!(0.0010));
    assert_eq!(carried.floor, dec!(0.00092) * dec!(71928) * dec!(0.999));
    // Not counted, the stop's quantity would not match G_B.
    leg.summarized_sold_before_b = Decimal::ZERO;
    assert_eq!(floor(&leg), Decimal::ZERO);
    // Summarized with everything else sold: finished before B.
    leg.summarized_sold_before_b = dec!(0.0016);
    assert_eq!(at_b(&leg, dec!(81000)), Ok(None));
    // A negative summary could only raise G_B: refused.
    leg.summarized_sold_before_b = dec!(-0.0001);
    assert_eq!(at_b(&leg, dec!(81000)), Err(Hold::Invalid));
}

#[test]
fn inputs_outside_the_ledger_domain_hold_k() {
    // Every amount outside the ledger's domain (M36, M49) holds K: none can
    // raise a floor, a basis or a cash figure, or prove protection.
    let mut reserve = review_leg();
    reserve.terms.remainder_reserve = dec!(-0.00008);
    let mut charge = in_flight_leg(&[(20, dec!(0.00161))]);
    charge.terms.admission_charge = dec!(-19.70677056);
    let mut quantity = review_leg();
    quantity.orders[1].fills = vec![sell(-HOUR, dec!(-0.0001), dec!(72000))];
    let mut trigger = review_leg();
    trigger.orders[1].stop_price = Some(dec!(-72000));
    let mut gap = review_leg();
    gap.terms.gap_bps = dec!(-10);
    let mut fee = review_leg();
    fee.terms.taker_bps = dec!(10000);
    let mut write_off = review_leg();
    let close_at = valid(-HOUR);
    write_off.dust = Some(Dust {
        write_off: dec!(-0.00008),
        close_at,
        last_fill_at: at(-HOUR),
    });
    let mut summary = review_leg();
    summary.summarized_sold_before_b = dec!(-0.0001);
    let mut no_charge = in_flight_leg(&[(20, dec!(0.00161))]);
    no_charge.terms.admission_charge = Decimal::ZERO;
    // Code review r2's case: a sole stop ended EXPIRED, protected only by an
    // invalid trade at or after B.
    let invalid_after_b = |qty: Decimal, price: Decimal, offset: i64| {
        let mut ended = ended_stop("EXPIRED");
        ended.fills = vec![Fill {
            qty,
            price,
            ..sell(offset, dec!(1), dec!(1))
        }];
        with(ended)
    };
    for (name, leg) in [
        ("negative trade at B", invalid_after_b(dec!(-0.0016), dec!(72000), 0)),
        ("empty trade after B", invalid_after_b(Decimal::ZERO, dec!(72000), SEC)),
        ("free trade after B", invalid_after_b(dec!(0.0016), Decimal::ZERO, SEC)),
        ("entry trade without a price", {
            let mut leg = review_leg();
            leg.orders[0].fills[0].price = Decimal::ZERO;
            leg
        }),
        ("zero charge", no_charge),
        ("zero trigger", {
            let mut leg = review_leg();
            leg.orders[1].stop_price = Some(Decimal::ZERO);
            leg
        }),
    ] {
        assert_eq!(at_b(&leg, dec!(81000)), Err(Hold::Invalid), "{name}");
    }
    // The same stop with a valid trade at B is protected (control).
    assert!(floor(&invalid_after_b(dec!(0.0016), dec!(72000), 0)) > Decimal::ZERO);
    for (name, leg) in [
        ("reserve", reserve),
        ("charge", charge),
        ("quantity", quantity),
        ("trigger", trigger),
        ("gap", gap),
        ("fee", fee),
        ("write-off", write_off),
        ("summary", summary),
    ] {
        assert_eq!(at_b(&leg, dec!(81000)), Err(Hold::Invalid), "{name}");
    }
    let mut negative_x = review_leg();
    negative_x.executable = Some(dec!(-0.0016));
    assert_eq!(at_b(&negative_x, dec!(81000)), Err(Hold::Deficit(LEG)));
    // The public floor, cash and partition functions refuse them too.
    let free = LegTerms {
        gap_bps: dec!(0),
        taker_bps: dec!(0),
        ..terms()
    };
    let quote = |g, reserve| {
        let terms = LegTerms { remainder_reserve: reserve, ..free };
        floor_at(g, dec!(100), dec!(100), &terms).map(|floor| floor.quote)
    };
    assert_eq!(quote(dec!(1), dec!(-1)), Err(Hold::Invalid));
    assert_eq!(quote(dec!(-1), dec!(0)), Err(Hold::Invalid));
    assert_eq!(quote(dec!(1), dec!(0)), Ok(dec!(100)));
    assert_eq!(floor_at(dec!(1), Decimal::ZERO, dec!(100), &free), Err(Hold::Invalid));
    let mut rebate = review_leg();
    let fill = sell(HOUR, dec!(0.0016), dec!(72000));
    rebate.orders[1].fills = vec![Fill { commission: dec!(-1), ..fill }];
    let (from, to) = november();
    assert_eq!(delta_cash(&rebate.orders, from..to), Err(Hold::Invalid));
    for (qty, price) in [(dec!(1), Decimal::ZERO), (Decimal::ZERO, dec!(80000))] {
        let mut free = in_flight_leg(&[(20, dec!(0.00161))]);
        free.orders[0].fills[0] = Fill { qty, price, ..free.orders[0].fills[0] };
        assert_eq!(delta_cash(&free.orders, from..to), Err(Hold::Invalid), "{qty} {price}");
    }
    assert_eq!(month_net(dec!(-3.25), &[dec!(1), dec!(-1)]), Err(Hold::Invalid));
}

#[test]
fn facts_breaking_ledger_constraints_hold_k() {
    // Code review r4: facts no ledger row can hold (M36's keys and CHECKs).
    let mut live_but_seen_ended = order(3, LegOrderKind::Sell, "NEW", dec!(0.0016));
    live_but_seen_ended.terminal_seen = Some(valid(-HOUR));
    let mut ended_but_unseen = ended_stop("EXPIRED");
    ended_but_unseen.terminal_seen = None;
    let mut overfilled = ended_stop("EXPIRED");
    overfilled.fills = vec![sell(0, dec!(0.0032), dec!(72000))];
    let twice = leg_of(vec![
        entry(),
        stop(2, dec!(72000), dec!(0.0016)),
        ended_stop("EXPIRED"),
    ]);
    let cases = [
        (
            "a live order seen terminal",
            leg_of(vec![
                entry(),
                stop(2, dec!(72000), dec!(0.0016)),
                live_but_seen_ended,
            ]),
        ),
        ("a terminal order never seen terminal", with(ended_but_unseen)),
        ("trades beyond the order", with(overfilled)),
        ("an order twice", twice.clone()),
    ];
    for (name, leg) in cases {
        assert_eq!(at_b(&leg, dec!(81000)), Err(Hold::Invalid), "{name}");
        assert_eq!(
            delta_cash(&leg.orders, november().0..november().1),
            Err(Hold::Invalid),
            "{name}"
        );
    }
    // A leg twice in one snapshot.
    let held = snapshot(dec!(1871.20), &[review_leg(), review_leg()], &nov(dec!(81000)));
    assert_eq!(held, Err(Hold::Invalid));
    // N_K sums the excesses exactly before one subtraction: order-free.
    let tiny = dec!(0.0000000000000000000000000001);
    for realized in [[tiny, dec!(5)], [dec!(5), tiny]] {
        assert_eq!(month_net(dec!(10), &realized), Ok(dec!(4.9999999999999999999999999999)));
    }
    // The 2 s margins never overflow at chrono's limits: nothing is proven.
    for edge in [DateTime::<Utc>::MIN_UTC, DateTime::<Utc>::MAX_UTC] {
        let close_at = RobsonTime {
            at: edge,
            verdict: Some(ClockVerdict::Valid),
        };
        let dust = Dust {
            write_off: dec!(0.00008),
            close_at,
            last_fill_at: edge,
        };
        assert!(!write_off_before(&dust, DateTime::<Utc>::MIN_UTC));
    }
}

#[test]
fn contradictions_hold_k_and_sums_ignore_order() {
    // Code review r5: facts one ledger cannot hold together.
    let mut status_not_phase = order(3, LegOrderKind::Sell, "NEW", dec!(0.0016));
    status_not_phase.phase = OrderPhase::Terminal;
    status_not_phase.terminal_seen = Some(valid(-HOUR));
    let mut empty = order(3, LegOrderKind::Sell, "NEW", dec!(0.0016));
    empty.orig_qty = Decimal::ZERO;
    let mut late_but_acknowledged = order(3, LegOrderKind::Sell, "NEW", dec!(0.0016));
    late_but_acknowledged.first_attempt = Some(valid(HOUR));
    late_but_acknowledged.live_seen = valid(-HOUR);
    let mut ended_then_new = ended_stop("EXPIRED");
    ended_then_new.terminal_seen = Some(valid(-HOUR));
    ended_then_new.lookup = Some(Lookup {
        sent: valid(70 * SEC),
        status: "NEW".to_string(),
    });
    let mut ended_then_traded = ended_stop("FILLED");
    ended_then_traded.terminal_seen = Some(valid(-HOUR));
    ended_then_traded.fills = vec![sell(0, dec!(0.0016), dec!(72000))];
    let mut ended_then_cancelled = with(ended_stop("CANCELED"));
    ended_then_cancelled.orders[1].terminal_seen = Some(valid(-HOUR));
    ended_then_cancelled.cancels = vec![cancel(2, valid(60 * SEC))];
    let mut dust_before_a_fill = review_leg();
    let close_at = valid(-3 * SEC);
    let last_fill_at = at(-5 * DAY);
    dust_before_a_fill.dust = Some(Dust {
        write_off: dec!(0.00008),
        close_at,
        last_fill_at,
    });
    // Code review r6: a competing stop of another quantity seen terminal
    // before B yet NEW after it; a competing sell first attempted after B
    // that traded before B; X above the entry's acquisitions.
    let mut competing_stop = stop(3, dec!(70000), dec!(0.0008));
    competing_stop.terminal_seen = Some(valid(-HOUR));
    (competing_stop.status, competing_stop.phase) = ("EXPIRED".to_string(), OrderPhase::Terminal);
    let mut traded_before_attempt = order(3, LegOrderKind::Sell, "FILLED", dec!(0.0001));
    traded_before_attempt.first_attempt = Some(valid(HOUR));
    traded_before_attempt.live_seen = valid(HOUR + SEC);
    traded_before_attempt.fills = vec![sell(-HOUR, dec!(0.0001), dec!(80000))];
    // Code review r7: a competing stop seen terminal before B that Robson's
    // own cancel found open after B; a competing sell first attempted after
    // it was seen terminal.
    let mut competing_cancelled = ended_stop("CANCELED");
    competing_cancelled.exchange_order_id = oid(3);
    (competing_cancelled.orig_qty, competing_cancelled.terminal_seen) =
        (dec!(0.0008), Some(valid(-HOUR)));
    let mut cancelled_open = leg_of(vec![
        entry(),
        stop(2, dec!(72000), dec!(0.0016)),
        competing_cancelled,
    ]);
    cancelled_open.cancels = vec![cancel(3, valid(60 * SEC))];
    let mut attempted_after_end = order(3, LegOrderKind::Sell, "CANCELED", dec!(0.0016));
    attempted_after_end.terminal_seen = Some(valid(-HOUR));
    attempted_after_end.first_attempt = Some(valid(-HOUR + 2 * SEC));
    attempted_after_end.live_seen = valid(-HOUR + SEC);
    // Code review r8: acknowledged after it was seen terminal; Robson's
    // successful cancel attempted after the stop was seen terminal (both
    // after B).
    let mut acknowledged_after_end = order(3, LegOrderKind::Sell, "CANCELED", dec!(0.0016));
    acknowledged_after_end.first_attempt = Some(valid(-2 * HOUR));
    acknowledged_after_end.terminal_seen = Some(valid(-HOUR));
    acknowledged_after_end.live_seen = valid(HOUR);
    // Code review r9: a dust close proven before B, yet X − sells − R > 0.
    let mut closed_yet_held = review_leg();
    let (close_at, last_fill_at) = (valid(-3 * SEC), at(-4 * DAY));
    closed_yet_held.dust = Some(Dust {
        write_off: Decimal::ZERO,
        close_at,
        last_fill_at,
    });
    let mut cancelled_after_end = with(ended_stop("CANCELED"));
    cancelled_after_end.orders[1].terminal_seen = Some(valid(3 * SEC));
    cancelled_after_end.cancels = vec![cancel(2, valid(HOUR))];
    let mut beyond_acquisitions = review_leg();
    beyond_acquisitions.executable = Some(dec!(0.0017));
    beyond_acquisitions.orders[1].orig_qty = dec!(0.0017);
    let competing =
        |order: LegOrder| leg_of(vec![entry(), stop(2, dec!(72000), dec!(0.0016)), order]);
    for (name, leg) in [
        ("a status of another phase", competing(status_not_phase)),
        ("an empty order of no quantity", competing(empty.clone())),
        (
            "first attempted after B, acknowledged before it",
            competing(late_but_acknowledged),
        ),
        ("seen terminal before B, NEW after it", with(ended_then_new)),
        ("seen terminal before B, traded at B", with(ended_then_traded)),
        ("seen terminal before B, cancelled open after it", ended_then_cancelled),
        ("a trade after the dust row's last fill", dust_before_a_fill),
        (
            "a competing stop seen terminal before B, NEW after it",
            competing(competing_stop),
        ),
        (
            "a competing sell that traded before its first attempt",
            competing(traded_before_attempt),
        ),
        ("X above the entry's acquisitions", beyond_acquisitions),
        ("a competing stop ended before B, cancelled open after it", cancelled_open),
        ("a competing sell attempted after it ended", competing(attempted_after_end)),
        (
            "a competing sell acknowledged after it ended",
            competing(acknowledged_after_end),
        ),
        ("Robson's cancel found open a stop it saw ended", cancelled_after_end),
        ("a close proven before B that leaves inventory", closed_yet_held),
    ] {
        assert_eq!(at_b(&leg, dec!(81000)), Err(Hold::Invalid), "{name}");
    }
    let (from, to) = november();
    assert_eq!(delta_cash(&[empty], from..to), Err(Hold::Invalid));
    // delta_cash refuses an order seen terminal before one of its trades.
    let mut sold_after_ended = ended_stop("FILLED");
    sold_after_ended.terminal_seen = Some(valid(-HOUR));
    sold_after_ended.fills = vec![sell(0, dec!(0.0016), dec!(72000))];
    assert_eq!(delta_cash(&[sold_after_ended], from..to), Err(Hold::Invalid));
    // Sums are exact and independent of input order (ε = 10⁻²⁸).
    let epsilon = dec!(0.0000000000000000000000000001);
    let expected = dec!(-4.9999999999999999999999999999);
    for values in [[dec!(-10), epsilon, dec!(5)], [dec!(-10), dec!(5), epsilon]] {
        assert_eq!(floors::sum(values), Ok(expected));
    }
    let trade = |n: u64, kind, qty, price| {
        let mut order = order(n, kind, "FILLED", qty);
        order.fills = vec![Fill {
            trade_time: at(DAY),
            qty,
            price,
            commission: Decimal::ZERO,
        }];
        order.terminal_seen = Some(valid(DAY + SEC));
        order
    };
    let buy = trade(1, LegOrderKind::Entry, dec!(1), dec!(10));
    let tiny = trade(2, LegOrderKind::Sell, dec!(0.5), epsilon * dec!(2));
    let half = trade(3, LegOrderKind::Sell, dec!(0.5), dec!(10));
    for orders in [[buy.clone(), tiny.clone(), half.clone()], [buy, half, tiny]] {
        assert_eq!(delta_cash(&orders, from..to), Ok(expected));
    }
    let in_flight = |n: u128, charge: Decimal| {
        let mut leg = in_flight_leg(&[(20, dec!(0.00161))]);
        (leg.position_id, leg.terms.admission_charge) = (Uuid::from_u128(n), charge);
        leg
    };
    let (small, large) = (in_flight(7, epsilon), in_flight(8, dec!(5)));
    for legs in [[small.clone(), large.clone()], [large, small]] {
        let (_, totals) = snapshot(dec!(10), &legs, &nov(dec!(81000))).unwrap();
        assert_eq!(
            (totals.floors, totals.basis),
            (dec!(-5.0000000000000000000000000001), -expected)
        );
    }
}

#[test]
fn a_deficit_or_a_missing_x_keeps_k_pending() {
    let mut oversold = order(2, LegOrderKind::Sell, "FILLED", dec!(0.0017));
    oversold.fills = vec![sell(-HOUR, dec!(0.0017), dec!(80000))];
    assert_eq!(at_b(&with(oversold), dec!(81000)), Err(Hold::Deficit(LEG)));
    let mut no_x = review_leg();
    no_x.executable = None;
    assert_eq!(at_b(&no_x, dec!(81000)), Err(Hold::NoExecutable(LEG)));
}

#[test]
fn armed_and_current_cohort_legs_carry_nothing() {
    // SD16 (example 4): an Armed leg has no cohort, carries nothing and
    // deducts nothing: November's basis is October's untouched 2,000.00.
    let mut armed = review_leg();
    armed.cohort_month = None;
    armed.orders.clear();
    let (carried, totals) = snapshot(dec!(2000), &[armed], &nov(dec!(81000))).unwrap();
    assert!(carried.is_empty());
    assert_eq!(totals, BoundaryTotals {
        floors: dec!(0),
        downside: dec!(0),
        basis: dec!(2000)
    });
    // A leg of K's own cohort is not carried into K: its entry is
    // November's, attempted, acknowledged, traded and ended after B.
    let mut current = review_leg();
    current.cohort_month = Some(first(2026, 11));
    let entry = &mut current.orders[0];
    entry.first_attempt = Some(valid(DAY - 2 * SEC));
    entry.live_seen = valid(DAY - SEC);
    entry.fills[0].trade_time = at(DAY);
    entry.terminal_seen = Some(valid(DAY + SEC));
    assert_eq!(at_b(&current, dec!(81000)), Ok(None));
    // An entry that traded, or was only acknowledged, before B was handed
    // off before it (code review r6, r7): with no cohort, or K's, the facts
    // contradict each other.
    let mut working = review_leg();
    working.orders[0] = order(1, LegOrderKind::Entry, "NEW", dec!(0.00161));
    for cohort in [None, Some(first(2026, 11))] {
        let mut leg = in_flight_leg(&[(-1, dec!(0.000805)), (20, dec!(0.000805))]);
        leg.cohort_month = cohort;
        assert_eq!(at_b(&leg, dec!(81000)), Err(Hold::Invalid), "{cohort:?}");
        working.cohort_month = cohort;
        assert_eq!(at_b(&working, dec!(81000)), Err(Hold::Invalid), "working {cohort:?}");
    }
}

#[test]
fn a_result_at_or_above_its_floor_changes_no_consumption() {
    // SD15: N_K = cohort net − Σ CE_real; a close at or above F_B adds 0.
    let f_b = floor(&review_leg());
    let cohort_net = dec!(-3.25);
    let at_floor = realized_excess(f_b, f_b).unwrap();
    let above = realized_excess(f_b, dec!(115.00488)).unwrap();
    assert_eq!(month_net(cohort_net, &[at_floor, above]), Ok(cohort_net));
    // Example 3's realized excess is consumed by November.
    let gapped = realized_excess(f_b, dec!(96.7032)).unwrap();
    assert_eq!(month_net(cohort_net, &[above, gapped]), Ok(dec!(-15.76802944)));
}

#[test]
fn multi_boundary_leg_records_prior_month_excess() {
    // Carried into November; 0.0010 sold at 71,950 on 5 November by its stop,
    // which is cancelled; the rest re-protected that day and still open at
    // 1 December.
    let dec_b = b() + TimeDelta::days(30);
    let twice = |trigger: Decimal| {
        let mut first_stop = ended_stop("CANCELED");
        first_stop.fills = vec![sell(4 * DAY, dec!(0.0010), dec!(71950))];
        first_stop.terminal_seen = Some(valid(4 * DAY + SEC));
        let mut second = stop(3, trigger, dec!(0.0006));
        second.first_attempt = Some(valid(4 * DAY + 2 * SEC));
        second.live_seen = valid(4 * DAY + 3 * SEC);
        second.lookup = Some(Lookup {
            sent: valid(30 * DAY + 70 * SEC),
            status: "NEW".into(),
        });
        leg_of(vec![entry(), first_stop, second])
    };
    let december = Boundary::new(first(2026, 12), dec!(75000)).unwrap();
    for (trigger, f_next, prior) in [
        (dec!(72000), dec!(37.36515744), dec!(0)),
        (dec!(60000), dec!(31.1376312), dec!(6.20554824)),
    ] {
        let leg = twice(trigger);
        let in_november = at_b(&leg, dec!(81000)).unwrap().unwrap();
        assert_eq!(in_november.floor, dec!(109.22122944));
        assert_eq!(in_november.protection.unwrap().open_after_b, OpenAfterB::TradeAfterB);
        let in_december = leg_at_b(&leg, &december).unwrap().unwrap();
        assert_eq!((in_december.g_b, in_december.floor), (dec!(0.0006), f_next), "{trigger}");
        let cash = delta_cash(&leg.orders, b()..dec_b).unwrap();
        assert_eq!(cash, dec!(71.87805));
        assert_eq!(latent_excess(in_november.floor, cash, in_december.floor), Ok(prior));
    }
}

/// Every branch fixture above, at P_B = 81,000 unless it says otherwise.
fn branch_fixtures() -> Vec<CandidateLeg> {
    let mut cancelled = with(ended_stop("CANCELED"));
    cancelled.cancels = vec![cancel(2, valid(60 * SEC))];
    let mut expired_cancelled = with(ended_stop("EXPIRED"));
    expired_cancelled.cancels = vec![cancel(2, valid(60 * SEC))];
    let mut traded = ended_stop("FILLED");
    traded.fills = vec![sell(10 * 60 * SEC, dec!(0.0016), dec!(71990))];
    traded.terminal_seen = Some(valid(10 * 60 * SEC + SEC));
    let mut early = stop(2, dec!(72000), dec!(0.0016));
    early.live_seen = valid(-1999);
    let mut unanswered = stop(2, dec!(72000), dec!(0.0016));
    unanswered.lookup = None;
    let mut late = order(3, LegOrderKind::Sell, "NEW", dec!(0.0016));
    late.first_attempt = Some(valid(HOUR));
    late.live_seen = valid(HOUR + SEC);
    let mut ended = order(3, LegOrderKind::Sell, "CANCELED", dec!(0.0016));
    ended.terminal_seen = Some(valid(-HOUR));
    let working = order(3, LegOrderKind::Sell, "NEW", dec!(0.0016));
    let mut sold = order(2, LegOrderKind::Sell, "FILLED", dec!(0.00152));
    sold.fills = vec![sell(-HOUR, dec!(0.00152), dec!(81000))];
    let mut write_off = leg_of(vec![entry(), sold]);
    let close_at = valid(-SEC);
    write_off.dust = Some(Dust {
        write_off: dec!(0.00008),
        close_at,
        last_fill_at: at(-HOUR),
    });
    let mut entry_at_b = entry();
    entry_at_b.fills.push(buy(0, dec!(0.00001), dec!(80000)));
    entry_at_b.orig_qty = dec!(0.00162);
    entry_at_b.terminal_seen = Some(valid(70 * SEC));
    vec![
        review_leg(),
        with(traded),
        cancelled,
        expired_cancelled,
        with(ended_stop("EXPIRED")),
        with(early),
        with(unanswered),
        leg_of(vec![entry(), stop(2, dec!(72000), dec!(0.0016)), late]),
        leg_of(vec![entry(), stop(2, dec!(72000), dec!(0.0016)), ended]),
        leg_of(vec![entry(), stop(2, dec!(72000), dec!(0.0016)), working]),
        write_off,
        in_flight_leg(&[(20, dec!(0.00161))]),
        with_entry(entry_at_b),
    ]
}

#[test]
fn misleading_venue_times_change_nothing() {
    let ms = |offset: i64| Some(at(offset).timestamp_millis());
    let patterns = [
        VenueRecordTimes::default(),
        misleading(),
        VenueRecordTimes {
            time_ms: ms(-HOUR),
            update_time_ms: ms(-HOUR),
            transact_time_ms: ms(-HOUR),
            working_time_ms: ms(-HOUR),
        },
        VenueRecordTimes {
            time_ms: ms(HOUR),
            update_time_ms: ms(HOUR),
            transact_time_ms: ms(HOUR),
            working_time_ms: ms(HOUR),
        },
        VenueRecordTimes {
            time_ms: ms(0),
            update_time_ms: ms(-1),
            transact_time_ms: ms(0),
            working_time_ms: ms(-1),
        },
    ];
    for (index, fixture) in branch_fixtures().into_iter().enumerate() {
        let results: Vec<_> = patterns
            .iter()
            .map(|times| {
                let mut leg = fixture.clone();
                leg.orders.iter_mut().for_each(|order| order.venue_record_times = *times);
                at_b(&leg, dec!(81000))
            })
            .collect();
        assert!(
            results[0] != Err(Hold::Invalid) && results.iter().all(|result| *result == results[0]),
            "fixture {index}: {results:?}"
        );
    }
    // Source guard: production names the venue's times only in their type.
    let production = |source: &'static str| source.split("#[cfg(test)]").next().unwrap();
    let root = production(include_str!("../boundary_carry.rs"));
    let floors = production(include_str!("floors.rs"));
    assert_eq!(root.matches("venue_record_times").count(), 1);
    assert_eq!(root.matches("time_ms").count(), 4);
    assert!(!floors.contains("venue_record_times") && !floors.contains("time_ms"));
}
