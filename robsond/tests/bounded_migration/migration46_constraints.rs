//! Every CHECK, FOREIGN KEY, UNIQUE and trigger rule of S8-4 (PLAN-r4 §4.3,
//! with the re-anchor `mode` of operator decision Q2 = R), on the installed
//! schema. Rows are built as JSON and inserted with
//! `jsonb_populate_record`, so each refusal case changes one field.
use serde_json::{json, Value};
use uuid::Uuid;

use super::*;

const VENUE: &str = "binance:spot";
const NIL: &str = "00000000-0000-0000-0000-000000000000";

/// The production route: through 45 in order, then 46 to 50 through the
/// bounded adapter and the official runner; then all five S8-4 schemas exist.
async fn install(pool: &PgPool) -> Result<()> {
    install_through(pool, VERSION50).await?;
    let mut observer = pool.acquire().await?;
    for version in VERSIONS {
        ensure!(landed(&mut *observer, version).await?, "M{} missing", version % 100);
    }
    Ok(())
}

/// A venue scope: its version row, a process epoch and a clock sample.
#[derive(Clone, Copy)]
struct Scope {
    tenant: Uuid,
    sample: Uuid,
    epoch: Uuid,
}

async fn scope(pool: &PgPool) -> Result<Scope> {
    let tenant = Uuid::now_v7();
    let epoch = Uuid::now_v7();
    let sample = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO accounting_state(tenant_id,venue_id,budget_model) VALUES($1,$2,'hwm_v1')",
    )
    .bind(tenant)
    .bind(VENUE)
    .execute(pool)
    .await?;
    sqlx::query("INSERT INTO process_epochs (epoch_id,tenant_id,binary_version) VALUES ($1,$2,'s84-fixture')")
        .bind(epoch)
        .bind(tenant)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO clock_samples (sample_id,tenant_id,venue_id,process_epoch,clock_domain,send_wall_seconds,send_wall_nanoseconds,send_tick_before,send_tick_after,receive_wall_seconds,receive_wall_nanoseconds,receive_tick_before,receive_tick_after,server_ms,theta2_ns,r_low_ns,r_high_ns,u2_ns,numeric_verdict,numeric_failures,algorithm_version,recorded_at) VALUES ($1,$2,$3,$4,ROW('boottime',$5::uuid,0,0)::budget_clock_domain_v38,1794614400,0,0,0,1794614400,0,0,0,0,-3589228800000000000,0,0,3589228800200000000,'invalid',1,1,now())")
        .bind(sample).bind(tenant).bind(VENUE).bind(epoch).bind(Uuid::now_v7())
        .execute(pool).await?;
    Ok(Scope { tenant, sample, epoch })
}

/// A tenant with a process epoch and a clock sample but no version row.
async fn orphan(pool: &PgPool) -> Result<Scope> {
    let s = scope(pool).await?;
    let orphan = Uuid::now_v7();
    sqlx::query("INSERT INTO process_epochs (epoch_id,tenant_id,binary_version) VALUES ($1,$2,'s84-orphan')")
        .bind(orphan)
        .bind(orphan)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO clock_samples SELECT (jsonb_populate_record(c, jsonb_build_object('sample_id',$1::uuid,'tenant_id',$1::uuid,'process_epoch',$1::uuid))).* FROM clock_samples c WHERE sample_id=$2")
        .bind(orphan)
        .bind(s.sample)
        .execute(pool)
        .await?;
    Ok(Scope {
        tenant: orphan,
        sample: orphan,
        epoch: orphan,
    })
}

/// Milliseconds after 2026-11-10 12:00:00 UTC.
fn ts(ms: i64) -> String {
    let t = 12 * 3_600_000 + ms;
    format!(
        "2026-11-10 {:02}:{:02}:{:02}.{:03}+00",
        t / 3_600_000,
        t / 60_000 % 60,
        t / 1000 % 60,
        t % 1000
    )
}

/// An observation whose cut is `cut`: sent 150 ms and received 350 ms after
/// it with u = 150 ms, so its interval is [cut, cut + 500 ms].
fn observe(row: &mut Value, prefix: &str, s: Scope, cut: i64) {
    let row = row.as_object_mut().expect("row object");
    for (key, value) in [
        ("send_at", json!(ts(cut + 150))),
        ("receive_at", json!(ts(cut + 350))),
        ("u_ns", json!(150_000_000)),
        ("sample_id", json!(s.sample)),
        ("epoch_id", json!(s.epoch)),
        ("interval_start", json!(ts(cut))),
        ("interval_end", json!(ts(cut + 500))),
    ] {
        row.insert(format!("{prefix}{key}"), value);
    }
}

fn anchor_row(s: Scope, id: Uuid, kind: &str, cutover: Option<Uuid>, cut: Option<i64>) -> Value {
    let mut row = json!({
        "anchor_id": id, "tenant_id": s.tenant, "venue_id": VENUE, "kind": kind,
        "cutover_id": cutover, "quote": "1000",
    });
    if let Some(cut) = cut {
        observe(&mut row, "", s, cut);
        row["as_of"] = json!(ts(cut));
        row["depends_start"] = json!(ts(cut - 10_000));
        row["depends_end"] = json!(ts(cut + 500));
    }
    row
}

fn checkpoint_row(s: Scope, cut: i64, source: &str, anchor: Option<Uuid>) -> Value {
    let mut row = json!({
        "tenant_id": s.tenant, "venue_id": VENUE, "cut": ts(cut), "source": source, "anchor_id": anchor,
    });
    observe(&mut row, "", s, cut);
    row
}

fn discrepancy_row(s: Scope, id: Uuid, direction: &str, delta: &str, cut: i64) -> Value {
    let mut row = json!({
        "discrepancy_id": id, "tenant_id": s.tenant, "venue_id": VENUE, "direction": direction,
        "state": "open", "first_delta": delta, "checkpoint_cut": ts(0),
        "checkpoint_interval_start": ts(0), "checkpoint_interval_end": ts(500), "opened_version": 3,
        "last_seen_at": ts(cut + 350), "min_delta": delta,
    });
    observe(&mut row, "first_", s, cut);
    row
}

fn sighting_row(s: Scope, discrepancy: Uuid, seq: i32, delta: &str, cut: i64) -> Value {
    let mut row = json!({
        "discrepancy_id": discrepancy, "seq": seq, "tenant_id": s.tenant, "venue_id": VENUE, "delta": delta,
    });
    observe(&mut row, "", s, cut);
    row
}

/// An attested re-anchor: delta -5 against the old expected quote, minimum
/// -10, so W = 10 and C = 5, with O1 at 60 s and O2 at 70 s.
fn reanchor_row(s: Scope, id: Uuid, old: Uuid, new: Uuid) -> Value {
    let mut row = json!({
        "reanchor_id": id, "tenant_id": s.tenant, "venue_id": VENUE, "operator_subject": "operator-fixture",
        "reason": "attested withdrawal", "o1_total": "995", "o2_total": "995", "mode": "attested",
        "checked_version": 7, "old_anchor_id": old, "new_anchor_id": new, "dating_cut": ts(0),
        "dating_interval_start": ts(0), "dating_interval_end": ts(500), "delta": "-5", "min_delta": "-10",
        "withdrawal": "10", "contribution": "5", "resolved_discrepancy_id": null,
        "excluded_sighting_seqs": [], "voided_discrepancy_id": null,
    });
    observe(&mut row, "o1_", s, 60_000);
    observe(&mut row, "o2_", s, 70_000);
    row
}

/// The same command in recovery mode: no delta, dating copy or amounts.
fn recovery_row(s: Scope, id: Uuid, old: Uuid, new: Uuid) -> Value {
    let mut row = reanchor_row(s, id, old, new);
    row["mode"] = json!("recovery");
    for key in [
        "dating_cut",
        "dating_interval_start",
        "dating_interval_end",
        "delta",
        "min_delta",
        "withdrawal",
        "contribution",
    ] {
        row[key] = Value::Null;
    }
    row
}

fn flow_row(
    s: Scope,
    reanchor: Uuid,
    direction: &str,
    amount: &str,
    date: &str,
    month: &str,
) -> Value {
    json!({
        "tenant_id": s.tenant, "venue_id": VENUE, "economic_date": date, "flow_id": Uuid::now_v7(),
        "reanchor_id": reanchor, "direction": direction, "amount": amount,
        "date_rule": if direction == "withdrawal" { "checkpoint_cut" } else { "o1_receipt_plus_u" },
        "attributed_month": month, "reanchor_mode": "attested",
    })
}

/// `table` is always one of this file's fixed S8-4 table names.
async fn insert<'e>(
    executor: impl sqlx::PgExecutor<'e>,
    table: &str,
    row: &Value,
) -> Result<(), sqlx::Error> {
    sqlx::query(&format!(
        "INSERT INTO public.{table} SELECT * FROM jsonb_populate_record(NULL::public.{table}, $1)"
    ))
    .bind(row)
    .execute(executor)
    .await
    .map(|_| ())
}

fn set(row: &Value, pairs: &[(&str, Value)]) -> Value {
    let mut row = row.clone();
    for (key, value) in pairs {
        row[*key] = value.clone();
    }
    row
}

fn with(row: &Value, key: &str, value: Value) -> Value {
    let mut row = row.clone();
    row[key] = value;
    row
}

async fn exec(pool: &PgPool, sql: &str, id: Uuid) -> Result<u64, sqlx::Error> {
    sqlx::query(sql).bind(id).execute(pool).await.map(|done| done.rows_affected())
}

