use proptest::{
    prelude::*,
    test_runner::{Config, RngSeed, TestCaseResult, TestRunner},
};
use uuid::Uuid;

use super::*;

const INSERT: &str = "INSERT INTO balance_transitions
    (transition_id,position_id,tenant_id,venue_id,account_scope,symbol,kind,target_trigger,reason,
     started_at,first_cancel_intent_id,original_target_order_id)
    VALUES($1,$2,$3,'binance-spot','m40-local','BTCUSDT','replace',90,'initial',now(),$4,$5)";

async fn install(pool: &PgPool) -> Result<()> {
    let mut f = Fixture::new(pool).await?;
    f.adapter.apply(migration()).await?;
    f.adapter.unlock().await?;
    Ok(())
}

async fn admit(pool: &PgPool, tenant: Uuid) -> Result<Uuid> {
    let position = Uuid::now_v7();
    let entry = Uuid::now_v7();
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO accounting_state(tenant_id,venue_id,budget_model)
        VALUES($1,'binance-spot','hwm_v1') ON CONFLICT DO NOTHING",
    )
    .bind(tenant)
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE accounting_state SET version=version+1 WHERE tenant_id=$1 AND venue_id='binance-spot'")
        .bind(tenant).execute(&mut *tx).await?;
    sqlx::query(
        "INSERT INTO leg_routes(position_id,tenant_id,venue_id,account_scope,routed_at)
        VALUES($1,$2,'binance-spot','m40-local',now())",
    )
    .bind(position)
    .bind(tenant)
    .execute(&mut *tx)
    .await?;
    sqlx::query("INSERT INTO order_intents
        (intent_id,tenant_id,venue_id,account_scope,client_order_id,position_id,kind,params,created_at)
        VALUES($1,$2,'binance-spot','m40-local',$1::uuid::text,$3,'entry','{}',now())")
        .bind(entry).bind(tenant).bind(position).execute(&mut *tx).await?;
    sqlx::query("SELECT balance_leg_admit($1,'binance-spot','m40-local','BTCUSDT',$2,$3,'2026-11-01',
        10,10,200,5,0.01,'base','fixture-chart',now(),'fixture-hash',1,1.01,100,0.1,12,90,now(),false)")
        .bind(tenant).bind(position).bind(entry).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(position)
}

async fn original_stop(pool: &PgPool, tenant: Uuid, position: Uuid, target: &str) -> Result<()> {
    let intent = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO venue_orders(venue_id,account_scope,symbol,exchange_order_id,tenant_id,
        client_order_id,last_client_order_id,owner,intent_id,position_id,kind,side,order_type,
        orig_qty,stop_price,status,phase,last_checked_at,live_seen_at)
        VALUES('binance-spot','m40-local','BTCUSDT',$1,$2,$3::uuid::text,$3::uuid::text,
        'leg',$3,$4,'stop','SELL','STOP_LOSS',1,90,'NEW','live',now(),now())",
    )
    .bind(target)
    .bind(tenant)
    .bind(intent)
    .bind(position)
    .execute(pool)
    .await?;
    Ok(())
}

async fn cancel(pool: &PgPool, tenant: Uuid, position: Uuid, target: &str) -> Result<Uuid> {
    let intent = Uuid::now_v7();
    sqlx::query("INSERT INTO order_intents
        (intent_id,tenant_id,venue_id,account_scope,client_order_id,position_id,kind,cancels_order_id,params,created_at)
        VALUES($1,$2,'binance-spot','m40-local',$1::uuid::text,$3,'cancel',$4,'{}',now())")
        .bind(intent).bind(tenant).bind(position).bind(target).execute(pool).await?;
    Ok(intent)
}

fn insert(
    transition: Uuid,
    position: Uuid,
    tenant: Uuid,
    cancel: Option<Uuid>,
    target: Option<&str>,
) -> sqlx::query::Query<'_, sqlx::Postgres, sqlx::postgres::PgArguments> {
    sqlx::query(INSERT)
        .bind(transition)
        .bind(position)
        .bind(tenant)
        .bind(cancel)
        .bind(target)
}

