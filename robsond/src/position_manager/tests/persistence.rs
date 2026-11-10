//! ADR-0056 step 7, PR P: results never dropped, per-stream serialization,
//! suspension, and admissions closed on the latch.
//!
//! The tests with no database suspend a stream by hand, as a failed write
//! would, and run on a manager with no event log: in a PostgreSQL build the
//! suspension is checked before anything else, so every later write to the
//! stream is refused exactly as in production. The `DATABASE_URL` tests make
//! real writes fail.

use super::*;

/// Suspend `position_id`'s stream the way a failed write does: refused from
/// here on, recorded through the retry set, latch set.
#[cfg(feature = "postgres")]
fn suspend_leg(manager: &PositionManager<StubExchange, MemoryStore>, position_id: PositionId) {
    let row = manager
        .event_streams
        .suspend(&format!("position:{position_id}"), None, "injected")
        .expect("not suspended yet");
    manager.executor.order_journal().record_suspension(row);
}

fn latched(manager: &PositionManager<StubExchange, MemoryStore>) -> bool {
    manager.executor.order_journal().latch_set()
}

/// A long of `quantity` at 100 with its live exchange stop at 90.
#[cfg(feature = "postgres")]
async fn leg_of(
    manager: &Arc<PositionManager<StubExchange, MemoryStore>>,
    quantity: Decimal,
) -> (Position, String) {
    let (mut position, stop_id) = active_leg_with_live_stop(manager).await;
    position.quantity = Quantity::new(quantity).unwrap();
    manager.store.positions().save(&position).await.unwrap();
    (position, stop_id)
}

fn is_closed(position: &Position) -> bool {
    matches!(position.state, PositionState::Closed { .. })
}

// =============================================================================
// Admissions close on the latch, and on this month's suspended system stream
// =============================================================================

/// Every latch source closes admissions until a restart, at each of the three
/// doors, before anything is written: a governed refusal, and the signal's
/// detector re-arms under the backoff, as for a residual.
#[tokio::test]
async fn the_latch_closes_admissions_at_every_door() {
    let closed = |result: Option<DaemonError>| {
        assert!(matches!(result, Some(DaemonError::AdmissionsClosed { .. })), "{result:?}");
    };
    let manager = create_test_manager().await;
    let symbol = Symbol::from_pair("BTCUSDT").unwrap();
    let entry = Price::new(dec!(100)).unwrap();
    let stop = Price::new(dec!(98)).unwrap();
    let arm = || {
        manager.arm_position(
            symbol.clone(),
            Side::Long,
            create_test_risk_config(),
            Some(TechnicalStopDistance::from_entry_and_stop(entry, stop)),
            Uuid::now_v7(),
        )
    };
    let armed = arm().await.unwrap();
    // A venue mutation with no permit, one of the latch's sources since PR S.
    manager.executor.order_journal().gate().mutation("POST /order").unwrap();
    assert!(latched(&manager));

    closed(arm().await.err());
    closed(manager.approve_query(Uuid::now_v7()).await.err());
    closed(
        manager
            .handle_signal(DetectorSignal {
                signal_id: Uuid::now_v7(),
                position_id: armed.id,
                symbol: symbol.clone(),
                side: Side::Long,
                entry_price: entry,
                stop_loss: stop,
                technical_stop_analysis: None,
                timestamp: chrono::Utc::now(),
            })
            .await
            .err(),
    );
    assert_eq!(manager.exchange().market_order_call_count(), 0);
    assert_eq!(manager.governed_rearm_counts.read().await.get(&armed.id), Some(&1));
    let armed = manager.get_position(armed.id).await.unwrap().unwrap();
    assert!(matches!(armed.state, PositionState::Armed), "{:?}", armed.state);
    assert!(!armed.carries_admission_evidence(), "refused before any evidence");
}

/// A suspended system stream of the current month, loaded at start, keeps
/// admissions closed with the latch clear; a past month's does not.
#[tokio::test]
async fn a_suspended_stream_of_this_month_keeps_admissions_closed() {
    let now = chrono::Utc::now();
    let last_month = now.with_day(1).unwrap() - chrono::Duration::days(1);
    for (year, month, closes) in [
        (now.year(), now.month(), true),
        (last_month.year(), last_month.month(), false),
    ] {
        let manager = create_test_manager().await;
        manager.event_streams.restore([(
            format!("system:month_boundary:{year:04}-{month:02}"),
            crate::event_streams::Suspended {
                suspension_id: Uuid::now_v7(),
                stream: crate::event_streams::ManagedStream::Month { year, month },
                repaired_through_seq: None,
            },
        )]);
        assert!(!latched(&manager));

        let armed = manager
            .arm_position(
                Symbol::from_pair("BTCUSDT").unwrap(),
                Side::Long,
                create_test_risk_config(),
                None,
                Uuid::now_v7(),
            )
            .await;
        assert_eq!(
            matches!(armed, Err(DaemonError::AdmissionsClosed { .. })),
            closes,
            "{year}-{month}: {armed:?}"
        );
    }
}

// =============================================================================
// Results never dropped: reductions on a suspended leg reach the venue
// =============================================================================

/// The software exit on a suspended leg: cancel and exit reach the venue and
/// the leg closes in memory, though nothing more of it persists.
#[cfg(feature = "postgres")]
#[tokio::test]
async fn a_software_exit_on_a_suspended_leg_reaches_the_venue_and_closes_it() {
    let manager = create_test_manager().await;
    let (position, stop_id) = active_leg_with_live_stop(&manager).await;
    suspend_leg(&manager, position.id);
    manager.exchange().set_price("BTCUSDT", dec!(90));
    let mut events = manager.event_bus.subscribe();

    stop_crossing_tick(&manager, &position).await.unwrap();

    assert_eq!(manager.exchange().market_order_call_count(), 1);
    assert!(!manager.exchange().has_stop_order(&stop_id), "the cancel went out");
    assert!(is_closed(&manager.get_position(position.id).await.unwrap().unwrap()));
    let mut closed_on_the_bus = false;
    while let Ok(Some(Ok(event))) =
        tokio::time::timeout(std::time::Duration::from_millis(20), events.recv()).await
    {
        closed_on_the_bus |= matches!(event, DaemonEvent::CorePositionClosed { .. });
    }
    assert!(closed_on_the_bus, "what follows the close still ran");
    assert!(latched(&manager));
}

