//! Generated ledgers against an independent integer oracle (PLAN-r3 §8,
//! 8p-3). Each case is one candidate leg around B = 2026-11-01: an entry,
//! stops and sells with trades on both sides of B and of B_{K+1}, Robson's
//! cancels, the owner's lookups, a dust row, misleading venue times and
//! clock verdicts at the 2 s edges. The oracle states §C4 literally in
//! integers: milliseconds from B, lots of 10⁻⁵ BTC, cents, tenths of a basis
//! point and fees in 10⁻⁸ USDT.

use std::{cell::RefCell, collections::BTreeMap};

use chrono::{DateTime, NaiveDate, TimeDelta, Utc};
use proptest::{
    collection::vec,
    prelude::*,
    sample::select,
    test_runner::{Config, RngAlgorithm, TestRng, TestRunner},
};
use rust_decimal::Decimal;
use uuid::Uuid;

use super::*;

const LEG: Uuid = Uuid::from_u128(0x0000_8003_0000_4000_8000_0000_0000_00aa);
const OTHER_LEG: Uuid = Uuid::from_u128(0x0000_8003_0000_4000_8000_0000_0000_00bb);
const DAY: i64 = 86_400_000;
/// B_{K+1} − B for November.
const NEXT: i64 = 30 * DAY;
/// The edges the rules draw, around B and B_{K+1}.
const EDGES: [i64; 15] = [
    -DAY,
    -3000,
    -2001,
    -2000,
    -1999,
    -1,
    0,
    1,
    1999,
    2000,
    2001,
    3000,
    DAY,
    NEXT - 1,
    NEXT,
];

#[derive(Debug, Clone, Copy)]
struct Time {
    ms: i64,
    /// 0 valid, 1 invalid, 2 no evidence.
    verdict: u8,
}

#[derive(Debug, Clone, Copy)]
struct Trade {
    ms: i64,
    lots: i64,
    cents: i64,
    fee: i64,
}

