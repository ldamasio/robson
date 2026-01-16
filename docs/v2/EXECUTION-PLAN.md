# Robson v2 Execution Plan

**Version**: 2.0.0-alpha
**Last Updated**: 2026-01-12
**Status**: Planning Phase

---

## Overview

This document outlines the **step-by-step implementation plan** for Robson v2, broken down into small, measurable phases with clear acceptance criteria.

**Guiding Principles**:
1. **Build iteratively**: Each phase produces working, testable code
2. **Validate early**: Tests and validation commands at every step
3. **Fail fast**: Catch errors at compile time, not runtime
4. **Document as you go**: Update docs when behavior changes

---

## Phase 0: Project Bootstrap

**Goal**: Set up Rust workspace, Bun CLI skeleton, and basic tooling

### Tasks

#### 0.1: Create Rust Workspace

```bash
cd v2
cargo init --name robson-v2
```

**Create**:
- `Cargo.toml` (workspace root)
- `robson-domain/Cargo.toml`
- `robson-engine/Cargo.toml`
- `robson-exec/Cargo.toml`
- `robson-connectors/Cargo.toml`
- `robson-store/Cargo.toml`
- `robsond/Cargo.toml`
- `robson-sim/Cargo.toml`

**Workspace Dependencies**:
```toml
[workspace]
members = [
    "robson-domain",
    "robson-engine",
    "robson-exec",
    "robson-connectors",
    "robson-store",
    "robsond",
    "robson-sim",
]
resolver = "2"

[workspace.dependencies]
tokio = { version = "1.35", features = ["full"] }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
rust_decimal = { version = "1.33", features = ["serde"] }
uuid = { version = "1.6", features = ["v7", "serde"] }
chrono = { version = "0.4", features = ["serde"] }
anyhow = "1.0"
thiserror = "1.0"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
sqlx = { version = "0.7", features = ["runtime-tokio-native-tls", "postgres", "uuid", "chrono", "json"] }
axum = "0.7"
tower-http = { version = "0.5", features = ["trace"] }
```

**Acceptance Criteria**:
- [ ] `cargo build` succeeds (empty crates)
- [ ] All crates have `Cargo.toml` with correct dependencies
- [ ] `cargo test` runs (no tests yet, but should not error)

#### 0.2: Create Bun CLI Project

```bash
cd v2/cli
bun init
```

**Create**:
- `package.json`
- `tsconfig.json`
- `src/index.ts` (entry point)
- `src/commands/` (command handlers)
- `src/api/` (API client)

**Dependencies**:
```json
{
  "dependencies": {
    "commander": "^11.1.0",
    "axios": "^1.6.2",
    "chalk": "^5.3.0",
    "cli-table3": "^0.6.3",
    "dotenv": "^16.3.1"
  },
  "devDependencies": {
    "@types/node": "^20.10.5",
    "bun-types": "^1.0.18",
    "typescript": "^5.3.3"
  }
}
```

**Acceptance Criteria**:
- [ ] `bun install` succeeds
- [ ] `bun run src/index.ts --help` shows CLI help
- [ ] TypeScript compilation works (`bun run build`)

#### 0.3: CI/CD Setup

**Create**:
- `.github/workflows/rust-ci.yml`
- `.github/workflows/cli-ci.yml`

**Rust CI**:
```yaml
name: Rust CI

on: [push, pull_request]

jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - run: cargo build --verbose
      - run: cargo test --verbose
      - run: cargo clippy -- -D warnings
      - run: cargo fmt -- --check
```

**Acceptance Criteria**:
- [ ] CI runs on push
- [ ] All checks pass (build, test, clippy, fmt)

---

## Phase 1: Domain Types (Pure Logic)

**Goal**: Implement core domain types in `robson-domain` (zero I/O)

### Tasks

#### 1.1: Value Objects

**Implement** in `robson-domain/src/value_objects.rs`:
- `Price`
- `Quantity`
- `Symbol`
- `Side`
- `Leverage`
- `PalmaDaMao`

