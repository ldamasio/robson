//! Native opt-in trade pages publish fills and their projection version
//! together.
use std::time::Duration;

use anyhow::{ensure, Result};
use proptest::{
    strategy::{Strategy, ValueTree},
    test_runner::{Config, RngSeed, TestRunner},
};
use robson_exec::{
    venue::OrderPhase,
    venue_ledger::{LedgerError, OrderKey, OrderOwner},
};
use robson_testkit::db_proxy::ProxyMode;
use rust_decimal::Decimal;
use sqlx::PgPool;
use uuid::Uuid;

use crate as robsond;
use crate::{process_epoch, spot_order_set_store::JournalGeneration};

#[allow(dead_code)]
#[path = "support.rs"]
pub(super) mod support;
use support::{at, page, seen, snapshot, through_proxy, Fixture, SYMBOL};

async fn migrate(pool: &PgPool) -> Result<()> {
    let options = pool.connect_options();
    let mut addresses = tokio::net::lookup_host((options.get_host(), options.get_port())).await?;
    ensure!(addresses.next().is_some_and(|ip| ip.ip().is_loopback()));
    robson_db::migrate(pool).await?;
    Ok(())
}

async fn generation(pool: &PgPool, fixture: &Fixture) -> Result<(JournalGeneration, i64)> {
    let native = fixture.ledger(pool);
    let other = native
        .record_order(
            &fixture.record(2, Decimal::ZERO, OrderPhase::Live),
            &fixture.link(2, OrderOwner::Discovered),
            seen(),
        )
        .await?;
    ensure!(other.changed);
    let version = fixture.version(pool).await;
    let start =
        process_epoch::start_epoch(pool, fixture.tenant, "projected-trades-fixture").await?;
    let binding = JournalGeneration {
        tenant_id: fixture.tenant,
        account_scope: fixture.scope.clone(),
        generation_id: Uuid::now_v7(),
        process_epoch: start.epoch.epoch_id,
        source_audit_id: Uuid::now_v7(),
    };
    sqlx::query(
        "INSERT INTO spot_order_set_generation
            (tenant_id,venue_id,account_scope,generation_id,process_epoch,source_audit_id,accounting_version)
         VALUES ($1,'binance:spot',$2,$3,$4,$5,$6)",
    )
    .bind(binding.tenant_id).bind(&binding.account_scope).bind(binding.generation_id)
    .bind(binding.process_epoch).bind(binding.source_audit_id).bind(version)
    .execute(pool).await?;
    // A leg-owned live SELL without J, a discovered SELL by symbol, an
    // unresolved unbound SELL, and all three native cancel scope shapes.
    sqlx::query(include_str!("projected_seed.sql"))
        .bind(binding.tenant_id)
        .bind(&binding.account_scope)
        .bind(binding.generation_id)
        .bind(fixture.position)
        .bind(SYMBOL)
        .bind(Uuid::now_v7())
        .bind(Uuid::now_v7())
        .bind(Uuid::now_v7())
        .bind(Uuid::now_v7())
        .execute(pool)
        .await?;
    Ok((binding, version))
}

async fn membership(pool: &PgPool) -> Result<Vec<String>> {
    let mut result = Vec::new();
    for table in [
        "spot_order_set_sell_intents",
        "spot_order_set_sell_orders",
        "spot_order_set_sell_legs",
        "spot_order_set_sell_symbols",
        "spot_order_set_sell_unbound",
        "spot_order_set_cancel_members",
        "spot_order_set_cancel_groups",
        "spot_order_set_cancel_legs",
        "spot_order_set_cancel_ungroupable",
        "spot_order_set_cancel_unbound",
    ] {
        let values = sqlx::query_scalar::<_, String>(&format!(
            "SELECT row_to_json(t)::text FROM {table} t ORDER BY row_to_json(t)::text"
        ))
        .fetch_all(pool)
        .await?;
        ensure!(!values.is_empty(), "missing fixture evidence in {table}");
        result.extend(values.into_iter().map(|value| format!("{table}/{value}")));
    }
    Ok(result)
}

