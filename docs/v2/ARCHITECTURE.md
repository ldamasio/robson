# Robson v2 Architecture

**Version**: 2.0.0-alpha
**Last Updated**: 2026-01-12
**Status**: Planning Phase

---

## Executive Summary

Robson v2 is a complete rewrite focused on **reliability, safety, and operational excellence** for isolated margin trading operations. The core is built in **Rust** for performance, safety, and correctness guarantees, with a **Bun/TypeScript CLI** for developer experience.

### Key Principles

1. **User Never Closes Manually**: All exits (SL/SG) are automated market orders
2. **"Palma da Mão" is Universal**: Technical stop distance is the structural foundation
3. **Safety by Default**: Position sizing and risk management are always enforced
4. **High Availability**: Survive pod/node failures, network issues, restarts
5. **Deterministic Core**: Pure, testable engine with clear I/O boundaries
6. **Idempotent Execution**: Safe retries, no duplicate orders
7. **Source of Truth**: Exchange state always wins in reconciliation

---

## System Overview

```
┌─────────────────────────────────────────────────────────────┐
│                     CLI (Bun/TypeScript)                     │
│  robson arm/disarm/status/panic                             │
└───────────────────────┬─────────────────────────────────────┘
                        │ HTTP/gRPC (localhost)
┌───────────────────────▼─────────────────────────────────────┐
│                      robsond (Rust)                          │
│  ┌─────────────┐  ┌──────────────┐  ┌──────────────────┐  │
│  │   API       │  │   Runtime    │  │  Reconciliation  │  │
│  │   Server    │  │   Loop       │  │  Engine          │  │
│  └─────────────┘  └──────────────┘  └──────────────────┘  │
│         │                │                    │             │
│  ┌──────▼────────────────▼────────────────────▼─────────┐  │
│  │            robson-engine (Pure Logic)                 │  │
│  │  State Machine | Decision Making | Invariants        │  │
│  └───────────────────────────┬───────────────────────────┘  │
│                              │                               │
│  ┌───────────────────────────▼───────────────────────────┐  │
│  │           robson-exec (Execution Layer)               │  │
│  │  Intent Journal | Order Manager | Idempotency        │  │
│  └───────┬─────────────────────────────────┬─────────────┘  │
│          │                                 │                 │
│  ┌───────▼──────────┐             ┌───────▼──────────────┐  │
│  │ robson-connectors│             │   robson-store       │  │
│  │ Exchange Adapter │             │   Postgres/Journal   │  │
│  └──────────────────┘             └──────────────────────┘  │
└─────────────────────────────────────────────────────────────┘
         │                                    │
         │ REST/WebSocket                     │ SQL/Event Log
         ▼                                    ▼
┌─────────────────┐                  ┌──────────────────┐
│   Exchange      │                  │   PostgreSQL     │
│   (Binance)     │                  │   + Locks/Lease  │
└─────────────────┘                  └──────────────────┘
```

---

## Component Architecture

### Rust Crates

#### 1. `robson-domain`
**Pure domain logic, zero dependencies**

- **Entities**: `Position`, `Order`, `Trade`, `RiskParams`, `PalmaDaMao`
- **Value Objects**: `Price`, `Quantity`, `Symbol`, `Side`, `Leverage`
- **Invariants**: Position state transitions, risk limits, sizing rules
- **State Machine**: `Armed → Entering → Active → Exiting → Closed → Error`

**No I/O, no side effects, 100% testable**

```rust
pub struct Position {
    pub id: PositionId,
    pub symbol: Symbol,
    pub side: Side,
    pub state: PositionState,
    pub palma: PalmaDaMao,
    pub entry_price: Price,
    pub stop_loss: Price,
    pub stop_gain: Price,
    pub quantity: Quantity,
    pub leverage: Leverage,
}

pub enum PositionState {
    Armed { detector_config: DetectorConfig },
    Entering { entry_order: OrderId },
    Active { monitor_active: bool },
    Exiting { exit_reason: ExitReason },
    Closed { pnl: PnL },
    Error { error: DomainError },
}

pub struct PalmaDaMao {
    pub distance: Decimal,      // |entry - stop_loss|
    pub distance_pct: Decimal,  // distance / entry_price
}
```

#### 2. `robson-engine`
**Pure decision engine, deterministic**

- **Input**: `EngineInput` (market data, position state, intents)
- **Output**: `EngineAction` (place order, cancel order, update state, emit event)
- **Logic**: State transitions, entry/exit decisions, risk checks
- **No I/O**: Returns actions to be executed by runtime

