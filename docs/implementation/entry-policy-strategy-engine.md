# Entry Policy Strategy Engine Implementation Guide

## Current Phase

PHASE 7 - VALIDATION is complete and verified.

All validation tests pass. 464 tests, 0 failures.

## Completed Steps

- Created this resumable implementation guide.
- Located the current Rust entry path:
  - API `POST /positions` deserializes `ArmRequest` in `v2/robsond/src/api.rs`.
  - `PositionManager::arm_position` creates `PositionArmed` and spawns one `DetectorTask`.
  - `DetectorTask` in `v2/robsond/src/detector.rs` owns the implicit SMA 9/21 crossover detector.
  - `DetectorTask::create_signal` computes a chart-derived technical stop through `TechnicalStopAnalyzer`.
  - `PositionManager::handle_signal` processes `DetectorSignal`, persists `TechnicalStopAnalyzed`, calls `Engine::decide_entry`, then runs risk, approval, and execution.
  - `Engine::decide_entry` performs signal validation, `TechnicalStopDistance` validation, position sizing, and emits `EntrySignalReceived`, `EntryOrderRequested`, and `PlaceEntryOrder`.
- Located current approval handling:
  - `v2/robsond/src/query_engine.rs` contains a notional-threshold `ApprovalPolicy`.
  - Approval is currently coupled to risk-approved actions, not to user-selected entry policy.
- Located current state and event model:
  - `PositionState` is `Armed`, `Entering`, `Active`, `Exiting`, `Closed`, `Error`.
  - `ExecutionQuery` state already models `Accepted`, `Processing`, `RiskChecked`, `AwaitingApproval`, `Authorized`, `Acting`, terminal states.
  - Entry order request/ack/failure events already exist for exchange-safe replay semantics.
- Defined domain policy identifiers:
  - `EntryPolicy`: `Immediate`, `ConfirmedTrend`, `ConfirmedReversal`, `ConfirmedKeyLevel`.
  - `ApprovalPolicy`: `Automatic`, `HumanConfirmation`.
  - `EntryPolicyConfig` combines the independent entry and approval policies.
  - `StrategyId` provides stable `{ name, version }` strategy identity.
- Defined event-model additions:
  - `EntryPolicyResolved` records entry policy, approval policy, and selected strategy.
  - `SignalStrategyEvaluated` records strategy outcome, reason text, observation time, side, and reference price.
- Defined strategy engine interface in `robson-engine`:
  - `SignalStrategy`
  - `SignalContext`
  - `SignalDecision`
  - `SignalReason`
  - `StrategyRegistry`
  - internal `SignalPrecondition`
- Verified touched crates:
  - `rtk cargo test -p robson-domain -p robson-engine` passed: 182 passed, 2 ignored.
  - Workspace-wide `cargo fmt --check` is blocked by pre-existing formatting drift outside this slice; changed Rust files were formatted directly with `rustfmt`.
- Implemented deterministic strategy engine:
  - `SmaCrossoverStrategy` v1 with reusable SMA crossover detection.
  - `ReversalPatternStrategy` v1 with Hammer, Shooting Star, Bullish Engulfing, and Bearish Engulfing rules.
  - `KeyLevelStrategy` v1 with deterministic local high/low detection, level interaction, and reaction confirmation.
  - `StrategyRegistry` populated with `sma_crossover:v1`, `reversal_patterns:v1`, and `key_level:v1`.
- Integrated detector evaluation with policy resolution:
  - `EntryPolicy::Immediate` confirms without a strategy but still computes the system technical stop before risk/execution.
  - Confirmed policies resolve through `EntryPolicy -> StrategyId -> Strategy -> SignalDecision`.
  - Detector strategy evaluation uses OHLCV candles and no longer keeps an implicit tick-buffer SMA detector.
  - Confirmed strategy decisions emit `SignalStrategyEvaluated` audit events before `DetectorSignal`.
  - `DaemonEvent::DomainEvent` is persisted by the daemon/position-manager event listeners and hidden from SSE clients.
- Updated detector integration tests:
  - Confirmed-trend detector fixtures now create an actual 9/21 SMA crossover.
  - E2E detector test now ignores internal domain audit events and waits for the matching `DetectorSignal`.
- Verified Phase 3:
  - `rtk cargo test -p robson-engine signal_strategy` passed: 8 passed.
  - `rtk cargo test -p robsond detector --lib` passed: 17 passed.
  - `rtk cargo test -p robsond test_e2e_detector_ma_crossover_signal --lib` passed: 1 passed.
