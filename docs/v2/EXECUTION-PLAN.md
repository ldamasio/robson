# Robson v2 Execution Plan

**Version**: 2.0.0-alpha
**Last Updated**: 2026-01-16
**Status**: Phase 4 Ready

---

## Progress Summary

| Phase | Status | Tests | Key Deliverables |
|-------|--------|-------|------------------|
| 0 - Bootstrap | ✅ Complete | - | Workspace, crates, tooling |
| 1 - Domain | ✅ Complete | 38 | Entities, value objects, events, state machine |
| 2 - Engine | ✅ Complete | 21 | Entry logic, trailing stop, exit triggers |
| 3 - Storage | ✅ Complete | 14 | Repository pattern, in-memory store |
| 4 - Execution | ⏳ Ready | - | Ports, intent journal, executor |
| 5 - Daemon | ⏳ Blocked | - | Runtime orchestration |
| 6 - CLI | ⏳ Blocked | - | TypeScript commands |
| 7 - Detector | ⏳ Blocked | - | Pluggable interface |
| 8 - E2E Test | ⏳ Blocked | - | Full workflow validation |
| 9 - Exchange | ⏳ Blocked | - | Binance connector |
| 10 - Production | ⏳ Blocked | - | Observability, deployment |

**Total Tests**: 73 passing

---

## Design Evolution Notes

During implementation, several design decisions evolved from the original plan:

### 1. Naming: PalmaDaMao → TechnicalStopDistance

The original Portuguese term was renamed for clarity:
- **Old**: `PalmaDaMao` (literal translation: "palm of the hand")
- **New**: `TechnicalStopDistance` (self-documenting)

### 2. Exit Strategy: Fixed Stops → Trailing Stop Only

The original design had separate stop_loss and stop_gain. The implementation uses a **single trailing stop** that moves with favorable price action:

- **Old Design**: `stop_loss` (fixed) + `stop_gain` (fixed)
- **New Design**: `trailing_stop` (dynamic) that follows price by `tech_stop_distance`

**Rationale**: Trailing stops capture more profit in trending markets while maintaining the same risk per trade.

### 3. Removed: Leverage Value Object

Leverage was removed as a configurable parameter:
- **Old**: `Leverage(u8)` value object with 1-10x range
- **New**: Fixed 10x leverage (Binance isolated margin default)

**Rationale**: Simplifies position sizing. Risk is controlled by position size, not leverage.

### 4. Removed: Trade Entity

The separate Trade entity was consolidated into Order:
- **Old**: `Order` (instruction) → `Trade` (execution)
- **New**: `Order` with fill fields (`fill_price`, `filled_quantity`, `filled_at`)

**Rationale**: Market orders fill immediately in isolated margin. Separate Trade entity added complexity without benefit.

### 5. Detector Architecture: Per-Position Watchers

Detectors are now explicitly **per-position watchers** (not market scanners):
- Position creates detector → Detector monitors → Detector fires single signal → Detector dies
- Each `DetectorSignal` includes `signal_id` for idempotency

### 6. No Insurance Stop on Exchange

The original design had a backup stop on exchange. Removed for simplicity:
- **Old**: Local monitor + exchange stop-limit as backup
- **New**: Robson manages all exits in runtime (no exchange stops)

---

## Phase 0: Bootstrap ✅ COMPLETE

### Delivered

- [x] Rust workspace with 7 crates
- [x] Shared dependencies in workspace `Cargo.toml`
- [x] CLI skeleton (TypeScript/Bun)
- [x] Development tooling (rustfmt, clippy, verify.sh)
- [x] CLAUDE.md context file

### Not Implemented (Deferred)

- [ ] GitHub Actions CI for v2 (Phase 10)
- [ ] Pre-commit hooks integration

---

## Phase 1: Domain Types ✅ COMPLETE

### Delivered

- [x] **Value Objects** (`robson-domain/src/value_objects.rs`)
  - `Price`, `Quantity`, `Symbol`, `Side`, `OrderSide`
  - `RiskConfig` (capital, risk_percent, max drawdown)
  - `TechnicalStopDistance` (renamed from PalmaDaMao)
  - `DomainError` with variants