```rust
pub struct Engine {
    risk_config: RiskConfig,
}

impl Engine {
    pub fn decide(&self, input: EngineInput) -> EngineDecision {
        // Pure function: same input → same output
        // Returns actions + new state
    }
}

pub enum EngineAction {
    PlaceOrder(OrderIntent),
    CancelOrder(OrderId),
    UpdatePosition(Position),
    EmitEvent(Event),
    Reconcile(ReconcileIntent),
}
```

#### 3. `robson-exec`
**Execution layer with idempotency**

- **Intent Journal**: Write-ahead log for all actions
- **Order Manager**: Correlates orders → fills → trades
- **Idempotency**: Dedupe via intent IDs, safe retries
- **Fill Reconciliation**: Match exchange events to intents

```rust
pub struct ExecutionEngine {
    journal: IntentJournal,
    order_manager: OrderManager,
    connector: Box<dyn ExchangeConnector>,
}

impl ExecutionEngine {
    pub async fn execute(&mut self, action: EngineAction) -> ExecResult {
        // 1. Write intent to journal (WAL)
        // 2. Execute on exchange
        // 3. Correlate response
        // 4. Mark intent as completed
        // 5. Emit reconciliation events
    }
}
```

#### 4. `robson-connectors`
**Exchange adapters (REST + WebSocket)**

- **Normalization**: Exchange-specific → Domain types
- **Rate Limiting**: Built-in backoff/retry
- **Event Stream**: Unified market data + order updates
- **Testability**: `SimExchange` for tests

```rust
#[async_trait]
pub trait ExchangeConnector: Send + Sync {
    async fn place_order(&self, intent: OrderIntent) -> Result<OrderResponse>;
    async fn cancel_order(&self, order_id: OrderId) -> Result<CancelResponse>;
    async fn get_position(&self, symbol: Symbol) -> Result<PositionSnapshot>;
    async fn subscribe_market_data(&self, symbol: Symbol) -> EventStream;
    async fn subscribe_user_data(&self) -> EventStream;
}

pub struct BinanceConnector { /* ... */ }
pub struct SimExchange { /* ... */ }
```

#### 5. `robson-store`
**Persistence and event sourcing**

- **Event Log**: Immutable append-only journal
- **Snapshots**: Periodic position state snapshots
- **Locks/Leases**: Leader election per (account, symbol)
- **Reconciliation**: Rebuild state from events

```rust
pub struct Store {
    pool: PgPool,
}

impl Store {
    pub async fn append_event(&self, event: Event) -> Result<EventId>;
    pub async fn load_events(&self, position_id: PositionId) -> Result<Vec<Event>>;
    pub async fn save_snapshot(&self, snapshot: PositionSnapshot) -> Result<()>;
    pub async fn acquire_lease(&self, key: LeaseKey, ttl: Duration) -> Result<Lease>;
}
```

#### 6. `robsond`
**Runtime daemon and orchestration**

- **Main Loop**: Drives engine, executes actions, monitors positions
- **API Server**: HTTP/gRPC for CLI commands
- **Health Checks**: Liveness/readiness probes
- **Leader Election**: Acquire lease before starting
- **Reconciliation**: On startup, verify state vs exchange
- **Graceful Shutdown**: Cancel orders, release lease

```rust
pub struct Daemon {
    engine: Engine,
    exec: ExecutionEngine,
    store: Store,
    api_server: ApiServer,
    lease_manager: LeaseManager,
}

impl Daemon {
    pub async fn run(&mut self) -> Result<()> {
        // 1. Acquire lease (leader election)
        // 2. Reconcile state
        // 3. Start API server
        // 4. Main loop: poll market data, execute engine, handle events
        // 5. Graceful shutdown on SIGTERM
    }
}
```

#### 7. `robson-sim`
**Simulator and backtesting**

- **Replay Events**: Replay historical market data
- **Deterministic Testing**: Verify engine behavior
- **Performance Analysis**: PnL, drawdown, win rate

---

## Data Flow

### Happy Path: Arm → Entry → Active → Exit

```
1. CLI: robson arm BTCUSDT --strategy all-in
   ↓
2. robsond API: POST /arm
   ↓
3. Engine: Armed state, waiting for detector signal
   ↓
4. Detector (stub): Emits entry signal (long/short + entry price)
   ↓
5. Engine: Calculates stop loss (technical), palma, position size
   ↓
6. Engine: Emits PlaceOrder(market entry order)
   ↓
7. ExecutionEngine: Journals intent → Places order → Correlates fill
   ↓
8. Engine: Position state → Active, starts SL/SG monitor
   ↓
9. Monitor Loop: Checks price vs SL/SG every tick
   ↓
10. Trigger: Price hits SL
    ↓
11. Engine: Emits PlaceOrder(market exit order)
    ↓
12. ExecutionEngine: Journals intent → Places order → Correlates fill
    ↓
13. Engine: Position state → Closed, calculates PnL
    ↓
14. CLI: robson status → Shows closed position + PnL
```

