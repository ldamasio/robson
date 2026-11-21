//! A balance account's incident, suspension and release transactions bound
//! their own server work first and retire raw, like its batch commits. The
//! legacy journal's stay unbounded and unchanged (operator deferral D5).
use std::time::Instant;

use robson_exec::{
    gate::{Incident, IncidentReason},
    order_journal::ReleaseRecord,
};

use super::*;

const BOUNDS: &str = "SELECT set_config('lock_timeout', $1, true)";
/// The accounting bump right after them (ADR-0056 step 8l-4). With no
/// generation installed there is no CAS, and with no accounting row it moves
/// nothing.
const BUMP: &str = "WITH locked AS (";

fn incident() -> Incident {
    Incident {
        reason: IncidentReason::UncertainOutcome,
        first_at: Utc::now(),
        last_at: Utc::now(),
        count: 1,
        first_generation: 1,
        last_generation: 1,
    }
}

fn suspension() -> StreamSuspensionRow {
    StreamSuspensionRow {
        suspension_id: Uuid::now_v7(),
        stream_key: format!("position:{}", Uuid::now_v7()),
        stream_kind: "position",
        failed_seq: None,
        month: None,
        error: "test refusal".to_string(),
        epoch_id: None,
        suspended_at: Utc::now(),
    }
}

fn release() -> ReleaseRecord {
    ReleaseRecord {
        decision_id: Uuid::now_v7(),
        generation: 1,
        actor: "operator@example.test".to_string(),
        auth_kind: "verified",
        note: "reviewed the pending list".to_string(),
        treatment: "unbooked_accepted",
        decided_at: Utc::now(),
    }
}

/// The three auxiliary writes, each reported as `Ok` or its failure's text.
async fn auxiliary(backend: &PgJournalBackend, table: &str) -> Result<(), String> {
    match table {
        "recovery_incidents" => {
            backend.upsert_incident(&incident()).await.map_err(|f| format!("{f:?}"))
        },
        "stream_suspensions" => {
            backend.record_suspension(&suspension()).await.map_err(|f| format!("{f:?}"))
        },
        _ => backend.release(&release()).await.map(|_| ()),
    }
}

/// The statements one write sent, as the proxy received them.
async fn traced(
    proxy: &DbProxy,
    backend: &PgJournalBackend,
    table: &str,
) -> (Result<(), String>, Vec<String>) {
    proxy.enable_query_trace(16);
    let before = proxy.stats().statements;
    let result = auxiliary(backend, table).await;
    let trace: Vec<String> = proxy.query_trace().iter().map(|sql| sql.trim().to_owned()).collect();
    assert_eq!(proxy.stats().statements - before, trace.len() as u64, "{table}: {trace:?}");
    (result, trace)
}

const WRITES: [(&str, usize); 3] = [
    ("recovery_incidents", 3),
    ("stream_suspensions", 3),
    ("order_intents", 5),
];

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn balance_auxiliary_transactions_send_their_bounds_first(pool: PgPool) {
    let tenant = Uuid::now_v7();
    let (proxy, proxied) = through_proxy(&pool).await;
    let balance = PgJournalBackend::new(&proxied, tenant).with_balance_account(true);
    let legacy = PgJournalBackend::new(&proxied, tenant);
    balance.check_ready().await.unwrap();
    legacy.check_ready().await.unwrap();
    for (table, legacy_statements) in WRITES {
        let (result, trace) = traced(&proxy, &balance, table).await;
        result.unwrap_or_else(|error| panic!("{table}: {error}"));
        assert_eq!(trace.len(), legacy_statements + 2, "{table}: {trace:?}");
        assert_eq!(trace[0], "BEGIN", "{table}: {trace:?}");
        assert!(trace[1].starts_with(BOUNDS), "{table}: bounds not first: {trace:?}");
        assert!(trace[2].starts_with(BUMP), "{table}: no bump after the bounds: {trace:?}");
        assert_eq!(trace.last().map(String::as_str), Some("COMMIT"), "{table}: {trace:?}");
        // The legacy journal's transaction is the same minus the bounds and
        // the bump.
        let (result, trace) = traced(&proxy, &legacy, table).await;
        result.unwrap_or_else(|error| panic!("legacy {table}: {error}"));
        assert_eq!(trace.len(), legacy_statements, "legacy {table}: {trace:?}");
        assert!(
            !trace.iter().any(|sql| sql.contains("set_config")),
            "legacy {table} must stay unbounded: {trace:?}"
        );
    }
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn balance_auxiliary_lock_waits_end_at_the_server_bound_and_retire_raw(pool: PgPool) {
    let tenant = Uuid::now_v7();
    let (proxy, proxied) = through_proxy(&pool).await;
    let backend = PgJournalBackend::new(&proxied, tenant).with_balance_account(true);
    for (table, _) in WRITES {
        // Warm the pool first: its probe reads every journal table.
        backend.check_ready().await.unwrap();
        let mut held = pool.begin().await.unwrap();
        sqlx::query(&format!("LOCK TABLE {table} IN ACCESS EXCLUSIVE MODE"))
            .execute(&mut *held)
            .await
            .unwrap();
        let closed = proxy.closed_relays();
        let started = Instant::now();
        // Far past the 300 ms statement budget; unbounded, the wait would
        // last until `held` ends.
        let watched =
            tokio::time::timeout(Duration::from_secs(3), traced(&proxy, &backend, table)).await;
        let elapsed = started.elapsed();
        held.rollback().await.unwrap();
        let (result, _) = watched.expect("the server bound must end the lock wait");
        let error = result.expect_err("a lock wait past the bound must fail");
        // Lock and statement timeouts share one budget: either may fire first.
        assert!(
            error.contains("lock timeout") || error.contains("statement timeout"),
            "{table}: {error}"
        );
        if table != "order_intents" {
            assert!(error.starts_with("Known("), "{table}: nothing committed: {error}");
        }
        assert!(elapsed < Duration::from_secs(2), "{table}: {elapsed:?}");
        // The failed connection is closed, not handed back; once its relay
        // ends, the trace holds everything it sent.
        tokio::time::timeout(Duration::from_secs(3), async {
            while proxy.closed_relays() == closed {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("{table}: the failed connection was not closed"));
        let trace: Vec<String> =
            proxy.query_trace().iter().map(|sql| sql.trim().to_owned()).collect();
        // The statement that waited never executed: the table lock stops it in
        // its Parse, and the proxy counts executed statements only. Nothing
        // follows the bounds and the accounting bump, not even a ROLLBACK,
        // since the connection is dropped raw.
        assert_eq!(trace.len(), 3, "{table}: {trace:?}");
        assert_eq!(trace[0], "BEGIN", "{table}: {trace:?}");
        assert!(trace[1].starts_with(BOUNDS), "{table}: {trace:?}");
        assert!(trace[2].starts_with(BUMP), "{table}: {trace:?}");
    }
    // Each raw retirement gave back its pool permit.
    backend.check_ready().await.unwrap();
    for (table, _) in WRITES {
        auxiliary(&backend, table)
            .await
            .unwrap_or_else(|error| panic!("{table}: {error}"));
    }
}
