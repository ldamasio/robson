//! Original DO is an independent, rolled-back TEST oracle; never a ready grant.
#[path = "precommit.rs"]
mod cancellation;
#[path = "coexistence.rs"]
mod coexistence;
#[path = "catalog_contract.rs"]
mod contract;
#[path = "late_commit_absence.rs"]
mod late_commit_absence;
#[path = "late_failures.rs"]
mod late_failures;
#[path = "ordinary_lineage.rs"]
mod ordinary_lineage;

use std::time::Duration;

use anyhow::{ensure, Context, Result};
use serde_json::Value;
use sqlx::{ConnectOptions, Connection, PgConnection, PgPool};
use uuid::Uuid;

use super::{helpers, *};

const QUERY: &str = include_str!("catalog_query.sql");
async fn capture(c: &mut PgConnection, stage: &'static str) -> Result<Value> {
    let text: String = sqlx::query_scalar(QUERY).persistent(false).fetch_one(c).await?;
    ensure!(text.len() <= 4 * 1024 * 1024, "catalog wire payload bound");
    let value: Value = serde_json::from_str(&text).context("catalog TEXT JSON decode")?;
    if value["overflow"] == true {
        match overflow_diagnostic(&value, stage) {
            Ok(message) => eprintln!("{message}"),
            Err(_) => eprintln!("CATALOG_OVERFLOW stage={stage} diagnostic=invalid"),
        }
    }
    contract::maps(&value)?;
    ensure!(
        value["context"].is_object() && serde_json::to_vec(&value["context"])?.len() <= 65536,
        "catalog context byte bound"
    );
    Ok(value)
}
// This log is diagnostic only; malformed diagnostics never bypass maps().
fn overflow_diagnostic(value: &Value, stage: &'static str) -> Result<String> {
    ensure!(matches!(stage, "B0" | "R" | "B1" | "C"), "diagnostic stage");
    const FIELDS: [&str; 12] = [
        "precheck_n",
        "objects_n",
        "indexes_n",
        "dependencies_n",
        "public_names_n",
        "encoded_n",
        "unique_n",
        "null_key_n",
        "max_bytes",
        "bytes",
        "context_bytes",
        "body_bytes",
    ];
    let diagnostic = value["diagnostic"].as_object().context("diagnostic object")?;
    ensure!(diagnostic.len() == FIELDS.len() + 1, "diagnostic fields");
    let mut message = format!("CATALOG_OVERFLOW stage={stage}");
    for field in FIELDS {
        let number = diagnostic.get(field).and_then(Value::as_u64).context("diagnostic integer")?;
        message.push_str(&format!(" {field}={number}"));
    }
    let keys = diagnostic
        .get("duplicate_keys")
        .and_then(Value::as_array)
        .context("diagnostic keys")?;
    ensure!(keys.len() <= 4, "diagnostic key count");
    for (index, key) in keys.iter().enumerate() {
        ensure!(key.as_object().is_some_and(|v| v.len() == 3), "diagnostic key fields");
        let prefix = key["key_prefix"].as_str().context("diagnostic prefix")?;
        let multiplicity = key["multiplicity"].as_u64().context("diagnostic multiplicity")?;
        let bytes = key["key_bytes"].as_u64().context("diagnostic key bytes")?;
        ensure!(
            prefix.chars().count() <= 96 && multiplicity >= 2 && bytes >= prefix.len() as u64,
            "diagnostic key shape"
        );
        let escaped = serde_json::to_string(prefix)?;
        message.push_str(&format!(
            " dup{index}={escaped} multiplicity={multiplicity} key_bytes={bytes}"
        ));
    }
    ensure!(message.len() <= 4096, "diagnostic log bytes");
    Ok(message)
}
// Diagnostics classify errors without formatting server messages or row
// payloads.
fn cause<T>(result: &Result<T>) -> String {
    let Err(error) = result else {
        return "ok".into();
    };
    for source in error.chain() {
        if let Some(error) = source.downcast_ref::<sqlx::Error>() {
            return match error {
                sqlx::Error::Database(db) => {
                    let code = db.code();
                    let code = code
                        .as_deref()
                        .filter(|s| s.len() == 5 && s.bytes().all(|b| b.is_ascii_alphanumeric()))
                        .unwrap_or("unknown");
                    format!("database:{code}")
                },
                sqlx::Error::Io(error) => format!("io:{:?}", error.kind()),
                sqlx::Error::Protocol(_) => "protocol".into(),
                sqlx::Error::Decode(_) | sqlx::Error::ColumnDecode { .. } => "decode".into(),
                _ => "sqlx_other".into(),
            };
        }
        if source.is::<tokio::time::error::Elapsed>() {
            return "deadline".into();
        }
    }
    "invariant_or_other".into()
}
fn initial_context(v: &Value, pid: i32) -> Result<Value> {
    let c = &v["context"];
    ensure!(matches!(c["major"].as_i64(), Some(12 | 16)), "PG12/16 required");
    ensure!(c["pid"] == pid && c["schema"] == "public", "actual catalog backend/schema");
    ensure!(
        c["role"].is_string() && c["search_path"].is_string() && c["encoding"].is_string(),
        "catalog context incomplete"
    );
    ensure!(
        c["lock"] == "310ms" && c["statement"] == "410ms" && c["idle"] == "510ms",
        "reference GUCs changed"
    );
    ensure!(c["metadata"].is_null(), "38 already present before reference");
    Ok(c.clone())
}
async fn finish(
    adapter: BoundedMigration,
    mut observer: PgConnection,
    pid: Option<i32>,
    legacy_before: Option<&Value>,
) -> Result<()> {
    let deadline = tokio::time::Instant::now() + ACQUIRE_BOUND;
    let status = adapter.status();
    drop(adapter); // Also discards a queued reference ROLLBACK on failure.
    let retirement = tokio::time::timeout(NATIVE_RETIREMENT, async {
        let pid = pid.context("backend PID unavailable; no retirement proof")?;
        for _ in 0..NATIVE_RETIREMENT_POLLS {
            if !helpers::alive(&mut observer, pid).await? {
                return Ok::<_, anyhow::Error>(true);
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        Ok(false)
    })
    .await
    .context("catalog retirement deadline")
    .and_then(|r| r);
    let snapshots = tokio::time::timeout_at(deadline, async {
        observer.lock().await?;
        let legacy = helpers::legacy(&mut observer).await?;
        let metadata = helpers::metadata(&mut observer).await?;
        observer.unlock().await?;
        Ok::<_, anyhow::Error>((legacy, metadata))
    })
    .await
    .context("catalog cleanup snapshots deadline")
    .and_then(|r| r);
    drop(observer);
    eprintln!(
        "CATALOG_CLEANUP status={status:?} retired={:?} retirement_cause={} snapshots_cause={}",
        retirement.as_ref().ok(),
        cause(&retirement),
        cause(&snapshots)
    );
    let retired = retirement?;
    let (legacy, _) = snapshots?;
    ensure!(retired, "catalog actor backend not retired");
    if let Some(before) = legacy_before {
        ensure!(&legacy == before, "catalog test changed legacy");
    }
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires DATABASE_URL"]
async fn actual52_catalog_matches_rolled_back_original_do(pool: PgPool) -> Result<()> {
    use std::borrow::Cow;
    let mut through37 = sqlx::migrate!("../migrations");
    through37.migrations =
        Cow::Owned(through37.iter().filter(|m| m.version < VERSION).cloned().collect());
    through37.run(&pool).await?;
    sqlx::query("INSERT INTO order_intents (intent_id,tenant_id,venue_id,account_scope,client_order_id,kind,params,created_at) VALUES ($1,$2,'binance:usdm','scope','catalog-original','exit','{}',now())")
        .bind(Uuid::now_v7()).bind(Uuid::now_v7()).execute(&pool).await?;
    sqlx::query("INSERT INTO submit_attempts (attempt_id,venue_id,account_scope,client_order_id,state,created_at) VALUES ($1,'binance:usdm','scope','catalog-original','started',now())")
        .bind(Uuid::now_v7()).execute(&pool).await?;
    let options = pool.connect_options();
    let mut observer = tokio::time::timeout(ACQUIRE_BOUND, options.connect()).await??;
    let legacy = tokio::time::timeout(ACQUIRE_BOUND, helpers::legacy(&mut observer))
        .await
        .context("initial legacy deadline")
        .and_then(|r| r.map_err(anyhow::Error::from));
    eprintln!("CATALOG_INITIAL_LEGACY cause={}", cause(&legacy));
    let (legacy, legacy_error) = match legacy {
        Ok(value) => (Some(value), None),
        Err(error) => (None, Some(error)),
    };
    let mut adapter = match BoundedMigration::connect(&options).await {
        Ok(adapter) => adapter,
        Err(error) => {
            drop(observer);
            return Err(legacy_error.unwrap_or_else(|| error.into()));
        },
    };
    let configured = tokio::time::timeout(ACQUIRE_BOUND, async {
        let c = adapter.connection()?;
        Ok::<_, MigrateError>(sqlx::query_as::<_, (i32, String, String, String)>(
            "SELECT pg_backend_pid(),set_config('lock_timeout','310ms',false),set_config('statement_timeout','410ms',false),set_config('idle_in_transaction_session_timeout','510ms',false)"
        ).fetch_one(c).await?)
    }).await.context("actor GUC/PID deadline").and_then(|r| r.map_err(anyhow::Error::from));
    let pid = configured.as_ref().ok().map(|v| v.0);
    let primary = tokio::time::timeout(APPLY_FIXTURE, async {
        if let Some(error) = legacy_error { return Err(error.context("initial legacy snapshot failed")); }
        let (pid, lock, statement, idle) = configured?;
        ensure!((lock.as_str(), statement.as_str(), idle.as_str()) == ("310ms", "410ms", "510ms"), "reference setup GUC mismatch");
        let source_before = contract::source_hash()?;
        let baseline = capture(adapter.connection()?, "B0").await?;
        let context = initial_context(&baseline, pid)?;
        // No candidate chunks/ranges/helper are used for the reference.
        let reference = {
            let mut transaction = adapter.connection()?.begin().await?;
            contract::run_original(&mut transaction).await?;
            let reference = capture(&mut transaction, "R").await?;
            transaction.rollback().await?;
            reference
        };
        ensure!(reference["context"] == context, "reference changed context/GUC/history");
        contract::declared(&reference)?;
        let rolled_back = capture(adapter.connection()?, "B1").await?;
        ensure!(rolled_back["context"] == context, "rollback context/GUC/history drift");
        {
            let (before, before_ids) = contract::maps(&baseline)?;
            let (after, after_ids) = contract::maps(&rolled_back)?;
            ensure!(before == after && before_ids == after_ids, "reference rollback did not preserve complete baseline");
        }
        drop(baseline);
        drop(rolled_back);
        ensure!(contract::source_hash()? == source_before, "source drift after reference");
        let started = std::time::Instant::now();
        let applied = adapter.apply(helpers::migration()).await;
        let duration = started.elapsed();
        eprintln!("CATALOG_PRIMARY apply_ok={} duration={duration:?}", applied.is_ok());
        applied?;
        ensure!(duration < APPLY_BOUND, "candidate global APPLY_BOUND exceeded");
        ensure!(adapter.status().phase == Phase::Restored && adapter.status().failure.is_none(), "candidate not restored");
        let candidate = capture(adapter.connection()?, "C").await?;
        contract::declared(&candidate)?;
        let mut candidate_context = candidate["context"].clone();
        let metadata = candidate_context["metadata"].take();
        ensure!(candidate_context == context, "candidate same-PID context/GUC restoration mismatch");
        ensure!(metadata["description"] == helpers::migration().description.as_ref() && metadata["success"] == true, "candidate metadata identity");
        ensure!(metadata["checksum"] == contract::CHECKSUM, "candidate original checksum");
        let elapsed = metadata["elapsed"].as_str().context("candidate elapsed missing")?.parse::<i64>()?;
        ensure!(elapsed >= 0, "candidate elapsed missing durable update");
        let (expected, _) = contract::maps(&reference)?;
        let (actual, _) = contract::maps(&candidate)?;
        ensure!(expected == actual, "full semantic catalog differs from original DO reference");
        ensure!(contract::source_hash()? == source_before, "source drift after candidate");
        eprintln!("CATALOG_COUNTS reference_records={} candidate_records={} payload_cap=4194304 source={source_before}", expected.len(), actual.len());
        Ok::<_, anyhow::Error>(())
    }).await.context("reference/candidate observation deadline").and_then(|r| r);
    // Never propagate reference/rollback/candidate/observation error before raw
    // owner drop and the one shared cleanup ceiling have actually completed.
    let cleanup = finish(adapter, observer, pid, legacy.as_ref()).await;
    eprintln!(
        "CATALOG_RESULT primary_cause={} cleanup_cause={}",
        cause(&primary),
        cause(&cleanup)
    );
    primary?;
    cleanup?;
    Ok(())
}
