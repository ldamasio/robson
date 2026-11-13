//! Test helpers for Robson v2 database-backed tests.
//!
//! Provides seeding helpers for tenant/account, event appending, and projection
//! state; [`spot_book`], an in-memory spot balance account for venue tests;
//! and [`balance_stub`], the spot venue's port contracts over it.

pub mod balance_stub;
#[cfg(test)]
mod balance_stub_tests;
pub mod daemon_process;
pub mod db_proxy;
pub mod fake_venue;
mod helpers;
pub mod spot_book;
#[cfg(test)]
mod spot_book_model_tests;
#[cfg(test)]
mod spot_book_tests;

use anyhow::Result;
pub use helpers::{
    append_event, seed_balance_sampled_event, seed_position_opened_event, seed_tenant_account,
    AppendEventOptions,
};
use sqlx::PgPool;

/// Setup a clean test database by running migrations.
///
/// Convenience function for tests that need a fresh schema.
/// Note: migrations are located at v2/migrations relative to workspace root.
pub async fn setup_test_db(pool: &PgPool) -> Result<()> {
    sqlx::migrate!("../migrations").run(pool).await?;
    Ok(())
}
