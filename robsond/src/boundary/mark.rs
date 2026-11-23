//! ADR-0056 step 8p-4: the boundary mark P_B and the owner's lookup seam (8p
//! PLAN-r3 §6.4). No production caller until 8p-5. The venue side of the mark
//! is one narrow seam, [`MarkVenue`], that package 5a binds to the PS lanes'
//! budget and read owner; no PS code is here.
use std::{future::Future, time::Duration};

use chrono::{Datelike as _, NaiveDate, NaiveTime, TimeDelta};
use robson_domain::PositionId;
use robson_exec::{
    boundary_carry::{Lookup, RobsonTime},
    venue_ledger::{ClockVerdict, SeenAt, VenueIntId},
};
use rust_decimal::Decimal;
use serde_json::Value;

use super::Failure;

/// The documented request weight of one kline read (`KLINES`).
pub(crate) const KLINE_WEIGHT: u32 = 2;

/// The mark's venue side. In 5a, [`Self::try_reserve`] is PS-5a's
/// non-trading budget reserving the closed kline read (a steady reservation
/// whose `now` is captured under the budget lock; any weight but the kline's
/// is refused) and [`Self::read_kline`] is PS-5b's single read path for PS-4's
/// prepared kline read at `start_ms` on that very reservation (no second one;
/// 16 KiB body cap). No connector item is named here (the inertness scan).
pub(crate) trait MarkVenue: Sync {
    /// A held reservation; dropped unsent, it is cancelled and refunded.
    type Reserved: Send;

    /// Reserves `weight` before anything is prepared; a busy budget refuses
    /// at once.
    fn try_reserve(&self, weight: u32) -> Result<Self::Reserved, Failure>;

    /// One kline read on `reserved`, sent unsigned: the answer's HTTP status
    /// and body.
    fn read_kline(
        &self,
        reserved: Self::Reserved,
        start_ms: u64,
    ) -> impl Future<Output = Result<(u16, String), Failure>> + Send;
}

/// P_B (v16 §C6): the close of the 1-minute kline that ends at B, the first
/// instant of `month`. Requested only once Robson's clock reads B + 60 s or
/// later with a valid verdict, on a reservation of [`KLINE_WEIGHT`] taken
/// before the send, within 10 s; accepted only with open time B − 60 s, close
/// time B − 1 ms and an exact positive decimal close. Anything else, a failure
/// included, is an error: K stays pending and the next poll retries. A
/// recomputation reuses its revision's stored mark instead.
pub(crate) async fn kline_at(
    venue: &impl MarkVenue,
    month: NaiveDate,
    now: RobsonTime,
) -> Result<Decimal, Failure> {
    let b = month.and_time(NaiveTime::MIN).and_utc();
    let clock = now.verdict == Some(ClockVerdict::Valid) && now.at >= b + TimeDelta::seconds(60);
    if month.day() != 1 || !clock {
        return Err("the boundary mark waits for B + 60 s on a valid clock".into());
    }
    let open = b.timestamp_millis() - 60_000;
    let reserved = venue.try_reserve(KLINE_WEIGHT)?;
    let read = venue.read_kline(reserved, u64::try_from(open)?);
    let (status, body) = tokio::time::timeout(Duration::from_secs(10), read).await??;
    let rows: Vec<Vec<Value>> = serde_json::from_str(&body)?;
    let field = |at: usize| rows.first().filter(|_| rows.len() == 1).and_then(|row| row.get(at));
    let times = (field(0).and_then(Value::as_i64), field(6).and_then(Value::as_i64));
    let close = field(4)
        .and_then(Value::as_str)
        .and_then(|text| Decimal::from_str_exact(text).ok());
    let answered = status == 200 && times == (Some(open), Some(open + 59_999));
    let close = close.filter(|close| answered && *close > Decimal::ZERO);
    close.ok_or_else(|| format!("kline for B refused (HTTP {status})").into())
}

/// §C4 (b1): the owner's fresh lookup of one stop of a leg, asked outside any
/// transaction when 8p-3 holds K on `Hold::LookupNeeded`. 8j binds the seam to
/// the owner loop; until then only a fixture owner answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BoundaryLookup {
    pub(crate) position_id: PositionId,
    pub(crate) exchange_order_id: VenueIntId,
}

/// The owner's answer: the status the venue gave, and when Robson sent the
/// lookup with that time's clock sample and verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LookupAnswer {
    pub(crate) status: String,
    pub(crate) sent: SeenAt,
}

impl LookupAnswer {
    /// 8p-3's (b1) input: the status, and the send time with its verdict.
    pub(crate) fn lookup(&self) -> Lookup {
        let verdict = self.sent.clock.as_ref().map(|clock| clock.verdict);
        let sent = RobsonTime { at: self.sent.at, verdict };
        Lookup { sent, status: self.status.clone() }
    }
}

pub(crate) trait BoundaryOwner: Sync {
    fn lookup(
        &self,
        request: BoundaryLookup,
    ) -> impl Future<Output = Result<LookupAnswer, Failure>> + Send;
}

#[cfg(test)]
#[path = "mark_tests.rs"]
mod tests;
