//! Unqualified clock coordinates. Neither parsing nor elapsed arithmetic proves
//! durable provenance, freshness, request eligibility, or permission to trade.

use std::time::Duration;

use thiserror::Error;
use uuid::Uuid;

#[cfg(all(
    target_os = "linux",
    target_env = "gnu",
    target_pointer_width = "64",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
mod native;
#[cfg(not(all(
    target_os = "linux",
    target_env = "gnu",
    target_pointer_width = "64",
    any(target_arch = "x86_64", target_arch = "aarch64")
)))]
mod native {
    use super::*;
    pub(super) fn initialize() -> Result<TickCapture, ClockError> {
        Err(ClockError::Unsupported)
    }
    pub(super) fn tick() -> Result<(i64, i64), ClockError> {
        Err(ClockError::Unsupported)
    }
    pub(super) fn wall_bracket(
        _domain: ClockDomain,
        _epoch: Uuid,
    ) -> Result<crate::clock_records::RawClockCapture, ClockError> {
        Err(ClockError::Unsupported)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ClockError {
    #[error("unsupported native clock target")]
    Unsupported,
    #[error("native {operation} failed with errno {errno}")]
    Native { operation: &'static str, errno: i32 },
    #[error("invalid clock metadata: {0}")]
    Metadata(&'static str),
    #[error("clock metadata exceeds its byte bound")]
    Oversized,
    #[error("clock metadata read attempt bound exhausted")]
    ReadLimit,
    #[error("current and child time namespaces disagree")]
    NamespaceMismatch,
    #[error("clock domain changed during initialization")]
    DomainChanged,
    #[error("invalid native clock result")]
    InvalidTick,
    #[error("clock domains are incomparable")]
    Incomparable,
    #[error("clock tick reversed")]
    Reversed,
    #[error("clock duration is unrepresentable")]
    Range,
}

/// Validated raw identity, without any claim of durable provenance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClockDomain {
    boot: Uuid,
    offset_seconds: i64,
    offset_nanoseconds: u32,
}

impl ClockDomain {
    pub fn new(
        boot: Uuid,
        offset_seconds: i64,
        offset_nanoseconds: u32,
    ) -> Result<Self, ClockError> {
        if boot.is_nil() || offset_nanoseconds >= 1_000_000_000 {
            return Err(ClockError::Metadata("domain"));
        }
        Ok(Self { boot, offset_seconds, offset_nanoseconds })
    }
    pub fn boot(&self) -> Uuid {
        self.boot
    }
    pub fn offset(&self) -> (i64, u32) {
        (self.offset_seconds, self.offset_nanoseconds)
    }
}

/// An unqualified capture, including when reconstructed from persisted parts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TickCapture {
    domain: ClockDomain,
    nanoseconds: u128,
}

impl TickCapture {
    pub fn new(domain: ClockDomain, seconds: i64, nanoseconds: i64) -> Result<Self, ClockError> {
        if seconds < 0 || !(0..1_000_000_000).contains(&nanoseconds) {
            return Err(ClockError::InvalidTick);
        }
        let nanoseconds = (seconds as u128)
            .checked_mul(1_000_000_000)
            .and_then(|n| n.checked_add(nanoseconds as u128))
            .ok_or(ClockError::Range)?;
        Ok(Self { domain, nanoseconds })
    }
    pub fn domain(&self) -> ClockDomain {
        self.domain
    }
    pub fn nanoseconds(&self) -> u128 {
        self.nanoseconds
    }
    pub fn elapsed_since(&self, earlier: &Self) -> Result<Duration, ClockError> {
        if self.domain != earlier.domain {
            return Err(ClockError::Incomparable);
        }
        let n = self.nanoseconds.checked_sub(earlier.nanoseconds).ok_or(ClockError::Reversed)?;
        let seconds = u64::try_from(n / 1_000_000_000).map_err(|_| ClockError::Range)?;
        Ok(Duration::new(seconds, (n % 1_000_000_000) as u32))
    }
}

/// Requires an authentic stable proc mount and a fixed time namespace in every
/// calling thread throughout its lifetime. Startup checks cannot prove future
/// topology. Initialization is work-bounded, not scheduler-deadline-bounded;
/// never perform it in a send seam. Captures alone are not durable age or
/// budget authority.
#[derive(Debug)]
pub struct NativeClock {
    initial: TickCapture,
}

impl NativeClock {
    pub fn initialize() -> Result<Self, ClockError> {
        Ok(Self { initial: native::initialize()? })
    }
    pub fn initial_capture(&self) -> TickCapture {
        self.initial
    }
    pub fn capture(&self) -> Result<TickCapture, ClockError> {
        let (seconds, nanoseconds) = native::tick()?;
        TickCapture::new(self.initial.domain, seconds, nanoseconds)
    }
    /// One actual realtime read bracketed by two BOOTTIME reads, retaining
    /// the original coordinates and initialized domain. This does no procfs
    /// work and supplies raw evidence only, never freshness or send authority.
    pub fn wall_bracket(
        &self,
        epoch: Uuid,
    ) -> Result<crate::clock_records::RawClockCapture, ClockError> {
        native::wall_bracket(self.initial.domain, epoch)
    }
}

#[cfg(test)]
mod tests;
