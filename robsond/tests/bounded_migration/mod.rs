//! Public52 acceptance descendants; stages are application facts, never wire
//! authority. All PostgreSQL databases belong to sqlx::test; infrastructure
//! supplies access.
use std::borrow::Cow;

use sqlx::migrate::Migrator;

use super::*;

mod cases;
mod catalog_cases;
mod helpers;
mod late;
mod migration39;
mod migration40;

/// Freeze a predecessor binary's real embedded inventory in a local fixture.
fn through(version: i64) -> Migrator {
    let mut inventory = sqlx::migrate!("../migrations");
    inventory.migrations =
        Cow::Owned(inventory.iter().filter(|m| m.version <= version).cloned().collect());
    inventory
}
