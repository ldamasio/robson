use anyhow::{anyhow, ensure, Context};
use robson_connectors::inventory_loopback::{InventoryBody, InventoryRead};
use sqlx::{postgres::PgRow, PgPool, Row};

use super::*;
use crate::{
    process_epoch,
    venue_budget::{self, bootstrap, restore, Trace},
};

#[path = "pg_tests/http.rs"]
mod http;

#[cfg(all(test, feature = "crash-points"))]
#[path = "c8_account_http.rs"]
mod c8_account_http;

#[cfg(all(test, feature = "crash-points"))]
#[path = "c8_attribution.rs"]
mod c8_attribution;

#[path = "pg_tests/restart_lookup.rs"]
pub(super) mod restart_lookup;

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn actual_bootstrap_time_sample_and_closing(pool: PgPool) -> anyhow::Result<()> {
    // Fixture ceiling only: the real native 120s barrier, HTTP 10s, B2 <=2s,
    // cadence 60s and production prerequisites below remain unchanged.
    let body_deadline = Instant::now() + Duration::from_secs(150);
    let tenant = Uuid::now_v7();
    let mut epoch = None;
    let mut owner = None;
    let mut http = None;
    let mut account_http = None;
    let mut account_slot: Option<inventory::AccountPreparation> = None;
    let mut supersession_http = [None, None];
    let mut supersession_slots = [None, None];
    let mut lookup_control_http = None;
    let mut lookup_control_slot = None;
    let primary = timeout_at(body_deadline, async {
        ensure!(matches!(bootstrap::production_input(), Err(BudgetError::PrerequisiteRefusal)));
        let clock = Arc::new(NativeClock::initialize()?);
        epoch = Some(process_epoch::start_epoch(&pool, tenant, "I1-controlled-test").await?);
        let start = epoch.as_ref().context("committed test epoch")?;
        let setup = Trace::default();
        // Seven actual SQL commands in an empty sqlx macro DB; this sealed
        // descriptive fixture is not the missing production C2 producer.
        let input = venue_budget::fixture::prepare(
            &pool, &clock, Instant::now() + Duration::from_secs(2), &setup,
        ).await.map_err(|e| anyhow!("fixture: {e:?}"))?;
        ensure!(setup.issued() == 7, "fixture SQL count");
        let trace = Trace::default();
        let mut rows = bootstrap::observe_bootstrap(
            &pool, tenant, start, &input, Instant::now() + Duration::from_secs(2), &trace,
        ).await.map_err(|e| anyhow!("bootstrap: {e:?}"))?;
        ensure!(trace.issued() == 6, "bootstrap SQL count");
        restore::existing_pause(
            &pool, tenant, &mut rows, &clock, Instant::now() + Duration::from_secs(2), &trace,
        ).await.map_err(|e| anyhow!("restore: {e:?}"))?;
        ensure!(trace.issued() == 6, "absent pause issues no SQL");
        let budget = SharedBudget::from_rows(rows, start.predecessor.unclean_reason().is_some())
            .map_err(|e| anyhow!("budget: {e:?}"))?;
        // The listener also covers the unchanged shared stop deadline if
        // the primary reaches its fixture ceiling. No extra request allowance.
        http = Some(http::Fixture::start(
            body_deadline + crate::stop::StopTimings::default().deadline,
        ).await?);
        let fixture = http.as_ref().context("owned HTTP fixture")?;
        let client = Arc::new(BinanceRestClient::with_harness_base_url(
            fixture.base_url.clone(), String::new(), String::new(),
        ));
        let store = ClockSampleStore::from_committed_epoch(&pool, tenant, start);
        owner = Some(ClockOwner::controlled(
            Arc::clone(&clock), client, store, budget.clone(), tenant, start.epoch.epoch_id,
        ).map_err(|e| anyhow!("owner: {e:?}"))?);
        let owner = owner.as_mut().context("owned TIME component")?;
        let initial: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM clock_samples WHERE process_epoch=$1",
        ).bind(start.epoch.epoch_id).fetch_one(&pool).await?;
        ensure!(initial == 0, "no initial sample row");
        ensure!(matches!(owner.commission_signing_time(), Err(TimeError::Native)),
            "ACCOUNT has no signing authority before a positively published sample");
        ensure!(matches!(owner.run_cycle().await, Err(TimeError::Budget(BudgetError::Deferred))),
            "no TIME before the real unclean barrier");
        ensure!(budget.handoffs().map_err(|e| anyhow!("handoffs: {e:?}"))? == 0);
        ensure!(fixture.connections() == 0, "no physical connection before native barrier");
        // Real elapsed time, with a real native comparison after waking.
        tokio::time::sleep(Duration::from_secs(120)).await;
        let now = clock.capture()?;
        ensure!(now.elapsed_since(&budget.start().map_err(|e| anyhow!("start: {e:?}"))?)?
            >= Duration::from_secs(120));
        let waiting = budget.protective_waiter().map_err(|e| anyhow!("waiter: {e:?}"))?;
        ensure!(matches!(budget.try_reserve_time(now, after(now, Duration::from_secs(10))
            .map_err(|e| anyhow!("deadline: {e:?}"))?), Err(BudgetError::Deferred)));
        ensure!(budget.handoffs().map_err(|e| anyhow!("handoffs: {e:?}"))? == 0);
        ensure!(fixture.connections() == 0, "protective waiter still permits no physical TIME");
        drop(waiting);
        let before = clock.capture()?;
        let sample = owner.run_cycle().await.map_err(|e| anyhow!("TIME cycle: {e:?}"))?;
        let after = clock.capture()?;
        ensure!(budget.handoffs().map_err(|e| anyhow!("handoffs: {e:?}"))? == 1);
        ensure!(fixture.connections() == 1, "actual physical 0 -> 1 connection");
        ensure!(owner.preparation.is_none() && owner.parser.is_none(), "completed jobs consumed");
        // Independent readback is fixture work, outside the TIME operation.
        let rows = sqlx::query("SELECT sample_id,tenant_id,venue_id,process_epoch,server_ms,send_wall_seconds,send_wall_nanoseconds,send_tick_before::text AS sb,send_tick_after::text AS sa,receive_wall_seconds,receive_wall_nanoseconds,receive_tick_before::text AS rb,receive_tick_after::text AS ra,(clock_domain).boot_uuid AS boot,(clock_domain).offset_seconds AS os,(clock_domain).offset_nanoseconds AS ons,theta2_ns::text AS theta,r_low_ns::text AS low,r_high_ns::text AS high,u2_ns::text AS bound,numeric_failures FROM clock_samples WHERE process_epoch=$1")
            .bind(start.epoch.epoch_id).fetch_all(&pool).await?;
        ensure!(rows.len() == 1, "exactly one persisted original row");
        verify_row(&rows[0], sample, tenant, start.epoch.epoch_id, before, after,
            fixture.server_ms()?)?;
        ensure!(matches!(owner.run_cycle().await, Err(TimeError::Budget(BudgetError::Deferred))),
            "the existing 60s cadence prevents immediate replay");
        ensure!(budget.handoffs().map_err(|e| anyhow!("handoffs: {e:?}"))? == 1);
        ensure!(owner.state.lock().map_err(|_| anyhow!("state lock poisoned"))?
            .published.as_ref().is_some_and(|p| p.id == sample), "actual publication before Closing");
        ensure!(!owner.state.lock().map_err(|_| anyhow!("state lock poisoned"))?.closing,
            "positive path remains open until the real stop");

        // C8 inventory triple, no capital: the same owner, SharedBudget and
        // positively read-back publication. Listener setup is fixture work.
        account_http = Some(c8_account_http::AccountFixture::triple(
            body_deadline + crate::stop::StopTimings::default().deadline,
        ).await?);
        let venue = account_http.as_ref().context("owned inventory fixture")?;
        let read_deadline = Instant::now() + Duration::from_secs(10);
        let claim = clock.capture()?;
        let signed = owner.commission_signing_time().map_err(|e| anyhow!("signing: {e:?}"))?;
        ensure!(budget.handoffs().map_err(|e| anyhow!("handoffs: {e:?}"))? == 1);
        ensure!(venue.connections() == 0, "nothing sent before the admitted first poll");
        let triple = owner
            .read_inventory_triple(&signed, claim, &mut account_slot, venue.port, read_deadline)
            .await.map_err(|e| anyhow!("inventory triple: {e:?}"))?;
        ensure!(account_slot.is_none(), "completed preparations consumed");
        for (receipt, read) in triple.receipts.iter().zip(InventoryRead::ALL) {
            ensure!(receipt.read == read && receipt.sample == sample,
                "each EOF receipt judged on the same published sample");
            ensure!(receipt.send.epoch == start.epoch.epoch_id
                && receipt.receive.epoch == start.epoch.epoch_id
                && receipt.send.wall_agrees(&receipt.receive)?
                && receipt.receive.end_tick()?.elapsed_since(&receipt.send.end_tick()?).is_ok(),
                "original first poll and EOF brackets share the committed epoch and ordered TIME");
        }
        ClockOwner::check_triple(&triple.receipts)
            .map_err(|e| anyhow!("triple provenance: {e:?}"))?;
        ensure!(matches!(&triple.receipts[1].body, InventoryBody::OpenOrders(o)
                if o.len() == 2 && o.iter().any(|o| o.symbol == "zzzQQQ"))
            && matches!(&triple.receipts[2].body, InventoryBody::OpenOrderLists(l)
                if l.len() == 1 && l[0].orders.len() == 1),
            "every row and raw symbol kept");
        ensure!(venue.signing_stamps()? == vec![signed.timestamp_ms(); 3],
            "actual wire stamps are the original signed one");
        ensure!(budget.handoffs().map_err(|e| anyhow!("handoffs: {e:?}"))? == 4,
            "TIME 1 + three inventory handoffs on the one SharedBudget");
        ensure!(venue.connections() == 3 && fixture.connections() == 1,
            "one physical connection per read, no further TIME");
        verify_triple_budget(&budget, &clock)?;
        commission_admission_precedence(owner, &clock)?;
        for (i, replace_generation) in [false, true].into_iter().enumerate() {
            supersession_during_account_body(
                owner, &clock, replace_generation, body_deadline,
                &mut supersession_http[i], &mut supersession_slots[i],
            ).await?;
        }
        ensure!(venue.connections() == 3 && budget.handoffs()
            .map_err(|e| anyhow!("handoffs: {e:?}"))? == 6,
            "one TIME, one accepted triple and two charged rejected ACCOUNT reads");
        eprintln!("C8_INVENTORY_GUARDED time=1 inventory=3 superseded_account=2 handoffs=6 weight_before_faults=107 weight_total=147 no_venue_orders=true");
        superseded_owned_order(
            owner, &clock, body_deadline, &mut lookup_control_http, &mut lookup_control_slot,
        ).await?;
        ensure!(budget.handoffs().map_err(|e| anyhow!("{e:?}"))? == 7,
            "one new charged lookup survives supersession");
        // 400 - (107 + 2 * 20 + 4) = 249: twelve ACCOUNT reservations,
        // then exactly nine unit weights. No fresh lease or refund on EOF.
        verify_budget_remaining(&budget, &clock, 12, 9)?;
        eprintln!("C8_LOOKUP_CLOCK_CONTROL superseded_http_400=1 retained_lookup_weight=4 connections=1");

        Ok::<_, anyhow::Error>(())
    }).await.map_err(|_| anyhow!("150s fixture body ceiling elapsed")).and_then(|r| r);

    // Returned errors and the body ceiling all reach teardown. Only a failed
    // primary cancels before spawn: the real worker retires its owned jobs.
    // The happy path stays open until run_stop_with_clock invalidates it.
    let asked_at = Instant::now();
    let deadline = asked_at + crate::stop::StopTimings::default().deadline;
    // The test-owned ACCOUNT preparation retires on every path, before the
    // owner's worker permit is needed by its own retirement.
    let account_retired = timeout_at(deadline, ClockOwner::retire_account(&mut account_slot)).await;
    let mut supersession_errors = Vec::new();
    for (fixture, slot) in supersession_http.into_iter().zip(&mut supersession_slots) {
        if let Some(fixture) = fixture {
            if let Err(error) = fixture.finish(deadline).await {
                supersession_errors.push(format!("supersession listener retirement: {error}"));
            }
        }
        let retired = timeout_at(deadline, ClockOwner::retire_account(slot)).await;
        if !matches!(retired, Ok(true)) {
            supersession_errors.push(format!("supersession preparation retirement: {retired:?}"));
        }
    }
    if let Some(fixture) = lookup_control_http {
        if let Err(error) = fixture.finish(deadline).await {
            supersession_errors.push(format!("lookup control listener retirement: {error}"));
        }
    }
    let retired = timeout_at(deadline, jobs::retire(&mut lookup_control_slot)).await;
    if !matches!(retired, Ok(true)) {
        supersession_errors.push(format!("lookup control preparation retirement: {retired:?}"));
    }
    let closing = owner.take().map(|owner| {
        // No await or spawned owner can advance this original baseline.
        let expected_handoffs = owner.budget.handoffs();
        if primary.is_err() {
            owner.cancel.cancel();
        }
        let state = Arc::clone(&owner.state);
        let budget = owner.budget.clone();
        let closing = owner.spawn();
        (closing, state, budget, expected_handoffs)
    });
    let mut failures = supersession_errors;
    if !matches!(account_retired, Ok(true)) {
        failures.push(format!("ACCOUNT preparation retirement: {account_retired:?}"));
    }
    if let Some((closing, state, budget, expected_handoffs)) = closing {
        let result =
            close_owner(&closing, &state, &budget, expected_handoffs, epoch.as_ref(), asked_at)
                .await;
        if let Err(e) = result {
            failures.push(format!("owner retirement: {e:#}"));
        }
    }
    // Keep accepting/detecting physical requests through all owner retirement.
    if let Some(http) = http.take() {
        if let Err(e) = http.finish(deadline).await {
            failures.push(format!("HTTP retirement: {e:#}"));
        }
    }
    if let Some(account_http) = account_http.take() {
        if let Err(e) = account_http.finish(deadline).await {
            failures.push(format!("ACCOUNT HTTP retirement: {e:#}"));
        }
    }
    // Check the real marker even when the primary sample path failed.
    if let Some(start) = epoch.as_ref() {
        let unmarked = timeout_at(deadline, sqlx::query_scalar::<_, bool>(
            "SELECT clean_stop_at IS NULL AND marker_version IS NULL FROM process_epochs WHERE epoch_id=$1",
        ).bind(start.epoch.epoch_id).fetch_one(&pool)).await;
        if !matches!(unmarked, Ok(Ok(true))) {
            failures.push(format!("real epoch must remain unmarked: {unmarked:?}"));
        }
    }
    if timeout_at(deadline, pool.close()).await.is_err() {
        failures.push("pool close exceeded the shared original stop deadline".into());
    }
    let cleanup = if failures.is_empty() {
        Ok(())
    } else {
        Err(anyhow!(failures.join("; ")))
    };
    eprintln!(
        "I1_FINAL primary={:?} cleanup={:?}",
        primary.as_ref().err(),
        cleanup.as_ref().err()
    );
    match (primary, cleanup) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(p), Ok(())) => Err(p),
        (Ok(()), Err(c)) => Err(c),
        (Err(p), Err(c)) => Err(anyhow!("primary: {p:#}; cleanup: {c:#}")),
    }
}

