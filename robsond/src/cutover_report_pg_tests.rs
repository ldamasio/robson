//! The report's database part on PostgreSQL 16 (ADR-0056 step 8m-3, 8m
//! PLAN-r4 §6.2, §7.2 and §8): each database check's pass and fail as the one
//! statement reads it in its own snapshot, a report on empty Spot tables that
//! keeps its verdicts and hash through `ensure_accounting_row`, the run shown,
//! the read's four statements and its bound at size.
use std::time::Instant;

use anyhow::{ensure, Result};
use robson_domain::MonthlyBudgetModel;
use robson_exec::venue::VenueId;
use robson_testkit::db_proxy::DbProxy;
use sqlx::postgres::PgPoolOptions;

use super::{
    tests::{green, symbol, CLEAN},
    *,
};
use crate::venue_ledger_store::PgVenueLedger;

const SPOT: &str = "binance:spot";
const NET: &str = "net_from_start_non_expanding_v1";
/// A time in the previous UTC month, as SQL.
const LAST_MONTH: &str =
    "(date_trunc('month', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC' - INTERVAL '10 days')";
/// A time in the next UTC month, whose partition the migrations create.
const NEXT_MONTH: &str =
    "(date_trunc('month', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC' + INTERVAL '40 days')";

/// One tenant in one test database: its predecessor stopped clean at marker
/// 2 and its latest epoch is this process's; the month's legacy reset landed
/// at `base` (the system stream and the month's row are per database).
struct Rig {
    pool: PgPool,
    tenant: Uuid,
    epoch: Uuid,
    /// The statement's UTC month, `YYYY-MM`.
    month: String,
}

impl Rig {
    async fn new(pool: PgPool, base: &str) -> Result<Self> {
        let (tenant, old, epoch) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
        sqlx::query(
            "INSERT INTO process_epochs (epoch_id, tenant_id, binary_version, clean_stop_at, marker_version)
             VALUES ($1, $2, '8m-3-old', now(), 2)",
        )
        .bind(old)
        .bind(tenant)
        .execute(&pool)
        .await?;
        sqlx::query("INSERT INTO process_epochs (epoch_id, tenant_id, binary_version) VALUES ($1, $2, '8m-3')")
            .bind(epoch)
            .bind(tenant)
            .execute(&pool)
            .await?;
        let month: String =
            sqlx::query_scalar("SELECT to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM')")
                .fetch_one(&pool)
                .await?;
        let rig = Self { pool, tenant, epoch, month };
        let reset = json!({"type": "month_boundary_reset", "capital_base": base, "carried_positions_risk": "0"});
        rig.event(tenant, &rig.system(), 1, "month_boundary_reset", reset, "now()", "now()", None)
            .await?;
        rig.exec(&format!(
            "INSERT INTO monthly_state (year, month, capital_base, monthly_budget_model, boundary_reset_at)
             SELECT EXTRACT(YEAR FROM d), EXTRACT(MONTH FROM d), {base}, '{NET}', now()
             FROM date_trunc('month', now() AT TIME ZONE 'UTC') AS d"
        ))
        .await?;
        Ok(rig)
    }

    fn system(&self) -> String {
        format!("system:month_boundary:{}", self.month)
    }

    /// One event row of `tenant`; `occurred` and `ingested` are SQL.
    #[allow(clippy::too_many_arguments)]
    async fn event(
        &self,
        tenant: Uuid,
        stream: &str,
        seq: i64,
        event_type: &str,
        payload: Value,
        occurred: &str,
        ingested: &str,
        command: Option<Uuid>,
    ) -> Result<()> {
        sqlx::query(&format!(
            "INSERT INTO event_log (tenant_id, stream_key, seq, event_type, payload, occurred_at, ingested_at,
                 idempotency_key, command_id)
             VALUES ($1, $2, $3, $4, $5, {occurred}, {ingested}, gen_random_uuid()::TEXT, $6)"
        ))
        .bind(tenant)
        .bind(stream)
        .bind(seq)
        .bind(event_type)
        .bind(payload)
        .bind(command)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// A position stream of this tenant with no row: one event per type,
    /// occurred and ingested last month, in order.
    async fn stream(&self, types: &[&str]) -> Result<String> {
        let stream = format!("position:{}", Uuid::now_v7());
        for (seq, event_type) in (1..).zip(types) {
            let payload = json!({"type": event_type});
            self.event(
                self.tenant,
                &stream,
                seq,
                event_type,
                payload,
                LAST_MONTH,
                LAST_MONTH,
                None,
            )
            .await?;
        }
        Ok(stream)
    }

