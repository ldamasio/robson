//! Twenty-four generated schedules on a four-worker runtime. The oracle is
//! independent of the planners: it folds fixed typed facts and counts writes.
use proptest::{
    strategy::{Strategy, ValueTree},
    test_runner::TestRunner,
};
use robson_domain::OrderSide;
use robson_exec::{
    order_journal::{CommitFailure, JournalBackend, JournalBatch, OutcomeRow, VenueOutcome},
    venue::{TradePage, VenueOrderRecord},
    venue_ledger::{LegOrderKind, OrderKey, OrderLink},
};
use robsond::{order_journal_store::PgJournalBackend, venue_ledger_store::PgVenueLedger};
use uuid::Uuid;

use super::*;

#[derive(Clone)]
enum Write {
    Journal(Arc<PgJournalBackend>, JournalBatch),
    Order(Arc<PgVenueLedger>, Arc<VenueOrderRecord>, OrderLink),
    Trades(Arc<PgVenueLedger>, OrderKey, Arc<TradePage>),
}
impl Write {
    async fn apply(&self) -> Result<bool, LedgerError> {
        match self {
            Self::Journal(backend, batch) => {
                match tokio::time::timeout(Duration::from_millis(500), backend.commit(batch)).await
                {
                    Ok(Ok(())) => Ok(true),
                    Ok(Err(CommitFailure::Uncertain(s))) => Err(LedgerError::Uncertain(s)),
                    Ok(Err(error)) => {
                        Err(LedgerError::Backend(format!("journal refusal: {error:?}")))
                    },
                    Err(_) => {
                        Err(LedgerError::Uncertain("bounded generated journal commit".into()))
                    },
                }
            },
            Self::Order(ledger, record, link) => {
                ledger.record_order(record, link, seen()).await.map(|result| result.changed)
            },
            Self::Trades(ledger, key, page) => {
                ledger.record_trades(key, page, at()).await.map(|result| result.changed)
            },
        }
    }
}

async fn prepared_journal(pool: &PgPool, tenant: Uuid) -> Arc<PgJournalBackend> {
    let backend = Arc::new(PgJournalBackend::new(pool, tenant));
    backend.check_ready().await.unwrap();
    backend
}