- [x] **Entities** (`robson-domain/src/entities.rs`)
  - `Position` with state machine
  - `Order` with lifecycle
  - `DetectorSignal` with `signal_id` for idempotency
  - `PositionState` enum (Armed, Entering, Active, Exiting, Closed, Error)
  - Position sizing: `calculate_position_size()`

- [x] **Events** (`robson-domain/src/events.rs`)
  - 12 event types covering full position lifecycle
  - `EntrySignalReceived` (new - for detector integration)
  - JSON serialization/deserialization
  - Event accessors (`position_id()`, `timestamp()`, `event_type()`)

### Tests: 38 passing

### Changes from Original Plan

| Original | Implemented | Reason |
|----------|-------------|--------|
| `PalmaDaMao` | `TechnicalStopDistance` | Clearer naming |
| `Leverage(u8)` | Removed (fixed 10x) | Simplification |
| `stop_gain: Price` | Trailing stop in Active state | Better exit strategy |
| `Trade` entity | Merged into `Order` | Market orders fill immediately |

---

## Phase 2: Engine ✅ COMPLETE

### Delivered

- [x] **Entry Decision** (`Engine::decide_entry`)
  - Validates position is Armed
  - Validates signal matches position (id, symbol, side)
  - Validates tech stop distance (0.1% - 10%)
  - Calculates position size (Golden Rule)
  - Returns `PlaceEntryOrder` action
  - Transitions `Armed → Entering`

- [x] **Entry Fill Processing** (`Engine::process_entry_fill`)
  - Validates position is Entering
  - Calculates initial trailing stop
  - Transitions `Entering → Active`
  - Emits `EntryFilled` event

- [x] **Active Position Processing** (`Engine::process_active_position`)
  - Checks if trailing stop should update (new favorable extreme)
  - Checks if exit should trigger (price hit trailing stop)
  - Returns appropriate actions

- [x] **EngineAction Variants**
  - `PlaceEntryOrder` (new)
  - `UpdateTrailingStop`
  - `TriggerExit`
  - `PlaceExitOrder`
  - `EmitEvent`

### Tests: 21 passing

- Entry tests: 9 (decide_entry, process_entry_fill, validations)
- Exit tests: 11 (trailing stop, trigger exit, edge cases)
- E2E test: 1 (full entry-to-exit flow)

### Changes from Original Plan

| Original | Implemented | Reason |
|----------|-------------|--------|
| `check_stop_loss()` | `process_active_position()` | Combined with trailing stop |
| `check_stop_gain()` | Trailing stop logic | Dynamic targets, not fixed |
| Separate SL/SG monitors | Single trailing stop monitor | Simpler, more effective |

---

## Phase 3: Storage ✅ COMPLETE

### Delivered

- [x] **Repository Traits** (`robson-store/src/repository.rs`)
  - `PositionRepository` (save, find_by_id, find_active, delete)
  - `OrderRepository` (save, find_by_id, find_by_position, find_pending)
  - `EventRepository` (append, find_by_position, get_latest_seq)
  - `Store` trait (combines all repositories)

- [x] **In-Memory Implementation** (`robson-store/src/memory.rs`)
  - `MemoryStore` with RwLock for thread safety
  - Atomic sequence numbers for events
  - All repository traits implemented
  - Helper methods (position_count, clear)

- [x] **Error Types** (`robson-store/src/error.rs`)
  - `StoreError` (NotFound, Duplicate, InvalidState, Database)
  - SQLx error conversion (prepared for PostgreSQL)

### Tests: 14 passing

### Not Implemented (Deferred to Phase 4+)

- [ ] PostgreSQL implementation (can proceed with in-memory for now)
- [ ] Database migrations
- [ ] Lease manager (for leader election)

---

## Phase 4: Execution Layer ⏳ READY

**Goal**: Idempotent order execution with intent journal and port definitions

### 4.1: Port Definitions

Define execution ports in `robson-exec/src/ports.rs`:

```rust
use async_trait::async_trait;
use robson_domain::{Order, OrderId, Price, Quantity, Symbol};

/// Port for exchange operations
#[async_trait]
pub trait ExchangePort: Send + Sync {
    /// Place a market order
    async fn place_market_order(
        &self,
        symbol: &Symbol,
        side: OrderSide,
        quantity: Quantity,
    ) -> Result<OrderResult, ExchangeError>;

    /// Cancel an order
    async fn cancel_order(
        &self,
        order_id: &str,
    ) -> Result<(), ExchangeError>;

    /// Get current price
    async fn get_price(&self, symbol: &Symbol) -> Result<Price, ExchangeError>;
}

/// Port for market data
#[async_trait]
pub trait MarketDataPort: Send + Sync {
    /// Subscribe to price updates
    async fn subscribe(&self, symbol: &Symbol) -> Result<PriceStream, ExchangeError>;
}

/// Result of order execution
pub struct OrderResult {
    pub exchange_order_id: String,
    pub fill_price: Price,
    pub filled_quantity: Quantity,
    pub fee: Decimal,
    pub timestamp: DateTime<Utc>,
}
```

### 4.2: Intent Journal

Implement idempotent execution tracking:

```rust
/// Intent journal for idempotent execution
pub struct IntentJournal {
    store: Arc<dyn Store>,
}

impl IntentJournal {
    /// Record intent before execution
    pub async fn record_intent(&self, intent: &Intent) -> Result<(), ExecError>;

    /// Check if intent already processed
    pub async fn get_intent(&self, intent_id: Uuid) -> Result<Option<Intent>, ExecError>;

    /// Mark intent completed with result
    pub async fn complete_intent(
        &self,
        intent_id: Uuid,
        result: &IntentResult,
    ) -> Result<(), ExecError>;
}

pub struct Intent {
    pub id: Uuid,
    pub position_id: PositionId,
    pub action: IntentAction,
    pub status: IntentStatus,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

pub enum IntentAction {
    PlaceEntryOrder { symbol: Symbol, side: OrderSide, quantity: Quantity },
    PlaceExitOrder { symbol: Symbol, side: OrderSide, quantity: Quantity, reason: ExitReason },
    CancelOrder { order_id: String },
}
```

### 4.3: Executor

Orchestrate engine decisions to exchange actions:

```rust
pub struct Executor {
    exchange: Arc<dyn ExchangePort>,
    journal: IntentJournal,
    store: Arc<dyn Store>,
}

impl Executor {
    /// Execute engine actions with idempotency
    pub async fn execute(&self, actions: Vec<EngineAction>) -> Result<Vec<ExecResult>, ExecError> {
        let mut results = Vec::new();

        for action in actions {
            let result = match action {
                EngineAction::PlaceEntryOrder { position_id, symbol, side, quantity, signal_id } => {
                    self.execute_entry_order(position_id, symbol, side, quantity, signal_id).await?
                }
                EngineAction::PlaceExitOrder { position_id, symbol, side, quantity, reason } => {
                    self.execute_exit_order(position_id, symbol, side, quantity, reason).await?
                }
                EngineAction::EmitEvent(event) => {
                    self.store.events().append(&event).await?;
                    ExecResult::EventEmitted
                }
                // ...
            };
            results.push(result);
        }

        Ok(results)
    }

    async fn execute_entry_order(
        &self,
        position_id: PositionId,
        symbol: Symbol,
        side: OrderSide,
        quantity: Quantity,
        signal_id: Uuid,
    ) -> Result<ExecResult, ExecError> {
        // 1. Check idempotency (signal_id as intent_id)
        if let Some(existing) = self.journal.get_intent(signal_id).await? {
            return Ok(ExecResult::AlreadyProcessed(existing));
        }

        // 2. Record intent
        let intent = Intent {
            id: signal_id,
            position_id,
            action: IntentAction::PlaceEntryOrder { symbol: symbol.clone(), side, quantity },
            status: IntentStatus::Pending,
            created_at: Utc::now(),
            completed_at: None,
        };
        self.journal.record_intent(&intent).await?;

        // 3. Execute on exchange
        let result = self.exchange.place_market_order(&symbol, side, quantity).await?;

        // 4. Mark complete
        self.journal.complete_intent(signal_id, &IntentResult::Success(result.clone())).await?;

        Ok(ExecResult::OrderPlaced(result))
    }
}
```

