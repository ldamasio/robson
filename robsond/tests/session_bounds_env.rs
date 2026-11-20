//! T1 option preservation and the USD-M equality with `PGOPTIONS` set (PLAN r4
//! §6). The test changes the process environment, so it lives alone in its own
//! test binary and never shares a process with the parallel PostgreSQL tier.

#![cfg(feature = "postgres")]

use robson_exec::venue::AccountKind;
use robsond::db_session::connect_daemon_pool;
use sqlx::PgPool;

type Settings = (String, String, String, String);

async fn settings(pool: &PgPool) -> Settings {
    sqlx::query_as(
        "SELECT current_setting('work_mem'), current_setting('lock_timeout'), \
         current_setting('statement_timeout'), current_setting('idle_in_transaction_session_timeout')",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

/// What a balance account and a position account see for one database URL,
/// next to a direct `PgPool::connect` on the same URL.
async fn observe(url: &str) -> (Settings, Settings, Settings) {
    let balance = connect_daemon_pool(url, AccountKind::BalanceAccount).await.unwrap();
    let position = connect_daemon_pool(url, AccountKind::PositionAccount).await.unwrap();
    let direct = PgPool::connect(url).await.unwrap();
    let observed = (settings(&balance).await, settings(&position).await, settings(&direct).await);
    for pool in [balance, position, direct] {
        pool.close().await;
    }
    observed
}

#[sqlx::test(migrations = false)]
#[ignore = "requires DATABASE_URL"]
async fn existing_startup_options_are_kept_and_a_position_account_is_unchanged(pool: PgPool) {
    let database = pool.connect_options().get_database().unwrap().to_string();
    let base = std::env::var("DATABASE_URL").unwrap();
    let url = format!("{}/{database}", base.rsplit_once('/').unwrap().0);
    let bounds = ("2s".to_string(), "5s".to_string(), "5s".to_string());
    let with_bounds = |work_mem: &str| {
        (work_mem.to_string(), bounds.0.clone(), bounds.1.clone(), bounds.2.clone())
    };

    std::env::remove_var("PGOPTIONS");
    let (balance, position, direct) = observe(&url).await;
    assert_eq!(position, direct, "no options: a position account is a plain connect");
    assert_eq!(balance, with_bounds(&direct.0));

    // Options already in the URL, one of them a bound: the position account
    // keeps them as a plain connect does; the balance account keeps the
    // others and its own bounds come last, so they win.
    let url_with_options = format!("{url}?options=-c%20work_mem%3D8MB%20-c%20lock_timeout%3D3s");
    let (balance, position, direct) = observe(&url_with_options).await;
    assert_eq!(position, direct, "URL options: a position account is a plain connect");
    assert_eq!(direct.0, "8MB");
    assert_eq!(direct.1, "3s");
    assert_eq!(balance, with_bounds("8MB"), "URL options kept, bounds appended");

    // Options from the environment, as libpq reads PGOPTIONS.
    std::env::set_var("PGOPTIONS", "-c work_mem=16MB");
    let (balance, position, direct) = observe(&url).await;
    std::env::remove_var("PGOPTIONS");
    assert_eq!(position, direct, "PGOPTIONS: a position account is a plain connect");
    assert_eq!(direct.0, "16MB");
    assert_eq!(balance, with_bounds("16MB"), "PGOPTIONS kept, bounds appended");
}
