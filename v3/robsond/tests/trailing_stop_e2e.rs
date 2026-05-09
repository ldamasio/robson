//! E2E test: Trailing stop updates are persisted correctly.
//!
//! Flow:
//! 1. Create Active position with initial trailing stop
//! 2. Tick A: Price rises -> stop should update
//! 3. Verify: TrailingStopUpdated emitted, state persisted with new
//!    stop/extreme
//! 4. Tick B: Price falls to stop -> ExitTriggered emitted

use std::sync::Arc;

use chrono::Utc;
use robson_domain::{
    AccountId, Event, ExitReason, Position, PositionState, Price, Quantity, Side, Symbol,
    TechnicalStopDistance,
};
use robson_engine::{Engine, MarketData};
use robson_exec::{Executor, StubExchange};
use robson_store::{MemoryStore, Store};
use rust_decimal_macros::dec;
use uuid::Uuid;

// =============================================================================
// Test: Trailing Stop E2E
// =============================================================================

#[tokio::test]
async fn test_trailing_stop_e2e() {
    // Setup
    let exchange = Arc::new(StubExchange::new(dec!(95000)));
    let journal = Arc::new(robson_exec::IntentJournal::new());
    let store = Arc::new(MemoryStore::new());
    let executor = Executor::new(exchange, journal, store.clone());
    let risk_config = robson_domain::RiskConfig::new(dec!(10000)).unwrap();
    let engine = Engine::new(risk_config);

    let position_id = Uuid::now_v7();
    let account_id: AccountId = Uuid::now_v7();
    let symbol = Symbol::from_pair("BTCUSDT").unwrap();
    let side = Side::Long;

    // Initial state: entry $95k, stop $93.5k (distance $1.5k)
    let entry_price = Price::new(dec!(95000)).unwrap();
    let initial_stop = Price::new(dec!(93500)).unwrap();
    let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry_price, initial_stop);

    let mut position = Position::new(account_id, symbol.clone(), side);
    position.id = position_id;
    position.tech_stop_distance = Some(tech_stop);
    position.entry_price = Some(entry_price);
    position.entry_filled_at = Some(Utc::now());
    position.quantity = Quantity::new(dec!(0_0667)).unwrap();

    // Set Active state with initial trailing stop
    position.state = PositionState::Active {
        current_price: entry_price,
        trailing_stop: initial_stop,
        favorable_extreme: entry_price,
        extreme_at: Utc::now(),
        insurance_stop_id: None,
        last_emitted_stop: Some(initial_stop),
    };

    // Persist initial position
    store.positions().save(&position).await.unwrap();

    // TICK A: Price rises to $96.5k (new high)
    let market_data_high = MarketData::new(symbol.clone(), Price::new(dec!(96500)).unwrap());
    let decision_a = engine.process_active_position(&position, &market_data_high).unwrap();

    // Verify: Decision has updated position and actions
    assert!(decision_a.updated_position.is_some(), "Tick A: Should have updated position");

    let updated_a = decision_a.updated_position.as_ref().unwrap();

    // Verify: New stop = $96.5k - $1.5k = $95k (anchored 1x)
    if let PositionState::Active {
        current_price,
        trailing_stop,
        favorable_extreme,
        ..
    } = updated_a.state
    {
        assert_eq!(current_price.as_decimal(), dec!(96500));
        assert_eq!(trailing_stop.as_decimal(), dec!(95000));
        assert_eq!(favorable_extreme.as_decimal(), dec!(96500));
    } else {
        panic!("Position should still be Active after Tick A");
    }

    // Verify: TrailingStopUpdated event emitted
    let has_update_event = decision_a.actions.iter().any(|action| {
        matches!(
            action,
            robson_engine::EngineAction::EmitEvent(Event::TrailingStopUpdated { .. })
        )
    });
    assert!(has_update_event, "Tick A: Should emit TrailingStopUpdated");

    // Execute actions (persists event)
    let results_a = executor.execute(decision_a.actions).await.unwrap();
    assert!(!results_a.is_empty(), "Tick A: Should have action results");

    // Persist the updated position state
    if let Some(ref updated) = decision_a.updated_position {
        store.positions().save(updated).await.unwrap();
    }

    // Verify: State was persisted correctly
    let loaded_a = store.positions().find_by_id(position_id).await.unwrap().unwrap();

    if let PositionState::Active { trailing_stop, favorable_extreme, .. } = loaded_a.state {
        assert_eq!(trailing_stop.as_decimal(), dec!(95000));
        assert_eq!(favorable_extreme.as_decimal(), dec!(96500));
    } else {
        panic!("Persisted position should be Active after Tick A");
    }

    // TICK B: Price falls to $95k (hits trailing stop at $95k)
    let market_data_drop = MarketData::new(symbol.clone(), Price::new(dec!(95000)).unwrap());
    let decision_b = engine.process_active_position(&loaded_a, &market_data_drop).unwrap();

    // Verify: Exit decision
    let has_exit_event = decision_b.actions.iter().any(|action| {
        matches!(action, robson_engine::EngineAction::EmitEvent(Event::ExitTriggered { .. }))
    });
    assert!(has_exit_event, "Tick B: Should emit ExitTriggered");

    // Verify: Exit reason is TrailingStop
    let exit_reason = decision_b.actions.iter().find_map(|action| {
        if let robson_engine::EngineAction::TriggerExit { reason, .. } = action {
            Some(*reason)
        } else {
            None
        }
    });
    assert_eq!(exit_reason, Some(ExitReason::TrailingStop));
}