### 4.4: Stub Exchange Adapter

For testing without real exchange:

```rust
pub struct StubExchange {
    prices: HashMap<Symbol, Price>,
}

#[async_trait]
impl ExchangePort for StubExchange {
    async fn place_market_order(
        &self,
        symbol: &Symbol,
        side: OrderSide,
        quantity: Quantity,
    ) -> Result<OrderResult, ExchangeError> {
        let price = self.prices.get(symbol)
            .ok_or(ExchangeError::SymbolNotFound)?;

        Ok(OrderResult {
            exchange_order_id: Uuid::now_v7().to_string(),
            fill_price: *price,
            filled_quantity: quantity,
            fee: Decimal::ZERO,
            timestamp: Utc::now(),
        })
    }
    // ...
}
```

### Acceptance Criteria

- [ ] Port traits defined (`ExchangePort`, `MarketDataPort`)
- [ ] Intent journal with idempotency
- [ ] Executor orchestrates engine → exchange
- [ ] Stub exchange for testing
- [ ] Tests pass: `cargo test -p robson-exec`

---

## Phase 5: Daemon Runtime

**Goal**: Orchestrate engine, execution, and position lifecycle

### 5.1: Event Bus

Internal channel for detector → engine communication:

```rust
pub struct EventBus {
    sender: broadcast::Sender<DaemonEvent>,
}

pub enum DaemonEvent {
    DetectorSignal(DetectorSignal),
    MarketData(MarketData),
    OrderFill(OrderFill),
}
```

### 5.2: Position Manager

Manages position lifecycle and detector tasks:

```rust
pub struct PositionManager {
    engine: Engine,
    executor: Executor,
    store: Arc<dyn Store>,
    detectors: HashMap<PositionId, JoinHandle<()>>,
}

impl PositionManager {
    /// Arm a new position, spawn detector
    pub async fn arm_position(&mut self, position: Position) -> Result<PositionId, DaemonError>;

    /// Process detector signal
    pub async fn handle_signal(&mut self, signal: DetectorSignal) -> Result<(), DaemonError>;

    /// Process market data for active positions
    pub async fn process_market_data(&mut self, data: MarketData) -> Result<(), DaemonError>;
}
```

### 5.3: Daemon Main Loop

```rust
pub struct Daemon {
    position_manager: PositionManager,
    event_bus: EventBus,
    api_server: ApiServer,
}

impl Daemon {
    pub async fn run(self) -> Result<(), DaemonError> {
        // 1. Load active positions from store
        // 2. Spawn detector tasks for Armed positions
        // 3. Start API server
        // 4. Main event loop
        loop {
            tokio::select! {
                Some(event) = self.event_bus.recv() => {
                    self.handle_event(event).await?;
                }
                _ = tokio::signal::ctrl_c() => {
                    break;
                }
            }
        }
        // 5. Graceful shutdown
        self.shutdown().await
    }
}
```

### Acceptance Criteria

- [ ] Event bus for internal communication
- [ ] Position manager with detector lifecycle
- [ ] Daemon main loop with graceful shutdown
- [ ] API endpoints (health, status, arm, disarm, panic)
- [ ] Can run: `cargo run -p robsond`

---

## Phase 6: CLI Integration

**Goal**: Connect TypeScript CLI to daemon API

### 6.1: API Client

Update existing stub in `cli/src/api/client.ts`:

```typescript
export class RobsonClient {
  constructor(private baseURL: string = 'http://localhost:8080') {}

  async status(): Promise<StatusResponse>;
  async arm(request: ArmRequest): Promise<ArmResponse>;
  async disarm(positionId: string): Promise<void>;
  async panic(options?: PanicOptions): Promise<PanicResponse>;
}
```

### 6.2: Commands

Update command implementations to use real API:
- `robson status` - Show positions table
- `robson arm <symbol>` - Arm new position
- `robson disarm <id>` - Cancel armed position
- `robson panic` - Emergency close all

### Acceptance Criteria