/// `handle_exit_fill` keeps a close that did not persist, and MonthlyHalt is
/// still evaluated on it: here the close exhausts the month's budget.
#[cfg(feature = "postgres")]
#[tokio::test]
async fn an_exit_fill_whose_close_did_not_persist_still_evaluates_the_halt() {
    let manager = create_test_manager().await;
    let (position, _) = leg_of(&manager, dec!(50)).await;
    suspend_leg(&manager, position.id);
    manager.exchange().set_price("BTCUSDT", dec!(90));

    stop_crossing_tick(&manager, &position).await.unwrap();

    let leg = manager.get_position(position.id).await.unwrap().unwrap();
    assert!(is_closed(&leg), "{:?}", leg.state);
    assert!(
        manager.circuit_breaker.blocks_new_entries().await,
        "a 500 loss halts a 400 budget"
    );
}

/// `panic_close_all` reports a leg it closed as closed although its close did
/// not persist.
#[cfg(feature = "postgres")]
#[tokio::test]
async fn panic_reports_a_leg_closed_when_its_close_did_not_persist() {
    let manager = create_test_manager().await;
    let (position, _) = active_leg_with_live_stop(&manager).await;
    suspend_leg(&manager, position.id);

    let closed = manager.panic_close_all().await.unwrap();

    assert_eq!(closed, vec![position.id]);
    assert_eq!(manager.exchange().market_order_call_count(), 1);
    assert!(is_closed(&manager.get_position(position.id).await.unwrap().unwrap()));
}

/// `reconcile_close` keeps a close that did not persist, and MonthlyHalt is
/// still evaluated on it.
#[cfg(feature = "postgres")]
#[tokio::test]
async fn a_reconciled_close_that_did_not_persist_still_evaluates_the_halt() {
    let manager = create_test_manager().await;
    let (position, stop_id) = leg_of(&manager, dec!(50)).await;
    manager.exchange().set_price("BTCUSDT", dec!(90));
    let input = stop_fill_of(&manager, &position, &stop_id, dec!(50)).await;
    suspend_leg(&manager, position.id);

    let outcome = manager.reconcile_close(input).await.unwrap();

    assert!(matches!(outcome, ReconcileCloseOutcome::Closed), "{outcome:?}");
    assert!(is_closed(&manager.get_position(position.id).await.unwrap().unwrap()));
    assert!(
        manager.circuit_breaker.blocks_new_entries().await,
        "a 500 loss halts a 400 budget"
    );
}

/// The trailing replacement of a suspended leg reaches the venue.
#[cfg(feature = "postgres")]
#[tokio::test]
async fn a_trailing_replacement_on_a_suspended_leg_reaches_the_venue() {
    let manager = create_test_manager().await;
    let (position, stop_id) = active_leg_with_live_stop(&manager).await;
    suspend_leg(&manager, position.id);
    let placements = manager.exchange().protection_call_count();

    manager
        .process_market_data(MarketData {
            venue: test_venue(),
            symbol: position.symbol.clone(),
            price: Price::new(dec!(110)).unwrap(),
            timestamp: chrono::Utc::now(),
            source: crate::event_bus::MarketDataSource::Ws,
        })
        .await
        .unwrap();

    assert_eq!(manager.exchange().protection_call_count(), placements + 1);
    assert!(!manager.exchange().has_stop_order(&stop_id), "the old stop was replaced");
    let leg = manager.get_position(position.id).await.unwrap().unwrap();
    match &leg.state {
        PositionState::Active { trailing_stop, insurance_stop_id, .. } => {
            assert_eq!(*trailing_stop, Price::new(dec!(100)).unwrap());
            assert_ne!(insurance_stop_id.as_deref(), Some(stop_id.as_str()));
        },
        other => panic!("{other:?}"),
    }
}

/// A due heal of a suspended leg re-places its stop.
#[cfg(feature = "postgres")]
#[tokio::test]
async fn a_heal_on_a_suspended_leg_places_its_stop() {
    let manager = create_test_manager().await;
    let (position, stop_id) = active_leg_with_live_stop(&manager).await;
    suspend_leg(&manager, position.id);
    let placements = manager.exchange().protection_call_count();

    // Due now, and run.
    retry_stop_now(&manager, position.id).await;

    assert_eq!(manager.exchange().protection_call_count(), placements + 1);
    let leg = manager.get_position(position.id).await.unwrap().unwrap();
    let healed = leg.insurance_stop_id.expect("the stop is on the leg in memory");
    assert_ne!(healed, stop_id);
    assert!(manager.exchange().has_stop_order(&healed));
}

/// One leg's refused batch never skips the other legs on that tick: the
/// suspended leg is put first in the tick's order, and the healthy one behind
/// it still exits.
#[cfg(feature = "postgres")]
#[tokio::test]
async fn a_suspended_leg_never_starves_the_others_on_its_tick() {
    for _ in 0..64 {
        let manager = create_test_manager().await;
        // Advances its favourable extreme at 90, an event-only batch.
        let mut suspended =
            save_active_position(&manager, "BTCUSDT", Side::Long, dec!(80), dec!(1)).await;
        suspended.tech_stop_distance = Some(TechnicalStopDistance::from_entry_and_stop(
            Price::new(dec!(80)).unwrap(),
            Price::new(dec!(60)).unwrap(),
        ));
        if let PositionState::Active { trailing_stop, .. } = &mut suspended.state {
            *trailing_stop = Price::new(dec!(60)).unwrap();
        }
        manager.store.positions().save(&suspended).await.unwrap();
        let (healthy, _) = active_leg_with_live_stop(&manager).await;
        let order = manager.store.positions().find_active().await.unwrap();
        if order[0].id != suspended.id {
            continue;
        }
        suspend_leg(&manager, suspended.id);
        manager.exchange().set_price("BTCUSDT", dec!(90));

        stop_crossing_tick(&manager, &healthy).await.unwrap();

        let events = manager.store.events().find_by_position(suspended.id).await.unwrap();
        assert!(
            events.iter().any(|e| matches!(e, Event::FavorableExtremeAdvanced { .. })),
            "the suspended leg's batch ran, and was refused: {events:?}"
        );
        assert!(is_closed(&manager.get_position(healthy.id).await.unwrap().unwrap()));
        return;
    }
    panic!("the suspended leg never came first");
}

