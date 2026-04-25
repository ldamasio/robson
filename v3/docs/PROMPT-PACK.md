# Robson v2 Prompt Pack (Agentic Coding)

**Version**: 2.0.0-alpha
**Last Updated**: 2026-01-12
**Purpose**: Copy-paste prompts for step-by-step implementation

---

## How to Use This Prompt Pack

1. **Work Sequentially**: Execute prompts in order (Phase 0 → Phase 10)
2. **One Prompt at a Time**: Complete each prompt before moving to next
3. **Validate After Each Step**: Run tests/commands shown in acceptance criteria
4. **Commit After Each Phase**: Use conventional commits

---

## Phase 0: Project Bootstrap

### Prompt 0.1: Create Rust Workspace

```
Create the Rust workspace for Robson v2 in /home/psyctl/apps/robson/v2/

Structure:
- Root Cargo.toml (workspace)
- 7 crates: robson-domain, robson-engine, robson-exec, robson-connectors, robson-store, robsond, robson-sim

Requirements:
- Use workspace dependencies from EXECUTION-PLAN.md
- Each crate should have lib.rs (or main.rs for robsond)
- Add common dependencies: tokio, serde, rust_decimal, uuid, chrono, anyhow, thiserror

Validation:
- cargo build --all
- cargo test --all (passes even with empty tests)
```

**Checklist**:
- [ ] Create v2/ directory
- [ ] Create workspace Cargo.toml
- [ ] Create all 7 crates with Cargo.toml
- [ ] Add workspace dependencies
- [ ] Run `cargo build` successfully

---

## Prompt 0.2: Create Bun CLI Skeleton

```
Create the Bun CLI project skeleton at v2/cli/ with:

1. Initialize Bun project:
   - package.json with dependencies (commander, axios, cli-table3, chalk)
   - tsconfig.json with strict mode
   - src/index.ts as entry point

2. Create directory structure:
   - src/commands/ (command handlers)
   - src/api/ (API client)
   - src/types/ (TypeScript types)
   - src/utils/ (formatters, config)

3. Implement basic CLI structure:
   - Commander.js setup
   - Help text
   - Version command

Validation:
```bash
cd v2/cli
bun install
bun run src/index.ts --help
bun run src/index.ts --version
```

Expected output:
```
Usage: robson [options] [command]

Options:
  -V, --version   output the version number
  -h, --help      display help for command

Commands:
  arm <symbol>    Arm position for symbol
  disarm <id>     Disarm position
  status          Show position status
  panic           Emergency close all positions
  help [command]  Display help for command
```

Checklist:
- [ ] CLI parses commands
- [ ] Help text displays
- [ ] Version flag works

---

## Phase 1: Domain Types (Pure Logic)

### Prompt 1.1: Implement Value Objects

```
Implement value objects in robson-domain/src/value_objects.rs:

1. Create value_objects.rs with:
   - Price (Decimal, must be > 0)
   - Quantity (Decimal, must be > 0)
   - Symbol (base + quote, from_pair parser)
   - Side (Long/Short enum)
   - Leverage (u8, 1-10 range)
   - PalmaDaMao (distance, distance_pct, entry_price, stop_loss)

2. Each value object must:
   - Have private inner field
   - Implement new() constructor with validation
   - Return Result<Self, DomainError>
   - Implement Display, Debug, Clone, PartialEq, Serialize, Deserialize

3. Add PalmaDaMao::from_entry_and_stop() method
4. Add PalmaDaMao::validate() method (checks 0.1% < palma < 10%)

5. Add tests for:
   - Valid values accepted
   - Invalid values rejected (negative, zero, out of range)
   - Palma calculation correct
   - Palma validation correct

Validation:
```bash
cd v2
cargo test -p robson-domain
cargo clippy -p robson-domain -- -D warnings
```

