#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn scan_close_append_failure_preserves_memory_fence_and_basis(pool: sqlx::PgPool) {
    close_survives_failure(pool, true).await;
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn scan_close_projection_failure_preserves_memory_fence_and_basis(pool: sqlx::PgPool) {
    close_survives_failure(pool, false).await;
}

async fn interleaving(pool: sqlx::PgPool, schedule: (u8, bool, bool, bool, i16, u8, i16, u8)) {
    let (phase, boundary, contested, nfs, residual, close_count, price_delta, fee_millis) =
        schedule;
    let model = if nfs {
        "net_from_start_non_expanding_v1"
    } else {
        "hwm_v1"
    };
    let fee = Decimal::from(fee_millis) / dec!(1000);
    let net = (Decimal::from(price_delta) * dec!(0.010) - fee) * Decimal::from(close_count);
    let (daemon, worker, id, tenant) = prepared_scan_with_fills(
        &pool,
        model,
        !boundary,
        close_count,
        dec!(100) + Decimal::from(price_delta),
        fee,
    )
    .await;
    let phases = [
        ScanTestPhase::AfterClose,
        ScanTestPhase::BeforeRiskSnapshot,
        ScanTestPhase::AfterRiskSnapshot,
        ScanTestPhase::AfterGuardSnapshot,
        ScanTestPhase::AfterWallet,
        ScanTestPhase::AfterMonth,
        ScanTestPhase::AfterNet,
    ];
    let pause = if phase == 1 {
        // This pause is INSIDE live_risk_open_positions: the repository
        // has returned and the real inventory port has not been awaited.
        daemon.position_manager.read().await.test_pause_before_inventory()
    } else {
        worker.test_pause_at(phases[usize::from(phase)])
    };
    // Net/fee come from the actual fill; the independent producer receives
    // a separate real typed TRANSFER. The scan is not given that authority.
    let delta = Decimal::from(residual);
    daemon.exchange.set_futures_balance(dec!(10000) + net + delta);
    let scan = tokio::spawn({
        let worker = worker.clone();
        async move { worker.scan_and_reconcile().await }
    });
    if tokio::time::timeout(Duration::from_secs(3), pause.reached.notified())
        .await
        .is_err()
    {
        scan.abort();
        let stopped = scan.await;
        panic!("scan did not reach its selected real boundary: {stopped:?}");
    }
    let mut consumer = daemon.event_bus.subscribe();
    let producer = tokio::spawn({
        let daemon = daemon.clone();
        let pool = pool.clone();
        async move {
            if boundary {
                daemon.handle_month_boundary(Utc::now()).await?;
                Ok::<_, DaemonError>(None)
            } else {
                daemon.exchange.set_income_records(vec![robson_exec::ports::IncomeRecord {
                    exchange_income_id: format!("transfer-{id}"),
                    symbol: None,
                    income_type: robson_exec::ports::IncomeType::Transfer,
                    amount: delta,
                    asset: "USDT".into(),
                    exchange_trade_id: None,
                    income_time: Utc::now(),
                }]);
                let producer = crate::income_ledger::IncomeLedgerWorker::new(
                    daemon.exchange.clone(),
                    daemon.position_manager.clone(),
                    daemon.store.clone(),
                    pool,
                    daemon.event_bus.clone(),
                    Duration::from_secs(60),
                    CancellationToken::new(),
                );
                let outcome = producer.test_outcome();
                producer.poll_and_match().await?;
                let recorded = *outcome.lock().unwrap();
                Ok(Some(recorded.expect("producer must record its actual reached outcome")))
            }
        }
    });
    let producer_result = if contested {
        pause.resume.notify_one();
        joined(producer).await
    } else {
        let result = joined(producer).await;
        pause.resume.notify_one();
        result
    };
    let scan_result = joined(scan).await;
    let producer_outcome = producer_result
        .unwrap()
        .expect("independent producer returned an actual outer error");
    assert_ne!(
        producer_outcome,
        Some(crate::income_ledger::IncomeTestOutcome::Error),
        "a swallowed producer error is never a legitimate guard/policy refusal"
    );
    // Exercise the actual daemon consumer, including duplicate append recovery
    // and projection reapplication, before computing the final oracle.
    let projection_before_reapply: (Decimal, Decimal, Decimal, i32, Decimal) = sqlx::query_as(
            "SELECT capital_base,carried_risk,realized_loss,trades_opened,month_peak_net FROM monthly_state WHERE year=$1 AND month=$2")
            .bind(Utc::now().year() as i16).bind(Utc::now().month() as i16).fetch_one(&pool).await.unwrap();
    let mut consumed = 0;
    while let Some(event) = consumer.try_recv() {
        if let DaemonEvent::DomainEvent(event) = event.expect("event receiver must not lag") {
            daemon.handle_event(DaemonEvent::DomainEvent(event)).await.unwrap();
            consumed += 1;
        }
    }
    let projection_after_reapply: (Decimal, Decimal, Decimal, i32, Decimal) = sqlx::query_as(
            "SELECT capital_base,carried_risk,realized_loss,trades_opened,month_peak_net FROM monthly_state WHERE year=$1 AND month=$2")
            .bind(Utc::now().year() as i16).bind(Utc::now().month() as i16).fetch_one(&pool).await.unwrap();
    assert_eq!(
        projection_after_reapply, projection_before_reapply,
        "actual daemon reapplication must not add a second financial projection effect"
    );
    assert_eq!(scan_result.unwrap().unwrap(), usize::from(close_count));
    assert!(matches!(
        daemon.store.positions().find_by_id(id).await.unwrap().unwrap().state,
        PositionState::Closed { .. }
    ));
    let events: Vec<(String, serde_json::Value)> = sqlx::query_as(
        "SELECT event_type,payload FROM event_log WHERE tenant_id=$1 ORDER BY ingested_at,event_id",
    )
    .bind(tenant)
    .fetch_all(&pool)
    .await
    .unwrap();
    let mut authorized = Vec::new();
    for (kind, payload) in &events {
        if kind == "capital_base_recalibrated" {
            assert!(
                payload["reason"]
                    .as_str()
                    .unwrap()
                    .starts_with("income_ledger:transfer_confirmed:"),
                "scan_authored_adjustment_under_real_interleaving: {payload}"
            );
            authorized
                .push(payload["new_capital_base"].as_str().unwrap().parse::<Decimal>().unwrap());
        } else if kind == "month_boundary_reset" {
            authorized.push(payload["capital_base"].as_str().unwrap().parse::<Decimal>().unwrap());
        }
    }
    use crate::income_ledger::IncomeTestOutcome;
    let expected_writes = match producer_outcome {
        None => {
            assert!(boundary);
            1
        },
        Some(IncomeTestOutcome::Applied) => {
            assert!(!boundary && (!nfs || residual < 0));
            1
        },
        Some(IncomeTestOutcome::Guarded) => {
            assert!(!boundary && phase == 0 && close_count > 1);
            0
        },
        Some(IncomeTestOutcome::PolicyRefused) => {
            assert!(!boundary && nfs && residual > 0);
            0
        },
        other => panic!("unexpected independent producer outcome: {other:?}"),
    };
    assert_eq!(
        authorized.len(),
        expected_writes,
        "actual producer completion must agree with the independent event set"
    );
    assert_eq!(
        consumed,
        usize::from(producer_outcome == Some(IncomeTestOutcome::Applied)),
        "every queued income DomainEvent must exercise the real daemon consumer"
    );
    assert_eq!(
        projected_basis(&pool).await,
        authorized.last().copied().unwrap_or(dec!(10000)),
        "persisted_basis_must_follow_only_actual_independent_events"
    );
    let observed_closes = events.iter().filter(|(kind, _)| kind == "position_closed").count();
    assert_eq!(observed_closes, usize::from(close_count));
    assert_eq!(worker.scan_and_reconcile().await.unwrap(), 0);
    assert_eq!(daemon.exchange.position_close_call_count(), 0);
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn generated_async_scans_preserve_independent_income_and_month_updates(pool: sqlx::PgPool) {
    use proptest::{
        prelude::*,
        test_runner::{Config, TestRng, TestRunner},
    };
    // Forced orders cover every stop and both release styles. The property
    // below adds generated values/models/orders and retains counterexamples.
    for phase in 0u8..7 {
        for contested in [false, true] {
            interleaving(
                pool.clone(),
                (phase, phase % 2 == 0, contested, phase % 2 != 0, -10, 1, -10, 10),
            )
            .await;
        }
    }
    let handle = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
            let config = Config { cases: 24, max_shrink_iters: 128, failure_persistence: None, ..Config::default() };
            let rng = TestRng::deterministic_rng(config.rng_algorithm);
            let mut runner = TestRunner::new_with_rng(config, rng);
            runner.run(&(0u8..7, any::<bool>(), any::<bool>(), any::<bool>(), prop_oneof![-100i16..=-1, 1i16..=100], 1u8..=3, -20i16..=20, 1u8..=20), |(phase,boundary,contested,nfs,residual,count,price_delta,fee_millis)| {
                eprintln!("SCAN_SCHEDULE deterministic_rng phase={phase} boundary={boundary} contested={contested} nfs={nfs} residual={residual} count={count} price_delta={price_delta} fee_millis={fee_millis}");
                handle.block_on(interleaving(pool.clone(), (phase, boundary, contested, nfs, residual, count, price_delta, fee_millis)));
                Ok(())
            }).unwrap();
        }).await.unwrap();
}
