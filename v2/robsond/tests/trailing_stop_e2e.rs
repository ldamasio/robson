//! E2E test: Trailing stop updates are persisted correctly.
//!
//! Flow:
//! 1. Create Active position with initial trailing stop
//! 2. Tick A: Price rises -> stop should update
//! 3. Verify: TrailingStopUpdated emitted, state persisted with new stop/extreme
//! 4. Tick B: Price falls to stop -> ExitTriggered emitted

use std::sync::Arc;

use chrono::Utc;
use robson_domain::{
    AccountId, ExitReason, Event, Position, PositionState, Price,
    Quantity, Side, Symbol, TechnicalStopDistance,
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
    let risk_config = robson_domain::RiskConfig::new(dec!(10000), dec!(1)).unwrap();
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
    let decision_a = engine
        .process_active_position(&position, &market_data_high)
        .unwrap();

    // Verify: Decision has updated position and actions
    assert!(
        decision_a.updated_position.is_some(),
        "Tick A: Should have updated position"
    );

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
    let loaded_a = store
        .positions()
        .find_by_id(position_id)
        .await
        .unwrap()
        .unwrap();

    if let PositionState::Active {
        trailing_stop,
        favorable_extreme,
        ..
    } = loaded_a.state
    {
        assert_eq!(trailing_stop.as_decimal(), dec!(95000));
        assert_eq!(favorable_extreme.as_decimal(), dec!(96500));
    } else {
        panic!("Persisted position should be Active after Tick A");
    }

    // TICK B: Price falls to $95k (hits trailing stop at $95k)
    let market_data_drop = MarketData::new(symbol.clone(), Price::new(dec!(95000)).unwrap());
    let decision_b = engine
        .process_active_position(&loaded_a, &market_data_drop)
        .unwrap();

    // Verify: Exit decision
    let has_exit_event = decision_b.actions.iter().any(|action| {
        matches!(
            action,
            robson_engine::EngineAction::EmitEvent(Event::ExitTriggered { .. })
        )
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