/// The exit's own journal result write fails: the leg still closes, the write
/// waits in the retry set, and that alone sets no latch (PR J's rule; the
/// entry's counterpart is
/// `a_failed_journal_result_write_keeps_the_fill_and_places_the_stop`).
#[tokio::test]
async fn a_failed_journal_result_write_after_an_exit_keeps_the_close() {
    use robson_exec::order_journal::JournalFault;
    let (manager, backend) = journaled_manager().await;
    let (position, _) = active_leg_with_live_stop(&manager).await;
    manager.exchange().set_price("BTCUSDT", dec!(90));
    // The cancel's two writes, the exit's pre-send write, then its result.
    backend.fail_commit(3, Some(JournalFault::Known));

    stop_crossing_tick(&manager, &position).await.unwrap();

    assert_eq!(manager.exchange().market_order_call_count(), 1);
    assert!(is_closed(&manager.get_position(position.id).await.unwrap().unwrap()));
    assert!(manager.executor.order_journal().retry_set_depth() >= 1);
    assert!(!latched(&manager));
}

// =============================================================================
// DATABASE_URL: real writes that fail
// =============================================================================

/// The rows of `position_id`'s stream, in seq order.
#[cfg(feature = "postgres")]
async fn stream_rows(pool: &sqlx::PgPool, position_id: PositionId) -> Vec<(i64, String)> {
    sqlx::query_as("SELECT seq, event_type FROM event_log WHERE stream_key = $1 ORDER BY seq")
        .bind(format!("position:{position_id}"))
        .fetch_all(pool)
        .await
        .unwrap()
}

/// Fails the projection apply of `position_id`'s rows, after they landed:
/// every update of its `positions_current` row that matches `condition`.
#[cfg(feature = "postgres")]
async fn fail_projection_updates(pool: &sqlx::PgPool, position_id: PositionId, condition: &str) {
    let function = format!(
        r#"
        CREATE OR REPLACE FUNCTION injected_projection_failure() RETURNS trigger
        LANGUAGE plpgsql AS $$
        BEGIN
            IF NEW.position_id = '{position_id}' AND {condition} THEN
                RAISE EXCEPTION 'injected projection failure';
            END IF;
            RETURN NEW;
        END
        $$
        "#
    );
    sqlx::query(&function).execute(pool).await.unwrap();
    sqlx::query(
        "CREATE TRIGGER injected_projection_failure BEFORE UPDATE ON positions_current \
         FOR EACH ROW EXECUTE FUNCTION injected_projection_failure()",
    )
    .execute(pool)
    .await
    .unwrap();
}

/// The entry filled and its accepted event did not land: the result is kept,
/// the stop goes on, the stream refuses everything after the failure, and the
/// latch is set. The backoff still resets: the entry was ticketed.
#[cfg(feature = "postgres")]
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn an_entry_whose_accepted_event_did_not_land_still_gets_its_stop(pool: sqlx::PgPool) {
    let manager = create_audited_postgres_test_manager(pool.clone(), 300);
    let (armed, signal) = recorded_armed_position_and_signal(&manager).await;
    manager.governed_rearm_counts.write().await.insert(armed.id, 4);

    fail_event_log_writes(&pool, "NEW.event_type = 'entry_order_accepted'").await;
    manager.handle_signal(signal).await.unwrap();
    restore_event_log_writes(&pool).await;

    assert_eq!(manager.exchange().market_order_call_count(), 1);
    assert_eq!(manager.exchange().protection_call_count(), 1, "the stop went on");
    let leg = manager.get_position(armed.id).await.unwrap().unwrap();
    assert!(matches!(leg.state, PositionState::Active { .. }), "{:?}", leg.state);
    assert!(leg.insurance_stop_id.is_some());
    assert!(latched(&manager));
    assert!(manager.event_streams.position_suspended(armed.id));
    let types: Vec<String> = stream_rows(&pool, armed.id).await.into_iter().map(|r| r.1).collect();
    assert_eq!(types.last().map(String::as_str), Some("entry_order_requested"), "{types:?}");
    assert!(!manager.detectors.read().await.contains_key(&armed.id));
    assert_eq!(manager.governed_rearm_counts.read().await.get(&armed.id), None);
}

/// The accepted event landed and its apply failed: the suspension names that
/// seq, nothing lands after it, and the stop still goes on.
#[cfg(feature = "postgres")]
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn an_entry_whose_accepted_event_did_not_apply_still_gets_its_stop(pool: sqlx::PgPool) {
    use robson_exec::order_journal::{OrderJournal, StubJournalBackend};
    let backend = Arc::new(StubJournalBackend::default());
    let journal =
        Arc::new(OrderJournal::new(backend.clone(), &robson_exec::venue::VenueId::new("test:v")));
    let manager = audited_postgres_test_manager_with_journal(pool.clone(), 300, Some(journal));
    let (armed, signal) = recorded_armed_position_and_signal(&manager).await;

    fail_projection_updates(&pool, armed.id, "NEW.state = 'entering'").await;
    manager.handle_signal(signal).await.unwrap();

    assert_eq!(manager.exchange().protection_call_count(), 1, "the stop went on");
    let rows = stream_rows(&pool, armed.id).await;
    let (accepted_seq, last) = rows.last().cloned().unwrap();
    assert_eq!(last, "entry_order_accepted", "{rows:?}");
    manager.executor.order_journal().flush_pending().await;
    let recorded = backend.suspensions();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].stream_key, format!("position:{}", armed.id));
    assert_eq!(
        (recorded[0].stream_kind, recorded[0].failed_seq),
        ("position", Some(accepted_seq))
    );
    assert!(latched(&manager));
}

/// An exit's events cannot reach the log at all: the exit still went out, its
/// result came back, and the latch is set.
#[cfg(feature = "postgres")]
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn eventlog_failure_during_exit_cycle_keeps_the_result(pool: sqlx::PgPool) {
    let manager = create_postgres_test_manager(pool.clone());
    pool.close().await;

    let position_id = Uuid::now_v7();
    let symbol = Symbol::from_pair("BTCUSDT").unwrap();
    let quantity = Quantity::new(dec!(0.01)).unwrap();
    let trigger = Price::new(dec!(94000)).unwrap();
    let actions = vec![
        EngineAction::EmitEvent(Event::ExitTriggered {
            position_id,
            reason: robson_domain::ExitReason::TrailingStop,
            trigger_price: trigger,
            stop_price: trigger,
            timestamp: chrono::Utc::now(),
        }),
        EngineAction::PlaceExitOrder {
            position_id,
            cycle_id: Some(Uuid::now_v7()),
            symbol,
            side: Side::Long.exit_action(),
            quantity,
            reason: robson_domain::ExitReason::TrailingStop,
        },
    ];

    let results = manager.execute_and_persist(actions).await.unwrap();
    assert!(
        results.iter().any(|r| matches!(r, ActionResult::OrderPlaced {
            event: Some(Event::ExitOrderPlaced { .. }),
            ..
        })),
        "{results:?}"
    );
    assert_eq!(manager.exchange().market_order_call_count(), 1);
    assert!(manager.event_streams.position_suspended(position_id));
    assert!(latched(&manager));
}