    /// A position row of `tenant` in `state`, with its entry fill and close
    /// at the SQL times given, and no stream: an orphan row when terminal.
    async fn position(
        &self,
        tenant: Uuid,
        state: &str,
        filled: &str,
        closed: &str,
    ) -> Result<Uuid> {
        let id = Uuid::now_v7();
        sqlx::query(&format!(
            "INSERT INTO positions_current (position_id, tenant_id, account_id, symbol, side, current_quantity,
                 state, last_event_id, last_seq, created_at, updated_at, entry_filled_at, closed_at)
             VALUES ($1, $2, gen_random_uuid(), 'BTCUSDT', 'long', 0, $3, gen_random_uuid(), 1,
                 {LAST_MONTH}, {LAST_MONTH}, {filled}, {closed})"
        ))
        .bind(id)
        .bind(tenant)
        .bind(state)
        .execute(&self.pool)
        .await?;
        Ok(id)
    }

    /// Last month's event_log partition, which the migrations do not create.
    async fn last_month_partition(&self) -> Result<()> {
        self.exec(
            "DO $p$ DECLARE d TIMESTAMP := date_trunc('month', now() AT TIME ZONE 'UTC'); BEGIN
             EXECUTE format('CREATE TABLE event_log_cutover_past PARTITION OF event_log FOR VALUES FROM (%L) TO (%L)',
                 (d - INTERVAL '1 month') AT TIME ZONE 'UTC', d AT TIME ZONE 'UTC'); END $p$",
        )
        .await
    }

    async fn read(&self) -> Result<DatabaseFacts> {
        Ok(read_database(&self.pool, self.tenant, SPOT, self.epoch).await?)
    }

    /// The database checks that fail, in order.
    async fn failing(&self) -> Result<Vec<String>> {
        let facts = self.read().await?;
        ensure!(facts.checks.len() == DATABASE_CHECKS.len(), "{facts:?}");
        Ok(facts.checks.into_iter().filter(|(_, pass)| !pass).map(|(id, _)| id).collect())
    }

    async fn exec(&self, sql: &str) -> Result<()> {
        sqlx::raw_sql(sql).execute(&self.pool).await?;
        Ok(())
    }
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_quiet_month_passes_every_database_check(pool: PgPool) -> Result<()> {
    let rig = Rig::new(pool, "0").await?;
    // Another tenant's position event and open row in the month are not this
    // tenant's.
    let other = Uuid::now_v7();
    let stream = format!("position:{}", Uuid::now_v7());
    rig.event(other, &stream, 1, "position_armed", json!({}), "now()", "now()", None)
        .await?;
    rig.position(other, "active", "now()", "NULL").await?;
    let facts = rig.read().await?;
    ensure!(facts.checks.values().all(|pass| *pass), "{facts:#?}");
    ensure!(facts.previous == Some(Decimal::ZERO) && facts.run.is_none());
    ensure!(facts.incident_reasons.is_none() && !facts.values.contains_key("incident_reasons"));
    ensure!(facts.month.format("%Y-%m").to_string() == rig.month);
    for (key, value) in [
        ("position_events", json!(0)),
        ("open_rows", json!(0)),
        ("open_streams", json!(0)),
        ("month_rows", json!(0)),
        // Both models are recorded (v16:2046).
        ("accounting_model", json!(NET)),
        ("month_model", json!(NET)),
        ("event_capital", json!("0")),
    ] {
        ensure!(facts.values[key] == value, "{key}: {:?}", facts.values);
    }
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn c9a_fails_on_an_ingested_at_hit(pool: PgPool) -> Result<()> {
    let rig = Rig::new(pool, "0").await?;
    // It happened last month and was ingested in this one.
    let stream = format!("position:{}", Uuid::now_v7());
    rig.event(rig.tenant, &stream, 1, "position_closed", json!({}), LAST_MONTH, "now()", None)
        .await?;
    ensure!(rig.failing().await? == ["c9a"]);
    ensure!(rig.read().await?.values["position_events"] == json!(1));
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn c9a_fails_on_an_occurred_at_hit(pool: PgPool) -> Result<()> {
    let rig = Rig::new(pool, "0").await?;
    // It happened in this month and lands in next month's partition.
    let stream = format!("position:{}", Uuid::now_v7());
    rig.event(rig.tenant, &stream, 1, "position_closed", json!({}), "now()", NEXT_MONTH, None)
        .await?;
    ensure!(rig.failing().await? == ["c9a"]);
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn c9a_fails_on_a_detached_partition(pool: PgPool) -> Result<()> {
    let rig = Rig::new(pool, "0").await?;
    let partition = format!("event_log_{}", rig.month.replace('-', "_"));
    // Detached, the month's partition hides its events: no position event is
    // counted, and the month's reset event is gone from C9c's view too.
    rig.exec(&format!("ALTER TABLE event_log DETACH PARTITION {partition}")).await?;
    ensure!(rig.read().await?.values["position_events"] == json!(0));
    ensure!(rig.failing().await? == ["c9a", "c9c"]);
    // Missing, likewise.
    rig.exec(&format!("DROP TABLE {partition}")).await?;
    ensure!(rig.failing().await? == ["c9a", "c9c"]);
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn c9b_fails_on_an_open_row_or_an_open_stream_without_a_row(pool: PgPool) -> Result<()> {
    let rig = Rig::new(pool, "0").await?;
    rig.last_month_partition().await?;
    for state in ["armed", "entering", "active", "exiting"] {
        let id = rig.position(rig.tenant, state, "NULL", "NULL").await?;
        ensure!(rig.failing().await? == ["c9b"], "{state}");
        ensure!(rig.read().await?.values["open_rows"] == json!(1));
        sqlx::query(
            "UPDATE positions_current SET state = 'closed', closed_at = now() - INTERVAL '40 days'
             WHERE position_id = $1",
        )
        .bind(id)
        .execute(&rig.pool)
        .await?;
        ensure!(rig.failing().await?.is_empty(), "{state}");
    }
    // A stream with events and no row is open unless its last event is one
    // the projector ends a row with.
    for last in ["position_armed", "entry_filled", "position_error"] {
        let stream = rig.stream(&["position_armed", last]).await?;
        ensure!(rig.failing().await? == ["c9b"], "{last}");
        ensure!(rig.read().await?.values["open_streams"] == json!(1));
        sqlx::query("DELETE FROM event_log WHERE stream_key = $1")
            .bind(stream)
            .execute(&rig.pool)
            .await?;
    }
    for last in TERMINAL {
        rig.stream(&["position_armed", "entry_filled", last]).await?;
    }
    ensure!(rig.failing().await?.is_empty());
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn c9d_fails_on_a_terminal_orphan_closed_in_the_month(pool: PgPool) -> Result<()> {
    let rig = Rig::new(pool, "0").await?;
    // Codex's terminal orphan (v16:1573): a closed row whose stream is
    // absent; C9a and C9b pass, but it closed in this month.
    let closed = rig.position(rig.tenant, "closed", LAST_MONTH, "now()").await?;
    let events: i64 = sqlx::query_scalar("SELECT count(*) FROM event_log WHERE stream_key = $1")
        .bind(format!("position:{closed}"))
        .fetch_one(&rig.pool)
        .await?;
    ensure!(events == 0);
    ensure!(rig.failing().await? == ["c9d"]);
    ensure!(rig.read().await?.values["month_rows"] == json!(1));
    sqlx::query("DELETE FROM positions_current WHERE position_id = $1")
        .bind(closed)
        .execute(&rig.pool)
        .await?;
    ensure!(rig.failing().await?.is_empty());
    // Or it was filled in it.
    rig.position(rig.tenant, "closed", "now()", NEXT_MONTH).await?;
    ensure!(rig.failing().await? == ["c9d"]);
    Ok(())
}

/// The two production `error` positions of 2026-11-03 (UPD:47-52): terminal
/// rows, never filled, their streams ending last month. They block nothing.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn error_positions_are_terminal(pool: PgPool) -> Result<()> {
    let rig = Rig::new(pool, "0").await?;
    rig.last_month_partition().await?;
    for _ in 0..2 {
        let id = rig.position(rig.tenant, "error", "NULL", "NULL").await?;
        let stream = format!("position:{id}");
        for (seq, kind) in [(1, "position_armed"), (2, "entry_execution_rejected")] {
            rig.event(rig.tenant, &stream, seq, kind, json!({}), LAST_MONTH, LAST_MONTH, None)
                .await?;
        }
    }
    rig.position(rig.tenant, "cancelled", "NULL", LAST_MONTH).await?;
    let facts = rig.read().await?;
    ensure!(facts.checks.values().all(|pass| *pass), "{facts:#?}");
    ensure!(facts.values["open_rows"] == json!(0) && facts.values["open_streams"] == json!(0));
    Ok(())
}

/// 8p's zero reset (8p:340-346): a `month_boundary_reset` at 0 under the
/// month's command id is the month's reset.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn c9c_accepts_the_spot_zero_reset(pool: PgPool) -> Result<()> {
    let rig = Rig::new(pool, "0").await?;
    rig.exec("DELETE FROM event_log WHERE stream_key LIKE 'system:%'").await?;
    let zero =
        json!({"type": "month_boundary_reset", "capital_base": "0", "carried_positions_risk": "0"});
    let (system, command) = (rig.system(), Some(Uuid::now_v7()));
    rig.event(rig.tenant, &system, 1, "month_boundary_reset", zero, "now()", "now()", command)
        .await?;
    let facts = rig.read().await?;
    ensure!(facts.checks.values().all(|pass| *pass), "{facts:#?}");
    ensure!(facts.previous == Some(Decimal::ZERO) && facts.values["event_capital"] == json!("0"));
    Ok(())
}

/// The latest event by seq is a later recalibration's `new_capital_base`.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn c9c_follows_a_later_recalibration(pool: PgPool) -> Result<()> {
    let rig = Rig::new(pool, "2000").await?;
    let recalibrated = json!({"type": "capital_base_recalibrated", "previous_capital_base": "2000",
        "new_capital_base": "1500", "wallet_balance": "1500", "carried_risk": "0"});
    let system = rig.system();
    rig.event(
        rig.tenant,
        &system,
        2,
        "capital_base_recalibrated",
        recalibrated,
        "now()",
        "now()",
        None,
    )
    .await?;
    ensure!(rig.failing().await? == ["c9c"]);
    rig.exec("UPDATE monthly_state SET capital_base = 1500").await?;
    ensure!(rig.failing().await?.is_empty());
    ensure!(rig.read().await?.previous == Some(Decimal::from(1500)));
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn c9c_requires_a_landed_reset(pool: PgPool) -> Result<()> {
    let rig = Rig::new(pool, "0").await?;
    rig.exec("UPDATE monthly_state SET boundary_reset_at = NULL").await?;
    ensure!(rig.failing().await? == ["c9c"]);
    // No row at all: no reset, no previous basis, no model.
    rig.exec("DELETE FROM monthly_state").await?;
    ensure!(rig.read().await?.previous.is_none());
    ensure!(rig.failing().await? == ["c9c", "sd14"]);
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn c9c_requires_a_zero_month_peak(pool: PgPool) -> Result<()> {
    let rig = Rig::new(pool, "0").await?;
    rig.exec("UPDATE monthly_state SET month_peak_net = 0.00000001").await?;
    ensure!(rig.failing().await? == ["c9c"]);
    Ok(())
}

/// Only the capital field is compared: the event's other fields (its carried
/// risk, month, year and timestamp) and the row's (carried risk, counters)
/// differ freely.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn c9c_compares_the_capital_field_only(pool: PgPool) -> Result<()> {
    let rig = Rig::new(pool, "2000.5").await?;
    rig.exec("DELETE FROM event_log WHERE stream_key LIKE 'system:%'").await?;
    let reset = json!({"type": "month_boundary_reset", "capital_base": "2000.50000000",
        "carried_positions_risk": "12.5", "month": 1, "year": 1999, "timestamp": "2001-01-01T00:00:00Z"});
    let system = rig.system();
    rig.event(rig.tenant, &system, 1, "month_boundary_reset", reset, LAST_MONTH, "now()", None)
        .await?;
    rig.exec("UPDATE monthly_state SET carried_risk = 3, trades_opened = 2, realized_loss = 1")
        .await?;
    ensure!(rig.failing().await?.is_empty());
    rig.exec("UPDATE monthly_state SET capital_base = 2000.49999999").await?;
    ensure!(rig.failing().await? == ["c9c"]);
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn sd14_requires_net_from_start_for_the_month_and_the_venue(pool: PgPool) -> Result<()> {
    let rig = Rig::new(pool, "0").await?;
    rig.exec("UPDATE monthly_state SET monthly_budget_model = 'hwm_v1'").await?;
    ensure!(rig.failing().await? == ["sd14"]);
    rig.exec(&format!("UPDATE monthly_state SET monthly_budget_model = '{NET}'"))
        .await?;
    sqlx::query("INSERT INTO accounting_state (tenant_id, venue_id, budget_model) VALUES ($1, $2, 'hwm_v1')")
        .bind(rig.tenant)
        .bind(SPOT)
        .execute(&rig.pool)
        .await?;
    ensure!(rig.failing().await? == ["sd14"]);
    ensure!(rig.read().await?.values["accounting_model"] == json!("hwm_v1"));
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn c7_reads_each_durable_clause(pool: PgPool) -> Result<()> {
    let rig = Rig::new(pool, "0").await?;
    let t = rig.tenant;
    let old = "WHERE binary_version = '8m-3-old'";
    for (break_it, mend) in [
        (
            format!(
                "INSERT INTO recovery_incidents (incident_id, tenant_id, reason, first_at, last_at, count,
                 first_generation, last_generation) VALUES (gen_random_uuid(), '{t}', 'uncertain_outcome',
                 now(), now(), 1, 1, 1)"
            ),
            // A release, with its decision (recovery_incidents_decision_fk).
            "WITH d AS (INSERT INTO journal_decisions (decision_id, kind, actor, auth_kind, note, payload)
                 VALUES (gen_random_uuid(), 'release', 'operator', 'verified', 'released', '{}') RETURNING decision_id)
             UPDATE recovery_incidents SET released_at = now(), decision_id = (SELECT decision_id FROM d)"
                .to_string(),
        ),
        (
            // Cleared or not, a suspension row refuses (§6.11.2).
            format!(
                "INSERT INTO stream_suspensions (suspension_id, tenant_id, stream_key, stream_kind, error,
                 suspended_at, cleared_at) VALUES (gen_random_uuid(), '{t}', 'position:x', 'position', 'e',
                 now(), now())"
            ),
            "DELETE FROM stream_suspensions".to_string(),
        ),
        (
            // Boundary B is any tenant's.
            "INSERT INTO order_intents (intent_id, tenant_id, venue_id, account_scope, client_order_id, kind,
             params, created_at) VALUES (gen_random_uuid(), gen_random_uuid(), 'binance:usdm', 'operated', 'b-1',
             'exit', '{}', now())"
                .to_string(),
            "UPDATE order_intents SET venue_outcome = 'success'".to_string(),
        ),
        (
            format!(
                "INSERT INTO process_epochs (epoch_id, tenant_id, binary_version)
                 VALUES (gen_random_uuid(), '{t}', 'later')"
            ),
            "DELETE FROM process_epochs WHERE binary_version = 'later'".to_string(),
        ),
        (
            format!("UPDATE process_epochs SET clean_stop_at = NULL, marker_version = NULL {old}"),
            format!("UPDATE process_epochs SET clean_stop_at = now(), marker_version = 2 {old}"),
        ),
        (
            format!("UPDATE process_epochs SET marker_version = 1 {old}"),
            format!("UPDATE process_epochs SET marker_version = 2 {old}"),
        ),
    ] {
        ensure!(rig.failing().await?.is_empty(), "before {break_it}");
        rig.exec(&break_it).await?;
        ensure!(rig.failing().await? == ["c7"], "{break_it}");
        rig.exec(&mend).await?;
    }
    // No predecessor at all is not a clean one.
    rig.exec(&format!("DELETE FROM process_epochs {old}")).await?;
    ensure!(rig.failing().await? == ["c7"]);
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn c7_names_the_open_incident_reasons(pool: PgPool) -> Result<()> {
    let rig = Rig::new(pool, "0").await?;
    sqlx::query(
        "INSERT INTO recovery_incidents (incident_id, tenant_id, reason, first_at, last_at, count, first_generation,
             last_generation) VALUES (gen_random_uuid(), $1, 'stream_suspended', now(), now(), 1, 1, 1),
             (gen_random_uuid(), $1, 'unclean_start', now(), now(), 1, 1, 1)",
    )
    .bind(rig.tenant)
    .execute(&rig.pool)
    .await?;
    let facts = rig.read().await?;
    let reasons = ["stream_suspended", "unclean_start"].map(String::from).to_vec();
    ensure!(
        facts.incident_reasons == Some(reasons) && !facts.values.contains_key("incident_reasons")
    );
    ensure!(facts.values["incidents"] == json!(2) && !facts.checks["c7"]);
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn c8_fails_on_an_unfinished_saga(pool: PgPool) -> Result<()> {
    let rig = Rig::new(pool, "0").await?;
    let saga = "INSERT INTO funding_sagas (tenant_id, saga_id, state, quote) VALUES ($1, gen_random_uuid(), $2, '{}')";
    for state in ["REFRESHED", "FAILED"] {
        sqlx::query(saga).bind(rig.tenant).bind(state).execute(&rig.pool).await?;
    }
    ensure!(rig.failing().await?.is_empty());
    sqlx::query(saga)
        .bind(rig.tenant)
        .bind("TRANSFERRED")
        .execute(&rig.pool)
        .await?;
    ensure!(rig.failing().await? == ["c8"]);
    Ok(())
}

/// A report taken before any Spot row exists and the one certify re-runs
/// after `ensure_accounting_row` (§6.3 step 1) have the same verdicts and
/// hash: neither the accounting row's birth nor its version is a decision
/// value.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn report_on_empty_spot_tables_keeps_its_hash_through_certify(pool: PgPool) -> Result<()> {
    let rig = Rig::new(pool, "0").await?;
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM accounting_state")
        .fetch_one(&rig.pool)
        .await?;
    ensure!(rows == 0);
    let (_, venue) = green();
    let before = evaluate(&rig.read().await?, &venue, CLEAN, &symbol(), Utc::now());
    ensure!(before.passed(), "{before:#?}");
    let row = PgVenueLedger::new(&rig.pool, rig.tenant)
        .ensure_accounting_row(&VenueId::new(SPOT), MonthlyBudgetModel::NetFromStartNonExpandingV1)
        .await?;
    ensure!(row.version == 0);
    let after = evaluate(&rig.read().await?, &venue, CLEAN, &symbol(), Utc::now());
    ensure!(after.checks == before.checks && after.decision == before.decision);
    ensure!(after.hash == before.hash);
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn the_report_shows_the_venues_run(pool: PgPool) -> Result<()> {
    let rig = Rig::new(pool, "0").await?;
    sqlx::query(
        "INSERT INTO accounting_state (tenant_id, venue_id, budget_model) VALUES ($1, $2, $3)",
    )
    .bind(rig.tenant)
    .bind(SPOT)
    .bind(NET)
    .execute(&rig.pool)
    .await?;
    let run = |later: &str| {
        format!(
            "INSERT INTO venue_cutovers (cutover_id, tenant_id, venue_id, account_scope, state, month, uid, report,
                 report_hash, generation, generation_hash, v1, budget_model, previous_basis, quote_total, basis,
                 command_id, epoch_id, created_at)
             VALUES (gen_random_uuid(), '{}', '{SPOT}', 'main', 'pending', date_trunc('month', now())::DATE, 'u',
                 '{{}}', repeat('a', 64), '{{\"schema_version\": 1}}', repeat('b', 64), 0, '{NET}', 0, 0, 0,
                 gen_random_uuid(), gen_random_uuid(), now() + INTERVAL '{later}')",
            rig.tenant
        )
    };
    let state = |run: Option<Value>| run.map(|r| (r["state"].clone(), r["abandon_reason"].clone()));
    ensure!(rig.read().await?.run.is_none());
    rig.exec(&run("0 s")).await?;
    rig.exec("UPDATE venue_cutovers SET state = 'abandoned', abandoned_at = now(), abandon_reason = 'timeout'")
        .await?;
    rig.exec(&run("1 s")).await?;
    ensure!(state(rig.read().await?.run) == Some((json!("pending"), Value::Null)));
    rig.exec(
        "UPDATE venue_cutovers SET state = 'abandoned', abandoned_at = now(), abandon_reason = 'operator'
         WHERE state = 'pending'",
    )
    .await?;
    ensure!(state(rig.read().await?.run) == Some((json!("abandoned"), json!("operator"))));
    rig.exec(&run("2 s")).await?;
    rig.exec(
        "UPDATE venue_cutovers SET state = 'complete', completed_at = now(), observations = '{}'
         WHERE state = 'pending'",
    )
    .await?;
    rig.exec(&run("3 s")).await?;
    ensure!(state(rig.read().await?.run) == Some((json!("complete"), Value::Null)));
    Ok(())
}

/// The read is BEGIN (REPEATABLE READ, READ ONLY), the bounds, the one read
/// and COMMIT (§7.2): four statements.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn the_read_is_one_read_only_snapshot_of_four_statements(pool: PgPool) -> Result<()> {
    let rig = Rig::new(pool, "0").await?;
    let options = rig.pool.connect_options();
    let upstream = tokio::net::lookup_host((options.get_host(), options.get_port()))
        .await?
        .next()
        .ok_or_else(|| anyhow::anyhow!("no local address"))?;
    let proxy = DbProxy::start(upstream).await?;
    let url = robson_testkit::daemon_process::database_url_through(
        &std::env::var("DATABASE_URL")?,
        proxy.addr(),
        options.get_database().unwrap(),
    )?;
    let proxied = PgPoolOptions::new()
        .max_connections(1)
        .test_before_acquire(false)
        .connect(&url)
        .await?;
    drop(proxied.acquire().await?);
    proxy.enable_query_trace(16);
    read_database(&proxied, rig.tenant, SPOT, rig.epoch).await?;
    let trace: Vec<String> = proxy.query_trace().iter().map(|sql| sql.trim().to_owned()).collect();
    ensure!(trace.len() == 4, "{trace:#?}");
    ensure!(trace[0] == "BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY" && trace[3] == "COMMIT");
    ensure!(trace[1] == crate::balance_accounting::BOUNDS);
    ensure!(trace[2].starts_with("-- The cutover report's database part"));
    Ok(())
}

