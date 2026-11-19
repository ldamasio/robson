//! Native structural warm observation after a real typed leg admission.
use std::{sync::Arc, time::Duration};

use anyhow::{ensure, Context, Result};
use chrono::{Datelike, NaiveDate, Utc};
use robson_domain::MonthlyBudgetModel;
use robson_exec::{
    order_journal::{
        AdmissionRow, BalanceBatch, IntentKind, IntentRow, JournalBackend, JournalBatch,
    },
    venue::VenueId,
};
use rust_decimal_macros::dec;
use sqlx::{ConnectOptions, PgPool};

use super::*;
use crate::{order_journal_store::PgJournalBackend, venue_ledger_store::PgVenueLedger};

const ACCOUNT: &str = "warm-native";
const SYMBOL: &str = "BTCUSDT";
const ORDER: &str = "991100";

fn batch(kind: IntentKind, position: Uuid) -> JournalBatch {
    let intent_id = Uuid::now_v7();
    JournalBatch {
        intent: IntentRow {
            intent_id,
            venue_id: VenueId::BINANCE_SPOT.to_owned(),
            account_scope: ACCOUNT.to_owned(),
            client_order_id: format!("warm-{}", intent_id.simple()),
            position_id: Some(position),
            kind,
            cancels_order_id: None,
            params: serde_json::json!({}),
            created_at: Utc::now(),
        },
        outcome: None,
        attempts: Vec::new(),
        balance: Some(BalanceBatch {
            admission: None,
            handed_off: false,
            commit_bound: Duration::from_millis(500),
        }),
    }
}

async fn actual_leg(
    pool: &PgPool,
    tenant: Uuid,
    venue: &VenueId,
    position: Uuid,
) -> Result<PgJournalBackend> {
    let ledger = PgVenueLedger::new(pool, tenant);
    ledger
        .ensure_accounting_row(venue, MonthlyBudgetModel::NetFromStartNonExpandingV1)
        .await?;
    let now = Utc::now();
    ledger.insert_route(position, venue, ACCOUNT, now).await?;
    let mut j = batch(IntentKind::Entry, position);
    j.balance.as_mut().context("native entry balance")?.admission = Some(Arc::new(AdmissionRow {
        position_id: position,
        symbol: SYMBOL.to_owned(),
        decided_month: NaiveDate::from_ymd_opt(now.year(), now.month(), 1)
            .context("test admission month")?,
        taker_bps: dec!(10),
        gap_bps: dec!(10),
        notional_margin_bps: dec!(200),
        min_notional: dec!(5),
        lot_step: dec!(0.01),
        fee_asset_policy: "base".to_owned(),
        envelope_source: "native warm fixture".to_owned(),
        envelope_as_of: now,
        envelope_hash: "warm-native-test".to_owned(),
        planned_qty: dec!(1),
        requested_qty: dec!(1.01),
        reference_price: dec!(100),
        remainder_reserve: dec!(0.1),
        admission_charge: dec!(12),
        initial_trigger: dec!(90),
    }));
    let store = PgJournalBackend::new(pool, tenant);
    store
        .commit(&j)
        .await
        .map_err(|e| anyhow::anyhow!("real typed admission: {e:?}"))?;
    let stop = batch(IntentKind::Protection, position);
    store
        .commit(&stop)
        .await
        .map_err(|e| anyhow::anyhow!("real stop intent: {e:?}"))?;
    sqlx::query(
        "INSERT INTO venue_orders
        (venue_id,account_scope,symbol,exchange_order_id,tenant_id,client_order_id,
         last_client_order_id,owner,intent_id,position_id,kind,side,order_type,orig_qty,
         stop_price,status,phase,last_checked_at,live_seen_at)
        VALUES('binance:spot',$1,$2,$3,$4,$5,$5,'leg',$6,$7,'stop','SELL','STOP_LOSS',1,90,
               'NEW','live',now(),now())",
    )
    .bind(ACCOUNT)
    .bind(SYMBOL)
    .bind(ORDER)
    .bind(tenant)
    .bind(&stop.intent.client_order_id)
    .bind(stop.intent.intent_id)
    .bind(position)
    .execute(pool)
    .await?;
    Ok(store)
}

