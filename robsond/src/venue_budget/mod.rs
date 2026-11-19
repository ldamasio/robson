//! Controlled I1 budget integration. Descriptive bootstrap rows do not supply
//! the missing production C2/fencing/final-source-audit authority.
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

pub(crate) mod bootstrap;
mod completion;
#[cfg(test)]
pub(crate) mod fixture;
mod heads;
#[allow(
    dead_code,
    reason = "dormant N5 storage awaits the reviewed receipt producer"
)]
pub(crate) mod n5_claim;
mod pause_value;
pub(crate) mod restore;
mod snapshot;
pub(crate) mod state;

pub(crate) use snapshot::BootstrapRows;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BudgetError {
    Invalid,
    Expired,
    Backend,
    Unpublished,
    Deferred,
    Closing,
    PrerequisiteRefusal,
}

#[derive(Clone, Default)]
pub(crate) struct Trace(Arc<AtomicU64>);
impl Trace {
    pub(crate) fn issued(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
    pub(super) fn issue(&self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod fixture_tests;
// Descriptive page plus private running/failed audit foundation only; no
// production readiness consumer until the external C2 barrier exists.
#[allow(
    dead_code,
    reason = "private audit controller awaits the external C2 barrier"
)]
pub(crate) mod source_audit;
// Direct SQL migration38 store-guard behavior controls; TEST-ONLY.
#[cfg(test)]
mod store_guard_pg_tests;

#[cfg(test)]
pub(crate) use source_audit::physical_recovery;