**Tests** in `robson-domain/src/value_objects.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_price_validation() {
        assert!(Price::new(dec!(100.0)).is_ok());
        assert!(Price::new(dec!(-1.0)).is_err());
        assert!(Price::new(dec!(0.0)).is_err());
    }

    #[test]
    fn test_palma_calculation() {
        let entry = Price::new(dec!(95000.0)).unwrap();
        let stop = Price::new(dec!(93500.0)).unwrap();
        let palma = PalmaDaMao::from_entry_and_stop(entry, stop);

        assert_eq!(palma.distance, dec!(1500.0));
        assert_eq!(palma.distance_pct, dec!(1.578947368421052631578947368));
    }

    #[test]
    fn test_palma_validation() {
        // Valid palma (1.58%)
        let entry = Price::new(dec!(95000.0)).unwrap();
        let stop = Price::new(dec!(93500.0)).unwrap();
        let palma = PalmaDaMao::from_entry_and_stop(entry, stop);
        assert!(palma.validate().is_ok());

        // Too wide (>10%)
        let entry = Price::new(dec!(100.0)).unwrap();
        let stop = Price::new(dec!(80.0)).unwrap();
        let palma = PalmaDaMao::from_entry_and_stop(entry, stop);
        assert!(palma.validate().is_err());

        // Too tight (<0.1%)
        let entry = Price::new(dec!(100000.0)).unwrap();
        let stop = Price::new(dec!(99990.0)).unwrap();
        let palma = PalmaDaMao::from_entry_and_stop(entry, stop);
        assert!(palma.validate().is_err());
    }
}
```

**Acceptance Criteria**:
- [ ] All value objects have `new()` constructor with validation
- [ ] Invalid values return `Result::Err`
- [ ] All tests pass: `cargo test -p robson-domain`
- [ ] `cargo clippy` reports no warnings
- [ ] `cargo fmt` passes

#### 1.2: Entities (Position, Order, Trade)

**Implement** in `robson-domain/src/entities/`:
- `position.rs`
- `order.rs`
- `trade.rs`

**Focus**: Data structures only, no behavior yet

**Tests**:
```rust
#[test]
fn test_position_creation() {
    let position = Position::new(
        AccountId::new(),
        Symbol::from_pair("BTCUSDT").unwrap(),
        Side::Long,
        StrategyConfig::default(),
    );

    assert_eq!(position.state, PositionState::Armed { .. });
    assert!(position.entry_price.is_none());
    assert_eq!(position.realized_pnl, dec!(0));
}
```

**Acceptance Criteria**:
- [ ] Entities have proper field types (use value objects)
- [ ] `new()` constructors initialize with valid defaults
- [ ] Tests pass: `cargo test -p robson-domain`

#### 1.3: State Machine

**Implement** in `robson-domain/src/state_machine.rs`:
- `PositionState` enum
- State transition validation (compile-time where possible)

**Tests**:
```rust
#[test]
fn test_state_transitions() {
    let mut position = Position::new(...);

    // Armed → Entering
    assert!(position.can_enter());
    let result = position.apply_detector_signal(signal);
    assert!(result.is_ok());
    assert!(matches!(position.state, PositionState::Entering { .. }));

    // Entering → Active
    let result = position.apply_entry_filled(fill);
    assert!(result.is_ok());
    assert!(matches!(position.state, PositionState::Active { .. }));

    // Invalid transition: Armed → Active (should fail)
    let mut position = Position::new(...);
    let result = position.apply_entry_filled(fill);
    assert!(result.is_err());
}
```

**Acceptance Criteria**:
- [ ] All valid transitions succeed
- [ ] Invalid transitions return `Err`
- [ ] Tests cover all state transitions
- [ ] Tests pass: `cargo test -p robson-domain`

#### 1.4: Position Sizing Logic

**Implement** in `robson-domain/src/position.rs`:
```rust
impl Position {
    pub fn calculate_position_size(
        &self,
        entry_price: Price,
        stop_loss: Price,
    ) -> Result<Quantity, DomainError> {
        // Implementation from DOMAIN.md
    }
}
```

**Tests**:
```rust
#[test]
fn test_position_sizing() {
    let risk_config = RiskConfig {
        capital: dec!(10000),
        risk_per_trade_pct: dec!(1),
        ..Default::default()
    };

    let position = Position::new_with_config(risk_config);

    let entry = Price::new(dec!(95000)).unwrap();
    let stop = Price::new(dec!(93500)).unwrap();

    let size = position.calculate_position_size(entry, stop).unwrap();

    // Expected: (10000 * 0.01) / 1500 = 0.0666... BTC
    assert_eq!(size.as_decimal(), dec!(0.066666666666666666666666666667));
}
```

**Acceptance Criteria**:
- [ ] Formula matches golden rule from DOMAIN.md
- [ ] Tests validate calculation
- [ ] Edge cases handled (too small, too large)
- [ ] Tests pass: `cargo test -p robson-domain`

#### 1.5: Events

**Implement** in `robson-domain/src/events.rs`:
- `Event` enum
- Serialization/deserialization (`serde`)