// Read-time generation separation is exercised with native objects and
// thousands of same-owner rows for other generations.
const FILL: &str = "WITH revisions AS MATERIALIZED (
    SELECT CASE WHEN n=0 THEN $3::uuid ELSE md5($3::text||n)::uuid END AS generation, CASE WHEN n=0 THEN 0 ELSE 100 END AS noise
    FROM generate_series(0,$7::int) n
), sl AS (
    INSERT INTO spot_order_set_sell_legs
      (tenant_id,venue_id,account_scope,generation_id,symbol,position_id,potential_count,conflict_count)
    SELECT $1,'binance:spot',$2,generation,$4,$5,11+noise,12+noise FROM revisions
), sy AS (
    INSERT INTO spot_order_set_sell_symbols
      (tenant_id,venue_id,account_scope,generation_id,symbol,potential_count,conflict_count)
    SELECT $1,'binance:spot',$2,generation,$4,13+noise,14+noise FROM revisions
), su AS (
    INSERT INTO spot_order_set_sell_unbound
      (tenant_id,venue_id,account_scope,generation_id,potential_count,conflict_count)
    SELECT $1,'binance:spot',$2,generation,15+noise,16+noise FROM revisions
), cl AS (
    INSERT INTO spot_order_set_cancel_legs
      (tenant_id,venue_id,account_scope,generation_id,symbol,position_id,active_count,conflict_count)
    SELECT $1,'binance:spot',$2,generation,$4,$5,17+noise,18+noise FROM revisions
), cu AS (
    INSERT INTO spot_order_set_cancel_unbound
      (tenant_id,venue_id,account_scope,generation_id,active_count,conflict_count)
    SELECT $1,'binance:spot',$2,generation,19+noise,20+noise FROM revisions
), cg AS (
    INSERT INTO spot_order_set_cancel_ungroupable
      (tenant_id,venue_id,account_scope,generation_id,symbol,position_id,active_count,conflict_count)
    SELECT $1,'binance:spot',$2,generation,$4,$5,21+noise,22+noise FROM revisions
), sm AS (
    INSERT INTO spot_order_set_sell_orders
      (tenant_id,venue_id,account_scope,generation_id,symbol,exchange_order_id,scope_kind,
       potential,conflict,scope_symbol,scope_position)
    SELECT $1,'binance:spot',$2,generation,$4,$6,'leg',true,false,$4,$5 FROM revisions
)
INSERT INTO spot_order_set_cancel_groups
  (tenant_id,venue_id,account_scope,generation_id,symbol,position_id,target_id,
   pending_count,conflict_count,terminal_mask)
SELECT $1,'binance:spot',$2,generation,$4,$5,$6,23+noise,24+noise,true FROM revisions";