/// A held observation above the native accounting basis. No writer
/// receives this fixture's evidence and no durable acknowledgement exists.
pub(crate) struct HeldLookupWitness {
    pub(crate) sample: Uuid,
    pub(crate) lookup_sample: Uuid,
    pub(crate) lookup_send: RawClockCapture,
    pub(crate) lookup_receive: RawClockCapture,
    pub(crate) intent: Uuid,
    pub(crate) generation: Uuid,
    pub(crate) durable_executed: rust_decimal::Decimal,
    pub(crate) lookup_executed: rust_decimal::Decimal,
    pub(crate) lookup_handoffs: u64,
    pub(crate) cumulative_handoffs: u64,
    pub(crate) lookup_sql: u64,
}

/// Consumes the actual C2 bootstrap rows from the held physical restore.
#[allow(
    clippy::too_many_arguments,
    reason = "the held rehearsal's physical bindings, each checked separately by its caller"
)]
pub(crate) async fn run_held_inventory_rehearsal(
    pool: &PgPool,
    wire: &robson_testkit::db_proxy::DbProxy,
    start: &process_epoch::EpochStart,
    tenant: Uuid,
    clock: Arc<NativeClock>,
    mut rows: venue_budget::BootstrapRows,
    installed: &crate::spot_order_set_store::JournalGeneration,
    uid: &str,
    version: i64,
    operated: &str,
    owned: Uuid,
) -> anyhow::Result<HeldLookupWitness> {
    use c8_attribution::{Divergence, Fact, ListFact, Refusal, Scope};
    use robson_domain::OrderSide;
    use robson_exec::venue::{OrderObservation, OrderPhase};
    use rust_decimal::Decimal;
    let venue_id = VenueId::new(VenueId::BINANCE_SPOT);
    let scope = Scope {
        expected: crate::spot_order_set_store::ExpectedGeneration {
            tenant_id: installed.tenant_id,
            venue: &venue_id,
            account_scope: &installed.account_scope,
            generation_id: installed.generation_id,
            process_epoch: installed.process_epoch,
            source_audit_id: installed.source_audit_id,
        },
        uid,
        version,
        operated,
    };
    let attribution_options = pool.connect_options().as_ref().clone().options([
        ("default_transaction_read_only", "on"),
        ("lock_timeout", "300ms"),
        ("statement_timeout", "1s"),
    ]);
    let deadline = Instant::now() + Duration::from_secs(135);
    let mut time_http = None;
    let mut inventory_http = None;
    let mut lookup_http = None;
    let mut lookup_slot = None;
    let mut owner = None;
    let mut slot = None;
    let primary = timeout_at(deadline, async {
        ensure!(start.predecessor.unclean_reason().is_some());
        ensure!(matches!(bootstrap::production_input(), Err(BudgetError::PrerequisiteRefusal)));
        let restore_trace = Trace::default();
        restore::existing_pause(pool, tenant, &mut rows, &clock, deadline, &restore_trace)
            .await
            .map_err(|e| anyhow!("held pause: {e:?}"))?;
        ensure!(restore_trace.issued() == 0, "empty fixture has no restored pause");
        let budget =
            SharedBudget::from_rows(rows, true).map_err(|e| anyhow!("held budget: {e:?}"))?;

        time_http = Some(http::Fixture::start(deadline).await?);
        let time = time_http.as_ref().context("owned TIME listener")?;
        let client = Arc::new(BinanceRestClient::with_harness_base_url(
            time.base_url.clone(),
            String::new(),
            String::new(),
        ));
        let store = ClockSampleStore::from_committed_epoch(pool, tenant, start);
        owner = Some(
            ClockOwner::controlled(
                Arc::clone(&clock),
                client,
                store,
                budget.clone(),
                tenant,
                start.epoch.epoch_id,
            )
            .map_err(|e| anyhow!("held clock owner: {e:?}"))?,
        );
        let owner = owner.as_mut().context("owned ClockOwner")?;
        ensure!(matches!(owner.run_cycle().await, Err(TimeError::Budget(BudgetError::Deferred))));
        ensure!(time.connections() == 0 && budget.handoffs().map_err(|e| anyhow!("{e:?}"))? == 0);
        tokio::time::sleep(Duration::from_secs(120)).await;

        let sample = owner.run_cycle().await.map_err(|e| anyhow!("held TIME: {e:?}"))?;
        let durable: Vec<(Uuid, i64)> = sqlx::query_as(
            "SELECT sample_id, server_ms FROM public.clock_samples WHERE tenant_id=$1
             AND process_epoch=$2 AND venue_id='binance:spot' AND numeric_failures=0 LIMIT 2",
        )
        .bind(tenant)
        .bind(start.epoch.epoch_id)
        .fetch_all(pool)
        .await?;
        ensure!(durable == [(sample, time.server_ms()?)]);
        ensure!(time.connections() == 1 && budget.handoffs().map_err(|e| anyhow!("{e:?}"))? == 1);

        inventory_http = Some(c8_account_http::AccountFixture::attribution(deadline).await?);
        let listener = inventory_http.as_ref().context("owned inventory listener")?;
        let scan_deadline = deadline.min(Instant::now() + Duration::from_secs(10));
        let counters = || {
            (
                wire.stats().statements,
                wire.prepared_statement_parses(),
                wire.query_trace().len(),
                wire.query_trace_dropped(),
            )
        };
        // One scoped pre-snapshot, autocommit: nothing is held across HTTP.
        let before_pre = counters();
        let pre = c8_attribution::snapshot(&attribution_options, scope, &[], scan_deadline)
            .await
            .map_err(|e| anyhow!("attribution pre-snapshot: {e:?}"))?;
        ensure!(c8_attribution::attribute(scope, &[], &pre)
            .map_err(|e| anyhow!("pre header: {e:?}"))?
            .is_empty());
        ensure!(counters().0 == before_pre.0 + 1, "one pre-snapshot statement");
        let signed = owner.commission_signing_time().map_err(|e| anyhow!("{e:?}"))?;
        let claim = clock.capture()?;
        let previous = counters();
        ensure!(previous.3 == 0, "bounded restored wire trace");
        let triple = owner
            .read_inventory_triple(&signed, claim, &mut slot, listener.port, scan_deadline)
            .await
            .map_err(|e| anyhow!("held inventory: {e:?}"))?;
        ensure!(counters() == previous, "no SQL execution or preparation during inventory");
        ensure!(triple
            .receipts
            .iter()
            .all(|r| r.sample == sample && r.send.epoch == start.epoch.epoch_id));
        ensure!(
            listener.connections() == 3 && listener.signing_stamps()? == [signed.timestamp_ms(); 3]
        );
        ensure!(budget.handoffs().map_err(|e| anyhow!("{e:?}"))? == 4);
        verify_triple_budget(&budget, &clock)?;

        let (InventoryBody::OpenOrders(orders), InventoryBody::OpenOrderLists(lists)) =
            (&triple.receipts[1].body, &triple.receipts[2].body)
        else {
            return Err(anyhow!("typed inventory order bodies"));
        };
        let selected = c8_attribution::select(scope, orders, lists)
            .map_err(|e| anyhow!("operated subset: {e:?}"))?;
        ensure!(
            selected.unrelated_orders == 1 && selected.unrelated_lists == 1,
            "unrelated pair and list kept, never parsed as another asset"
        );
        ensure!(
            selected.lists
                == [ListFact {
                    order_list_id: 9,
                    list_client_order_id: "c8-operated-list".into(),
                    members: vec![(417012, "c8-absent-sell".into())],
                }],
            "operated list keeps its own native and client identity"
        );
        let post =
            c8_attribution::snapshot(&attribution_options, scope, &selected.orders, scan_deadline)
                .await
                .map_err(|e| anyhow!("attribution post-snapshot: {e:?}"))?;
        ensure!(
            counters().0 == previous.0 + 1 && counters().3 == 0,
            "one bounded post-snapshot statement"
        );
        let facts = c8_attribution::attribute(scope, &selected.orders, &post)
            .map_err(|e| anyhow!("attribution: {e:?}"))?;
        let summary: Vec<_> = facts
            .iter()
            .map(|f| {
                (
                    f.symbol.as_str(),
                    f.order_id,
                    f.client_order_id.as_str(),
                    f.side.as_str(),
                    f.order_list_id,
                    f.fact.clone(),
                )
            })
            .collect();
        ensure!(
            summary
                == [
                    (
                        "BTCUSDT",
                        417011,
                        "physical-recovery-observed-buy",
                        "BUY",
                        -1,
                        Fact::Discovered
                    ),
                    ("BTCUSDT", 417012, "c8-absent-sell", "SELL", 9, Fact::Absent),
                    ("BTCUSDT", 417013, "physical-recovery-owned-buy", "BUY", -1, Fact::Ledger {
                        intent_id: owned
                    }),
                ],
            "exact discovered, absent and owned operated facts: {summary:?}"
        );

        // Negative controls, counted outside the live scan.
        let negatives = counters().0;
        let other_scope = format!("{}-other", installed.account_scope);
        let moved = [
            Scope { uid: "0", ..scope },
            Scope { version: version + 1, ..scope },
            Scope {
                expected: crate::spot_order_set_store::ExpectedGeneration {
                    account_scope: &other_scope,
                    ..scope.expected
                },
                ..scope
            },
            Scope {
                expected: crate::spot_order_set_store::ExpectedGeneration {
                    generation_id: Uuid::now_v7(),
                    ..scope.expected
                },
                ..scope
            },
        ];
        for wrong in moved {
            let refused = c8_attribution::snapshot(
                &attribution_options,
                wrong,
                &selected.orders,
                scan_deadline,
            )
            .await;
            ensure!(
                matches!(refused, Err(Refusal::Scope)),
                "moved identity refused: {:?}",
                refused.as_ref().err()
            );
        }
        ensure!(counters().0 == negatives + 4, "one statement per moved-identity control");
        let many: Vec<_> =
            std::iter::repeat_n(selected.orders[0], c8_attribution::FIXTURE_KEY_CAP + 1).collect();
        ensure!(
            matches!(
                c8_attribution::snapshot(&attribution_options, scope, &many, scan_deadline).await,
                Err(Refusal::Cap)
            ) && counters().0 == negatives + 4,
            "cap refuses before SQL"
        );
        for (mutate, expected) in [(0, Divergence::Client), (1, Divergence::Side)] {
            let mut contrary = selected.orders[0].clone();
            if mutate == 0 {
                contrary.client_order_id = "c8-contrary-client".into();
            } else {
                contrary.side = "SELL".into();
            }
            let keyed = [&contrary, selected.orders[1], selected.orders[2]];
            let judged = c8_attribution::attribute(scope, &keyed, &post)
                .map_err(|e| anyhow!("contrary attribution: {e:?}"))?;
            ensure!(
                judged[0].fact == Fact::Contradiction(expected)
                    && judged[1].fact == Fact::Absent
                    && judged[2].fact == Fact::Ledger { intent_id: owned },
                "contradiction under the exact original key never reads as absence"
            );
        }
        // The same owned post row against a contrary wire type or client.
        for (mutate, expected) in [(0, Divergence::Journal), (1, Divergence::Client)] {
            let mut contrary = selected.orders[2].clone();
            if mutate == 0 {
                contrary.order_type = "LIMIT".into();
            } else {
                contrary.client_order_id = "c8-contrary-owned".into();
            }
            let keyed = [selected.orders[0], selected.orders[1], &contrary];
            let judged = c8_attribution::attribute(scope, &keyed, &post)
                .map_err(|e| anyhow!("contrary owned attribution: {e:?}"))?;
            ensure!(
                judged[0].fact == Fact::Discovered
                    && judged[1].fact == Fact::Absent
                    && judged[2].fact == Fact::Contradiction(expected),
                "owned identity mismatch is never a ledger binding"
            );
        }
        ensure!(counters().0 == negatives + 4, "contrary judgments issue no SQL");
        let durable = facts[2].durable_executed.context("native owned execution basis")?;
        ensure!(durable == Decimal::ZERO, "held fixture basis is exact zero");
        lookup_http = Some(c8_account_http::AccountFixture::owned_order(deadline).await?);
        let lookup_target = lookup_http.as_ref().context("owned lookup listener")?;
        let lookup_due = deadline.min(Instant::now() + Duration::from_secs(5));
        let handoffs_before = budget.handoffs().map_err(|e| anyhow!("{e:?}"))?;
        let lookup_before = counters();
        let lookup_signed = owner.commission_signing_time().map_err(|e| anyhow!("{e:?}"))?;
        let generation = owner.state.lock().map_err(|_| anyhow!("clock lock"))?.generation;
        let stale_claim = clock.capture()?;
        owner.state.lock().map_err(|_| anyhow!("clock lock"))?.generation = generation + 1;
        let refused = owner
            .read_owned_order(
                &lookup_signed,
                stale_claim,
                &mut lookup_slot,
                lookup_target.port,
                lookup_due,
            )
            .await;
        owner.state.lock().map_err(|_| anyhow!("clock lock"))?.generation = generation;
        ensure!(
            matches!(refused, Err(TimeError::Budget(BudgetError::Deferred)))
                && lookup_target.connections() == 0
                && budget.handoffs().map_err(|e| anyhow!("{e:?}"))? == handoffs_before,
            "stale lookup before first poll opens no connection and spends no charge"
        );
        verify_triple_budget(&budget, &clock)?;
        let lookup = owner
            .read_owned_order(
                &lookup_signed,
                clock.capture()?,
                &mut lookup_slot,
                lookup_target.port,
                lookup_due,
            )
            .await
            .map_err(|e| anyhow!("held exact lookup: {e:?}"))?;
        let OrderObservation::Found(record) = lookup.observation else {
            return Err(anyhow!("exact order lookup remained unresolved"));
        };
        ensure!(
            lookup.mark.sample == sample
                && lookup.mark.generation == generation
                && lookup.mark.raw.epoch == start.epoch.epoch_id
                && lookup.receive.epoch == start.epoch.epoch_id
                && lookup.mark.raw.after_ns < lookup.receive.before_ns
                && lookup.mark.raw.wall_agrees(&lookup.receive)?
                && record.symbol == facts[2].symbol
                && record.exchange_order_id == facts[2].order_id.to_string()
                && record.client_order_id == facts[2].client_order_id
                && record.side == OrderSide::Buy
                && record.order_type == "MARKET"
                && record.phase == OrderPhase::Live
                && record.status == "PARTIALLY_FILLED"
                && record.orig_qty == Decimal::ONE
                && record.executed_qty > durable
                && record.executed_qty == Decimal::new(25, 2),
            "typed observation above the exact order's durable basis"
        );
        let total_handoffs = budget.handoffs().map_err(|e| anyhow!("{e:?}"))?;
        ensure!(
            lookup_target.connections() == 1
                && lookup_slot.is_none()
                && lookup_target.signing_stamp()? == lookup_signed.timestamp_ms()
                && total_handoffs == 5
                && handoffs_before == 4
                && counters() == lookup_before,
            "one lookup handoff, no retry and no SQL during that request"
        );
        verify_budget_remaining(&budget, &clock, 14, 9)?;
        Ok::<_, anyhow::Error>(HeldLookupWitness {
            sample,
            lookup_sample: lookup.mark.sample,
            lookup_send: lookup.mark.raw,
            lookup_receive: lookup.receive,
            intent: owned,
            generation: installed.generation_id,
            durable_executed: durable,
            lookup_executed: record.executed_qty,
            lookup_handoffs: total_handoffs - handoffs_before,
            cumulative_handoffs: total_handoffs,
            lookup_sql: counters().0 - lookup_before.0,
        })
    })
    .await
    .context("held inventory window");

    let asked_at = Instant::now();
    let stop_deadline = asked_at + crate::stop::StopTimings::default().deadline;
    let preparation = timeout_at(stop_deadline, ClockOwner::retire_account(&mut slot)).await;
    let lookup_preparation = timeout_at(stop_deadline, jobs::retire(&mut lookup_slot)).await;
    let stopped = if let Some(owner) = owner {
        if !matches!(&primary, Ok(Ok(_))) {
            owner.cancel.cancel();
        }
        let state = Arc::clone(&owner.state);
        let budget = owner.budget.clone();
        let handoffs = budget.handoffs();
        let closing = owner.spawn();
        close_owner(&closing, &state, &budget, handoffs, Some(start), asked_at).await
    } else {
        Ok(())
    };
    let inventory_stopped = if let Some(listener) = inventory_http {
        listener.finish(stop_deadline).await
    } else {
        Ok(())
    };
    let lookup_stopped = if let Some(listener) = lookup_http {
        listener.finish(stop_deadline).await
    } else {
        Ok(())
    };
    let time_stopped = if let Some(listener) = time_http {
        listener.finish(stop_deadline).await
    } else {
        Ok(())
    };
    ensure!(matches!(preparation, Ok(true)), "held inventory preparation not retired");
    ensure!(matches!(lookup_preparation, Ok(true)), "held lookup preparation not retired");
    stopped?;
    inventory_stopped?;
    lookup_stopped?;
    time_stopped?;
    primary?
}