- Completed Phase 4 API/runtime wiring:
  - Added `DetectorConfig::from_position_with_policy`.
  - Added `DetectorTask::from_position_with_policy`.
  - Added `PositionManager::entry_policies` runtime map so detector re-arming keeps the original `EntryPolicyConfig`.
  - Added `PositionManager::arm_position_with_policy` as the explicit-policy ARM path.
  - Kept `PositionManager::arm_position` as a default-policy compatibility wrapper.
  - `EntryPolicyResolved` is emitted alongside `PositionArmed` at ARM time.
  - `ArmRequest` in `v2/robsond/src/api.rs` accepts optional `entry_policy.mode` and `entry_policy.approval`.
  - Serde defaults: omitted `entry_policy` or any sub-field resolves to `ConfirmedTrend + Automatic`.
  - `arm_handler` calls `arm_position_with_policy` with the parsed or default policy.
  - Added `QueryEngine::check_approval_with_domain_policy` in `v2/robsond/src/query_engine.rs`:
    - `Automatic` → execution proceeds without human approval regardless of the notional-threshold adapter.
    - `HumanConfirmation` → execution always waits for operator approval regardless of the notional-threshold adapter.
  - `handle_signal` in `v2/robsond/src/position_manager.rs` calls `check_approval_with_domain_policy` instead of `check_approval`; the notional-threshold adapter is no longer authoritative.
  - Added three focused Phase 4 tests:
    - `test_arm_position_with_explicit_policy_stores_policy`
    - `test_handle_signal_automatic_bypasses_notional_threshold`
    - `test_handle_signal_human_confirmation_always_waits`
- Verified Phase 4:
  - `rtk cargo check -p robsond` clean compile.
  - `rtk cargo test -p robson-engine signal_strategy` passed: 8 passed.
  - `rtk cargo test -p robsond detector --lib` passed: 17 passed.
  - `rtk cargo test -p robsond test_e2e_detector_ma_crossover_signal --lib` passed: 1 passed.
  - `rtk cargo test -p robsond test_arm_position_with_explicit_policy_stores_policy` passed.
  - `rtk cargo test -p robsond test_handle_signal_automatic_bypasses_notional_threshold` passed.
  - `rtk cargo test -p robsond test_handle_signal_human_confirmation_always_waits` passed.

## Completed Phase 5 Steps

- Added `PositionState::Cancelled` terminal state for pre-entry disarms (distinguished from `Closed` which implies a filled position).
- Added `EntryLifecycleStage` enum (`IntentCreated`, `AwaitingSignal`, `SignalConfirmed`, `AwaitingApproval`, `OrderSubmitted`, `Active`, `Cancelled`) as a pure computed projection (not stored).
- Added `entry_lifecycle_stage(events: &[Event]) -> EntryLifecycleStage` deterministic projection function in `robson-domain/src/events.rs` with 10 unit tests.
- Added `EntryApprovalPending` domain event for replay-safe AwaitingApproval stage evidence.
- Updated `PositionManager::disarm_position` to set state `Cancelled` (was `Closed`).
- Updated `robson-projector` `handle_position_disarmed` to write `state = 'cancelled'`.
- Created migration `v2/migrations/20240101000009_add_cancelled_position_state.sql` to extend the Postgres CHECK constraint.
- Updated `api.rs` `position_to_summary` to handle `Cancelled`.
- Updated `panic_close_all` exhaustive match to handle `Cancelled`.
- Emitted `EntryApprovalPending` event to event bus (broadcast) AND persisted via `execute_and_persist` (event log).
- Fixed `test_approval_expiry_re_arms_position`: rewritten to poll event bus for `QueryExpired` (background expiry task removes record before `approve_query` can see it).
- Fixed `test_risk_denial_leaves_position_armed_for_retry`: rewritten to trigger `DuplicatePosition` denial instead of slot exhaustion. Slot exhaustion uses `RiskCheck::MonthlyDrawdown` and correctly fires `panic_close_all`; `DuplicatePosition` is a governed retry denial.
- Fixed 4 tests that used `arm_position` with `Automatic` but expected approval (now use `arm_position_with_policy` with `HumanConfirmation`).
- Fixed `test_delete_active_position_closes_it` in `api_contract.rs` to arm with `HumanConfirmation`.
- Added new Phase 5 integration tests: `test_entry_approval_pending_event_emitted`, `test_approval_expiry_re_arms_position`, `test_risk_denial_leaves_position_armed_for_retry`, `test_cancelled_position_excluded_from_find_active`.
- Verified: `cargo test --workspace` → 460 passed, 23 ignored, 0 failed.

## Completed Phase 6 Steps

