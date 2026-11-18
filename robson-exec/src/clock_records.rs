//! Descriptive exact clock records. Supplied coordinates and numerical results
//! do not prove native provenance, persistence, freshness or permission to
//! send.
use uuid::Uuid;

use crate::{
    clock_sample_facts::{SampleCandidate, SampleFacts, WallMarkCandidate},
    venue::VenueId,
    venue_clock::{ClockDomain, ClockError, TickCapture},
};

/// Largest representable nonnegative native tick, in nanoseconds.
pub const TICK_MAX: i128 = 9_223_372_036_854_775_807_999_999_999;
/// Lower accepted doubled offset.
pub const THETA_MIN: i128 = -18_446_744_073_709_551_615_999_999_998;
/// Upper accepted doubled offset.
pub const THETA_MAX: i128 = 18_465_190_817_783_261_167_614_000_000;
/// Conservative upper bound of the doubled uncertainty.
pub const U2_MAX: i128 = 27_688_562_854_638_036_975_813_999_999;
/// The immutable arithmetic interpretation used by migration38.
pub const ALGORITHM_VERSION: i16 = 1;

/// Raw wall/tick bracket in one domain, including reconstructed historical
/// data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawClockCapture {
    /// Actual capture process, not the process reading this record.
    pub epoch: Uuid,
    /// Original wall seconds.
    pub wall_seconds: i64,
    /// Original normalized wall nanoseconds.
    pub wall_nanoseconds: u32,
    /// Original BOOTTIME domain.
    pub domain: ClockDomain,
    /// Original tick preceding the one wall read.
    pub before_ns: i128,
    /// Original tick following that wall read.
    pub after_ns: i128,
}

impl RawClockCapture {
    fn mark(&self) -> Result<WallMarkCandidate, ClockError> {
        let tick = |n: i128| {
            if !(0..=TICK_MAX).contains(&n) {
                return Err(ClockError::Range);
            }
            TickCapture::new(self.domain, (n / 1_000_000_000) as i64, (n % 1_000_000_000) as i64)
        };
        WallMarkCandidate::new(
            self.epoch,
            self.wall_seconds,
            self.wall_nanoseconds,
            tick(self.before_ns)?,
            tick(self.after_ns)?,
        )
        .map_err(|_| ClockError::Metadata("raw wall bracket"))
    }

    /// Exact descriptive wall-step comparison using the original C8 bounds.
    /// Native provenance and lifecycle authority remain the caller's burden.
    pub fn wall_agrees(&self, later: &Self) -> Result<bool, ClockError> {
        let before = self.mark()?;
        let after = later.mark()?;
        let bounds = crate::clock_sample_facts::interval(&before, &after)
            .map_err(|_| ClockError::Metadata("wall interval"))?;
        let wall = crate::clock_sample_facts::wall_delta(&before, &after)
            .map_err(|_| ClockError::Metadata("wall delta"))?;
        crate::clock_sample_facts::discrepancy(wall, bounds)
            .map_err(|_| ClockError::Metadata("wall discrepancy"))
    }

    /// Original end of this raw bracket, with representation checks only.
    pub fn end_tick(&self) -> Result<TickCapture, ClockError> {
        self.check()?;
        TickCapture::new(
            self.domain,
            i64::try_from(self.after_ns / 1_000_000_000).map_err(|_| ClockError::Range)?,
            i64::try_from(self.after_ns % 1_000_000_000).map_err(|_| ClockError::Range)?,
        )
    }

    /// Validate representation only; this does not authenticate a native read.
    pub fn check(&self) -> Result<(), ClockError> {
        self.mark().map(|_| ())
    }
}

/// Full original sample response and brackets; never a qualified sample token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawClockSample {
    /// Immutable sample identity.
    pub id: Uuid,
    /// Tenant bound by the real store to its committed epoch.
    pub tenant: Uuid,
    /// Original venue.
    pub venue: VenueId,
    /// Original send bracket.
    pub send: RawClockCapture,
    /// Original receive bracket.
    pub receive: RawClockCapture,
    /// Original response, not reconstructed from an offset.
    pub server_ms: i64,
}

/// Recomputed numerical diagnostics, with no lifecycle or durability authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SampleNumbers {
    /// Doubled offset in nanoseconds.
    pub theta2: i128,
    /// Conservative RTT lower endpoint.
    pub r_low: i128,
    /// Conservative RTT upper endpoint.
    pub r_high: i128,
    /// Doubled uncertainty in nanoseconds.
    pub u2: i128,
    /// Bit0: uncertainty; bit1: either discrepancy endpoint.
    pub failures: i16,
}

/// Frozen numerical comparison, with no persistence or lifecycle authority.
pub struct SampleArithmetic(SampleFacts);

impl SampleArithmetic {
    pub fn fits_capture(&self, capture: &RawClockCapture) -> Result<bool, ClockError> {
        let facts = self
            .0
            .compare(&capture.mark()?)
            .map_err(|_| ClockError::Metadata("sample comparison"))?;
        Ok(facts.fits_sample() && facts.fits_age() && facts.fits_discrepancy())
    }
}

impl RawClockSample {
    /// Reuses the existing exact C8 engine; the actual owner must additionally
    /// prove consume-once source, persistence and fresh generation/Closing CAS.
    pub fn arithmetic(&self) -> Result<SampleArithmetic, ClockError> {
        SampleFacts::calculate(SampleCandidate {
            id: self.id,
            tenant: self.tenant,
            venue: &self.venue,
            epoch: self.receive.epoch,
            send: self.send.mark()?,
            receive: self.receive.mark()?,
            server_ms: self.server_ms,
        })
        .map(SampleArithmetic)
        .map_err(|_| ClockError::Metadata("raw sample"))
    }
    /// Recompute through the existing private C8 arithmetic. Invalid numerical
    /// diagnostics remain invalid; a passing result still confers no authority.
    pub fn numbers(&self) -> Result<SampleNumbers, ClockError> {
        let facts = SampleFacts::calculate(SampleCandidate {
            id: self.id,
            tenant: self.tenant,
            venue: &self.venue,
            epoch: self.receive.epoch,
            send: self.send.mark()?,
            receive: self.receive.mark()?,
            server_ms: self.server_ms,
        })
        .map_err(|_| ClockError::Metadata("raw sample"))?;
        let (r_low, r_high) = facts.rtt_ns();
        Ok(SampleNumbers {
            theta2: facts.twice_offset_ns(),
            r_low,
            r_high,
            u2: facts.twice_bound_ns(),
            failures: i16::from(!facts.fits_bound()) | (i16::from(!facts.fits_discrepancy()) << 1),
        })
    }
}

#[cfg(test)]
mod tests;