/// §7.2's bound, measured [AE]: with more than 10,000 position events in the
/// month, 200 rows and 2,000 intents, the read stays under its 1 s statement
/// bound; its plan is printed (EXPLAIN ANALYZE, BUFFERS).
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn the_report_read_stays_within_its_bound_at_size(pool: PgPool) -> Result<()> {
    let rig = Rig::new(pool, "0").await?;
    let t = rig.tenant;
    rig.exec(&format!(
        "INSERT INTO event_log (tenant_id, stream_key, seq, event_type, payload, occurred_at, idempotency_key)
         SELECT '{t}', 'position:' || md5((g / 50)::TEXT)::UUID, g % 50 + 1, 'position_monitor_tick', '{{}}',
                now(), 'size-' || g FROM generate_series(0, 10199) g;
         INSERT INTO positions_current (position_id, tenant_id, account_id, symbol, side, current_quantity, state,
             last_event_id, last_seq, created_at, updated_at)
         SELECT md5(g::TEXT)::UUID, '{t}', gen_random_uuid(), 'BTCUSDT', 'long', 0, 'closed', gen_random_uuid(),
                50, now(), now() FROM generate_series(0, 199) g;
         INSERT INTO order_intents (intent_id, tenant_id, venue_id, account_scope, client_order_id, kind, params,
             venue_outcome, created_at)
         SELECT gen_random_uuid(), '{t}', 'binance:usdm', 'operated', 'size-' || g, 'exit', '{{}}', 'success',
                now() FROM generate_series(1, 2000) g;
         ANALYZE event_log, positions_current, order_intents;"
    ))
    .await?;
    let started = Instant::now();
    let facts = rig.read().await?;
    let elapsed = started.elapsed();
    ensure!(facts.values["position_events"] == json!(10200) && !facts.checks["c9a"]);
    ensure!(elapsed < Duration::from_secs(1), "{elapsed:?}");
    let terminal = TERMINAL.map(|t| format!("'{t}'")).join(",");
    let sql = include_str!("cutover_report.sql")
        .replace("$1", &format!("'{t}'::UUID"))
        .replace("$2", &format!("'{SPOT}'"))
        .replace("$3", &format!("'{}'::UUID", rig.epoch))
        .replace("$4::TEXT[]", &format!("ARRAY[{terminal}]::TEXT[]"));
    let plan: Vec<String> = sqlx::query_scalar(&format!("EXPLAIN (ANALYZE, BUFFERS) {sql}"))
        .fetch_all(&rig.pool)
        .await?;
    eprintln!("CUTOVER_REPORT_READ elapsed={elapsed:?}\n{}", plan.join("\n"));
    Ok(())
}