// =============================================================================
// Regression: BTCUSDT Long position 019db3dc-c107-7872-bdcb-c3e6602ebbe0
//
// Production state observed 2026-05-05 (left Active despite BTCUSDT futures
// touching 74_868.0 on 2026-04-28):
//   - entry_price       = 77_932.40   (filled 2026-04-22T06:24:42.328894Z)
//   - trailing_stop     = 76_158.25   (initial; never advanced)
//   - span (palmo)      =  1_774.15   (= entry - initial_stop)
//
// Acceptance criteria proved by this test (all required by the production fix
// brief):
//   1. When price walks one full span above entry, the engine advances the
//      trailing stop by exactly one span step.
//   2. When price touches or crosses the trailing stop, the engine emits
//      ExitTriggered + PlaceExitOrder (state must not stay Active).
//   3. The span/entry/extreme/trailing-stop quartet remains internally
//      consistent across both branches.
//
// Hand-derived expected values (anchored to entry=77_932.40, span=1_774.15):
//   - 1×span peak  = 79_706.55  → stop advances to 77_932.40 (breakeven)
//   - 2×span peak  = 81_480.70  → stop advances to 79_706.55
//   - exit price   = 74_868.00  → ≤ initial stop (76_158.25), MUST trigger
// =============================================================================

fn build_btcusdt_position_019db3dc() -> Position {
    let position_id = Uuid::now_v7();
    let account_id: AccountId = Uuid::now_v7();
    let symbol = Symbol::from_pair("BTCUSDT").unwrap();

    let entry_price = Price::new(dec!(77932.40)).unwrap();
    let initial_stop = Price::new(dec!(76158.25)).unwrap();
    let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry_price, initial_stop);

    let mut position = Position::new(account_id, symbol, Side::Long);
    position.id = position_id;
    position.tech_stop_distance = Some(tech_stop);
    position.entry_price = Some(entry_price);
    position.entry_filled_at = Some(Utc::now());
    position.quantity = Quantity::new(dec!(0.010)).unwrap();
    position.state = PositionState::Active {
        current_price: entry_price,
        trailing_stop: initial_stop,
        favorable_extreme: entry_price,
        extreme_at: Utc::now(),
        insurance_stop_id: None,
        last_emitted_stop: Some(initial_stop),
    };
    position
}