fn verify_triple_budget(budget: &SharedBudget, clock: &NativeClock) -> anyhow::Result<()> {
    verify_budget_remaining(budget, clock, 14, 13)
}

fn verify_budget_remaining(
    budget: &SharedBudget,
    clock: &NativeClock,
    accounts: usize,
    unit_weight: usize,
) -> anyhow::Result<()> {
    let now = clock.capture()?;
    let due = crate::venue_clock_owner::after(now, Duration::from_secs(1))
        .map_err(|e| anyhow!("{e:?}"))?;
    let mut account = Vec::new();
    for _ in 0..accounts {
        account.push(
            budget
                .try_reserve_account(now, due)
                .map_err(|e| anyhow!("ACCOUNT capacity: {e:?}"))?,
        );
    }
    let mut time = Vec::new();
    for _ in 0..unit_weight {
        time.push(budget.try_reserve_time(now, due).map_err(|e| anyhow!("TIME capacity: {e:?}"))?);
    }
    ensure!(
        matches!(budget.try_reserve_time(now, due), Err(BudgetError::Deferred)),
        "exact steady weight after observed requests"
    );
    Ok(())
}

fn commission_admission_precedence(owner: &ClockOwner, clock: &NativeClock) -> anyhow::Result<()> {
    let due = Instant::now() + Duration::from_secs(3);
    let old = owner.budget.start().map_err(|e| anyhow!("{e:?}"))?;
    let fresh = clock.capture()?;
    let signed = owner.commission_signing_time().map_err(|e| anyhow!("{e:?}"))?;
    let mut pending = owner.reserve_commission(due).map_err(|e| anyhow!("{e:?}"))?;
    let mut foreign = owner.commission_signing_time().map_err(|e| anyhow!("{e:?}"))?;
    let generation = owner.state.lock().map_err(|_| anyhow!("clock lock"))?.generation;
    foreign.state = Arc::new(Mutex::new(ClockState {
        generation,
        closing: false,
        published: None,
        start: old,
        last_valid: None,
        faults: 0,
        active_faults: 0,
    }));
    // The real budget gate is closed to steady work by a waiting protection.
    // The generated budget tests independently cover the recorded 429 latch.
    let waiting = owner.budget.protective_waiter().map_err(|e| anyhow!("{e:?}"))?;
    for (proof, claim, deadline, expected) in [
        (&foreign, old, Instant::now(), BudgetError::Invalid),
        (&signed, old, due, BudgetError::Expired),
        (&signed, fresh, due, BudgetError::Deferred),
    ] {
        let result = owner.poll_commission_first(proof, &mut pending, claim, deadline, || {
            panic!("refused Commission must not poll")
        });
        ensure!(matches!(result, Err(error) if error == expected));
    }
    owner.state.lock().map_err(|_| anyhow!("clock lock"))?.generation = generation + 1;
    let results = [Instant::now(), due].map(|deadline| {
        owner.poll_commission_first(&signed, &mut pending, old, deadline, || {
            panic!("stale Commission must not poll")
        })
    });
    owner.state.lock().map_err(|_| anyhow!("clock lock"))?.generation = generation;
    ensure!(
        matches!(results, [Err(BudgetError::Expired), Err(BudgetError::Deferred)]),
        "command deadline precedes generation, which precedes claim age"
    );
    drop(waiting);
    drop(pending);
    ensure!(owner.budget.handoffs().map_err(|e| anyhow!("{e:?}"))? == 4);
    Ok(())
}

