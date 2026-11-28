// Fixture workloads, not operational position/order limits. Real PG executions
// and production attribution are compared independently; Stub ports are not
// HTTP.
struct CardinalitySample {
    n: u8,
    old_stops: bool,
    cold: bool,
    result: DaemonResult<usize>,
    before: robson_testkit::db_proxy::ProxyStats,
    after: robson_testkit::db_proxy::ProxyStats,
    production: [u64; 9],
    window: Option<(DateTime<Utc>, DateTime<Utc>, usize)>,
    ports_before: FixtureActivity,
    ports_after: FixtureActivity,
    lookups: Vec<String>,
    expected_lookups: Vec<String>,
    next: Option<DaemonResult<usize>>,
    next_before: robson_testkit::db_proxy::ProxyStats,
    next_after: robson_testkit::db_proxy::ProxyStats,
    next_production: [u64; 9],
    next_window: Option<(DateTime<Utc>, DateTime<Utc>, usize)>,
    parses: u64,
    next_parses: u64,
    closed_in_memory: bool,
    closed_events: i64,
    closed_rows: i64,
    adjustments: i64,
    cancelled: i64,
    basis: Decimal,
    loss: Decimal,
    latch: bool,
    pending: usize,
}

impl CardinalitySample {
    fn predicted_months(&self) -> u64 {
        self.window.map_or(0, |(at, since, _)| {
            1 + u64::from((at.year(), at.month()) != (since.year(), since.month()))
        })
    }
    fn expected_statements(&self) -> u64 {
        // Per close: append/apply16 + halt2. Per orphan: before/result5+6.
        // 8m-2a: the append's try-lock and each journal commit's key. 8m-2b
        // (§7.2, USD-M fill or close 2 -> 8): the close's two projector writes,
        // formerly autocommit, are each BEGIN, key, write, COMMIT.
        18 * u64::from(self.n) + 11 * u64::from(self.n) + self.predicted_months() + 2
    }
    fn primary_matches(&self) -> bool {
        matches!(&self.result, Ok(value) if *value == usize::from(self.n))
            && self.after.statements - self.before.statements == self.expected_statements()
            && self.production[0] == self.expected_statements()
            && self.after.commits - self.before.commits == 5 * u64::from(self.n)
    }
    fn assert_after_pool_close(self) {
        let h = history_months(self.window);
        eprintln!("SCAN_CARDINALITY positions={} orphans={} old_stops={} cache={} statements={} commits={} query_attempts={} parses={} next_parses={} native_http={} fixture_ports={:?}", self.n,self.n,self.old_stops,if self.cold {"cleared"} else {"retained"},self.after.statements-self.before.statements,self.after.commits-self.before.commits,self.production[0],self.parses,self.next_parses,self.production[1],std::array::from_fn::<_,8,_>(|i|self.ports_after.0[i]-self.ports_before.0[i]));
        assert_eq!(self.result.unwrap(), usize::from(self.n));
        assert_wire_delta(
            "multi-leg close + orphan journal + tail",
            self.before,
            self.after,
            29 * u64::from(self.n) + h + 2,
            5 * u64::from(self.n),
        );
        assert_production_io(
            "multi-leg full scan",
            self.production,
            self.after.statements - self.before.statements,
            0,
        );
        assert_eq!(self.production[4], 0, "complete causal attribution");
        assert_eq!(self.production[7], 5 * u64::from(self.n));
        assert_eq!(self.production[8], 0);
        let mut actual = self.lookups;
        let mut expected = self.expected_lookups;
        actual.sort();
        expected.sort();
        assert_eq!(actual, expected, "each actual recorded/superseded ID is reached once");
        let reads: [u64; 8] =
            std::array::from_fn(|i| self.ports_after.0[i] - self.ports_before.0[i]);
        assert_eq!(reads, [
            2,
            1,
            0,
            u64::from(self.n) * if self.old_stops { 4 } else { 1 },
            u64::from(self.n),
            0,
            0,
            0
        ]);
        assert_eq!(self.ports_after.2[0] - self.ports_before.2[0], u64::from(self.n));
        assert_eq!(
            self.ports_after.2[1] - self.ports_before.2[1],
            0,
            "fills cause no new close send"
        );
        assert!(self.closed_in_memory);
        assert_eq!(self.closed_events, i64::from(self.n));
        assert_eq!(self.closed_rows, i64::from(self.n));
        assert_eq!(self.adjustments, 0, "scan never manufactures a capital adjustment");
        assert_eq!(self.cancelled, i64::from(self.n));
        assert_eq!(self.basis, dec!(10000));
        assert_eq!(self.loss, dec!(0.11) * Decimal::from(self.n));
        assert!(!self.latch);
        assert_eq!(self.pending, 0);
        assert_eq!(self.next.expect("matching primary reaches warm drained scan").unwrap(), 0);
        let next_h = history_months(self.next_window);
        assert_wire_delta(
            "multi-leg warm drained scan",
            self.next_before,
            self.next_after,
            next_h + 2,
            0,
        );
        assert_production_io(
            "multi-leg warm drained scan",
            self.next_production,
            self.next_after.statements - self.next_before.statements,
            0,
        );
        assert_eq!(self.next_production[4], 0);
        assert_eq!(self.next_parses, 0, "same prepared history/month queries remain warm");
    }
}