**Tests**:
```rust
#[test]
fn test_event_serialization() {
    let event = Event::PositionArmed {
        position_id: PositionId::new(),
        symbol: Symbol::from_pair("BTCUSDT").unwrap(),
        strategy: "all-in".to_string(),
    };

    let json = serde_json::to_string(&event).unwrap();
    let deserialized: Event = serde_json::from_str(&json).unwrap();

    assert_eq!(event, deserialized);
}
```

**Acceptance Criteria**:
- [ ] All event types defined
- [ ] Serialization/deserialization works
- [ ] Tests pass: `cargo test -p robson-domain`

---

## Phase 2: Engine (Pure Decision Logic)

**Goal**: Implement deterministic engine that decides actions based on input

### Tasks

#### 2.1: Engine Structure

**Implement** in `robson-engine/src/lib.rs`:
```rust
pub struct Engine {
    risk_config: RiskConfig,
}

pub struct EngineInput {
    pub position: Position,
    pub market_data: MarketData,
    pub intents: Vec<Intent>,
}

pub struct EngineDecision {
    pub actions: Vec<EngineAction>,
    pub new_state: Position,
}

pub enum EngineAction {
    PlaceOrder(OrderIntent),
    CancelOrder(OrderId),
    UpdatePosition(Position),
    EmitEvent(Event),
}
```

**Acceptance Criteria**:
- [ ] Compiles with no I/O dependencies
- [ ] Pure function: `decide(input) -> output`

#### 2.2: Entry Decision

**Implement**:
```rust
impl Engine {
    pub fn decide_entry(
        &self,
        position: &Position,
        signal: DetectorSignal,
    ) -> Result<EngineDecision, EngineError> {
        // Validate signal
        // Calculate palma, sizing
        // Return PlaceOrder action
    }
}
```

**Tests**:
```rust
#[test]
fn test_decide_entry() {
    let engine = Engine::new(RiskConfig::default());
    let position = Position::new(...);
    let signal = DetectorSignal {
        entry_price: Price::new(dec!(95000)).unwrap(),
        stop_loss: Price::new(dec!(93500)).unwrap(),
        stop_gain: Price::new(dec!(98500)).unwrap(),
    };

    let decision = engine.decide_entry(&position, signal).unwrap();

    assert_eq!(decision.actions.len(), 1);
    assert!(matches!(decision.actions[0], EngineAction::PlaceOrder(_)));
    assert!(matches!(decision.new_state.state, PositionState::Entering { .. }));
}
```

**Acceptance Criteria**:
- [ ] Tests validate entry logic
- [ ] Invalid signals rejected
- [ ] Tests pass: `cargo test -p robson-engine`

#### 2.3: Stop Loss Monitor

**Implement**:
```rust
impl Engine {
    pub fn check_stop_loss(
        &self,
        position: &Position,
        current_price: Price,
    ) -> Option<EngineAction> {
        // Check if SL triggered
        // Return exit action if triggered
    }
}
```

**Tests**:
```rust
#[test]
fn test_stop_loss_trigger() {
    let engine = Engine::new(RiskConfig::default());
    let position = active_position_with_sl(dec!(93500));

    // Price below SL → Trigger
    let current_price = Price::new(dec!(93400)).unwrap();
    let action = engine.check_stop_loss(&position, current_price);
    assert!(action.is_some());

    // Price above SL → No trigger
    let current_price = Price::new(dec!(94000)).unwrap();
    let action = engine.check_stop_loss(&position, current_price);
    assert!(action.is_none());
}
```

**Acceptance Criteria**:
- [ ] SL triggers when price crosses stop
- [ ] No false triggers
- [ ] Tests pass: `cargo test -p robson-engine`

#### 2.4: Stop Gain Monitor

Same as 2.3, but for stop gain.

**Acceptance Criteria**:
- [ ] SG triggers when price crosses target
- [ ] Tests pass: `cargo test -p robson-engine`

---

## Phase 3: Storage & Persistence

**Goal**: Implement PostgreSQL event store and lease manager

### Tasks

#### 3.1: Database Schema