async fn prepared_ledger_pool(pool: &PgPool) {
    tokio::time::timeout(Duration::from_secs(10), async {
        let mut held = Vec::new();
        let mut backends = std::collections::BTreeSet::new();
        for _ in 0..8 {
            let mut connection = pool.acquire().await.unwrap();
            let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(&mut *connection)
                .await
                .unwrap();
            assert!(backends.insert(pid), "setup must hold eight distinct ready connections");
            held.push(connection);
        }
        // Keep every connection checked out until all eight are established.
        drop(held);
    })
    .await
    .expect("generated ledger pool did not become ready before the schedule");
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn cold_journal_startup_precedes_the_generated_operation_budget(pool: PgPool) {
    let fixture = Fixture::new(&pool).await;
    let baseline = fixture.version(&pool).await;
    let (proxy, proxied) = through_proxy(&pool, 8).await;
    let connections = proxy.stats().connections;
    let mut result = fixture.entry.clone();
    result.outcome = Some(OutcomeRow {
        venue_outcome: Some(VenueOutcome::Success),
        ..OutcomeRow::default()
    });
    result.balance.as_mut().unwrap().handed_off = true;
    proxy.set_mode(ProxyMode::Blackhole);
    let worker_pool = proxied.clone();
    let tenant = fixture.tenant;
    let task = tokio::spawn(async move {
        let backend = prepared_journal(&worker_pool, tenant).await;
        Write::Journal(backend, result).apply().await
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        while proxy.stats().connections == connections {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("cold journal connection never reached the startup fault");
    // Setup is intentionally slower than the unchanged 500ms commit bound.
    // A real startup probe may retry; the generated operation itself may not.
    tokio::time::sleep(Duration::from_millis(650)).await;
    proxy.set_mode(ProxyMode::Pass);
    let outcome = tokio::time::timeout(Duration::from_secs(11), task)
        .await
        .expect("journal startup exceeded its existing readiness bound")
        .unwrap();
    assert!(
        matches!(outcome, Ok(true)),
        "cold setup must finish before the generated commit budget: {outcome:?}"
    );
    assert_eq!(proxy.stats().commits, 1, "the operation must commit exactly once");
    assert_eq!(fixture.version(&pool).await, baseline + 1);
    let landed: Option<String> =
        sqlx::query_scalar("SELECT venue_outcome FROM order_intents WHERE intent_id=$1")
            .bind(fixture.entry.intent.intent_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(landed.as_deref(), Some("success"));
    proxied.close().await;
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn ledger_invariants_hold_under_concurrent_writers(pool: PgPool) {
    tokio::task::spawn_blocking(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .thread_stack_size(1_048_576)
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(schedules(pool));
    })
    .await
    .unwrap();
}

async fn schedules(pool: PgPool) {
    let schedule = (
        proptest::collection::vec(1u32..5, 12),
        proptest::collection::vec(0u64..15, 48),
        0u8..4,
    );
    let mut runner = TestRunner::deterministic();
    for case in 0..24 {
        let (quantities, delays, fault) = schedule.new_tree(&mut runner).unwrap().current();
        let tenant = Uuid::now_v7();
        let first = Fixture::for_tenant(&pool, tenant).await;
        let second = Fixture::for_tenant_scope(&pool, tenant, Some(first.scope.clone())).await;
        let fixtures = [first, second];
        let (proxy, proxied) = through_proxy(&pool, 8).await;
        prepared_ledger_pool(&proxied).await;
        let ledger = Arc::new(fixtures[0].ledger(&proxied));
        let mut operations = Vec::new();
        let mut model = Vec::new();
        for (leg_index, f) in fixtures.iter().enumerate() {
            let backend = prepared_journal(&proxied, tenant).await;
            let mut result = f.entry.clone();
            result.outcome = Some(OutcomeRow {
                venue_outcome: Some(VenueOutcome::Success),
                ..OutcomeRow::default()
            });
            result.balance.as_mut().unwrap().handed_off = true;
            operations.push((Write::Journal(backend, result), false));
            for order_index in 0..3 {
                let id = (leg_index * 10 + order_index + 1) as u64;
                let is_entry = order_index == 0;
                let owned = f.link(id, OrderOwner::Leg {
                    intent_id: if is_entry {
                        f.entry.intent.intent_id
                    } else {
                        Uuid::now_v7()
                    },
                    position_id: f.position,
                    kind: if is_entry {
                        LegOrderKind::Entry
                    } else {
                        LegOrderKind::Sell
                    },
                });
                let original_link = if order_index == 2 {
                    f.link(id, OrderOwner::Discovered)
                } else {
                    owned.clone()
                };
                let mut original = f.record(id, Decimal::ZERO, OrderPhase::Live);
                if is_entry {
                    original.side = OrderSide::Buy;
                }
                f.ledger(&pool).record_order(&original, &original_link, seen()).await.unwrap();
                let qty_a = Decimal::from(quantities[leg_index * 6 + order_index * 2]);
                let qty_b = Decimal::from(quantities[leg_index * 6 + order_index * 2 + 1]);
                let total = qty_a + qty_b;
                let trade_a = 100 * (leg_index as u64) + 10 * (order_index as u64) + 1;
                let trade_b = trade_a + 1;
                let page_a = page(trade_a, qty_a);
                let page_b = page(trade_b, qty_b);
                operations.push((
                    Write::Trades(ledger.clone(), f.key(id), Arc::new(page_a.clone())),
                    false,
                ));
                operations
                    .push((Write::Trades(ledger.clone(), f.key(id), Arc::new(page_b)), false));
                let mut alias = page_a;
                alias.trades[0].trade_id = format!("000{trade_a}");
                operations.push((Write::Trades(ledger.clone(), f.key(id), Arc::new(alias)), false));
                let mut terminal = original.clone();
                terminal.phase = OrderPhase::Terminal;
                terminal.status = "CANCELED".into();
                terminal.executed_qty = total;
                operations.push((
                    Write::Order(ledger.clone(), Arc::new(terminal), original_link.clone()),
                    false,
                ));
                operations.push((
                    Write::Order(ledger.clone(), Arc::new(original.clone()), original_link),
                    false,
                ));
                if order_index == 2 {
                    // It may land before or after trades/terminal evidence.
                    // Adoption must count the whole stored execution either way.
                    operations.push((
                        Write::Order(ledger.clone(), Arc::new(original), owned.clone()),
                        false,
                    ));
                }
                let mut invalid = page(9000 + id, Decimal::ONE);
                invalid.trades[0].time = None;
                operations
                    .push((Write::Trades(ledger.clone(), f.key(id), Arc::new(invalid)), true));
                model.push((f.key(id), owned, is_entry, total, [
                    (trade_a, qty_a),
                    (trade_b, qty_b),
                ]));
            }
        }
        let baseline = fixtures[0].version(&pool).await;
        proxy.set_mode(match fault {
            0 => ProxyMode::Pass,
            1 => ProxyMode::DropCommitAckOf("order_fills"),
            2 => ProxyMode::DropCommitAckOf("venue_orders"),
            _ => ProxyMode::DropCommitAckOf("order_intents"),
        });
        let mut tasks = Vec::new();
        let barrier = Arc::new(tokio::sync::Barrier::new(operations.len() + 1));
        for (n, (operation, invalid)) in operations.into_iter().enumerate() {
            let delay = delays[n % delays.len()];
            let barrier = barrier.clone();
            tasks.push(tokio::spawn(async move {
                barrier.wait().await;
                tokio::time::sleep(Duration::from_millis(delay)).await;
                let started = Instant::now();
                let result = operation.apply().await;
                assert!(started.elapsed() < Duration::from_secs(3), "operation exceeded deadline");
                (operation, invalid, result)
            }));
        }
        barrier.wait().await;
        let mut ok_changed = 0i64;
        let mut uncertain = Vec::new();
        for task in tasks {
            let (operation, invalid, result) = task.await.unwrap();
            if invalid {
                assert!(
                    matches!(result, Err(LedgerError::InvalidEvidence(_))),
                    "case {case}: invalid trade result {result:?}"
                );
            } else {
                match result {
                    Ok(true) => ok_changed += 1,
                    Ok(false) => (),
                    Err(LedgerError::Uncertain(_)) => uncertain.push(operation),
                    other => panic!("case {case}: valid write failed {other:?}"),
                }
            }
        }
        assert_eq!(
            proxy.stats().commit_acks_dropped,
            u64::from(fault != 0),
            "fault must actually reach a committed transaction"
        );
        let mut replay_changed = 0i64;
        for operation in &uncertain {
            let started = Instant::now();
            if operation.apply().await.unwrap() {
                replay_changed += 1;
            }
            assert!(started.elapsed() < Duration::from_secs(3));
        }
        let version_delta = fixtures[0].version(&pool).await - baseline;
        assert!(ok_changed <= version_delta);
        assert!(version_delta <= ok_changed + uncertain.len() as i64 + replay_changed);
        for (key, owner, is_entry, total, trades) in &model {
            let row=sqlx::query("SELECT * FROM venue_orders WHERE venue_id=$1 AND account_scope=$2 AND symbol=$3 AND exchange_order_id=$4")
                .bind(key.venue.as_str()).bind(&key.account_scope).bind(&key.symbol).bind(key.exchange_order_id.to_string()).fetch_one(&pool).await.unwrap();
            let OrderOwner::Leg { intent_id, position_id, kind } = &owner.owner else {
                unreachable!()
            };
            assert_eq!(
                row.get::<String, _>("client_order_id"),
                format!("order-{}", key.exchange_order_id)
            );
            assert_eq!(
                row.get::<String, _>("last_client_order_id"),
                format!("order-{}", key.exchange_order_id)
            );
            assert_eq!(row.get::<String, _>("side"), if *is_entry { "BUY" } else { "SELL" });
            assert_eq!(row.get::<String, _>("order_type"), "MARKET");
            assert_eq!(row.get::<Decimal, _>("orig_qty"), dec!(10000));
            assert_eq!(row.get::<Option<Decimal>, _>("stop_price"), None);
            assert_eq!(row.get::<Option<Decimal>, _>("cumulative_quote"), None);
            assert_eq!(row.get::<String, _>("owner"), "leg");
            assert_eq!(row.get::<Uuid, _>("intent_id"), *intent_id);
            assert_eq!(row.get::<Uuid, _>("position_id"), *position_id);
            assert_eq!(row.get::<String, _>("kind"), kind.as_str());
            assert_eq!(row.get::<Decimal, _>("executed_cum"), *total);
            assert_eq!(row.get::<Decimal, _>("fills_qty"), *total);
            assert!(row.get::<bool, _>("fills_complete"));
            assert_eq!(row.get::<i64, _>("trades_cursor"), trades[1].0 as i64 + 1);
            assert_eq!(row.get::<String, _>("phase"), "terminal");
            assert_eq!(row.get::<String, _>("status"), "CANCELED");
            for column in ["live_seen_at", "terminal_seen_at", "last_checked_at"] {
                assert_eq!(row.get::<chrono::DateTime<chrono::Utc>, _>(column), at());
            }
            let fills=sqlx::query("SELECT trade_id,exchange_order_id,tenant_id,price,qty,quote_qty,commission,commission_asset,trade_time,recorded_at FROM order_fills WHERE venue_id=$1 AND account_scope=$2 AND symbol=$3 AND exchange_order_id=$4 ORDER BY trade_id::bigint")
                .bind(key.venue.as_str()).bind(&key.account_scope).bind(&key.symbol).bind(key.exchange_order_id.to_string()).fetch_all(&pool).await.unwrap();
            assert_eq!(fills.len(), 2);
            for (row, (id, qty)) in fills.iter().zip(trades) {
                assert_eq!(row.get::<String, _>("trade_id"), id.to_string());
                assert_eq!(
                    row.get::<String, _>("exchange_order_id"),
                    key.exchange_order_id.to_string()
                );
                assert_eq!(row.get::<Uuid, _>("tenant_id"), tenant);
                assert_eq!(row.get::<Decimal, _>("price"), dec!(100));
                assert_eq!(row.get::<Decimal, _>("qty"), *qty);
                assert_eq!(row.get::<Decimal, _>("quote_qty"), *qty * dec!(100));
                assert_eq!(row.get::<Decimal, _>("commission"), dec!(0.01));
                assert_eq!(row.get::<String, _>("commission_asset"), "USDT");
                assert_eq!(row.get::<chrono::DateTime<chrono::Utc>, _>("trade_time"), at());
                assert_eq!(row.get::<chrono::DateTime<chrono::Utc>, _>("recorded_at"), at());
            }
            assert_eq!(*is_entry, *kind == LegOrderKind::Entry);
        }
        for f in &fixtures {
            let expected=model.iter().filter(|(_,owner,is_entry,_,_)|!is_entry && matches!(owner.owner,OrderOwner::Leg{position_id,..} if position_id==f.position)).map(|(_,_,_,qty,_)|*qty).sum::<Decimal>();
            assert_eq!(f.consumed(&pool).await, expected);
            let (admission,decided,cohort,gross,closed):(Uuid,chrono::NaiveDate,Option<chrono::NaiveDate>,Option<Decimal>,Option<chrono::DateTime<chrono::Utc>>)=sqlx::query_as("SELECT admission_intent_id,decided_month,cohort_month,gross_qty,closed_at FROM balance_legs WHERE position_id=$1")
                .bind(f.position).fetch_one(&pool).await.unwrap();
            assert_eq!(admission, f.entry.intent.intent_id);
            assert_eq!(cohort, Some(decided));
            assert_eq!((gross, closed), (None, None));
            let (outcome,not_sent,position):(Option<String>,bool,Option<Uuid>)=sqlx::query_as("SELECT venue_outcome,not_sent,position_id FROM order_intents WHERE intent_id=$1")
                .bind(f.entry.intent.intent_id).fetch_one(&pool).await.unwrap();
            assert_eq!(
                (outcome, not_sent, position),
                (Some("success".into()), false, Some(f.position))
            );

            assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM order_fills WHERE tenant_id=$1 AND trade_id::numeric>=9000").bind(tenant).fetch_one(&pool).await.unwrap(),0,"refused calls left a trace");
        }
        proxied.close().await;
    }
}
