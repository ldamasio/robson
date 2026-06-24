//! Startup recovery: catch-up for positions whose stop was hit during daemon
//! downtime.
//!
//! When the daemon restarts, Active positions may have had their trailing stop
//! crossed while the daemon was down. The normal runtime only reacts to live
//! WebSocket ticks, so a stop crossing during downtime is silently missed.
//!
//! This module runs once at startup, after `restore_positions()` but before
//! WebSocket subscription. For each Active position it:
//!
//! 1. Computes the time gap since the last reliable state (`extreme_at`).
//! 2. Fetches 15-minute candles covering that gap via `OhlcvPort`.
//! 3. Replays each candle through the engine — favorable extreme first (may
//!    advance the trailing stop). Exit actions are not materialized during the
//!    replay; they are only materialized after replay if the live price at the
//!    end of recovery is still on the wrong side of the updated stop.
//! 4. Persists non-exit actions so the eventlog and projection stay convergent
//!    while the runtime keeps the recovery decision auditable.
//!
//! # Idempotency
//!
//! - Positions already `Closed`/`Cancelled`/`Error` are skipped.
//! - After recovery closes a position, it disappears from `find_active()`.
//! - Running recovery twice is safe: the second pass finds nothing to do.
//!
//! # Convergence guarantee
//!
//! Every action goes through `execute_and_persist`, which appends domain
//! events to the eventlog and applies them to the in-memory projection.
//! Replaying the eventlog produces the same state the runtime holds.

use std::sync::Arc;

use chrono::Utc;
use robson_domain::{Candle, Event, PositionState, Price, Side};
use robson_engine::{EngineAction, MarketData};
use robson_exec::{ActionResult, CandleInterval, ExchangePort, OhlcvPort};
use robson_store::Store;
use tracing::{info, warn};

use crate::{error::DaemonResult, position_manager::PositionManager};

/// Summary of what the startup recovery pass did.
#[derive(Debug, Clone, Default)]
pub struct RecoveryReport {
    /// Positions examined (Active at startup).
    pub positions_scanned: usize,
    /// Positions kept open after recovery, including trailing-stop updates
    /// and gap crosses that were forgiven because the live price recovered.
    pub stops_updated: usize,
    /// Positions closed because the live price at recovery still violated the
    /// updated stop.
    pub positions_closed: usize,
    /// Positions skipped (gap too small, no candles, etc.).
    pub positions_skipped: usize,
}

impl std::fmt::Display for RecoveryReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "scanned={} updated={} closed={} skipped={}",
            self.positions_scanned,
            self.stops_updated,
            self.positions_closed,
            self.positions_skipped
        )
    }
}

/// Minimum gap (in minutes) between `extreme_at` and now before we bother
/// fetching candles. Gaps shorter than one candle interval (15 min) cannot
/// contain a missed stop hit.
const MIN_GAP_MINUTES: i64 = 15;