#[tokio::test]
async fn regression_btcusdt_019db3dc_stop_advances_on_winning_span() {
    let risk_config = robson_domain::RiskConfig::new(dec!(10000)).unwrap();
    let engine = Engine::new(risk_config);

    let position = build_btcusdt_position_019db3dc();
    let symbol = position.symbol.clone();

    // Span boundary (entry + 1×span = 79_706.55) — must advance stop to 77_932.40.
    let market = MarketData::new(symbol.clone(), Price::new(dec!(79706.55)).unwrap());
    let decision = engine.process_active_position(&position, &market).unwrap();

    let updated = decision
        .updated_position
        .as_ref()
        .expect("engine must emit updated position when span advances");

    if let PositionState::Active { trailing_stop, favorable_extreme, .. } = updated.state {
        assert_eq!(
            trailing_stop.as_decimal(),
            dec!(77932.40),
            "trailing stop must advance exactly one span step (to breakeven) on +1×span peak"
        );
        assert_eq!(
            favorable_extreme.as_decimal(),
            dec!(79706.55),
            "favorable_extreme must record the new peak"
        );
    } else {
        panic!("position must remain Active after span advance");
    }

    let emitted_trailing_update = decision.actions.iter().any(|action| {
        matches!(
            action,
            robson_engine::EngineAction::EmitEvent(Event::TrailingStopUpdated { .. })
        )
    });
    assert!(emitted_trailing_update, "TrailingStopUpdated must be emitted on span advance");
}

#[tokio::test]
async fn regression_btcusdt_019db3dc_stop_fires_when_price_crosses_initial_stop() {
    // Reproduces the production failure: price reaches 74_868.0 (BTCUSDT
    // futures, 2026-04-28) while trailing_stop is still the initial 76_158.25.
    // The engine MUST emit ExitTriggered + PlaceExitOrder; the state MUST NOT
    // remain Active.
    let risk_config = robson_domain::RiskConfig::new(dec!(10000)).unwrap();
    let engine = Engine::new(risk_config);

    let position = build_btcusdt_position_019db3dc();
    let symbol = position.symbol.clone();

    let market = MarketData::new(symbol.clone(), Price::new(dec!(74868.00)).unwrap());
    let decision = engine.process_active_position(&position, &market).unwrap();

    // Acceptance: ExitTriggered emitted with TrailingStop reason.
    let exit_reason = decision.actions.iter().find_map(|action| {
        if let robson_engine::EngineAction::TriggerExit { reason, .. } = action {
            Some(*reason)
        } else {
            None
        }
    });
    assert_eq!(
        exit_reason,
        Some(ExitReason::TrailingStop),
        "engine must emit TriggerExit(TrailingStop) when price <= trailing_stop"
    );

    // Acceptance: PlaceExitOrder action queued (engine asks the executor to
    // place the market exit order).
    let placed_exit = decision
        .actions
        .iter()
        .any(|action| matches!(action, robson_engine::EngineAction::PlaceExitOrder { .. }));
    assert!(placed_exit, "engine must enqueue PlaceExitOrder on stop hit");

    // Acceptance: ExitTriggered domain event emitted (audit trail).
    let emitted_exit = decision.actions.iter().any(|action| {
        matches!(action, robson_engine::EngineAction::EmitEvent(Event::ExitTriggered { .. }))
    });
    assert!(emitted_exit, "ExitTriggered domain event must be emitted on stop hit");
}