async fn supersession_during_account_body(
    owner: &ClockOwner,
    clock: &NativeClock,
    replace_generation: bool,
    fixture_deadline: Instant,
    fixture_slot: &mut Option<c8_account_http::AccountFixture>,
    slot: &mut Option<inventory::AccountPreparation>,
) -> anyhow::Result<()> {
    let due = (Instant::now() + Duration::from_secs(5)).min(fixture_deadline);
    ensure!(fixture_slot.is_none() && slot.is_none());
    let (venue, head, release) =
        c8_account_http::AccountFixture::held_body(fixture_deadline).await?;
    let venue = fixture_slot.insert(venue);
    let work = async {
        let claim = clock.capture()?;
        let mut pending = owner.reserve_account(due).map_err(|e| anyhow!("{e:?}"))?;
        let prepared = owner.prepare_account(slot, due).await.map_err(|e| anyhow!("{e:?}"))?;
        let signed = owner.commission_signing_time().map_err(|e| anyhow!("{e:?}"))?;
        let original = {
            let state = owner.state.lock().map_err(|_| anyhow!("clock lock"))?;
            (state.generation, state.published.as_ref().context("positive sample")?.id)
        };
        let intervene = async {
            head.await.context("ACCOUNT response head")?;
            {
                let mut state = owner.state.lock().map_err(|_| anyhow!("clock lock"))?;
                if replace_generation {
                    state.generation = original.0 + 1;
                } else {
                    state.published.as_mut().context("positive sample")?.id = Uuid::now_v7();
                }
            }
            release.send(()).map_err(|_| anyhow!("ACCOUNT body release"))?;
            Ok::<_, anyhow::Error>(())
        };
        let read = owner.read_account(&signed, &mut pending, claim, prepared, venue.port, due);
        let (result, intervention) = tokio::join!(read, intervene);
        {
            let mut state = owner.state.lock().map_err(|_| anyhow!("clock lock"))?;
            state.generation = original.0;
            state.published.as_mut().context("positive sample retained")?.id = original.1;
        }
        intervention?;
        ensure!(matches!(result, Err(TimeError::Native)), "original receipt superseded at EOF");
        ensure!(venue.connections() == 1, "one real charged ACCOUNT without retry");
        Ok::<_, anyhow::Error>(())
    };
    timeout_at(due, work).await.context("ACCOUNT supersession deadline")?
}