/// Two writers on one stream: the first is held inside its append while the
/// second registers on the stream's lock, which the test sees before it lets
/// the first fail. The second then sees the suspension and appends nothing.
#[cfg(feature = "postgres")]
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_writer_queued_behind_a_failure_sees_the_suspension(pool: sqlx::PgPool) {
    use sqlx::Connection;
    const HELD: i64 = 4_242_001;
    let manager = create_postgres_test_manager(pool.clone());
    let position_id = Uuid::now_v7();
    let stream_key = format!("position:{position_id}");
    // The first writer's append waits on an advisory lock the test holds, then
    // fails.
    inject_event_log_fault(
        &pool,
        "NEW.event_type = 'exit_triggered'",
        &format!(
            "PERFORM pg_advisory_lock({HELD}); PERFORM pg_advisory_unlock({HELD}); \
             RAISE EXCEPTION 'injected event log failure';"
        ),
    )
    .await;
    let mut holder = sqlx::PgConnection::connect_with(&pool.connect_options()).await.unwrap();
    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(HELD)
        .execute(&mut holder)
        .await
        .unwrap();

    let failing = Event::ExitTriggered {
        position_id,
        reason: robson_domain::ExitReason::TrailingStop,
        trigger_price: Price::new(dec!(94000)).unwrap(),
        stop_price: Price::new(dec!(94000)).unwrap(),
        timestamp: chrono::Utc::now(),
    };
    let queued = stop_failed(position_id, "second writer");
    // Bounded, so a lock that is not taken fails this test instead of hanging it.
    let users_reach = |users: usize| {
        let manager = &manager;
        let stream_key = &stream_key;
        async move {
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
            while manager.event_streams.lock_users(stream_key) < users {
                if tokio::time::Instant::now() >= deadline {
                    return false;
                }
                tokio::time::sleep(std::time::Duration::from_millis(2)).await;
            }
            true
        }
    };
    let writers = async {
        tokio::join!(manager.persist_event_to_log(&failing), async {
            // The first holds the lock; once it does, the second registers.
            users_reach(1).await;
            manager.persist_event_to_log(&queued).await
        })
    };
    let release = async {
        let queued_first = users_reach(2).await;
        // Both registered, the second one waiting: let the first fail.
        sqlx::query("SELECT pg_advisory_unlock($1)")
            .bind(HELD)
            .execute(&mut holder)
            .await
            .unwrap();
        queued_first
    };
    let ((first, second), queued_first) = tokio::join!(writers, release);
    restore_event_log_writes(&pool).await;

    assert!(queued_first, "the second writer was queued on the lock before the first failed");
    assert!(matches!(first, Err(DaemonError::EventLog(_))), "{first:?}");
    assert!(matches!(second, Err(DaemonError::StreamSuspended { .. })), "{second:?}");
    assert!(stream_rows(&pool, position_id).await.is_empty());
    assert_eq!(manager.event_streams.lock_entries(), 0, "idle locks are dropped");
    // Another stream is untouched.
    let other = Uuid::now_v7();
    manager
        .persist_event_to_log(&stop_failed(other, "another stream"))
        .await
        .unwrap();
    assert_eq!(stream_rows(&pool, other).await.len(), 1);
}

// =============================================================================
// Query audit after venue work: reported, never a reason to stop
// =============================================================================

/// The tick's refused batch and then its failed query audit, both on the
/// suspended leg the tick reaches first: the healthy leg behind it still exits.
/// The recorder is the production one, on the event log.
#[cfg(feature = "postgres")]
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_failed_query_audit_on_a_refused_tick_batch_skips_no_other_leg(pool: sqlx::PgPool) {
    fail_event_log_writes(&pool, &audit_write("failed")).await;
    let reported = crate::metrics::QUERY_AUDIT_FAILURES_AFTER_VENUE_WORK.get();
    for _ in 0..64 {
        let manager = create_audited_postgres_test_manager(pool.clone(), 300);
        let mut suspended =
            save_active_position(&manager, "BTCUSDT", Side::Long, dec!(80), dec!(1)).await;
        suspended.tech_stop_distance = Some(TechnicalStopDistance::from_entry_and_stop(
            Price::new(dec!(80)).unwrap(),
            Price::new(dec!(60)).unwrap(),
        ));
        if let PositionState::Active { trailing_stop, .. } = &mut suspended.state {
            *trailing_stop = Price::new(dec!(60)).unwrap();
        }
        manager.store.positions().save(&suspended).await.unwrap();
        let (healthy, _) = active_leg_with_live_stop(&manager).await;
        if manager.store.positions().find_active().await.unwrap()[0].id != suspended.id {
            continue;
        }
        suspend_leg(&manager, suspended.id);
        manager.exchange().set_price("BTCUSDT", dec!(90));

        stop_crossing_tick(&manager, &healthy).await.unwrap();
        restore_event_log_writes(&pool).await;

        assert!(is_closed(&manager.get_position(healthy.id).await.unwrap().unwrap()));
        assert_eq!(manager.exchange().market_order_call_count(), 1);
        assert!(crate::metrics::QUERY_AUDIT_FAILURES_AFTER_VENUE_WORK.get() > reported);
        return;
    }
    panic!("the suspended leg never came first");
}

/// `/panic` closes a leg, then cannot record that query completed: the leg
/// is reported closed, and the next leg is still closed.
#[cfg(feature = "postgres")]
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_failed_query_audit_after_a_panic_close_keeps_the_other_legs(pool: sqlx::PgPool) {
    let manager = create_audited_postgres_test_manager(pool.clone(), 300);
    let (first, _) = active_leg_with_live_stop(&manager).await;
    let (second, _) = active_leg_with_live_stop(&manager).await;
    fail_event_log_writes(&pool, &audit_write("completed")).await;

    let closed = manager.panic_close_all().await;
    restore_event_log_writes(&pool).await;

    let mut closed = closed.unwrap();
    closed.sort();
    let mut both = vec![first.id, second.id];
    both.sort();
    assert_eq!(closed, both);
    assert_eq!(manager.exchange().market_order_call_count(), 2);
    for leg in [first.id, second.id] {
        assert!(is_closed(&manager.get_position(leg).await.unwrap().unwrap()));
    }
}