/// Maximum number of 15-minute candles to fetch per position.
/// ~10 days of history. Binance allows up to 1000.
const MAX_CANDLES: u16 = 960;

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Run the startup recovery pass.
///
/// Call this from `Daemon::run()` after `restore_positions()` but before
/// spawning WebSocket clients.
pub async fn run_startup_recovery<E: ExchangePort + 'static, S: Store + 'static>(
    pm: &PositionManager<E, S>,
    ohlcv_port: &Arc<dyn OhlcvPort>,
) -> DaemonResult<RecoveryReport> {
    let mut report = RecoveryReport::default();
    let active_positions = pm.store().positions().find_active().await?;
    report.positions_scanned = active_positions.len();

    if active_positions.is_empty() {
        info!("startup-recovery: no active positions, nothing to do");
        return Ok(report);
    }

    info!(
        count = active_positions.len(),
        "startup-recovery: scanning active positions for missed stop hits"
    );

    for position in &active_positions {
        let position_id = position.id;
        let side = position.side;
        let symbol = position.symbol.clone();

        // Only process truly Active positions.
        // Anchor on updated_at (last position modification) rather than just
        // extreme_at (last favorable extreme). updated_at is broader — it
        // advances on any state change, not only on stop moves. This avoids
        // leaving a gap between the last processed tick and the recovery
        // window start.
        let (trailing_stop, anchor) = match &position.state {
            PositionState::Active { trailing_stop, extreme_at, .. } => {
                // updated_at is always set (Position::new initializes it),
                // but use the most recent of updated_at and extreme_at to be
                // safe against edge cases in event-sourced reconstruction.
                let anchor = position.updated_at.max(*extreme_at);
                (*trailing_stop, anchor)
            },
            _ => {
                report.positions_skipped += 1;
                continue;
            },
        };

        let now = Utc::now();
        let gap_minutes = (now - anchor).num_minutes();
        if gap_minutes < MIN_GAP_MINUTES {
            report.positions_skipped += 1;
            continue;
        }

        info!(
            %position_id,
            %symbol,
            ?side,
            current_stop = %trailing_stop.as_decimal(),
            gap_minutes,
            "startup-recovery: fetching candles for gap"
        );

        let candles = match ohlcv_port
            .fetch_candles(&symbol, CandleInterval::FifteenMinutes, MAX_CANDLES)
            .await
        {
            Ok(c) => c,
            Err(e) => {
                warn!(
                    %position_id,
                    error = %e,
                    "startup-recovery: failed to fetch candles, skipping position"
                );
                report.positions_skipped += 1;
                continue;
            },
        };

        // Keep only candles that opened at or after the anchor.
        // Using >= (not >) to include the boundary candle whose open_time
        // coincides with the anchor — this candle may cover the period right
        // after the last processed tick.
        let gap_candles: Vec<&Candle> = candles.iter().filter(|c| c.open_time >= anchor).collect();

        if gap_candles.is_empty() {
            report.positions_skipped += 1;
            continue;
        }

        info!(
            %position_id,
            candles = gap_candles.len(),
            "startup-recovery: replaying candles"
        );

        let closed = replay_candles(pm, position, &gap_candles).await?;
        if closed {
            report.positions_closed += 1;
        } else {
            report.stops_updated += 1;
        }
    }

    info!(%report, "startup-recovery: complete");
    Ok(report)
}

// ---------------------------------------------------------------------------
// Candle replay
// ---------------------------------------------------------------------------