fn check_error(error: sqlx::Error, code: &str, detail: &str) {
    let db = error.as_database_error().expect("native PostgreSQL rejection");
    assert_eq!(db.code().as_deref(), Some(code));
    assert!(db.constraint() == Some(detail) || db.message().contains(detail), "{db:?}");
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn migration40_refuses_first_cancel_target_on_another_leg(pool: PgPool) -> Result<()> {
    install(&pool).await?;
    let tenant = Uuid::now_v7();
    let selected = admit(&pool, tenant).await?;
    let other = admit(&pool, tenant).await?;
    original_stop(&pool, tenant, selected, "301").await?;
    original_stop(&pool, tenant, other, "302").await?;
    let good = cancel(&pool, tenant, selected, "301").await?;
    let wrong = cancel(&pool, tenant, selected, "302").await?;
    check_error(
        insert(Uuid::now_v7(), selected, tenant, Some(wrong), Some("302"))
            .execute(&pool)
            .await
            .expect_err("migration40_refuses_first_cancel_target_on_another_leg"),
        "23514",
        "first cancel must target the transition leg's original stop",
    );
    let transition = Uuid::now_v7();
    insert(transition, selected, tenant, Some(good), Some("301"))
        .execute(&pool)
        .await?;
    let saved: (Uuid, String) = sqlx::query_as(
        "SELECT first_cancel_intent_id,original_target_order_id
        FROM balance_transitions WHERE transition_id=$1",
    )
    .bind(transition)
    .fetch_one(&pool)
    .await?;
    assert_eq!(saved, (good, "301".into()));
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn migration40_open_transition_moves_only_forward(pool: PgPool) -> Result<()> {
    install(&pool).await?;
    let tenant = Uuid::now_v7();
    let position = admit(&pool, tenant).await?;
    let id = Uuid::now_v7();
    original_stop(&pool, tenant, position, "303").await?;
    original_stop(&pool, tenant, position, "304").await?;
    let first = cancel(&pool, tenant, position, "303").await?;
    let later = cancel(&pool, tenant, position, "304").await?;
    check_error(
        insert(id, position, tenant, Some(first), None)
            .execute(&pool)
            .await
            .unwrap_err(),
        "23514",
        "balance_transitions_first_cancel_pair",
    );
    check_error(
        insert(id, position, Uuid::now_v7(), None, None)
            .execute(&pool)
            .await
            .unwrap_err(),
        "23503",
        "balance_transitions_leg",
    );
    check_error(sqlx::query("INSERT INTO balance_transitions
        (transition_id,position_id,tenant_id,venue_id,account_scope,symbol,kind,target_trigger,reason,started_at)
        VALUES($1,$2,$3,'binance-spot','m40-local','ETHUSDT','replace',90,'initial',now())")
        .bind(id).bind(position).bind(tenant).execute(&pool).await.unwrap_err(),
        "23503", "balance_transitions_leg");
    check_error(
        insert(id, position, tenant, Some(first), Some("304"))
            .execute(&pool)
            .await
            .unwrap_err(),
        "23514",
        "first cancel must target",
    );
    insert(id, position, tenant, None, None).execute(&pool).await?;
    sqlx::query(
        "UPDATE balance_transitions SET first_cancel_intent_id=$1,original_target_order_id='303',
        kind='flatten',target_trigger=95,reason='upgrade' WHERE transition_id=$2",
    )
    .bind(first)
    .bind(id)
    .execute(&pool)
    .await?;
    for sql in [
        "UPDATE balance_transitions SET started_at=now()+interval '1 second' WHERE transition_id=$1",
        "UPDATE balance_transitions SET reason='rewrite' WHERE transition_id=$1",
        "UPDATE balance_transitions SET target_trigger=94 WHERE transition_id=$1",
        "UPDATE balance_transitions SET kind='reprotect' WHERE transition_id=$1",
        "UPDATE balance_transitions SET first_cancel_intent_id=NULL,original_target_order_id=NULL WHERE transition_id=$1",
        "DELETE FROM balance_transitions WHERE transition_id=$1",
    ] {
        check_error(sqlx::query(sql).bind(id).execute(&pool).await.unwrap_err(), "23514", "ledger invariant:");
    }
    check_error(sqlx::query("UPDATE balance_transitions SET first_cancel_intent_id=$1,original_target_order_id='304'
        WHERE transition_id=$2").bind(later).bind(id).execute(&pool).await.unwrap_err(), "23514", "may only move forward");
    sqlx::query("UPDATE balance_transitions SET completed_at=now(),outcome='fixture' WHERE transition_id=$1")
        .bind(id).execute(&pool).await?;
    check_error(
        sqlx::query("UPDATE balance_transitions SET target_trigger=96 WHERE transition_id=$1")
            .bind(id)
            .execute(&pool)
            .await
            .unwrap_err(),
        "23514",
        "may only move forward",
    );
    let next = Uuid::now_v7();
    insert(next, position, tenant, None, None).execute(&pool).await?;
    check_error(
        sqlx::query("UPDATE balance_transitions SET kind='reprotect' WHERE transition_id=$1")
            .bind(next)
            .execute(&pool)
            .await
            .unwrap_err(),
        "23514",
        "may only move forward",
    );
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn migration40_refuses_two_open_transitions(pool: PgPool) -> Result<()> {
    install(&pool).await?;
    let tenant = Uuid::now_v7();
    let position = admit(&pool, tenant).await?;
    let mut first = pool.begin().await?;
    let mut second = pool.acquire().await?;
    let first_pid: i32 =
        sqlx::query_scalar("SELECT pg_backend_pid()").fetch_one(&mut *first).await?;
    let second_pid: i32 =
        sqlx::query_scalar("SELECT pg_backend_pid()").fetch_one(&mut *second).await?;
    let winner = Uuid::now_v7();
    insert(winner, position, tenant, None, None).execute(&mut *first).await?;
    let contender = insert(Uuid::now_v7(), position, tenant, None, None).execute(&mut *second);
    tokio::pin!(contender);
    let blocked = async {
        loop {
            let is_blocked: bool = sqlx::query_scalar("SELECT $1=ANY(pg_blocking_pids($2))")
                .bind(first_pid)
                .bind(second_pid)
                .fetch_one(&pool)
                .await?;
            if is_blocked {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        Ok::<_, sqlx::Error>(())
    };
    tokio::select! {
        result = &mut contender => {
            result.expect_err("migration40_refuses_two_open_transitions");
            anyhow::bail!("contender failed before the verified blocker");
        },
        seen = timeout(Duration::from_secs(2), blocked) => seen??,
    }
    first.commit().await?;
    check_error(
        timeout(Duration::from_secs(2), contender)
            .await?
            .expect_err("migration40_refuses_two_open_transitions"),
        "23505",
        "idx_balance_transitions_open",
    );
    let rows: Vec<Uuid> =
        sqlx::query_scalar("SELECT transition_id FROM balance_transitions WHERE position_id=$1")
            .bind(position)
            .fetch_all(&pool)
            .await?;
    assert_eq!(rows, [winner]);
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn migration40_generated_concurrent_rank_and_trigger_join(pool: PgPool) -> Result<()> {
    install(&pool).await?;
    let strategy = (0u8..3, 0u8..3, 90i32..300, 90i32..300, any::<bool>());
    let mut runner = TestRunner::new(Config {
        cases: 12,
        rng_seed: RngSeed::Fixed(401936),
        failure_persistence: None,
        ..Config::default()
    });
    let cases = std::cell::RefCell::new(Vec::new());
    runner
        .run(&strategy, |args| {
            cases.borrow_mut().push(args);
            TestCaseResult::Ok(())
        })
        .map_err(|error| anyhow::anyhow!("generating M40 fixtures: {error}"))?;
    let tenant = Uuid::now_v7();
    const KINDS: [&str; 3] = ["replace", "flatten", "exit"];
    const PROPOSE: &str =
        "UPDATE balance_transitions SET target_trigger=greatest(target_trigger,$2),
        kind=CASE WHEN array_position(ARRAY['replace','flatten','exit'],kind)<
             array_position(ARRAY['replace','flatten','exit'],$3::text) THEN $3 ELSE kind END
        WHERE transition_id=$1";
    for (left, right, left_price, right_price, swap_order) in cases.into_inner() {
        let position = admit(&pool, tenant).await?;
        let id = Uuid::now_v7();
        insert(id, position, tenant, None, None).execute(&pool).await?;
        let mut a = pool.begin().await?;
        let mut b = pool.begin().await?;
        let (first, second) = if swap_order {
            ((right, right_price), (left, left_price))
        } else {
            ((left, left_price), (right, right_price))
        };
        sqlx::query(PROPOSE)
            .bind(id)
            .bind(first.1)
            .bind(KINDS[first.0 as usize])
            .execute(&mut *a)
            .await?;
        let update = sqlx::query(PROPOSE)
            .bind(id)
            .bind(second.1)
            .bind(KINDS[second.0 as usize])
            .execute(&mut *b);
        let commit = async {
            tokio::task::yield_now().await;
            a.commit().await
        };
        let (joined, committed) =
            timeout(Duration::from_secs(2), async { tokio::join!(update, commit) }).await?;
        assert_eq!(joined?.rows_affected(), 1);
        committed?;
        b.commit().await?;
        let rows: Vec<(String, i32)> = sqlx::query_as(
            "SELECT kind,target_trigger::integer FROM balance_transitions
            WHERE position_id=$1 AND completed_at IS NULL",
        )
        .bind(position)
        .fetch_all(&pool)
        .await?;
        assert_eq!(rows, [(
            KINDS[left.max(right) as usize].to_owned(),
            left_price.max(right_price)
        )]);
    }
    Ok(())
}
