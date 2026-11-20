//! Exercise the real opted native record_order writer against the accepted
//! independent shadow classification in isolated PostgreSQL.
use anyhow::{ensure, Context, Result};
use robson_domain::OrderSide;
use robson_exec::{
    venue::{OrderPhase, VenueOrderRecord},
    venue_ledger::{LedgerError, LegOrderKind, OrderLink, OrderOwner, SeenAt},
};
use rust_decimal::Decimal;
use sqlx::PgPool;
use uuid::Uuid;

use crate as robsond;
use crate::{
    process_epoch, spot_order_set_store::JournalGeneration, venue_ledger_store::PgVenueLedger,
};

#[allow(dead_code)]
#[path = "../venue_ledger_trades/support.rs"]
mod support;
use support::{at, seen, snapshot, Fixture, SYMBOL};

async fn setup(pool: &PgPool) -> Result<Fixture> {
    setup_tables(pool).await?;
    Ok(Fixture::new(pool).await)
}

async fn setup_tables(pool: &PgPool) -> Result<()> {
    let options = pool.connect_options();
    let mut addresses = tokio::net::lookup_host((options.get_host(), options.get_port())).await?;
    ensure!(addresses.next().is_some_and(|address| address.ip().is_loopback()));
    robson_db::migrate(pool).await?;
    sqlx::raw_sql(include_str!("../spot_order_set_reference_pg/sell_shadow.sql"))
        .execute(pool)
        .await?;
    sqlx::raw_sql(include_str!("../spot_order_set_reference_pg/cancel_shadow.sql"))
        .execute(pool)
        .await?;
    Ok(())
}

async fn journal(
    pool: &PgPool,
    f: &Fixture,
    intent: Uuid,
    client: &str,
    kind: &str,
    position: Option<Uuid>,
    target: Option<&str>,
    not_sent: bool,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO public.order_intents (
            intent_id,tenant_id,venue_id,account_scope,client_order_id,position_id,kind,
            cancels_order_id,params,created_at,not_sent)
         VALUES ($1,$2,'binance:spot',$3,$4,$5,$6,$7,'{}',now(),$8)",
    )
    .bind(intent)
    .bind(f.tenant)
    .bind(&f.scope)
    .bind(client)
    .bind(position)
    .bind(kind)
    .bind(target)
    .bind(not_sent)
    .execute(pool)
    .await?;
    Ok(())
}

async fn generation(pool: &PgPool, f: &Fixture) -> Result<JournalGeneration> {
    Ok(generation_and_start(pool, f).await?.0)
}

async fn generation_and_start(
    pool: &PgPool,
    f: &Fixture,
) -> Result<(JournalGeneration, process_epoch::EpochStart)> {
    let process = process_epoch::start_epoch(pool, f.tenant, "native-order-projection").await?;
    let binding = JournalGeneration {
        tenant_id: f.tenant,
        account_scope: f.scope.clone(),
        generation_id: Uuid::now_v7(),
        source_audit_id: Uuid::now_v7(),
        process_epoch: process.epoch.epoch_id,
    };
    sqlx::query(
        "INSERT INTO public.spot_order_set_generation (
            tenant_id,venue_id,account_scope,generation_id,source_audit_id,process_epoch,accounting_version)
         VALUES ($1,'binance:spot',$2,$3,$4,$5,$6)",
    )
    .bind(f.tenant).bind(&f.scope).bind(binding.generation_id).bind(binding.source_audit_id)
    .bind(binding.process_epoch).bind(f.version(pool).await).execute(pool).await?;
    let receipt: Uuid = sqlx::query_scalar(include_str!("oracle_seed.sql"))
        .bind(binding.tenant_id)
        .bind(&binding.account_scope)
        .bind(binding.generation_id)
        .fetch_one(pool)
        .await?;
    ensure!(receipt == binding.generation_id);
    Ok((binding, process))
}

fn linked(f: &Fixture, order: u64, intent: Uuid) -> OrderLink {
    f.link(
        order,
        OrderOwner::Leg {
            position_id: f.position,
            intent_id: intent,
            kind: LegOrderKind::Sell,
        },
    )
}

async fn version(pool: &PgPool, b: &JournalGeneration) -> Result<i64> {
    Ok(sqlx::query_scalar(
        "SELECT g.accounting_version FROM public.spot_order_set_generation g
         JOIN public.accounting_state a USING (tenant_id,venue_id)
         WHERE (g.tenant_id,g.venue_id,g.account_scope,g.generation_id) = ($1,'binance:spot',$2,$3)
           AND g.accounting_version = a.version",
    )
    .bind(b.tenant_id)
    .bind(&b.account_scope)
    .bind(b.generation_id)
    .fetch_one(pool)
    .await?)
}

