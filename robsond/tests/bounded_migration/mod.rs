//! Public52 acceptance descendants; stages are application facts, never wire
//! authority. All PostgreSQL databases belong to sqlx::test; infrastructure
//! supplies access.
use std::borrow::Cow;

use sqlx::migrate::Migrator;

use super::*;

// Fixture ceilings keep their former relation to the production bounds: an
// apply-owning ceiling exceeds APPLY_BOUND (formerly 5 s over 200 ms), a stage
// controller ceiling is APPLY_BOUND plus 100 ms (formerly 300 ms), and a native
// retirement window after a raw drop also covers the 1 s statement_timeout
// that a backend may still be running (formerly 300 ms over 40 ms).
const APPLY_FIXTURE: Duration = APPLY_BOUND.saturating_add(ACQUIRE_BOUND);
const CONTROLLER_CEILING: Duration = APPLY_BOUND.saturating_add(Duration::from_millis(100));
const NATIVE_RETIREMENT: Duration = Duration::from_millis(1300);
const NATIVE_RETIREMENT_POLLS: usize = 260;

mod cases;
mod catalog_cases;
mod helpers;
mod late;
mod migration39;
mod migration40;
mod migration41;
mod migration46;

/// Freeze a predecessor binary's real embedded inventory in a local fixture.
fn through(version: i64) -> Migrator {
    let mut inventory = sqlx::migrate!("../migrations");
    inventory.migrations =
        Cow::Owned(inventory.iter().filter(|m| m.version <= version).cloned().collect());
    inventory
}
