//! ADR-0056 step 8p-3: what a leg carried across a month boundary B is as of
//! B, and its floor (design v16 §C2, §C4, §C6, §C7; PLAN-r3 §6.3). Pure: no
//! database, clock or venue call, and no production caller (8p-5 is the
//! first).
//!
//! Every fact is cut by venue trade time, half-open: a fill with `v < B`
//! belongs to the month that ends at B, one with `v ≥ B` to month K. A Robson
//! time proves something only in the sound direction, by the 2 s margin, and
//! only with a stored `valid` clock verdict. The venue's own order times are
//! carried as recorded and read by no decision (v16 §C4, R9-4).
//!
//! The inputs are 8p-4's leg facts, read after completion has established
//! that the venue is settled (8l-2's barrier): every order's trades are
//! booked, fees are final and no order is unknown.

use chrono::{DateTime, Datelike, NaiveDate, NaiveTime, TimeDelta, Utc};
use robson_domain::PositionId;
use rust_decimal::Decimal;
use serde::Deserialize;

use crate::{
    venue::OrderPhase,
    venue_ledger::{exact_sub, ClockVerdict, LegOrderKind, VenueIntId},
};

mod floors;
use floors::sum;
pub use floors::{delta_cash, floor_at, latent_excess, month_net, realized_excess, Floor};

/// Covers the clock bound u (at most 1 s) of every Robson time used here.
const MARGIN: TimeDelta = TimeDelta::seconds(2);

/// Why month K stays pending: completion must not proceed and the next poll
/// retries (v16 §C4, §C6; PLAN-r3 §6.12.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hold {
    /// The stop's protection can only rest on the owner's lookup (b1), not
    /// answered yet.
    LookupNeeded(VenueIntId),
    /// Robson's cancel found the stop open, but its terminal venue record is
    /// not booked yet, so its final status is unknown (b3).
    TerminalRecordNeeded(VenueIntId),
    /// The leg sold more than it held before B.
    Deficit(PositionId),
    /// A complete entry without an executable quantity X.
    NoExecutable(PositionId),
    /// The facts break the ledger's own constraints (M36, M49: an amount
    /// outside its domain, trades beyond their order, a terminal time without
    /// a terminal phase, an identity twice), or a result is not exactly
    /// representable.
    Invalid,
}

/// A time Robson recorded, with its clock evidence's verdict. `None` is NULL
/// evidence and proves nothing, as an `invalid` verdict (v16 §C4, R11-2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RobsonTime {
    pub at: DateTime<Utc>,
    pub verdict: Option<ClockVerdict>,
}

impl RobsonTime {
    /// Recorded at or before `b − 2 s` with valid evidence: it happened
    /// before `b`.
    fn before(self, b: DateTime<Utc>) -> bool {
        self.verdict == Some(ClockVerdict::Valid) && b.signed_duration_since(self.at) >= MARGIN
    }

    /// Recorded at or after `b + 2 s` with valid evidence: it happened at or
    /// after `b`.
    fn after(self, b: DateTime<Utc>) -> bool {
        self.verdict == Some(ClockVerdict::Valid) && self.at.signed_duration_since(b) >= MARGIN
    }
}

/// One venue trade of the leg's order (`order_fills`); `trade_time` is the
/// venue's. `commission` is a sell's quote fee; a buy's base fee is already
/// out of X.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fill {
    pub trade_time: DateTime<Utc>,
    pub qty: Decimal,
    pub price: Decimal,
    pub commission: Decimal,
}

/// The venue's own times of an order record (`venue_*_ms`), as recorded.
/// No decision reads them (v16 §C4, R9-4); 8p-4's reader leaves them empty.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VenueRecordTimes {
    pub time_ms: Option<i64>,
    pub update_time_ms: Option<i64>,
    pub transact_time_ms: Option<i64>,
    pub working_time_ms: Option<i64>,
}

/// The owner's lookup of a stop (§C4 (b1)): its send time and the status the
/// venue answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lookup {
    pub sent: RobsonTime,
    pub status: String,
}