async fn superseded_owned_order(
    owner: &ClockOwner,
    clock: &NativeClock,
    fixture_deadline: Instant,
    fixture_slot: &mut Option<c8_account_http::AccountFixture>,
    slot: &mut Option<inventory::OwnedOrderPreparation>,
) -> anyhow::Result<()> {
    let due = (Instant::now() + Duration::from_secs(5)).min(fixture_deadline);
    let (venue, head, release) =
        c8_account_http::AccountFixture::held_order_not_found(fixture_deadline).await?;
    let venue = fixture_slot.insert(venue);
    let signed = owner.commission_signing_time().map_err(|error| anyhow!("{error:?}"))?;
    let claim = clock.capture()?;
    let original = owner
        .state
        .lock()
        .map_err(|_| anyhow!("clock lock"))?
        .published
        .as_ref()
        .context("positive lookup sample")?
        .id;
    let retire_sample = async {
        timeout_at(due, head).await.context("lookup head deadline")??;
        owner
            .state
            .lock()
            .map_err(|_| anyhow!("clock lock"))?
            .published
            .as_mut()
            .context("sample present")?
            .id = Uuid::now_v7();
        release.send(()).map_err(|_| anyhow!("release lookup body"))?;
        Ok::<_, anyhow::Error>(())
    };
    let (result, changed) =
        tokio::join!(owner.read_owned_order(&signed, claim, slot, venue.port, due), retire_sample,);
    owner
        .state
        .lock()
        .map_err(|_| anyhow!("clock lock"))?
        .published
        .as_mut()
        .context("sample present")?
        .id = original;
    changed?;
    ensure!(
        matches!(result, Err(TimeError::Native)) && venue.connections() == 1,
        "superseded EOF refuses the 400 observation after the charged send"
    );
    Ok(())
}