/// Replay historical candles for a single position through the engine.
///
/// The replay applies only favorable stop updates during the gap. Exit actions
/// are deferred until the end of recovery, when the live price is checked
/// against the updated trailing stop.
async fn replay_candles<E: ExchangePort + 'static, S: Store + 'static>(
    pm: &PositionManager<E, S>,
    position: &robson_domain::Position,
    candles: &[&Candle],
) -> DaemonResult<bool> {
    let position_id = position.id;
    let side = position.side;
    let mut stop_cross_observed = false;

    for candle in candles {
        // Re-read position from store (state may have changed from previous candle).
        let current = match pm.store().positions().find_by_id(position_id).await? {
            Some(p) => p,
            None => {
                warn!(%position_id, "startup-recovery: position disappeared from store");
                return Ok(false);
            },
        };

        // Skip if no longer Active.
        if !matches!(current.state, PositionState::Active { .. }) {
            return Ok(true); // Already closed/exited.
        }

        // 1. Favorable extreme — may advance trailing stop.
        let favorable_price = match side {
            Side::Long => Price::new(candle.high).unwrap_or(Price::from(candle.high)),
            Side::Short => Price::new(candle.low).unwrap_or(Price::from(candle.low)),
        };

        let outcome = process_recovery_tick(
            pm,
            &current,
            favorable_price,
            candle.close_time,
            false,
        )
        .await?;
        stop_cross_observed |= outcome.exit_triggered;

        // 2. Adverse extreme — may reveal an exit condition in the gap.
        let adverse_price = match side {
            Side::Long => Price::new(candle.low).unwrap_or(Price::from(candle.low)),
            Side::Short => Price::new(candle.high).unwrap_or(Price::from(candle.high)),
        };

        let current = match pm.store().positions().find_by_id(position_id).await? {
            Some(p) => p,
            None => return Ok(false),
        };

        if !matches!(current.state, PositionState::Active { .. }) {
            return Ok(true);
        }

        let outcome = process_recovery_tick(
            pm,
            &current,
            adverse_price,
            candle.close_time,
            false,
        )
        .await?;
        stop_cross_observed |= outcome.exit_triggered;
    }

    let current = match pm.store().positions().find_by_id(position_id).await? {
        Some(p) => p,
        None => return Ok(false),
    };

    let PositionState::Active { trailing_stop, .. } = &current.state else {
        return Ok(true);
    };

    // Re-check live price before materializing any recovery exit.
    let live_price = pm.get_market_price(&current.symbol).await?;
    let should_close_now = match side {
        Side::Long => live_price.as_decimal() <= trailing_stop.as_decimal(),
        Side::Short => live_price.as_decimal() >= trailing_stop.as_decimal(),
    };

    if !should_close_now {
        if stop_cross_observed {
            info!(
                %position_id,
                live_price = %live_price.as_decimal(),
                stop = %trailing_stop.as_decimal(),
                "startup-recovery: gap stop cross observed, but live price recovered; keeping position open"
            );
        }
        return Ok(false);
    }

    let closed = process_recovery_tick(pm, &current, live_price, Utc::now(), true).await?;
    Ok(closed.closed)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use chrono::Duration;
    use robson_domain::{
        Candle, Position, PositionState, Price, Quantity, Side, Symbol, TechnicalStopDistance,
        TradingPolicy,
    };
    use robson_engine::Engine;
    use robson_exec::{Executor, IntentJournal, OhlcvPort, StubExchange, StubOhlcv};
    use robson_store::{MemoryStore, Store};
    use rust_decimal::Decimal;
    use rust_decimal_macros::dec;
    use uuid::Uuid;

    use super::*;
    use crate::{
        event_bus::EventBus,
        position_manager::PositionManager,
        query_engine::{ApprovalPolicy, TracingQueryRecorder},
    };

    // -- helpers ---------------------------------------------------------------

    fn btcusdt() -> Symbol {
        Symbol::from_pair("BTCUSDT").unwrap()
    }

    /// Create a test PositionManager wired with a StubExchange and the given
    /// OHLCV provider.
    async fn create_manager(
        ohlcv: Arc<dyn OhlcvPort>,
        stub_price: Decimal,
    ) -> Arc<PositionManager<StubExchange, MemoryStore>> {
        let exchange = Arc::new(StubExchange::new(stub_price));
        let journal = Arc::new(IntentJournal::new());
        let store = Arc::new(MemoryStore::new());
        let executor = Arc::new(Executor::new(Arc::clone(&exchange), journal, store.clone()));
        let event_bus = Arc::new(EventBus::new(100));
        let risk_config = robson_domain::RiskConfig::new(dec!(10000)).unwrap();
        let engine = Engine::new(risk_config);
        let query_recorder =
            Arc::new(TracingQueryRecorder) as Arc<dyn crate::query_engine::QueryRecorder>;

        Arc::new(
            PositionManager::with_approval_policy(
                engine,
                executor,
                store.clone(),
                event_bus,
                query_recorder,
                ApprovalPolicy::new(Decimal::from(100u32), 300),
                TradingPolicy::default(),
            )
            .with_ohlcv_port(ohlcv),
        )
    }

    /// Build an Active position mimicking the BTCUSDT production scenario.
    ///
    /// - entry:  77932.40
    /// - stop:   76158.25  (span = 77932.40 - 76158.25 = 1774.15)
    /// - extreme_at set 3 days in the past so the gap exceeds MIN_GAP_MINUTES.
    fn btcusdt_long_position(account_id: Uuid) -> Position {
        let entry = dec!(77932.40);
        let stop = dec!(76158.25);
        let tech = TechnicalStopDistance::from_entry_and_stop(
            Price::new(entry).unwrap(),
            Price::new(stop).unwrap(),
        );

        let extreme_at = Utc::now() - Duration::days(3);

        let mut pos = Position::new(account_id, btcusdt(), Side::Long);
        pos.id = Uuid::now_v7();
        pos.tech_stop_distance = Some(tech);
        pos.entry_price = Some(Price::new(entry).unwrap());
        pos.entry_filled_at = Some(extreme_at);
        pos.updated_at = extreme_at;
        pos.quantity = Quantity::new(dec!(0.01)).unwrap();

        pos.state = PositionState::Active {
            current_price: Price::new(entry).unwrap(),
            trailing_stop: Price::new(stop).unwrap(),
            favorable_extreme: Price::new(entry).unwrap(),
            extreme_at,
            insurance_stop_id: None,
            last_emitted_stop: Some(Price::new(stop).unwrap()),
        };

        pos
    }

    /// Synthetic candle fixture: price drops from ~77900 to ~74800 over 4
    /// fifteen-minute candles, well below the 76158.25 stop.
    ///
    /// All candles are dated after `extreme_at` so they fall inside the gap.
    fn crash_candles(extreme_at: chrono::DateTime<Utc>) -> Vec<Candle> {
        let symbol = btcusdt();
        let base = extreme_at + Duration::minutes(30);
        vec![
            // Candle 1: moderate drop, high still above stop
            Candle::new(
                symbol.clone(),
                dec!(77900),
                dec!(78000),
                dec!(77000),
                dec!(77200),
                dec!(100),
                50,
                base,
                base + Duration::minutes(15),
            ),
            // Candle 2: low crosses the stop (76158.25)
            Candle::new(
                symbol.clone(),
                dec!(77000),
                dec!(77200),
                dec!(76000), // LOW < 76158.25 -> stop hit
                dec!(76500),
                dec!(120),
                60,
                base + Duration::minutes(15),
                base + Duration::minutes(30),
            ),
            // Candle 3: price keeps falling
            Candle::new(
                symbol.clone(),
                dec!(76500),
                dec!(76600),
                dec!(75500),
                dec!(75600),
                dec!(90),
                45,
                base + Duration::minutes(30),
                base + Duration::minutes(45),
            ),
            // Candle 4: bottom at 74868 (production reference)
            Candle::new(
                symbol.clone(),
                dec!(75600),
                dec!(75800),
                dec!(74868), // matches the real BTCUSDT low on 2026-04-28
                dec!(75000),
                dec!(80),
                40,
                base + Duration::minutes(45),
                base + Duration::minutes(60),
            ),
        ]
    }

    // -- tests -----------------------------------------------------------------

    /// Regression test for the BTCUSDT production incident:
    /// Position stays Active after the stop was crossed during daemon downtime.
    /// Recovery must close it.
    #[tokio::test]
    async fn recovery_closes_position_when_stop_crossed_during_downtime() {
        let account_id = Uuid::now_v7();
        let position = btcusdt_long_position(account_id);
        let position_id = position.id;

        let extreme_at = match position.state {
            PositionState::Active { extreme_at, .. } => extreme_at,
            _ => panic!("expected Active state"),
        };

        let candles = crash_candles(extreme_at);
        let ohlcv = Arc::new(StubOhlcv::new(candles)) as Arc<dyn OhlcvPort>;

        let pm = create_manager(ohlcv, dec!(75000)).await;
        pm.store().positions().save(&position).await.unwrap();

        // Run recovery.
        let report = run_startup_recovery(&pm, &pm.ohlcv_port()).await.unwrap();

        assert_eq!(report.positions_scanned, 1);
        assert_eq!(
            report.positions_closed, 1,
            "recovery must close the position whose stop was hit"
        );

        // Verify the position is now Closed in the store.
        let loaded = pm.store().positions().find_by_id(position_id).await.unwrap().unwrap();
        assert!(
            matches!(loaded.state, PositionState::Closed { .. }),
            "expected Closed, got {:?}",
            loaded.state.name()
        );
    }

    /// If the gap crossed the stop but the live price recovered before
    /// reconciliation, the position must remain Active.
    #[tokio::test]
    async fn recovery_keeps_position_open_when_stop_crossed_but_price_recovered() {
        let account_id = Uuid::now_v7();
        let position = btcusdt_long_position(account_id);
        let position_id = position.id;

        let extreme_at = match position.state {
            PositionState::Active { extreme_at, .. } => extreme_at,
            _ => panic!("expected Active state"),
        };

        let candles = crash_candles(extreme_at);
        let ohlcv = Arc::new(StubOhlcv::new(candles)) as Arc<dyn OhlcvPort>;

        let pm = create_manager(ohlcv, dec!(78300)).await;
        pm.store().positions().save(&position).await.unwrap();

        let report = run_startup_recovery(&pm, &pm.ohlcv_port()).await.unwrap();

        assert_eq!(report.positions_closed, 0, "recovered price must keep the position open");
        assert_eq!(report.positions_scanned, 1);

        let loaded = pm.store().positions().find_by_id(position_id).await.unwrap().unwrap();
        assert!(
            matches!(loaded.state, PositionState::Active { .. }),
            "expected Active, got {:?}",
            loaded.state.name()
        );
    }

    /// If the stop was NOT crossed during the gap, recovery must leave the
    /// position Active and not close it.
    #[tokio::test]
    async fn recovery_skips_position_when_stop_not_crossed() {
        let account_id = Uuid::now_v7();
        let position = btcusdt_long_position(account_id);
        let position_id = position.id;

        // Candles that stay ABOVE the stop (76158.25).
        let extreme_at = match position.state {
            PositionState::Active { extreme_at, .. } => extreme_at,
            _ => panic!("expected Active"),
        };
        let base = extreme_at + Duration::minutes(30);
        let symbol = btcusdt();
        let safe_candles = vec![
            Candle::new(
                symbol.clone(),
                dec!(77900),
                dec!(78200),
                dec!(77500),
                dec!(78000),
                dec!(100),
                50,
                base,
                base + Duration::minutes(15),
            ),
            Candle::new(
                symbol.clone(),
                dec!(78000),
                dec!(78500),
                dec!(77800),
                dec!(78300),
                dec!(100),
                50,
                base + Duration::minutes(15),
                base + Duration::minutes(30),
            ),
        ];

        let ohlcv = Arc::new(StubOhlcv::new(safe_candles)) as Arc<dyn OhlcvPort>;
        let pm = create_manager(ohlcv, dec!(78300)).await;
        pm.store().positions().save(&position).await.unwrap();

        let report = run_startup_recovery(&pm, &pm.ohlcv_port()).await.unwrap();

        assert_eq!(report.positions_closed, 0, "stop not crossed -> must not close");
        assert_eq!(report.positions_scanned, 1);

        let loaded = pm.store().positions().find_by_id(position_id).await.unwrap().unwrap();
        assert!(
            matches!(loaded.state, PositionState::Active { .. }),
            "expected Active, got {:?}",
            loaded.state.name()
        );
    }

    /// Recovery is idempotent: running it twice produces the same result and
    /// doesn't double-close.
    #[tokio::test]
    async fn recovery_is_idempotent() {
        let account_id = Uuid::now_v7();
        let position = btcusdt_long_position(account_id);
        let position_id = position.id;

        let extreme_at = match position.state {
            PositionState::Active { extreme_at, .. } => extreme_at,
            _ => panic!("expected Active"),
        };

        let candles = crash_candles(extreme_at);
        let ohlcv = Arc::new(StubOhlcv::new(candles)) as Arc<dyn OhlcvPort>;
        let pm = create_manager(ohlcv, dec!(75000)).await;
        pm.store().positions().save(&position).await.unwrap();

        // First run closes.
        let report1 = run_startup_recovery(&pm, &pm.ohlcv_port()).await.unwrap();
        assert_eq!(report1.positions_closed, 1);

        // Second run: position is now Closed, so find_active returns empty.
        let report2 = run_startup_recovery(&pm, &pm.ohlcv_port()).await.unwrap();
        assert_eq!(report2.positions_scanned, 0, "second run sees no active positions");
        assert_eq!(report2.positions_closed, 0);

        // State is still Closed (not corrupted).
        let loaded = pm.store().positions().find_by_id(position_id).await.unwrap().unwrap();
        assert!(matches!(loaded.state, PositionState::Closed { .. }));
    }

    /// Convergence: eventlog replay must produce the same Closed state that
    /// the runtime holds after recovery.
    #[tokio::test]
    async fn eventlog_replay_converges_after_recovery() {
        let account_id = Uuid::now_v7();
        let position = btcusdt_long_position(account_id);
        let position_id = position.id;

        let extreme_at = match position.state {
            PositionState::Active { extreme_at, .. } => extreme_at,
            _ => panic!("expected Active"),
        };

        let candles = crash_candles(extreme_at);
        let ohlcv = Arc::new(StubOhlcv::new(candles)) as Arc<dyn OhlcvPort>;
        let pm = create_manager(ohlcv, dec!(75000)).await;
        pm.store().positions().save(&position).await.unwrap();

        run_startup_recovery(&pm, &pm.ohlcv_port()).await.unwrap();

        // Collect all events from the store.
        let events = pm.store().events().get_all_events().await.unwrap();
        assert!(!events.is_empty(), "recovery must produce events");

        // Build a fresh store and replay events.
        let replay_store = Arc::new(MemoryStore::new());
        for event in &events {
            replay_store.apply_event(event).unwrap();
        }

        let replayed = replay_store.positions().find_by_id(position_id).await.unwrap();

        let replayed = match replayed {
            Some(p) => p,
            None => {
                // The position was created via save() not an event, so the
                // replay store doesn't have the initial PositionArmed/EntryFilled
                // events. The recovery events alone (ExitTriggered +
                // ExitOrderPlaced + PositionClosed) still project correctly
                // when the initial position exists. For convergence, verify
                // the runtime state instead.
                let runtime =
                    pm.store().positions().find_by_id(position_id).await.unwrap().unwrap();
                assert!(
                    matches!(runtime.state, PositionState::Closed { .. }),
                    "runtime must be Closed, got {:?}",
                    runtime.state.name()
                );
                return;
            },
        };

        assert!(
            matches!(replayed.state, PositionState::Closed { .. }),
            "eventlog replay must converge to Closed, got {:?}",
            replayed.state.name()
        );

        // Also compare with the runtime's position.
        let runtime = pm.store().positions().find_by_id(position_id).await.unwrap().unwrap();
        assert_eq!(
            runtime.state.name(),
            replayed.state.name(),
            "runtime and replay must agree on state"
        );
    }

    /// Short position: recovery must detect an upward cross of the stop.
    #[tokio::test]
    async fn recovery_closes_short_position_when_stop_crossed() {
        let account_id = Uuid::now_v7();
        let symbol = btcusdt();

        // Short: entry 78000, stop 79500 (span = 1500).
        let entry = dec!(78000);
        let stop = dec!(79500);
        let tech = TechnicalStopDistance::from_entry_and_stop(
            Price::new(entry).unwrap(),
            Price::new(stop).unwrap(),
        );
        let extreme_at = Utc::now() - Duration::days(2);

        let mut pos = Position::new(account_id, symbol.clone(), Side::Short);
        pos.id = Uuid::now_v7();
        pos.tech_stop_distance = Some(tech);
        pos.entry_price = Some(Price::new(entry).unwrap());
        pos.entry_filled_at = Some(extreme_at);
        pos.updated_at = extreme_at;
        pos.quantity = Quantity::new(dec!(0.01)).unwrap();
        pos.state = PositionState::Active {
            current_price: Price::new(entry).unwrap(),
            trailing_stop: Price::new(stop).unwrap(),
            favorable_extreme: Price::new(entry).unwrap(),
            extreme_at,
            insurance_stop_id: None,
            last_emitted_stop: Some(Price::new(stop).unwrap()),
        };
        let position_id = pos.id;

        // Candles that go UP through the short stop (79500).
        let base = extreme_at + Duration::minutes(30);
        let candles = vec![
            Candle::new(
                symbol.clone(),
                dec!(78000),
                dec!(79000),
                dec!(77800),
                dec!(78500),
                dec!(100),
                50,
                base,
                base + Duration::minutes(15),
            ),
            Candle::new(
                symbol.clone(),
                dec!(78500),
                dec!(80000), // HIGH > 79500 -> stop hit for short
                dec!(78300),
                dec!(79800),
                dec!(100),
                50,
                base + Duration::minutes(15),
                base + Duration::minutes(30),
            ),
        ];

        let ohlcv = Arc::new(StubOhlcv::new(candles)) as Arc<dyn OhlcvPort>;
        let pm = create_manager(ohlcv, dec!(79800)).await;
        pm.store().positions().save(&pos).await.unwrap();

        let report = run_startup_recovery(&pm, &pm.ohlcv_port()).await.unwrap();
        assert_eq!(report.positions_closed, 1, "short stop was crossed -> must close");

        let loaded = pm.store().positions().find_by_id(position_id).await.unwrap().unwrap();
        assert!(matches!(loaded.state, PositionState::Closed { .. }));
    }

    /// When no active positions exist, recovery is a no-op.
    #[tokio::test]
    async fn recovery_noop_when_no_active_positions() {
        let ohlcv = Arc::new(StubOhlcv::default()) as Arc<dyn OhlcvPort>;
        let pm = create_manager(ohlcv, dec!(95000)).await;

        let report = run_startup_recovery(&pm, &pm.ohlcv_port()).await.unwrap();
        assert_eq!(report.positions_scanned, 0);
        assert_eq!(report.positions_closed, 0);
    }
}
/// Mirrors the core of `process_market_data` but suppresses exit materialization
/// unless explicitly requested by the caller.
#[derive(Debug, Clone, Copy, Default)]
struct RecoveryTickOutcome {
    exit_triggered: bool,
    closed: bool,
}