/// `/panic`'s exit fails on one leg and that failure cannot be recorded: the
/// other leg is still closed and reported, whichever came first.
#[cfg(feature = "postgres")]
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_failed_query_audit_after_a_failed_panic_leg_keeps_the_other_legs(pool: sqlx::PgPool) {
    let manager = create_audited_postgres_test_manager(pool.clone(), 300);
    let (first, _) = active_leg_with_live_stop(&manager).await;
    let (second, _) = active_leg_with_live_stop(&manager).await;
    manager.exchange().set_order_fail_next(true);
    fail_event_log_writes(&pool, &audit_write("failed")).await;

    let closed = manager.panic_close_all().await;
    restore_event_log_writes(&pool).await;

    let closed = closed.unwrap();
    assert_eq!(closed.len(), 1, "{closed:?}");
    let failed = if closed[0] == first.id {
        second.id
    } else {
        first.id
    };
    assert!(is_closed(&manager.get_position(closed[0]).await.unwrap().unwrap()));
    let failed = manager.get_position(failed).await.unwrap().unwrap();
    assert!(matches!(failed.state, PositionState::Active { .. }), "{:?}", failed.state);
}

/// A single close and an entry keep their venue outcome when the query's
/// completion cannot be recorded.
#[cfg(feature = "postgres")]
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_failed_query_audit_after_a_close_or_an_entry_keeps_the_outcome(pool: sqlx::PgPool) {
    let manager = create_audited_postgres_test_manager(pool.clone(), 300);
    let (leg, _) = active_leg_with_live_stop(&manager).await;
    fail_event_log_writes(&pool, &audit_write("completed")).await;
    let closed = manager.close_position(leg.id).await;
    restore_event_log_writes(&pool).await;
    assert!(closed.is_ok(), "{closed:?}");
    assert!(is_closed(&manager.get_position(leg.id).await.unwrap().unwrap()));

    let manager = create_audited_postgres_test_manager(pool.clone(), 300);
    let (armed, signal) = recorded_armed_position_and_signal(&manager).await;
    fail_event_log_writes(&pool, &audit_write("completed")).await;
    let entered = manager.handle_signal(signal).await;
    restore_event_log_writes(&pool).await;
    assert!(entered.is_ok(), "{entered:?}");
    assert_eq!(manager.exchange().protection_call_count(), 1, "the stop went on");
    let leg = manager.get_position(armed.id).await.unwrap().unwrap();
    assert!(matches!(leg.state, PositionState::Active { .. }), "{:?}", leg.state);
}

/// A manager of `tenant_id` whose journal is the PostgreSQL one, so its
/// suspensions land in `stream_suspensions`: one process of that tenant.
#[cfg(feature = "postgres")]
fn process_of(
    pool: &sqlx::PgPool,
    tenant_id: Uuid,
) -> Arc<PositionManager<StubExchange, MemoryStore>> {
    let order_journal = Arc::new(robson_exec::order_journal::OrderJournal::new(
        Arc::new(crate::order_journal_store::PgJournalBackend::new(pool, tenant_id)),
        &VenueId::new("test:venue"),
    ));
    let store = Arc::new(MemoryStore::new());
    let executor = Executor::new(
        Arc::new(StubExchange::new(dec!(95000))),
        Arc::new(IntentJournal::new()),
        store.clone(),
    )
    .with_order_journal(order_journal);
    Arc::new(
        PositionManager::with_approval_policy(
            Engine::new(RiskConfig::new(dec!(10000)).unwrap()),
            Arc::new(executor),
            store,
            Arc::new(EventBus::new(100)),
            Arc::new(TracingQueryRecorder),
            ApprovalPolicy::new(Decimal::from(100u32), 300),
            TradingPolicy::default(),
        )
        .with_event_log(pool.clone(), tenant_id),
    )
}

#[cfg(feature = "postgres")]
fn stop_failed(position_id: PositionId, error: &str) -> Event {
    Event::InsuranceStopFailed {
        position_id,
        stop_price: Price::new(dec!(94000)).unwrap(),
        error: error.to_string(),
        timestamp: chrono::Utc::now(),
    }
}

/// The suspension lands in `stream_suspensions` through the retry set, the
/// next start of the same tenant loads it, and the stream refuses appends
/// there too.
#[cfg(feature = "postgres")]
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_suspension_is_recorded_and_the_next_start_refuses_the_stream(pool: sqlx::PgPool) {
    let tenant_id = Uuid::now_v7();
    let manager = process_of(&pool, tenant_id);
    let position_id = Uuid::now_v7();
    fail_event_log_writes(&pool, "NEW.event_type = 'insurance_stop_failed'").await;
    let failed = manager.persist_event_to_log(&stop_failed(position_id, "first")).await;
    restore_event_log_writes(&pool).await;
    assert!(matches!(failed, Err(DaemonError::EventLog(_))), "{failed:?}");
    assert!(latched(&manager));
    assert_eq!(manager.executor.order_journal().retry_set_depth(), 1);

    assert_eq!(manager.executor.order_journal().flush_pending().await, 1);
    let row = sqlx::query_as::<_, (Uuid, String, Option<i64>, bool)>(
        "SELECT tenant_id, stream_kind, failed_seq, cleared_at IS NULL FROM stream_suspensions \
         WHERE stream_key = $1",
    )
    .bind(format!("position:{position_id}"))
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row, (tenant_id, "position".to_string(), None, true));

    // The next start of this tenant, and one of another.
    let next = process_of(&pool, tenant_id);
    assert_eq!(next.load_stream_suspensions().await.unwrap(), 1);
    assert!(next.event_streams.position_suspended(position_id));
    let refused = next.persist_event_to_log(&stop_failed(position_id, "after restart")).await;
    assert!(matches!(refused, Err(DaemonError::StreamSuspended { .. })), "{refused:?}");
    assert!(stream_rows(&pool, position_id).await.is_empty());
    assert!(!latched(&next), "the latch is this process's");
    assert_eq!(process_of(&pool, Uuid::now_v7()).load_stream_suspensions().await.unwrap(), 0);
}

