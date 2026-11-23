//! Every CHECK, FOREIGN KEY, UNIQUE and trigger rule of S8-5 (8p PLAN-r3
//! §4.3), on the schema installed through the bounded route. Rows are JSON
//! inserted with `jsonb_populate_record`, so each refusal case changes one
//! field and breaks exactly one CHECK conjunct or one key, so dropping that
//! predicate changes the outcome. A nil-id case whose fixture has no parent
//! row for that id (most parents refuse nil ids; the snapshot case adds the
//! nil tenant's version row) also lacks its parents: every CHECK runs before
//! any foreign key (RI triggers fire at the end of the statement), the CHECK
//! is the refusal, and without its conjunct the case is refused under the
//! key's name instead. A child table's version_row key
//! is implied by its parent key and has no case of its own (the catalog test
//! pins its mapping). A CHECK shadowed by a guard trigger is probed with that
//! trigger disabled in a rolled-back transaction. The snapshot's lifecycle is
//! driven by the updates its writers (8p-2, 8p-5..7) will issue, in miniature.
use serde_json::{json, Value};
use uuid::Uuid;

use super::{
    super::constraints::{
        admit, anchor_row, exec, insert, orphan, refused, scope, set, ts, with, Scope, IMMUTABLE,
        NIL, VENUE,
    },
    *,
};

/// Month K = December 2026, so B = 2026-12-01T00:00:00Z.
const MONTH: &str = "2026-12-01";
const B: &str = "2026-12-01 00:00:00+00";
const COHORT: &str = "2026-11-01";
const FORWARD: &str = "only moves forward";
const MODEL: &str = "net_from_start_non_expanding_v1";
/// The largest amount ledger_amount_fits accepts (2^96 - 1), and one past it.
const MAX: &str = "79228162514264337593543950335";
const OVER: &str = "79228162514264337593543950336";

/// The production route through 54; then every bounded Spot schema exists.
async fn install(pool: &PgPool) -> Result<()> {
    install_through(pool, VERSION54).await?;
    let mut observer = pool.acquire().await?;
    for version in SPOT_SCHEMA {
        ensure!(landed(&mut *observer, version).await?, "M{} missing", version % 100);
    }
    Ok(())
}

/// B plus `ms`, within a day either side.
fn at(ms: i64) -> String {
    let (day, t) = if ms < 0 {
        ("2026-11-30", 86_400_000 + ms)
    } else {
        ("2026-12-01", ms)
    };
    assert!((0..86_400_000).contains(&t), "fixture time out of range");
    format!(
        "{day} {:02}:{:02}:{:02}.{:03}+00",
        t / 3_600_000,
        t / 60_000 % 60,
        t / 1000 % 60,
        t % 1000
    )
}

/// The row inserted in a transaction that is always rolled back, with the
/// table's guard trigger disabled when `unguarded` names it: the CHECKs and
/// keys alone.
async fn probe(
    pool: &PgPool,
    table: &str,
    unguarded: Option<&str>,
    row: &Value,
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    if let Some(trigger) = unguarded {
        // Both names come from this file's fixed inventory.
        sqlx::raw_sql(&format!("ALTER TABLE public.{table} DISABLE TRIGGER {trigger}"))
            .execute(&mut *tx)
            .await?;
    }
    let result = insert(&mut *tx, table, row).await;
    tx.rollback().await?;
    result
}

fn snapshot_row(s: Scope) -> Value {
    json!({
        "tenant_id": s.tenant, "venue_id": VENUE, "month": MONTH, "as_of": B, "state": "pending",
        "revision": 0, "reopen_count": 0, "pending_since": at(30_000),
    })
}

/// A complete snapshot at revision 1 with its reset recorded, as a row (for
/// the CHECKs with the guard disabled).
fn published_row(s: Scope) -> Value {
    set(&snapshot_row(s), &[
        ("state", json!("complete")),
        ("revision", json!(1)),
        ("effective_basis", json!("2000")),
        ("completed_at", json!(at(120_000))),
        ("reset_revision", json!(1)),
        ("reset_basis", json!("2000")),
        ("published_revision", json!(1)),
        ("published_basis", json!("2000")),
    ])
}

/// Completion of the next revision at `basis` (8p-5): the effective basis is
/// the minimum over the revisions, completed_at moves forward.
fn complete(basis: &str) -> String {
    format!(
        "UPDATE boundary_snapshots SET state='complete', revision=revision+1,
         effective_basis=LEAST(COALESCE(effective_basis,{basis}),{basis}),
         completed_at=as_of+interval '2 minutes'+revision*interval '1 hour' WHERE tenant_id=$1"
    )
}
const REOPEN: &str =
    "UPDATE boundary_snapshots SET state='reopened', reopen_count=reopen_count+1 WHERE tenant_id=$1";
const RESET: &str = "UPDATE boundary_snapshots SET reset_revision=revision, reset_basis=GREATEST(effective_basis,0),
    published_revision=revision, published_basis=GREATEST(effective_basis,0) WHERE tenant_id=$1";
const RECAL: &str = "UPDATE boundary_snapshots SET published_revision=revision,
    published_basis=GREATEST(effective_basis,0), corrected_at=COALESCE(corrected_at,now()) WHERE tenant_id=$1";
const LATCH: &str = "UPDATE boundary_snapshots SET closed_by_correction_at=now()
    WHERE tenant_id=$1 AND closed_by_correction_at IS NULL";

async fn snapshot(pool: &PgPool, s: Scope) -> Result<Value> {
    Ok(sqlx::query_scalar(
        "SELECT jsonb_build_object('state',state,'revision',revision,'reopen_count',reopen_count,
            'effective',effective_basis::text,'reset',jsonb_build_array(reset_revision,reset_basis::text),
            'published',jsonb_build_array(published_revision,published_basis::text),
            'corrected',corrected_at IS NOT NULL,'closed',closed_by_correction_at IS NOT NULL)
         FROM boundary_snapshots WHERE tenant_id=$1",
    )
    .bind(s.tenant)
    .fetch_one(pool)
    .await?)
}