**Create** `robson-store/migrations/001_initial.sql`:
```sql
-- Positions table (snapshots)
CREATE TABLE positions (
    id UUID PRIMARY KEY,
    account_id UUID NOT NULL,
    symbol TEXT NOT NULL,
    side TEXT NOT NULL,
    state TEXT NOT NULL,
    state_data JSONB NOT NULL,
    entry_price DECIMAL,
    stop_loss DECIMAL NOT NULL,
    stop_gain DECIMAL NOT NULL,
    quantity DECIMAL NOT NULL,
    leverage SMALLINT NOT NULL,
    realized_pnl DECIMAL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    closed_at TIMESTAMPTZ
);

-- Events table (event sourcing)
CREATE TABLE events (
    id BIGSERIAL PRIMARY KEY,
    position_id UUID NOT NULL,
    event_type TEXT NOT NULL,
    event_data JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_events_position_id ON events(position_id);

-- Intents table (idempotency)
CREATE TABLE intents (
    id UUID PRIMARY KEY,
    position_id UUID NOT NULL,
    intent_type TEXT NOT NULL,
    intent_data JSONB NOT NULL,
    status TEXT NOT NULL,
    result JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    completed_at TIMESTAMPTZ
);

CREATE INDEX idx_intents_position_id ON intents(position_id);
CREATE INDEX idx_intents_status ON intents(status);

-- Leases table (leader election)
CREATE TABLE leases (
    key TEXT PRIMARY KEY,
    instance_id UUID NOT NULL,
    acquired_at TIMESTAMPTZ NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    CONSTRAINT expires_in_future CHECK (expires_at > acquired_at)
);

CREATE INDEX idx_leases_expires_at ON leases(expires_at);
```

**Run Migration**:
```bash
cargo install sqlx-cli
cd robson-store
sqlx migrate run
```

**Acceptance Criteria**:
- [ ] Migration creates all tables
- [ ] Indexes created
- [ ] Constraints enforced

#### 3.2: Store Implementation

**Implement** in `robson-store/src/lib.rs`:
```rust
pub struct Store {
    pool: PgPool,
}

impl Store {
    pub async fn new(database_url: &str) -> Result<Self, StoreError> {
        let pool = PgPool::connect(database_url).await?;
        Ok(Self { pool })
    }

    pub async fn append_event(&self, event: Event) -> Result<i64, StoreError> {
        // Insert into events table
    }

    pub async fn load_events(
        &self,
        position_id: PositionId,
    ) -> Result<Vec<Event>, StoreError> {
        // Query events table
    }

    pub async fn save_position_snapshot(
        &self,
        position: &Position,
    ) -> Result<(), StoreError> {
        // Upsert into positions table
    }

    pub async fn load_position(
        &self,
        position_id: PositionId,
    ) -> Result<Position, StoreError> {
        // Load snapshot + replay events
    }
}
```

**Tests** (requires PostgreSQL):
```rust
#[sqlx::test]
async fn test_event_append(pool: PgPool) {
    let store = Store::from_pool(pool);
    let event = Event::PositionArmed { .. };

    let event_id = store.append_event(event).await.unwrap();
    assert!(event_id > 0);
}

#[sqlx::test]
async fn test_load_events(pool: PgPool) {
    let store = Store::from_pool(pool);
    let position_id = PositionId::new();

    // Append multiple events
    store.append_event(Event::PositionArmed { position_id, .. }).await.unwrap();
    store.append_event(Event::EntryOrderPlaced { position_id, .. }).await.unwrap();

    // Load events
    let events = store.load_events(position_id).await.unwrap();
    assert_eq!(events.len(), 2);
}
```

**Acceptance Criteria**:
- [ ] All store methods implemented
- [ ] Tests pass: `cargo test -p robson-store` (requires Postgres)
- [ ] Can reconstruct position from events

#### 3.3: Lease Manager

**Implement** in `robson-store/src/lease.rs`:
```rust
pub struct LeaseManager {
    pool: PgPool,
    instance_id: Uuid,
    ttl: Duration,
}

impl LeaseManager {
    pub async fn acquire_lease(&self, key: &str) -> Result<Lease, LeaseError> {
        // Acquire advisory lock + insert lease record
    }

    pub async fn renew_lease(&self, lease: &Lease) -> Result<(), LeaseError> {
        // Update expires_at
    }

    pub async fn release_lease(&self, lease: &Lease) -> Result<(), LeaseError> {
        // Delete lease + release advisory lock
    }
}
```

**Tests**:
```rust
#[sqlx::test]
async fn test_lease_acquisition(pool: PgPool) {
    let manager = LeaseManager::new(pool.clone(), Duration::from_secs(30));

    let lease = manager.acquire_lease("BTCUSDT").await.unwrap();
    assert_eq!(lease.key, "BTCUSDT");

    // Second acquisition should fail
    let manager2 = LeaseManager::new(pool, Duration::from_secs(30));
    let result = manager2.acquire_lease("BTCUSDT").await;
    assert!(result.is_err());
}

#[sqlx::test]
async fn test_lease_renewal(pool: PgPool) {
    let manager = LeaseManager::new(pool, Duration::from_secs(30));
    let lease = manager.acquire_lease("BTCUSDT").await.unwrap();

    tokio::time::sleep(Duration::from_secs(5)).await;

    let result = manager.renew_lease(&lease).await;
    assert!(result.is_ok());
}
```