/// Refused natively with `code`, naming `needle` as the constraint or in
/// the message (a trigger's).
fn refused<T: std::fmt::Debug>(
    result: Result<T, sqlx::Error>,
    code: &str,
    needle: &str,
) -> Result<()> {
    let error = match result {
        Ok(accepted) => anyhow::bail!("accepted {accepted:?}; expected {code} {needle}"),
        Err(error) => error,
    };
    let db = error
        .as_database_error()
        .with_context(|| format!("not a native rejection: {error}"))?;
    ensure!(
        db.code().as_deref() == Some(code)
            && (db.constraint() == Some(needle) || db.message().contains(needle)),
        "expected {code} {needle}, got {db:?}"
    );
    Ok(())
}

const IMMUTABLE: &str = "rows are never updated or deleted";

/// A cutover anchor at `base` and a re-anchor's anchor 70 s later.
async fn anchors(pool: &PgPool, s: Scope, base: i64) -> Result<(Uuid, Uuid)> {
    let (old, new) = (Uuid::now_v7(), Uuid::now_v7());
    insert(
        pool,
        "cash_anchors",
        &anchor_row(s, old, "cutover", Some(Uuid::now_v7()), Some(base)),
    )
    .await?;
    insert(pool, "cash_anchors", &anchor_row(s, new, "reanchor", None, Some(base + 70_000)))
        .await?;
    Ok((old, new))
}

const SET_OBSERVATION: &str = "UPDATE cash_anchors SET as_of=c, send_at=c+interval '150 ms',
    receive_at=c+interval '350 ms', u_ns=150000000, sample_id=$3, epoch_id=$4, interval_start=c,
    interval_end=c+interval '500 ms', depends_start=c-interval '10 s', depends_end=c+interval '500 ms'
    FROM (SELECT $2::timestamptz AS c) t WHERE anchor_id=$1";