### Failure Scenarios

#### Pod Restart Mid-Trade

```
1. robsond crashes/restarts
   ↓
2. On startup: Attempt to acquire lease
   ↓
3. If lease acquired: Load position snapshot + events
   ↓
4. Reconcile with exchange: Query open orders + position
   ↓
5. Detect state mismatch (e.g., price already passed SL)
   ↓
6. Enter degraded mode: Close position immediately (market order)
   ↓
7. Resume normal operation
```

#### Network Partition

```
1. WebSocket disconnects
   ↓
2. Lease expires (TTL timeout)
   ↓
3. Another pod acquires lease (failover)
   ↓
4. Original pod: Lease check fails → Graceful shutdown
   ↓
5. New leader: Reconciles and resumes
```

#### Duplicate Order Protection

```
1. Engine emits PlaceOrder action
   ↓
2. ExecutionEngine: Write intent to journal with unique intent_id
   ↓
3. Place order on exchange
   ↓
4. Network timeout (no response)
   ↓
5. Retry: Check journal → Intent already exists → Query exchange for order
   ↓
6. If order exists: Correlate and mark complete
   ↓
7. If order missing: Safe to retry placement
```

---

## State Machine

```
                    ┌─────────┐
                    │  Armed  │  (Waiting for detector signal)
                    └────┬────┘
                         │ detector emits entry signal
                         ▼
                   ┌──────────┐
                   │ Entering │  (Market order placed)
                   └─────┬────┘
                         │ fill confirmed
                         ▼
                    ┌─────────┐
                    │  Active │  (Monitoring SL/SG)
                    └────┬────┘
                         │ SL/SG trigger OR user panic
                         ▼
                   ┌──────────┐
                   │ Exiting  │  (Market close order placed)
                   └─────┬────┘
                         │ fill confirmed
                         ▼
                    ┌─────────┐
                    │ Closed  │  (PnL calculated)
                    └─────────┘

                         │ error at any stage
                         ▼
                    ┌─────────┐
                    │  Error  │  (Manual intervention needed)
                    └─────────┘
```

---

## Technology Stack

| Component | Technology | Rationale |
|-----------|-----------|-----------|
| Core Engine | Rust | Safety, performance, correctness guarantees |
| CLI | Bun + TypeScript | Fast runtime, great DX, JSON output |
| Storage | PostgreSQL | ACID, advisory locks, event sourcing |
| Exchange | Binance API | Market leader, good API |
| Deployment | k3s (Kubernetes) | HA, pod management, health checks |
| IPC | HTTP/gRPC (localhost) | CLI ↔ daemon communication |
| Observability | tracing + JSON logs | Structured logging for debugging |

---

## Design Decisions

### Why Rust for Core?

- **Memory Safety**: No segfaults, no data races
- **Type Safety**: Strong guarantees at compile time
- **Performance**: Low latency, predictable performance
- **Ecosystem**: Excellent async, serialization, decimal math
- **Testability**: Pure functions, no global state

### Why Separate Engine from Execution?

- **Testability**: Engine is pure, no mocks needed
- **Determinism**: Same input → same output, always
- **Replay**: Can replay events and verify decisions
- **Safety**: Engine never touches I/O directly

### Why Event Sourcing?

- **Audit Trail**: Full history of all decisions
- **Reconciliation**: Rebuild state from events
- **Debugging**: Replay failure scenarios
- **Compliance**: Regulatory requirements

### Why Leader Election?

- **Split-Brain Prevention**: Only one active trader per (account, symbol)
- **High Availability**: Automatic failover
- **Safety**: No duplicate orders, no conflicting decisions

---

## Next Steps

See:
- [RELIABILITY.md](./RELIABILITY.md) - HA, failover, reconciliation
- [DOMAIN.md](./DOMAIN.md) - Domain model, invariants, state machine
- [CLI.md](./CLI.md) - CLI commands, examples, JSON output
- [EXECUTION-PLAN.md](./EXECUTION-PLAN.md) - Implementation roadmap
- [PROMPT-PACK.md](./PROMPT-PACK.md) - Agentic coding prompts

---

**Status**: Ready for implementation
**Review**: Pending approval from product/engineering
