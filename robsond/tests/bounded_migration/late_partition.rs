//! Existing Blackhole after a real return; server cleanup, never EOF-orphan.
use robson_testkit::db_proxy::{DbProxy, DiagnosticDelivery, DiagnosticEvent};
use sqlx::PgConnection;

use super::*;

#[path = "late_partition_scenario.rs"]
mod scenario;
use scenario::run_partition;

const OBSERVE: &str = "SELECT jsonb_build_object('activity',(SELECT jsonb_build_object('pid',a.pid,'backend',a.backend_start::text,'transaction',a.xact_start::text,'xid',a.backend_xid::text,'state',a.state) FROM pg_stat_activity a WHERE a.pid=$1),'locks',(SELECT count(*) FROM pg_locks WHERE pid=$1),'venue',EXISTS(SELECT 1 FROM pg_locks WHERE pid=$1 AND relation::bigint=$2 AND mode='AccessExclusiveLock' AND granted),'clock',EXISTS(SELECT 1 FROM pg_locks WHERE pid=$1 AND relation::bigint=$3 AND mode='AccessExclusiveLock' AND granted),'metadata',EXISTS(SELECT 1 FROM pg_locks WHERE pid=$1 AND relation::bigint=$4 AND mode='RowExclusiveLock' AND granted))";
const IDENTIFY: &str = "SELECT jsonb_build_object('backend',(SELECT backend_start::text FROM pg_stat_activity WHERE pid=$1),'venue','public.venue_orders'::regclass::oid::bigint,'clock','public.spot_intent_clock'::regclass::oid::bigint,'metadata','public._sqlx_migrations'::regclass::oid::bigint)";

async fn observe(c: &mut PgConnection, pid: i32, identity: &Value) -> Result<Value> {
    let value: Value = sqlx::query_scalar(OBSERVE)
        .bind(pid)
        .bind(identity["venue"].as_i64().context("venue OID missing")?)
        .bind(identity["clock"].as_i64().context("clock OID missing")?)
        .bind(identity["metadata"].as_i64().context("metadata OID missing")?)
        .fetch_one(c)
        .await?;
    ensure!(serde_json::to_vec(&value)?.len() <= 65536, "partition observer byte bound");
    Ok(value)
}

fn idle(value: &Value, identity: &Value, pid: i32, metadata: bool) -> Result<()> {
    let a = &value["activity"];
    ensure!(
        a["pid"] == pid
            && a["backend"] == identity["backend"]
            && a["backend"].is_string()
            && a["transaction"].is_string()
            && a["xid"].is_string()
            && a["state"] == "idle in transaction"
            && value["locks"].as_u64().is_some_and(|n| n > 0)
            && value["venue"] == true
            && value["clock"] == true
            && value["metadata"] == metadata,
        "partition native idle/identity/locks missing"
    );
    Ok(())
}

fn idle_report(
    value: &Value,
    identity: &Value,
    pid: i32,
    metadata: bool,
    phase: &'static str,
    poll: usize,
    elapsed: Duration,
) -> Result<()> {
    let result = idle(value, identity, pid, metadata);
    if result.is_err() {
        let a = &value["activity"];
        let payload = serde_json::json!({
            "phase": phase, "poll": poll, "elapsed_us": elapsed.as_micros(),
            "actual": {"pid": a["pid"], "backend": a["backend"],
                "transaction": a["transaction"], "xid": a["xid"], "state": a["state"],
                "locks": value["locks"], "venue": value["venue"],
                "clock": value["clock"], "metadata": value["metadata"]},
            "expected": {"pid": pid, "backend": identity["backend"],
                "venue_oid": identity["venue"], "clock_oid": identity["clock"],
                "metadata_oid": identity["metadata"], "metadata": metadata}
        });
        match serde_json::to_string(&payload) {
            Ok(encoded) if encoded.len() <= 4000 => eprintln!("PARTITION_OBSERVATION_FAILURE {encoded}"),
            _ => eprintln!("PARTITION_OBSERVATION_FAILURE phase={phase} poll={poll} diagnostic=unavailable_or_over4000"),
        }
    }
    result
}

fn retired(value: &Value) -> bool {
    value["activity"].is_null() && value["locks"] == 0
}

fn retiring(
    value: &Value,
    before: &Value,
    identity: &Value,
    pid: i32,
    poll: usize,
    elapsed: Duration,
) -> Result<()> {
    let a = &value["activity"];
    let old = &before["activity"];
    let valid = a["pid"] == pid
        && a["backend"] == identity["backend"]
        && (a["transaction"] == old["transaction"] || a["transaction"].is_null())
        && (a["xid"] == old["xid"] || a["xid"].is_null());
    if !valid {
        let payload = serde_json::json!({
            "phase": "retirement", "poll": poll, "elapsed_us": elapsed.as_micros(),
            "actual": {"pid": a["pid"], "backend": a["backend"],
                "transaction": a["transaction"], "xid": a["xid"], "state": a["state"],
                "locks": value["locks"], "venue": value["venue"],
                "clock": value["clock"], "metadata": value["metadata"]},
            "expected": {"pid": pid, "backend": identity["backend"],
                "transaction_original_or_null": old["transaction"],
                "xid_original_or_null": old["xid"]}
        });
        match serde_json::to_string(&payload) {
            Ok(encoded) if encoded.len() <= 4000 => {
                eprintln!("PARTITION_RETIREMENT_FAILURE {encoded}")
            },
            _ => eprintln!(
                "PARTITION_RETIREMENT_FAILURE poll={poll} diagnostic=unavailable_or_over4000"
            ),
        }
    }
    ensure!(valid, "partition retirement PID/backend/new-transaction drift");
    Ok(())
}