Checklist:
- [ ] All value objects implemented
- [ ] Validation logic works
- [ ] Tests pass (minimum 10 tests)
- [ ] No clippy warnings
```

---

### Prompt 1.2: Implement Position Entity

```
Implement Position entity in robson-domain/src/entities/position.rs:

1. Create entities/position.rs with Position struct:
   - Fields: id, account_id, symbol, side, state, entry_price, stop_loss, stop_gain, quantity, leverage, palma, realized_pnl, fees_paid, created_at, updated_at, closed_at

2. Implement Position::new() constructor:
   - Initialize with Armed state
   - Generate new PositionId (UUID v7)
   - Set timestamps

3. Implement helper methods:
   - can_enter() -> bool
   - can_exit() -> bool
   - is_closed() -> bool

4. Add tests:
   - Test position creation
   - Test state predicates

Validation:
```bash
cargo test -p robson-domain entities::position
```

Checklist:
- [ ] Position struct compiles
- [ ] Constructor works
- [ ] Helper methods implemented
- [ ] Tests pass
```

---

### Prompt 1.3: Implement State Machine

```
Implement state machine in robson-domain/src/state_machine.rs:

1. Create PositionState enum with variants:
   - Armed { detector_config: DetectorConfig }
   - Entering { entry_order_id: OrderId, expected_entry: Price }
   - Active { monitor_active: bool, last_price: Price, insurance_stop_id: Option<OrderId> }
   - Exiting { exit_order_id: OrderId, exit_reason: ExitReason }
   - Closed { exit_price: Price, realized_pnl: Decimal, exit_reason: ExitReason }
   - Error { error: DomainError, recoverable: bool }

2. Create ExitReason enum: StopLoss, StopGain, UserPanic, DegradedMode, InsuranceStop

3. Implement Position state transition methods:
   - apply_detector_signal(signal) -> Result<EngineAction>
   - apply_entry_filled(fill) -> Result<EngineAction>
   - apply_stop_loss_trigger(price) -> Result<EngineAction>
   - apply_stop_gain_trigger(price) -> Result<EngineAction>
   - apply_exit_filled(fill) -> Result<()>

4. Add tests for:
   - Valid transitions (Armed → Entering → Active → Exiting → Closed)
   - Invalid transitions (Armed → Active should fail)
   - All transition paths

Validation:
```bash
cargo test -p robson-domain state_machine
```

Checklist:
- [ ] State machine compiles
- [ ] All transitions implemented
- [ ] Invalid transitions return Err
- [ ] Tests pass (minimum 8 tests)
```

---

### Prompt 1.4: Implement Position Sizing

```
Add position sizing logic to Position in robson-domain/src/entities/position.rs:

1. Add calculate_position_size() method:
   - Input: entry_price, stop_loss
   - Calculate palma distance
   - Use formula: (capital × risk_pct) / palma_distance
   - Apply leverage multiplier
   - Return Quantity

2. Add validate_notional() method:
   - Check min notional ($10)
   - Check max notional (capital × leverage)

3. Add round_to_precision() helper:
   - Round to exchange precision (8 decimals for BTC)

4. Add tests:
   - Test sizing calculation (capital $10k, risk 1%, palma $1500 → 0.0666... BTC)
   - Test notional validation
   - Test leverage multiplication

Validation:
```bash
cargo test -p robson-domain position::calculate_position_size
```

Checklist:
- [ ] Formula matches DOMAIN.md
- [ ] Validation works
- [ ] Tests pass with known values
```

---

### Prompt 1.5: Implement Events

```
Implement events in robson-domain/src/events.rs:

1. Create Event enum with variants:
   - PositionArmed
   - EntrySignalReceived
   - EntryOrderPlaced
   - EntryOrderFilled
   - PositionActivated
   - StopLossTriggered
   - StopGainTriggered
   - ExitOrderPlaced
   - ExitOrderFilled
   - PositionClosed
   - PositionError
   - ReconciliationStarted
   - DiscrepancyDetected
   - DegradedModeEntered

2. Each variant should contain relevant data (position_id, prices, etc.)