/// A scope with its pending December snapshot.
async fn pending(pool: &PgPool) -> Result<Scope> {
    let s = scope(pool).await?;
    insert(pool, "boundary_snapshots", &snapshot_row(s)).await?;
    Ok(s)
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn boundary_state_moves_only_forward(pool: PgPool) -> Result<()> {
    install(&pool).await?;
    let s = pending(&pool).await?;
    // pending -> reopened skips the completion.
    refused(
        exec(
            &pool,
            "UPDATE boundary_snapshots SET state='reopened', revision=1, effective_basis=2000,
            completed_at=as_of+interval '2 minutes' WHERE tenant_id=$1",
            s.tenant,
        )
        .await,
        "23514",
        FORWARD,
    )?;
    // The first completion is revision 1, nothing else moves with it.
    refused(
        exec(
            &pool,
            "UPDATE boundary_snapshots SET state='complete', revision=2, effective_basis=2000,
            completed_at=as_of+interval '2 minutes' WHERE tenant_id=$1",
            s.tenant,
        )
        .await,
        "23514",
        FORWARD,
    )?;
    refused(
        exec(
            &pool,
            "UPDATE boundary_snapshots SET state='complete', revision=1, effective_basis=2000,
            completed_at=as_of+interval '2 minutes', corrected_at=now() WHERE tenant_id=$1",
            s.tenant,
        )
        .await,
        "23514",
        FORWARD,
    )?;
    ensure!(exec(&pool, &complete("2000"), s.tenant).await? == 1, "pending -> complete");
    for back in [
        "UPDATE boundary_snapshots SET state='pending', revision=0, effective_basis=NULL, completed_at=NULL
            WHERE tenant_id=$1",
        // a revision completes only after a reopening
        "UPDATE boundary_snapshots SET revision=2, completed_at=completed_at+interval '1 hour' WHERE tenant_id=$1",
        "UPDATE boundary_snapshots SET state='reopened', revision=2, reopen_count=1 WHERE tenant_id=$1",
    ] {
        refused(exec(&pool, back, s.tenant).await, "23514", FORWARD)?;
    }
    ensure!(exec(&pool, REOPEN, s.tenant).await? == 1, "complete -> reopened");
    for back in [
        REOPEN,
        "UPDATE boundary_snapshots SET state='pending', revision=0, effective_basis=NULL, completed_at=NULL
            WHERE tenant_id=$1",
        // the next revision is n + 1, and completed_at never goes back
        "UPDATE boundary_snapshots SET state='complete', revision=revision+2,
            completed_at=completed_at+interval '1 hour' WHERE tenant_id=$1",
        "UPDATE boundary_snapshots SET state='complete', revision=revision+1,
            completed_at=completed_at-interval '1 second' WHERE tenant_id=$1",
        // a reopened row changes nothing but its state through a completion
        "UPDATE boundary_snapshots SET completed_at=completed_at+interval '1 hour' WHERE tenant_id=$1",
    ] {
        refused(exec(&pool, back, s.tenant).await, "23514", FORWARD)?;
    }
    ensure!(exec(&pool, &complete("1990"), s.tenant).await? == 1, "reopened -> complete");
    // An update that changes nothing is no transition.
    ensure!(
        exec(&pool, "UPDATE boundary_snapshots SET state=state WHERE tenant_id=$1", s.tenant)
            .await?
            == 1
    );
    // The month, its B and pending_since never change; a snapshot is never deleted.
    for identity in [
        "UPDATE boundary_snapshots SET pending_since=pending_since+interval '1 second' WHERE tenant_id=$1",
        "UPDATE boundary_snapshots SET month='2027-01-01', as_of='2027-01-01 00:00:00+00' WHERE tenant_id=$1",
        "UPDATE boundary_snapshots SET venue_id='binance:usdm' WHERE tenant_id=$1",
    ] {
        refused(exec(&pool, identity, s.tenant).await, "23514", FORWARD)?;
    }
    refused(
        exec(&pool, "DELETE FROM boundary_snapshots WHERE tenant_id=$1", s.tenant).await,
        "23514",
        "never deleted",
    )?;
    ensure!(
        snapshot(&pool, s).await?
            == json!({"state": "complete", "revision": 2, "reopen_count": 1, "effective": "1990",
                "reset": [null, null], "published": [null, null], "corrected": false, "closed": false})
    );
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn boundary_effective_basis_never_rises(pool: PgPool) -> Result<()> {
    install(&pool).await?;
    let s = pending(&pool).await?;
    ensure!(exec(&pool, &complete("2000"), s.tenant).await? == 1);
    // Outside a completion it never moves, up or down.
    for moved in ["effective_basis+1", "effective_basis-1"] {
        refused(
            exec(
                &pool,
                &format!(
                    "UPDATE boundary_snapshots SET effective_basis={moved} WHERE tenant_id=$1"
                ),
                s.tenant,
            )
            .await,
            "23514",
            FORWARD,
        )?;
    }
    refused(
        exec(
            &pool,
            "UPDATE boundary_snapshots SET state='reopened', reopen_count=reopen_count+1,
            effective_basis=1000 WHERE tenant_id=$1",
            s.tenant,
        )
        .await,
        "23514",
        FORWARD,
    )?;
    ensure!(exec(&pool, REOPEN, s.tenant).await? == 1);
    // A recomputation never raises it (v16:324: effective = min).
    refused(
        exec(&pool, "UPDATE boundary_snapshots SET state='complete', revision=revision+1, effective_basis=2010,
            completed_at=completed_at+interval '1 hour' WHERE tenant_id=$1", s.tenant).await,
        "23514",
        FORWARD,
    )?;
    // Equal or lower completes.
    ensure!(
        exec(&pool, "UPDATE boundary_snapshots SET state='complete', revision=revision+1, effective_basis=2000,
            completed_at=completed_at+interval '1 hour' WHERE tenant_id=$1", s.tenant).await? == 1
    );
    ensure!(exec(&pool, REOPEN, s.tenant).await? == 1);
    ensure!(exec(&pool, &complete("1990"), s.tenant).await? == 1);
    ensure!(exec(&pool, REOPEN, s.tenant).await? == 1);
    // Codex's case (v16:1938-1939): a later revision at 2,005 keeps 1,990.
    ensure!(exec(&pool, &complete("2005"), s.tenant).await? == 1);
    ensure!(snapshot(&pool, s).await?["effective"] == json!("1990"));
    ensure!(snapshot(&pool, s).await?["revision"] == json!(4));
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn boundary_reopen_count_moves_only_on_reopen(pool: PgPool) -> Result<()> {
    install(&pool).await?;
    let s = pending(&pool).await?;
    let count = "UPDATE boundary_snapshots SET reopen_count=reopen_count+1 WHERE tenant_id=$1";
    refused(exec(&pool, count, s.tenant).await, "23514", FORWARD)?;
    ensure!(exec(&pool, &complete("2000"), s.tenant).await? == 1);
    for moved in [
        count,
        "UPDATE boundary_snapshots SET reopen_count=reopen_count-1 WHERE tenant_id=$1",
        // a reopening adds exactly one
        "UPDATE boundary_snapshots SET state='reopened' WHERE tenant_id=$1",
        "UPDATE boundary_snapshots SET state='reopened', reopen_count=reopen_count+2 WHERE tenant_id=$1",
    ] {
        refused(exec(&pool, moved, s.tenant).await, "23514", FORWARD)?;
    }
    ensure!(exec(&pool, REOPEN, s.tenant).await? == 1);
    refused(exec(&pool, count, s.tenant).await, "23514", FORWARD)?;
    // the recomputation keeps it
    refused(
        exec(&pool, "UPDATE boundary_snapshots SET state='complete', revision=revision+1,
            reopen_count=reopen_count+1, completed_at=completed_at+interval '1 hour' WHERE tenant_id=$1", s.tenant).await,
        "23514",
        FORWARD,
    )?;
    ensure!(exec(&pool, &complete("1990"), s.tenant).await? == 1);
    ensure!(exec(&pool, RESET, s.tenant).await? == 1);
    refused(exec(&pool, count, s.tenant).await, "23514", FORWARD)?;
    let row = snapshot(&pool, s).await?;
    ensure!(
        (row["revision"].clone(), row["reopen_count"].clone()) == (json!(2), json!(1)),
        "{row}"
    );
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn boundary_reset_is_recorded_once(pool: PgPool) -> Result<()> {
    install(&pool).await?;
    let s = pending(&pool).await?;
    // Only a complete row records its reset.
    refused(exec(&pool, RESET, s.tenant).await, "23514", FORWARD)?;
    ensure!(exec(&pool, &complete("2000"), s.tenant).await? == 1);
    for wrong in [
        // not max(0, effective) at the current revision, or the pointer elsewhere
        "UPDATE boundary_snapshots SET reset_revision=revision, reset_basis=1999, published_revision=revision,
            published_basis=1999 WHERE tenant_id=$1",
        "UPDATE boundary_snapshots SET reset_revision=revision, reset_basis=2000, published_revision=revision,
            published_basis=1990 WHERE tenant_id=$1",
    ] {
        refused(exec(&pool, wrong, s.tenant).await, "23514", FORWARD)?;
    }
    ensure!(exec(&pool, RESET, s.tenant).await? == 1, "first reset");
    // The same values again change nothing.
    ensure!(exec(&pool, RESET, s.tenant).await? == 1);
    ensure!(exec(&pool, REOPEN, s.tenant).await? == 1);
    // A reopened row records nothing.
    refused(
        exec(
            &pool,
            "UPDATE boundary_snapshots SET reset_basis=1990 WHERE tenant_id=$1",
            s.tenant,
        )
        .await,
        "23514",
        FORWARD,
    )?;
    ensure!(exec(&pool, &complete("1990"), s.tenant).await? == 1);
    for again in [
        RESET,
        "UPDATE boundary_snapshots SET reset_basis=1990 WHERE tenant_id=$1",
        "UPDATE boundary_snapshots SET reset_revision=NULL, reset_basis=NULL, published_revision=NULL,
            published_basis=NULL WHERE tenant_id=$1",
    ] {
        refused(exec(&pool, again, s.tenant).await, "23514", FORWARD)?;
    }
    ensure!(snapshot(&pool, s).await?["reset"] == json!([1, "2000"]));
    // A negative effective basis resets at 0 (max(0, ·), §6.12.7).
    let negative = pending(&pool).await?;
    ensure!(exec(&pool, &complete("-5"), negative.tenant).await? == 1);
    ensure!(exec(&pool, RESET, negative.tenant).await? == 1);
    let row = snapshot(&pool, negative).await?;
    ensure!(
        (row["reset"].clone(), row["published"].clone()) == (json!([1, "0"]), json!([1, "0"])),
        "{row}"
    );
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn two_successive_downward_corrections_publish_and_latch(pool: PgPool) -> Result<()> {
    install(&pool).await?;
    let s = pending(&pool).await?;
    ensure!(exec(&pool, &complete("2000"), s.tenant).await? == 1);
    ensure!(exec(&pool, RESET, s.tenant).await? == 1);
    // A correction publishes below the halt basis.
    refused(exec(&pool, RECAL, s.tenant).await, "23514", "boundary_snapshots_correction")?;
    ensure!(exec(&pool, REOPEN, s.tenant).await? == 1);
    ensure!(exec(&pool, &complete("1990"), s.tenant).await? == 1);
    // First correction: the pointer advances to revision 2 at 1,990.
    ensure!(exec(&pool, RECAL, s.tenant).await? == 1, "first correction");
    let first: String =
        sqlx::query_scalar("SELECT corrected_at::text FROM boundary_snapshots WHERE tenant_id=$1")
            .bind(s.tenant)
            .fetch_one(&pool)
            .await?;
    ensure!(exec(&pool, REOPEN, s.tenant).await? == 1);
    ensure!(exec(&pool, &complete("1980"), s.tenant).await? == 1);
    for wrong in [
        // not max(0, effective), or not the current revision
        "UPDATE boundary_snapshots SET published_revision=revision, published_basis=1985 WHERE tenant_id=$1",
        "UPDATE boundary_snapshots SET published_basis=1980 WHERE tenant_id=$1",
    ] {
        refused(exec(&pool, wrong, s.tenant).await, "23514", FORWARD)?;
    }
    // Second correction, then the closure latch in the same recalibration.
    ensure!(exec(&pool, RECAL, s.tenant).await? == 1, "second correction");
    ensure!(exec(&pool, LATCH, s.tenant).await? == 1, "latch");
    let kept: String =
        sqlx::query_scalar("SELECT corrected_at::text FROM boundary_snapshots WHERE tenant_id=$1")
            .bind(s.tenant)
            .fetch_one(&pool)
            .await?;
    ensure!(kept == first, "the first correction stays");
    for moved in [
        "UPDATE boundary_snapshots SET closed_by_correction_at=closed_by_correction_at+interval '1 second'
            WHERE tenant_id=$1",
        "UPDATE boundary_snapshots SET corrected_at=corrected_at+interval '1 second' WHERE tenant_id=$1",
        "UPDATE boundary_snapshots SET corrected_at=NULL, closed_by_correction_at=NULL WHERE tenant_id=$1",
    ] {
        refused(exec(&pool, moved, s.tenant).await, "23514", FORWARD)?;
    }
    // A later revision at a higher basis keeps 1,980: the pointer never rises,
    // to it or back to the reset basis.
    ensure!(exec(&pool, REOPEN, s.tenant).await? == 1);
    ensure!(exec(&pool, &complete("1985"), s.tenant).await? == 1);
    for rise in [
        "UPDATE boundary_snapshots SET published_revision=revision, published_basis=1985 WHERE tenant_id=$1",
        "UPDATE boundary_snapshots SET published_revision=revision, published_basis=reset_basis WHERE tenant_id=$1",
    ] {
        refused(exec(&pool, rise, s.tenant).await, "23514", FORWARD)?;
    }
    // A negative effective basis publishes 0.
    ensure!(exec(&pool, REOPEN, s.tenant).await? == 1);
    ensure!(exec(&pool, &complete("-5"), s.tenant).await? == 1);
    ensure!(exec(&pool, RECAL, s.tenant).await? == 1, "third correction");
    let row = snapshot(&pool, s).await?;
    ensure!(
        row == json!({"state": "complete", "revision": 5, "reopen_count": 4, "effective": "-5",
            "reset": [1, "2000"], "published": [5, "0"], "corrected": true, "closed": true}),
        "{row}"
    );
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn boundary_snapshots_refuse_incoherent_rows(pool: PgPool) -> Result<()> {
    install(&pool).await?;
    let s = scope(&pool).await?;
    let unknown = orphan(&pool).await?;
    let row = snapshot_row(s);
    let done = published_row(s);
    let unpublished = set(&done, &[
        ("reset_revision", Value::Null),
        ("reset_basis", Value::Null),
        ("published_revision", Value::Null),
        ("published_basis", Value::Null),
    ]);
    let guard = Some("boundary_snapshots_move_forward");
    // A version row of the nil tenant (accounting_state has no nil check), so
    // the nil-tenant case breaks only the CHECK.
    sqlx::query(
        "INSERT INTO accounting_state(tenant_id,venue_id,budget_model) VALUES($1,$2,'hwm_v1')",
    )
    .bind(Uuid::nil())
    .bind(VENUE)
    .execute(&pool)
    .await?;
    // Pending rows pass the guard; each case breaks one predicate.
    for (row, code, needle) in [
        (with(&row, "tenant_id", json!(NIL)), "23514", "boundary_snapshots_ids"),
        (
            set(&row, &[
                ("month", json!("2026-12-02")),
                ("as_of", json!("2026-12-02 00:00:00+00")),
            ]),
            "23514",
            "boundary_snapshots_month",
        ),
        (
            with(&row, "as_of", json!("2026-12-01 00:00:00-03")),
            "23514",
            "boundary_snapshots_as_of",
        ),
        // pending is revision 0, which has no effective basis and no completion
        (
            set(&row, &[
                ("revision", json!(1)),
                ("effective_basis", json!("2000")),
                ("completed_at", json!(at(120_000))),
            ]),
            "23514",
            "boundary_snapshots_revision",
        ),
        (
            with(&row, "effective_basis", json!("0")),
            "23514",
            "boundary_snapshots_revision",
        ),
        (
            with(&row, "completed_at", json!(at(120_000))),
            "23514",
            "boundary_snapshots_revision",
        ),
        (
            with(&row, "corrected_at", json!(at(1))),
            "23514",
            "boundary_snapshots_correction",
        ),
        (
            with(&row, "closed_by_correction_at", json!(at(1))),
            "23514",
            "boundary_snapshots_correction",
        ),
        (snapshot_row(unknown), "23503", "boundary_snapshots_version_row"),
    ] {
        refused(probe(&pool, "boundary_snapshots", None, &row).await, code, needle)?;
    }
    // The CHECKs alone, past the guard: a well-formed published row is
    // accepted; each case breaks one predicate. reset_revision <= revision
    // and reset_basis >= 0 follow from the pointer's own bounds, so they
    // have no case of their own.
    probe(&pool, "boundary_snapshots", guard, &done).await?;
    probe(&pool, "boundary_snapshots", guard, &unpublished).await?;
    for (row, needle) in [
        (with(&done, "state", json!("closed")), "boundary_snapshots_state"),
        (with(&unpublished, "revision", json!(-1)), "boundary_snapshots_revision"),
        (with(&done, "reopen_count", json!(-1)), "boundary_snapshots_revision"),
        (with(&done, "state", json!("pending")), "boundary_snapshots_revision"),
        (with(&done, "effective_basis", json!("NaN")), "boundary_snapshots_amounts"),
        (with(&done, "reset_basis", json!("Infinity")), "boundary_snapshots_amounts"),
        (with(&done, "published_basis", json!("1e-29")), "boundary_snapshots_amounts"),
        (with(&done, "reset_basis", Value::Null), "boundary_snapshots_publication"),
        (with(&done, "reset_revision", json!(0)), "boundary_snapshots_publication"),
        (
            set(&done, &[
                ("revision", json!(3)),
                ("reset_revision", json!(2)),
                ("published_revision", json!(1)),
            ]),
            "boundary_snapshots_publication",
        ),
        (with(&done, "published_revision", json!(2)), "boundary_snapshots_publication"),
        (with(&done, "published_basis", json!("-1")), "boundary_snapshots_publication"),
        (
            with(&done, "published_basis", json!("2000.01")),
            "boundary_snapshots_publication",
        ),
        (with(&done, "corrected_at", json!(at(1))), "boundary_snapshots_correction"),
    ] {
        refused(probe(&pool, "boundary_snapshots", guard, &row).await, "23514", needle)?;
    }
    // Below the halt basis a correction and its latch are coherent.
    probe(
        &pool,
        "boundary_snapshots",
        guard,
        &set(&done, &[
            ("revision", json!(2)),
            ("reopen_count", json!(1)),
            ("effective_basis", json!("1990")),
            ("published_revision", json!(2)),
            ("published_basis", json!("1990")),
            ("corrected_at", json!(at(1))),
            ("closed_by_correction_at", json!(at(1))),
        ]),
    )
    .await?;
    // A snapshot starts pending, with no reopening.
    for born in [done.clone(), with(&row, "reopen_count", json!(1))] {
        refused(probe(&pool, "boundary_snapshots", None, &born).await, "23514", "starts pending")?;
    }
    insert(&pool, "boundary_snapshots", &row).await?;
    refused(
        insert(&pool, "boundary_snapshots", &row).await,
        "23505",
        "boundary_snapshots_key",
    )?;
    // The same month of another venue row is another snapshot.
    let other = scope(&pool).await?;
    insert(&pool, "boundary_snapshots", &snapshot_row(other)).await?;
    Ok(())
}

fn reopening_row(s: Scope, seq: i32, cause: &str) -> Value {
    json!({
        "tenant_id": s.tenant, "venue_id": VENUE, "month": MONTH, "reopen_seq": seq, "from_revision": seq,
        "detected_at": at(3_600_000), "cause": cause, "revision_d_fills": 3, "revision_d_flows": 1,
        "revision_d_legs": 2, "observed_d_fills": 4, "observed_d_flows": 1, "observed_d_legs": 2,
        "evidence_ids": ["fill:binance:spot:s84-local:BTCUSDT:9001"],
    })
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn boundary_reopenings_follow_their_rules(pool: PgPool) -> Result<()> {
    install(&pool).await?;
    let s = pending(&pool).await?;
    let good = reopening_row(s, 1, "inputs_changed");
    let unchanged = set(&good, &[("cause", json!("impaired")), ("observed_d_fills", json!(3))]);
    let many: Vec<String> = (0..65).map(|k| format!("fill:{k}")).collect();
    for (row, code, needle) in [
        (with(&good, "tenant_id", json!(NIL)), "23514", "boundary_reopenings_ids"),
        (with(&good, "reopen_seq", json!(0)), "23514", "boundary_reopenings_seq"),
        (with(&good, "from_revision", json!(0)), "23514", "boundary_reopenings_seq"),
        (with(&good, "cause", json!("late_fill")), "23514", "boundary_reopenings_cause"),
        (
            with(&good, "revision_d_fills", json!(-1)),
            "23514",
            "boundary_reopenings_digests",
        ),
        (
            with(&good, "revision_d_flows", json!(-1)),
            "23514",
            "boundary_reopenings_digests",
        ),
        (
            with(&good, "revision_d_legs", json!(-1)),
            "23514",
            "boundary_reopenings_digests",
        ),
        (
            with(&good, "observed_d_fills", json!(-1)),
            "23514",
            "boundary_reopenings_digests",
        ),
        (
            with(&good, "observed_d_flows", json!(-1)),
            "23514",
            "boundary_reopenings_digests",
        ),
        (
            with(&good, "observed_d_legs", json!(-1)),
            "23514",
            "boundary_reopenings_digests",
        ),
        // late evidence is a changed digest
        (
            with(&good, "observed_d_fills", json!(3)),
            "23514",
            "boundary_reopenings_changed",
        ),
        (
            with(&good, "evidence_ids", json!({"fill": 1})),
            "23514",
            "boundary_reopenings_evidence",
        ),
        (
            with(&good, "evidence_ids", json!(many)),
            "23514",
            "boundary_reopenings_evidence",
        ),
        (
            with(&good, "month", json!("2027-01-01")),
            "23503",
            "boundary_reopenings_snapshot",
        ),
    ] {
        refused(probe(&pool, "boundary_reopenings", None, &row).await, code, needle)?;
    }
    // An impaired E(B) reopens with unchanged counts; 64 keys at most.
    probe(&pool, "boundary_reopenings", None, &unchanged).await?;
    probe(
        &pool,
        "boundary_reopenings",
        None,
        &with(&good, "evidence_ids", json!(many[..64])),
    )
    .await?;
    probe(&pool, "boundary_reopenings", None, &with(&good, "evidence_ids", json!([]))).await?;
    insert(&pool, "boundary_reopenings", &good).await?;
    refused(
        insert(&pool, "boundary_reopenings", &unchanged).await,
        "23505",
        "boundary_reopenings_key",
    )?;
    insert(&pool, "boundary_reopenings", &with(&unchanged, "reopen_seq", json!(2))).await?;
    // Immutable (migration 52).
    for change in [
        "UPDATE boundary_reopenings SET detected_at=detected_at WHERE tenant_id=$1",
        "DELETE FROM boundary_reopenings WHERE tenant_id=$1",
    ] {
        refused(exec(&pool, change, s.tenant).await, "23514", IMMUTABLE)?;
    }
    Ok(())
}

fn revision_row(s: Scope, anchor: Uuid, revision: i32) -> Value {
    json!({
        "tenant_id": s.tenant, "venue_id": VENUE, "month": MONTH, "revision": revision, "as_of": B,
        "anchor_id": anchor, "anchor_as_of": ts(0), "quote_b": "2000", "mark_price": "60000.5",
        "mark_kline_open_time": at(-60_000), "mark_kline_close_time": at(-1), "floor_total": "0",
        "downside_total": "0", "basis": "2000", "carried_count": 0, "budget_model": MODEL,
        "completed_version": 7, "evidence_hash": "ab".repeat(32), "d_fills": 3, "d_flows": 1, "d_legs": 2,
        "reason": if revision == 1 { "initial" } else { "reopened" },
        "reopen_seq": if revision == 1 { None } else { Some(revision - 1) },
        "computed_at": at(65_000), "computed_clock_sample_id": s.sample, "computed_clock_wall_delta_ms": 0,
        "computed_clock_mono_delta_ms": 0, "computed_clock_verdict": "valid",
    })
}

/// A scope with its pending December snapshot and an observed November
/// anchor (as_of = ts(0)).
async fn anchored(pool: &PgPool) -> Result<(Scope, Uuid)> {
    let s = pending(pool).await?;
    let anchor = Uuid::now_v7();
    insert(pool, "cash_anchors", &anchor_row(s, anchor, "reanchor", None, Some(0))).await?;
    Ok((s, anchor))
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn boundary_revision_basis_is_quote_plus_floors(pool: PgPool) -> Result<()> {
    install(&pool).await?;
    let (s, anchor) = anchored(&pool).await?;
    let good = revision_row(s, anchor, 1);
    // basis_n = quote_B + sum of F_B, an entry in flight's negative floor included.
    for (floors, basis) in [
        ("588.82059", "2588.82059"),
        ("-12", "1988"),
        ("576.82059", "2576.82059"),
    ] {
        probe(
            &pool,
            "boundary_snapshot_revisions",
            None,
            &set(&good, &[("floor_total", json!(floors)), ("basis", json!(basis))]),
        )
        .await?;
    }
    for (floors, basis) in [
        ("588.82059", "2000"),
        ("588.82059", "2588.82058"),
        ("-12", "2000"),
        ("0", "2000.00000001"),
    ] {
        refused(
            probe(
                &pool,
                "boundary_snapshot_revisions",
                None,
                &set(&good, &[("floor_total", json!(floors)), ("basis", json!(basis))]),
            )
            .await,
            "23514",
            "boundary_snapshot_revisions_basis",
        )?;
    }
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn boundary_snapshot_revisions_refuse_incoherent_rows(pool: PgPool) -> Result<()> {
    install(&pool).await?;
    let (s, anchor) = anchored(&pool).await?;
    let other = scope(&pool).await?;
    let foreign = Uuid::now_v7();
    insert(&pool, "cash_anchors", &anchor_row(other, foreign, "reanchor", None, Some(0))).await?;
    let good = revision_row(s, anchor, 1);
    let second = revision_row(s, anchor, 2);
    // Reopening 1 exists, so a case naming it breaks no key.
    insert(&pool, "boundary_reopenings", &reopening_row(s, 1, "inputs_changed")).await?;
    // Each case breaks one predicate (OVER is one past ledger_amount_fits).
    for (row, code, needle) in [
        (with(&good, "tenant_id", json!(NIL)), "23514", "boundary_snapshot_revisions_ids"),
        (with(&good, "anchor_id", json!(NIL)), "23514", "boundary_snapshot_revisions_ids"),
        (
            with(&good, "computed_clock_sample_id", json!(NIL)),
            "23514",
            "boundary_snapshot_revisions_ids",
        ),
        (
            set(&good, &[
                ("as_of", json!(at(1))),
                ("mark_kline_open_time", json!(at(-59_999))),
                ("mark_kline_close_time", json!(at(0))),
            ]),
            "23514",
            "boundary_snapshot_revisions_as_of",
        ),
        (
            with(&good, "anchor_as_of", json!(B)),
            "23514",
            "boundary_snapshot_revisions_as_of",
        ),
        (
            with(&good, "anchor_as_of", json!(at(1))),
            "23514",
            "boundary_snapshot_revisions_as_of",
        ),
        (
            with(&good, "mark_price", json!("0")),
            "23514",
            "boundary_snapshot_revisions_mark",
        ),
        (
            with(&good, "mark_kline_open_time", json!(at(-59_999))),
            "23514",
            "boundary_snapshot_revisions_mark",
        ),
        (
            with(&good, "mark_kline_close_time", json!(B)),
            "23514",
            "boundary_snapshot_revisions_mark",
        ),
        (
            with(&good, "mark_kline_close_time", json!(at(-2))),
            "23514",
            "boundary_snapshot_revisions_mark",
        ),
        (
            set(&good, &[
                ("quote_b", json!(format!("-{OVER}"))),
                ("floor_total", json!(MAX)),
                ("basis", json!("-1")),
            ]),
            "23514",
            "boundary_snapshot_revisions_amounts",
        ),
        (
            with(&good, "mark_price", json!("NaN")),
            "23514",
            "boundary_snapshot_revisions_amounts",
        ),
        (
            set(&good, &[
                ("floor_total", json!(format!("-{OVER}"))),
                ("basis", json!("-79228162514264337593543948336")),
            ]),
            "23514",
            "boundary_snapshot_revisions_amounts",
        ),
        (
            with(&good, "downside_total", json!("NaN")),
            "23514",
            "boundary_snapshot_revisions_amounts",
        ),
        (
            set(&good, &[
                ("quote_b", json!(MAX)),
                ("floor_total", json!("1")),
                ("basis", json!(OVER)),
            ]),
            "23514",
            "boundary_snapshot_revisions_amounts",
        ),
        (
            set(&good, &[
                ("revision", json!(0)),
                ("reason", json!("reopened")),
                ("reopen_seq", json!(1)),
            ]),
            "23514",
            "boundary_snapshot_revisions_counts",
        ),
        (
            with(&good, "carried_count", json!(-1)),
            "23514",
            "boundary_snapshot_revisions_counts",
        ),
        (
            with(&good, "completed_version", json!(-1)),
            "23514",
            "boundary_snapshot_revisions_counts",
        ),
        (with(&good, "d_fills", json!(-1)), "23514", "boundary_snapshot_revisions_counts"),
        (with(&good, "d_flows", json!(-1)), "23514", "boundary_snapshot_revisions_counts"),
        (with(&good, "d_legs", json!(-1)), "23514", "boundary_snapshot_revisions_counts"),
        (
            with(&good, "budget_model", json!("hwm_v1")),
            "23514",
            "boundary_snapshot_revisions_budget_model",
        ),
        (
            with(&good, "evidence_hash", json!("AB".repeat(32))),
            "23514",
            "boundary_snapshot_revisions_evidence_hash",
        ),
        (
            with(&good, "evidence_hash", json!("ab".repeat(31))),
            "23514",
            "boundary_snapshot_revisions_evidence_hash",
        ),
        (
            set(&second, &[("reason", json!("recomputed")), ("reopen_seq", Value::Null)]),
            "23514",
            "boundary_snapshot_revisions_reason",
        ),
        (
            set(&second, &[("reason", json!("initial")), ("reopen_seq", Value::Null)]),
            "23514",
            "boundary_snapshot_revisions_reason",
        ),
        (
            with(&good, "reopen_seq", json!(1)),
            "23514",
            "boundary_snapshot_revisions_reason",
        ),
        (
            with(&second, "reopen_seq", Value::Null),
            "23514",
            "boundary_snapshot_revisions_reason",
        ),
        (
            with(&good, "computed_clock_verdict", json!("invalid")),
            "23514",
            "boundary_snapshot_revisions_computed",
        ),
        (
            with(&good, "computed_at", json!(at(59_999))),
            "23514",
            "boundary_snapshot_revisions_computed",
        ),
        // the other venue row's own anchor and sample, but no snapshot of its month
        (revision_row(other, foreign, 1), "23503", "boundary_snapshot_revisions_snapshot"),
        (revision_row(s, anchor, 3), "23503", "boundary_snapshot_revisions_reopening"),
        (
            with(&good, "anchor_id", json!(foreign)),
            "23503",
            "boundary_snapshot_revisions_anchor",
        ),
        (
            with(&good, "computed_clock_sample_id", json!(other.sample)),
            "23503",
            "boundary_snapshot_revisions_computed_sample",
        ),
    ] {
        refused(probe(&pool, "boundary_snapshot_revisions", None, &row).await, code, needle)?;
    }
    // Revision 1, then a reopening and the revision that follows it, at
    // exactly B + 60 s.
    insert(&pool, "boundary_snapshot_revisions", &good).await?;
    refused(
        insert(&pool, "boundary_snapshot_revisions", &good).await,
        "23505",
        "boundary_snapshot_revisions_key",
    )?;
    insert(
        &pool,
        "boundary_snapshot_revisions",
        &with(&second, "computed_at", json!(at(60_000))),
    )
    .await?;
    Ok(())
}

/// A scope with its December snapshot, revision 1 and one admitted leg.
struct Carry {
    s: Scope,
    position: Uuid,
}

async fn carry(pool: &PgPool) -> Result<Carry> {
    let (s, anchor) = anchored(pool).await?;
    insert(pool, "boundary_snapshot_revisions", &revision_row(s, anchor, 1)).await?;
    let position = admit(pool, s.tenant).await?;
    Ok(Carry { s, position })
}

/// An inventory leg protected at B through `branch`: G_B 0.01, the stop at
/// 59,000 acknowledged at B - 10 s, gap and fee 10 bps, no reserve, so
/// AF_B = p_B = 58,941 and F_B = 0.01 * 58,941 * 0.999 = 588.82059 against a
/// mark value of 600.
fn leg_row(c: &Carry, branch: &str) -> Value {
    let mut row = json!({
        "tenant_id": c.s.tenant, "venue_id": VENUE, "month": MONTH, "revision": 1, "position_id": c.position,
        "account_scope": "s84-local", "symbol": "BTCUSDT", "cohort_month": COHORT, "kind": "inventory",
        "g_b": "0.01", "vwap": "60000", "protected_at_b": true, "stop_order_id": "123", "trigger_b": "59000",
        "open_after_b": branch, "stop_live_seen_at": at(-10_000), "stop_live_clock_verdict": "valid",
        "exclusions": [], "gap_bps": "10", "fee_bps": "10", "reserve_qty": "0", "admission_charge": "12",
        "adverse_fill_b": "58941", "floor_price_b": "58941", "floor_quote": "588.82059", "mark_value": "600",
        "downside": "11.17941",
    });
    let evidence = match branch {
        "new_at_completion" => json!({
            "lookup_sent_at": at(62_000), "lookup_clock_sample_id": c.s.sample, "lookup_clock_verdict": "valid",
            "lookup_status": "NEW",
        }),
        "trade_after_b" => json!({"stop_trade_id": "456"}),
        "robson_cancel_after_b" => json!({
            "cancel_intent_id": Uuid::now_v7(), "cancel_attempt_at": at(70_000),
            "cancel_attempt_clock_verdict": "valid",
        }),
        other => panic!("unexpected branch {other}"),
    };
    for (key, value) in evidence.as_object().expect("evidence object") {
        row[key] = value.clone();
    }
    row
}

/// The same inventory leg unprotected at B: floor 0, downside the mark value.
fn unprotected_row(c: &Carry) -> Value {
    set(&leg_row(c, "trade_after_b"), &[
        ("protected_at_b", json!(false)),
        ("stop_order_id", Value::Null),
        ("trigger_b", Value::Null),
        ("open_after_b", Value::Null),
        ("stop_trade_id", Value::Null),
        ("stop_live_seen_at", Value::Null),
        ("stop_live_clock_verdict", Value::Null),
        ("adverse_fill_b", Value::Null),
        ("floor_price_b", Value::Null),
        ("floor_quote", json!("0")),
        ("downside", json!("600")),
    ])
}

/// An entry in flight at B: nothing carried, floor -a.
fn in_flight_row(c: &Carry) -> Value {
    set(&unprotected_row(c), &[
        ("kind", json!("entry_in_flight")),
        ("g_b", json!("0")),
        ("vwap", Value::Null),
        ("floor_quote", json!("-12")),
        ("mark_value", json!("0")),
        ("downside", json!("12")),
    ])
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn entry_in_flight_floor_is_minus_the_charge(pool: PgPool) -> Result<()> {
    install(&pool).await?;
    let c = carry(&pool).await?;
    let flight = in_flight_row(&c);
    probe(&pool, "boundary_carried_legs", None, &flight).await?;
    probe(
        &pool,
        "boundary_carried_legs",
        None,
        &with(&flight, "prior_month_excess", json!("1.3792")),
    )
    .await?;
    for row in [
        set(&flight, &[("floor_quote", json!("0")), ("downside", json!("0"))]),
        set(&flight, &[("floor_quote", json!("-11")), ("downside", json!("11"))]),
        set(&flight, &[("floor_quote", json!("12")), ("downside", json!("-12"))]),
        with(&flight, "g_b", json!("0.01")),
    ] {
        refused(
            probe(&pool, "boundary_carried_legs", None, &row).await,
            "23514",
            "boundary_carried_legs_in_flight",
        )?;
    }
    // An entry in flight is never protected, whatever the evidence.
    let protected = set(&leg_row(&c, "trade_after_b"), &[
        ("kind", json!("entry_in_flight")),
        ("g_b", json!("0")),
        ("floor_quote", json!("-12")),
        ("mark_value", json!("0")),
        ("downside", json!("12")),
    ]);
    refused(
        probe(&pool, "boundary_carried_legs", None, &protected).await,
        "23514",
        "boundary_carried_legs_in_flight",
    )?;
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn boundary_carried_legs_refuse_incoherent_rows(pool: PgPool) -> Result<()> {
    install(&pool).await?;
    let c = carry(&pool).await?;
    let other = scope(&pool).await?;
    let lookup = leg_row(&c, "new_at_completion");
    let trade = leg_row(&c, "trade_after_b");
    let cancel = leg_row(&c, "robson_cancel_after_b");
    let bare = unprotected_row(&c);
    let flight = in_flight_row(&c);
    for healthy in [&lookup, &trade, &cancel, &bare, &flight] {
        probe(&pool, "boundary_carried_legs", None, healthy).await?;
    }
    // Exactly at the 2 s margins.
    probe(
        &pool,
        "boundary_carried_legs",
        None,
        &with(&trade, "stop_live_seen_at", json!(at(-2_000))),
    )
    .await?;
    probe(
        &pool,
        "boundary_carried_legs",
        None,
        &with(&lookup, "lookup_sent_at", json!(at(2_000))),
    )
    .await?;
    probe(
        &pool,
        "boundary_carried_legs",
        None,
        &with(&cancel, "cancel_attempt_at", json!(at(2_000))),
    )
    .await?;
    let too_many: Vec<String> = (0..65).map(|k| format!("order:{k}")).collect();
    // A protected row whose branch names no proof: only the branch is wrong.
    let unnamed = set(&trade, &[
        ("open_after_b", json!("filled")),
        ("stop_trade_id", Value::Null),
    ]);
    let tiny = json!("1e-29");
    // Each case breaks one predicate (OVER is one past ledger_amount_fits;
    // g_b >= 0 follows from the kind's own rule and has no case of its own).
    let cases = [
        (with(&trade, "tenant_id", json!(NIL)), "23514", "boundary_carried_legs_ids"),
        (with(&trade, "position_id", json!(NIL)), "23514", "boundary_carried_legs_ids"),
        (
            with(&trade, "stop_order_id", json!("0123")),
            "23514",
            "boundary_carried_legs_ids",
        ),
        (with(&trade, "stop_trade_id", json!("-4")), "23514", "boundary_carried_legs_ids"),
        (
            with(&cancel, "cancel_intent_id", json!(NIL)),
            "23514",
            "boundary_carried_legs_ids",
        ),
        (
            with(&lookup, "lookup_clock_sample_id", json!(NIL)),
            "23514",
            "boundary_carried_legs_ids",
        ),
        (
            with(&trade, "cohort_month", json!(MONTH)),
            "23514",
            "boundary_carried_legs_months",
        ),
        (
            with(&trade, "cohort_month", json!("2026-11-02")),
            "23514",
            "boundary_carried_legs_months",
        ),
        (with(&trade, "kind", json!("dust")), "23514", "boundary_carried_legs_kind"),
        (with(&trade, "g_b", tiny.clone()), "23514", "boundary_carried_legs_amounts"),
        (with(&trade, "vwap", tiny.clone()), "23514", "boundary_carried_legs_amounts"),
        (
            with(&trade, "trigger_b", tiny.clone()),
            "23514",
            "boundary_carried_legs_amounts",
        ),
        (with(&trade, "gap_bps", tiny.clone()), "23514", "boundary_carried_legs_amounts"),
        (with(&trade, "fee_bps", tiny.clone()), "23514", "boundary_carried_legs_amounts"),
        (
            with(&trade, "reserve_qty", tiny.clone()),
            "23514",
            "boundary_carried_legs_amounts",
        ),
        (
            with(&trade, "admission_charge", tiny.clone()),
            "23514",
            "boundary_carried_legs_amounts",
        ),
        (
            with(&trade, "adverse_fill_b", tiny.clone()),
            "23514",
            "boundary_carried_legs_amounts",
        ),
        (
            with(&trade, "floor_price_b", tiny.clone()),
            "23514",
            "boundary_carried_legs_amounts",
        ),
        (
            set(&trade, &[
                ("floor_quote", json!(OVER)),
                ("mark_value", json!(MAX)),
                ("downside", json!("-1")),
            ]),
            "23514",
            "boundary_carried_legs_amounts",
        ),
        (
            set(&trade, &[
                ("mark_value", json!(OVER)),
                ("floor_quote", json!(MAX)),
                ("downside", json!("1")),
            ]),
            "23514",
            "boundary_carried_legs_amounts",
        ),
        (
            set(&flight, &[
                ("admission_charge", json!("1")),
                ("floor_quote", json!("-1")),
                ("mark_value", json!(MAX)),
                ("downside", json!(OVER)),
            ]),
            "23514",
            "boundary_carried_legs_amounts",
        ),
        (
            with(&trade, "prior_month_excess", tiny.clone()),
            "23514",
            "boundary_carried_legs_amounts",
        ),
        (with(&trade, "vwap", json!("0")), "23514", "boundary_carried_legs_terms"),
        (with(&trade, "trigger_b", json!("0")), "23514", "boundary_carried_legs_terms"),
        (with(&trade, "gap_bps", json!("-1")), "23514", "boundary_carried_legs_terms"),
        (with(&trade, "gap_bps", json!("10000")), "23514", "boundary_carried_legs_terms"),
        (with(&trade, "fee_bps", json!("-1")), "23514", "boundary_carried_legs_terms"),
        (with(&trade, "fee_bps", json!("10000")), "23514", "boundary_carried_legs_terms"),
        (
            with(&trade, "reserve_qty", json!("-0.001")),
            "23514",
            "boundary_carried_legs_terms",
        ),
        (
            with(&trade, "admission_charge", json!("0")),
            "23514",
            "boundary_carried_legs_terms",
        ),
        (
            with(&trade, "adverse_fill_b", json!("0")),
            "23514",
            "boundary_carried_legs_terms",
        ),
        (
            with(&trade, "floor_price_b", json!("0")),
            "23514",
            "boundary_carried_legs_terms",
        ),
        (
            set(&trade, &[
                ("mark_value", json!("-1")),
                ("downside", json!("-589.82059")),
            ]),
            "23514",
            "boundary_carried_legs_terms",
        ),
        (
            with(&trade, "prior_month_excess", json!("-0.01")),
            "23514",
            "boundary_carried_legs_terms",
        ),
        // an inventory leg carries something, never floors below 0, and an
        // unprotected one floors at 0
        (with(&trade, "g_b", json!("0")), "23514", "boundary_carried_legs_inventory"),
        (
            set(&trade, &[("floor_quote", json!("-1")), ("downside", json!("601"))]),
            "23514",
            "boundary_carried_legs_inventory",
        ),
        (
            set(&bare, &[("floor_quote", json!("1")), ("downside", json!("599"))]),
            "23514",
            "boundary_carried_legs_inventory",
        ),
        (
            with(&trade, "downside", json!("11.17942")),
            "23514",
            "boundary_carried_legs_downside",
        ),
        // protected iff the stop, trigger, branch, adverse fill and floor price are present
        (
            with(&trade, "stop_order_id", Value::Null),
            "23514",
            "boundary_carried_legs_protection",
        ),
        (
            with(&trade, "trigger_b", Value::Null),
            "23514",
            "boundary_carried_legs_protection",
        ),
        (
            with(&trade, "adverse_fill_b", Value::Null),
            "23514",
            "boundary_carried_legs_protection",
        ),
        (
            with(&trade, "floor_price_b", Value::Null),
            "23514",
            "boundary_carried_legs_protection",
        ),
        (
            with(&bare, "trigger_b", json!("59000")),
            "23514",
            "boundary_carried_legs_protection",
        ),
        (unnamed, "23514", "boundary_carried_legs_open_after_b"),
        // (a): acknowledged by B - 2 s with a valid verdict, and only then
        (
            with(&trade, "stop_live_seen_at", json!(at(-1_999))),
            "23514",
            "boundary_carried_legs_acknowledged",
        ),
        (
            with(&trade, "stop_live_clock_verdict", json!("invalid")),
            "23514",
            "boundary_carried_legs_acknowledged",
        ),
        (
            with(&trade, "stop_live_clock_verdict", Value::Null),
            "23514",
            "boundary_carried_legs_acknowledged",
        ),
        (
            with(&bare, "stop_live_seen_at", json!(at(-10_000))),
            "23514",
            "boundary_carried_legs_acknowledged",
        ),
        // (b1): sent at or after B + 2 s with valid evidence, answered NEW
        (
            with(&lookup, "lookup_sent_at", json!(at(1_999))),
            "23514",
            "boundary_carried_legs_lookup",
        ),
        (
            with(&lookup, "lookup_clock_verdict", json!("invalid")),
            "23514",
            "boundary_carried_legs_lookup",
        ),
        (
            with(&lookup, "lookup_status", json!("FILLED")),
            "23514",
            "boundary_carried_legs_lookup",
        ),
        (
            with(&lookup, "lookup_clock_sample_id", Value::Null),
            "23514",
            "boundary_carried_legs_lookup",
        ),
        (
            with(&trade, "lookup_status", json!("NEW")),
            "23514",
            "boundary_carried_legs_lookup",
        ),
        // (b2): the trade id, and only for that branch
        (
            with(&trade, "stop_trade_id", Value::Null),
            "23514",
            "boundary_carried_legs_trade",
        ),
        (
            with(&lookup, "stop_trade_id", json!("456")),
            "23514",
            "boundary_carried_legs_trade",
        ),
        // (b3): Robson's cancel attempted at or after B + 2 s with a valid verdict
        (
            with(&cancel, "cancel_attempt_at", json!(at(1_999))),
            "23514",
            "boundary_carried_legs_cancel",
        ),
        (
            with(&cancel, "cancel_attempt_clock_verdict", json!("invalid")),
            "23514",
            "boundary_carried_legs_cancel",
        ),
        (
            with(&cancel, "cancel_intent_id", Value::Null),
            "23514",
            "boundary_carried_legs_cancel",
        ),
        (
            with(&trade, "cancel_attempt_at", json!(at(70_000))),
            "23514",
            "boundary_carried_legs_cancel",
        ),
        (
            with(&trade, "exclusions", json!({"order": 1})),
            "23514",
            "boundary_carried_legs_exclusions",
        ),
        (
            with(&trade, "exclusions", json!(too_many)),
            "23514",
            "boundary_carried_legs_exclusions",
        ),
        (with(&trade, "revision", json!(2)), "23503", "boundary_carried_legs_revision"),
        (with(&trade, "symbol", json!("ETHUSDT")), "23503", "boundary_carried_legs_leg"),
        (
            with(&lookup, "lookup_clock_sample_id", json!(other.sample)),
            "23503",
            "boundary_carried_legs_lookup_sample",
        ),
    ];
    for (row, code, needle) in cases {
        refused(probe(&pool, "boundary_carried_legs", None, &row).await, code, needle)?;
    }
    insert(&pool, "boundary_carried_legs", &trade).await?;
    refused(
        insert(&pool, "boundary_carried_legs", &lookup).await,
        "23505",
        "boundary_carried_legs_key",
    )?;
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn boundary_revisions_and_legs_are_immutable(pool: PgPool) -> Result<()> {
    install(&pool).await?;
    let c = carry(&pool).await?;
    insert(&pool, "boundary_carried_legs", &leg_row(&c, "new_at_completion")).await?;
    for change in [
        "UPDATE boundary_snapshot_revisions SET carried_count=carried_count WHERE tenant_id=$1",
        "UPDATE boundary_snapshot_revisions SET basis=basis-1, floor_total=floor_total-1 WHERE tenant_id=$1",
        "UPDATE boundary_carried_legs SET floor_quote=floor_quote WHERE tenant_id=$1",
        "UPDATE boundary_carried_legs SET g_b=g_b+1 WHERE tenant_id=$1",
        "DELETE FROM boundary_carried_legs WHERE tenant_id=$1",
        "DELETE FROM boundary_snapshot_revisions WHERE tenant_id=$1",
    ] {
        refused(exec(&pool, change, c.s.tenant).await, "23514", IMMUTABLE)?;
    }
    let rows: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM boundary_snapshot_revisions WHERE tenant_id=$1),
                (SELECT count(*) FROM boundary_carried_legs WHERE tenant_id=$1)",
    )
    .bind(c.s.tenant)
    .fetch_one(&pool)
    .await?;
    ensure!(rows == (1, 1));
    Ok(())
}

/// A pending cutover anchor of `s` (8m's tx2b), whose cutover_id is returned.
async fn cutover(pool: &PgPool, s: Scope) -> Result<Uuid> {
    let cutover = Uuid::now_v7();
    insert(
        pool,
        "cash_anchors",
        &anchor_row(s, Uuid::now_v7(), "cutover", Some(cutover), None),
    )
    .await?;
    Ok(cutover)
}

fn activation_row(s: Scope, cutover: Uuid, previous: &str, quote: &str, basis: &str) -> Value {
    json!({
        "tenant_id": s.tenant, "venue_id": VENUE, "cutover_id": cutover, "month": COHORT,
        "previous_basis": previous, "quote_total": quote, "basis": basis, "state": "pending",
        "created_at": ts(0),
    })
}

/// 8m's completion: certified, as_of set from NULL to the anchor's cut.
fn certify(as_of: &str) -> String {
    format!(
        "UPDATE activation_bases SET state='certified', certified_at=now(), as_of='{as_of}' WHERE cutover_id=$1"
    )
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn activation_basis_is_min_of_previous_and_quote(pool: PgPool) -> Result<()> {
    install(&pool).await?;
    let s = scope(&pool).await?;
    for (previous, quote, basis) in [
        ("1500", "2000", "1500"),
        ("2000", "1500", "1500"),
        ("0", "2000", "0"),
    ] {
        let id = cutover(&pool, s).await?;
        insert(&pool, "activation_bases", &activation_row(s, id, previous, quote, basis)).await?;
    }
    for (previous, quote, basis) in [
        ("1500", "2000", "2000"),
        ("0", "2000", "2000"),
        ("2000", "1500", "1499.99"),
    ] {
        let id = cutover(&pool, s).await?;
        refused(
            insert(&pool, "activation_bases", &activation_row(s, id, previous, quote, basis)).await,
            "23514",
            "activation_bases_basis",
        )?;
    }
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn activation_bases_certify_at_most_one_per_venue(pool: PgPool) -> Result<()> {
    install(&pool).await?;
    let s = scope(&pool).await?;
    let (first, second) = (cutover(&pool, s).await?, cutover(&pool, s).await?);
    for id in [first, second] {
        insert(&pool, "activation_bases", &activation_row(s, id, "0", "0", "0")).await?;
    }
    ensure!(exec(&pool, &certify(&ts(1_000)), first).await? == 1);
    refused(
        exec(&pool, &certify(&ts(2_000)), second).await,
        "23505",
        "idx_activation_bases_certified",
    )?;
    // Another venue row certifies its own.
    let other = scope(&pool).await?;
    let theirs = cutover(&pool, other).await?;
    insert(&pool, "activation_bases", &activation_row(other, theirs, "0", "0", "0")).await?;
    ensure!(exec(&pool, &certify(&ts(1_000)), theirs).await? == 1);
    let certified: i64 =
        sqlx::query_scalar("SELECT count(*) FROM activation_bases WHERE state='certified'")
            .fetch_one(&pool)
            .await?;
    ensure!(certified == 2);
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn activation_basis_moves_forward_once(pool: PgPool) -> Result<()> {
    install(&pool).await?;
    let s = scope(&pool).await?;
    let id = cutover(&pool, s).await?;
    let row = activation_row(s, id, "1500", "2000", "1500");
    let certified = set(&row, &[
        ("state", json!("certified")),
        ("as_of", json!(ts(1_000))),
        ("certified_at", json!(ts(1_000))),
    ]);
    let guard = Some("activation_bases_move_forward");
    for (row, code, needle) in [
        (with(&row, "cutover_id", json!(NIL)), "23514", "activation_bases_ids"),
        (with(&row, "tenant_id", json!(NIL)), "23514", "activation_bases_ids"),
        (with(&row, "month", json!("2026-11-02")), "23514", "activation_bases_month"),
        // An infinite month has no day (EXTRACT is NULL): refused, not passed
        // as unknown, even pending with no as_of to bound it.
        (with(&row, "month", json!("infinity")), "23514", "activation_bases_month"),
        (with(&row, "month", json!("-infinity")), "23514", "activation_bases_month"),
        (
            set(&row, &[("previous_basis", json!("-1")), ("basis", json!("-1"))]),
            "23514",
            "activation_bases_amounts",
        ),
        (set(&row, &[("quote_total", json!("NaN"))]), "23514", "activation_bases_amounts"),
        (
            set(&row, &[("quote_total", json!("-1")), ("basis", json!("-1"))]),
            "23514",
            "activation_bases_amounts",
        ),
        // basis fits whenever both of its terms do, so it has no case of its own
        (
            set(&row, &[
                ("previous_basis", json!("Infinity")),
                ("basis", json!("2000")),
            ]),
            "23514",
            "activation_bases_amounts",
        ),
        (
            with(&row, "cutover_id", json!(Uuid::now_v7())),
            "23503",
            "activation_bases_anchor",
        ),
    ] {
        refused(probe(&pool, "activation_bases", None, &row).await, code, needle)?;
    }
    // The CHECKs alone, past the guard.
    probe(&pool, "activation_bases", guard, &certified).await?;
    for (row, needle) in [
        (
            set(&certified, &[("state", json!("void")), ("certified_at", Value::Null)]),
            "activation_bases_state",
        ),
        (with(&certified, "certified_at", Value::Null), "activation_bases_certified"),
        (with(&certified, "as_of", Value::Null), "activation_bases_certified"),
        (with(&row, "certified_at", json!(ts(1_000))), "activation_bases_certified"),
        (
            with(&certified, "as_of", json!("2026-10-31 23:59:59.999999+00")),
            "activation_bases_as_of",
        ),
        (
            with(&certified, "as_of", json!("2026-12-01 00:00:00+00")),
            "activation_bases_as_of",
        ),
    ] {
        refused(probe(&pool, "activation_bases", guard, &row).await, "23514", needle)?;
    }
    probe(
        &pool,
        "activation_bases",
        guard,
        &with(&certified, "as_of", json!("2026-11-01 00:00:00+00")),
    )
    .await?;
    // A basis starts pending, with no as_of.
    for born in [certified.clone(), with(&row, "as_of", json!(ts(1_000)))] {
        refused(probe(&pool, "activation_bases", None, &born).await, "23514", "starts pending")?;
    }
    insert(&pool, "activation_bases", &row).await?;
    refused(insert(&pool, "activation_bases", &row).await, "23505", "activation_bases_key")?;
    for fixed in [
        "UPDATE activation_bases SET basis=basis-1, previous_basis=previous_basis-1 WHERE cutover_id=$1",
        "UPDATE activation_bases SET quote_total=quote_total+1 WHERE cutover_id=$1",
        "UPDATE activation_bases SET month='2026-10-01' WHERE cutover_id=$1",
        "UPDATE activation_bases SET created_at=created_at+interval '1 second' WHERE cutover_id=$1",
    ] {
        refused(exec(&pool, fixed, id).await, "23514", FORWARD)?;
    }
    // as_of is set once, from NULL; pending -> certified only; then nothing moves.
    let set_as_of =
        format!("UPDATE activation_bases SET as_of='{}' WHERE cutover_id=$1", ts(1_000));
    ensure!(exec(&pool, &set_as_of, id).await? == 1);
    refused(
        exec(
            &pool,
            &format!("UPDATE activation_bases SET as_of='{}' WHERE cutover_id=$1", ts(2_000)),
            id,
        )
        .await,
        "23514",
        FORWARD,
    )?;
    ensure!(exec(&pool, &certify(&ts(1_000)), id).await? == 1);
    for moved in [
        "UPDATE activation_bases SET state='pending', certified_at=NULL WHERE cutover_id=$1",
        "UPDATE activation_bases SET certified_at=certified_at+interval '1 second' WHERE cutover_id=$1",
        "UPDATE activation_bases SET as_of=NULL WHERE cutover_id=$1",
    ] {
        refused(exec(&pool, moved, id).await, "23514", FORWARD)?;
    }
    refused(
        exec(&pool, "DELETE FROM activation_bases WHERE cutover_id=$1", id).await,
        "23514",
        "never deleted",
    )?;
    // An update that changes nothing is no move.
    ensure!(
        exec(&pool, "UPDATE activation_bases SET state=state WHERE cutover_id=$1", id).await? == 1
    );
    Ok(())
}

const TABLES: [&str; 5] = [
    "boundary_snapshots",
    "boundary_reopenings",
    "boundary_snapshot_revisions",
    "boundary_carried_legs",
    "activation_bases",
];

/// (name, type, NOT NULL) of each column, in order.
fn columns(table: &str) -> Value {
    const T: &str = "text";
    const TZ: &str = "timestamp with time zone";
    const B8: &str = "bigint";
    const I4: &str = "integer";
    const N: &str = "numeric";
    const U: &str = "uuid";
    const D: &str = "date";
    const J: &str = "jsonb";
    let scoped = [
        ("tenant_id", U, true),
        ("venue_id", T, true),
        ("month", D, true),
    ];
    let list: Vec<(&str, &str, bool)> = match table {
        "boundary_snapshots" => [&scoped[..], &[
            ("as_of", TZ, true),
            ("state", T, true),
            ("revision", I4, true),
            ("reopen_count", I4, true),
            ("effective_basis", N, false),
            ("reset_revision", I4, false),
            ("reset_basis", N, false),
            ("published_revision", I4, false),
            ("published_basis", N, false),
            ("pending_since", TZ, true),
            ("completed_at", TZ, false),
            ("corrected_at", TZ, false),
            ("closed_by_correction_at", TZ, false),
        ]]
        .concat(),
        "boundary_reopenings" => [&scoped[..], &[
            ("reopen_seq", I4, true),
            ("from_revision", I4, true),
            ("detected_at", TZ, true),
            ("cause", T, true),
            ("revision_d_fills", B8, true),
            ("revision_d_flows", B8, true),
            ("revision_d_legs", B8, true),
            ("observed_d_fills", B8, true),
            ("observed_d_flows", B8, true),
            ("observed_d_legs", B8, true),
            ("evidence_ids", J, true),
        ]]
        .concat(),
        "boundary_snapshot_revisions" => [&scoped[..], &[
            ("revision", I4, true),
            ("as_of", TZ, true),
            ("anchor_id", U, true),
            ("anchor_as_of", TZ, true),
            ("quote_b", N, true),
            ("mark_price", N, true),
            ("mark_kline_open_time", TZ, true),
            ("mark_kline_close_time", TZ, true),
            ("floor_total", N, true),
            ("downside_total", N, true),
            ("basis", N, true),
            ("carried_count", I4, true),
            ("budget_model", T, true),
            ("completed_version", B8, true),
            ("evidence_hash", T, true),
            ("d_fills", B8, true),
            ("d_flows", B8, true),
            ("d_legs", B8, true),
            ("reason", T, true),
            ("reopen_seq", I4, false),
            ("computed_at", TZ, true),
            ("computed_clock_sample_id", U, true),
            ("computed_clock_wall_delta_ms", B8, true),
            ("computed_clock_mono_delta_ms", B8, true),
            ("computed_clock_verdict", T, true),
        ]]
        .concat(),
        "boundary_carried_legs" => [&scoped[..], &[
            ("revision", I4, true),
            ("position_id", U, true),
            ("account_scope", T, true),
            ("symbol", T, true),
            ("cohort_month", D, true),
            ("kind", T, true),
            ("g_b", N, true),
            ("vwap", N, false),
            ("protected_at_b", "boolean", true),
            ("stop_order_id", T, false),
            ("trigger_b", N, false),
            ("open_after_b", T, false),
            ("stop_live_seen_at", TZ, false),
            ("stop_live_clock_verdict", T, false),
            ("lookup_sent_at", TZ, false),
            ("lookup_clock_sample_id", U, false),
            ("lookup_clock_verdict", T, false),
            ("lookup_status", T, false),
            ("stop_trade_id", T, false),
            ("cancel_intent_id", U, false),
            ("cancel_attempt_at", TZ, false),
            ("cancel_attempt_clock_verdict", T, false),
            ("exclusions", J, true),
            ("gap_bps", N, true),
            ("fee_bps", N, true),
            ("reserve_qty", N, true),
            ("admission_charge", N, true),
            ("adverse_fill_b", N, false),
            ("floor_price_b", N, false),
            ("floor_quote", N, true),
            ("mark_value", N, true),
            ("downside", N, true),
            ("prior_month_excess", N, false),
        ]]
        .concat(),
        "activation_bases" => Vec::from([
            ("tenant_id", U, true),
            ("venue_id", T, true),
            ("cutover_id", U, true),
            ("month", D, true),
            ("previous_basis", N, true),
            ("quote_total", N, true),
            ("basis", N, true),
            ("as_of", TZ, false),
            ("state", T, true),
            ("created_at", TZ, true),
            ("certified_at", TZ, false),
        ]),
        other => panic!("unexpected table {other}"),
    };
    Value::Array(
        list.into_iter()
            .map(|(name, kind, required)| json!([name, kind, required]))
            .collect(),
    )
}

fn constraint_names(table: &str) -> Vec<String> {
    let names: &[&str] = match table {
        "boundary_snapshots" => &[
            "key p",
            "version_row f",
            "ids c",
            "month c",
            "as_of c",
            "state c",
            "revision c",
            "amounts c",
            "publication c",
            "correction c",
        ],
        "boundary_reopenings" => &[
            "key p",
            "snapshot f",
            "version_row f",
            "ids c",
            "seq c",
            "cause c",
            "digests c",
            "changed c",
            "evidence c",
        ],
        "boundary_snapshot_revisions" => &[
            "key p",
            "snapshot f",
            "reopening f",
            "version_row f",
            "anchor f",
            "computed_sample f",
            "ids c",
            "as_of c",
            "mark c",
            "amounts c",
            "basis c",
            "counts c",
            "budget_model c",
            "evidence_hash c",
            "reason c",
            "computed c",
        ],
        "boundary_carried_legs" => &[
            "key p",
            "revision f",
            "leg f",
            "version_row f",
            "lookup_sample f",
            "ids c",
            "months c",
            "kind c",
            "amounts c",
            "terms c",
            "in_flight c",
            "inventory c",
            "downside c",
            "protection c",
            "open_after_b c",
            "acknowledged c",
            "lookup c",
            "trade c",
            "cancel c",
            "exclusions c",
        ],
        "activation_bases" => &[
            "key p",
            "version_row f",
            "anchor f",
            "ids c",
            "month c",
            "amounts c",
            "basis c",
            "as_of c",
            "state c",
            "certified c",
        ],
        other => panic!("unexpected table {other}"),
    };
    let mut names: Vec<String> = names.iter().map(|name| format!("{table}_{name}")).collect();
    names.sort();
    names
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn migrations51_to_54_install_the_reviewed_catalog(pool: PgPool) -> Result<()> {
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
        let mut constraints: Vec<String> = sqlx::query_scalar(
            "SELECT conname||' '||contype::text FROM pg_constraint WHERE conrelid=to_regclass('public.'||$1)",
        )
        .bind(table)
        .fetch_all(&pool)
        .await?;
        constraints.sort();
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
        ("activation_bases_anchor", "FOREIGN KEY (tenant_id, venue_id, cutover_id) REFERENCES cash_anchors(tenant_id, venue_id, cutover_id)"),
        ("activation_bases_key", "PRIMARY KEY (tenant_id, venue_id, cutover_id)"),
        ("activation_bases_version_row", "FOREIGN KEY (tenant_id, venue_id) REFERENCES accounting_state(tenant_id, venue_id)"),
        ("boundary_carried_legs_key", "PRIMARY KEY (tenant_id, venue_id, month, revision, position_id)"),
        ("boundary_carried_legs_leg", "FOREIGN KEY (position_id, tenant_id, venue_id, account_scope, symbol) REFERENCES balance_legs(position_id, tenant_id, venue_id, account_scope, symbol)"),
        ("boundary_carried_legs_lookup_sample", "FOREIGN KEY (lookup_clock_sample_id, tenant_id, venue_id) REFERENCES clock_samples(sample_id, tenant_id, venue_id)"),
        ("boundary_carried_legs_revision", "FOREIGN KEY (tenant_id, venue_id, month, revision) REFERENCES boundary_snapshot_revisions(tenant_id, venue_id, month, revision)"),
        ("boundary_carried_legs_version_row", "FOREIGN KEY (tenant_id, venue_id) REFERENCES accounting_state(tenant_id, venue_id)"),
        ("boundary_reopenings_key", "PRIMARY KEY (tenant_id, venue_id, month, reopen_seq)"),
        ("boundary_reopenings_snapshot", "FOREIGN KEY (tenant_id, venue_id, month) REFERENCES boundary_snapshots(tenant_id, venue_id, month)"),
        ("boundary_reopenings_version_row", "FOREIGN KEY (tenant_id, venue_id) REFERENCES accounting_state(tenant_id, venue_id)"),
        ("boundary_snapshot_revisions_anchor", "FOREIGN KEY (anchor_id, tenant_id, venue_id) REFERENCES cash_anchors(anchor_id, tenant_id, venue_id)"),
        ("boundary_snapshot_revisions_computed_sample", "FOREIGN KEY (computed_clock_sample_id, tenant_id, venue_id) REFERENCES clock_samples(sample_id, tenant_id, venue_id)"),
        ("boundary_snapshot_revisions_key", "PRIMARY KEY (tenant_id, venue_id, month, revision)"),
        ("boundary_snapshot_revisions_reopening", "FOREIGN KEY (tenant_id, venue_id, month, reopen_seq) REFERENCES boundary_reopenings(tenant_id, venue_id, month, reopen_seq)"),
        ("boundary_snapshot_revisions_snapshot", "FOREIGN KEY (tenant_id, venue_id, month) REFERENCES boundary_snapshots(tenant_id, venue_id, month)"),
        ("boundary_snapshot_revisions_version_row", "FOREIGN KEY (tenant_id, venue_id) REFERENCES accounting_state(tenant_id, venue_id)"),
        ("boundary_snapshots_key", "PRIMARY KEY (tenant_id, venue_id, month)"),
        ("boundary_snapshots_version_row", "FOREIGN KEY (tenant_id, venue_id) REFERENCES accounting_state(tenant_id, venue_id)"),
    ];
    let mut keys: Vec<(String, String)> = sqlx::query_as(
        "SELECT conname::text,pg_get_constraintdef(oid) FROM pg_constraint
         WHERE contype IN ('p','u','f')
           AND conrelid=ANY(ARRAY(SELECT to_regclass('public.'||t)::oid FROM unnest($1::text[]) t))",
    )
    .bind(&TABLES[..])
    .fetch_all(&pool)
    .await?;
    keys.sort();
    let expected: Vec<(String, String)> =
        KEYS.iter().map(|(name, def)| (name.to_string(), def.to_string())).collect();
    ensure!(keys == expected, "keys and foreign keys: {keys:#?}");
    let mut triggers: Vec<(String, String, String, i16, String)> = sqlx::query_as(
        "SELECT tgrelid::regclass::text,tgname::text,tgfoid::regprocedure::text,tgtype,tgenabled::text
         FROM pg_trigger WHERE NOT tgisinternal
           AND tgrelid=ANY(ARRAY(SELECT to_regclass('public.'||t)::oid FROM unnest($1::text[]) t))",
    )
    .bind(&TABLES[..])
    .fetch_all(&pool)
    .await?;
    triggers.sort();
    // 27 = ROW | BEFORE | DELETE | UPDATE; 31 adds INSERT.
    let expected: Vec<(String, String, String, i16, String)> = [
        (
            "activation_bases",
            "activation_bases_move_forward",
            "activation_basis_moves_forward()",
            31,
        ),
        (
            "boundary_carried_legs",
            "boundary_carried_legs_are_immutable",
            "ledger_row_is_immutable()",
            27,
        ),
        (
            "boundary_reopenings",
            "boundary_reopenings_are_immutable",
            "ledger_row_is_immutable()",
            27,
        ),
        (
            "boundary_snapshot_revisions",
            "boundary_snapshot_revisions_are_immutable",
            "ledger_row_is_immutable()",
            27,
        ),
        (
            "boundary_snapshots",
            "boundary_snapshots_move_forward",
            "boundary_snapshot_moves_forward()",
            31,
        ),
    ]
    .into_iter()
    .map(|(t, n, f, k)| (t.to_owned(), n.to_owned(), f.to_owned(), k, "O".to_owned()))
    .collect();
    ensure!(triggers == expected, "triggers: {triggers:?}");
    let guards: Value = sqlx::query_scalar(
        "SELECT jsonb_agg(jsonb_build_array(p.oid::regprocedure::text,pg_get_function_result(p.oid),l.lanname,p.proconfig)
                 ORDER BY p.oid::regprocedure::text COLLATE \"C\")
         FROM pg_proc p JOIN pg_language l ON l.oid=p.prolang
         WHERE p.oid IN (to_regprocedure('public.boundary_snapshot_moves_forward()'),
                         to_regprocedure('public.activation_basis_moves_forward()'))",
    )
    .fetch_one(&pool)
    .await?;
    ensure!(
        guards
            == json!([
                ["activation_basis_moves_forward()", "trigger", "plpgsql", [
                    "search_path=pg_catalog, public"
                ]],
                ["boundary_snapshot_moves_forward()", "trigger", "plpgsql", [
                    "search_path=pg_catalog, public"
                ]]
            ]),
        "guards: {guards}"
    );
    let certified: Value = sqlx::query_scalar(
        "SELECT jsonb_build_array(indisunique,indisvalid,pg_get_indexdef(indexrelid,1,false),
                pg_get_indexdef(indexrelid,2,false),pg_get_expr(indpred,indrelid))
         FROM pg_index WHERE indexrelid=to_regclass('public.idx_activation_bases_certified')",
    )
    .fetch_one(&pool)
    .await?;
    ensure!(
        certified
            == json!([
                true,
                true,
                "tenant_id",
                "venue_id",
                "(state = 'certified'::text)"
            ]),
        "certified index: {certified}"
    );
    Ok(())
}
