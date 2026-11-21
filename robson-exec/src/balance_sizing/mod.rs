//! Pure unqualified balance candidate data. No production caller or send grant.
mod arithmetic;
mod bridge;
mod geometry;
#[cfg(test)]
mod geometry_tests;
mod handoff;
mod inputs;
mod leg_quantity;
pub use handoff::{candidate_admission, HandoffError};
pub(crate) use leg_quantity::{initial_executable, reduced_executable};
#[cfg(test)]
mod test_support;

mod sizing;
pub use inputs::*;
pub use sizing::calculate_balance_candidate;