async fn cardinality_sample(
    wire: &ObservedScanPool,
    upstream: &sqlx::PgPool,
    n: u8,
    old_stops: bool,
    cold: bool,
) -> CardinalitySample {
    use sqlx::Connection as _;
    let (daemon, worker, _, tenant) = prepared_scan_with_fills_journal(
        &wire.pool,
        "net_from_start_non_expanding_v1",
        true,
        n,
        dec!(90),
        dec!(0.01),
        true,
    )
    .await;
    let mut positions = daemon.store.positions().find_active().await.unwrap();
    let ids: Vec<_> = positions.iter().map(|p| p.id).collect();
    let mut expected_lookups = Vec::new();
    for position in &mut positions {
        let filled = position.insurance_stop_id.clone().unwrap();
        if old_stops {
            let current = format!("UNFILLED-CURRENT-{}", position.id);
            let old1 = format!("UNFILLED-OLD-1-{}", position.id);
            let old2 = format!("UNFILLED-OLD-2-{}", position.id);
            position.insurance_stop_id = Some(current.clone());
            if let PositionState::Active { insurance_stop_id, .. } = &mut position.state {
                *insurance_stop_id = Some(current.clone());
            }
            position.superseded_stop_ids = vec![old1.clone(), old2.clone(), filled.clone()];
            expected_lookups.extend([current.clone(), old1, old2, filled]);
            daemon.store.positions().save(position).await.unwrap();
            sqlx::query("UPDATE positions_current SET insurance_stop_id=$1 WHERE position_id=$2")
                .bind(current)
                .bind(position.id)
                .execute(upstream)
                .await
                .unwrap();
        } else {
            expected_lookups.push(filled);
        }
    }
    // A burst of n real journaled orphan orders on the helper's chosen active
    // symbol, while n distinct closed symbols must all be visited by the sweep.
    for _ in 0..n {
        place_journaled_fixture_orphan(&daemon).await;
    }
    daemon.exchange.set_futures_balance(dec!(10000) - dec!(0.11) * Decimal::from(n));
    if cold {
        let mut connection = wire.pool.acquire().await.unwrap();
        connection.clear_cached_statements().await.unwrap();
    }
    let before = wire.proxy.stats();
    let parse_before = wire.proxy.prepared_statement_parses();
    let ports_before = fixture_activity(&daemon);
    let lookup_before = daemon.exchange.protection_fill_lookups().len();
    let probe = crate::operation_observation::probe::Probe::new();
    let result = worker.scan_and_reconcile().with_subscriber(probe.dispatch()).await;
    let after = wire.proxy.stats();
    let production = probe.snapshot();
    let window = worker.test_recent_closed_window();
    let ports_after = fixture_activity(&daemon);
    let lookups = daemon.exchange.protection_fill_lookups()[lookup_before..].to_vec();
    let parses = wire.proxy.prepared_statement_parses() - parse_before;
    let next_before = wire.proxy.stats();
    let next_parse_before = wire.proxy.prepared_statement_parses();
    let h = window.map_or(0, |(at, since, _)| {
        1 + u64::from((at.year(), at.month()) != (since.year(), since.month()))
    });
    // As `expected_statements` (8m-2b: the close's two projector writes are
    // each BEGIN, key, write, COMMIT).
    let expected = 29 * u64::from(n) + h + 2;
    let next = if matches!(&result,Ok(value) if *value==usize::from(n))
        && after.statements - before.statements == expected
        && production[0] == expected
        && after.commits - before.commits == 5 * u64::from(n)
    {
        Some(worker.scan_and_reconcile().with_subscriber(probe.dispatch()).await)
    } else {
        None
    };
    let next_after = wire.proxy.stats();
    let next_production = if next.is_some() {
        probe.snapshot()
    } else {
        [0; 9]
    };
    let next_window = worker.test_recent_closed_window();
    let next_parses = wire.proxy.prepared_statement_parses() - next_parse_before;
    let mut closed_in_memory = true;
    for id in ids {
        closed_in_memory &= matches!(
            daemon.store.positions().find_by_id(id).await.unwrap().unwrap().state,
            PositionState::Closed { .. }
        );
    }
    // Independent financial reads use the macro pool, outside observed deltas.
    let (closed_events,adjustments): (i64,i64) = sqlx::query_as("SELECT count(*) FILTER(WHERE event_type='position_closed'),count(*) FILTER(WHERE event_type='capital_base_recalibrated') FROM event_log WHERE tenant_id=$1").bind(tenant).fetch_one(upstream).await.unwrap();
    let closed_rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM positions_current WHERE tenant_id=$1 AND state='closed'",
    )
    .bind(tenant)
    .fetch_one(upstream)
    .await
    .unwrap();
    let cancelled: i64 = sqlx::query_scalar("SELECT count(*) FROM order_intents WHERE tenant_id=$1 AND kind='cancel' AND cancel_class='cancelled'").bind(tenant).fetch_one(upstream).await.unwrap();
    let (basis, loss): (Decimal, Decimal) = sqlx::query_as(
        "SELECT capital_base,realized_loss FROM monthly_state WHERE year=$1 AND month=$2",
    )
    .bind(Utc::now().year() as i16)
    .bind(Utc::now().month() as i16)
    .fetch_one(upstream)
    .await
    .unwrap();
    CardinalitySample {
        n,
        old_stops,
        cold,
        result,
        before,
        after,
        production,
        window,
        ports_before,
        ports_after,
        lookups,
        expected_lookups,
        next,
        next_before,
        next_after,
        next_production,
        next_window,
        parses,
        next_parses,
        closed_in_memory,
        closed_events,
        closed_rows,
        adjustments,
        cancelled,
        basis,
        loss,
        latch: daemon.order_journal.latch_set(),
        pending: daemon.order_journal.retry_set_depth(),
    }
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn multi_position_orphan_costs_match_real_wire_and_production_observation(
    pool: sqlx::PgPool,
) {
    // Examples of account cardinality, never product position/order ceilings.
    for n in [1, 2, 4, 8] {
        for old_stops in [false, true] {
            let wire = ObservedScanPool::new(&pool).await;
            let mut samples = Vec::new();
            for cold in [true, false] {
                let sample = cardinality_sample(&wire, &pool, n, old_stops, cold).await;
                let matched = sample.primary_matches();
                samples.push(sample);
                if !matched {
                    break;
                } // preserve first failure; no warm retry.
            }
            wire.pool.close().await;
            // All scans were directly awaited and the observed pool is closed
            // before the independent wire/count/financial assertions.
            for sample in samples {
                sample.assert_after_pool_close();
            }
        }
    }
}