**Acceptance Criteria**:
- [ ] Lease acquisition/renewal/release works
- [ ] Multiple instances cannot hold same lease
- [ ] Expired leases can be acquired
- [ ] Tests pass: `cargo test -p robson-store`

---

## Phase 4: Execution Layer

**Goal**: Idempotent order execution with intent journal

### Tasks

#### 4.1: Intent Journal

**Implement** in `robson-exec/src/journal.rs`:
```rust
pub struct IntentJournal {
    store: Store,
}

impl IntentJournal {
    pub async fn append_intent(
        &self,
        intent: &OrderIntent,
    ) -> Result<(), ExecError> {
        // Insert into intents table
    }

    pub async fn mark_completed(
        &self,
        intent_id: IntentId,
        result: &OrderResult,
    ) -> Result<(), ExecError> {
        // Update status to Completed
    }

    pub async fn get_intent(
        &self,
        intent_id: IntentId,
    ) -> Result<Option<Intent>, ExecError> {
        // Query intents table
    }
}
```

**Tests**:
```rust
#[sqlx::test]
async fn test_intent_idempotency(pool: PgPool) {
    let store = Store::from_pool(pool);
    let journal = IntentJournal::new(store);

    let intent = OrderIntent { id: IntentId::new(), .. };

    // First append
    journal.append_intent(&intent).await.unwrap();

    // Second append (same ID) should succeed (idempotent)
    journal.append_intent(&intent).await.unwrap();

    // Verify only one record
    let loaded = journal.get_intent(intent.id).await.unwrap().unwrap();
    assert_eq!(loaded.id, intent.id);
}
```

**Acceptance Criteria**:
- [ ] Intents can be appended
- [ ] Duplicate intent IDs are idempotent
- [ ] Tests pass: `cargo test -p robson-exec`

#### 4.2: Execution Engine (Stub Connector)

**Implement** in `robson-exec/src/lib.rs`:
```rust
pub struct ExecutionEngine {
    journal: IntentJournal,
    connector: Box<dyn ExchangeConnector>,
}

impl ExecutionEngine {
    pub async fn execute_action(
        &mut self,
        action: EngineAction,
    ) -> Result<ExecResult, ExecError> {
        match action {
            EngineAction::PlaceOrder(intent) => {
                self.place_order(intent).await
            }
            // ...
        }
    }

    async fn place_order(
        &mut self,
        intent: OrderIntent,
    ) -> Result<ExecResult, ExecError> {
        // 1. Check journal for existing intent
        if let Some(existing) = self.journal.get_intent(&intent.id).await? {
            return Ok(existing.result);
        }

        // 2. Append intent to journal (WAL)
        self.journal.append_intent(&intent).await?;

        // 3. Execute on exchange
        let result = self.connector.place_order(intent).await?;

        // 4. Mark completed
        self.journal.mark_completed(&intent.id, &result).await?;

        Ok(ExecResult::Success(result))
    }
}
```

**Use Stub Connector**:
```rust
pub struct StubConnector;

#[async_trait]
impl ExchangeConnector for StubConnector {
    async fn place_order(&self, intent: OrderIntent) -> Result<OrderResponse> {
        Ok(OrderResponse {
            order_id: OrderId::new(),
            status: OrderStatus::Filled,
            filled_quantity: intent.quantity,
            average_fill_price: intent.price,
        })
    }

    // Other methods return stub responses
}
```

**Tests**:
```rust
#[sqlx::test]
async fn test_idempotent_order_placement(pool: PgPool) {
    let store = Store::from_pool(pool);
    let journal = IntentJournal::new(store.clone());
    let connector = Box::new(StubConnector);
    let mut exec = ExecutionEngine::new(journal, connector);

    let intent = OrderIntent { id: IntentId::new(), .. };

    // First execution
    let result1 = exec.execute_action(EngineAction::PlaceOrder(intent.clone())).await.unwrap();

    // Second execution (same intent ID)
    let result2 = exec.execute_action(EngineAction::PlaceOrder(intent)).await.unwrap();

    // Should return same result (from journal)
    assert_eq!(result1, result2);

    // Verify connector called only once
    // (Would need mock connector to assert this)
}
```

**Acceptance Criteria**:
- [ ] Orders placed via stub connector
- [ ] Idempotency enforced (journal check)
- [ ] Tests pass: `cargo test -p robson-exec`

---

## Phase 5: Daemon Runtime