- 6-A: Updated `v3-migration-plan.md` — MIG-v3#9 and MIG-v3#10 marked as ✅ Implemented with one-line summaries.
- 6-B: Added ADR-v3-027 (EntryApprovalPending dual emission) to `v3-architectural-decisions.md`.
- 6-C: Created `ADR-0028-entry-policy-strategy-engine.md` covering policy-backed strategy mapping, approval independence, Cancelled vs Closed, replay safety, and non-negotiables.
- 6-D: Updated `ArmEntryPolicyRequest` and `ArmRequest` doc comments in `api.rs` with valid mode/approval values and default equivalence.

## Completed Phase 7 Steps

- 7-1: `test_entry_lifecycle_stage_deterministic_replay_all_stages` — all seven stages replayed 100x with and without noise events; non-advancing events do not alter projection.
- 7-2: `strategy_determinism_same_candles_same_decision` — SMA crossover, reversal pattern, and key level strategies each evaluated 100x on identical candles; all produce identical `SignalDecision`.
- 7-3: `test_risk_denial_never_produces_entering_state` — DuplicatePosition risk denial verified: denied position stays Armed, never reaches Entering.
- 7-4: `test_cancelled_position_never_has_entry_fill_in_lifecycle` — PositionDisarmed at any point before fill resolves to Cancelled; EntryFilled prevents Cancelled projection.
- Verified: `cargo test --workspace` → 464 passed, 23 ignored, 0 failed.

## Pending Steps

(No pending steps — all phases complete.)

## Architectural Decisions

- Entry policy must be a selector only. It must not contain strategy logic.
- Technical stop analysis remains separate from opportunity detection. Strategies decide whether there is a signal; `TechnicalStopAnalyzer` remains responsible for where the system-defined stop is.
- Position sizing remains derived only from capital and chart-derived technical stop distance.
- Risk remains mandatory after signal confirmation and before any exchange action.
- Approval policy is independent from entry policy. A strategy-backed policy can still be automatic or require human confirmation.
- The current implicit ARM + SMA detector is the first refactor surface. `ConfirmedTrend` must reuse its SMA crossover behavior, but strategy evaluation must be expressed through the new strategy interface.
- Strategy mapping is centralized in `StrategyRegistry::strategy_id_for_policy`:
  - `ConfirmedTrend -> sma_crossover:v1`
  - `ConfirmedReversal -> reversal_patterns:v1`
  - `ConfirmedKeyLevel -> key_level:v1`
  - `Immediate -> None`
- Strategy events are audit-only for the current projection layer. They are added to the domain event enum now so later replay/state-machine work has explicit evidence.
- Domain `ApprovalPolicy` is authoritative over the notional-threshold adapter in `query_engine.rs`. The adapter remains for backward compatibility and future extensions, but `check_approval_with_domain_policy` always respects the operator-selected approval mode first.

## Assumptions

- The v3 runtime migration target is the Rust code under `v2/`.
- Existing direct signal injection endpoint remains useful for tests and compatibility, but it is not the target production entry path.
- `Immediate` entry policy means no signal strategy is required; it still must pass system technical stop analysis, risk governance, and execution controls.
- Strategy v1 evaluation should use deterministic candle vectors supplied from the configured OHLCV source. Runtime persistence/replay of those candles is a follow-up unless an existing event-log market-data source is found during implementation.
- Reversal and key-level v1 rules are intentionally simple and deterministic; no volume, order book, liquidity sweep, ML, or probabilistic logic is allowed.
- Current daemon integration preserves the existing default entry behavior as `ConfirmedTrend + Automatic` until API callers explicitly select a different policy.

## Refactor Surface

- `v2/robson-domain/src/value_objects.rs`
  - Add policy and strategy identifiers if they belong in pure domain.
- `v2/robson-domain/src/entities.rs`
  - Add signal-related pure domain types if they should be reusable across engine and daemon.
- `v2/robson-domain/src/events.rs`
  - Add policy/strategy audit events for replay evidence.
- `v2/robsond/src/detector.rs`
  - Replace implicit detector behavior with policy-backed strategy evaluation.
- `v2/robsond/src/position_manager.rs`
  - Carry entry policy from arm request into detector config and entry query lifecycle.
- `v2/robsond/src/api.rs`
  - Add API shape for `entry_policy.mode` and `entry_policy.approval`.
- `v2/robson-engine/src/lib.rs`
  - Keep execution decision and sizing behind mandatory risk controls; avoid embedding opportunity detection logic here.

## Next Actions

All planned phases (1-7) are complete. The entry-policy strategy engine is fully implemented, documented, and validated.