#[derive(Debug, Clone)]
struct Order {
    stop: bool,
    status: &'static str,
    orig: i64,
    trigger: i64,
    live: Time,
    terminal: Option<Time>,
    attempt: Option<Time>,
    trades: Vec<Trade>,
    lookup: Option<(Time, &'static str)>,
    venue_ms: Option<i64>,
}

#[derive(Debug, Clone)]
struct Cancel {
    target: usize,
    own: bool,
    outcome: Option<&'static str>,
    class: Option<&'static str>,
    attempt: Option<Time>,
}

#[derive(Debug, Clone)]
struct Case {
    /// 0 none (Armed), 1 October, 2 November (K itself).
    cohort: u8,
    x: Option<i64>,
    entry: Option<(&'static str, Vec<Trade>, Time)>,
    /// When Robson first recorded the entry's acknowledgement.
    entry_live: Time,
    orders: Vec<Order>,
    cancels: Vec<Cancel>,
    dust: Option<(i64, Time, i64)>,
    /// Lots sold before B by orders the reader summarized.
    summarized: i64,
    gap: i64,
    fee: i64,
    reserve: i64,
    charge: i64,
    mark: i64,
}

/// True in `tenths` out of ten cases.
fn chance(tenths: u32) -> impl Strategy<Value = bool> {
    prop_oneof![tenths => Just(true), 10 - tenths => Just(false)]
}

/// `Some` in `tenths` out of ten cases.
fn sometimes<S: Strategy>(tenths: u32, inner: S) -> impl Strategy<Value = Option<S::Value>>
where
    S::Value: Clone,
{
    prop_oneof![tenths => inner.prop_map(Some), 10 - tenths => Just(None)]
}

fn ms() -> impl Strategy<Value = i64> {
    prop_oneof![3 => Just(-DAY), 3 => Just(DAY), 4 => select(EDGES.to_vec())]
}

fn time() -> impl Strategy<Value = Time> {
    let verdict = prop_oneof![8 => Just(0_u8), 1 => Just(1_u8), 1 => Just(2_u8)];
    (ms(), verdict).prop_map(|(ms, verdict)| Time { ms, verdict })
}

fn trade(ms: impl Strategy<Value = i64>) -> impl Strategy<Value = Trade> {
    (ms, 1..=60_i64, 5_000_000..=9_000_000_i64, 0..=20_000_i64)
        .prop_map(|(ms, lots, cents, fee)| Trade { ms, lots, cents, fee })
}

/// A Robson time recorded well before B with valid evidence in most cases.
fn early() -> impl Strategy<Value = Time> {
    prop_oneof![6 => Just(Time { ms: -DAY, verdict: 0 }), 4 => time()]
}

/// A first attempt well before every generated trade in most cases.
fn first() -> impl Strategy<Value = Time> {
    prop_oneof![6 => Just(Time { ms: -2 * DAY, verdict: 0 }), 4 => time()]
}

/// A terminal record after every generated trade in most cases.
fn ended_at() -> impl Strategy<Value = Time> {
    prop_oneof![4 => Just(Time { ms: NEXT + 1000, verdict: 0 }), 6 => time()]
}

/// A Robson time recorded after B + 2 s with valid evidence in most cases.
fn late() -> impl Strategy<Value = Time> {
    prop_oneof![6 => Just(Time { ms: 70_000, verdict: 0 }), 4 => time()]
}

fn order() -> impl Strategy<Value = Order> {
    let statuses = select(vec!["NEW", "NEW", "FILLED", "CANCELED", "CANCELED", "EXPIRED"]);
    let answers = select(vec!["NEW", "NEW", "CANCELED", "EXPIRED"]);
    let kind = (chance(7), statuses, 1..=200_i64, 5_000_000..=9_000_000_i64);
    let times = (early(), ended_at(), sometimes(8, first()));
    let rest = (vec(trade(ms()), 0..=2), sometimes(6, (late(), answers)), sometimes(5, ms()));
    (kind, times, rest).prop_map(
        |((stop, status, orig, trigger), (live, terminal, attempt), (trades, lookup, venue_ms))| {
            let ended = OrderPhase::from_binance_spot_status(status) == OrderPhase::Terminal;
            let terminal = ended.then_some(terminal);
            let lookup = lookup.filter(|_| stop);
            Order {
                stop,
                status,
                orig,
                trigger,
                live,
                terminal,
                attempt,
                trades,
                lookup,
                venue_ms,
            }
        },
    )
}

fn cancel() -> impl Strategy<Value = Cancel> {
    let target = prop_oneof![6 => Just(0_usize), 2 => Just(1_usize), 1 => Just(2_usize)];
    let outcome = prop_oneof![
        6 => Just(Some("success")),
        1 => Just(Some("rejected")),
        1 => Just(Some("gone_terminal")),
        1 => Just(None),
    ];
    let class = prop_oneof![6 => Just(Some("cancelled")), 1 => Just(Some("gone_terminal")), 1 => Just(None)];
    (target, chance(9), outcome, class, sometimes(9, late())).prop_map(
        |(target, own, outcome, class, attempt)| Cancel { target, own, outcome, class, attempt },
    )
}

/// Lots sold before B by listed orders and the summary, and the write-off
/// when its close is proven before B: shaping only, the oracle decides.
fn shaping_g(case: &Case) -> i64 {
    let sold: i64 = case.orders.iter().map(|o| lots_before(&o.trades)).sum();
    let written = match case.dust {
        Some((r, close, last)) if seen_before(close) && last < 0 => r,
        _ => 0,
    };
    case.x.unwrap_or(0) - sold - case.summarized - written
}

fn case() -> impl Strategy<Value = Case> {
    let entry_ms = prop_oneof![9 => Just(-DAY), 1 => select(EDGES.to_vec())];
    let entry_status = select(vec![
        "FILLED", "FILLED", "FILLED", "FILLED", "FILLED", "CANCELED", "EXPIRED", "NEW",
    ]);
    let entry = sometimes(9, (entry_status, vec(trade(entry_ms), 1..=2), early()));
    let cohort = prop_oneof![18 => Just(1_u8), 1 => Just(0_u8), 1 => Just(2_u8)];
    let x = prop_oneof![19 => (20..=400_i64).prop_map(Some), 1 => Just(None)];
    let terms = (0..=500_i64, 0..=200_i64, 0..=12_i64, 1..=5_000_i64, 5_000_000..=9_000_000_i64);
    let orders =
        prop_oneof![1 => vec(order(), 0..=0), 6 => vec(order(), 1..=1), 3 => vec(order(), 2..=3)];
    let cancels = prop_oneof![5 => vec(cancel(), 0..=0), 5 => vec(cancel(), 1..=2)];
    let last = prop_oneof![5 => Just(NEXT + 1000), 5 => ms()];
    let dust = sometimes(3, (0..=10_i64, time(), last));
    let summarized = prop_oneof![7 => Just(0_i64), 3 => 1..=60_i64];
    let shape = (vec(chance(7), 3), chance(1), chance(8));
    let entry_live = prop_oneof![6 => Just(Time { ms: -DAY - 1000, verdict: 0 }), 4 => time()];
    let parts = ((cohort, x, entry, entry_live), orders, cancels, dust, summarized);
    (parts, terms, shape).prop_map(|(parts, terms, (matches, finish, within))| {
        let ((cohort, x, entry, entry_live), orders, cancels, dust, summarized) = parts;
        let (gap, fee, reserve, charge, mark) = terms;
        let mut case = Case {
            cohort,
            x,
            entry,
            entry_live,
            orders,
            cancels,
            dust,
            summarized,
            gap,
            fee,
            reserve,
            charge,
            mark,
        };
        // Shape most X within the entry's acquisitions, some legs to be
        // finished before B and most stops' quantities to the leg's open
        // quantity, so that (c) is often met.
        let bought = case
            .entry
            .as_ref()
            .map_or(0, |(_, trades, _)| trades.iter().map(|t| t.lots).sum());
        if let Some(x) = case.x.as_mut().filter(|_| within && bought > 0) {
            *x = (*x).min(bought);
        }
        let g = shaping_g(&case);
        if finish && g > 0 {
            case.summarized += g;
        }
        let g = shaping_g(&case);
        for (order, matches) in case.orders.iter_mut().zip(matches) {
            let own = lots_before(&order.trades);
            if matches && g + own > 0 {
                order.orig = g + own;
            }
        }
        case
    })
}

fn b() -> DateTime<Utc> {
    "2026-11-01T00:00:00Z".parse().unwrap()
}

fn at(ms: i64) -> DateTime<Utc> {
    b() + TimeDelta::milliseconds(ms)
}

fn robson(time: Time) -> RobsonTime {
    let verdict = [Some(ClockVerdict::Valid), Some(ClockVerdict::Invalid), None];
    RobsonTime {
        at: at(time.ms),
        verdict: verdict[usize::from(time.verdict)],
    }
}

fn oid(index: usize) -> VenueIntId {
    (index as u64 + 2).to_string().parse().unwrap()
}

fn lots(lots: i64) -> Decimal {
    Decimal::new(lots, 5)
}

fn fill(trade: &Trade) -> Fill {
    let (qty, price) = (lots(trade.lots), Decimal::new(trade.cents, 2));
    Fill {
        trade_time: at(trade.ms),
        qty,
        price,
        commission: Decimal::new(trade.fee, 8),
    }
}

/// The case as 8p-4's reader would hand it over.
fn production(case: &Case) -> (CandidateLeg, Boundary) {
    let mut orders = Vec::new();
    if let Some((status, trades, seen)) = &case.entry {
        let phase = OrderPhase::from_binance_spot_status(status);
        orders.push(LegOrder {
            exchange_order_id: "1".parse().unwrap(),
            kind: LegOrderKind::Entry,
            status: status.to_string(),
            phase,
            orig_qty: lots(1_000),
            stop_price: None,
            live_seen: robson(case.entry_live),
            terminal_seen: (phase == OrderPhase::Terminal).then_some(robson(*seen)),
            first_attempt: Some(robson(Time { ms: -DAY - 2000, verdict: 0 })),
            fills: trades.iter().map(fill).collect(),
            lookup: None,
            venue_record_times: VenueRecordTimes::default(),
        });
    }
    for (index, order) in case.orders.iter().enumerate() {
        orders.push(LegOrder {
            exchange_order_id: oid(index),
            kind: if order.stop {
                LegOrderKind::Stop
            } else {
                LegOrderKind::Sell
            },
            status: order.status.to_string(),
            phase: OrderPhase::from_binance_spot_status(order.status),
            orig_qty: lots(order.orig),
            stop_price: order.stop.then_some(Decimal::new(order.trigger, 2)),
            live_seen: robson(order.live),
            terminal_seen: order.terminal.map(robson),
            first_attempt: order.attempt.map(robson),
            fills: order.trades.iter().map(fill).collect(),
            lookup: order.lookup.map(|(sent, status)| Lookup {
                sent: robson(sent),
                status: status.to_string(),
            }),
            venue_record_times: VenueRecordTimes {
                time_ms: order.venue_ms,
                update_time_ms: order.venue_ms.map(|ms| ms - 3_600_000),
                transact_time_ms: order.venue_ms,
                working_time_ms: order.venue_ms.map(|ms| ms + 3_600_000),
            },
        });
    }
    let cancels = case
        .cancels
        .iter()
        .map(|c| CancelIntent {
            position_id: if c.own { LEG } else { OTHER_LEG },
            cancels_order_id: oid(c.target),
            venue_outcome: c.outcome.map(String::from),
            cancel_class: c.class.map(String::from),
            first_attempt: c.attempt.map(robson),
        })
        .collect();
    let month = |m| NaiveDate::from_ymd_opt(2026, m, 1).unwrap();
    let leg = CandidateLeg {
        position_id: LEG,
        cohort_month: [None, Some(month(10)), Some(month(11))][usize::from(case.cohort)],
        executable: case.x.map(lots),
        terms: LegTerms {
            gap_bps: Decimal::new(case.gap, 1),
            taker_bps: Decimal::new(case.fee, 1),
            remainder_reserve: lots(case.reserve),
            admission_charge: Decimal::new(case.charge, 2),
        },
        orders,
        summarized_sold_before_b: lots(case.summarized),
        cancels,
        dust: case.dust.map(|(write_off, close, last)| Dust {
            write_off: lots(write_off),
            close_at: robson(close),
            last_fill_at: at(last),
        }),
    };
    (leg, Boundary::new(month(11), Decimal::new(case.mark, 2)).unwrap())
}

#[derive(Debug, Clone, PartialEq)]
enum Outcome {
    Nothing(&'static str),
    InFlight,
    Inventory {
        g: i64,
        stop: Option<(usize, OpenAfterB)>,
    },
    Held(Hold),
}

fn proven(t: Time) -> bool {
    t.verdict == 0
}

/// Robson saw it by B − 2 s.
fn seen_before(t: Time) -> bool {
    proven(t) && t.ms <= -2000
}

/// Robson sent or saw it from B + 2 s.
fn seen_after(t: Time) -> bool {
    proven(t) && t.ms >= 2000
}

fn ended(status: &str) -> bool {
    status != "NEW"
}

fn lots_before(trades: &[Trade]) -> i64 {
    trades.iter().filter(|t| t.ms < 0).map(|t| t.lots).sum()
}

/// §C4 in integers: exactly one stop satisfies (a), (b) and (c), and every
/// other sell-capable order is shown not open at B.
/// One possible order of an order's times, by the 2 s margin: attempted
/// no later than acknowledged and than any trade; seen terminal no earlier
/// than any trade and than a valid lookup that answered NEW.
fn in_order(
    live: Time,
    attempt: Option<Time>,
    terminal: Option<Time>,
    trades: &[Trade],
    lookup: Option<(Time, &str)>,
) -> bool {
    let attempted_after = |ms: i64| attempt.is_some_and(|a| proven(a) && a.ms >= ms + 2000);
    let ended_before = |ms: i64| terminal.is_some_and(|t| proven(t) && t.ms <= ms - 2000);
    let new_after_end = lookup
        .is_some_and(|(sent, status)| status == "NEW" && proven(sent) && ended_before(sent.ms));
    let attempted_after_end = terminal.is_some_and(|t| proven(t) && attempted_after(t.ms));
    !(proven(live) && (attempted_after(live.ms) || ended_before(live.ms)))
        && !attempted_after_end
        && trades.iter().all(|t| !attempted_after(t.ms) && !ended_before(t.ms))
        && !new_after_end
}

fn oracle(case: &Case) -> Outcome {
    let entry_trades =
        case.entry.as_ref().map(|(_, trades, _)| trades.as_slice()).unwrap_or_default();
    if case.cohort != 1 {
        // An entry acknowledged or traded before B had a cohort before K.
        let acknowledged = case.entry.is_some() && seen_before(case.entry_live);
        if acknowledged || entry_trades.iter().any(|t| t.ms < 0) {
            return Outcome::Held(Hold::Invalid);
        }
        return Outcome::Nothing("no_cohort_before_k");
    }
    // M36: an order's trades never exceed its quantity; every order's times
    // in one possible order; no trade after the dust row's last fill.
    let entry_ok = case.entry.as_ref().is_none_or(|(status, trades, seen)| {
        let attempt = Time { ms: -DAY - 2000, verdict: 0 };
        in_order(case.entry_live, Some(attempt), ended(status).then_some(*seen), trades, None)
    });
    let orders_ok = case.orders.iter().all(|o| {
        o.trades.iter().map(|t| t.lots).sum::<i64>() <= o.orig
            && in_order(o.live, o.attempt, o.terminal, &o.trades, o.lookup)
    });
    let mut every_trade = entry_trades.iter().chain(case.orders.iter().flat_map(|o| &o.trades));
    let dust_ok = case.dust.is_none_or(|(_, _, last)| every_trade.all(|t| t.ms <= last));
    if !(entry_ok && orders_ok && dust_ok) {
        return Outcome::Held(Hold::Invalid);
    }
    let complete = case.entry.as_ref().is_some_and(|(status, trades, seen)| {
        ended(status)
            && trades.iter().all(|t| t.ms < 0)
            && (*status == "FILLED" || seen_before(*seen))
    });
    if !complete {
        return Outcome::InFlight;
    }
    let Some(x) = case.x else {
        return Outcome::Held(Hold::NoExecutable(LEG));
    };
    if x > entry_trades.iter().map(|t| t.lots).sum::<i64>() {
        return Outcome::Held(Hold::Invalid);
    }
    let sold = case.orders.iter().map(|o| lots_before(&o.trades)).sum::<i64>() + case.summarized;
    let written = match case.dust {
        Some((r, close, last)) if seen_before(close) && last < 0 => r,
        _ => 0,
    };
    let g = x - sold - written;
    if g < 0 {
        return Outcome::Held(Hold::Deficit(LEG));
    }
    // A close proven before B leaves no inventory.
    let closed = case.dust.is_some_and(|(_, close, last)| seen_before(close) && last < 0);
    if closed && g != 0 {
        return Outcome::Held(Hold::Invalid);
    }
    if g == 0 {
        return Outcome::Nothing("finished_before_b");
    }
    let shut: Vec<bool> = case
        .orders
        .iter()
        .map(|o| {
            o.attempt.is_some_and(seen_after)
                || (o.terminal.is_some_and(seen_before) && o.trades.iter().all(|t| t.ms < 0))
        })
        .collect();
    let alone = |i: usize| shut.iter().enumerate().all(|(j, shut)| j == i || *shut);
    let a_and_c = |o: &Order| o.stop && seen_before(o.live) && o.orig - lots_before(&o.trades) == g;
    let open_after = |i: usize, o: &Order| -> Option<Result<OpenAfterB, Hold>> {
        let robson_cancel = case.cancels.iter().any(|c| {
            c.own
                && c.target == i
                && c.outcome == Some("success")
                && c.class == Some("cancelled")
                && c.attempt.is_some_and(seen_after)
        });
        if o.trades.iter().any(|t| t.ms >= 0) {
            Some(Ok(OpenAfterB::TradeAfterB))
        } else if robson_cancel && o.status == "CANCELED" {
            Some(Ok(OpenAfterB::RobsonCancelAfterB))
        } else if o.lookup.is_some_and(|(sent, status)| status == "NEW" && seen_after(sent)) {
            Some(Ok(OpenAfterB::NewAtCompletion))
        } else if robson_cancel && !ended(o.status) {
            Some(Err(Hold::TerminalRecordNeeded(oid(i))))
        } else if o.lookup.is_none() && !ended(o.status) {
            Some(Err(Hold::LookupNeeded(oid(i))))
        } else {
            None
        }
    };
    let candidates: Vec<(usize, &Order)> =
        case.orders.iter().enumerate().filter(|(_, o)| a_and_c(o)).collect();
    // Any order Robson's own successful cancel found open 2 s or more after
    // Robson saw it terminal.
    let contradicted = |(i, o): (usize, &Order)| {
        let ended = o.terminal.filter(|t| proven(*t));
        case.cancels.iter().any(|c| {
            c.own
                && c.target == i
                && c.outcome == Some("success")
                && c.class == Some("cancelled")
                && c.attempt
                    .is_some_and(|a| proven(a) && ended.is_some_and(|t| a.ms >= t.ms + 2000))
        })
    };
    if case.orders.iter().enumerate().any(contradicted) {
        return Outcome::Held(Hold::Invalid);
    }
    let satisfying: Vec<(usize, OpenAfterB)> = candidates
        .iter()
        .filter_map(|&(i, o)| open_after(i, o).and_then(Result::ok).map(|branch| (i, branch)))
        .collect();
    match satisfying.as_slice() {
        [(i, branch)] if alone(*i) => Outcome::Inventory { g, stop: Some((*i, *branch)) },
        [] => candidates
            .iter()
            .filter(|(i, _)| alone(*i))
            .find_map(|&(i, o)| open_after(i, o).and_then(Result::err))
            .map_or(Outcome::Inventory { g, stop: None }, Outcome::Held),
        _ => Outcome::Inventory { g, stop: None },
    }
}

fn units(value: i128, scale: u32) -> Decimal {
    Decimal::from_i128_with_scale(value, scale)
}

/// The oracle's figures: F_B at scale 17 (lots 5 + cents 2 + two
/// complements 5 + 5), G_B × P_B at scale 7.
fn expected(case: &Case, outcome: &Outcome) -> Result<Option<CarriedLeg>, Hold> {
    let charge = i128::from(case.charge);
    let (g, stop) = match outcome {
        Outcome::Nothing(_) => return Ok(None),
        Outcome::Held(hold) => return Err(hold.clone()),
        Outcome::InFlight => {
            return Ok(Some(CarriedLeg {
                position_id: LEG,
                g_b: Decimal::ZERO,
                protection: None,
                floor: units(-charge, 2),
                mark_value: Decimal::ZERO,
                downside: units(charge, 2),
            }))
        },
        Outcome::Inventory { g, stop } => (i128::from(*g), *stop),
    };
    let off_gap = i128::from(100_000 - case.gap);
    let mut floor = 0;
    let protection = stop.map(|(i, open_after_b)| {
        let trigger = i128::from(case.orders[i].trigger);
        let adverse = trigger * off_gap;
        let price = adverse.min(i128::from(case.mark) * off_gap);
        let held = (g - i128::from(case.reserve)).max(0);
        let quote = held * price * i128::from(100_000 - case.fee);
        floor = quote;
        Protection {
            stop: oid(i),
            trigger: units(trigger, 2),
            open_after_b,
            floor: Floor {
                adverse_fill: units(adverse, 7),
                price: units(price, 7),
                quote: units(quote, 17),
            },
        }
    });
    let mark_value = g * i128::from(case.mark);
    Ok(Some(CarriedLeg {
        position_id: LEG,
        g_b: units(g, 5),
        protection,
        floor: units(floor, 17),
        mark_value: units(mark_value, 7),
        downside: units(mark_value * 10_i128.pow(10) - floor, 17),
    }))
}

/// Δcash in K at scale 8: sells add q × p less the fee, buys subtract q × p.
fn cash_in_k(case: &Case) -> i128 {
    let in_k = |t: &&Trade| (0..NEXT).contains(&t.ms);
    let quote = |t: &Trade| i128::from(t.lots) * i128::from(t.cents) * 10;
    let buys: i128 = case
        .entry
        .iter()
        .flat_map(|(_, trades, _)| trades)
        .filter(in_k)
        .map(quote)
        .sum();
    let sells: i128 = case
        .orders
        .iter()
        .flat_map(|o| &o.trades)
        .filter(in_k)
        .map(|t| quote(t) - i128::from(t.fee))
        .sum();
    sells - buys
}

fn label(outcome: &Outcome) -> &'static str {
    match outcome {
        Outcome::Nothing(reason) => reason,
        Outcome::InFlight => "entry_in_flight",
        Outcome::Held(Hold::LookupNeeded(_)) => "held_lookup",
        Outcome::Held(Hold::TerminalRecordNeeded(_)) => "held_terminal_record",
        Outcome::Held(Hold::Deficit(_)) => "held_deficit",
        Outcome::Held(Hold::NoExecutable(_)) => "held_no_executable",
        Outcome::Held(Hold::Invalid) => "held_invalid",
        Outcome::Inventory { stop: None, .. } => "unprotected",
        Outcome::Inventory {
            stop: Some((_, OpenAfterB::NewAtCompletion)),
            ..
        } => "protected_b1",
        Outcome::Inventory {
            stop: Some((_, OpenAfterB::TradeAfterB)), ..
        } => "protected_b2",
        Outcome::Inventory {
            stop: Some((_, OpenAfterB::RobsonCancelAfterB)),
            ..
        } => "protected_b3",
    }
}

#[test]
fn generated_ledgers_match_the_integer_oracle() {
    let config = Config {
        cases: 20_000,
        failure_persistence: None,
        ..Config::default()
    };
    let rng = TestRng::deterministic_rng(RngAlgorithm::ChaCha);
    let mut runner = TestRunner::new_with_rng(config, rng);
    let seen = RefCell::new(BTreeMap::<&'static str, u32>::new());
    runner
        .run(&case(), |case| {
            let outcome = oracle(&case);
            *seen.borrow_mut().entry(label(&outcome)).or_default() += 1;
            let (leg, k) = production(&case);
            let actual = leg_at_b(&leg, &k);
            prop_assert_eq!(&actual, &expected(&case, &outcome));
            if let Ok(Some(carried)) = actual {
                // Δcash over [B, B_{K+1}) and both carried excesses.
                let cash = cash_in_k(&case);
                let delta = delta_cash(&leg.orders, b()..at(NEXT));
                prop_assert_eq!(delta, Ok(units(cash, 8)));
                let f_b = carried.floor.mantissa() * 10_i128.pow(17 - carried.floor.scale());
                let short = f_b - cash * 10_i128.pow(9);
                let realized = realized_excess(carried.floor, units(cash, 8));
                prop_assert_eq!(realized, Ok(units(short.max(0), 17)));
                let f_now = i128::from(case.mark);
                let latent = latent_excess(carried.floor, units(cash, 8), units(f_now, 7));
                prop_assert_eq!(latent, Ok(units((short - f_now * 10_i128.pow(10)).max(0), 17)));
            }
            Ok(())
        })
        .unwrap();
    let seen = seen.into_inner();
    println!("GENERATED_BOUNDARY_LEGS {seen:?}");
    for label in [
        "no_cohort_before_k",
        "entry_in_flight",
        "finished_before_b",
        "held_deficit",
        "held_invalid",
        "held_no_executable",
        "held_lookup",
        "held_terminal_record",
        "unprotected",
        "protected_b1",
        "protected_b2",
        "protected_b3",
    ] {
        assert!(seen.get(label).copied().unwrap_or(0) >= 10, "{label} under-covered: {seen:?}");
    }
}
