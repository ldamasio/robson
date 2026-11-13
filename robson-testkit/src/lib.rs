//! Test helpers for Robson v2 database-backed tests.
//!
//! Provides seeding helpers for tenant/account, event appending, and projection
//! state; [`spot_book`], an in-memory spot balance account for venue tests;
//! [`balance_stub`], the spot venue's port contracts over it;
//! [`conformance`], the venue conformance suite's inventory, coverage,
//! `check` and scenarios; and [`dependency_check`], which proves from
//! `cargo metadata` that only dev-dependency edges reach this crate.

pub mod balance_stub;
#[cfg(test)]
mod balance_stub_tests;
pub mod conformance;
#[cfg(test)]
mod conformance_tests;
pub mod daemon_process;
pub mod db_proxy;
pub mod dependency_check;
#[cfg(test)]
mod dependency_check_tests;
pub mod fake_venue;
mod helpers;
#[cfg(test)]
mod sd12_tests;
pub mod spot_book;
#[cfg(test)]
mod spot_book_model_tests;
#[cfg(test)]
mod spot_book_tests;
mod spot_evidence;
#[cfg(test)]
mod spot_evidence_tests;
#[cfg(test)]
mod spot_mirror_tests;

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