/// A pool through a proxy in front of this test's database.
#[cfg(feature = "postgres")]
async fn through_proxy(pool: &sqlx::PgPool) -> (robson_testkit::db_proxy::DbProxy, sqlx::PgPool) {
    let options = pool.connect_options();
    let upstream = tokio::net::lookup_host((options.get_host(), options.get_port()))
        .await
        .unwrap()
        .next()
        .unwrap();
    let proxy = robson_testkit::db_proxy::DbProxy::start(upstream).await.unwrap();
    let database = options.get_database().expect("sqlx::test names its database");
    let url = robson_testkit::daemon_process::database_url_through(
        &std::env::var("DATABASE_URL").unwrap(),
        proxy.addr(),
        database,
    )
    .unwrap();
    let proxied = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .unwrap();
    (proxy, proxied)
}

/// A month's recalibration lands in the log and its acknowledgement is lost,
/// so its apply never runs and the month's stream is suspended. The peak then
/// advances at runtime. At the next start nothing replays it: the stream stays
/// suspended, the base and the peak are what they were, and with that month
/// current admissions stay closed (ADR-0056 step 7, design R6-2).
#[cfg(feature = "postgres")]
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_system_stream_suspended_by_a_lost_ack_is_not_replayed_at_the_next_start(
    pool: sqlx::PgPool,
) {
    use robson_testkit::db_proxy::ProxyMode;
    let tenant_id = Uuid::now_v7();
    let now = chrono::Utc::now();
    sqlx::query(
        "INSERT INTO monthly_state (year, month, capital_base, boundary_reset_at) \
         VALUES ($1, $2, 1000, NOW())",
    )
    .bind(now.year() as i16)
    .bind(now.month() as i16)
    .execute(&pool)
    .await
    .unwrap();
    let month = || async {
        sqlx::query_as::<_, (Decimal, Decimal)>(
            "SELECT capital_base, month_peak_net FROM monthly_state WHERE year = $1 AND month = $2",
        )
        .bind(now.year() as i16)
        .bind(now.month() as i16)
        .fetch_one(&pool)
        .await
        .unwrap()
    };
    let stream_key = format!("system:month_boundary:{:04}-{:02}", now.year(), now.month());
    let rows = || async {
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM event_log WHERE stream_key = $1")
            .bind(&stream_key)
            .fetch_one(&pool)
            .await
            .unwrap()
    };
    let recalibrated = |new_capital_base| Event::CapitalBaseRecalibrated {
        previous_capital_base: dec!(1000),
        new_capital_base,
        wallet_balance: new_capital_base,
        carried_risk: Decimal::ZERO,
        reason: "manual_account_change".to_string(),
        evidence: "test".to_string(),
        month: now.month(),
        year: now.year(),
        timestamp: chrono::Utc::now(),
    };

    let (proxy, proxied) = through_proxy(&pool).await;
    let manager = process_of(&proxied, tenant_id);
    proxy.set_mode(ProxyMode::DropCommitAckOf("INSERT INTO event_log"));
    let lost = manager.emit_domain_event(recalibrated(dec!(900))).await;
    assert!(matches!(lost, Err(DaemonError::EventLog(_))), "{lost:?}");
    assert_eq!(proxy.stats().commit_acks_dropped, 1);
    assert_eq!(rows().await, 1, "the recalibration landed");
    assert_eq!(month().await, (dec!(1000), Decimal::ZERO), "and was never applied");
    assert!(manager.event_streams.month_suspended(now.year(), now.month()));
    assert_eq!(manager.executor.order_journal().flush_pending().await, 1);
    // The runtime peak, written as `refresh_month_peak_net_from_snapshot` does.
    sqlx::query("UPDATE monthly_state SET month_peak_net = 50 WHERE year = $1 AND month = $2")
        .bind(now.year() as i16)
        .bind(now.month() as i16)
        .execute(&pool)
        .await
        .unwrap();

    let next = process_of(&pool, tenant_id);
    assert_eq!(next.load_stream_suspensions().await.unwrap(), 1);
    assert_eq!(next.repair_suspended_position_streams().await, (0, 0), "positions only");
    assert!(next.event_streams.month_suspended(now.year(), now.month()));
    let armed = next
        .arm_position(
            Symbol::from_pair("BTCUSDT").unwrap(),
            Side::Long,
            create_test_risk_config(),
            None,
            Uuid::now_v7(),
        )
        .await;
    assert!(matches!(armed, Err(DaemonError::AdmissionsClosed { .. })), "{armed:?}");
    assert!(!latched(&next));
    let refused = next.emit_domain_event(recalibrated(dec!(800))).await;
    assert!(matches!(refused, Err(DaemonError::StreamSuspended { .. })), "{refused:?}");
    assert_eq!(rows().await, 1);
    assert_eq!(month().await, (dec!(1000), dec!(50)), "nothing replayed; the peak stands");
}