**Goal**: Orchestrate engine, execution, storage, and API

### Tasks

#### 5.1: Daemon Structure

**Implement** in `robsond/src/main.rs`:
```rust
#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let config = Config::from_env()?;
    let daemon = Daemon::new(config).await?;

    daemon.run().await
}

pub struct Daemon {
    engine: Engine,
    exec: ExecutionEngine,
    store: Store,
    lease_manager: LeaseManager,
    api_server: ApiServer,
}

impl Daemon {
    pub async fn run(mut self) -> Result<()> {
        // 1. Acquire lease
        let lease = self.lease_manager.acquire_lease("default").await?;
        info!("Acquired lease: {}", lease.key);

        // 2. Reconcile state
        self.reconcile_all_positions().await?;

        // 3. Start API server
        let api_task = tokio::spawn(async move {
            self.api_server.serve().await
        });

        // 4. Main loop
        let main_task = tokio::spawn(async move {
            self.main_loop(lease).await
        });

        // 5. Wait for shutdown signal
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                info!("Shutdown signal received");
            }
            _ = api_task => {}
            _ = main_task => {}
        }

        // 6. Graceful shutdown
        self.shutdown().await?;

        Ok(())
    }

    async fn main_loop(&mut self, lease: Lease) -> Result<()> {
        let mut interval = tokio::time::interval(Duration::from_secs(1));

        loop {
            interval.tick().await;

            // Renew lease every 10 ticks
            if interval.elapsed().as_secs() % 10 == 0 {
                self.lease_manager.renew_lease(&lease).await?;
            }

            // Process active positions
            let positions = self.store.load_active_positions().await?;
            for position in positions {
                self.process_position(position).await?;
            }
        }
    }

    async fn process_position(&mut self, position: Position) -> Result<()> {
        // Get market data
        let market_data = self.get_market_data(&position.symbol).await?;

        // Run engine
        let input = EngineInput {
            position: position.clone(),
            market_data,
            intents: vec![],
        };

        let decision = self.engine.decide(input)?;

        // Execute actions
        for action in decision.actions {
            self.exec.execute_action(action).await?;
        }

        // Save updated position
        self.store.save_position_snapshot(&decision.new_state).await?;

        Ok(())
    }

    async fn shutdown(&mut self) -> Result<()> {
        info!("Shutting down gracefully...");

        // 1. Cancel all open orders
        // 2. Release lease
        // 3. Close database connections

        Ok(())
    }
}
```

**Acceptance Criteria**:
- [ ] Daemon starts and acquires lease
- [ ] Main loop runs (even with stub data)
- [ ] Graceful shutdown on SIGTERM
- [ ] Can run: `cargo run -p robsond`

#### 5.2: API Server (Basic Endpoints)

**Implement** in `robsond/src/api/mod.rs`:
```rust
use axum::{Router, Json, extract::State};

pub async fn serve(store: Store) -> Result<()> {
    let app = Router::new()
        .route("/health/live", get(health_live))
        .route("/health/ready", get(health_ready))
        .route("/status", get(status))
        .with_state(store);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:8080").await?;
    info!("API server listening on http://0.0.0.0:8080");

    axum::serve(listener, app).await?;

    Ok(())
}

async fn health_live() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok".to_string() })
}

async fn health_ready(State(store): State<Store>) -> Json<HealthResponse> {
    // Check DB connection
    match store.health_check().await {
        Ok(_) => Json(HealthResponse { status: "ready".to_string() }),
        Err(_) => Json(HealthResponse { status: "not ready".to_string() }),
    }
}

async fn status(State(store): State<Store>) -> Json<StatusResponse> {
    let positions = store.load_all_positions().await.unwrap_or_default();

    Json(StatusResponse {
        positions,
        summary: Summary {
            active_count: positions.iter().filter(|p| p.is_active()).count(),
            armed_count: positions.iter().filter(|p| p.can_enter()).count(),
            closed_today_count: 0,
            total_pnl_today: dec!(0),
        },
    })
}
```

**Test**:
```bash
# Terminal 1
cargo run -p robsond

# Terminal 2
curl http://localhost:8080/health/live
curl http://localhost:8080/status
```

**Acceptance Criteria**:
- [ ] Health endpoints respond
- [ ] Status endpoint returns JSON
- [ ] Can query: `curl http://localhost:8080/status | jq`

---

## Phase 6: CLI Integration

**Goal**: Connect Bun CLI to robsond API

### Tasks

#### 6.1: API Client