fn projection_nodes<'a>(node: &'a serde_json::Value, found: &mut Vec<&'a serde_json::Value>) {
    if node["Relation Name"].as_str().is_some_and(|name| {
        name.starts_with("spot_order_set_") && name != "spot_order_set_generation"
    }) {
        found.push(node);
    }
    if let Some(children) = node["Plans"].as_array() {
        for child in children {
            projection_nodes(child, found);
        }
    }
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn persisted_warm_order_set_native_same_snapshot_scope_and_absence(
    pool: PgPool,
) -> Result<()> {
    let tenant = Uuid::now_v7();
    let foreign = Uuid::now_v7();
    let position = Uuid::now_v7();
    let venue = VenueId::new(VenueId::BINANCE_SPOT);
    let expected = ExpectedGeneration {
        tenant_id: tenant,
        venue: &venue,
        account_scope: ACCOUNT,
        generation_id: Uuid::now_v7(),
        process_epoch: crate::process_epoch::start_epoch(&pool, tenant, "warm-native")
            .await?
            .epoch
            .epoch_id,
        source_audit_id: Uuid::now_v7(),
    };
    let store = actual_leg(&pool, tenant, &venue, position).await?;
    let version: i64 = sqlx::query_scalar(
        "SELECT version FROM accounting_state WHERE tenant_id=$1 AND venue_id='binance:spot'",
    )
    .bind(tenant)
    .fetch_one(&pool)
    .await?;
    let keys = WarmKeys {
        binding: expected,
        position_id: position,
        symbol: SYMBOL,
        candidate_id: Some(ORDER),
    };
    let mut plain = pool.connect_options().connect().await?;
    ensure!(observe_warm(&mut plain, keys).await?.is_none());
    sqlx::query(
        "INSERT INTO spot_order_set_generation(tenant_id,venue_id,account_scope,generation_id)
         VALUES($1,'binance:spot',$2,$3)",
    )
    .bind(tenant)
    .bind(ACCOUNT)
    .bind(expected.generation_id)
    .execute(&pool)
    .await?;
    ensure!(observe_warm(&mut plain, keys).await?.is_none());
    sqlx::query(
        "UPDATE spot_order_set_generation
         SET process_epoch=$4,source_audit_id=$5,accounting_version=$6
         WHERE (tenant_id,venue_id,account_scope,generation_id)=($1,'binance:spot',$2,$3)",
    )
    .bind(tenant)
    .bind(ACCOUNT)
    .bind(expected.generation_id)
    .bind(expected.process_epoch)
    .bind(expected.source_audit_id)
    .bind(version)
    .execute(&pool)
    .await?;

    for (owner, account, fill) in [
        (tenant, ACCOUNT, 4096),
        (foreign, ACCOUNT, 0),
        (tenant, "other", 0),
    ] {
        sqlx::query(FILL)
            .bind(owner)
            .bind(account)
            .bind(expected.generation_id)
            .bind(SYMBOL)
            .bind(position)
            .bind(ORDER)
            .bind(fill)
            .execute(&pool)
            .await?;
    }
    sqlx::query(
        "INSERT INTO accounting_state(tenant_id,venue_id,budget_model)
         VALUES($1,'binance:spot','net_from_start_non_expanding_v1')",
    )
    .bind(foreign)
    .execute(&pool)
    .await?;
    sqlx::query(
        "INSERT INTO venue_orders(venue_id,account_scope,symbol,exchange_order_id,tenant_id,
          client_order_id,last_client_order_id,owner,side,order_type,orig_qty,status,phase,
          last_checked_at,live_seen_at)
         VALUES('binance:spot',$1,$2,'991101',$3,'foreign','foreign','discovered','SELL',
                'LIMIT',99,'NEW','live',now(),now())",
    )
    .bind(ACCOUNT)
    .bind(SYMBOL)
    .bind(foreign)
    .execute(&pool)
    .await?;
    sqlx::raw_sql(
        "ANALYZE spot_order_set_sell_legs; ANALYZE spot_order_set_sell_symbols;
         ANALYZE spot_order_set_sell_unbound; ANALYZE spot_order_set_sell_orders;
         ANALYZE spot_order_set_cancel_legs; ANALYZE spot_order_set_cancel_unbound;
         ANALYZE spot_order_set_cancel_ungroupable; ANALYZE spot_order_set_cancel_groups",
    )
    .execute(&pool)
    .await?;

    let options = pool.connect_options();
    let upstream = tokio::net::lookup_host((options.get_host(), options.get_port()))
        .await?
        .next()
        .context("isolated PostgreSQL native endpoint")?;
    ensure!(upstream.ip().is_loopback());
    let proxy = robson_testkit::db_proxy::DbProxy::start(upstream).await?;
    let mut proxied = options
        .as_ref()
        .clone()
        .host("127.0.0.1")
        .port(proxy.addr().port())
        .ssl_mode(sqlx::postgres::PgSslMode::Disable)
        .connect()
        .await?;
    proxy.enable_exact_query_trace(4);
    let before = proxy.stats();
    let observed = observe_warm(&mut proxied, keys)
        .await?
        .context("native positive structural observation")?;
    ensure!(
        proxy.stats().statements - before.statements == 1
            && proxy.query_trace().len() == 1
            && proxy.query_trace_dropped() == 0
    );
    ensure!(
        (
            observed.generation_id,
            observed.process_epoch,
            observed.source_audit_id,
            observed.accounting_version
        ) == (
            expected.generation_id,
            expected.process_epoch,
            expected.source_audit_id,
            version
        )
    );
    ensure!(
        observed.position_id == position
            && observed.symbol == SYMBOL
            && observed.gross_qty.is_none()
            && observed.target_trigger == dec!(90)
            && observed.consumed_cum == dec!(0)
    );
    ensure!(
        [
            observed.leg_sell,
            observed.leg_sell_conflicts,
            observed.symbol_sell,
            observed.symbol_sell_conflicts,
            observed.unbound_sell,
            observed.unbound_sell_conflicts,
            observed.grouped_cancel,
            observed.grouped_cancel_conflicts,
            observed.unbound_cancel,
            observed.unbound_cancel_conflicts,
            observed.ungroupable_cancel,
            observed.ungroupable_cancel_conflicts
        ] == [11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22].map(Some)
    );
    ensure!(
        observed.candidate_scope.as_deref() == Some("leg")
            && observed.candidate_position == Some(position)
            && observed.candidate_potential == Some(true)
            && observed.candidate_conflict == Some(false)
    );
    ensure!(
        (
            observed.target_cancel_pending,
            observed.target_cancel_conflicts,
            observed.target_cancel_terminal
        ) == (Some(23), Some(24), Some(true))
    );
    ensure!(
        observed.stop_id.as_deref() == Some(ORDER)
            && observed.stop_qty == Some(dec!(1))
            && observed.stop_executed == Some(dec!(0))
            && observed.stop_trigger == Some(dec!(90))
            && observed.exact_stop_identity
    );

    let plan: serde_json::Value =
        sqlx::query_scalar(&format!("EXPLAIN (ANALYZE,BUFFERS,FORMAT JSON) {WARM_SQL}"))
            .bind(tenant)
            .bind(venue.as_str())
            .bind(ACCOUNT)
            .bind(expected.generation_id)
            .bind(expected.process_epoch)
            .bind(expected.source_audit_id)
            .bind(position)
            .bind(SYMBOL)
            .bind(ORDER)
            .fetch_one(&mut plain)
            .await?;
    ensure!(plan[0]["Plan"]["Actual Rows"] == 1);
    let mut leaves = Vec::new();
    projection_nodes(&plan[0]["Plan"], &mut leaves);
    ensure!(
        leaves.len() == 8
            && leaves.iter().all(|node| {
                node["Index Cond"].as_str().is_some()
                    && node["Index Name"].as_str().is_some()
                    && node["Actual Rows"] == 1
                    && node["Actual Loops"] == 1
            }),
        "each of eight projected facts must be an exact one-row native probe: {plan}"
    );
    eprintln!("Warm Spot native one wire, eight exact indexed leaf probes, 4096 alternate generations each");

    for candidate_id in [None, Some("991101"), Some("991102")] {
        let f = observe_warm(&mut plain, WarmKeys { candidate_id, ..keys })
            .await?
            .context("structural scope only")?;
        ensure!(
            f.candidate_scope.is_none()
                && f.target_cancel_pending.is_none()
                && f.stop_id.is_none()
                && f.stop_qty.is_none()
                && !f.exact_stop_identity
        );
    }
    for different in [
        ExpectedGeneration {
            generation_id: Uuid::now_v7(),
            ..expected
        },
        ExpectedGeneration {
            process_epoch: Uuid::now_v7(),
            ..expected
        },
        ExpectedGeneration {
            source_audit_id: Uuid::now_v7(),
            ..expected
        },
        ExpectedGeneration { account_scope: "other", ..expected },
        ExpectedGeneration { tenant_id: foreign, ..expected },
    ] {
        ensure!(observe_warm(&mut plain, WarmKeys { binding: different, ..keys })
            .await?
            .is_none());
    }
    ensure!(observe_warm(&mut plain, WarmKeys { position_id: Uuid::now_v7(), ..keys })
        .await?
        .is_none());
    ensure!(observe_warm(&mut plain, WarmKeys { symbol: "ETHUSDT", ..keys })
        .await?
        .is_none());
    sqlx::query(
        "DELETE FROM spot_order_set_sell_symbols
        WHERE (tenant_id,venue_id,account_scope,generation_id,symbol)=($1,'binance:spot',$2,$3,$4)",
    )
    .bind(tenant)
    .bind(ACCOUNT)
    .bind(expected.generation_id)
    .bind(SYMBOL)
    .execute(&pool)
    .await?;
    let incomplete = observe_warm(&mut plain, keys).await?.context("structural owner remains")?;
    ensure!(incomplete.symbol_sell.is_none() && incomplete.leg_sell == Some(11));

    store
        .commit(&batch(IntentKind::Exit, position))
        .await
        .map_err(|e| anyhow::anyhow!("real source invalidates generation: {e:?}"))?;
    ensure!(observe_warm(&mut plain, keys).await?.is_none());
    Ok(())
}