async fn exact_reference(pool: &PgPool, b: &JournalGeneration) -> Result<()> {
    const NORMAL_SCOPE: &str =
        "s.tenant_id=$1 AND s.venue_id='binance:spot' AND s.account_scope=$2";
    const STRIP_CLASS: &str = "- ARRAY['scope_tenant','scope_venue','scope_account']";
    let sell_intent = format!(
        "SELECT (to_jsonb(s) {STRIP_CLASS}) ||
         jsonb_build_object('tenant_id',j.tenant_id,'venue_id',j.venue_id,'account_scope',j.account_scope,
         'source_position',j.position_id) AS fact
         FROM sell_shadow_intents s JOIN order_intents j USING(intent_id)
         WHERE j.tenant_id=$1 AND j.venue_id='binance:spot' AND j.account_scope=$2
           AND (s.potential OR s.conflict)"
    );
    let sell_order = format!(
        "SELECT to_jsonb(s) {STRIP_CLASS} AS fact
         FROM sell_shadow_orders s WHERE {NORMAL_SCOPE} AND (s.potential OR s.conflict)"
    );
    let cancel_member = format!(
        "SELECT to_jsonb(s) || jsonb_build_object('position_id',CASE WHEN s.symbol IS NOT NULL THEN s.position_id END, 'source_position',j.position_id)
         AS fact FROM cancel_shadow_members s
         JOIN order_intents j USING (tenant_id,venue_id,account_scope,intent_id) WHERE {NORMAL_SCOPE}"
    );
    for table in [
        "sell_intents",
        "sell_orders",
        "sell_legs",
        "sell_symbols",
        "sell_unbound",
        "cancel_members",
        "cancel_groups",
        "cancel_legs",
        "cancel_unbound",
        "cancel_ungroupable",
    ] {
        let (rhs, active_filter) = match table {
            "sell_intents" => (sell_intent.as_str(), "AND (x.potential OR x.conflict)"),
            "sell_orders" => (sell_order.as_str(), "AND (x.potential OR x.conflict)"),
            "cancel_members" => (cancel_member.as_str(), ""),
            _ => ("", ""),
        };
        let independent;
        let rhs = if rhs.is_empty() {
            let (family, rest) = table.split_once('_').context("shadow family")?;
            let payload = if family == "sell" {
                "to_jsonb(s) - 'potential_sell' || jsonb_build_object('potential_count',s.potential_sell)"
            } else {
                "to_jsonb(s)"
            };
            independent = format!(
                "SELECT {payload} AS fact FROM {family}_shadow_{rest} s WHERE {NORMAL_SCOPE}"
            );
            independent.as_str()
        } else {
            rhs
        };
        let delta = format!(
            "WITH actual AS (
                SELECT to_jsonb(x)-'generation_id' AS fact FROM spot_order_set_{table} x
                WHERE x.tenant_id=$1 AND x.venue_id='binance:spot' AND x.account_scope=$2
                  AND x.generation_id=$3 {active_filter}
             ), expected AS ({rhs})
             SELECT (SELECT count(*) FROM (SELECT fact FROM actual EXCEPT SELECT fact FROM expected) extra),
                    (SELECT count(*) FROM (SELECT fact FROM expected EXCEPT SELECT fact FROM actual) missing)"
        );
        let counts: (i64, i64) = sqlx::query_as(&delta)
            .bind(b.tenant_id)
            .bind(&b.account_scope)
            .bind(b.generation_id)
            .fetch_one(pool)
            .await?;
        ensure!(counts == (0, 0), "{table} extra/missing={counts:?}");
    }
    // Active-state normalization must never hide nonzero inactive flags or
    // a fictitious active scope on a stored SELL tombstone.
    let malformed: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM (
            SELECT scope_kind,potential,conflict FROM spot_order_set_sell_intents WHERE generation_id=$1
            UNION ALL
            SELECT scope_kind,potential,conflict FROM spot_order_set_sell_orders WHERE generation_id=$1
         ) m WHERE (scope_kind='none')=(potential OR conflict)",
    )
    .bind(b.generation_id).fetch_one(pool).await?;
    ensure!(malformed == 0);
    Ok(())
}

fn later(second: i64) -> SeenAt {
    SeenAt {
        at: at() + chrono::Duration::seconds(second),
        ..seen()
    }
}

