//! Pure unqualified balance candidate data. No production caller or send grant.
mod arithmetic;
mod bridge;
mod geometry;
#[cfg(test)]
mod geometry_tests;
mod inputs;
#[cfg(test)]
mod test_support;

mod sizing;
pub use inputs::*;
pub use sizing::calculate_balance_candidate;