async fn close_owner(
    closing: &ClockClosing,
    state: &Mutex<ClockState>,
    budget: &SharedBudget,
    expected_handoffs: Result<u64, BudgetError>,
    epoch: Option<&process_epoch::EpochStart>,
    asked_at: Instant,
) -> anyhow::Result<()> {
    let Some(start) = epoch else {
        closing.begin_closing();
        let joined = closing.join(asked_at + crate::stop::StopTimings::default().deadline).await;
        return Err(anyhow!("owner without committed epoch; actual join={joined}"));
    };
    let backend = Arc::new(robson_exec::order_journal::StubJournalBackend::default());
    let journal =
        robson_exec::order_journal::OrderJournal::new(backend, &VenueId::new("test:venue"));
    journal.gate().begin_epoch(
        robson_exec::gate::EpochView {
            epoch_seq: start.epoch.epoch_seq,
            unclean_reason: start.predecessor.unclean_reason(),
        },
        start.unclean_generation,
        start.open_incidents.clone(),
    );
    let marker = UncalledMarker(std::sync::atomic::AtomicUsize::new(0));
    let invalidated = std::sync::atomic::AtomicBool::new(false);
    let report = crate::stop::run_stop_with_clock(
        &journal,
        asked_at,
        || {
            invalidated.store(
                state.lock().is_ok_and(|s| s.closing && s.published.is_none()),
                std::sync::atomic::Ordering::SeqCst,
            );
        },
        Some(&marker),
        Some(closing),
        crate::stop::StopTimings::default(),
    )
    .await;
    // Preserve the real stop callback oracle before any cleanup repair.
    let stop_invalidated = invalidated.load(std::sync::atomic::Ordering::SeqCst);
    closing.begin_closing();
    let final_joined = closing.join(asked_at + crate::stop::StopTimings::default().deadline).await;
    // Even a wrong report/callback must not skip the final owned join attempt.
    ensure!(
        report.result == crate::stop::StopResult::IncidentOpen && final_joined,
        "actual stop: {report:?}; final owner join={final_joined}"
    );
    ensure!(report.marker == crate::stop::MarkerWrite::NotAttempted);
    ensure!(marker.0.load(std::sync::atomic::Ordering::SeqCst) == 0);
    ensure!(stop_invalidated, "the original stop callback observed no invalidation");
    ensure!(
        budget.handoffs().map_err(|e| anyhow!("handoffs: {e:?}"))?
            == expected_handoffs.map_err(|e| anyhow!("initial handoffs: {e:?}"))?,
        "Closing never starts another TIME"
    );
    ensure!(
        closing.worker.try_lock().map_err(|e| anyhow!("worker slot: {e}"))?.is_none(),
        "owner, preparation and parser must actually retire"
    );
    ensure!(
        closing
            .watchdog
            .try_lock()
            .map_err(|e| anyhow!("watchdog slot: {e}"))?
            .is_none(),
        "independent watchdog must actually retire"
    );
    Ok(())
}