- [ ] CLI connects to daemon
- [ ] Status command shows positions
- [ ] Arm/disarm/panic commands work
- [ ] Error handling and feedback

---

## Phase 7: Detector Interface

**Goal**: Pluggable detector implementation

### 7.1: Detector Trait

```rust
#[async_trait]
pub trait Detector: Send + Sync {
    /// Start monitoring for entry signal
    async fn start(
        &mut self,
        position: &Position,
        market_data: impl Stream<Item = MarketData>,
    ) -> Result<(), DetectorError>;

    /// Stop monitoring (called when position leaves Armed)
    async fn stop(&mut self) -> Result<(), DetectorError>;
}

/// Detector that fires after delay (for testing)
pub struct DelayDetector {
    delay: Duration,
    entry_price: Price,
    stop_loss: Price,
}
```

### 7.2: Integration with Position Manager

```rust
impl PositionManager {
    async fn spawn_detector(&mut self, position: &Position) -> Result<(), DaemonError> {
        let detector = self.create_detector(&position.strategy)?;
        let market_data = self.market_data_port.subscribe(&position.symbol).await?;

        let handle = tokio::spawn(async move {
            if let Some(signal) = detector.run(market_data).await {
                event_bus.send(DaemonEvent::DetectorSignal(signal));
            }
        });

        self.detectors.insert(position.id, handle);
        Ok(())
    }
}
```

### Acceptance Criteria

- [ ] Detector trait defined
- [ ] Stub detector (fires after delay)
- [ ] Integration with position manager
- [ ] Detector lifecycle (spawn on arm, kill on transition)

---

## Phase 8: End-to-End Test

**Goal**: Validate complete workflow with stub components

### Test Scenario

```bash
# 1. Start daemon (with stub exchange)
ROBSON_ENV=test cargo run -p robsond

# 2. Arm position
robson arm BTCUSDT --strategy test --capital 10000

# 3. Watch status (detector fires after 5s)
robson status --watch
# Output: Armed → Entering → Active

# 4. Simulate price movement (stub exchange)
# Price moves up: trailing stop updates
# Price drops to trailing stop: exit triggers

# 5. Verify final state
robson status
# Output: Closed with PnL
```

### Acceptance Criteria

- [ ] Position arms via CLI
- [ ] Detector fires signal (stub)
- [ ] Entry order executes (stub)
- [ ] Position becomes Active
- [ ] Trailing stop updates on favorable movement
- [ ] Exit triggers when stop hit
- [ ] Position closes with PnL
- [ ] All events logged

---

## Phase 9: Real Exchange Connector

**Goal**: Binance isolated margin integration

### 9.1: REST Client

```rust
pub struct BinanceClient {
    http: reqwest::Client,
    api_key: String,
    secret_key: String,
}

impl BinanceClient {
    pub async fn place_market_order(...) -> Result<OrderResponse, BinanceError>;
    pub async fn get_isolated_account(...) -> Result<AccountInfo, BinanceError>;
    pub async fn get_price(&self, symbol: &str) -> Result<Price, BinanceError>;
}
```

### 9.2: WebSocket Client

```rust
pub struct BinanceWebSocket {
    stream: WebSocketStream<...>,
}

impl BinanceWebSocket {
    pub async fn subscribe_ticker(&mut self, symbol: &str) -> Result<(), BinanceError>;
    pub async fn next_event(&mut self) -> Result<MarketEvent, BinanceError>;
}
```

### 9.3: Adapter Implementation

```rust
#[async_trait]
impl ExchangePort for BinanceAdapter {
    async fn place_market_order(...) -> Result<OrderResult, ExchangeError> {
        let response = self.client.place_market_order(...).await?;
        Ok(OrderResult::from(response))
    }
}
```

### Acceptance Criteria

- [ ] REST client with signature
- [ ] WebSocket client with reconnect
- [ ] Adapter implements ExchangePort
- [ ] Tests pass (testnet)

---

## Phase 10: Production Readiness

### 10.1: Observability

- [ ] Structured logging (tracing)
- [ ] Prometheus metrics
- [ ] Health check endpoints

### 10.2: Deployment

- [ ] Docker image
- [ ] Kubernetes manifests
- [ ] ArgoCD integration