async fn set_observation(
    pool: &PgPool,
    s: Scope,
    anchor: Uuid,
    cut: i64,
) -> Result<u64, sqlx::Error> {
    sqlx::query(SET_OBSERVATION)
        .bind(anchor)
        .bind(ts(cut))
        .bind(s.sample)
        .bind(s.epoch)
        .execute(pool)
        .await
        .map(|done| done.rows_affected())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn cash_anchor_as_of_is_set_once(pool: PgPool) -> Result<()> {
    install(&pool).await?;
    let s = scope(&pool).await?;
    // 8m's pending cutover anchor: quote, no observation yet.
    let pending = Uuid::now_v7();
    insert(
        &pool,
        "cash_anchors",
        &anchor_row(s, pending, "cutover", Some(Uuid::now_v7()), None),
    )
    .await?;
    ensure!(set_observation(&pool, s, pending, 1_000).await? == 1, "completion sets it once");
    refused(
        set_observation(&pool, s, pending, 2_000).await,
        "23514",
        "only sets its cutover observation once",
    )?;
    refused(
        exec(&pool, "UPDATE cash_anchors SET as_of=NULL,send_at=NULL,receive_at=NULL,u_ns=NULL,sample_id=NULL,epoch_id=NULL,interval_start=NULL,interval_end=NULL,depends_start=NULL,depends_end=NULL WHERE anchor_id=$1", pending).await,
        "23514",
        "only sets its cutover observation once",
    )?;
    refused(
        exec(&pool, "UPDATE cash_anchors SET quote=quote+1 WHERE anchor_id=$1", pending).await,
        "23514",
        "only sets",
    )?;
    refused(exec(&pool, "UPDATE cash_anchors SET cutover_id='00000000-0000-0000-0000-000000000001'::uuid WHERE anchor_id=$1", pending).await, "23514", "only sets")?;
    refused(
        exec(&pool, "DELETE FROM cash_anchors WHERE anchor_id=$1", pending).await,
        "23514",
        "never deleted",
    )?;
    // A re-anchor's anchor is written whole and never changes.
    let reanchor = Uuid::now_v7();
    insert(&pool, "cash_anchors", &anchor_row(s, reanchor, "reanchor", None, Some(5_000))).await?;
    refused(set_observation(&pool, s, reanchor, 6_000).await, "23514", "only sets")?;
    refused(
        exec(&pool, "UPDATE cash_anchors SET kind='cutover' WHERE anchor_id=$1", reanchor).await,
        "23514",
        "only sets",
    )?;
    // An update that changes nothing is not a second setting.
    ensure!(
        exec(&pool, "UPDATE cash_anchors SET quote=quote WHERE anchor_id=$1", reanchor).await? == 1
    );
    let first: bool =
        sqlx::query_scalar("SELECT as_of=$2::timestamptz FROM cash_anchors WHERE anchor_id=$1")
            .bind(pending)
            .bind(ts(1_000))
            .fetch_one(&pool)
            .await?;
    ensure!(first, "the first setting stands");
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn cash_anchors_refuse_incoherent_rows(pool: PgPool) -> Result<()> {
    install(&pool).await?;
    let s = scope(&pool).await?;
    let other = scope(&pool).await?;
    let unknown = orphan(&pool).await?;
    let good = anchor_row(s, Uuid::now_v7(), "reanchor", None, Some(5_000));
    let fresh = |row: &Value| with(row, "anchor_id", json!(Uuid::now_v7()));
    for (row, code, needle) in [
        (
            anchor_row(s, Uuid::now_v7(), "reanchor", None, None),
            "23514",
            "cash_anchors_pending",
        ),
        (
            anchor_row(s, Uuid::now_v7(), "cutover", None, Some(1)),
            "23514",
            "cash_anchors_cutover",
        ),
        (
            anchor_row(s, Uuid::now_v7(), "reanchor", Some(Uuid::now_v7()), Some(1)),
            "23514",
            "cash_anchors_cutover",
        ),
        (
            anchor_row(s, Uuid::now_v7(), "boundary", None, Some(1)),
            "23514",
            "cash_anchors_kind",
        ),
        (
            with(&fresh(&good), "sample_id", Value::Null),
            "23514",
            "cash_anchors_observation_whole",
        ),
        (
            with(&fresh(&good), "as_of", json!(ts(5_001))),
            "23514",
            "cash_anchors_observation",
        ),
        (
            with(&fresh(&good), "send_at", json!(ts(5_400))),
            "23514",
            "cash_anchors_observation",
        ),
        (with(&fresh(&good), "u_ns", json!(-1)), "23514", "cash_anchors_observation"),
        (
            with(&fresh(&good), "depends_end", json!(ts(5_499))),
            "23514",
            "cash_anchors_hull",
        ),
        (
            with(&fresh(&good), "depends_start", json!(ts(5_001))),
            "23514",
            "cash_anchors_hull",
        ),
        (with(&fresh(&good), "quote", json!("-0.01")), "23514", "cash_anchors_quote"),
        (with(&fresh(&good), "quote", json!("NaN")), "23514", "cash_anchors_quote"),
        (with(&good, "anchor_id", json!(NIL)), "23514", "cash_anchors_ids"),
        (with(&fresh(&good), "epoch_id", json!(NIL)), "23514", "cash_anchors_ids"),
        (
            anchor_row(unknown, Uuid::now_v7(), "reanchor", None, Some(5_000)),
            "23503",
            "cash_anchors_version_row",
        ),
        (
            with(&fresh(&good), "sample_id", json!(other.sample)),
            "23503",
            "cash_anchors_sample",
        ),
    ] {
        refused(insert(&pool, "cash_anchors", &row).await, code, needle)?;
    }
    insert(&pool, "cash_anchors", &good).await?;
    refused(
        insert(&pool, "cash_anchors", &fresh(&good)).await,
        "23505",
        "cash_anchors_as_of_once",
    )?;
    let cutover = Uuid::now_v7();
    insert(
        &pool,
        "cash_anchors",
        &anchor_row(s, Uuid::now_v7(), "cutover", Some(cutover), None),
    )
    .await?;
    refused(
        insert(
            &pool,
            "cash_anchors",
            &anchor_row(s, Uuid::now_v7(), "cutover", Some(cutover), Some(9_000)),
        )
        .await,
        "23505",
        "cash_anchors_cutover_once",
    )?;
    // The same cut in another scope is another anchor.
    insert(
        &pool,
        "cash_anchors",
        &anchor_row(other, Uuid::now_v7(), "reanchor", None, Some(5_000)),
    )
    .await?;
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn cash_checkpoints_follow_their_anchor(pool: PgPool) -> Result<()> {
    install(&pool).await?;
    let s = scope(&pool).await?;
    let other = scope(&pool).await?;
    let unknown = orphan(&pool).await?;
    let (cutover, reanchor) = anchors(&pool, s, 0).await?;
    let pending = Uuid::now_v7();
    insert(
        &pool,
        "cash_anchors",
        &anchor_row(s, pending, "cutover", Some(Uuid::now_v7()), None),
    )
    .await?;
    for (row, code, needle) in [
        (
            checkpoint_row(s, 1_000, "scan", Some(cutover)),
            "23514",
            "cash_checkpoints_anchor_pair",
        ),
        (
            checkpoint_row(s, 70_000, "reanchor", None),
            "23514",
            "cash_checkpoints_anchor_pair",
        ),
        (
            checkpoint_row(s, 71_000, "reanchor", Some(reanchor)),
            "23503",
            "cash_checkpoints_anchor",
        ),
        (
            checkpoint_row(s, 70_000, "cutover", Some(reanchor)),
            "23503",
            "cash_checkpoints_anchor",
        ),
        (
            checkpoint_row(s, 0, "reanchor", Some(cutover)),
            "23503",
            "cash_checkpoints_anchor",
        ),
        (
            checkpoint_row(s, 0, "cutover", Some(pending)),
            "23503",
            "cash_checkpoints_anchor",
        ),
        (
            checkpoint_row(s, 70_000, "boundary", Some(reanchor)),
            "23514",
            "cash_checkpoints_source",
        ),
        (
            with(&checkpoint_row(s, 1_000, "scan", None), "cut", json!(ts(999))),
            "23514",
            "cash_checkpoints_observation",
        ),
        (
            with(&checkpoint_row(s, 1_000, "scan", None), "sample_id", json!(NIL)),
            "23514",
            "cash_checkpoints_ids",
        ),
        (
            checkpoint_row(unknown, 1_000, "scan", None),
            "23503",
            "cash_checkpoints_version_row",
        ),
        // Scoped links: another venue scope's sample or anchor is refused.
        (
            with(&checkpoint_row(s, 1_000, "scan", None), "sample_id", json!(other.sample)),
            "23503",
            "cash_checkpoints_sample",
        ),
        (
            checkpoint_row(other, 70_000, "reanchor", Some(reanchor)),
            "23503",
            "cash_checkpoints_anchor",
        ),
    ] {
        refused(insert(&pool, "cash_checkpoints", &row).await, code, needle)?;
    }
    // The opening checkpoint is the cutover anchor's own cut; then a pointer
    // that moves in place, with no trigger and no history.
    insert(&pool, "cash_checkpoints", &checkpoint_row(s, 0, "cutover", Some(cutover))).await?;
    let upsert = "INSERT INTO public.cash_checkpoints SELECT * FROM jsonb_populate_record(NULL::public.cash_checkpoints, $1)
        ON CONFLICT (tenant_id, venue_id) DO UPDATE SET cut=EXCLUDED.cut, send_at=EXCLUDED.send_at,
        receive_at=EXCLUDED.receive_at, interval_start=EXCLUDED.interval_start, interval_end=EXCLUDED.interval_end,
        source=EXCLUDED.source, anchor_id=EXCLUDED.anchor_id";
    for row in [
        checkpoint_row(s, 30_000, "scan", None),
        checkpoint_row(s, 70_000, "reanchor", Some(reanchor)),
    ] {
        sqlx::query(upsert).bind(row).execute(&pool).await?;
    }
    let current: (String, bool, Option<Uuid>) = sqlx::query_as(
        "SELECT source, cut=$2::timestamptz, anchor_id FROM cash_checkpoints WHERE tenant_id=$1",
    )
    .bind(s.tenant)
    .bind(ts(70_000))
    .fetch_one(&pool)
    .await?;
    ensure!(current == ("reanchor".into(), true, Some(reanchor)), "{current:?}");
    let triggers: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pg_trigger WHERE tgrelid='public.cash_checkpoints'::regclass AND NOT tgisinternal",
    )
    .fetch_one(&pool)
    .await?;
    ensure!(triggers == 0, "the checkpoint pointer has no trigger (PLAN-r4 §4.3)");
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn cash_discrepancies_refuse_a_second_open_row(pool: PgPool) -> Result<()> {
    install(&pool).await?;
    let s = scope(&pool).await?;
    let other = scope(&pool).await?;
    let missing = Uuid::now_v7();
    insert(
        &pool,
        "cash_discrepancies",
        &discrepancy_row(s, missing, "missing", "-100", 10_000),
    )
    .await?;
    for (direction, delta) in [("extra", "50"), ("missing", "-1")] {
        refused(
            insert(
                &pool,
                "cash_discrepancies",
                &discrepancy_row(s, Uuid::now_v7(), direction, delta, 20_000),
            )
            .await,
            "23505",
            "idx_cash_discrepancies_open",
        )?;
    }
    insert(
        &pool,
        "cash_discrepancies",
        &discrepancy_row(other, Uuid::now_v7(), "missing", "-1", 20_000),
    )
    .await?;
    ensure!(
        exec(&pool, "UPDATE cash_discrepancies SET state='resolved', closed_at=now() WHERE discrepancy_id=$1", missing).await? == 1
    );
    insert(
        &pool,
        "cash_discrepancies",
        &discrepancy_row(s, Uuid::now_v7(), "extra", "50", 20_000),
    )
    .await?;
    let open: i64 =
        sqlx::query_scalar("SELECT count(*) FROM cash_discrepancies WHERE state='open'")
            .fetch_one(&pool)
            .await?;
    ensure!(open == 2, "one open row per scope");
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn cash_discrepancy_min_delta_never_rises(pool: PgPool) -> Result<()> {
    install(&pool).await?;
    let s = scope(&pool).await?;
    let id = Uuid::now_v7();
    insert(&pool, "cash_discrepancies", &discrepancy_row(s, id, "missing", "-100", 10_000)).await?;
    let seen = |delta: &'static str, at: i64| {
        sqlx::query("UPDATE cash_discrepancies SET min_delta=$2::numeric, last_seen_at=$3::timestamptz WHERE discrepancy_id=$1")
            .bind(id)
            .bind(delta)
            .bind(ts(at))
    };
    seen("-120", 20_000).execute(&pool).await?;
    seen("-120", 20_000).execute(&pool).await?;
    refused(seen("-110", 30_000).execute(&pool).await, "23514", "only moves forward")?;
    refused(seen("-120", 15_000).execute(&pool).await, "23514", "only moves forward")?;
    seen("-130", 40_000).execute(&pool).await?;
    let state: (String, bool) = sqlx::query_as(
        "SELECT min_delta::text, last_seen_at=$2::timestamptz FROM cash_discrepancies WHERE discrepancy_id=$1",
    )
    .bind(id)
    .bind(ts(40_000))
    .fetch_one(&pool)
    .await?;
    ensure!(state == ("-130".into(), true), "{state:?}");
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn cash_discrepancy_state_only_moves_forward(pool: PgPool) -> Result<()> {
    install(&pool).await?;
    let s = scope(&pool).await?;
    let missing = Uuid::now_v7();
    insert(
        &pool,
        "cash_discrepancies",
        &discrepancy_row(s, missing, "missing", "-100", 10_000),
    )
    .await?;
    for change in [
        "first_delta=-200",
        "direction='extra'",
        "checkpoint_cut=checkpoint_cut-interval '1 s'",
        "checkpoint_interval_end=checkpoint_interval_end+interval '1 s'",
        "first_interval_end=first_interval_end+interval '1 s'",
        "first_sample_id='00000000-0000-0000-0000-000000000001'::uuid",
        "opened_version=opened_version+1",
        "tenant_id='00000000-0000-0000-0000-000000000001'::uuid",
    ] {
        // Each change comes from this fixed list.
        let sql = format!("UPDATE cash_discrepancies SET {change} WHERE discrepancy_id=$1");
        refused(exec(&pool, &sql, missing).await, "23514", "only moves forward")?;
    }
    // A clean observation never voids missing cash; only late evidence does.
    for reason in ["clean_observation", "superseded_by_missing"] {
        let sql = format!(
            "UPDATE cash_discrepancies SET state='void', void_reason='{reason}', closed_at=now() WHERE discrepancy_id=$1"
        );
        refused(exec(&pool, &sql, missing).await, "23514", "cash_discrepancies_missing_void")?;
    }
    refused(
        exec(
            &pool,
            "UPDATE cash_discrepancies SET state='resolved' WHERE discrepancy_id=$1",
            missing,
        )
        .await,
        "23514",
        "cash_discrepancies_closed",
    )?;
    refused(
        exec(
            &pool,
            "UPDATE cash_discrepancies SET state='void', closed_at=now() WHERE discrepancy_id=$1",
            missing,
        )
        .await,
        "23514",
        "cash_discrepancies_void_pair",
    )?;
    ensure!(
        exec(&pool, "UPDATE cash_discrepancies SET state='void', void_reason='late_evidence', closed_at=now() WHERE discrepancy_id=$1", missing).await? == 1
    );
    // A closed row never changes again and is never deleted.
    for sql in [
        "UPDATE cash_discrepancies SET state='open', void_reason=NULL, closed_at=NULL WHERE discrepancy_id=$1",
        "UPDATE cash_discrepancies SET last_seen_at=last_seen_at+interval '1 s' WHERE discrepancy_id=$1",
        "UPDATE cash_discrepancies SET state='resolved', void_reason=NULL WHERE discrepancy_id=$1",
    ] {
        refused(exec(&pool, sql, missing).await, "23514", "only moves forward")?;
    }
    refused(
        exec(&pool, "DELETE FROM cash_discrepancies WHERE discrepancy_id=$1", missing).await,
        "23514",
        "never deleted",
    )?;
    // Extra cash is voided by a clean observation or superseded by missing cash.
    for reason in [
        "clean_observation",
        "superseded_by_missing",
        "late_evidence",
    ] {
        let extra = Uuid::now_v7();
        insert(&pool, "cash_discrepancies", &discrepancy_row(s, extra, "extra", "50", 20_000))
            .await?;
        let sql = format!(
            "UPDATE cash_discrepancies SET state='void', void_reason='{reason}', closed_at=now() WHERE discrepancy_id=$1"
        );
        ensure!(exec(&pool, &sql, extra).await? == 1, "{reason}");
    }
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn cash_discrepancies_refuse_incoherent_first_sightings(pool: PgPool) -> Result<()> {
    install(&pool).await?;
    let s = scope(&pool).await?;
    let other = scope(&pool).await?;
    let unknown = orphan(&pool).await?;
    let good = discrepancy_row(s, Uuid::now_v7(), "missing", "-100", 10_000);
    let closed_extra = set(&discrepancy_row(s, Uuid::now_v7(), "extra", "50", 10_000), &[
        ("state", json!("void")),
        ("closed_at", json!(ts(20_000))),
    ]);
    for (row, code, needle) in [
        (
            discrepancy_row(s, Uuid::now_v7(), "missing", "5", 10_000),
            "23514",
            "cash_discrepancies_first_delta",
        ),
        (
            discrepancy_row(s, Uuid::now_v7(), "missing", "-0.01", 10_000),
            "23514",
            "cash_discrepancies_first_delta",
        ),
        (
            discrepancy_row(s, Uuid::now_v7(), "extra", "0.01", 10_000),
            "23514",
            "cash_discrepancies_first_delta",
        ),
        (
            discrepancy_row(s, Uuid::now_v7(), "extra", "-50", 10_000),
            "23514",
            "cash_discrepancies_first_delta",
        ),
        (
            discrepancy_row(s, Uuid::now_v7(), "neutral", "-50", 10_000),
            "23514",
            "cash_discrepancies_direction",
        ),
        (
            set(&good, &[
                ("state", json!("pending")),
                ("closed_at", json!(ts(20_000))),
            ]),
            "23514",
            "cash_discrepancies_state",
        ),
        (
            with(&good, "checkpoint_cut", json!(ts(10_001))),
            "23514",
            "cash_discrepancies_checkpoint",
        ),
        (
            with(&good, "checkpoint_interval_end", json!(ts(-1))),
            "23514",
            "cash_discrepancies_checkpoint",
        ),
        (with(&good, "min_delta", json!("-99")), "23514", "cash_discrepancies_min_delta"),
        (with(&good, "min_delta", json!("NaN")), "23514", "cash_discrepancies_min_delta"),
        (
            with(&good, "void_reason", json!("late_evidence")),
            "23514",
            "cash_discrepancies_void_pair",
        ),
        (
            with(&closed_extra, "void_reason", json!("operator")),
            "23514",
            "cash_discrepancies_void_reason",
        ),
        (with(&good, "closed_at", json!(ts(1))), "23514", "cash_discrepancies_closed"),
        (
            with(&good, "opened_version", json!(-1)),
            "23514",
            "cash_discrepancies_opened_version",
        ),
        (
            with(&good, "first_receive_at", json!(ts(10_600))),
            "23514",
            "cash_discrepancies_first_observation",
        ),
        (with(&good, "first_epoch_id", json!(NIL)), "23514", "cash_discrepancies_ids"),
        (
            with(&good, "first_sample_id", json!(other.sample)),
            "23503",
            "cash_discrepancies_first_sample",
        ),
        (
            discrepancy_row(unknown, Uuid::now_v7(), "missing", "-100", 10_000),
            "23503",
            "cash_discrepancies_version_row",
        ),
    ] {
        refused(insert(&pool, "cash_discrepancies", &row).await, code, needle)?;
    }
    insert(&pool, "cash_discrepancies", &good).await?;
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn cash_sightings_are_immutable(pool: PgPool) -> Result<()> {
    install(&pool).await?;
    let s = scope(&pool).await?;
    let other = scope(&pool).await?;
    let id = Uuid::now_v7();
    insert(&pool, "cash_discrepancies", &discrepancy_row(s, id, "missing", "-100", 10_000)).await?;
    insert(&pool, "cash_sightings", &sighting_row(s, id, 1, "-100", 10_000)).await?;
    insert(&pool, "cash_sightings", &sighting_row(s, id, 2, "0", 20_000)).await?;
    refused(
        exec(
            &pool,
            "UPDATE cash_sightings SET delta=-1 WHERE discrepancy_id=$1 AND seq=2",
            id,
        )
        .await,
        "23514",
        IMMUTABLE,
    )?;
    refused(
        exec(&pool, "DELETE FROM cash_sightings WHERE discrepancy_id=$1", id).await,
        "23514",
        IMMUTABLE,
    )?;
    for (row, code, needle) in [
        (sighting_row(s, id, 0, "1", 30_000), "23514", "cash_sightings_seq"),
        (sighting_row(s, id, 2, "1", 30_000), "23505", "cash_sightings_key"),
        (sighting_row(s, id, 3, "Infinity", 30_000), "23514", "cash_sightings_delta"),
        (
            with(&sighting_row(s, id, 3, "1", 30_000), "interval_end", json!(ts(30_349))),
            "23514",
            "cash_sightings_observation",
        ),
        (
            with(&sighting_row(s, id, 3, "1", 30_000), "sample_id", json!(NIL)),
            "23514",
            "cash_sightings_ids",
        ),
        (
            with(&sighting_row(s, id, 3, "1", 30_000), "sample_id", json!(other.sample)),
            "23503",
            "cash_sightings_sample",
        ),
        (sighting_row(other, id, 3, "1", 30_000), "23503", "cash_sightings_discrepancy"),
        (
            sighting_row(s, Uuid::now_v7(), 1, "1", 30_000),
            "23503",
            "cash_sightings_discrepancy",
        ),
    ] {
        refused(insert(&pool, "cash_sightings", &row).await, code, needle)?;
    }
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM cash_sightings WHERE discrepancy_id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await?;
    ensure!(count == 2);
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn cash_reanchors_refuse_unbalanced_flows(pool: PgPool) -> Result<()> {
    install(&pool).await?;
    let s = scope(&pool).await?;
    let (old, new) = anchors(&pool, s, 0).await?;
    let good = reanchor_row(s, Uuid::now_v7(), old, new);
    // C - W must equal delta; every other rule holds for these rows.
    for (contribution, withdrawal, min_delta) in
        [("4", "10", "-10"), ("6", "10", "-10"), ("0", "10", "-10")]
    {
        let mut row = with(&good, "contribution", json!(contribution));
        row["withdrawal"] = json!(withdrawal);
        row["min_delta"] = json!(min_delta);
        refused(
            insert(&pool, "cash_reanchors", &row).await,
            "23514",
            "cash_reanchors_flow_balance",
        )?;
    }
    insert(&pool, "cash_reanchors", &good).await?;
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn cash_reanchors_refuse_incoherent_rows(pool: PgPool) -> Result<()> {
    install(&pool).await?;
    let s = scope(&pool).await?;
    let other = scope(&pool).await?;
    let (old, new) = anchors(&pool, s, 0).await?;
    let (foreign_old, foreign_new) = anchors(&pool, other, 0).await?;
    let discrepancy = Uuid::now_v7();
    insert(
        &pool,
        "cash_discrepancies",
        &discrepancy_row(s, discrepancy, "missing", "-10", 10_000),
    )
    .await?;
    let foreign_discrepancy = Uuid::now_v7();
    insert(
        &pool,
        "cash_discrepancies",
        &discrepancy_row(other, foreign_discrepancy, "missing", "-10", 10_000),
    )
    .await?;
    let good = reanchor_row(s, Uuid::now_v7(), old, new);
    let fresh = || with(&good, "reanchor_id", json!(Uuid::now_v7()));
    let set = |pairs: &[(&str, Value)]| set(&fresh(), pairs);
    for (row, code, needle) in [
        (
            set(&[("withdrawal", json!("9")), ("contribution", json!("4"))]),
            "23514",
            "cash_reanchors_amounts",
        ),
        (
            set(&[
                ("delta", json!("5")),
                ("min_delta", json!("1")),
                ("withdrawal", json!("-1")),
                ("contribution", json!("4")),
            ]),
            "23514",
            "cash_reanchors_amounts",
        ),
        (
            set(&[
                ("min_delta", json!("-4")),
                ("withdrawal", json!("4")),
                ("contribution", json!("-1")),
            ]),
            "23514",
            "cash_reanchors_amounts",
        ),
        (set(&[("delta", json!("NaN"))]), "23514", "cash_reanchors_amounts"),
        (
            set(&[
                ("o1_interval_end", json!(ts(70_000))),
                ("o1_receive_at", json!(ts(60_350))),
            ]),
            "23514",
            "cash_reanchors_observations",
        ),
        (set(&[("o2_total", json!("995.02"))]), "23514", "cash_reanchors_observations"),
        (set(&[("o1_send_at", json!(ts(60_400)))]), "23514", "cash_reanchors_o1"),
        (set(&[("o2_total", json!("-1"))]), "23514", "cash_reanchors_o2"),
        (
            set(&[
                ("dating_cut", json!(ts(70_000))),
                ("dating_interval_start", json!(ts(70_000))),
                ("dating_interval_end", json!(ts(70_500))),
            ]),
            "23514",
            "cash_reanchors_dating",
        ),
        (
            set(&[("dating_interval_start", json!(ts(1)))]),
            "23514",
            "cash_reanchors_dating",
        ),
        (set(&[("reason", json!("  "))]), "23514", "cash_reanchors_identity"),
        (set(&[("operator_subject", json!(""))]), "23514", "cash_reanchors_identity"),
        (set(&[("new_anchor_id", json!(old))]), "23514", "cash_reanchors_anchors"),
        (
            set(&[("checked_version", json!(-1))]),
            "23514",
            "cash_reanchors_checked_version",
        ),
        (set(&[("mode", json!("manual"))]), "23514", "cash_reanchors_mode"),
        (set(&[("mode", json!("recovery"))]), "23514", "cash_reanchors_mode_shape"),
        (set(&[("dating_cut", Value::Null)]), "23514", "cash_reanchors_mode_shape"),
        (
            set(&[
                ("resolved_discrepancy_id", json!(discrepancy)),
                ("voided_discrepancy_id", json!(discrepancy)),
            ]),
            "23514",
            "cash_reanchors_discrepancy",
        ),
        (
            set(&[("excluded_sighting_seqs", json!([1]))]),
            "23514",
            "cash_reanchors_excluded",
        ),
        (
            set(&[
                ("resolved_discrepancy_id", json!(discrepancy)),
                ("excluded_sighting_seqs", json!([0])),
            ]),
            "23514",
            "cash_reanchors_excluded",
        ),
        (
            set(&[
                ("resolved_discrepancy_id", json!(discrepancy)),
                ("excluded_sighting_seqs", json!([2, null])),
            ]),
            "23514",
            "cash_reanchors_excluded",
        ),
        (set(&[("o2_epoch_id", json!(NIL))]), "23514", "cash_reanchors_ids"),
        (
            set(&[("old_anchor_id", json!(foreign_old))]),
            "23503",
            "cash_reanchors_old_anchor",
        ),
        (
            set(&[("new_anchor_id", json!(Uuid::now_v7()))]),
            "23503",
            "cash_reanchors_new_anchor",
        ),
        (
            set(&[("resolved_discrepancy_id", json!(Uuid::now_v7()))]),
            "23503",
            "cash_reanchors_resolved",
        ),
        (
            set(&[("o1_sample_id", json!(other.sample))]),
            "23503",
            "cash_reanchors_o1_sample",
        ),
        // Scoped links: another venue scope's sample, anchor or discrepancy is
        // refused even though the row exists.
        (
            set(&[("o2_sample_id", json!(other.sample))]),
            "23503",
            "cash_reanchors_o2_sample",
        ),
        (
            set(&[("new_anchor_id", json!(foreign_new))]),
            "23503",
            "cash_reanchors_new_anchor",
        ),
        (
            set(&[("resolved_discrepancy_id", json!(foreign_discrepancy))]),
            "23503",
            "cash_reanchors_resolved",
        ),
        (
            set(&[("voided_discrepancy_id", json!(foreign_discrepancy))]),
            "23503",
            "cash_reanchors_voided",
        ),
        (
            set(&[("voided_discrepancy_id", json!(Uuid::now_v7()))]),
            "23503",
            "cash_reanchors_voided",
        ),
    ] {
        refused(insert(&pool, "cash_reanchors", &row).await, code, needle)?;
    }
    let first = set(&[
        ("resolved_discrepancy_id", json!(discrepancy)),
        ("excluded_sighting_seqs", json!([2, 3])),
    ]);
    insert(&pool, "cash_reanchors", &first).await?;
    let id: Uuid = serde_json::from_value(first["reanchor_id"].clone())?;
    let another_new = Uuid::now_v7();
    insert(
        &pool,
        "cash_anchors",
        &anchor_row(s, another_new, "reanchor", None, Some(90_000)),
    )
    .await?;
    refused(
        insert(&pool, "cash_reanchors", &fresh()).await,
        "23505",
        "cash_reanchors_new_anchor_once",
    )?;
    refused(
        insert(
            &pool,
            "cash_reanchors",
            &set(&[
                ("new_anchor_id", json!(another_new)),
                ("resolved_discrepancy_id", json!(discrepancy)),
            ]),
        )
        .await,
        "23505",
        "cash_reanchors_resolves_once",
    )?;
    refused(
        exec(&pool, "UPDATE cash_reanchors SET reason='edited' WHERE reanchor_id=$1", id).await,
        "23514",
        IMMUTABLE,
    )?;
    refused(
        exec(&pool, "DELETE FROM cash_reanchors WHERE reanchor_id=$1", id).await,
        "23514",
        IMMUTABLE,
    )?;
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn recovery_reanchor_admits_no_attested_flow(pool: PgPool) -> Result<()> {
    install(&pool).await?;
    let s = scope(&pool).await?;
    let (old, new) = anchors(&pool, s, 0).await?;
    let recovery = Uuid::now_v7();
    insert(&pool, "cash_reanchors", &recovery_row(s, recovery, old, new)).await?;
    let date = "2026-11-10 12:00:00+00";
    // Neither direction, claimed as attested or as recovery.
    for direction in ["withdrawal", "contribution"] {
        let row = flow_row(s, recovery, direction, "10", date, "2026-11-01");
        refused(
            insert(&pool, "cash_attested_flows", &row).await,
            "23503",
            "cash_attested_flows_reanchor",
        )?;
        let claimed = with(&row, "reanchor_mode", json!("recovery"));
        refused(
            insert(&pool, "cash_attested_flows", &claimed).await,
            "23514",
            "cash_attested_flows_reanchor_mode",
        )?;
    }
    // A recovery row with a delta, a dating copy or any amount is refused.
    for key in [
        "delta",
        "min_delta",
        "withdrawal",
        "contribution",
        "dating_cut",
    ] {
        let mut row = recovery_row(s, Uuid::now_v7(), old, new);
        row[key] = reanchor_row(s, Uuid::now_v7(), old, new)[key].clone();
        refused(
            insert(&pool, "cash_reanchors", &row).await,
            "23514",
            "cash_reanchors_mode_shape",
        )?;
    }
    let flows: i64 = sqlx::query_scalar("SELECT count(*) FROM cash_attested_flows")
        .fetch_one(&pool)
        .await?;
    ensure!(flows == 0);
    // The attested re-anchor of the same anchors keeps its flows.
    let (old2, new2) = anchors(&pool, s, 200_000).await?;
    let attested = Uuid::now_v7();
    insert(&pool, "cash_reanchors", &reanchor_row(s, attested, old2, new2)).await?;
    insert(
        &pool,
        "cash_attested_flows",
        &flow_row(s, attested, "withdrawal", "10", date, "2026-11-01"),
    )
    .await?;
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn cash_attested_flows_follow_their_rules(pool: PgPool) -> Result<()> {
    install(&pool).await?;
    let s = scope(&pool).await?;
    let other = scope(&pool).await?;
    let (old, new) = anchors(&pool, s, 0).await?;
    let reanchor = Uuid::now_v7();
    insert(&pool, "cash_reanchors", &reanchor_row(s, reanchor, old, new)).await?;
    // The month is the UTC month of the economic date, whatever the session
    // zone: in Kiritimati (UTC+14) this date is already December 1.
    let mut zoned = pool.acquire().await?;
    sqlx::query("SET TIME ZONE 'Pacific/Kiritimati'").execute(&mut *zoned).await?;
    let late = "2026-11-30 23:59:59.999999+00";
    let withdrawal = flow_row(s, reanchor, "withdrawal", "10", late, "2026-11-01");
    for (row, code, needle) in [
        (
            with(&withdrawal, "attributed_month", json!("2026-12-01")),
            "23514",
            "cash_attested_flows_month",
        ),
        (
            with(&withdrawal, "date_rule", json!("o1_receipt_plus_u")),
            "23514",
            "cash_attested_flows_date_rule",
        ),
        (
            with(&withdrawal, "date_rule", json!("operator")),
            "23514",
            "cash_attested_flows_date_rule",
        ),
        (
            set(&withdrawal, &[
                ("direction", json!("deposit")),
                ("date_rule", json!("o1_receipt_plus_u")),
            ]),
            "23514",
            "cash_attested_flows_direction",
        ),
        (with(&withdrawal, "amount", json!("0")), "23514", "cash_attested_flows_amount"),
        (with(&withdrawal, "flow_id", json!(NIL)), "23514", "cash_attested_flows_ids"),
        (
            with(&withdrawal, "tenant_id", json!(other.tenant)),
            "23503",
            "cash_attested_flows_reanchor",
        ),
    ] {
        refused(insert(&mut *zoned, "cash_attested_flows", &row).await, code, needle)?;
    }
    insert(&mut *zoned, "cash_attested_flows", &withdrawal).await?;
    let contribution =
        flow_row(s, reanchor, "contribution", "5", "2026-12-01 00:00:00+00", "2026-12-01");
    refused(
        insert(
            &pool,
            "cash_attested_flows",
            &with(&contribution, "date_rule", json!("checkpoint_cut")),
        )
        .await,
        "23514",
        "cash_attested_flows_date_rule",
    )?;
    insert(&pool, "cash_attested_flows", &contribution).await?;
    refused(
        insert(
            &pool,
            "cash_attested_flows",
            &flow_row(s, reanchor, "withdrawal", "1", late, "2026-11-01"),
        )
        .await,
        "23505",
        "cash_attested_flows_once",
    )?;
    refused(
        exec(
            &pool,
            "UPDATE cash_attested_flows SET amount=amount+1 WHERE reanchor_id=$1",
            reanchor,
        )
        .await,
        "23514",
        IMMUTABLE,
    )?;
    refused(
        exec(&pool, "DELETE FROM cash_attested_flows WHERE reanchor_id=$1", reanchor).await,
        "23514",
        IMMUTABLE,
    )?;
    Ok(())
}

/// One admitted leg, as migration 40's constraint fixture admits it.
async fn admit(pool: &PgPool, tenant: Uuid) -> Result<Uuid> {
    let position = Uuid::now_v7();
    let entry = Uuid::now_v7();
    let mut tx = pool.begin().await?;
    sqlx::query("UPDATE accounting_state SET version=version+1 WHERE tenant_id=$1 AND venue_id=$2")
        .bind(tenant)
        .bind(VENUE)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO leg_routes(position_id,tenant_id,venue_id,account_scope,routed_at) VALUES($1,$2,$3,'s84-local',now())")
        .bind(position).bind(tenant).bind(VENUE).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO order_intents
        (intent_id,tenant_id,venue_id,account_scope,client_order_id,position_id,kind,params,created_at)
        VALUES($1,$2,$3,'s84-local',$1::uuid::text,$4,'entry','{}',now())")
        .bind(entry).bind(tenant).bind(VENUE).bind(position).execute(&mut *tx).await?;
    sqlx::query("SELECT balance_leg_admit($1,$4,'s84-local','BTCUSDT',$2,$3,'2026-11-01',
        10,10,200,5,0.01,'base','fixture-chart',now(),'fixture-hash',1,1.01,100,0.1,12,90,now(),false)")
        .bind(tenant).bind(position).bind(entry).bind(VENUE).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(position)
}

fn dust_row(s: Scope, position: Uuid, dust: &str, write_off: &str, pooled: &str) -> Value {
    json!({
        "position_id": position, "tenant_id": s.tenant, "venue_id": VENUE, "account_scope": "s84-local",
        "symbol": "BTCUSDT", "entry_dust": dust, "write_off": write_off, "pooled": pooled,
        "close_at": ts(90_000), "close_clock_sample_id": s.sample, "close_clock_wall_delta_ms": 0,
        "close_clock_mono_delta_ms": 0, "close_clock_verdict": "valid", "last_fill_at": ts(89_000),
    })
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn dust_ledger_pools_entry_dust_and_write_off(pool: PgPool) -> Result<()> {
    install(&pool).await?;
    let s = scope(&pool).await?;
    let other = scope(&pool).await?;
    let dust_only = admit(&pool, s.tenant).await?;
    let written_off = admit(&pool, s.tenant).await?;
    let covered = |row: Value| {
        let mut row = row;
        row["loss_within_charge"] = json!(true);
        row["unsellable_in_envelope"] = json!(true);
        row["min_notional"] = json!("5");
        row["write_off_price"] = json!("90000");
        row
    };
    let good = covered(dust_row(s, written_off, "0.00001", "0.00002", "0.00003"));
    for (row, code, needle) in [
        (dust_row(s, dust_only, "0.00001", "0", "0.00002"), "23514", "dust_ledger_pool"),
        (
            dust_row(s, dust_only, "-0.00001", "0", "-0.00001"),
            "23514",
            "dust_ledger_amounts",
        ),
        (
            dust_row(s, written_off, "0.00001", "0.00002", "0.00003"),
            "23514",
            "dust_ledger_covered_write_off",
        ),
        (
            with(&good, "loss_within_charge", json!(false)),
            "23514",
            "dust_ledger_covered_write_off",
        ),
        (
            with(&good, "unsellable_in_envelope", Value::Null),
            "23514",
            "dust_ledger_covered_write_off",
        ),
        (
            with(&good, "write_off_price", Value::Null),
            "23514",
            "dust_ledger_covered_write_off",
        ),
        (with(&good, "min_notional", json!("0")), "23514", "dust_ledger_evidence_amounts"),
        (
            with(&good, "close_clock_sample_id", Value::Null),
            "23514",
            "dust_ledger_close_evidence_whole",
        ),
        (
            with(&good, "close_clock_verdict", json!("stale")),
            "23514",
            "dust_ledger_close_verdict",
        ),
        (with(&good, "symbol", json!("ETHUSDT")), "23503", "dust_ledger_leg"),
        (with(&good, "position_id", json!(Uuid::now_v7())), "23503", "dust_ledger_leg"),
        // Scoped links: another venue scope's sample, or this leg claimed by
        // another scope (with that scope's own sample), is refused.
        (
            with(&good, "close_clock_sample_id", json!(other.sample)),
            "23503",
            "dust_ledger_close_sample",
        ),
        (
            set(&good, &[
                ("tenant_id", json!(other.tenant)),
                ("close_clock_sample_id", json!(other.sample)),
            ]),
            "23503",
            "dust_ledger_leg",
        ),
    ] {
        refused(insert(&pool, "dust_ledger", &row).await, code, needle)?;
    }
    insert(&pool, "dust_ledger", &good).await?;
    // No write-off: no covered evidence is needed, and invalid clock evidence
    // is recorded as invalid.
    let mut plain = dust_row(s, dust_only, "0.00001", "0", "0.00001");
    plain["close_clock_verdict"] = json!("invalid");
    plain["close_clock_wall_delta_ms"] = Value::Null;
    insert(&pool, "dust_ledger", &plain).await?;
    refused(insert(&pool, "dust_ledger", &plain).await, "23505", "dust_ledger_key")?;
    let pool_total: String =
        sqlx::query_scalar("SELECT sum(pooled)::text FROM dust_ledger WHERE tenant_id=$1")
            .bind(s.tenant)
            .fetch_one(&pool)
            .await?;
    ensure!(pool_total == "0.00004", "W_pool = sum of D + R: {pool_total}");
    refused(
        exec(
            &pool,
            "UPDATE dust_ledger SET write_off=0, pooled=entry_dust WHERE position_id=$1",
            written_off,
        )
        .await,
        "23514",
        IMMUTABLE,
    )?;
    refused(
        exec(&pool, "DELETE FROM dust_ledger WHERE position_id=$1", dust_only).await,
        "23514",
        IMMUTABLE,
    )?;
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn accounting_invalidations_are_idempotent_and_immutable(pool: PgPool) -> Result<()> {
    install(&pool).await?;
    let s = scope(&pool).await?;
    // A trade at the last microsecond of November UTC is November's in any zone.
    let mut zoned = pool.acquire().await?;
    sqlx::query("SET TIME ZONE 'Pacific/Kiritimati'").execute(&mut *zoned).await?;
    let row = json!({
        "invalidation_id": Uuid::now_v7(), "tenant_id": s.tenant, "venue_id": VENUE, "month": "2026-11-01",
        "reason": "unattributed_fill", "evidence_key": "s84-local/BTCUSDT/4242", "account_scope": "s84-local",
        "symbol": "BTCUSDT", "exchange_order_id": "777", "trade_id": "4242",
        "trade_time": "2026-11-30 23:59:59.999999+00",
    });
    for (bad, code, needle) in [
        (
            with(&row, "month", json!("2026-12-01")),
            "23514",
            "accounting_invalidations_fill",
        ),
        (with(&row, "trade_id", Value::Null), "23514", "accounting_invalidations_fill"),
        (
            with(&row, "reason", json!("late_evidence")),
            "23514",
            "accounting_invalidations_reason",
        ),
        (
            with(&row, "evidence_key", json!(" ")),
            "23514",
            "accounting_invalidations_evidence_key",
        ),
        (
            with(&row, "invalidation_id", json!(NIL)),
            "23514",
            "accounting_invalidations_ids",
        ),
        (
            with(&row, "tenant_id", json!(Uuid::now_v7())),
            "23503",
            "accounting_invalidations_version_row",
        ),
    ] {
        refused(insert(&mut *zoned, "accounting_invalidations", &bad).await, code, needle)?;
    }
    insert(&mut *zoned, "accounting_invalidations", &row).await?;
    let again = with(&row, "invalidation_id", json!(Uuid::now_v7()));
    refused(
        insert(&pool, "accounting_invalidations", &again).await,
        "23505",
        "accounting_invalidations_once",
    )?;
    let idempotent = sqlx::query(
        "INSERT INTO accounting_invalidations SELECT * FROM jsonb_populate_record(NULL::accounting_invalidations, $1)
         ON CONFLICT (tenant_id, venue_id, month, reason, evidence_key) DO NOTHING",
    )
    .bind(&again)
    .execute(&pool)
    .await?;
    ensure!(idempotent.rows_affected() == 0, "a repeated booking adds nothing");
    let id: Uuid = serde_json::from_value(row["invalidation_id"].clone())?;
    refused(
        exec(
            &pool,
            "UPDATE accounting_invalidations SET month='2026-10-01' WHERE invalidation_id=$1",
            id,
        )
        .await,
        "23514",
        IMMUTABLE,
    )?;
    refused(
        exec(&pool, "DELETE FROM accounting_invalidations WHERE invalidation_id=$1", id).await,
        "23514",
        IMMUTABLE,
    )?;
    Ok(())
}

const TABLES: [&str; 8] = [
    "cash_anchors",
    "cash_checkpoints",
    "cash_discrepancies",
    "cash_sightings",
    "cash_reanchors",
    "cash_attested_flows",
    "dust_ledger",
    "accounting_invalidations",
];

/// (name, type, NOT NULL) of each column, in order.
fn columns(table: &str) -> Value {
    const T: &str = "text";
    const TZ: &str = "timestamp with time zone";
    const B: &str = "bigint";
    const N: &str = "numeric";
    const U: &str = "uuid";
    fn cols(list: &mut Vec<Value>, columns: &[(&str, &str, bool)]) {
        list.extend(columns.iter().map(|(name, kind, required)| json!([name, kind, required])));
    }
    fn observation(list: &mut Vec<Value>, prefix: &str, required: bool) {
        for (name, kind) in [
            ("send_at", TZ),
            ("receive_at", TZ),
            ("u_ns", B),
            ("sample_id", U),
            ("epoch_id", U),
            ("interval_start", TZ),
            ("interval_end", TZ),
        ] {
            list.push(json!([format!("{prefix}{name}"), kind, required]));
        }
    }
    let mut list = Vec::new();
    match table {
        "cash_anchors" => {
            cols(&mut list, &[
                ("anchor_id", U, true),
                ("tenant_id", U, true),
                ("venue_id", T, true),
                ("kind", T, true),
                ("cutover_id", U, false),
                ("quote", N, true),
                ("as_of", TZ, false),
            ]);
            observation(&mut list, "", false);
            cols(&mut list, &[("depends_start", TZ, false), ("depends_end", TZ, false)]);
        },
        "cash_checkpoints" => {
            cols(&mut list, &[
                ("tenant_id", U, true),
                ("venue_id", T, true),
                ("cut", TZ, true),
            ]);
            observation(&mut list, "", true);
            cols(&mut list, &[("source", T, true), ("anchor_id", U, false)]);
        },
        "cash_discrepancies" => {
            cols(&mut list, &[
                ("discrepancy_id", U, true),
                ("tenant_id", U, true),
                ("venue_id", T, true),
                ("direction", T, true),
                ("state", T, true),
            ]);
            observation(&mut list, "first_", true);
            cols(&mut list, &[
                ("first_delta", N, true),
                ("checkpoint_cut", TZ, true),
                ("checkpoint_interval_start", TZ, true),
                ("checkpoint_interval_end", TZ, true),
                ("opened_version", B, true),
                ("last_seen_at", TZ, true),
                ("min_delta", N, true),
                ("void_reason", T, false),
                ("closed_at", TZ, false),
            ]);
        },
        "cash_sightings" => {
            cols(&mut list, &[
                ("discrepancy_id", U, true),
                ("seq", "integer", true),
                ("tenant_id", U, true),
                ("venue_id", T, true),
            ]);
            observation(&mut list, "", true);
            cols(&mut list, &[("delta", N, true)]);
        },
        "cash_reanchors" => {
            cols(&mut list, &[
                ("reanchor_id", U, true),
                ("tenant_id", U, true),
                ("venue_id", T, true),
                ("operator_subject", T, true),
                ("reason", T, true),
            ]);
            observation(&mut list, "o1_", true);
            cols(&mut list, &[("o1_total", N, true)]);
            observation(&mut list, "o2_", true);
            cols(&mut list, &[
                ("o2_total", N, true),
                ("mode", T, true),
                ("checked_version", B, true),
                ("old_anchor_id", U, true),
                ("new_anchor_id", U, true),
                ("dating_cut", TZ, false),
                ("dating_interval_start", TZ, false),
                ("dating_interval_end", TZ, false),
                ("delta", N, false),
                ("min_delta", N, false),
                ("withdrawal", N, false),
                ("contribution", N, false),
                ("resolved_discrepancy_id", U, false),
                ("excluded_sighting_seqs", "integer[]", true),
                ("voided_discrepancy_id", U, false),
            ]);
        },
        "cash_attested_flows" => cols(&mut list, &[
            ("tenant_id", U, true),
            ("venue_id", T, true),
            ("economic_date", TZ, true),
            ("flow_id", U, true),
            ("reanchor_id", U, true),
            ("direction", T, true),
            ("amount", N, true),
            ("date_rule", T, true),
            ("attributed_month", "date", true),
            ("reanchor_mode", T, true),
        ]),
        "dust_ledger" => cols(&mut list, &[
            ("position_id", U, true),
            ("tenant_id", U, true),
            ("venue_id", T, true),
            ("account_scope", T, true),
            ("symbol", T, true),
            ("entry_dust", N, true),
            ("write_off", N, true),
            ("pooled", N, true),
            ("close_at", TZ, true),
            ("close_clock_sample_id", U, false),
            ("close_clock_wall_delta_ms", B, false),
            ("close_clock_mono_delta_ms", B, false),
            ("close_clock_verdict", T, false),
            ("last_fill_at", TZ, true),
            ("loss_within_charge", "boolean", false),
            ("unsellable_in_envelope", "boolean", false),
            ("min_notional", N, false),
            ("write_off_price", N, false),
        ]),
        "accounting_invalidations" => cols(&mut list, &[
            ("invalidation_id", U, true),
            ("tenant_id", U, true),
            ("venue_id", T, true),
            ("month", "date", true),
            ("reason", T, true),
            ("evidence_key", T, true),
            ("account_scope", T, false),
            ("symbol", T, false),
            ("exchange_order_id", T, false),
            ("trade_id", T, false),
            ("trade_time", TZ, false),
        ]),
        other => panic!("unexpected table {other}"),
    }
    Value::Array(list)
}

fn constraint_names(table: &str) -> Vec<String> {
    let names: &[&str] = match table {
        "cash_anchors" => &[
            "key p",
            "scope u",
            "checkpoint_key u",
            "as_of_once u",
            "cutover_once u",
            "version_row f",
            "sample f",
            "ids c",
            "kind c",
            "cutover c",
            "quote c",
            "pending c",
            "observation_whole c",
            "observation c",
            "hull c",
        ],
        "cash_checkpoints" => &[
            "key p",
            "version_row f",
            "sample f",
            "anchor f",
            "ids c",
            "source c",
            "anchor_pair c",
            "observation c",
        ],
        "cash_discrepancies" => &[
            "key p",
            "scope u",
            "version_row f",
            "first_sample f",
            "ids c",
            "direction c",
            "state c",
            "first_observation c",
            "first_delta c",
            "checkpoint c",
            "opened_version c",
            "min_delta c",
            "void_reason c",
            "void_pair c",
            "closed c",
            "missing_void c",
        ],
        "cash_sightings" => &[
            "key p",
            "discrepancy f",
            "version_row f",
            "sample f",
            "ids c",
            "seq c",
            "delta c",
            "observation c",
        ],
        "cash_reanchors" => &[
            "key p",
            "flow_key u",
            "new_anchor_once u",
            "resolves_once u",
            "version_row f",
            "o1_sample f",
            "o2_sample f",
            "old_anchor f",
            "new_anchor f",
            "resolved f",
            "voided f",
            "ids c",
            "identity c",
            "o1 c",
            "o2 c",
            "observations c",
            "mode c",
            "mode_shape c",
            "checked_version c",
            "anchors c",
            "dating c",
            "amounts c",
            "flow_balance c",
            "discrepancy c",
            "excluded c",
        ],
        "cash_attested_flows" => &[
            "key p",
            "once u",
            "version_row f",
            "reanchor f",
            "reanchor_mode c",
            "ids c",
            "direction c",
            "amount c",
            "date_rule c",
            "month c",
        ],
        "dust_ledger" => &[
            "key p",
            "leg f",
            "version_row f",
            "close_sample f",
            "amounts c",
            "pool c",
            "close_verdict c",
            "close_evidence_whole c",
            "evidence_amounts c",
            "covered_write_off c",
        ],
        "accounting_invalidations" => &[
            "key p",
            "once u",
            "version_row f",
            "ids c",
            "month c",
            "reason c",
            "evidence_key c",
            "fill c",
        ],
        other => panic!("unexpected table {other}"),
    };
    let mut names: Vec<String> = names.iter().map(|name| format!("{table}_{name}")).collect();
    names.sort();
    names
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn migrations46_to_50_install_the_reviewed_catalog(pool: PgPool) -> Result<()> {
    install(&pool).await?;
    for table in TABLES {
        let actual: Value = sqlx::query_scalar(
            "SELECT jsonb_agg(jsonb_build_array(attname,format_type(atttypid,atttypmod),attnotnull) ORDER BY attnum)
             FROM pg_attribute WHERE attrelid=to_regclass('public.'||$1) AND attnum>0 AND NOT attisdropped",
        )
        .bind(table)
        .fetch_one(&pool)
        .await?;
        ensure!(actual == columns(table), "{table} columns: {actual}");
        let constraints: Vec<String> = sqlx::query_scalar(
            "SELECT conname||' '||contype::text FROM pg_constraint WHERE conrelid=to_regclass('public.'||$1) ORDER BY 1",
        )
        .bind(table)
        .fetch_all(&pool)
        .await?;
        ensure!(constraints == constraint_names(table), "{table} constraints: {constraints:?}");
        let validated: bool = sqlx::query_scalar(
            "SELECT bool_and(convalidated AND NOT condeferrable) FROM pg_constraint WHERE conrelid=to_regclass('public.'||$1)",
        )
        .bind(table)
        .fetch_one(&pool)
        .await?;
        ensure!(validated, "{table}: every constraint validated and immediate");
        // Every amount is decodable: each NUMERIC column sits in a
        // ledger_amount_fits CHECK of its table.
        let unguarded: Vec<String> = sqlx::query_scalar(
            "SELECT a.attname::text FROM pg_attribute a WHERE a.attrelid=to_regclass('public.'||$1) AND a.attnum>0
               AND a.atttypid='numeric'::regtype AND NOT EXISTS (SELECT 1 FROM pg_constraint c
                 WHERE c.conrelid=a.attrelid AND c.contype='c'
                   AND pg_get_constraintdef(c.oid) LIKE '%ledger_amount_fits('||a.attname||')%')",
        )
        .bind(table)
        .fetch_all(&pool)
        .await?;
        ensure!(
            unguarded.is_empty(),
            "{table} amounts without ledger_amount_fits: {unguarded:?}"
        );
        let rows: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM public.{table}"))
            .fetch_one(&pool)
            .await?;
        ensure!(rows == 0, "{table} is created empty");
    }
    // Every key and foreign key, with its exact column mapping.
    const KEYS: &[(&str, &str)] = &[
        ("accounting_invalidations_key", "PRIMARY KEY (invalidation_id)"),
        ("accounting_invalidations_once", "UNIQUE (tenant_id, venue_id, month, reason, evidence_key)"),
        ("accounting_invalidations_version_row", "FOREIGN KEY (tenant_id, venue_id) REFERENCES accounting_state(tenant_id, venue_id)"),
        ("cash_anchors_as_of_once", "UNIQUE (tenant_id, venue_id, as_of)"),
        ("cash_anchors_checkpoint_key", "UNIQUE (anchor_id, tenant_id, venue_id, kind, as_of)"),
        ("cash_anchors_cutover_once", "UNIQUE (tenant_id, venue_id, cutover_id)"),
        ("cash_anchors_key", "PRIMARY KEY (anchor_id)"),
        ("cash_anchors_sample", "FOREIGN KEY (sample_id, tenant_id, venue_id) REFERENCES clock_samples(sample_id, tenant_id, venue_id)"),
        ("cash_anchors_scope", "UNIQUE (anchor_id, tenant_id, venue_id)"),
        ("cash_anchors_version_row", "FOREIGN KEY (tenant_id, venue_id) REFERENCES accounting_state(tenant_id, venue_id)"),
        ("cash_attested_flows_key", "PRIMARY KEY (tenant_id, venue_id, economic_date, flow_id)"),
        ("cash_attested_flows_once", "UNIQUE (reanchor_id, direction)"),
        ("cash_attested_flows_reanchor", "FOREIGN KEY (reanchor_id, tenant_id, venue_id, reanchor_mode) REFERENCES cash_reanchors(reanchor_id, tenant_id, venue_id, mode)"),
        ("cash_attested_flows_version_row", "FOREIGN KEY (tenant_id, venue_id) REFERENCES accounting_state(tenant_id, venue_id)"),
        ("cash_checkpoints_anchor", "FOREIGN KEY (anchor_id, tenant_id, venue_id, source, cut) REFERENCES cash_anchors(anchor_id, tenant_id, venue_id, kind, as_of)"),
        ("cash_checkpoints_key", "PRIMARY KEY (tenant_id, venue_id)"),
        ("cash_checkpoints_sample", "FOREIGN KEY (sample_id, tenant_id, venue_id) REFERENCES clock_samples(sample_id, tenant_id, venue_id)"),
        ("cash_checkpoints_version_row", "FOREIGN KEY (tenant_id, venue_id) REFERENCES accounting_state(tenant_id, venue_id)"),
        ("cash_discrepancies_first_sample", "FOREIGN KEY (first_sample_id, tenant_id, venue_id) REFERENCES clock_samples(sample_id, tenant_id, venue_id)"),
        ("cash_discrepancies_key", "PRIMARY KEY (discrepancy_id)"),
        ("cash_discrepancies_scope", "UNIQUE (discrepancy_id, tenant_id, venue_id)"),
        ("cash_discrepancies_version_row", "FOREIGN KEY (tenant_id, venue_id) REFERENCES accounting_state(tenant_id, venue_id)"),
        ("cash_reanchors_flow_key", "UNIQUE (reanchor_id, tenant_id, venue_id, mode)"),
        ("cash_reanchors_key", "PRIMARY KEY (reanchor_id)"),
        ("cash_reanchors_new_anchor", "FOREIGN KEY (new_anchor_id, tenant_id, venue_id) REFERENCES cash_anchors(anchor_id, tenant_id, venue_id)"),
        ("cash_reanchors_new_anchor_once", "UNIQUE (new_anchor_id)"),
        ("cash_reanchors_o1_sample", "FOREIGN KEY (o1_sample_id, tenant_id, venue_id) REFERENCES clock_samples(sample_id, tenant_id, venue_id)"),
        ("cash_reanchors_o2_sample", "FOREIGN KEY (o2_sample_id, tenant_id, venue_id) REFERENCES clock_samples(sample_id, tenant_id, venue_id)"),
        ("cash_reanchors_old_anchor", "FOREIGN KEY (old_anchor_id, tenant_id, venue_id) REFERENCES cash_anchors(anchor_id, tenant_id, venue_id)"),
        ("cash_reanchors_resolved", "FOREIGN KEY (resolved_discrepancy_id, tenant_id, venue_id) REFERENCES cash_discrepancies(discrepancy_id, tenant_id, venue_id)"),
        ("cash_reanchors_resolves_once", "UNIQUE (resolved_discrepancy_id)"),
        ("cash_reanchors_version_row", "FOREIGN KEY (tenant_id, venue_id) REFERENCES accounting_state(tenant_id, venue_id)"),
        ("cash_reanchors_voided", "FOREIGN KEY (voided_discrepancy_id, tenant_id, venue_id) REFERENCES cash_discrepancies(discrepancy_id, tenant_id, venue_id)"),
        ("cash_sightings_discrepancy", "FOREIGN KEY (discrepancy_id, tenant_id, venue_id) REFERENCES cash_discrepancies(discrepancy_id, tenant_id, venue_id)"),
        ("cash_sightings_key", "PRIMARY KEY (discrepancy_id, seq)"),
        ("cash_sightings_sample", "FOREIGN KEY (sample_id, tenant_id, venue_id) REFERENCES clock_samples(sample_id, tenant_id, venue_id)"),
        ("cash_sightings_version_row", "FOREIGN KEY (tenant_id, venue_id) REFERENCES accounting_state(tenant_id, venue_id)"),
        ("dust_ledger_close_sample", "FOREIGN KEY (close_clock_sample_id, tenant_id, venue_id) REFERENCES clock_samples(sample_id, tenant_id, venue_id)"),
        ("dust_ledger_key", "PRIMARY KEY (position_id)"),
        ("dust_ledger_leg", "FOREIGN KEY (position_id, tenant_id, venue_id, account_scope, symbol) REFERENCES balance_legs(position_id, tenant_id, venue_id, account_scope, symbol)"),
        ("dust_ledger_version_row", "FOREIGN KEY (tenant_id, venue_id) REFERENCES accounting_state(tenant_id, venue_id)"),
    ];
    let keys: Vec<(String, String)> = sqlx::query_as(
        "SELECT conname::text,pg_get_constraintdef(oid) FROM pg_constraint
         WHERE contype IN ('p','u','f')
           AND conrelid=ANY(ARRAY(SELECT to_regclass('public.'||t)::oid FROM unnest($1::text[]) t))
         ORDER BY conname",
    )
    .bind(&TABLES[..])
    .fetch_all(&pool)
    .await?;
    let expected: Vec<(String, String)> =
        KEYS.iter().map(|(name, def)| (name.to_string(), def.to_string())).collect();
    ensure!(keys == expected, "keys and foreign keys: {keys:#?}");
    let triggers: Value = sqlx::query_scalar(
        "SELECT jsonb_agg(jsonb_build_array(tgrelid::regclass::text,tgname,tgfoid::regprocedure::text,tgtype,tgenabled::text)
                 ORDER BY tgrelid::regclass::text)
         FROM pg_trigger WHERE NOT tgisinternal AND tgrelid=ANY(ARRAY(SELECT to_regclass('public.'||t)::oid FROM unnest($1::text[]) t))",
    )
    .bind(&TABLES[..])
    .fetch_one(&pool)
    .await?;
    // 27 = ROW | BEFORE | DELETE | UPDATE.
    ensure!(
        triggers
            == json!([
                [
                    "accounting_invalidations",
                    "accounting_invalidations_are_immutable",
                    "ledger_row_is_immutable()",
                    27,
                    "O"
                ],
                [
                    "cash_anchors",
                    "cash_anchors_move_forward",
                    "cash_anchor_moves_forward()",
                    27,
                    "O"
                ],
                [
                    "cash_attested_flows",
                    "cash_attested_flows_are_immutable",
                    "ledger_row_is_immutable()",
                    27,
                    "O"
                ],
                [
                    "cash_discrepancies",
                    "cash_discrepancies_move_forward",
                    "cash_discrepancy_moves_forward()",
                    27,
                    "O"
                ],
                [
                    "cash_reanchors",
                    "cash_reanchors_are_immutable",
                    "ledger_row_is_immutable()",
                    27,
                    "O"
                ],
                [
                    "cash_sightings",
                    "cash_sightings_are_immutable",
                    "ledger_row_is_immutable()",
                    27,
                    "O"
                ],
                [
                    "dust_ledger",
                    "dust_ledger_is_immutable",
                    "ledger_row_is_immutable()",
                    27,
                    "O"
                ]
            ]),
        "triggers: {triggers}"
    );
    let guards: Value = sqlx::query_scalar(
        "SELECT jsonb_agg(jsonb_build_array(p.oid::regprocedure::text,pg_get_function_result(p.oid),l.lanname,p.proconfig)
                 ORDER BY p.proname)
         FROM pg_proc p JOIN pg_language l ON l.oid=p.prolang
         WHERE p.oid IN (to_regprocedure('public.cash_anchor_moves_forward()'),
                         to_regprocedure('public.cash_discrepancy_moves_forward()'))",
    )
    .fetch_one(&pool)
    .await?;
    ensure!(
        guards
            == json!([
                ["cash_anchor_moves_forward()", "trigger", "plpgsql", [
                    "search_path=pg_catalog, public"
                ]],
                ["cash_discrepancy_moves_forward()", "trigger", "plpgsql", [
                    "search_path=pg_catalog, public"
                ]]
            ]),
        "guards: {guards}"
    );
    let open_index: Value = sqlx::query_scalar(
        "SELECT jsonb_build_array(indisunique,indisvalid,pg_get_indexdef(indexrelid,1,false),
                pg_get_indexdef(indexrelid,2,false),pg_get_expr(indpred,indrelid))
         FROM pg_index WHERE indexrelid=to_regclass('public.idx_cash_discrepancies_open')",
    )
    .fetch_one(&pool)
    .await?;
    ensure!(
        open_index
            == json!([
                true,
                true,
                "tenant_id",
                "venue_id",
                "(state = 'open'::text)"
            ]),
        "open index: {open_index}"
    );
    Ok(())
}