async fn observe(pool: &PgPool, fixture: &Fixture, key: &OrderKey) -> Result<(i64, i64, Decimal)> {
    let values: (i64, i64, i64, Decimal, Decimal, Decimal) = sqlx::query_as(
        "SELECT a.version,g.accounting_version,
                (SELECT count(*) FROM order_fills f WHERE (f.venue_id,f.account_scope,f.symbol,f.exchange_order_id)
                       = (o.venue_id,o.account_scope,o.symbol,o.exchange_order_id)),
                o.fills_qty,o.executed_cum,l.consumed_cum
         FROM accounting_state a
         JOIN spot_order_set_generation g USING (tenant_id,venue_id)
         JOIN venue_orders o USING (tenant_id,venue_id,account_scope)
         JOIN balance_legs l USING (tenant_id,venue_id,account_scope,symbol,position_id)
         WHERE a.tenant_id=$1 AND a.venue_id='binance:spot' AND g.account_scope=$2
               AND o.symbol=$3 AND o.exchange_order_id=$4",
    )
    .bind(fixture.tenant).bind(&fixture.scope).bind(&key.symbol)
    .bind(key.exchange_order_id.to_string()).fetch_one(pool).await?;
    ensure!(
        values.0 == values.1 && values.3 == values.4 && values.4 == values.5,
        "{values:?}"
    );
    Ok((values.0, values.2, values.3))
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn projected_trade_peer_cases_keep_atomic_versions_and_membership(
    pool: PgPool,
) -> Result<()> {
    migrate(&pool).await?;
    let f = Fixture::new(&pool).await;
    let key = f.seed_order(&pool, 1, Decimal::ZERO, OrderPhase::Live).await;
    let (binding, version) = generation(&pool, &f).await?;
    let mut committed = 0;
    let original_members = membership(&pool).await?;
    let mut runner = TestRunner::new(Config {
        rng_seed: RngSeed::Fixed(0xfa17ec52),
        failure_persistence: None,
        ..Config::default()
    });
    let schedules = (0_u64..=3000, 0_u64..=3000, proptest::bool::ANY);
    let ledger = f.ledger(&pool).with_trade_order_set_projection(binding);
    for case in 0..24 {
        let (a_wait, b_wait, duplicate) =
            schedules.new_tree(&mut runner).expect("bounded schedule").current();
        ensure!(
            observe(&pool, &f, &key).await?
                == (version + committed, committed, Decimal::from(committed))
        );
        let a = 2 * case + 1;
        let b = a + u64::from(!duplicate);
        let writer = |id, delay| {
            let ledger = &ledger;
            let key = &key;
            async move {
                tokio::time::sleep(Duration::from_micros(delay)).await;
                ledger.record_trades(key, &page(id, Decimal::ONE), at()).await
            }
        };
        let observer = async {
            for _ in 0..3 {
                let (seen_version, fills, qty) = observe(&pool, &f, &key).await?;
                ensure!(seen_version == version + fills);
                ensure!(qty == Decimal::from(fills));
                tokio::task::yield_now().await;
            }
            Ok::<_, anyhow::Error>(())
        };
        let (one, two, reads) = tokio::join!(writer(a, a_wait), writer(b, b_wait), observer);
        reads?;
        let acknowledgements = i64::from(one?.changed) + i64::from(two?.changed);
        ensure!(acknowledgements == if duplicate { 1 } else { 2 }, "seeded case {case}");
        committed += acknowledgements;
        ensure!(
            observe(&pool, &f, &key).await?
                == (version + committed, committed, Decimal::from(committed))
        );
        ensure!(membership(&pool).await? == original_members);
    }
    let before = ledger.statements_total();
    ensure!(!ledger.record_trades(&key, &page(47, Decimal::ONE), at()).await?.changed);
    // BEGIN, the server bounds, one read and ROLLBACK.
    ensure!(ledger.statements_total() - before == 4);
    eprintln!(
        "Projected trade pages: 24 seeded native concurrent schedules and 72 external snapshots"
    );
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn projected_trade_refused_generation_rolls_back_fills_and_ledger(
    pool: PgPool,
) -> Result<()> {
    migrate(&pool).await?;
    let f = Fixture::new(&pool).await;
    let key = f.seed_order(&pool, 1, Decimal::ZERO, OrderPhase::Live).await;
    let (binding, version) = generation(&pool, &f).await?;
    let original_members = membership(&pool).await?;
    let before = snapshot(&pool).await;
    let foreign = f.ledger(&pool).with_trade_order_set_projection(JournalGeneration {
        account_scope: "other-scope".into(),
        ..binding.clone()
    });
    ensure!(matches!(
        foreign.record_trades(&key, &page(1, Decimal::ONE), at()).await,
        Err(LedgerError::Invariant(reason)) if reason.contains("scope mismatch")
    ));
    ensure!(foreign.statements_total() == 0);
    sqlx::raw_sql(
        "CREATE FUNCTION trade_probe_hold_leg() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN PERFORM pg_advisory_xact_lock(719353); RETURN NEW; END $$;
         CREATE TRIGGER trade_probe_hold_leg AFTER UPDATE OF consumed_cum ON balance_legs
         FOR EACH ROW EXECUTE FUNCTION trade_probe_hold_leg()",
    )
    .execute(&pool)
    .await?;
    let mut runner = TestRunner::new(Config {
        rng_seed: RngSeed::Fixed(0x518c9730),
        failure_persistence: None,
        ..Config::default()
    });
    let schedule = (0_u64..=2500, 0_u64..=2500, proptest::bool::ANY);
    for remove_row in [false, true] {
        if remove_row {
            sqlx::query("DELETE FROM spot_order_set_generation WHERE generation_id=$1")
                .bind(binding.generation_id)
                .execute(&pool)
                .await?;
        } else {
            sqlx::query("UPDATE spot_order_set_generation SET accounting_version=accounting_version-1 WHERE generation_id=$1")
                .bind(binding.generation_id).execute(&pool).await?;
        }
        let expected = (!remove_row).then_some(version - 1);
        let inspect =
            async || -> Result<()> {
                let generation_version: Option<i64> = sqlx::query_scalar(
                "SELECT accounting_version FROM spot_order_set_generation WHERE generation_id=$1",
            )
            .bind(binding.generation_id).fetch_optional(&pool).await?;
                ensure!(generation_version == expected);
                ensure!(snapshot(&pool).await == before);
                ensure!(membership(&pool).await? == original_members);
                Ok(())
            };
        for case in 0..8 {
            let (a_delay, b_delay, duplicate) =
                schedule.new_tree(&mut runner).expect("bounded schedule").current();
            inspect().await?;
            let mut guard = pool.acquire().await?;
            sqlx::query("SELECT pg_advisory_lock(719353)").execute(&mut *guard).await?;
            let a = f.ledger(&pool).with_trade_order_set_projection(binding.clone());
            let b = f.ledger(&pool).with_trade_order_set_projection(binding.clone());
            let writer = |ledger: super::PgVenueLedger, id, delay| {
                let key = &key;
                async move {
                    tokio::time::sleep(Duration::from_micros(delay)).await;
                    let outcome = ledger.record_trades(key, &page(id, Decimal::ONE), at()).await;
                    (outcome, ledger.statements_total())
                }
            };
            let witness = async {
                tokio::time::timeout(Duration::from_millis(200), async {
                    loop {
                        let blocked: bool = sqlx::query_scalar(
                            "SELECT EXISTS (SELECT 1 FROM pg_stat_activity
                              WHERE datname=current_database() AND pid<>pg_backend_pid()
                                AND query LIKE 'UPDATE balance_legs SET consumed_cum%'
                                AND wait_event='advisory')",
                        )
                        .fetch_one(&pool)
                        .await?;
                        if blocked {
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(3)).await;
                    }
                    Ok::<_, sqlx::Error>(())
                })
                .await??;
                // At least one transaction already inserted a fill and
                // updated its order and leg while the observer sees the prior state.
                let seen = inspect().await;
                let unlocked: bool = sqlx::query_scalar("SELECT pg_advisory_unlock(719353)")
                    .fetch_one(&mut *guard)
                    .await?;
                ensure!(unlocked);
                seen
            };
            let (left, right, seen) = tokio::join!(
                writer(a, 2 * case + 1, a_delay),
                writer(b, 2 * case + 1 + u64::from(!duplicate), b_delay),
                witness,
            );
            seen?;
            for (result, statements) in [left, right] {
                ensure!(matches!(
                    result,
                    Err(LedgerError::Invariant(reason)) if reason.contains("generation refused")
                ));
                ensure!(statements == 10, "fixture never reached the exact CAS");
            }
            inspect().await?;
        }
    }
    eprintln!("Projected trade refusal: 16 seeded native concurrent cases held before CAS, with witnessed old snapshots");
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn projected_trade_duplicate_replans_after_real_accounting_lock(pool: PgPool) -> Result<()> {
    migrate(&pool).await?;
    let f = Fixture::new(&pool).await;
    let key = f.seed_order(&pool, 1, Decimal::ZERO, OrderPhase::Live).await;
    let (binding, version) = generation(&pool, &f).await?;
    let original_members = membership(&pool).await?;
    let mut barrier = pool.begin().await?;
    sqlx::query("SELECT version FROM accounting_state WHERE tenant_id=$1 AND venue_id='binance:spot' FOR NO KEY UPDATE")
        .bind(f.tenant).execute(&mut *barrier).await?;
    let a = f.ledger(&pool).with_trade_order_set_projection(binding.clone());
    let b = f.ledger(&pool).with_trade_order_set_projection(binding);
    let on_hold = async {
        tokio::time::timeout(Duration::from_millis(230), async {
            loop {
                let waiting: i64 = sqlx::query_scalar(
                    "SELECT count(*) FROM pg_stat_activity
                     WHERE datname=current_database() AND pid<>pg_backend_pid()
                       AND query LIKE 'UPDATE accounting_state SET version=version+1%'
                       AND wait_event_type='Lock'",
                )
                .fetch_one(&pool)
                .await?;
                if waiting == 2 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(3)).await;
            }
            Ok::<_, sqlx::Error>(())
        })
        .await??;
        ensure!(observe(&pool, &f, &key).await? == (version, 0, Decimal::ZERO));
        barrier.commit().await?;
        Ok::<_, anyhow::Error>(())
    };
    let trade = page(1, Decimal::ONE);
    let (left, right, held) = tokio::join!(
        a.record_trades(&key, &trade, at()),
        b.record_trades(&key, &trade, at()),
        on_hold,
    );
    held?;
    let counts = if left?.changed {
        ensure!(!right?.changed);
        [a.statements_total(), b.statements_total()]
    } else {
        ensure!(right?.changed);
        [b.statements_total(), a.statements_total()]
    };
    ensure!(counts == [11, 7], "{counts:?}");
    ensure!(observe(&pool, &f, &key).await? == (version + 1, 1, Decimal::ONE));
    ensure!(membership(&pool).await? == original_members);
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn projected_trade_lost_commit_requires_retirement_then_observes_accepted_version(
    pool: PgPool,
) -> Result<()> {
    migrate(&pool).await?;
    let f = Fixture::new(&pool).await;
    let key = f.seed_order(&pool, 1, Decimal::ZERO, OrderPhase::Live).await;
    let (binding, version) = generation(&pool, &f).await?;
    let original_members = membership(&pool).await?;
    let (proxy, proxied) = through_proxy(&pool, 1).await;
    let (pid, birth): (i32, String) = sqlx::query_as(
        "SELECT pid,backend_start::text FROM pg_stat_activity WHERE pid=pg_backend_pid()",
    )
    .fetch_one(&proxied)
    .await?;
    let ledger = f.ledger(&proxied).with_trade_order_set_projection(binding);
    proxy.enable_exact_query_trace(16);
    proxy.set_mode(ProxyMode::DropCommitAckOf("INSERT INTO order_fills"));
    ensure!(matches!(
        ledger.record_trades(&key, &page(1, Decimal::ONE), at()).await,
        Err(LedgerError::Uncertain(_))
    ));
    let trace = proxy.query_trace();
    ensure!(proxy.stats().commit_acks_dropped == 1);
    ensure!(
        trace.len() == 11 && trace[9].contains("UPDATE public.spot_order_set_generation"),
        "{trace:?}"
    );
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let still: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM pg_stat_activity
                                WHERE pid=$1 AND backend_start::text=$2 AND datname=current_database())",
            ).bind(pid).bind(&birth).fetch_one(&pool).await?;
            if !still {
                break;
            }
            tokio::time::sleep(Duration::from_millis(4)).await;
        }
        Ok::<_,sqlx::Error>(())
    }).await??;
    ensure!(observe(&pool, &f, &key).await? == (version + 1, 1, Decimal::ONE));
    ensure!(membership(&pool).await? == original_members);
    eprintln!("Projected trade page: 11 physical commands, original backend retired after lost COMMIT acknowledgement");
    Ok(())
}