/// Design §8, generated interleavings 2: writers on one or two streams, run
/// concurrently with an injected failure, a failed append or a failed apply.
/// On each stream nothing is appended after the failed write, every appended
/// event but a failed apply is applied, and `last_seq` skips none of them.
#[cfg(feature = "postgres")]
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn generated_writers_never_append_past_a_failure(pool: sqlx::PgPool) {
    use proptest::{
        prelude::*,
        strategy::{Strategy, ValueTree},
        test_runner::TestRunner,
    };

    let strategy = (
        1usize..=2,
        2usize..=6,
        prop::option::of((0usize..6, any::<bool>())),
        prop::collection::vec(0u64..4, 6),
    );
    let mut runner = TestRunner::deterministic();
    // What the sampled cases covered, so the property cannot pass vacuously.
    let (mut append_failures, mut apply_failures, mut refusals) = (0, 0, 0);
    for case in 0..32 {
        let (streams, writers, failure, delays) = strategy.new_tree(&mut runner).unwrap().current();
        let failure = failure.filter(|(writer, _)| *writer < writers);
        let manager = process_of(&pool, Uuid::now_v7());
        let legs: Vec<PositionId> = (0..streams).map(|_| Uuid::now_v7()).collect();
        for leg in &legs {
            manager
                .persist_event_to_log(&Event::PositionArmed {
                    position_id: *leg,
                    account_id: Uuid::now_v7(),
                    symbol: Symbol::from_pair("BTCUSDT").unwrap(),
                    side: Side::Long,
                    tech_stop_distance: None,
                    stop_policy: StopPolicy::ExecutableSpan,
                    stop_buffer_bps_at_arm: None,
                    timestamp: chrono::Utc::now(),
                })
                .await
                .unwrap();
        }
        // The failing write carries `human_confirmation`; no other does.
        match failure {
            Some((_, true)) => {
                fail_event_log_writes(
                    &pool,
                    "NEW.payload->>'approval_policy' = 'human_confirmation'",
                )
                .await
            },
            Some((writer, false)) => {
                let condition = "NEW.approval_mode = 'human_confirmation'";
                fail_projection_updates(&pool, legs[writer % streams], condition).await;
            },
            None => {},
        }
        let base = chrono::Utc::now();
        let mut tasks = Vec::new();
        for writer in 0..writers {
            let manager = Arc::clone(&manager);
            let leg = legs[writer % streams];
            let delay = delays[writer];
            let fails = failure.is_some_and(|(w, _)| w == writer);
            tasks.push(tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                let event = Event::EntryPolicyResolved {
                    position_id: leg,
                    entry_policy: EntryPolicy::ConfirmedTrend,
                    approval_policy: if fails {
                        robson_domain::ApprovalPolicy::HumanConfirmation
                    } else {
                        robson_domain::ApprovalPolicy::Automatic
                    },
                    // The event log dedupes on everything but timestamps.
                    strategy_id: Some(robson_domain::StrategyId {
                        name: format!("writer-{writer}"),
                        version: 1,
                    }),
                    timestamp: base + chrono::Duration::milliseconds(writer as i64),
                };
                (leg, fails, manager.persist_event_to_log(&event).await)
            }));
        }
        let mut outcomes = Vec::new();
        for task in tasks {
            outcomes.push(task.await.unwrap());
        }
        sqlx::query("DROP TRIGGER IF EXISTS injected_event_log_failure ON event_log")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DROP TRIGGER IF EXISTS injected_projection_failure ON positions_current")
            .execute(&pool)
            .await
            .unwrap();

        for leg in &legs {
            let label = format!("case {case}: {streams} streams, {writers} writers, {failure:?}");
            let mine: Vec<_> = outcomes.iter().filter(|(l, _, _)| l == leg).collect();
            let ok = mine.iter().filter(|(_, _, r)| r.is_ok()).count();
            let failed: Vec<_> = mine
                .iter()
                .filter(|(_, _, r)| matches!(r, Err(DaemonError::EventLog(_))))
                .collect();
            let refused = mine
                .iter()
                .filter(|(_, _, r)| matches!(r, Err(DaemonError::StreamSuspended { .. })))
                .count();
            assert_eq!(ok + failed.len() + refused, mine.len(), "{label}");
            assert!(failed.len() <= 1, "{label}");
            assert!(
                failed.iter().all(|(_, fails, _)| *fails),
                "{label}: only the marked write fails"
            );
            assert!(refused == 0 || failed.len() == 1, "{label}: refused only behind a failure");
            refusals += refused;
            if failed.len() == 1 {
                match failure {
                    Some((_, true)) => append_failures += 1,
                    _ => apply_failures += 1,
                }
            }
            assert_eq!(
                manager.event_streams.position_suspended(*leg),
                failed.len() == 1,
                "{label}"
            );

            let rows: Vec<i64> =
                stream_rows(&pool, *leg).await.into_iter().map(|(seq, _)| seq).skip(1).collect();
            assert_eq!(rows, (2..2 + rows.len() as i64).collect::<Vec<_>>(), "{label}: contiguous");
            let apply_failed = failed.len() == 1 && matches!(failure, Some((_, false)));
            assert_eq!(
                rows.len(),
                ok + usize::from(apply_failed),
                "{label}: nothing after the failure"
            );
            let last_seq: i64 =
                sqlx::query_scalar("SELECT last_seq FROM positions_current WHERE position_id = $1")
                    .bind(leg)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            let applied_through = 1 + ok as i64;
            assert_eq!(last_seq, applied_through, "{label}: last_seq skips nothing applied");
        }
    }
    assert!(
        append_failures > 0 && apply_failures > 0 && refusals > 0,
        "{append_failures} {apply_failures} {refusals}"
    );
}

// =============================================================================
// Startup repair of suspended position streams (part 3)
// =============================================================================

/// A position stream that stays suspended invalidates its leg's snapshot
/// through the latent-failure path: no remaining budget while it is open.
#[cfg(feature = "postgres")]
#[tokio::test]
async fn a_leg_whose_stream_stays_suspended_invalidates_the_snapshot() {
    let manager = create_test_manager().await;
    let (position, _) = active_leg_with_live_stop(&manager).await;
    manager.event_streams.restore([(
        format!("position:{}", position.id),
        crate::event_streams::Suspended {
            suspension_id: Uuid::now_v7(),
            stream: crate::event_streams::ManagedStream::Position(position.id),
            repaired_through_seq: None,
        },
    )]);

    let (snapshot, _, _, failures) = manager
        .canonical_budget_snapshot_with_positions(chrono::Utc::now())
        .await
        .unwrap();

    assert!(snapshot.latent_risk_invalid);
    assert_eq!((snapshot.remaining_active, snapshot.slots_active), (Decimal::ZERO, 0));
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0].position_id, position.id);
    assert!(failures[0].reason.contains("suspended"), "{}", failures[0].reason);
}

/// A leg whose `position_armed` applied and an open suspension of its stream:
/// with [`append_unapplied`], what a process that died with a suspended stream
/// leaves.
#[cfg(feature = "postgres")]
async fn suspended_leg(pool: &sqlx::PgPool, tenant_id: Uuid) -> PositionId {
    let position_id = Uuid::now_v7();
    process_of(pool, tenant_id)
        .persist_event_to_log(&Event::PositionArmed {
            position_id,
            account_id: Uuid::now_v7(),
            symbol: Symbol::from_pair("BTCUSDT").unwrap(),
            side: Side::Long,
            tech_stop_distance: None,
            stop_policy: StopPolicy::ExecutableSpan,
            stop_buffer_bps_at_arm: None,
            timestamp: chrono::Utc::now(),
        })
        .await
        .unwrap();
    let stream_key = format!("position:{position_id}");
    sqlx::query(
        "INSERT INTO stream_suspensions (suspension_id, tenant_id, stream_key, stream_kind, \
         failed_seq, error, suspended_at) VALUES ($1, $2, $3, 'position', 2, 'injected', NOW())",
    )
    .bind(Uuid::now_v7())
    .bind(tenant_id)
    .bind(&stream_key)
    .execute(pool)
    .await
    .unwrap();
    position_id
}