/// One order of the leg (`venue_orders`), with its intent's first attempt
/// (`spot_intent_clock`), its trades and, for a stop, the owner's lookup.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LegOrder {
    pub exchange_order_id: VenueIntId,
    pub kind: LegOrderKind,
    /// The venue's status, as recorded (`NEW`, `FILLED`, `CANCELED`, ...).
    pub status: String,
    pub phase: OrderPhase,
    pub orig_qty: Decimal,
    pub stop_price: Option<Decimal>,
    pub live_seen: RobsonTime,
    pub terminal_seen: Option<RobsonTime>,
    pub first_attempt: Option<RobsonTime>,
    pub fills: Vec<Fill>,
    /// Never read from the ledger: 8p-5 attaches the owner's answer.
    #[serde(skip)]
    pub lookup: Option<Lookup>,
    /// Never read from the ledger (v16 §C4, R9-4).
    #[serde(skip)]
    pub venue_record_times: VenueRecordTimes,
}

impl LegOrder {
    /// The ledger's constraints on an order (M36): positive quantity and
    /// trigger; trades of positive quantity and price, fees ≥ 0, in all at
    /// most the order's quantity; the phase of its status; a terminal time
    /// exactly when terminal; and a possible chronology.
    fn well_formed(&self) -> bool {
        let valid = |f: &Fill| f.qty.min(f.price) > Decimal::ZERO && f.commission >= Decimal::ZERO;
        let traded = sum(self.fills.iter().map(|f| f.qty)).is_ok_and(|q| q <= self.orig_qty);
        self.orig_qty > Decimal::ZERO
            && self.fills.iter().all(valid)
            && traded
            && OrderPhase::from_binance_spot_status(&self.status) == self.phase
            && self.stop_price.is_none_or(|t| t > Decimal::ZERO)
            && self.terminal_seen.is_some() == (self.phase == OrderPhase::Terminal)
            && self.chronological()
    }

    /// Robson's valid times and the venue's trade times in one possible order
    /// (by the 2 s margin): first attempted no later than acknowledged, seen
    /// terminal and any trade; seen terminal no earlier than acknowledged, any
    /// trade and a lookup that answered `NEW`.
    fn chronological(&self) -> bool {
        let proven = |t: &RobsonTime| t.verdict == Some(ClockVerdict::Valid);
        let attempted_after = |at| self.first_attempt.is_some_and(|t| t.after(at));
        let ended_before = |at| self.terminal_seen.is_some_and(|t| t.before(at));
        let new = self.lookup.as_ref().filter(|l| l.status == "NEW" && proven(&l.sent));
        let in_life = |f: &Fill| !attempted_after(f.trade_time) && !ended_before(f.trade_time);
        let live = Some(self.live_seen).filter(proven);
        live.is_none_or(|l| !attempted_after(l.at) && !ended_before(l.at))
            && self.terminal_seen.filter(proven).is_none_or(|t| !attempted_after(t.at))
            && self.fills.iter().all(in_life)
            && new.is_none_or(|l| !ended_before(l.sent.at))
    }
}

/// One of Robson's cancel intents of the leg (`order_intents`, kind
/// `cancel`), with its first attempt's time.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CancelIntent {
    pub position_id: PositionId,
    pub cancels_order_id: VenueIntId,
    pub venue_outcome: Option<String>,
    pub cancel_class: Option<String>,
    pub first_attempt: Option<RobsonTime>,
}

/// The leg's dust row (`dust_ledger`): the write-off R and its close.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dust {
    pub write_off: Decimal,
    pub close_at: RobsonTime,
    /// Venue time of the leg's last fill.
    pub last_fill_at: DateTime<Utc>,
}

/// The current admission's terms (`balance_leg_admissions`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LegTerms {
    pub gap_bps: Decimal,
    pub taker_bps: Decimal,
    /// R̂, in the base asset.
    pub remainder_reserve: Decimal,
    /// a, in the quote asset.
    pub admission_charge: Decimal,
}

/// A candidate leg of month K (§6.4): its cohort, X, terms, entry and
/// sell-capable orders, Robson's cancels of them and its dust row.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateLeg {
    pub position_id: PositionId,
    pub cohort_month: Option<NaiveDate>,
    /// X (`balance_legs.executable_qty`), set by the entry partition.
    pub executable: Option<Decimal>,
    pub terms: LegTerms,
    pub orders: Vec<LegOrder>,
    /// The executed quantity before B of the sell-capable orders the reader
    /// summarized instead of listing (§6.4): only orders §6.3's exclusion
    /// rule already shows not open at B. They count in G_B and nowhere else.
    pub summarized_sold_before_b: Decimal,
    pub cancels: Vec<CancelIntent>,
    pub dust: Option<Dust>,
}