/// ADR-0056 step 8l-3a (PLAN-r4 §6.10): new fills of an order with no
/// governed leg invalidate the UTC month of each fill's venue time, through
/// one idempotent statement on the page's transaction; leg fills add none.
#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn unattributed_fill_invalidates_its_month(pool: PgPool) -> Result<()> {
    use crate::venue_ledger_store::settlement::{
        read_settlement, CommissionAssets, SettlementQuery,
    };
    migrate(&pool).await?;
    let f = Fixture::new(&pool).await;
    let ledger = f.ledger(&pool);
    let last_october: chrono::DateTime<chrono::Utc> = "2026-10-31T23:59:59.999Z".parse()?;
    let fill = |id: u64, time| robson_exec::ports::TradeFill {
        time: Some(time),
        ..support::fill(id, Decimal::ONE)
    };
    let across = robson_exec::venue::TradePage {
        trades: vec![
            fill(71, last_october),
            fill(72, last_october + chrono::TimeDelta::milliseconds(1)),
        ],
        next_from_id: None,
    };
    let record = f.record(7, Decimal::ZERO, OrderPhase::Live);
    ledger.record_order(&record, &f.link(7, OrderOwner::Discovered), seen()).await?;
    let key = f.key(7);
    let before = ledger.statements_total();
    ensure!(ledger.record_trades(&key, &across, at()).await?.changed);
    // BEGIN, the bounds, three reads, the bump, the fills, the invalidation,
    // the order, COMMIT: one more than the same page without it.
    ensure!(ledger.statements_total() - before == 10);
    let invalid = |month: &str| {
        let month = chrono::NaiveDate::parse_from_str(month, "%Y-%m-%d").expect("month");
        let (pool, tenant) = (pool.clone(), f.tenant);
        async move {
            let venue = support::venue();
            let assets = [CommissionAssets {
                symbol: SYMBOL,
                base: "BTC",
                quote: "USDT",
            }];
            let query = SettlementQuery {
                tenant_id: tenant,
                venue: &venue,
                month,
                now: at(),
                assets: &assets,
            };
            Ok::<_, anyhow::Error>(
                read_settlement(&mut *pool.acquire().await?, &query).await?.invalidations,
            )
        }
    };
    ensure!(invalid("2026-10-01").await? == 1 && invalid("2026-11-01").await? == 1);
    ensure!(invalid("2026-09-01").await? == 0 && invalid("2026-12-01").await? == 0);
    let evidence: Vec<String> = sqlx::query_scalar(
        "SELECT evidence_key FROM accounting_invalidations WHERE tenant_id=$1 ORDER BY month",
    )
    .bind(f.tenant)
    .fetch_all(&pool)
    .await?;
    ensure!(
        evidence
            == [
                format!("{}/{SYMBOL}/71", f.scope),
                format!("{}/{SYMBOL}/72", f.scope)
            ]
    );
    // A replay lands nothing; a leg's page adds no invalidation.
    ensure!(!ledger.record_trades(&key, &across, at()).await?.changed);
    let leg = f.seed_order(&pool, 8, Decimal::ZERO, OrderPhase::Live).await;
    let before = ledger.statements_total();
    ensure!(ledger.record_trades(&leg, &page(81, Decimal::ONE), at()).await?.changed);
    ensure!(ledger.statements_total() - before == 10, "a leg page keeps its ten");
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM accounting_invalidations")
        .fetch_one(&pool)
        .await?;
    ensure!(rows == 2);
    // Opted: the generation CAS too, and the generation stays current.
    let g = Fixture::new(&pool).await;
    let (binding, version) = generation(&pool, &g).await?;
    let projected = g.ledger(&pool).with_trade_order_set_projection(binding);
    let before = projected.statements_total();
    ensure!(projected.record_trades(&g.key(2), &page(91, Decimal::ONE), at()).await?.changed);
    ensure!(projected.statements_total() - before == 11);
    let (current, advanced): (i64, i64) = sqlx::query_as(
        "SELECT a.version, s.accounting_version FROM accounting_state a JOIN spot_order_set_generation s USING (tenant_id, venue_id) WHERE a.tenant_id=$1",
    )
    .bind(g.tenant)
    .fetch_one(&pool)
    .await?;
    ensure!(current == version + 1 && advanced == current);
    let rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM accounting_invalidations WHERE tenant_id=$1")
            .bind(g.tenant)
            .fetch_one(&pool)
            .await?;
    ensure!(rows == 1);
    Ok(())
}