async fn write(
    pool: &PgPool,
    ledger: &PgVenueLedger,
    b: &JournalGeneration,
    order: &VenueOrderRecord,
    link: &OrderLink,
    seen: SeenAt,
) -> Result<()> {
    let before = version(pool, b).await?;
    let commands = ledger.statements_total();
    ensure!(ledger.record_order(order, link, seen).await?.changed);
    ensure!(ledger.statements_total() - commands <= 9);
    ensure!(version(pool, b).await? == before + 1);
    exact_reference(pool, b).await
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn projected_native_order_keeps_reference_membership_and_cancel_masks(
    pool: PgPool,
) -> Result<()> {
    let f = setup(&pool).await?;
    let exits: Vec<_> = (1..=4).map(|_| Uuid::now_v7()).collect();
    for (idx, intent) in exits.iter().enumerate() {
        let order = idx + 1;
        let client = if order == 2 {
            "different-client".to_owned()
        } else {
            format!("order-{order}")
        };
        journal(&pool, &f, *intent, &client, "exit", Some(f.position), None, order == 4).await?;
    }
    let cancels: Vec<_> = (5..=8).map(|_| Uuid::now_v7()).collect();
    for (idx, (target, position)) in [
        (Some("1"), Some(f.position)),
        (Some("6"), Some(f.position)),
        (None, Some(f.position)),
        (Some("999"), None),
    ]
    .into_iter()
    .enumerate()
    {
        journal(
            &pool,
            &f,
            cancels[idx],
            &format!("order-{}", idx + 5),
            "cancel",
            position,
            target,
            false,
        )
        .await?;
    }
    sqlx::query(
        "INSERT INTO public.order_intents (
            intent_id,tenant_id,venue_id,account_scope,client_order_id,position_id,kind,
            cancels_order_id,params,created_at)
         SELECT md5($2||'bulk-cancel'||i)::uuid,$1,'binance:spot',$2,'bulk-cancel-'||i,$3,
                'cancel','1','{}',now()
           FROM generate_series(1,256) i",
    )
    .bind(f.tenant)
    .bind(&f.scope)
    .bind(f.position)
    .execute(&pool)
    .await?;
    let native = f.ledger(&pool);
    let discovered = f.record(3, Decimal::ZERO, OrderPhase::Live);
    let unowned = f.link(3, OrderOwner::Discovered);
    ensure!(native.record_order(&discovered, &unowned, seen()).await?.changed);
    let mut same_id_elsewhere = f.record(1, Decimal::ZERO, OrderPhase::Live);
    same_id_elsewhere.symbol = "ETHUSDT".into();
    ensure!(
        native
            .record_order(&same_id_elsewhere, &f.link(1, OrderOwner::Discovered), seen())
            .await?
            .changed
    );

    let b = generation(&pool, &f).await?;
    let ledger = f.ledger(&pool).with_native_order_set_projection(b.clone());
    exact_reference(&pool, &b).await?;
    for (idx, intent) in exits.iter().enumerate() {
        let order = 1 + idx as u64;
        let record = f.record(order, Decimal::ZERO, OrderPhase::Live);
        write(&pool, &ledger, &b, &record, &linked(&f, order, *intent), later(1)).await?;
    }
    let live = f.record(2, Decimal::ONE, OrderPhase::Live);
    write(&pool, &ledger, &b, &live, &linked(&f, 2, exits[1]), later(2)).await?;
    let before = version(&pool, &b).await?;
    let commands = ledger.statements_total();
    ensure!(!ledger.record_order(&live, &linked(&f, 2, exits[1]), later(3)).await?.changed);
    ensure!(ledger.statements_total() - commands == 1 && version(&pool, &b).await? == before);

    sqlx::raw_sql(
        "CREATE TABLE projected_cancel_member_changes (id bigint generated always as identity);
         CREATE FUNCTION count_projected_cancel_member() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN INSERT INTO projected_cancel_member_changes DEFAULT VALUES; RETURN NEW; END $$;
         CREATE TRIGGER counted_cancel_member AFTER INSERT OR UPDATE OR DELETE
         ON spot_order_set_cancel_members FOR EACH ROW EXECUTE FUNCTION count_projected_cancel_member()",
    )
    .execute(&pool).await?;
    let terminal = f.record(1, Decimal::ZERO, OrderPhase::Terminal);
    write(&pool, &ledger, &b, &terminal, &linked(&f, 1, exits[0]), later(4)).await?;
    let rewritten: i64 = sqlx::query_scalar("SELECT count(*) FROM projected_cancel_member_changes")
        .fetch_one(&pool)
        .await?;
    ensure!(rewritten == 0);
    let masked: (i64, i64, bool) = sqlx::query_as(
        "SELECT pending_count,conflict_count,terminal_mask FROM spot_order_set_cancel_groups
         WHERE generation_id=$1 AND symbol=$2 AND position_id=$3 AND target_id='1'",
    )
    .bind(b.generation_id)
    .bind(SYMBOL)
    .bind(f.position)
    .fetch_one(&pool)
    .await?;
    ensure!(masked == (257, 0, true));
    for (idx, intent) in cancels.iter().enumerate() {
        let order = 5 + idx as u64;
        let record = f.record(order, Decimal::ZERO, OrderPhase::Live);
        write(&pool, &ledger, &b, &record, &linked(&f, order, *intent), later(5)).await?;
    }
    let masked: (i64, i64, bool) = sqlx::query_as(
        "SELECT pending_count,conflict_count,terminal_mask FROM spot_order_set_cancel_groups
         WHERE generation_id=$1 AND symbol=$2 AND position_id=$3 AND target_id='1'",
    )
    .bind(b.generation_id)
    .bind(SYMBOL)
    .bind(f.position)
    .fetch_one(&pool)
    .await?;
    ensure!(masked == (257, 1, true));

    let mut buy = f.record(9, Decimal::ZERO, OrderPhase::Live);
    buy.side = OrderSide::Buy;
    write(&pool, &ledger, &b, &buy, &f.link(9, OrderOwner::Discovered), later(6)).await?;
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn projected_native_order_checks_exact_prior_and_rolls_back_rejected_changes(
    pool: PgPool,
) -> Result<()> {
    let f = setup(&pool).await?;
    let exit = Uuid::now_v7();
    journal(&pool, &f, exit, "order-1", "exit", Some(f.position), None, false).await?;
    let link = linked(&f, 1, exit);
    let live = f.record(1, Decimal::ZERO, OrderPhase::Live);
    ensure!(f.ledger(&pool).record_order(&live, &link, seen()).await?.changed);
    let cancel = Uuid::now_v7();
    journal(&pool, &f, cancel, "pending-op", "cancel", Some(f.position), Some("1"), false).await?;
    let b = generation(&pool, &f).await?;
    let ledger = f.ledger(&pool).with_native_order_set_projection(b.clone());
    exact_reference(&pool, &b).await?;

    let terminal = f.record(1, Decimal::ONE, OrderPhase::Terminal);
    let before = snapshot(&pool).await;
    let v = version(&pool, &b).await?;
    for (fault,repair) in [
        (
            "DELETE FROM spot_order_set_sell_orders WHERE exchange_order_id='1'",
            "INSERT INTO spot_order_set_sell_orders (
                tenant_id,venue_id,account_scope,generation_id,symbol,exchange_order_id,scope_kind,
                potential,conflict,scope_symbol,scope_position)
             SELECT g.tenant_id,g.venue_id,g.account_scope,g.generation_id,o.symbol,o.exchange_order_id,
                    'leg',true,false,o.symbol,o.position_id
             FROM spot_order_set_generation g JOIN venue_orders o USING (tenant_id,venue_id,account_scope)
             WHERE o.exchange_order_id='1'",
        ),
        (
            "UPDATE spot_order_set_sell_orders SET conflict=true WHERE exchange_order_id='1'",
            "UPDATE spot_order_set_sell_orders SET conflict=false WHERE exchange_order_id='1'",
        ),
        (
            "UPDATE spot_order_set_sell_orders SET scope_symbol='NOT_THE_LEG' WHERE exchange_order_id='1'",
            "UPDATE spot_order_set_sell_orders SET scope_symbol=symbol WHERE exchange_order_id='1'",
        ),
        (
            "UPDATE spot_order_set_cancel_groups SET terminal_mask=true WHERE target_id='1'",
            "UPDATE spot_order_set_cancel_groups SET terminal_mask=false WHERE target_id='1'",
        ),
        (
            "UPDATE spot_order_set_sell_legs SET potential_count=0",
            "UPDATE spot_order_set_sell_legs SET potential_count=1",
        ),
        (
            "CREATE FUNCTION refuse_projected_update() RETURNS trigger LANGUAGE plpgsql AS $$
              BEGIN RETURN NULL; END $$;
             CREATE TRIGGER refuse_projected_update BEFORE INSERT OR UPDATE OR DELETE
                 ON spot_order_set_sell_orders FOR EACH ROW EXECUTE FUNCTION refuse_projected_update()",
            "DROP TRIGGER refuse_projected_update ON spot_order_set_sell_orders",
        ),
    ] {
        sqlx::raw_sql(fault).execute(&pool).await?;
        let projected_before = projection_snapshot(&pool,&b).await?;
        ensure!(matches!(
            ledger.record_order(&terminal,&link,later(1)).await,
            Err(LedgerError::Invariant(_))
        ), "fault did not refuse {fault}");
        ensure!(snapshot(&pool).await==before && version(&pool,&b).await?==v);
        ensure!(projection_snapshot(&pool,&b).await? == projected_before);
        sqlx::raw_sql(repair).execute(&pool).await?;
        exact_reference(&pool,&b).await?;
    }
    write(&pool, &ledger, &b, &terminal, &link, later(2)).await?;
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn projected_native_order_commit_loss_has_one_atomic_publication(pool: PgPool) -> Result<()> {
    use robson_testkit::db_proxy::ProxyMode;
    let f = setup(&pool).await?;
    let b = generation(&pool, &f).await?;
    let v = version(&pool, &b).await?;
    let (proxy, proxied) = support::through_proxy(&pool, 1).await;
    let (pid, birth): (i32, String) = sqlx::query_as(
        "SELECT pid,backend_start::text FROM pg_stat_activity WHERE pid=pg_backend_pid()",
    )
    .fetch_one(&proxied)
    .await?;
    let ledger = f.ledger(&proxied).with_native_order_set_projection(b.clone());
    let order = f.record(1, Decimal::ZERO, OrderPhase::Live);
    let link = f.link(1, OrderOwner::Discovered);
    proxy.enable_exact_query_trace(16);
    proxy.set_mode(ProxyMode::DropCommitAckOf("INSERT INTO venue_orders"));
    ensure!(matches!(
        ledger.record_order(&order, &link, seen()).await,
        Err(LedgerError::Uncertain(_))
    ));
    let trace = proxy.query_trace();
    ensure!(proxy.stats().commit_acks_dropped == 1);
    ensure!(trace.len() == 8 && trace.last().is_some_and(|cmd| cmd == "COMMIT"), "{trace:?}");
    ensure!(trace.iter().filter(|cmd| cmd.contains("spot_order_set_generation")).count() == 1);

    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let original: bool=sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM pg_stat_activity
                                WHERE pid=$1 AND backend_start::text=$2 AND datname=current_database())",
            )
            .bind(pid).bind(&birth).fetch_one(&pool).await?;
            if !original {break;}
            tokio::time::sleep(std::time::Duration::from_millis(3)).await;
        }
        Ok::<_,sqlx::Error>(())
    }).await??;
    ensure!(version(&pool, &b).await? == v + 1);
    exact_reference(&pool, &b).await
}

