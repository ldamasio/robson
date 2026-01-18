//! Test helpers for Robson v2 database-backed tests.
//!
//! Provides seeding helpers for tenant/account, event appending, and projection state.

mod helpers;

pub use helpers::{
    AppendEventOptions, append_event, seed_balance_sampled_event, seed_position_opened_event,
    seed_tenant_account,
};

use anyhow::Result;
use sqlx::PgPool;

/// Setup a clean test database by running migrations.
///
/// Convenience function for tests that need a fresh schema.
/// Note: migrations are located at v2/migrations relative to workspace root.
pub async fn setup_test_db(pool: &PgPool) -> Result<()> {
    sqlx::migrate!("../migrations").run(pool).await?;
    Ok(())
}
