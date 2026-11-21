//! Unqualified C8 arithmetic, with no evidence or request authority.
use uuid::Uuid;

use crate::{
    venue::VenueId,
    venue_clock::{ClockError, TickCapture},
};

const SECOND: i128 = 1_000_000_000;
const DISCREPANCY: i128 = 50_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(crate) enum FactsError {
    #[error("inconsistent sample identity")]
    Identity,
    #[error("invalid wall or server timestamp")]
    Timestamp,
    #[error("wall timestamp reversed")]
    ReversedWall,
    #[error("clock arithmetic range exceeded")]
    Range,
    #[error(transparent)]
    Clock(#[from] ClockError),
}

/// Caller-supplied coordinates, not proof that the wall read was bracketed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WallMarkCandidate {
    epoch: Uuid,
    wall_ns: i128,
    before: TickCapture,
    after: TickCapture,
}

impl WallMarkCandidate {
    pub(crate) fn new(
        epoch: Uuid,
        wall_seconds: i64,
        wall_nanos: u32,
        before: TickCapture,
        after: TickCapture,
    ) -> Result<Self, FactsError> {
        if epoch.is_nil() {
            return Err(FactsError::Identity);
        }
        if wall_nanos >= SECOND as u32 {
            return Err(FactsError::Timestamp);
        }
        after.elapsed_since(&before)?;
        let wall_ns = i128::from(wall_seconds)
            .checked_mul(SECOND)
            .and_then(|n| n.checked_add(i128::from(wall_nanos)))
            .ok_or(FactsError::Range)?;
        Ok(Self { epoch, wall_ns, before, after })
    }
}

pub(crate) struct SampleCandidate<'a> {
    pub(crate) id: Uuid,
    pub(crate) tenant: Uuid,
    pub(crate) venue: &'a VenueId,
    pub(crate) epoch: Uuid,
    pub(crate) send: WallMarkCandidate,
    pub(crate) receive: WallMarkCandidate,
    pub(crate) server_ms: i64,
}

/// Immutable numerical facts only. Original marks remain the time origins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SampleFacts {
    id: Uuid,
    tenant: Uuid,
    send: WallMarkCandidate,
    receive: WallMarkCandidate,
    theta2: i128,
    rtt: (i128, i128),
    u2: i128,
    discrepancy_fits: bool,
}

impl SampleFacts {
    pub(crate) fn calculate(i: SampleCandidate<'_>) -> Result<Self, FactsError> {
        if i.id.is_nil()
            || i.tenant.is_nil()
            || i.venue.as_str() != VenueId::BINANCE_SPOT
            || i.epoch != i.send.epoch
            || i.epoch != i.receive.epoch
        {
            return Err(FactsError::Identity);
        }
        if i.server_ms < 0 {
            return Err(FactsError::Timestamp);
        }
        let rtt = interval(&i.send, &i.receive)?;
        let wall = wall_delta(&i.send, &i.receive)?;
        let theta2 = i128::from(i.server_ms)
            .checked_mul(2_000_000)
            .and_then(|n| n.checked_sub(i.send.wall_ns))
            .and_then(|n| n.checked_sub(i.receive.wall_ns))
            .ok_or(FactsError::Range)?;
        let u2 = theta2
            .checked_abs()
            .and_then(|n| n.checked_add(rtt.1))
            .and_then(|n| n.checked_add(200_000_000))
            .ok_or(FactsError::Range)?;
        Ok(Self {
            id: i.id,
            tenant: i.tenant,
            send: i.send,
            receive: i.receive,
            theta2,
            rtt,
            u2,
            discrepancy_fits: discrepancy(wall, rtt)?,
        })
    }

    pub(crate) fn identity(&self) -> (Uuid, Uuid, Uuid) {
        (self.id, self.tenant, self.receive.epoch)
    }
    pub(crate) fn original_marks(&self) -> (WallMarkCandidate, WallMarkCandidate) {
        (self.send, self.receive)
    }
    pub(crate) fn twice_offset_ns(&self) -> i128 {
        self.theta2
    }
    pub(crate) fn rtt_ns(&self) -> (i128, i128) {
        self.rtt
    }
    pub(crate) fn twice_bound_ns(&self) -> i128 {
        self.u2
    }
    pub(crate) fn bound_ns_ceil(&self) -> Result<i128, FactsError> {
        self.u2.checked_add(1).map(|n| n / 2).ok_or(FactsError::Range)
    }
    pub(crate) fn fits_bound(&self) -> bool {
        self.u2 <= 2 * SECOND
    }
    pub(crate) fn fits_discrepancy(&self) -> bool {
        self.discrepancy_fits
    }

    /// A necessary numerical comparison, not a sticky lifecycle verdict.
    pub(crate) fn compare(&self, later: &WallMarkCandidate) -> Result<CaptureFacts, FactsError> {
        let age = interval(&self.receive, later)?;
        let wall = wall_delta(&self.receive, later)?;
        Ok(CaptureFacts {
            sample: self.id,
            u2: self.u2,
            wall_delta: wall,
            age,
            sample_fits: self.fits_bound() && self.fits_discrepancy(),
            age_fits: age.1 <= 60 * SECOND,
            discrepancy_fits: discrepancy(wall, age)?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CaptureFacts {
    sample: Uuid,
    u2: i128,
    wall_delta: i128,
    age: (i128, i128),
    sample_fits: bool,
    age_fits: bool,
    discrepancy_fits: bool,
}
impl CaptureFacts {
    pub(crate) fn sample_id(&self) -> Uuid {
        self.sample
    }
    pub(crate) fn twice_bound_ns(&self) -> i128 {
        self.u2
    }
    pub(crate) fn wall_delta_ns(&self) -> i128 {
        self.wall_delta
    }
    pub(crate) fn age_ns(&self) -> (i128, i128) {
        self.age
    }
    pub(crate) fn fits_sample(&self) -> bool {
        self.sample_fits
    }
    pub(crate) fn fits_age(&self) -> bool {
        self.age_fits
    }
    pub(crate) fn fits_discrepancy(&self) -> bool {
        self.discrepancy_fits
    }
}

pub(crate) fn interval(
    a: &WallMarkCandidate,
    b: &WallMarkCandidate,
) -> Result<(i128, i128), FactsError> {
    if a.epoch != b.epoch {
        return Err(FactsError::Identity);
    }
    let low = b.before.elapsed_since(&a.after)?.as_nanos();
    let high = b.after.elapsed_since(&a.before)?.as_nanos();
    Ok((
        i128::try_from(low).map_err(|_| FactsError::Range)?,
        i128::try_from(high).map_err(|_| FactsError::Range)?,
    ))
}
pub(crate) fn wall_delta(a: &WallMarkCandidate, b: &WallMarkCandidate) -> Result<i128, FactsError> {
    let delta = b.wall_ns.checked_sub(a.wall_ns).ok_or(FactsError::Range)?;
    if delta < 0 {
        return Err(FactsError::ReversedWall);
    }
    Ok(delta)
}
pub(crate) fn discrepancy(wall: i128, interval: (i128, i128)) -> Result<bool, FactsError> {
    let low = wall
        .checked_sub(interval.0)
        .and_then(i128::checked_abs)
        .ok_or(FactsError::Range)?;
    let high = wall
        .checked_sub(interval.1)
        .and_then(i128::checked_abs)
        .ok_or(FactsError::Range)?;
    Ok(low <= DISCREPANCY && high <= DISCREPANCY)
}

#[cfg(test)]
mod tests;