/// A coordinate exists only after a positively confirmed COMMIT of a changed,
/// leg-owned projected write. It is not an observation ACK and clears nothing.
#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn projected_native_order_commit_coordinate_only_follows_confirmed_commit(
    pool: PgPool,
) -> Result<()> {
    use robson_testkit::db_proxy::ProxyMode;
    let f = setup(&pool).await?;
    let exits = [Uuid::now_v7(), Uuid::now_v7()];
    journal(&pool, &f, exits[0], "order-1", "exit", Some(f.position), None, false).await?;
    journal(&pool, &f, exits[1], "order-2", "exit", Some(f.position), None, false).await?;
    let b = generation(&pool, &f).await?;
    exact_reference(&pool, &b).await?;
    let (proxy, proxied) = support::through_proxy(&pool, 1).await;
    let ledger = f.ledger(&proxied).with_native_order_set_projection(b.clone());

    let first_link = linked(&f, 1, exits[0]);
    let original = f.record(1, Decimal::ZERO, OrderPhase::Live);
    write(&pool, &ledger, &b, &original, &first_link, seen()).await?;
    // Linked SELL execution updates the order and consumed leg: nine commands.
    let executed = Decimal::new(25, 2);
    let first = f.record(1, executed, OrderPhase::Live);
    let v = version(&pool, &b).await?;
    let commands = ledger.statements_total();
    proxy.enable_exact_query_trace(16);
    let (result, commit) = ledger.record_order_observed(&first, &first_link, later(1)).await?;
    let trace = proxy.query_trace();
    ensure!(result.changed && result.anomalies.is_empty());
    ensure!(ledger.statements_total() - commands == 9);
    ensure!(trace.len() == 9 && trace.last().is_some_and(|cmd| cmd == "COMMIT"), "{trace:?}");
    ensure!(trace.iter().filter(|cmd| cmd.contains("spot_order_set_generation")).count() == 1);
    ensure!(version(&pool, &b).await? == v + 1);
    let commit = commit.context("confirmed projected leg write has a coordinate")?;
    ensure!(commit.key() == &f.key(1));
    ensure!(commit.owner() == &first_link.owner);
    ensure!(commit.generation() == &b);
    ensure!(commit.executed_cum() == executed);
    ensure!(commit.accounting_version() == v + 1);
    exact_reference(&pool, &b).await?;

    // The same record at the exact same SeenAt is one read and no coordinate.
    let commands = ledger.statements_total();
    proxy.enable_exact_query_trace(16);
    let (result, commit) = ledger.record_order_observed(&first, &first_link, later(1)).await?;
    let trace = proxy.query_trace();
    ensure!(!result.changed && commit.is_none());
    ensure!(ledger.statements_total() - commands == 1 && trace.len() == 1, "{trace:?}");
    ensure!(version(&pool, &b).await? == v + 1);

    let (pid, birth): (i32, String) = sqlx::query_as(
        "SELECT pid,backend_start::text FROM pg_stat_activity WHERE pid=pg_backend_pid()",
    )
    .fetch_one(&proxied)
    .await?;
    let second = f.record(2, Decimal::ZERO, OrderPhase::Live);
    let second_link = linked(&f, 2, exits[1]);
    let commands = ledger.statements_total();
    proxy.enable_exact_query_trace(16);
    proxy.set_mode(ProxyMode::DropCommitAckOf("INSERT INTO venue_orders"));
    ensure!(matches!(
        ledger.record_order_observed(&second, &second_link, later(2)).await,
        Err(LedgerError::Uncertain(_))
    ));
    let trace = proxy.query_trace();
    ensure!(proxy.stats().commit_acks_dropped == 1);
    ensure!(ledger.statements_total() - commands == 8);
    ensure!(trace.len() == 8 && trace.last().is_some_and(|cmd| cmd == "COMMIT"), "{trace:?}");
    ensure!(trace.iter().filter(|cmd| cmd.contains("spot_order_set_generation")).count() == 1);

    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let original: bool=sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM pg_stat_activity
                                WHERE pid=$1 AND backend_start::text=$2 AND datname=current_database())",
            )
            .bind(pid).bind(&birth).fetch_one(&pool).await?;
            if !original {break;}
            tokio::time::sleep(std::time::Duration::from_millis(3)).await;
        }
        Ok::<_,sqlx::Error>(())
    }).await??;
    // The lost acknowledgement still landed exactly once.
    ensure!(version(&pool, &b).await? == v + 2);
    let landed: (i64, Option<Uuid>) = sqlx::query_as(
        "SELECT count(*),min(intent_id::text)::uuid FROM venue_orders
         WHERE tenant_id=$1 AND account_scope=$2 AND symbol=$3 AND exchange_order_id='2'",
    )
    .bind(f.tenant)
    .bind(&f.scope)
    .bind(SYMBOL)
    .fetch_one(&pool)
    .await?;
    ensure!(landed == (1, Some(exits[1])));
    exact_reference(&pool, &b).await?;
    eprintln!("C8_COMMIT_COORDINATE confirmed=1 observed_execution=0.25 changed_sql=9 replay_sql=1 lost_ack_sql=8 lost_ack_coordinate=0");
    Ok(())
}