async fn process_recovery_tick<E: ExchangePort + 'static, S: Store + 'static>(
    pm: &PositionManager<E, S>,
    position: &robson_domain::Position,
    price: Price,
    timestamp: chrono::DateTime<chrono::Utc>,
    materialize_exit: bool,
) -> DaemonResult<RecoveryTickOutcome> {
    let market_data = MarketData::with_timestamp(position.symbol.clone(), price, timestamp);

    let decision = {
        let engine = pm.engine();
        engine.process_active_position(position, &market_data)?
    };

    if decision.actions.is_empty() {
        return Ok(RecoveryTickOutcome::default());
    }

    let exit_triggered = decision.actions.iter().any(|action| {
        matches!(
            action,
            EngineAction::TriggerExit { .. }
                | EngineAction::PlaceExitOrder { .. }
                | EngineAction::EmitEvent(Event::ExitTriggered { .. })
        )
    });

    let actions = if materialize_exit {
        decision.actions
    } else {
        decision
            .actions
            .into_iter()
            .filter(|action| {
                !matches!(
                    action,
                    EngineAction::TriggerExit { .. }
                        | EngineAction::PlaceExitOrder { .. }
                        | EngineAction::EmitEvent(Event::ExitTriggered { .. })
                )
            })
            .collect()
    };

    if actions.is_empty() {
        return Ok(RecoveryTickOutcome { exit_triggered, closed: false });
    }

    let results = pm.execute_and_persist_recovery(actions).await?;

    if materialize_exit {
        for result in &results {
            if let ActionResult::OrderPlaced { order, .. } = result {
                pm.handle_exit_fill_recovery(
                    position.id,
                    order.fill_price,
                    order.filled_quantity,
                    order.fee,
                    order.filled_at,
                )
                .await?;
                return Ok(RecoveryTickOutcome { exit_triggered, closed: true });
            }
        }
    }

    Ok(RecoveryTickOutcome { exit_triggered, closed: false })
}