fn verify_row(
    row: &PgRow,
    sample: Uuid,
    tenant: Uuid,
    epoch: Uuid,
    before: TickCapture,
    after: TickCapture,
    server_ms: i64,
) -> anyhow::Result<()> {
    ensure!(row.try_get::<Uuid, _>("sample_id")? == sample);
    ensure!(row.try_get::<Uuid, _>("tenant_id")? == tenant);
    ensure!(row.try_get::<Uuid, _>("process_epoch")? == epoch);
    ensure!(row.try_get::<String, _>("venue_id")? == VenueId::BINANCE_SPOT);
    ensure!(
        row.try_get::<i64, _>("server_ms")? == server_ms,
        "retain original actual HTTP reply"
    );
    let domain = before.domain();
    ensure!(after.domain() == domain);
    ensure!(row.try_get::<Uuid, _>("boot")? == domain.boot());
    ensure!(row.try_get::<i64, _>("os")? == domain.offset().0);
    ensure!(u32::try_from(row.try_get::<i32, _>("ons")?)? == domain.offset().1);
    let mark = |wall, nanos, lo, hi| -> anyhow::Result<RawClockCapture> {
        Ok(RawClockCapture {
            epoch,
            domain,
            wall_seconds: row.try_get(wall)?,
            wall_nanoseconds: u32::try_from(row.try_get::<i32, _>(nanos)?)?,
            before_ns: row.try_get::<String, _>(lo)?.parse()?,
            after_ns: row.try_get::<String, _>(hi)?.parse()?,
        })
    };
    let raw = RawClockSample {
        id: sample,
        tenant,
        venue: VenueId::new(VenueId::BINANCE_SPOT),
        send: mark("send_wall_seconds", "send_wall_nanoseconds", "sb", "sa")?,
        receive: mark("receive_wall_seconds", "receive_wall_nanoseconds", "rb", "ra")?,
        server_ms,
    };
    ensure!(i128::try_from(before.nanoseconds())? <= raw.send.before_ns);
    ensure!(raw.send.before_ns <= raw.send.after_ns && raw.send.after_ns <= raw.receive.before_ns);
    ensure!(raw.receive.before_ns <= raw.receive.after_ns);
    ensure!(raw.receive.after_ns <= i128::try_from(after.nanoseconds())?);
    let numbers = raw.numbers()?;
    for (field, expected) in [
        ("theta", numbers.theta2),
        ("low", numbers.r_low),
        ("high", numbers.r_high),
        ("bound", numbers.u2),
    ] {
        ensure!(
            row.try_get::<String, _>(field)? == expected.to_string(),
            "original arithmetic {field}"
        );
    }
    ensure!(row.try_get::<i16, _>("numeric_failures")? == numbers.failures);
    Ok(())
}

struct UncalledMarker(std::sync::atomic::AtomicUsize);
#[async_trait::async_trait]
impl crate::stop::StopMarker for UncalledMarker {
    async fn mark_clean(&self) -> Result<(), String> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Err("actual startup incident remains open".into())
    }
}