#[cfg(feature = "crash-points")]
#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn projected_native_order_pending_ack_preserves_newer_execution(pool: PgPool) -> Result<()> {
    use std::{sync::Arc, time::Duration};

    use robson_exec::{
        gate::{EpochView, PendingExecutionEvidence},
        order_journal::OrderJournal,
        venue_clock::NativeClock,
    };
    use robson_testkit::db_proxy::ProxyMode;
    use tokio::sync::oneshot;

    use crate::order_journal_store::PgJournalBackend;
    use super::pending_execution::OrderExecutionReceiver;

    let f = setup(&pool).await?;
    let intent = Uuid::now_v7();
    journal(&pool, &f, intent, "order-1", "exit", Some(f.position), None, false).await?;
    let (binding, start) = generation_and_start(&pool, &f).await?;
    let (proxy, proxied) = support::through_proxy(&pool, 1).await;
    let ledger = f.ledger(&proxied).with_native_order_set_projection(binding.clone());
    let link = linked(&f, 1, intent);
    let initial = f.record(1, Decimal::ZERO, OrderPhase::Live);
    write(&pool, &ledger, &binding, &initial, &link, seen()).await?;
    let initial_version = version(&pool, &binding).await?;
    let journal = OrderJournal::new(
        Arc::new(PgJournalBackend::new(&pool, f.tenant)), &link.venue,
    );
    let gate = journal.gate();
    gate.begin_epoch(
        EpochView {
            epoch_seq: start.epoch.epoch_seq,
            unclean_reason: start.predecessor.unclean_reason(),
        },
        start.unclean_generation,
        start.open_incidents,
    );
    let receiver = OrderExecutionReceiver::bind(gate, &ledger, f.key(1), link)?;
    let clock = NativeClock::initialize()?;
    let sample = Uuid::now_v7();
    let observe = |executed| -> Result<PendingExecutionEvidence> {
        let send = clock.wall_bracket(binding.process_epoch)?;
        let receive = clock.wall_bracket(binding.process_epoch)?;
        Ok(PendingExecutionEvidence { sample, send, receive, executed })
    };
    let readback = || async {
        sqlx::query_as::<_, (Decimal, Uuid)>(
            "SELECT executed_cum, intent_id FROM venue_orders
             WHERE tenant_id=$1 AND venue_id='binance:spot' AND account_scope=$2
               AND symbol=$3 AND exchange_order_id='1' AND owner='leg'",
        ).bind(f.tenant).bind(&f.scope).bind(SYMBOL).fetch_one(&pool).await
    };

    let first = observe(Decimal::new(25, 2))?;
    let work = receiver.register(f.record(1, first.executed, OrderPhase::Live), first)?.context("first ticket")?;
    ensure!(receiver.pending() == Some(first), "registered before any persistence await");

    // Hold delivery in the application after a real COMMIT, with no
    // transaction, database connection or operation gate lock kept by ACK.
    let (ack_ready, committed) = oneshot::channel();
    let (allow_ack, resume_ack) = oneshot::channel();
    let complete_old = async {
        let before = ledger.statements_total();
        let ack = work.persist().await?.context("first changed commit receipt")?;
        ensure!(ledger.statements_total() - before == 9);
        ensure!(ack.accounting_version == initial_version + 1);
        ack_ready.send(()).map_err(|_| anyhow::anyhow!("ACK controller missing"))?;
        resume_ack.await?;
        ensure!(!ack.apply(), "old proof cannot retire a newer observation");
        Ok::<_, anyhow::Error>(())
    };
    let register_new = async {
        committed.await?;
        ensure!(readback().await? == (first.executed, intent));
        let second = observe(Decimal::new(50, 2))?;
        let work = receiver.register(f.record(1, second.executed, OrderPhase::Live), second)?.context("second ticket")?;
        ensure!(receiver.pending() == Some(second));
        allow_ack.send(()).map_err(|_| anyhow::anyhow!("ACK waiter missing"))?;
        Ok::<_, anyhow::Error>((second, work))
    };
    let (old, new) = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(complete_old, register_new)
    }).await?;
    old?;
    let (second, work) = new?;
    ensure!(receiver.pending() == Some(second), "ACK1 preserves 0.50 and original marks");
    ensure!(readback().await? == (first.executed, intent), "no early persistence of 0.50");

    let before = ledger.statements_total();
    let ack = work.persist().await?.context("newer changed commit receipt")?;
    ensure!(ledger.statements_total() - before == 9);
    ensure!(ack.accounting_version == initial_version + 2);
    ensure!(readback().await? == (second.executed, intent));
    ensure!(receiver.pending() == Some(second), "only applying the exact ACK retires it");
    let invalid = observe(Decimal::new(55, 2))?;
    let mut negative_quote = f.record(1, invalid.executed, OrderPhase::Live);
    negative_quote.cumulative_quote_qty = Some(-Decimal::ONE);
    ensure!(matches!(receiver.register(negative_quote, invalid), Err(LedgerError::InvalidEvidence(_))));
    ensure!(receiver.pending() == Some(second) && ack.apply() && receiver.pending().is_none(),
        "invalid newer quote preserves the exact valid pending ticket");

    let a = observe(Decimal::new(55, 2))?;
    let b = observe(Decimal::new(60, 2))?;
    let a_ticket = receiver.register(f.record(1, a.executed, OrderPhase::Live), a)?.context("first capacity ticket")?;
    let b_ticket = receiver.register(f.record(1, b.executed, OrderPhase::Live), b)?.context("second capacity ticket")?;
    let retained = observe(Decimal::new(65, 2))?;
    let mut complete_record = f.record(1, retained.executed, OrderPhase::Live);
    complete_record.cumulative_quote_qty = Some(Decimal::new(65, 0));
    complete_record.update_time_ms = Some(417013);
    ensure!(receiver.register(complete_record, retained)?.is_none());
    ensure!(receiver.pending() == Some(retained) && receiver.take_retained()?.is_none());
    drop(a_ticket);
    drop(b_ticket);
    let retained_work = receiver.take_retained()?.context("retained original payload")?;
    ensure!(receiver.take_retained()?.is_none(), "retained work issued exactly once");
    let before = ledger.statements_total();
    let ack = retained_work.persist().await?.context("retained exact PG commit")?;
    ensure!(ledger.statements_total() - before == 9 && ack.accounting_version == initial_version + 3);
    let durable: (Decimal, Decimal, i64) = sqlx::query_as(
        "SELECT executed_cum,cumulative_quote,venue_update_time_ms FROM venue_orders
         WHERE tenant_id=$1 AND venue_id='binance:spot' AND account_scope=$2
           AND symbol=$3 AND exchange_order_id='1' AND intent_id=$4",
    ).bind(f.tenant).bind(&f.scope).bind(SYMBOL).bind(intent).fetch_one(&pool).await?;
    ensure!(durable == (retained.executed, Decimal::new(65, 0), 417013));
    ensure!(ack.apply() && receiver.pending().is_none());

    let same = observe(retained.executed)?;
    let before = ledger.statements_total();
    let replay = receiver.register(f.record(1, same.executed, OrderPhase::Live), same)?.context("same ticket")?;
    ensure!(replay.persist().await?.is_none() && ledger.statements_total() - before == 1);
    ensure!(receiver.pending() == Some(same), "no-op returned no proof and preserves pending");

    let last = observe(Decimal::new(75, 2))?;
    let unknown = receiver.register(f.record(1, last.executed, OrderPhase::Live), last)?.context("last ticket")?;
    let before = ledger.statements_total();
    proxy.enable_exact_query_trace(16);
    proxy.set_mode(ProxyMode::DropCommitAckOf("UPDATE venue_orders"));
    ensure!(matches!(unknown.persist().await, Err(LedgerError::Uncertain(_))));
    ensure!(ledger.statements_total() - before == 9 && proxy.stats().commit_acks_dropped == 1);
    ensure!(receiver.pending() == Some(last), "lost native reply cannot clear pending");
    ensure!(readback().await? == (last.executed, intent));
    ensure!(version(&pool, &binding).await? == initial_version + 4);
    exact_reference(&pool, &binding).await?;
    ensure!(gate.fenced(), "this recovery proof does not open admissions");
    eprintln!("C8_EXACT_PENDING_ACK previous=0.25 retained=0.50 cleared_by_exact_ack=1 no_op_retained=1 lost_ack_retained=0.75 changed_sql=9 replay_sql=1 invalid_quote_refused=1 saturated_payload_resumed=1");
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn projected_native_order_concurrent_changes_keep_versions_and_memberships_together(
    pool: PgPool,
) -> Result<()> {
    use std::time::Duration;

    use proptest::{
        strategy::{Strategy, ValueTree},
        test_runner::{Config, RngSeed, TestRunner},
    };
    let f = setup(&pool).await?;
    let b = generation(&pool, &f).await?;
    let initial = version(&pool, &b).await?;
    let ledger = f.ledger(&pool).with_native_order_set_projection(b.clone());
    let mut runner = TestRunner::new(Config {
        rng_seed: RngSeed::Fixed(0x51aed5a1),
        failure_persistence: None,
        ..Config::default()
    });
    let strategy = (0_u64..=2000, 0_u64..=2000, proptest::bool::ANY);
    let mut landed = 0_i64;
    let observe = async || -> Result<i64> {
        let (accounting,generation,total,potential): (i64,i64,i64,i64)=sqlx::query_as(
            "SELECT a.version,g.accounting_version,
                (SELECT count(*) FROM venue_orders o WHERE(o.tenant_id,o.venue_id,o.account_scope)=(g.tenant_id,g.venue_id,g.account_scope)),
                (SELECT coalesce(sum(s.potential_count),0)::bigint FROM spot_order_set_sell_symbols s
                  WHERE (s.tenant_id,s.venue_id,s.account_scope,s.generation_id)=(g.tenant_id,g.venue_id,g.account_scope,g.generation_id))
             FROM spot_order_set_generation g JOIN accounting_state a USING(tenant_id,venue_id)
             WHERE g.generation_id=$1",
        )
        .bind(b.generation_id).fetch_one(&pool).await?;
        ensure!(accounting == generation && generation == initial + total && total == potential);
        Ok(total)
    };
    for i in 0..20 {
        let (a_delay, b_delay, duplicate) =
            strategy.new_tree(&mut runner).expect("bounded schedule").current();
        ensure!(observe().await? == landed);
        let writer = async |id, delay| {
            tokio::time::sleep(Duration::from_micros(delay)).await;
            ledger
                .record_order(
                    &f.record(id, Decimal::ZERO, OrderPhase::Live),
                    &f.link(id, OrderOwner::Discovered),
                    later(1),
                )
                .await
        };
        let reader = async {
            for _ in 0..3 {
                observe().await?;
                tokio::task::yield_now().await;
            }
            Ok::<_, anyhow::Error>(())
        };
        let (left, right, checked) = tokio::join!(
            writer(2 * i + 1, a_delay),
            writer(2 * i + 1 + u64::from(!duplicate), b_delay),
            reader
        );
        checked?;
        let newly_landed = i64::from(left?.changed) + i64::from(right?.changed);
        ensure!(newly_landed == if duplicate { 1 } else { 2 });
        landed += newly_landed;
        ensure!(observe().await? == landed);
        exact_reference(&pool, &b).await?;
    }
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn projected_native_order_requires_inactive_cancel_and_exact_journal_owner(
    pool: PgPool,
) -> Result<()> {
    let f = setup(&pool).await?;
    let cancel = Uuid::now_v7();
    journal(&pool, &f, cancel, "order-1", "cancel", Some(f.position), Some("1"), true).await?;
    let exit = Uuid::now_v7();
    journal(&pool, &f, exit, "order-2", "exit", Some(f.position), None, false).await?;
    let b = generation(&pool, &f).await?;
    let ledger = f.ledger(&pool).with_native_order_set_projection(b.clone());
    exact_reference(&pool, &b).await?;
    let before = snapshot(&pool).await;
    let v = version(&pool, &b).await?;

    sqlx::query("DELETE FROM spot_order_set_cancel_members WHERE intent_id=$1")
        .bind(cancel)
        .execute(&pool)
        .await?;
    ensure!(matches!(
        ledger
            .record_order(
                &f.record(1, Decimal::ZERO, OrderPhase::Live),
                &linked(&f, 1, cancel),
                seen()
            )
            .await,
        Err(LedgerError::Invariant(_))
    ));
    ensure!(snapshot(&pool).await == before && version(&pool, &b).await? == v);
    sqlx::query(
        "INSERT INTO spot_order_set_cancel_members (
          tenant_id,venue_id,account_scope,generation_id,intent_id,symbol,position_id,source_position,target_id,pending,conflict)
         SELECT tenant_id,venue_id,account_scope,$2,intent_id,s.symbol,s.position_id,j.position_id,s.target_id,s.pending,s.conflict
         FROM cancel_shadow_members s JOIN order_intents j USING (tenant_id,venue_id,account_scope,intent_id)
         WHERE intent_id=$1",
    )
    .bind(cancel).bind(b.generation_id).execute(&pool).await?;

    sqlx::query("UPDATE spot_order_set_sell_intents SET source_position=$2 WHERE intent_id=$1")
        .bind(exit)
        .bind(Uuid::now_v7())
        .execute(&pool)
        .await?;
    ensure!(matches!(
        ledger
            .record_order(
                &f.record(2, Decimal::ZERO, OrderPhase::Live),
                &linked(&f, 2, exit),
                seen()
            )
            .await,
        Err(LedgerError::Invariant(_))
    ));
    ensure!(snapshot(&pool).await == before && version(&pool, &b).await? == v);
    sqlx::query("UPDATE spot_order_set_sell_intents SET source_position=$2 WHERE intent_id=$1")
        .bind(exit)
        .bind(f.position)
        .execute(&pool)
        .await?;
    exact_reference(&pool, &b).await?;
    write(
        &pool,
        &ledger,
        &b,
        &f.record(1, Decimal::ZERO, OrderPhase::Live),
        &linked(&f, 1, cancel),
        seen(),
    )
    .await?;
    write(
        &pool,
        &ledger,
        &b,
        &f.record(2, Decimal::ZERO, OrderPhase::Live),
        &linked(&f, 2, exit),
        seen(),
    )
    .await
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn projected_native_order_refuses_foreign_journal_and_same_version_obsolete_writer(
    pool: PgPool,
) -> Result<()> {
    let f = setup(&pool).await?;
    let b = generation(&pool, &f).await?;
    let version_before = version(&pool, &b).await?;
    let before = snapshot(&pool).await;
    let ledger = f.ledger(&pool).with_native_order_set_projection(b.clone());
    let foreign = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO order_intents
          (intent_id,tenant_id,venue_id,account_scope,client_order_id,position_id,kind,params,created_at)
         VALUES ($1,$2,'binance:spot','foreign-journal','order-1',$3,'exit','{}',now())",
    )
    .bind(foreign).bind(Uuid::now_v7()).bind(f.position).execute(&pool).await?;
    // The independent oracle deliberately refuses cross-account native linkage
    // too. Disable only those oracle hooks in this isolated refusal witness,
    // so they cannot be what aborts the original native statement.
    sqlx::raw_sql(
        "ALTER TABLE venue_orders DISABLE TRIGGER sell_shadow_venue;
         ALTER TABLE venue_orders DISABLE TRIGGER cancel_shadow_venue",
    )
    .execute(&pool)
    .await?;
    let order = f.record(1, Decimal::ZERO, OrderPhase::Live);
    let foreign_link = linked(&f, 1, foreign);
    ensure!(matches!(
        ledger.record_order(&order,&foreign_link,seen()).await,
        Err(LedgerError::Invariant(reason)) if reason.contains("projection membership refused")
    ));
    ensure!(snapshot(&pool).await == before && version(&pool, &b).await? == version_before);

    let successor = process_epoch::start_epoch(&pool, f.tenant, "native-order-successor").await?;
    ensure!(successor.predecessor_epoch_id == Some(b.process_epoch));
    let still_open: bool =
        sqlx::query_scalar("SELECT clean_stop_at IS NULL FROM process_epochs WHERE epoch_id=$1")
            .bind(b.process_epoch)
            .fetch_one(&pool)
            .await?;
    ensure!(still_open);
    let unowned = f.link(1, OrderOwner::Discovered);
    ensure!(matches!(
        ledger.record_order(&order,&unowned,seen()).await,
        Err(LedgerError::Invariant(reason)) if reason.contains("projection membership refused")
    ));
    ensure!(snapshot(&pool).await == before && version(&pool, &b).await? == version_before);
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn projected_native_order_caller_abort_revokes_held_native_writer(
    pool: PgPool,
) -> Result<()> {
    use std::time::Duration;

    let f = setup(&pool).await?;
    let b = generation(&pool, &f).await?;
    let prior = version(&pool, &b).await?;
    let before = snapshot(&pool).await;
    sqlx::raw_sql(
        "CREATE FUNCTION hold_native_order_projection() RETURNS trigger LANGUAGE plpgsql AS $$
             BEGIN PERFORM pg_advisory_xact_lock(719361); RETURN NEW; END $$;
         CREATE TRIGGER hold_native_order_projection AFTER INSERT ON spot_order_set_sell_symbols
         FOR EACH ROW EXECUTE FUNCTION hold_native_order_projection()",
    )
    .execute(&pool)
    .await?;
    let mut guard = pool.acquire().await?;
    sqlx::query("SELECT pg_advisory_lock(719361)").execute(&mut *guard).await?;
    let (proxy, proxied) = support::through_proxy(&pool, 1).await;
    let (pid,birth): (i32,String)=sqlx::query_as(
        "SELECT pg_backend_pid(),backend_start::text FROM pg_stat_activity WHERE pid=pg_backend_pid()",
    ).fetch_one(&proxied).await?;
    let ledger = f.ledger(&proxied).with_native_order_set_projection(b.clone());
    let order = f.record(1, Decimal::ZERO, OrderPhase::Live);
    let link = f.link(1, OrderOwner::Discovered);
    let task = tokio::spawn(async move { ledger.record_order(&order, &link, seen()).await });
    tokio::time::timeout(Duration::from_millis(500), async {
        loop {
            let physically_held: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM pg_stat_activity
                    WHERE pid=$1 AND backend_start::text=$2 AND wait_event='advisory'
                      AND query LIKE '%WITH gen AS MATERIALIZED%')",
            )
            .bind(pid)
            .bind(&birth)
            .fetch_one(&pool)
            .await?;
            if physically_held {
                break;
            }
            tokio::time::sleep(Duration::from_millis(4)).await;
        }
        Ok::<_, sqlx::Error>(())
    })
    .await??;
    ensure!(snapshot(&pool).await == before && version(&pool, &b).await? == prior);
    task.abort();
    ensure!(task.await.unwrap_err().is_cancelled());
    tokio::time::timeout(Duration::from_secs(3),async {
        loop {
            let still: bool=sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM pg_stat_activity WHERE pid=$1 AND backend_start::text=$2)",
            ).bind(pid).bind(&birth).fetch_one(&pool).await?;
            if !still {break;}
            tokio::time::sleep(Duration::from_millis(4)).await;
        }
        Ok::<_,sqlx::Error>(())
    }).await??;
    ensure!(snapshot(&pool).await == before && version(&pool, &b).await? == prior);
    exact_reference(&pool, &b).await?;
    let unlocked: bool = sqlx::query_scalar("SELECT pg_advisory_unlock(719361)")
        .fetch_one(&mut *guard)
        .await?;
    ensure!(unlocked);
    ensure!(proxy.stats().connections >= 1);
    Ok(())
}