3. Derive: Debug, Clone, PartialEq, Serialize, Deserialize

4. Add tests:
   - Test serialization/deserialization
   - Test round-trip (Event → JSON → Event)

Validation:
```bash
cargo test -p robson-domain events
```

Checklist:
- [ ] All event variants defined
- [ ] Serde works
- [ ] Tests pass
```

---

## Phase 2: Engine (Pure Decision Logic)

### Prompt 2.1: Create Engine Structure

```
Create engine structure in robson-engine/src/lib.rs:

1. Add robson-domain as dependency in Cargo.toml

2. Create Engine struct:
   ```rust
   pub struct Engine {
       risk_config: RiskConfig,
   }
   ```

3. Create EngineInput struct:
   - position: Position
   - market_data: MarketData
   - intents: Vec<Intent>

4. Create EngineDecision struct:
   - actions: Vec<EngineAction>
   - new_state: Position

5. Create EngineAction enum:
   - PlaceOrder(OrderIntent)
   - CancelOrder(OrderId)
   - UpdatePosition(Position)
   - EmitEvent(Event)

6. Implement Engine::new(config) constructor

7. Add basic decide() method skeleton (returns empty decision)

Validation:
```bash
cargo build -p robson-engine
cargo test -p robson-engine
```

Checklist:
- [ ] Compiles without errors
- [ ] No I/O dependencies (only robson-domain)
- [ ] Basic structure complete
```

---

### Prompt 2.2: Implement Entry Decision

```
Implement entry decision logic in robson-engine/src/entry.rs:

1. Create decide_entry() method:
   ```rust
   pub fn decide_entry(
       &self,
       position: &Position,
       signal: DetectorSignal,
   ) -> Result<EngineDecision, EngineError>
   ```

2. Logic:
   - Validate position is in Armed state
   - Calculate palma from signal
   - Validate palma (0.1% < palma < 10%)
   - Calculate position size
   - Create OrderIntent for entry
   - Return decision with PlaceOrder action

3. Add tests:
   - Test valid signal creates order
   - Test invalid palma rejected
   - Test correct position size calculation
   - Test state transition Armed → Entering

Validation:
```bash
cargo test -p robson-engine entry
```

Checklist:
- [ ] Entry logic complete
- [ ] Tests pass (minimum 4 tests)
- [ ] No panics, all errors handled
```

---

### Prompt 2.3: Implement Stop Loss Monitor

```
Implement stop loss monitoring in robson-engine/src/monitor.rs:

1. Create check_stop_loss() method:
   ```rust
   pub fn check_stop_loss(
       &self,
       position: &Position,
       current_price: Price,
   ) -> Option<EngineAction>
   ```

2. Logic:
   - Only check if position is Active
   - For Long: Trigger if price <= stop_loss
   - For Short: Trigger if price >= stop_loss
   - Return PlaceOrder(exit) action if triggered

3. Add tests:
   - Test Long SL trigger (price drops below SL)
   - Test Long no trigger (price above SL)
   - Test Short SL trigger (price rises above SL)
   - Test Short no trigger (price below SL)
   - Test non-Active position returns None

Validation:
```bash
cargo test -p robson-engine monitor::check_stop_loss
```

Checklist:
- [ ] SL monitoring works for both sides
- [ ] No false triggers
- [ ] Tests pass (minimum 5 tests)
```

---

### Prompt 2.4: Implement Stop Gain Monitor

```
Same as Prompt 2.3, but for stop gain:

1. Create check_stop_gain() method
2. Logic: Trigger when price crosses stop_gain
3. Add tests for both Long and Short

Validation:
```bash
cargo test -p robson-engine monitor::check_stop_gain
```

Checklist:
- [ ] SG monitoring works
- [ ] Tests pass
```

---

## Phase 3: Storage & Persistence

### Prompt 3.1: Create Database Schema

```
Create PostgreSQL schema in robson-store/migrations/001_initial.sql:

1. Create migrations directory
2. Copy schema from EXECUTION-PLAN.md Phase 3.1
3. Tables to create:
   - positions (snapshots)
   - events (event sourcing)
   - intents (idempotency)
   - leases (leader election)

4. Add indexes:
   - idx_events_position_id
   - idx_intents_position_id
   - idx_intents_status
   - idx_leases_expires_at

Validation:
```bash
# Start PostgreSQL (Docker)
docker run --name robson-postgres -e POSTGRES_PASSWORD=robson -p 5432:5432 -d postgres:16

# Install sqlx-cli
cargo install sqlx-cli --no-default-features --features postgres

# Run migration
cd v2/robson-store
export DATABASE_URL="postgres://postgres:robson@localhost/robson"
sqlx database create
sqlx migrate run

# Verify tables
psql $DATABASE_URL -c "\dt"
```

Checklist:
- [ ] Migration file created
- [ ] All tables created
- [ ] Indexes exist
- [ ] Can connect to database
```

---

### Prompt 3.2: Implement Store

```
Implement store in robson-store/src/lib.rs:

1. Add dependencies: sqlx, uuid, chrono

2. Create Store struct:
   ```rust
   pub struct Store {
       pool: PgPool,
   }
   ```

3. Implement methods:
   - new(database_url) -> Result<Self>
   - append_event(event) -> Result<i64>
   - load_events(position_id) -> Result<Vec<Event>>
   - save_position_snapshot(position) -> Result<()>
   - load_position(position_id) -> Result<Position>

