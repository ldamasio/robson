//! Every CHECK, FOREIGN KEY, UNIQUE and trigger rule of S8-6 (8m PLAN-r4
//! §4.3), on the schema installed through the bounded route. Rows are JSON
//! inserted with `jsonb_populate_record`, so each refusal case changes one
//! field and breaks exactly one CHECK conjunct or one key. A BEFORE ROW
//! trigger runs before the CHECKs, and the CHECKs before any foreign key (RI
//! triggers fire at the end of the statement). A run is born pending, so a
//! CHECK that needs another state is probed by the UPDATE that reaches it,
//! which the guard lets through from pending. A foreign key that another key
//! implies (the activation basis names its run through its anchor) is probed
//! with that other key dropped in a rolled-back transaction. The run's
//! lifecycle is driven by the statements its writers (8m-4's tx1, 8m-5's
//! completion, the abandonment) will issue, in miniature.
use serde_json::{json, Value};
use uuid::Uuid;

use super::{
    super::constraints::{anchor_row, exec, insert, refused, scope, set, with, Scope, NIL, VENUE},
    *,
};

/// Month K = December 2026 (C10-Z's cutover month).
const MONTH: &str = "2026-12-01";
const MODEL: &str = "net_from_start_non_expanding_v1";
const FORWARD: &str = "only moves forward";
/// The largest amount ledger_amount_fits accepts (2^96 - 1), and one past it.
const MAX: &str = "79228162514264337593543950335";
const OVER: &str = "79228162514264337593543950336";
/// A quote with ten decimals: kept at the ledger's precision (§6.11.6).
const QUOTE: &str = "2100.1234567891";
const AT: &str = "2026-12-15 12:05:00+00";
/// Completion's write of its run (8m-5) and an abandonment's.
const COMPLETE: &str =
    "UPDATE venue_cutovers SET state='complete', completed_at='2026-12-15 12:05:00+00',
    observations='{\"o1\":{\"uid\":\"1234567\"},\"o2\":{\"uid\":\"1234567\"}}' WHERE cutover_id=$1";
const ABANDON: &str =
    "UPDATE venue_cutovers SET state='abandoned', abandoned_at='2026-12-15 12:05:00+00',
    abandon_reason='operator' WHERE cutover_id=$1";
const REASONS: [&str; 11] = [
    "check_failed",
    "report_changed",
    "generation_changed",
    "observation_failed",
    "observations_disagree",
    "clock_invalid",
    "discrepancy_open",
    "month_changed",
    "version_mismatch",
    "timeout",
    "operator",
];

/// The production route through 56. The objects themselves are pinned by
/// the catalog test, so each rule's test reaches its own assertion even when
/// that rule's object is missing.
async fn install(pool: &PgPool) -> Result<()> {
    install_through(pool, VERSION56).await
}

fn hash(digit: char) -> String {
    std::iter::repeat_n(digit, 64).collect()
}

/// A run as tx1 inserts it (8m PLAN-r4 §6.3 step 4), with C10-Z's decision
/// values: previous 0, a ten-decimal quote, basis 0. Its documents carry one
/// integer each, so a renumbered (equal but rescaled) value is a change.
fn run_row(s: Scope, id: Uuid) -> Value {
    json!({
        "cutover_id": id, "tenant_id": s.tenant, "venue_id": VENUE, "account_scope": "spot-main",
        "state": "pending", "month": MONTH, "uid": "1234567",
        "report": {"checks": {"c1": "pass"}, "weight": 73, "quote_total": QUOTE},
        "report_hash": hash('a'), "generation": {"schema_version": 1, "g4": 12}, "generation_hash": hash('b'),
        "v1": 0, "budget_model": MODEL, "previous_basis": "0", "quote_total": QUOTE, "basis": "0",
        "command_id": Uuid::now_v7(), "epoch_id": s.epoch, "created_at": "2026-12-15 12:00:00+00",
    })
}

/// tx1: a pending run of `s`, whose id is returned.
async fn pending(pool: &PgPool, s: Scope) -> Result<Uuid> {
    let id = Uuid::now_v7();
    insert(pool, "venue_cutovers", &run_row(s, id)).await?;
    Ok(id)
}