**Implement** in `cli/src/api/client.ts`:
```typescript
import axios, { AxiosInstance } from 'axios';

export class RobsonClient {
  private client: AxiosInstance;

  constructor(baseURL: string) {
    this.client = axios.create({
      baseURL,
      timeout: 5000,
      headers: {
        'Content-Type': 'application/json',
      },
    });
  }

  async status(): Promise<StatusResponse> {
    const response = await this.client.get<StatusResponse>('/status');
    return response.data;
  }

  async arm(request: ArmRequest): Promise<ArmResponse> {
    const response = await this.client.post<ArmResponse>('/arm', request);
    return response.data;
  }

  // Other methods...
}
```

**Acceptance Criteria**:
- [ ] Client connects to daemon
- [ ] Can fetch status
- [ ] Error handling implemented

#### 6.2: Status Command

**Implement** in `cli/src/commands/status.ts`:
```typescript
import { Command } from 'commander';
import { RobsonClient } from '../api/client';
import Table from 'cli-table3';

export function registerStatusCommand(program: Command) {
  program
    .command('status')
    .option('--json', 'Output as JSON')
    .action(async (options) => {
      const client = new RobsonClient('http://localhost:8080');

      try {
        const status = await client.status();

        if (options.json) {
          console.log(JSON.stringify(status, null, 2));
        } else {
          printTable(status);
        }
      } catch (error) {
        console.error('Error:', error.message);
        process.exit(1);
      }
    });
}

function printTable(status: StatusResponse) {
  const table = new Table({
    head: ['ID', 'Symbol', 'Side', 'State', 'Entry', 'SL', 'SG', 'PnL'],
  });

  for (const position of status.positions) {
    table.push([
      position.id.substring(0, 8),
      position.symbol,
      position.side,
      position.state,
      position.entry_price?.toFixed(2) || '-',
      position.stop_loss.toFixed(2),
      position.stop_gain.toFixed(2),
      position.realized_pnl?.toFixed(2) || '-',
    ]);
  }

  console.log(table.toString());
  console.log(`\nSummary: ${status.summary.active_count} active, ${status.summary.armed_count} armed`);
}
```

**Test**:
```bash
# Terminal 1
cargo run -p robsond

# Terminal 2
cd cli
bun run src/index.ts status
bun run src/index.ts status --json
```

**Acceptance Criteria**:
- [ ] Status command works
- [ ] Table output formatted correctly
- [ ] JSON output parseable
- [ ] Can run: `robson status`

#### 6.3: Arm Command (Stub)

**Implement** basic arm command (full implementation in Phase 7):
```typescript
program
  .command('arm <symbol>')
  .option('--strategy <name>', 'Strategy name')
  .action(async (symbol, options) => {
    console.log(`Arming ${symbol} with strategy ${options.strategy}`);
    // TODO: Call API when endpoint implemented
  });
```

**Acceptance Criteria**:
- [ ] Command parses arguments
- [ ] Placeholder response shown
- [ ] Can run: `robson arm BTCUSDT --strategy all-in`

---

## Phase 7: Detector (Pluggable Interface)

**Goal**: Create pluggable detector interface with stub implementation

### Tasks

#### 7.1: Detector Interface

**Implement** in `robson-domain/src/detector.rs`:
```rust
#[async_trait]
pub trait Detector: Send + Sync {
    async fn scan_for_entry(
        &self,
        symbol: Symbol,
        config: DetectorConfig,
    ) -> Result<Option<DetectorSignal>, DetectorError>;
}

pub struct DetectorSignal {
    pub symbol: Symbol,
    pub side: Side,
    pub entry_price: Price,
    pub stop_loss: Price,
    pub stop_gain: Price,
    pub confidence: f64,
}

pub struct DetectorConfig {
    pub strategy: String,
    pub timeframe: String,
    pub parameters: serde_json::Value,
}
```

#### 7.2: Stub Detector

**Implement** in `robson-connectors/src/detector/stub.rs`:
```rust
pub struct StubDetector;

#[async_trait]
impl Detector for StubDetector {
    async fn scan_for_entry(
        &self,
        symbol: Symbol,
        config: DetectorConfig,
    ) -> Result<Option<DetectorSignal>, DetectorError> {
        // Always return a signal after 10 seconds (for testing)
        tokio::time::sleep(Duration::from_secs(10)).await;

        Ok(Some(DetectorSignal {
            symbol,
            side: Side::Long,
            entry_price: Price::new(dec!(95000)).unwrap(),
            stop_loss: Price::new(dec!(93500)).unwrap(),
            stop_gain: Price::new(dec!(98500)).unwrap(),
            confidence: 0.8,
        }))
    }
}
```