/// Land `suffix` on `position_id`'s stream without applying it.
#[cfg(feature = "postgres")]
async fn append_unapplied(
    pool: &sqlx::PgPool,
    tenant_id: Uuid,
    position_id: PositionId,
    suffix: Vec<(String, serde_json::Value)>,
) {
    let stream_key = format!("position:{position_id}");
    for (event_type, payload) in suffix {
        let event = robson_eventlog::Event::new(tenant_id, &stream_key, &event_type, payload);
        robson_eventlog::append_event(pool, &stream_key, None, event).await.unwrap();
    }
}

/// Trailing updates of `position_id`, one per stop, each to a stop of its own.
#[cfg(feature = "postgres")]
fn trailing_updates(
    position_id: PositionId,
    stops: std::ops::Range<i64>,
) -> Vec<(String, serde_json::Value)> {
    stops
        .map(|stop| {
            let event = Event::TrailingStopUpdated {
                position_id,
                previous_stop: Price::new(Decimal::from(stop)).unwrap(),
                new_stop: Price::new(Decimal::from(stop + 1)).unwrap(),
                trigger_price: Price::new(Decimal::from(stop + 1)).unwrap(),
                timestamp: chrono::Utc::now(),
            };
            (event.event_type().to_string(), serde_json::to_value(&event).unwrap())
        })
        .collect()
}

/// The leg's projection and its suspension's row: `last_seq`, the trailing
/// stop, `repaired_through_seq`, `repair_error` set, cleared.
#[cfg(feature = "postgres")]
async fn repair_state(
    pool: &sqlx::PgPool,
    position_id: PositionId,
) -> (i64, Option<Decimal>, Option<i64>, bool, bool) {
    let (last_seq, trailing) = sqlx::query_as::<_, (i64, Option<Decimal>)>(
        "SELECT last_seq, trailing_stop_price FROM positions_current WHERE position_id = $1",
    )
    .bind(position_id)
    .fetch_one(pool)
    .await
    .unwrap();
    let (through, errored, cleared) = sqlx::query_as::<_, (Option<i64>, bool, bool)>(
        "SELECT repaired_through_seq, repair_error IS NOT NULL, cleared_at IS NOT NULL \
         FROM stream_suspensions WHERE stream_key = $1",
    )
    .bind(format!("position:{position_id}"))
    .fetch_one(pool)
    .await
    .unwrap();
    (last_seq, trailing, through, errored, cleared)
}

/// A suffix of three pages and a little more: validated, applied in seq order
/// with a checkpoint per page, then cleared, and the stream takes appends
/// again.
#[cfg(feature = "postgres")]
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_suspended_position_stream_is_repaired_across_pages(pool: sqlx::PgPool) {
    let tenant_id = Uuid::now_v7();
    let position_id = suspended_leg(&pool, tenant_id).await;
    append_unapplied(&pool, tenant_id, position_id, trailing_updates(position_id, 1000..2100))
        .await;
    let process = process_of(&pool, tenant_id);
    assert_eq!(process.load_stream_suspensions().await.unwrap(), 1);

    assert_eq!(process.repair_suspended_position_streams().await, (1, 0));

    assert_eq!(
        repair_state(&pool, position_id).await,
        (1101, Some(dec!(2100)), Some(1101), false, true)
    );
    assert!(!process.event_streams.position_suspended(position_id));
    process
        .persist_event_to_log(&stop_failed(position_id, "after the repair"))
        .await
        .unwrap();
    assert_eq!(stream_rows(&pool, position_id).await.len(), 1102);
}

/// A repair interrupted on its second page keeps the first page's checkpoint
/// and stays suspended. The next start resumes past that checkpoint and
/// clears it. The first page is audit-only, so the checkpoint is ahead of the
/// leg's `last_seq` and only the checkpoint says where to resume.
#[cfg(feature = "postgres")]
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn an_interrupted_repair_resumes_past_its_checkpoint(pool: sqlx::PgPool) {
    let tenant_id = Uuid::now_v7();
    let id = suspended_leg(&pool, tenant_id).await;
    let ticks = (0..500)
        .map(|n| ("position_monitor_tick".to_string(), serde_json::json!({ "tick": n })))
        .collect();
    append_unapplied(&pool, tenant_id, id, ticks).await;
    append_unapplied(&pool, tenant_id, id, trailing_updates(id, 1000..1200)).await;
    // Seq 502, the second page's first event, moves the stop to 1001: its
    // apply fails.
    fail_projection_updates(&pool, id, "NEW.trailing_stop_price = 1001").await;
    let first = process_of(&pool, tenant_id);
    first.load_stream_suspensions().await.unwrap();
    assert_eq!(first.repair_suspended_position_streams().await, (0, 1));
    assert_eq!(repair_state(&pool, id).await, (1, None, Some(501), true, false));
    assert!(first.event_streams.position_suspended(id));
    let refused = first.persist_event_to_log(&stop_failed(id, "still suspended")).await;
    assert!(matches!(refused, Err(DaemonError::StreamSuspended { .. })), "{refused:?}");

    sqlx::query("DROP TRIGGER injected_projection_failure ON positions_current")
        .execute(&pool)
        .await
        .unwrap();
    let next = process_of(&pool, tenant_id);
    next.load_stream_suspensions().await.unwrap();
    let (stream_key, suspended) = next.event_streams.suspended_positions().remove(0);
    let outcome = crate::event_streams::repair_position_stream(
        &pool,
        tenant_id,
        &next.event_streams,
        &stream_key,
        &suspended,
    )
    .await;
    assert_eq!(outcome, crate::event_streams::RepairOutcome::Cleared { applied: 200 });
    assert_eq!(repair_state(&pool, id).await, (701, Some(dec!(1200)), Some(701), false, true));
}

/// A type with no guarded position handler anywhere in the suffix means no
/// repair at all: not even the valid events ahead of it apply.
#[cfg(feature = "postgres")]
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_suffix_with_a_non_position_type_is_not_repaired(pool: sqlx::PgPool) {
    let tenant_id = Uuid::now_v7();
    let id = suspended_leg(&pool, tenant_id).await;
    let mut suffix = trailing_updates(id, 1000..1003);
    suffix.insert(2, ("month_boundary_reset".to_string(), serde_json::json!({ "probe": true })));
    append_unapplied(&pool, tenant_id, id, suffix).await;
    let process = process_of(&pool, tenant_id);
    process.load_stream_suspensions().await.unwrap();

    assert_eq!(process.repair_suspended_position_streams().await, (0, 1));

    assert_eq!(repair_state(&pool, id).await, (1, None, None, true, false));
    assert!(process.event_streams.position_suspended(id));
}