async fn run_state(pool: &PgPool, id: Uuid) -> Result<Value> {
    Ok(
        sqlx::query_scalar("SELECT to_jsonb(c) FROM venue_cutovers c WHERE cutover_id=$1")
            .bind(id)
            .fetch_one(pool)
            .await?,
    )
}

/// A second venue row of `s`'s tenant.
async fn second_venue(pool: &PgPool, s: Scope) -> Result<Scope> {
    sqlx::query(
        "INSERT INTO accounting_state(tenant_id,venue_id,budget_model) VALUES($1,'binance:other','hwm_v1')",
    )
    .bind(s.tenant)
    .execute(pool)
    .await?;
    Ok(s)
}

fn other_venue(row: &Value) -> Value {
    with(row, "venue_id", json!("binance:other"))
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn venue_cutover_state_moves_only_forward(pool: PgPool) -> Result<()> {
    install(&pool).await?;
    let s = scope(&pool).await?;
    // Born pending: a complete or an abandoned row, coherent with every
    // CHECK, is refused by the guard.
    let row = run_row(s, Uuid::now_v7());
    for born in [
        set(&row, &[
            ("state", json!("complete")),
            ("completed_at", json!(AT)),
            ("observations", json!({"o1": {}, "o2": {}})),
        ]),
        set(&row, &[
            ("state", json!("abandoned")),
            ("abandoned_at", json!(AT)),
            ("abandon_reason", json!("operator")),
        ]),
    ] {
        refused(insert(&pool, "venue_cutovers", &born).await, "23514", "starts pending")?;
    }
    const SAME: &str = "UPDATE venue_cutovers SET state=state WHERE cutover_id=$1";
    // pending -> complete; an update that changes nothing is no move.
    let done = pending(&pool, s).await?;
    ensure!(exec(&pool, SAME, done).await? == 1);
    ensure!(exec(&pool, COMPLETE, done).await? == 1);
    let complete = run_state(&pool, done).await?;
    ensure!(complete["state"] == "complete" && complete["observations"]["o2"]["uid"] == "1234567");
    // A complete run never moves back or across, and its end never changes.
    for sql in [
        "UPDATE venue_cutovers SET state='pending', completed_at=NULL, observations=NULL WHERE cutover_id=$1",
        "UPDATE venue_cutovers SET state='abandoned', completed_at=NULL, observations=NULL,
            abandoned_at=completed_at, abandon_reason='operator' WHERE cutover_id=$1",
        "UPDATE venue_cutovers SET completed_at=completed_at+interval '1 second' WHERE cutover_id=$1",
        "UPDATE venue_cutovers SET observations=observations||'{\"o3\":{}}' WHERE cutover_id=$1",
        "UPDATE venue_cutovers SET observations='{\"o1\":{\"uid\":\"1234567\"},\"o2\":{\"uid\":\"1234567.0\"}}'
            WHERE cutover_id=$1",
    ] {
        refused(exec(&pool, sql, done).await, "23514", FORWARD)?;
    }
    ensure!(exec(&pool, SAME, done).await? == 1);
    ensure!(run_state(&pool, done).await? == complete);
    // pending -> abandoned, on another venue; an abandoned run never moves
    // back or across, and its end never changes.
    let t = scope(&pool).await?;
    let gone = pending(&pool, t).await?;
    ensure!(exec(&pool, ABANDON, gone).await? == 1);
    let abandoned = run_state(&pool, gone).await?;
    for sql in [
        "UPDATE venue_cutovers SET state='pending', abandoned_at=NULL, abandon_reason=NULL WHERE cutover_id=$1",
        "UPDATE venue_cutovers SET state='complete', abandoned_at=NULL, abandon_reason=NULL,
            completed_at=abandoned_at, observations='{}' WHERE cutover_id=$1",
        "UPDATE venue_cutovers SET abandon_reason='timeout' WHERE cutover_id=$1",
        "UPDATE venue_cutovers SET abandoned_at=abandoned_at+interval '1 second' WHERE cutover_id=$1",
    ] {
        refused(exec(&pool, sql, gone).await, "23514", FORWARD)?;
    }
    ensure!(exec(&pool, SAME, gone).await? == 1);
    ensure!(run_state(&pool, gone).await? == abandoned);
    // No run is ever deleted, whatever its state.
    let open = pending(&pool, t).await?;
    for id in [done, gone, open] {
        refused(
            exec(&pool, "DELETE FROM venue_cutovers WHERE cutover_id=$1", id).await,
            "23514",
            "never deleted",
        )?;
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM venue_cutovers")
        .fetch_one(&pool)
        .await?;
    ensure!(count == 3);
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn venue_cutover_identity_and_generation_are_immutable(pool: PgPool) -> Result<()> {
    install(&pool).await?;
    let (s, t) = (scope(&pool).await?, scope(&pool).await?);
    let open = pending(&pool, s).await?;
    let done = pending(&pool, t).await?;
    ensure!(exec(&pool, COMPLETE, done).await? == 1);
    // Each statement changes one value tx1 wrote and keeps every CHECK and
    // key, so only the guard refuses it: a rescaled amount and a renumbered
    // document field are changes too.
    for id in [open, done] {
        let before = run_state(&pool, id).await?;
        for change in [
            "cutover_id=gen_random_uuid()",
            "tenant_id=gen_random_uuid()",
            "venue_id='binance:other'",
            "account_scope='spot-other'",
            "month='2027-01-01'",
            "uid='7654321'",
            "report=jsonb_set(report,'{checks,c1}','\"fail\"')",
            "report=jsonb_set(report,'{weight}','73.0')",
            "report_hash=repeat('c',64)",
            "generation=jsonb_set(generation,'{g4}','13')",
            "generation=jsonb_set(generation,'{g4}','12.0')",
            "generation=generation||'{\"schema_version\":2}'",
            "generation_hash=repeat('d',64)",
            "v1=v1+1",
            "budget_model='hwm_v1'",
            "previous_basis=1, basis=1",
            "quote_total=quote_total+1",
            "quote_total='2100.12345678910'",
            "basis='0.0'",
            "previous_basis='0.00'",
            "command_id=gen_random_uuid()",
            "epoch_id=gen_random_uuid()",
            "created_at=created_at+interval '1 microsecond'",
        ] {
            let sql = format!("UPDATE venue_cutovers SET {change} WHERE cutover_id=$1");
            refused(exec(&pool, &sql, id).await, "23514", FORWARD).context(change)?;
        }
        ensure!(run_state(&pool, id).await? == before);
    }
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn at_most_one_complete_cutover_per_venue(pool: PgPool) -> Result<()> {
    install(&pool).await?;
    let s = second_venue(&pool, scope(&pool).await?).await?;
    let first = pending(&pool, s).await?;
    ensure!(exec(&pool, COMPLETE, first).await? == 1);
    // A later run of the venue may start, and end abandoned, any number of
    // times; it never completes beside the certified one.
    let second = pending(&pool, s).await?;
    refused(exec(&pool, COMPLETE, second).await, "23505", "idx_venue_cutovers_complete")?;
    ensure!(exec(&pool, ABANDON, second).await? == 1);
    let third = pending(&pool, s).await?;
    ensure!(exec(&pool, ABANDON, third).await? == 1);
    // The tenant's other venue and another tenant certify their own.
    let elsewhere = Uuid::now_v7();
    insert(&pool, "venue_cutovers", &other_venue(&run_row(s, elsewhere))).await?;
    ensure!(exec(&pool, COMPLETE, elsewhere).await? == 1);
    let t = scope(&pool).await?;
    let theirs = pending(&pool, t).await?;
    ensure!(exec(&pool, COMPLETE, theirs).await? == 1);
    let states: Vec<(String, i64)> =
        sqlx::query_as("SELECT state,count(*) FROM venue_cutovers GROUP BY state ORDER BY state")
            .fetch_all(&pool)
            .await?;
    ensure!(
        states == [("abandoned".to_owned(), 2), ("complete".to_owned(), 3)],
        "{states:?}"
    );
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn at_most_one_pending_cutover_per_venue(pool: PgPool) -> Result<()> {
    install(&pool).await?;
    let s = second_venue(&pool, scope(&pool).await?).await?;
    let first = pending(&pool, s).await?;
    refused(
        insert(&pool, "venue_cutovers", &run_row(s, Uuid::now_v7())).await,
        "23505",
        "idx_venue_cutovers_pending",
    )?;
    // The tenant's other venue and another tenant run their own meanwhile.
    insert(&pool, "venue_cutovers", &other_venue(&run_row(s, Uuid::now_v7()))).await?;
    pending(&pool, scope(&pool).await?).await?;
    // Once the run has ended, abandoned or complete, the next one starts.
    ensure!(exec(&pool, ABANDON, first).await? == 1);
    let second = pending(&pool, s).await?;
    refused(
        insert(&pool, "venue_cutovers", &run_row(s, Uuid::now_v7())).await,
        "23505",
        "idx_venue_cutovers_pending",
    )?;
    ensure!(exec(&pool, COMPLETE, second).await? == 1);
    pending(&pool, s).await?;
    let open: i64 = sqlx::query_scalar("SELECT count(*) FROM venue_cutovers WHERE state='pending'")
        .fetch_one(&pool)
        .await?;
    ensure!(open == 3);
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn cutover_basis_is_min_of_previous_and_quote(pool: PgPool) -> Result<()> {
    install(&pool).await?;
    let amounts = |s: Scope, previous: &str, quote: &str, basis: &str| {
        set(&run_row(s, Uuid::now_v7()), &[
            ("previous_basis", json!(previous)),
            ("quote_total", json!(quote)),
            ("basis", json!(basis)),
        ])
    };
    // C10-Z: previous 0 certifies at 0 whatever the quote's scale.
    for (previous, quote, basis) in [
        ("1500", "2000", "1500"),
        ("2000", "1500", "1500"),
        ("2000", "2000", "2000"),
        ("0", QUOTE, "0"),
        ("0", MAX, "0"),
        ("2100.5", QUOTE, QUOTE),
    ] {
        let s = scope(&pool).await?;
        insert(&pool, "venue_cutovers", &amounts(s, previous, quote, basis)).await?;
    }
    for (previous, quote, basis) in [
        ("1500", "2000", "2000"),
        ("0", "2000", "2000"),
        ("2000", "1500", "1499.99"),
        ("2000", "1500", "2000"),
        ("0", QUOTE, QUOTE),
    ] {
        let s = scope(&pool).await?;
        refused(
            insert(&pool, "venue_cutovers", &amounts(s, previous, quote, basis)).await,
            "23514",
            "venue_cutovers_basis",
        )?;
    }
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn venue_cutovers_refuse_incoherent_rows(pool: PgPool) -> Result<()> {
    install(&pool).await?;
    let s = scope(&pool).await?;
    let row = run_row(s, Uuid::now_v7());
    let generation = |value: Value| with(&row, "generation", value);
    // At insert: a nil tenant also lacks its version row, and the CHECK
    // refuses first.
    for (case, needle) in [
        (with(&row, "cutover_id", json!(NIL)), "venue_cutovers_ids"),
        (with(&row, "tenant_id", json!(NIL)), "venue_cutovers_ids"),
        (with(&row, "command_id", json!(NIL)), "venue_cutovers_ids"),
        (with(&row, "epoch_id", json!(NIL)), "venue_cutovers_ids"),
        (with(&row, "account_scope", json!("")), "venue_cutovers_account"),
        (with(&row, "uid", json!("")), "venue_cutovers_account"),
        (with(&row, "month", json!("2026-12-02")), "venue_cutovers_month"),
        (with(&row, "month", json!("infinity")), "venue_cutovers_month"),
        (with(&row, "month", json!("-infinity")), "venue_cutovers_month"),
        (with(&row, "report", json!(["c1"])), "venue_cutovers_report"),
        (with(&row, "report", json!("report")), "venue_cutovers_report"),
        (with(&row, "report_hash", json!(hash('A'))), "venue_cutovers_report"),
        (with(&row, "report_hash", json!(&hash('a')[1..])), "venue_cutovers_report"),
        (
            with(&row, "report_hash", json!(format!("{}g", &hash('a')[1..]))),
            "venue_cutovers_report",
        ),
        (generation(json!({"g4": 12})), "venue_cutovers_generation"),
        (generation(json!([1])), "venue_cutovers_generation"),
        (generation(json!({"schema_version": "1"})), "venue_cutovers_generation"),
        (generation(json!({"schema_version": 0})), "venue_cutovers_generation"),
        (generation(json!({"schema_version": 1.5})), "venue_cutovers_generation"),
        (generation(json!({"schema_version": -1})), "venue_cutovers_generation"),
        (generation(json!({"schema_version": null})), "venue_cutovers_generation"),
        (with(&row, "generation_hash", json!("")), "venue_cutovers_generation"),
        (with(&row, "v1", json!(-1)), "venue_cutovers_v1"),
        (with(&row, "budget_model", json!("hwm_v1")), "venue_cutovers_budget_model"),
        (
            set(&row, &[("previous_basis", json!("-1")), ("basis", json!("-1"))]),
            "venue_cutovers_amounts",
        ),
        (
            set(&row, &[
                ("previous_basis", json!("5")),
                ("quote_total", json!("-1")),
                ("basis", json!("-1")),
            ]),
            "venue_cutovers_amounts",
        ),
        (
            set(&row, &[
                ("previous_basis", json!(OVER)),
                ("quote_total", json!("1")),
                ("basis", json!("1")),
            ]),
            "venue_cutovers_amounts",
        ),
        (with(&row, "quote_total", json!(OVER)), "venue_cutovers_amounts"),
        (with(&row, "quote_total", json!("NaN")), "venue_cutovers_amounts"),
        (
            with(&row, "quote_total", json!("0.00000000000000000000000000001")),
            "venue_cutovers_amounts",
        ),
        (with(&row, "completed_at", json!(AT)), "venue_cutovers_complete"),
        (with(&row, "observations", json!({"o1": {}})), "venue_cutovers_complete"),
        (with(&row, "abandoned_at", json!(AT)), "venue_cutovers_abandoned"),
        (with(&row, "abandon_reason", json!("operator")), "venue_cutovers_abandoned"),
    ] {
        refused(insert(&pool, "venue_cutovers", &case).await, "23514", needle)
            .with_context(|| case.to_string())?;
    }
    // At an update from pending, which the guard lets through: the state's
    // set and each end's columns, whole.
    let id = pending(&pool, s).await?;
    for (sql, needle) in [
        ("state='done'", "venue_cutovers_state"),
        ("state='complete'", "venue_cutovers_complete"),
        ("state='complete', completed_at=now()", "venue_cutovers_complete"),
        ("state='complete', observations='{}'", "venue_cutovers_complete"),
        ("completed_at=now(), observations='{}'", "venue_cutovers_complete"),
        ("state='abandoned'", "venue_cutovers_abandoned"),
        ("state='abandoned', abandoned_at=now()", "venue_cutovers_abandoned"),
        ("state='abandoned', abandon_reason='operator'", "venue_cutovers_abandoned"),
        (
            "state='abandoned', abandoned_at=now(), abandon_reason='later'",
            "venue_cutovers_abandon_reason",
        ),
        (
            "state='complete', completed_at=now(), observations='{}', abandoned_at=now(),
             abandon_reason='operator'",
            "venue_cutovers_abandoned",
        ),
    ] {
        let sql = format!("UPDATE venue_cutovers SET {sql} WHERE cutover_id=$1");
        refused(exec(&pool, &sql, id).await, "23514", needle).with_context(|| sql.clone())?;
    }
    // Every closed-set reason ends a run; the largest amount fits.
    ensure!(exec(&pool, ABANDON, id).await? == 1);
    for reason in REASONS {
        let id = pending(&pool, s).await?;
        let sql = format!(
            "UPDATE venue_cutovers SET state='abandoned', abandoned_at=now(), abandon_reason='{reason}'
             WHERE cutover_id=$1"
        );
        ensure!(exec(&pool, &sql, id).await? == 1, "{reason}");
    }
    let largest = set(&run_row(s, Uuid::now_v7()), &[
        ("previous_basis", json!(MAX)),
        ("quote_total", json!(MAX)),
        ("basis", json!(MAX)),
        ("v1", json!(i64::MAX)),
    ]);
    insert(&pool, "venue_cutovers", &largest).await?;
    Ok(())
}

/// A venue account of `s` bound to `cutover` (8m-5's completion insert).
fn account_row(s: Scope, account_scope: &str, cutover: Uuid) -> Value {
    json!({
        "venue_id": VENUE, "account_scope": account_scope, "tenant_id": s.tenant, "uid": "1234567",
        "bound_at": AT, "cutover_id": cutover,
    })
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn venue_account_names_an_existing_cutover(pool: PgPool) -> Result<()> {
    install(&pool).await?;
    let s = second_venue(&pool, scope(&pool).await?).await?;
    let t = scope(&pool).await?;
    let run = pending(&pool, s).await?;
    let theirs = pending(&pool, t).await?;
    let elsewhere = Uuid::now_v7();
    insert(&pool, "venue_cutovers", &other_venue(&run_row(s, elsewhere))).await?;
    // No run, another tenant's run, and the tenant's run of another venue.
    for cutover in [Uuid::now_v7(), theirs, elsewhere] {
        refused(
            insert(&pool, "venue_accounts", &account_row(s, "spot-main", cutover)).await,
            "23503",
            "venue_accounts_venue_cutover",
        )?;
    }
    insert(&pool, "venue_accounts", &account_row(s, "spot-main", run)).await?;
    let bound: Vec<Uuid> = sqlx::query_scalar("SELECT cutover_id FROM venue_accounts")
        .fetch_all(&pool)
        .await?;
    ensure!(bound == [run]);
    Ok(())
}

fn activation_row(s: Scope, cutover: Uuid) -> Value {
    json!({
        "tenant_id": s.tenant, "venue_id": VENUE, "cutover_id": cutover, "month": MONTH,
        "previous_basis": "0", "quote_total": QUOTE, "basis": "0", "state": "pending",
        "created_at": AT,
    })
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn anchors_and_activation_bases_name_an_existing_cutover(pool: PgPool) -> Result<()> {
    install(&pool).await?;
    let s = scope(&pool).await?;
    let t = scope(&pool).await?;
    let run = pending(&pool, s).await?;
    let theirs = pending(&pool, t).await?;
    // tx2b's cutover anchor names a run of its tenant and venue.
    for cutover in [Uuid::now_v7(), theirs] {
        refused(
            insert(
                &pool,
                "cash_anchors",
                &anchor_row(s, Uuid::now_v7(), "cutover", Some(cutover), None),
            )
            .await,
            "23503",
            "cash_anchors_venue_cutover",
        )?;
    }
    insert(
        &pool,
        "cash_anchors",
        &anchor_row(s, Uuid::now_v7(), "cutover", Some(run), None),
    )
    .await?;
    // A re-anchor's anchor has no cutover: MATCH SIMPLE leaves it free.
    insert(&pool, "cash_anchors", &anchor_row(s, Uuid::now_v7(), "reanchor", None, Some(0)))
        .await?;
    // tx2b's activation basis of the run.
    insert(&pool, "activation_bases", &activation_row(s, run)).await?;
    // Its own key, with the anchor's key (which implies it) dropped in a
    // rolled-back transaction: no run, and another tenant's run, are refused.
    for cutover in [Uuid::now_v7(), theirs] {
        let mut tx = pool.begin().await?;
        sqlx::raw_sql(
            "ALTER TABLE public.activation_bases DROP CONSTRAINT activation_bases_anchor",
        )
        .execute(&mut *tx)
        .await?;
        let result = insert(&mut *tx, "activation_bases", &activation_row(s, cutover)).await;
        tx.rollback().await?;
        refused(result, "23503", "activation_bases_venue_cutover")?;
    }
    let rows: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM cash_anchors), (SELECT count(*) FROM activation_bases)",
    )
    .fetch_one(&pool)
    .await?;
    ensure!(rows == (2, 1));
    Ok(())
}

/// (name, type, NOT NULL) of each column of venue_cutovers, in order.
fn columns() -> Value {
    const T: &str = "text";
    const TZ: &str = "timestamp with time zone";
    const U: &str = "uuid";
    const N: &str = "numeric";
    const J: &str = "jsonb";
    let list = [
        ("cutover_id", U, true),
        ("tenant_id", U, true),
        ("venue_id", T, true),
        ("account_scope", T, true),
        ("state", T, true),
        ("month", "date", true),
        ("uid", T, true),
        ("report", J, true),
        ("report_hash", T, true),
        ("generation", J, true),
        ("generation_hash", T, true),
        ("v1", "bigint", true),
        ("budget_model", T, true),
        ("previous_basis", N, true),
        ("quote_total", N, true),
        ("basis", N, true),
        ("command_id", U, true),
        ("epoch_id", U, true),
        ("created_at", TZ, true),
        ("completed_at", TZ, false),
        ("observations", J, false),
        ("abandoned_at", TZ, false),
        ("abandon_reason", T, false),
    ];
    Value::Array(
        list.into_iter()
            .map(|(name, kind, required)| json!([name, kind, required]))
            .collect(),
    )
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn migrations55_to_56_install_the_reviewed_catalog(pool: PgPool) -> Result<()> {
    install(&pool).await?;
    let actual: Value = sqlx::query_scalar(
        "SELECT jsonb_agg(jsonb_build_array(attname,format_type(atttypid,atttypmod),attnotnull) ORDER BY attnum)
         FROM pg_attribute WHERE attrelid='public.venue_cutovers'::regclass AND attnum>0 AND NOT attisdropped",
    )
    .fetch_one(&pool)
    .await?;
    ensure!(actual == columns(), "venue_cutovers columns: {actual}");
    let mut constraints: Vec<String> = sqlx::query_scalar(
        "SELECT conname||' '||contype::text FROM pg_constraint WHERE conrelid='public.venue_cutovers'::regclass",
    )
    .fetch_all(&pool)
    .await?;
    constraints.sort();
    let mut expected: Vec<String> = [
        "key p",
        "scope u",
        "version_row f",
        "ids c",
        "account c",
        "month c",
        "report c",
        "generation c",
        "v1 c",
        "budget_model c",
        "amounts c",
        "basis c",
        "state c",
        "complete c",
        "abandoned c",
        "abandon_reason c",
    ]
    .iter()
    .map(|name| format!("venue_cutovers_{name}"))
    .collect();
    expected.sort();
    ensure!(constraints == expected, "venue_cutovers constraints: {constraints:?}");
    // Every amount is decodable; the table is created empty.
    let unguarded: Vec<String> = sqlx::query_scalar(
        "SELECT a.attname::text FROM pg_attribute a WHERE a.attrelid='public.venue_cutovers'::regclass AND a.attnum>0
           AND a.atttypid='numeric'::regtype AND NOT EXISTS (SELECT 1 FROM pg_constraint c
             WHERE c.conrelid=a.attrelid AND c.contype='c'
               AND pg_get_constraintdef(c.oid) LIKE '%ledger_amount_fits('||a.attname||')%')",
    )
    .fetch_all(&pool)
    .await?;
    ensure!(unguarded.is_empty(), "amounts without ledger_amount_fits: {unguarded:?}");
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM venue_cutovers")
        .fetch_one(&pool)
        .await?;
    ensure!(rows == 0, "venue_cutovers is created empty");
    // Every key of the run and every key that names it, with its exact
    // mapping: validated, immediate, MATCH SIMPLE, NO ACTION.
    const KEYS: &[(&str, &str)] = &[
        ("activation_bases_venue_cutover", "FOREIGN KEY (tenant_id, venue_id, cutover_id) REFERENCES venue_cutovers(tenant_id, venue_id, cutover_id)"),
        ("cash_anchors_venue_cutover", "FOREIGN KEY (tenant_id, venue_id, cutover_id) REFERENCES venue_cutovers(tenant_id, venue_id, cutover_id)"),
        ("venue_accounts_venue_cutover", "FOREIGN KEY (tenant_id, venue_id, cutover_id) REFERENCES venue_cutovers(tenant_id, venue_id, cutover_id)"),
        ("venue_cutovers_key", "PRIMARY KEY (cutover_id)"),
        ("venue_cutovers_scope", "UNIQUE (tenant_id, venue_id, cutover_id)"),
        ("venue_cutovers_version_row", "FOREIGN KEY (tenant_id, venue_id) REFERENCES accounting_state(tenant_id, venue_id)"),
    ];
    let mut keys: Vec<(String, String)> = sqlx::query_as(
        "SELECT conname::text,pg_get_constraintdef(oid) FROM pg_constraint
         WHERE contype IN ('p','u','f')
           AND (conrelid='public.venue_cutovers'::regclass OR confrelid='public.venue_cutovers'::regclass)",
    )
    .fetch_all(&pool)
    .await?;
    keys.sort();
    let expected: Vec<(String, String)> =
        KEYS.iter().map(|(name, def)| (name.to_string(), def.to_string())).collect();
    ensure!(keys == expected, "keys and foreign keys: {keys:#?}");
    let flags: Vec<(String, bool, bool, String, String, String)> = sqlx::query_as(
        "SELECT conname::text,convalidated,condeferrable,confmatchtype::text,confupdtype::text,confdeltype::text
         FROM pg_constraint WHERE confrelid='public.venue_cutovers'::regclass OR conrelid='public.venue_cutovers'::regclass
         ORDER BY conname",
    )
    .fetch_all(&pool)
    .await?;
    for (name, validated, deferrable, matching, on_update, on_delete) in &flags {
        ensure!(*validated && !*deferrable, "{name}: validated and immediate");
        if name.ends_with("_venue_cutover") || name == "venue_cutovers_version_row" {
            ensure!(
                (matching.as_str(), on_update.as_str(), on_delete.as_str()) == ("s", "a", "a"),
                "{name}"
            );
        }
    }
    ensure!(flags.len() == 19, "{flags:?}");
    let triggers: Vec<(String, String, i16, String)> = sqlx::query_as(
        "SELECT tgname::text,tgfoid::regprocedure::text,tgtype,tgenabled::text
         FROM pg_trigger WHERE NOT tgisinternal AND tgrelid='public.venue_cutovers'::regclass",
    )
    .fetch_all(&pool)
    .await?;
    // 31 = ROW | BEFORE | INSERT | DELETE | UPDATE.
    ensure!(
        triggers
            == [(
                "venue_cutovers_move_forward".to_owned(),
                "venue_cutover_moves_forward()".to_owned(),
                31,
                "O".to_owned()
            )],
        "triggers: {triggers:?}"
    );
    let guard: Value = sqlx::query_scalar(
        "SELECT jsonb_build_array(pg_get_function_result(p.oid),l.lanname,p.proconfig)
         FROM pg_proc p JOIN pg_language l ON l.oid=p.prolang
         WHERE p.oid=to_regprocedure('public.venue_cutover_moves_forward()')",
    )
    .fetch_one(&pool)
    .await?;
    ensure!(
        guard == json!(["trigger", "plpgsql", ["search_path=pg_catalog, public"]]),
        "guard: {guard}"
    );
    for (index, state) in [
        ("idx_venue_cutovers_complete", "complete"),
        ("idx_venue_cutovers_pending", "pending"),
    ] {
        let actual: Value = sqlx::query_scalar(
            "SELECT jsonb_build_array(indisunique,indisvalid,indnatts,pg_get_indexdef(indexrelid,1,false),
                    pg_get_indexdef(indexrelid,2,false),pg_get_expr(indpred,indrelid))
             FROM pg_index WHERE indexrelid=to_regclass('public.'||$1)",
        )
        .bind(index)
        .fetch_one(&pool)
        .await?;
        let predicate = format!("(state = '{state}'::text)");
        ensure!(
            actual == json!([true, true, 2, "tenant_id", "venue_id", predicate]),
            "{index}: {actual}"
        );
    }
    Ok(())
}