**Acceptance Criteria**:
- [ ] Interface defined
- [ ] Stub detector always returns signal after delay
- [ ] Can be integrated into daemon

---

## Phase 8: End-to-End Test

**Goal**: Validate full workflow with stub components

### Test Scenario

```bash
# 1. Start daemon
cargo run -p robsond

# 2. Arm position
robson arm BTCUSDT --strategy all-in

# 3. Watch status (should transition Armed → Entering → Active after 10s)
robson status --watch

# 4. Simulate price drop (trigger SL)
# (Manual: Update stub market data to return price below SL)

# 5. Verify exit
robson status
# Should show: Exiting → Closed

# 6. Check history
robson history
```

**Acceptance Criteria**:
- [ ] Position arms successfully
- [ ] Detector signal triggers entry
- [ ] Entry order placed (stub)
- [ ] Position becomes active
- [ ] SL monitor detects trigger
- [ ] Exit order placed (stub)
- [ ] Position closes with PnL
- [ ] All events logged to database

---

## Phase 9: Real Exchange Connector

**Goal**: Replace stub connector with real Binance API

### Tasks

#### 9.1: Binance REST Client

**Implement** in `robson-connectors/src/binance/rest.rs`:
```rust
pub struct BinanceRestClient {
    client: reqwest::Client,
    api_key: String,
    secret_key: String,
}

impl BinanceRestClient {
    pub async fn place_market_order(
        &self,
        symbol: &str,
        side: Side,
        quantity: Decimal,
    ) -> Result<OrderResponse> {
        // Sign request
        // POST /api/v3/order
    }

    pub async fn get_position(
        &self,
        symbol: &str,
    ) -> Result<PositionSnapshot> {
        // GET /sapi/v1/margin/isolated/account
    }
}
```

**Tests** (requires testnet credentials):
```rust
#[tokio::test]
#[ignore] // Run manually with credentials
async fn test_place_order_testnet() {
    let client = BinanceRestClient::new_testnet(
        env::var("BINANCE_API_KEY").unwrap(),
        env::var("BINANCE_SECRET_KEY").unwrap(),
    );

    let response = client.place_market_order("BTCUSDT", Side::Buy, dec!(0.001)).await.unwrap();
    assert!(response.order_id.len() > 0);
}
```

**Acceptance Criteria**:
- [ ] Can place orders on testnet
- [ ] Can query positions
- [ ] Error handling for rate limits
- [ ] Tests pass: `cargo test -p robson-connectors -- --ignored`

#### 9.2: Binance WebSocket Client

**Implement** in `robson-connectors/src/binance/websocket.rs`:
```rust
pub struct BinanceWebSocketClient {
    stream: WebSocketStream<MaybeTlsStream<TcpStream>>,
}

impl BinanceWebSocketClient {
    pub async fn subscribe_market_data(&mut self, symbol: &str) -> Result<()> {
        // Subscribe to ticker stream
    }

    pub async fn next_event(&mut self) -> Result<MarketEvent> {
        // Read next WebSocket message
    }
}
```

**Acceptance Criteria**:
- [ ] Can subscribe to market data
- [ ] Can receive price updates
- [ ] Reconnect on disconnect
- [ ] Tests pass (with testnet)

---

## Phase 10: Production Readiness

### Tasks

- [ ] Add comprehensive logging
- [ ] Add metrics (Prometheus)
- [ ] Kubernetes deployment manifests
- [ ] Health check probes
- [ ] Graceful shutdown
- [ ] Backup/restore procedures
- [ ] Monitoring dashboard
- [ ] Alert rules
- [ ] Documentation

---

## Summary

| Phase | Goal | Estimated LOC | Key Deliverable |
|-------|------|---------------|-----------------|
| 0 | Bootstrap | 100 | Workspace setup, CI |
| 1 | Domain | 500 | Pure domain types, tests |
| 2 | Engine | 400 | Deterministic decision logic |
| 3 | Storage | 600 | PostgreSQL store, leases |
| 4 | Execution | 500 | Idempotent order execution |
| 5 | Daemon | 400 | Runtime orchestration |
| 6 | CLI | 300 | Bun CLI with API client |
| 7 | Detector | 200 | Pluggable interface + stub |
| 8 | E2E Test | - | Full workflow validation |
| 9 | Exchange | 800 | Real Binance connector |
| 10 | Production | 400 | Observability, deployment |

**Total Estimated LOC**: ~4,200 Rust + 300 TypeScript

**Estimated Timeline**: 4-6 weeks (1 developer)

---

**Next**: See [PROMPT-PACK.md](./PROMPT-PACK.md) for agentic coding prompts