/// Month K and its validated boundary mark P_B.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Boundary {
    month: NaiveDate,
    mark: Decimal,
}

impl Boundary {
    /// `None` unless `month` is a first day and the mark is positive.
    pub fn new(month: NaiveDate, mark: Decimal) -> Option<Self> {
        (month.day() == 1 && mark > Decimal::ZERO).then_some(Self { month, mark })
    }

    /// B, the first instant of month K (UTC).
    pub fn instant(&self) -> DateTime<Utc> {
        self.month.and_time(NaiveTime::MIN).and_utc()
    }
}

/// The post-B evidence that showed the protecting stop open after B, hence
/// at B (an order's status only moves forward).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenAfterB {
    /// (b1) the owner's lookup, sent at or after B + 2 s, answered `NEW`.
    NewAtCompletion,
    /// (b2) a trade of the stop at or after B.
    TradeAfterB,
    /// (b3) Robson's own cancel, first attempted at or after B + 2 s, found it
    /// open: answered Cancelled and its terminal record is `CANCELED`.
    RobsonCancelAfterB,
}

/// The one stop proven in force at B, and its floor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Protection {
    pub stop: VenueIntId,
    pub trigger: Decimal,
    pub open_after_b: OpenAfterB,
    pub floor: Floor,
}

/// A leg carried into K: G_B, its protection at B, F_B, G_B × P_B and D_B.
/// G_B is 0 only for an entry in flight (entry not complete before B, F_B =
/// −a); otherwise the leg holds inventory at B.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CarriedLeg {
    pub position_id: PositionId,
    pub g_b: Decimal,
    pub protection: Option<Protection>,
    pub floor: Decimal,
    pub mark_value: Decimal,
    pub downside: Decimal,
}

/// The legs carried into K and the revision's totals.
pub type Snapshot = (Vec<CarriedLeg>, BoundaryTotals);

/// Revision totals: Σ F_B, Σ D_B and `basis = quote_B + Σ F_B`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoundaryTotals {
    pub floors: Decimal,
    pub downside: Decimal,
    pub basis: Decimal,
}

/// The legs carried into K and the revision's totals, from `quote_B = E(B)`
/// and the candidate legs. Any hold keeps K pending.
pub fn snapshot(quote_b: Decimal, legs: &[CandidateLeg], k: &Boundary) -> Result<Snapshot, Hold> {
    ensure(distinct(legs.iter().map(|leg| leg.position_id)))?;
    let carried: Vec<CarriedLeg> = legs
        .iter()
        .filter_map(|leg| leg_at_b(leg, k).transpose())
        .collect::<Result<_, _>>()?;
    let floors = sum(carried.iter().map(|leg| leg.floor))?;
    let downside = sum(carried.iter().map(|leg| leg.downside))?;
    let basis = sum(carried.iter().map(|leg| leg.floor).chain([quote_b]))?;
    Ok((carried, BoundaryTotals { floors, downside, basis }))
}

/// One candidate leg as of B. `None` when it carries nothing: no cohort
/// before K (an Armed position or a never-sent entry, SD16), or finished
/// before B (G_B = 0).
pub fn leg_at_b(leg: &CandidateLeg, k: &Boundary) -> Result<Option<CarriedLeg>, Hold> {
    let b = k.instant();
    if leg.cohort_month.is_none_or(|cohort| cohort >= k.month) {
        // An entry acknowledged or traded before B was handed off before it:
        // an earlier cohort. Otherwise the leg carries nothing.
        let early =
            |e: &LegOrder| e.live_seen.before(b) || e.fills.iter().any(|f| f.trade_time < b);
        return (!entries(leg).any(early)).then_some(None).ok_or(Hold::Invalid);
    }
    // The charge, every order as the ledger constrains them (M36) and no
    // trade after the dust row's last fill, or K holds.
    let mut fills = leg.orders.iter().flat_map(|o| &o.fills);
    let dust = leg.dust.is_none_or(|d| fills.all(|f| f.trade_time <= d.last_fill_at));
    ensure(leg.terms.admission_charge > Decimal::ZERO && ledger_orders(&leg.orders) && dust)?;
    let (g_b, protection) = if entry_complete(leg, b) {
        let g_b = inventory_at(leg, b)?;
        if g_b.is_zero() {
            return Ok(None);
        }
        (g_b, protection(leg, k, g_b)?)
    } else {
        (Decimal::ZERO, None)
    };
    let floor = match &protection {
        Some(protected) => protected.floor.quote,
        None if g_b.is_zero() => -leg.terms.admission_charge,
        None => Decimal::ZERO,
    };
    let mark_value = floors::product(g_b, k.mark)?;
    let downside = sub(mark_value, floor)?;
    Ok(Some(CarriedLeg {
        position_id: leg.position_id,
        g_b,
        protection,
        floor,
        mark_value,
        downside,
    }))
}