### 10.3: Documentation

- [ ] Update DELIVERY-SUMMARY.md
- [ ] API documentation
- [ ] Runbook

---

## Architecture Diagram

```
┌─────────────────────────────────────────────────────────────────────────┐
│                              CLI (Bun/TypeScript)                       │
│                         robson status | arm | panic                     │
└────────────────────────────────────┬────────────────────────────────────┘
                                     │ HTTP
                                     ▼
┌─────────────────────────────────────────────────────────────────────────┐
│                              DAEMON (robsond)                           │
│                                                                         │
│  ┌─────────────┐    ┌─────────────────┐    ┌──────────────────┐        │
│  │  API Server │    │ Position Manager │    │    Event Bus     │        │
│  │   (Axum)    │───▶│                 │◀───│   (broadcast)    │        │
│  └─────────────┘    └────────┬────────┘    └────────▲─────────┘        │
│                              │                       │                  │
│                              ▼                       │                  │
│  ┌──────────────────────────────────────────────────┼─────────────┐    │
│  │                      ENGINE                       │             │    │
│  │  ┌───────────────┐  ┌─────────────────────────┐  │             │    │
│  │  │ decide_entry  │  │ process_active_position │  │             │    │
│  │  └───────────────┘  └─────────────────────────┘  │             │    │
│  └──────────────────────────────────────────────────┼─────────────┘    │
│                              │                       │                  │
│                              ▼                       │                  │
│  ┌──────────────────────────────────────────────────┼─────────────┐    │
│  │                     EXECUTOR                      │             │    │
│  │  ┌───────────────┐  ┌─────────────┐              │             │    │
│  │  │ Intent Journal│  │ Order Exec  │──────────────┼─────────────┼────┼──▶ Exchange
│  │  └───────────────┘  └─────────────┘              │             │    │
│  └──────────────────────────────────────────────────┼─────────────┘    │
│                              │                       │                  │
│  ┌───────────────────────────┼───────────────────────┼─────────────┐    │
│  │                        STORE                      │             │    │
│  │  ┌─────────────┐  ┌───────────┐  ┌────────────┐  │             │    │
│  │  │ Positions   │  │  Orders   │  │   Events   │  │             │    │
│  │  └─────────────┘  └───────────┘  └────────────┘  │             │    │
│  └───────────────────────────────────────────────────┼─────────────┘    │
│                                                      │                  │
│  ┌───────────────────────────────────────────────────┼─────────────┐    │
│  │                 DETECTOR TASKS                    │             │    │
│  │  ┌─────────────────┐  ┌─────────────────┐        │             │    │
│  │  │ Detector(pos_1) │  │ Detector(pos_2) │────────┘             │    │
│  │  └─────────────────┘  └─────────────────┘                      │    │
│  └────────────────────────────────────────────────────────────────┘    │
└─────────────────────────────────────────────────────────────────────────┘
```

---

## Crate Dependencies

```
robson-domain (no external deps)
     ↑
     ├── robson-engine (depends on domain)
     │        ↑
     │        └── robson-exec (depends on engine + domain)
     │                 ↑
     │                 └── robsond (depends on exec + connectors + store)
     │
     ├── robson-connectors (depends on domain)
     │        ↑
     │        └── robsond
     │
     └── robson-store (depends on domain)
              ↑
              └── robsond
```

---

## Verification Commands

```bash
# Full verification
cd v2 && ./scripts/verify.sh

# Quick check (no tests)
./scripts/verify.sh --fast

# Individual crates
cargo test -p robson-domain    # 38 tests
cargo test -p robson-engine    # 21 tests
cargo test -p robson-store     # 14 tests
cargo test -p robson-exec      # Phase 4
cargo test -p robsond          # Phase 5

# All tests
cargo test --all               # 73 tests currently
```

---

## Next Action

**Phase 4**: Implement execution layer ports and intent journal.

Start with:
1. Define `ExchangePort` and `MarketDataPort` traits
2. Implement `IntentJournal` for idempotency
3. Create `Executor` to orchestrate engine → exchange
4. Add `StubExchange` for testing