4. Add sqlx::test tests (requires #[sqlx::test] attribute)

Validation:
```bash
cargo test -p robson-store
```

Checklist:
- [ ] All methods implemented
- [ ] Tests pass with PostgreSQL
- [ ] Can append and load events
```

---

### Prompt 3.3: Implement Lease Manager

```
Implement lease manager in robson-store/src/lease.rs:

1. Create LeaseManager struct
2. Implement acquire_lease(key) using Postgres advisory locks
3. Implement renew_lease(lease)
4. Implement release_lease(lease)

5. Add tests:
   - Test single acquisition
   - Test double acquisition fails
   - Test renewal works
   - Test expiration allows re-acquisition

Validation:
```bash
cargo test -p robson-store lease
```

Checklist:
- [ ] Leader election works
- [ ] Tests pass
- [ ] No split-brain possible
```

---

## Phase 4: Execution Layer

### Prompt 4.1: Implement Intent Journal

```
Create intent journal in robson-exec/src/journal.rs:

1. Create IntentJournal struct
2. Implement:
   - append_intent(intent) -> Result<()>
   - get_intent(intent_id) -> Result<Option<Intent>>
   - mark_completed(intent_id, result) -> Result<()>
   - mark_failed(intent_id, error) -> Result<()>

3. Add tests for idempotency:
   - Duplicate append is idempotent
   - Can retrieve intent by ID
   - Status updates work

Validation:
```bash
cargo test -p robson-exec journal
```

Checklist:
- [ ] Journal implemented
- [ ] Idempotency guaranteed
- [ ] Tests pass
```

---

### Prompt 4.2: Implement Execution Engine with Stub Connector

```
Create execution engine in robson-exec/src/lib.rs:

1. Create ExecutionEngine struct:
   - journal: IntentJournal
   - connector: Box<dyn ExchangeConnector>

2. Implement execute_action(action) -> Result<ExecResult>

3. Create StubConnector in robson-connectors/src/stub.rs:
   - place_order() returns immediate fill
   - cancel_order() succeeds
   - get_position() returns empty position

4. Add tests with stub connector

Validation:
```bash
cargo test -p robson-exec
```

Checklist:
- [ ] Execution engine works
- [ ] Stub connector integrated
- [ ] Tests pass
```

---

## Phase 5: Daemon Runtime

### Prompt 5.1: Create Basic Daemon

```
Create daemon in robsond/src/main.rs:

1. Create Daemon struct with:
   - engine: Engine
   - exec: ExecutionEngine
   - store: Store
   - lease_manager: LeaseManager

2. Implement main():
   - Parse config from env
   - Create daemon
   - Run daemon

3. Implement Daemon::run():
   - Acquire lease
   - Log "Daemon started"
   - Sleep forever (for now)

4. Add graceful shutdown on SIGTERM

Validation:
```bash
export DATABASE_URL="postgres://postgres:robson@localhost/robson"
cargo run -p robsond

# In another terminal:
curl http://localhost:8080/health/live  # Should fail (not implemented yet)
```

Checklist:
- [ ] Daemon compiles and runs
- [ ] Acquires lease
- [ ] Responds to SIGTERM
```

---

### Prompt 5.2: Add API Server

```
Add API server to daemon in robsond/src/api/mod.rs:

1. Add dependencies: axum, tower-http

2. Create routes:
   - GET /health/live → 200 "ok"
   - GET /health/ready → 200 if DB connected, 503 otherwise
   - GET /status → Return positions JSON

3. Start API server on :8080 in Daemon::run()

4. Test:
```bash
cargo run -p robsond

# In another terminal:
curl http://localhost:8080/health/live
curl http://localhost:8080/health/ready
curl http://localhost:8080/status | jq
```

Checklist:
- [ ] API server runs
- [ ] Health endpoints work
- [ ] Status endpoint returns JSON
```

---

## Phase 6: CLI Integration

### Prompt 6.1: Implement API Client

```
Create API client in v2/cli/src/api/client.ts:

1. Create RobsonClient class:
   - Constructor takes baseURL
   - Uses axios for HTTP requests

2. Implement methods:
   - status() -> Promise<StatusResponse>
   - arm(request) -> Promise<ArmResponse> (stub for now)

3. Add error handling

Validation:
```bash
cd v2/cli
bun run src/test-client.ts  # Create test script
```

Checklist:
- [ ] Client connects to daemon
- [ ] Can fetch status
- [ ] Error handling works
```

---

### Prompt 6.2: Implement Status Command

```
Implement status command in v2/cli/src/commands/status.ts:

1. Create status command handler:
   - Fetch from API client
   - Format as table (cli-table3)
   - Support --json flag

2. Register command in index.ts

3. Test:
```bash
cd v2/cli
bun run src/index.ts status
bun run src/index.ts status --json
```

Checklist:
- [ ] Status command works
- [ ] Table formatting correct
- [ ] JSON output valid
```

---

## Phase 7: End-to-End Test

### Prompt 7: Run Full Workflow Test

```
Execute end-to-end test with stub components:

1. Start daemon:
```bash
cargo run -p robsond
```

2. In another terminal, run status:
```bash
cd v2/cli
bun run src/index.ts status
```

Expected output:
```
POSITIONS

(empty table)

Summary: 0 active, 0 armed
```

3. Verify health:
```bash
curl http://localhost:8080/health/live
curl http://localhost:8080/health/ready
```

Both should return 200 OK.

Checklist:
- [ ] Daemon starts and acquires lease
- [ ] API server responds
- [ ] CLI can query status
- [ ] No crashes or errors
```

---

## Summary of Validation Commands

After completing all phases, these should work:

```bash
# Rust
cd v2
cargo build --all
cargo test --all
cargo clippy --all -- -D warnings
cargo fmt --all -- --check

# CLI
cd v2/cli
bun install
bun test
bun run build

# Integration
cargo run -p robsond
cd v2/cli && bun run src/index.ts status
```

---

## Next Steps After Completing Prompt Pack

1. **Phase 8**: Implement real detector (not stub)
2. **Phase 9**: Implement real Binance connector
3. **Phase 10**: Production readiness (monitoring, deployment)

See [EXECUTION-PLAN.md](./EXECUTION-PLAN.md) for details.

---

**Ready to start? Begin with Prompt 0.1!**