/// G_B = X − the leg's sells before B (listed and summarized) − R when its
/// write-off is proven before B. A deficit holds K (v16 §C4); an X above the
/// entry's acquisitions is refused.
fn inventory_at(leg: &CandidateLeg, b: DateTime<Utc>) -> Result<Decimal, Hold> {
    let x = leg.executable.ok_or(Hold::NoExecutable(leg.position_id))?;
    ensure(x <= sum(entries(leg).flat_map(|o| &o.fills).map(|f| f.qty))?)?;
    let sold = sell_capable(leg).flat_map(|o| &o.fills).filter(|f| f.trade_time < b);
    let proven = leg.dust.filter(|dust| write_off_before(dust, b));
    let written_off = proven.map_or(Ok(Decimal::ZERO), |dust| nonneg(dust.write_off))?;
    let summarized = nonneg(leg.summarized_sold_before_b)?;
    let g_b = sum(sold.map(|f| -f.qty).chain([x, -summarized, -written_off]))?;
    (g_b >= Decimal::ZERO).then_some(()).ok_or(Hold::Deficit(leg.position_id))?;
    // A close proven before B leaves nothing: X = Σ sells + R (v16 §3.3.2).
    ensure(proven.is_none() || g_b.is_zero())?;
    Ok(g_b)
}

/// The write-off is pre-boundary, and E4 dated before `b`, only when its
/// close is proven before `b` and the leg's last fill is before it (§6.12.3).
/// Otherwise the leg is carried with its remainder and E4 is dated in K.
pub fn write_off_before(dust: &Dust, b: DateTime<Utc>) -> bool {
    dust.close_at.before(b) && dust.last_fill_at < b
}

/// Entry complete before B: every entry order is terminal with every trade
/// before B, and is `FILLED` or seen terminal by B − 2 s. Anything else,
/// missing evidence included, is an entry in flight.
fn entry_complete(leg: &CandidateLeg, b: DateTime<Utc>) -> bool {
    let mut found = entries(leg).peekable();
    found.peek().is_some()
        && found.all(|e| {
            e.phase == OrderPhase::Terminal
                && e.fills.iter().all(|f| f.trade_time < b)
                && (e.status == "FILLED" || e.terminal_seen.is_some_and(|t| t.before(b)))
        })
}

fn entries(leg: &CandidateLeg) -> impl Iterator<Item = &LegOrder> {
    leg.orders.iter().filter(|o| o.kind == LegOrderKind::Entry)
}

fn sell_capable(leg: &CandidateLeg) -> impl Iterator<Item = &LegOrder> {
    leg.orders.iter().filter(|o| o.kind != LegOrderKind::Entry)
}

/// Protected at B ⇔ exactly one stop with a trigger satisfies (a), (b) and
/// (c), and every other sell-capable order is shown not open at B. A stop
/// that would be the one but whose (b) evidence is not in yet holds K.
fn protection(leg: &CandidateLeg, k: &Boundary, g_b: Decimal) -> Result<Option<Protection>, Hold> {
    let b = k.instant();
    // Robson's cancel finding open an order it saw terminal holds K (trades
    // and lookups after the terminal record fail its chronology).
    ensure(!sell_capable(leg).any(|o| open_after(leg, o, b) == Err(Hold::Invalid)))?;
    let alone = |s: &LegOrder| sell_capable(leg).all(|o| std::ptr::eq(o, s) || excluded(o, b));
    let (mut proven, mut held) = (Vec::new(), None);
    for s in sell_capable(leg).filter(|o| o.kind == LegOrderKind::Stop) {
        // (a) Robson recorded the venue's acknowledgement before B.
        let Some(trigger) = s.stop_price.filter(|_| s.live_seen.before(b)) else {
            continue;
        };
        // (c) its open quantity at B is exactly G_B.
        let before = sum(s.fills.iter().filter(|f| f.trade_time < b).map(|f| f.qty))?;
        if sub(s.orig_qty, before)? != g_b {
            continue;
        }
        match open_after(leg, s, b) {
            Ok(Some(open)) => proven.push((s, trigger, open)),
            Err(hold) if alone(s) => held = held.or(Some(hold)),
            Ok(None) | Err(_) => {},
        }
    }
    match proven.as_slice() {
        [(s, trigger, open)] if alone(s) => Ok(Some(Protection {
            stop: s.exchange_order_id,
            trigger: *trigger,
            open_after_b: *open,
            floor: floor_at(g_b, *trigger, k.mark, &leg.terms)?,
        })),
        [] => held.map_or(Ok(None), Err),
        _ => Ok(None),
    }
}