#[tokio::test]
async fn regression_btcusdt_019db3dc_runtime_and_projection_converge_after_replay() {
    // Replays the full event stream that SHOULD have been produced for
    // 019db3dc end-to-end: entry → 1×span advance → 2×span advance → stop hit
    // → exit fill → close. Both the runtime engine path and the in-memory
    // projection (apply_event) must converge to the same Closed state.
    let exchange = Arc::new(StubExchange::new(dec!(77932.40)));
    let journal = Arc::new(robson_exec::IntentJournal::new());
    let store = Arc::new(MemoryStore::new());
    let executor = Executor::new(exchange.clone(), journal, store.clone());
    let risk_config = robson_domain::RiskConfig::new(dec!(10000)).unwrap();
    let engine = Engine::new(risk_config);

    let position = build_btcusdt_position_019db3dc();
    let position_id = position.id;
    let symbol = position.symbol.clone();
    store.positions().save(&position).await.unwrap();

    // Tick A: peak at +1×span advances stop to 77_932.40.
    let tick_a = MarketData::new(symbol.clone(), Price::new(dec!(79706.55)).unwrap());
    let position_a = store.positions().find_by_id(position_id).await.unwrap().unwrap();
    let decision_a = engine.process_active_position(&position_a, &tick_a).unwrap();
    executor.execute(decision_a.actions).await.unwrap();
    if let Some(updated) = decision_a.updated_position {
        store.positions().save(&updated).await.unwrap();
    }

    // Tick B: peak at +2×span advances stop to 79_706.55.
    let tick_b = MarketData::new(symbol.clone(), Price::new(dec!(81480.70)).unwrap());
    let position_b = store.positions().find_by_id(position_id).await.unwrap().unwrap();
    let decision_b = engine.process_active_position(&position_b, &tick_b).unwrap();
    executor.execute(decision_b.actions).await.unwrap();
    if let Some(updated) = decision_b.updated_position {
        store.positions().save(&updated).await.unwrap();
    }

    // Verify stop has advanced through 1×span → 2×span.
    let after_advances = store.positions().find_by_id(position_id).await.unwrap().unwrap();
    if let PositionState::Active { trailing_stop, favorable_extreme, .. } = after_advances.state {
        assert_eq!(trailing_stop.as_decimal(), dec!(79706.55));
        assert_eq!(favorable_extreme.as_decimal(), dec!(81480.70));
    } else {
        panic!("position must remain Active after favorable advances");
    }

    // Tick C: exit price 74_868.0 — well below current stop 79_706.55.
    exchange.set_price("BTCUSDT", dec!(74868.00));
    let tick_c = MarketData::new(symbol.clone(), Price::new(dec!(74868.00)).unwrap());
    let decision_c = engine.process_active_position(&after_advances, &tick_c).unwrap();
    let results_c = executor.execute(decision_c.actions).await.unwrap();

    // Executor's PlaceExitOrder both placed the market order and emitted
    // ExitOrderPlaced (transitioning Active → Exiting in the in-memory store).
    let placed_exit = results_c
        .iter()
        .any(|r| matches!(r, robson_exec::ActionResult::OrderPlaced { event: Some(_), .. }));
    assert!(placed_exit, "exit market order must be placed and ExitOrderPlaced emitted");

    let exiting_position = store.positions().find_by_id(position_id).await.unwrap().unwrap();
    assert!(
        matches!(exiting_position.state, PositionState::Exiting { .. }),
        "after PlaceExitOrder the projection must be Exiting, got {:?}",
        exiting_position.state
    );

    // Apply terminal PositionClosed (mirrors PositionManager::handle_exit_fill).
    let closed_event = Event::PositionClosed {
        position_id,
        exit_reason: ExitReason::TrailingStop,
        entry_price: Price::new(dec!(77932.40)).unwrap(),
        exit_price: Price::new(dec!(74868.00)).unwrap(),
        realized_pnl: (dec!(74868.00) - dec!(77932.40)) * dec!(0.010),
        total_fees: dec!(0),
        closure_evidence: robson_domain::ClosureEvidence::real_exit_fill(None),
        timestamp: Utc::now(),
    };
    store.events().append(&closed_event).await.unwrap();
    store.apply_event(&closed_event).unwrap();

    // Final acceptance: state must be Closed; nothing must leave it Active.
    let final_position = store.positions().find_by_id(position_id).await.unwrap().unwrap();
    assert!(
        matches!(final_position.state, PositionState::Closed { .. }),
        "final state must be Closed after the production scenario, got {:?}",
        final_position.state
    );

    // Replay convergence: rebuild a fresh store by re-applying every event in
    // append order; the reconstructed state must match the runtime state.
    let replay_store = Arc::new(MemoryStore::new());
    let mut seed = build_btcusdt_position_019db3dc();
    seed.id = position_id;
    replay_store.positions().save(&seed).await.unwrap();
    let events = store.events().find_by_position(position_id).await.unwrap();
    for event in &events {
        replay_store.apply_event(event).unwrap();
    }
    let replayed = replay_store.positions().find_by_id(position_id).await.unwrap().unwrap();
    assert!(
        matches!(replayed.state, PositionState::Closed { .. }),
        "replay must converge to Closed, got {:?}",
        replayed.state
    );
}