fn timeout_evidence(proxy: &DbProxy, pid: i32) -> Result<()> {
    let d = proxy.diagnostics().context("partition diagnostics missing")?;
    ensure!(
        d.dropped == 0
            && d.connection_overflow == 0
            && d.phase_overflow == 0
            && d.connections.len() == 1
            && d.connections[0].pid == Some(pid),
        "partition diagnostic overflow/identity"
    );
    // Fatal responses deliberately lose phase attribution, not actual PID.
    let records: Vec<_> = d
        .records
        .iter()
        .filter(|r| {
            r.identity.connection == d.connections[0].connection
                && r.identity.pid == Some(pid)
                && r.event == DiagnosticEvent::Error
                && r.sqlstate == Some(*b"25P03")
        })
        .collect();
    ensure!(
        records.iter().filter(|r| r.delivery == DiagnosticDelivery::Received).count() == 1
            && records.iter().filter(|r| r.delivery == DiagnosticDelivery::Dropped).count() == 1,
        "actual server idle25P03 Received+Dropped missing"
    );
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires DATABASE_URL"]
async fn actual52_partition_after_ddl43_releases_server_locks(pool: PgPool) -> Result<()> {
    run_partition(pool, Cut::AfterDdl(43)).await
}

#[sqlx::test(migrations = false)]
#[ignore = "requires DATABASE_URL"]
async fn actual52_partition_after_ddl52_releases_server_locks(pool: PgPool) -> Result<()> {
    run_partition(pool, Cut::AfterDdl(52)).await
}

#[sqlx::test(migrations = false)]
#[ignore = "requires DATABASE_URL"]
async fn actual52_partition_after_metadata_releases_server_locks(pool: PgPool) -> Result<()> {
    run_partition(pool, Cut::AfterMetadata).await
}

#[cfg(test)]
mod retirement_guards {
    use super::*;

    fn snapshot(pid: i32, xid: Option<&str>, locks: u64) -> Value {
        serde_json::json!({"activity": {"pid":pid,
            "backend":"2026-11-17 13:28:37.808715+00",
            "transaction":"2026-11-17 13:28:37.911301+00", "xid":xid,
            "state":"idle in transaction"}, "locks":locks,
            "venue":locks>1, "clock":locks>1, "metadata":false})
    }

    #[test]
    fn recorded_teardown_releases_resources_before_activity_exit() {
        // Recorded failing sample: same43/backend/transaction, xidNULL,
        // locks1 and both relation locks gone. Original xid is immaterial.
        let before = snapshot(43, Some("7"), 3);
        let identity = serde_json::json!({"backend":before["activity"]["backend"]});
        let middle = snapshot(43, None, 1);
        assert!(retiring(&middle, &before, &identity, 43, 4, Duration::ZERO).is_ok());
        assert!(!retired(&middle));
        let residual = serde_json::json!({"activity":null,"locks":1});
        assert!(!retired(&residual));
        assert!(retired(&serde_json::json!({"activity":null,"locks":0})));
    }

    proptest::proptest! {
        #![proptest_config(proptest::test_runner::Config::with_cases(64))]
        #[test]
        fn new_owner_or_transaction_is_never_accepted(
            pid in 1_i32..1_000_000, xid in 1_u64..1_000_000, field in 0_u8..4,
        ) {
            let xid = xid.to_string();
            let before = snapshot(pid, Some(&xid), 3);
            let identity = serde_json::json!({"backend":before["activity"]["backend"]});
            let mut changed = before.clone();
            match field {
                0 => changed["activity"]["pid"] = (pid + 1).into(),
                1 => changed["activity"]["backend"] = "reused backend".into(),
                2 => changed["activity"]["transaction"] = "new transaction".into(),
                _ => changed["activity"]["xid"] = format!("{xid}0").into(),
            }
            proptest::prop_assert!(retiring(&changed, &before, &identity, pid, 2, Duration::ZERO).is_err());
            proptest::prop_assert!(!retired(&changed));
        }

        #[test]
        fn trace_completes_only_after_all_native_resources_exit(
            states in proptest::collection::vec(0_u8..3, 1..32),
        ) {
            let before = snapshot(43, Some("7"), 3);
            let identity = serde_json::json!({"backend":before["activity"]["backend"]});
            let mut trace = Vec::with_capacity(states.len() + 1);
            for state in states {
                let value = match state {
                    0 => before.clone(),
                    1 => snapshot(43, None, 0),
                    _ => serde_json::json!({"activity":null,"locks":1}),
                };
                if !value["activity"].is_null() {
                    proptest::prop_assert!(retiring(&value, &before, &identity, 43, 2, Duration::ZERO).is_ok());
                }
                trace.push(value);
            }
            let terminal = trace.len();
            trace.push(serde_json::json!({"activity":null,"locks":0}));
            proptest::prop_assert_eq!(trace.iter().position(retired), Some(terminal));
        }
    }
}