/// (b)'s answer: the branch that proves it, `None` when nothing does, or a
/// hold when the proof waits on evidence not booked yet.
type Proof = Result<Option<OpenAfterB>, Hold>;

/// (b): s was open after B.
fn open_after(leg: &CandidateLeg, s: &LegOrder, b: DateTime<Utc>) -> Proof {
    let terminal = s.phase == OrderPhase::Terminal;
    let attempts: Vec<RobsonTime> = leg
        .cancels
        .iter()
        .filter(|c| c.position_id == leg.position_id && c.cancels_order_id == s.exchange_order_id)
        .filter(|c| c.venue_outcome.as_deref() == Some("success"))
        .filter(|c| c.cancel_class.as_deref() == Some("cancelled"))
        .filter_map(|c| c.first_attempt)
        .collect();
    // Robson's own cancel cannot find open an order it saw terminal 2 s or
    // more before attempting it: contradictory facts.
    let ended_before = |a: &RobsonTime| {
        a.verdict == Some(ClockVerdict::Valid) && s.terminal_seen.is_some_and(|t| t.before(a.at))
    };
    ensure(!attempts.iter().any(ended_before))?;
    let cancelled = attempts.iter().any(|t| t.after(b));
    if s.fills.iter().any(|f| f.trade_time >= b) {
        Ok(Some(OpenAfterB::TradeAfterB))
    } else if cancelled && terminal && s.status == "CANCELED" {
        Ok(Some(OpenAfterB::RobsonCancelAfterB))
    } else if s.lookup.as_ref().is_some_and(|l| l.status == "NEW" && l.sent.after(b)) {
        Ok(Some(OpenAfterB::NewAtCompletion))
    } else if cancelled && !terminal {
        Err(Hold::TerminalRecordNeeded(s.exchange_order_id))
    } else if s.lookup.is_none() && !terminal {
        Err(Hold::LookupNeeded(s.exchange_order_id))
    } else {
        Ok(None)
    }
}

/// Shown not open at B: its intent was first attempted at or after B + 2 s
/// (it did not exist), or Robson saw it terminal by B − 2 s and every trade
/// of it is before B.
fn excluded(o: &LegOrder, b: DateTime<Utc>) -> bool {
    o.first_attempt.is_some_and(|t| t.after(b))
        || (o.terminal_seen.is_some_and(|t| t.before(b))
            && o.fills.iter().all(|f| f.trade_time < b))
}

fn sub(left: Decimal, right: Decimal) -> Result<Decimal, Hold> {
    exact_sub(left, right).map_err(|_| Hold::Invalid)
}

/// Orders as the ledger holds them: each well formed, each once (its key).
fn ledger_orders(orders: &[LegOrder]) -> bool {
    distinct(orders.iter().map(|o| o.exchange_order_id)) && orders.iter().all(LegOrder::well_formed)
}

fn distinct<T: Ord>(mut ids: impl Iterator<Item = T>) -> bool {
    let mut seen = std::collections::BTreeSet::new();
    ids.all(|id| seen.insert(id))
}

/// `Hold::Invalid` unless the facts hold together.
fn ensure(holds: bool) -> Result<(), Hold> {
    holds.then_some(()).ok_or(Hold::Invalid)
}

/// An amount the ledger keeps nonnegative; anything else holds K.
fn nonneg(value: Decimal) -> Result<Decimal, Hold> {
    (value >= Decimal::ZERO).then_some(value).ok_or(Hold::Invalid)
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod oracle_tests;