async fn projection_snapshot(pool: &PgPool, b: &JournalGeneration) -> Result<Vec<Vec<String>>> {
    let mut tables = Vec::new();
    for name in [
        "generation",
        "sell_intents",
        "sell_orders",
        "sell_legs",
        "sell_symbols",
        "sell_unbound",
        "cancel_members",
        "cancel_groups",
        "cancel_legs",
        "cancel_ungroupable",
        "cancel_unbound",
    ] {
        let states = sqlx::query_scalar::<_, String>(&format!(
            "SELECT row_to_json(q)::text FROM spot_order_set_{name} q
             WHERE q.generation_id=$1 ORDER BY row_to_json(q)::text"
        ))
        .bind(b.generation_id)
        .fetch_all(pool)
        .await?;
        tables.push(states);
    }
    Ok(tables)
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn projected_native_order_checks_old_counts_even_with_zero_or_positive_net_delta(
    pool: PgPool,
) -> Result<()> {
    let f = setup(&pool).await?;
    let original = Uuid::now_v7();
    let arriving = Uuid::now_v7();
    let malformed_cancel = Uuid::now_v7();
    journal(&pool, &f, original, "order-1", "exit", Some(f.position), None, false).await?;
    journal(&pool, &f, arriving, "order-2", "exit", Some(f.position), None, false).await?;
    journal(
        &pool,
        &f,
        malformed_cancel,
        "order-3",
        "cancel",
        Some(f.position),
        Some("1"),
        false,
    )
    .await?;
    let first = f.record(1, Decimal::ZERO, OrderPhase::Live);
    let linked_first = linked(&f, 1, original);
    ensure!(f.ledger(&pool).record_order(&first, &linked_first, seen()).await?.changed);
    let b = generation(&pool, &f).await?;
    let ledger = f.ledger(&pool).with_native_order_set_projection(b.clone());
    exact_reference(&pool, &b).await?;

    let numeric = f.record(1, Decimal::ONE, OrderPhase::Live);
    let fresh = f.record(2, Decimal::ZERO, OrderPhase::Live);
    let named = f.record(3, Decimal::ZERO, OrderPhase::Live);
    let next_link = linked(&f, 2, arriving);
    let cancel_link = linked(&f, 3, malformed_cancel);
    for (fault, repair, order, link) in [
        (
            "UPDATE spot_order_set_sell_legs SET potential_count=0",
            "UPDATE spot_order_set_sell_legs SET potential_count=2",
            &numeric,
            &linked_first,
        ),
        (
            "UPDATE spot_order_set_sell_legs SET potential_count=0",
            "UPDATE spot_order_set_sell_legs SET potential_count=2",
            &fresh,
            &next_link,
        ),
        (
            "UPDATE spot_order_set_cancel_groups SET pending_count=0 WHERE target_id='1'",
            "UPDATE spot_order_set_cancel_groups SET pending_count=1 WHERE target_id='1'",
            &named,
            &cancel_link,
        ),
        (
            "UPDATE spot_order_set_cancel_legs SET active_count=0",
            "UPDATE spot_order_set_cancel_legs SET active_count=1",
            &named,
            &cancel_link,
        ),
    ] {
        sqlx::raw_sql(fault).execute(&pool).await?;
        let projection = projection_snapshot(&pool, &b).await?;
        let native = snapshot(&pool).await;
        ensure!(
            matches!(
                ledger.record_order(order,link,later(1)).await,
                Err(LedgerError::Invariant(reason)) if reason.contains("projection membership refused")
            ),
            "expected prior undercount refusal: {fault}"
        );
        ensure!(
            snapshot(&pool).await == native && projection_snapshot(&pool, &b).await? == projection
        );
        sqlx::raw_sql(repair).execute(&pool).await?;
        exact_reference(&pool, &b).await?;
    }
    write(&pool, &ledger, &b, &numeric, &linked_first, later(2)).await?;
    write(&pool, &ledger, &b, &fresh, &next_link, later(3)).await?;
    write(&pool, &ledger, &b, &named, &cancel_link, later(4)).await
}

#[path = "plan_skew.rs"]
mod plan_skew;

#[path = "entry_partition.rs"]
mod entry_partition;

#[cfg(feature = "crash-points")]
#[path = "restart.rs"]
mod restart;
