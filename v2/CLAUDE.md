# Claude Code Context: Robson v2

**Optimized context for Claude Code AI assistant working on Robson v2 rewrite.**

This document provides essential context for effective code generation, refactoring, and problem-solving in the Robson v2 project.

---

## Quick Context

**Project**: Robson v2 - Rust rewrite of cryptocurrency trading platform
**Architecture**: Hexagonal (Ports & Adapters) + Event-Driven
**Backend**: Rust 1.83 (Tokio async runtime)
**CLI**: Bun + TypeScript (user-facing commands)
**Daemon**: `robsond` (persistent background service)
**Database**: PostgreSQL with SQLx (compile-time checked queries)
**Language Policy**: **100% English** (code, comments, docs)

---

## Critical Rules

### 1. English Only

**ALL code, comments, documentation, and commit messages MUST be in English.**

No exceptions. This is a project-wide policy inherited from the parent Robson repository.

### 2. Hexagonal Architecture in Rust

Backend code follows **Ports & Adapters** pattern:

```
v2/
├── robson-domain/       # Pure domain logic (NO external deps)
│   ├── entities.rs      # Business entities
│   ├── value_objects.rs # Immutable values
│   └── errors.rs        # Domain errors
│
├── robson-engine/       # Decision engine (core business logic)
│   ├── decision.rs      # Decision algorithms
│   ├── risk.rs          # Risk calculations
│   └── portfolio.rs     # Portfolio management
│
├── robson-exec/         # Execution layer (outbound ports)
│   ├── ports.rs         # Port trait definitions
│   └── executor.rs      # Execution orchestration
│
├── robson-connectors/   # Exchange adapters (inbound/outbound)
│   ├── binance.rs       # Binance adapter
│   └── http.rs          # HTTP client
│
├── robson-store/        # PostgreSQL persistence
│   ├── repositories.rs  # Repository implementations
│   └── migrations/      # SQLx migrations
│
├── robsond/             # Runtime daemon (inbound ports)
│   ├── main.rs          # Entry point
│   ├── api.rs           # HTTP API (Axum)
│   └── config.rs        # Configuration
│
└── robson-sim/          # Backtesting/simulation
    ├── simulator.rs     # Simulation engine
    └── metrics.rs       # Performance metrics
```

**Key Principle**: `robson-domain` has **ZERO external dependencies** (only std/serde/rust_decimal).

### 3. Type Safety Everywhere

Always use Rust's type system for correctness:

```rust
// ✅ Good: Type-safe domain types
use rust_decimal::Decimal;
use uuid::Uuid;

pub struct Order {
    pub id: Uuid,
    pub symbol: Symbol,
    pub quantity: Quantity,
    pub price: Price,
}

pub struct Quantity(Decimal);  // Newtype pattern
pub struct Price(Decimal);

// ❌ Bad: Raw primitives
pub struct Order {
    pub id: String,
    pub symbol: String,
    pub quantity: f64,  // NO! Financial amounts need precision
    pub price: f64,
}
```

### 4. Error Handling: No Unwrap/Expect in Production Code

Use `Result` and `?` operator. Only use `unwrap()` in tests or when provably safe:

```rust
// ✅ Good: Propagate errors
pub fn calculate_position_size(
    capital: Decimal,
    risk_percent: Decimal,
    stop_distance: Decimal,
) -> Result<Decimal, DomainError> {
    if stop_distance.is_zero() {
        return Err(DomainError::InvalidStopDistance);
    }
    Ok(capital * risk_percent / stop_distance)
}

// ❌ Bad: Panic on error
pub fn calculate_position_size(capital: Decimal, stop_distance: Decimal) -> Decimal {
    capital / stop_distance  // Panics if stop_distance is 0!
}
```

### 5. Async/Await Best Practices

Use Tokio idioms properly:

```rust
// ✅ Good: Explicit async with proper error handling
use tokio::time::{sleep, Duration};

pub async fn fetch_price(symbol: &str) -> Result<Decimal, ApiError> {
    let client = reqwest::Client::new();
    let response = client
        .get(format!("https://api.binance.com/api/v3/ticker/price?symbol={}", symbol))
        .timeout(Duration::from_secs(5))
        .send()
        .await?;

    let data: PriceResponse = response.json().await?;
    Ok(data.price)
}

// ❌ Bad: Blocking calls in async context
pub async fn fetch_price(symbol: &str) -> Decimal {
    let response = std::thread::sleep(Duration::from_secs(1)); // Blocks entire runtime!
    // ...
}
```

### 6. Test-Driven Development

Write tests for all business logic:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_position_size_calculation() {
        // Arrange
        let capital = dec!(10000);
        let risk_percent = dec!(0.01);  // 1%
        let stop_distance = dec!(1500);

        // Act
        let result = calculate_position_size(capital, risk_percent, stop_distance);

        // Assert
        assert_eq!(result.unwrap(), dec!(0.0667));  // OK in tests
    }

    #[test]
    fn test_zero_stop_distance_returns_error() {
        let result = calculate_position_size(dec!(10000), dec!(0.01), dec!(0));
        assert!(result.is_err());
    }
}
```

### 7. Conventional Commits

```
<type>(<scope>): <subject>

<body>

<footer>
```

**Types**: `feat`, `fix`, `docs`, `refactor`, `test`, `chore`, `perf`
**Scopes**: `domain`, `engine`, `exec`, `connectors`, `store`, `cli`, `daemon`

**Examples**:
```
feat(domain): add Position entity with lifecycle states
fix(connectors): handle Binance rate limit errors gracefully
docs(engine): document risk calculation algorithm
refactor(store): migrate to SQLx compile-time checked queries
test(exec): add integration tests for order execution
```

---

## Project Structure Deep Dive

### Crate Dependencies (Directed Acyclic Graph)

```
robson-domain (no deps)
    ↑
    ├─── robson-engine (depends on domain)
    │       ↑
    │       ├─── robson-exec (depends on engine)
    │       │       ↑
    │       │       └─── robsond (depends on exec, connectors, store)
    │       │
    │       └─── robson-sim (depends on engine, exec)
    │
    ├─── robson-connectors (depends on domain)
    │       ↑
    │       └─── robsond
    │
    └─── robson-store (depends on domain)
            ↑
            └─── robsond
```

### Workspace Configuration

Located at `v2/Cargo.toml`:

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

[workspace.dependencies]
tokio = { version = "1.42", features = ["full"] }
serde = { version = "1.0", features = ["derive"] }
rust_decimal = "1.37"
sqlx = { version = "0.8", features = ["postgres", "runtime-tokio", "uuid"] }
# ... more shared deps
```

### CLI Structure (Bun + TypeScript)

Located at `v2/cli/`:

```
cli/
├── package.json       # Bun scripts
├── tsconfig.json      # TypeScript config (strict mode)
├── src/
│   ├── index.ts       # Entry point (Commander.js)
│   ├── commands/      # Command implementations
│   │   ├── arm.ts     # Arm strategy for trading
│   │   ├── disarm.ts  # Disarm strategy
│   │   ├── panic.ts   # Emergency stop
│   │   └── status.ts  # Portfolio status
│   ├── api/
│   │   └── client.ts  # HTTP client for robsond API
│   └── types/
│       └── index.ts   # TypeScript type definitions
└── README.md
```

---

## Development Workflow

### Setup (First Time)

```bash
# 1. Install Rust toolchain
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
rustup default stable

# 2. Install Bun (for CLI)
curl -fsSL https://bun.sh/install | bash

# 3. Install SQLx CLI (for database migrations)
cargo install sqlx-cli --no-default-features --features postgres

# 4. Clone and navigate to v2/
cd /path/to/robson/v2

# 5. Build Rust workspace
cargo build

# 6. Install CLI dependencies
cd cli && bun install && cd ..

# 7. Run verification
./scripts/verify.sh  # (will be created)
```

### Daily Development Loop

```bash
# 1. Start with a clean slate
git checkout main
git pull origin main
git checkout -b feature/my-feature

# 2. Make changes (small, incremental)
# ... edit code ...

# 3. Verify locally (BEFORE committing)
cargo fmt --all                        # Format Rust code
cargo clippy --all-targets -- -D warnings  # Lint with Clippy
cargo test --all                       # Run all tests

cd cli && bun test && cd ..           # Test CLI (if tests exist)

# 4. Commit (Conventional Commits)
git add .
git commit -m "feat(domain): add Order entity"

# 5. Push and create PR
git push origin feature/my-feature
gh pr create --title "feat: add Order entity"
```

### Verification Commands

| Command | Purpose | When to Run |
|---------|---------|-------------|
| `cargo fmt --all --check` | Check Rust formatting | Pre-commit, CI |
| `cargo clippy --all-targets -- -D warnings` | Lint Rust code | Pre-commit, CI |
| `cargo test --all` | Run all Rust tests | Pre-commit, CI |
| `cargo build --release` | Build optimized binaries | Before deploy |
| `cd cli && bun test` | Run CLI tests | Pre-commit |
| `./scripts/verify.sh` | Full verification | Before push |

---

## Definition of Done

Before marking a task as complete, ensure:

- [ ] **Code compiles** without warnings (`cargo build`)
- [ ] **Formatted** with `cargo fmt --all`
- [ ] **Linted** with `cargo clippy -- -D warnings`
- [ ] **Tests pass** (`cargo test --all`)
- [ ] **Tests written** for new business logic (aim for >80% coverage on domain/engine)
- [ ] **Docs updated** (inline `///` doc comments + README if needed)
- [ ] **No `unwrap()` or `expect()` in production code** (only in tests)
- [ ] **Commit message follows Conventional Commits**
- [ ] **English only** (no Portuguese in code/comments/docs)

---

## How Claude Code Should Work Here

### General Principles

1. **Small, incremental changes**: One feature/fix per commit
2. **Read before write**: Always read existing code before modifying
3. **Validate constantly**: Run tests after every change
4. **Ask when uncertain**: Use questions to clarify requirements
5. **Document decisions**: Update docs when changing behavior
6. **Safety first**: Prefer compile-time errors over runtime panics

### Specific Guidelines for Robson v2

1. **Respect crate boundaries**:
   - Domain crate stays pure (no Tokio, no SQLx, no HTTP)
   - Engine crate only depends on domain
   - Adapters in connectors/store implement ports from exec

2. **Financial precision**:
   - Always use `rust_decimal::Decimal` for money/quantities
   - Never use `f64` for financial calculations
   - Use `rust_decimal_macros::dec!` for decimal literals in tests

3. **Error handling**:
   - Create domain-specific error types (e.g., `DomainError`, `EngineError`)
   - Use `thiserror` for error boilerplate
   - Propagate errors with `?` operator
   - Only `unwrap()` in tests or when provably safe with comment

4. **Testing strategy**:
   - Unit tests in same file as implementation (`#[cfg(test)]`)
   - Integration tests in `tests/` directory
   - Use `#[tokio::test]` for async tests
   - Use `rust_decimal_macros::dec!` for test fixtures

5. **Database interactions**:
   - Use SQLx compile-time checked queries (`query_as!`)
   - Store migrations in `robson-store/migrations/`
   - Use UUID v7 for entity IDs (time-ordered)
   - Always use transactions for multi-step operations

6. **When to use each crate**:
   - `robson-domain`: Entities, value objects, domain errors
   - `robson-engine`: Business logic, calculations, decision algorithms
   - `robson-exec`: Port definitions, execution orchestration
   - `robson-connectors`: Exchange API clients (Binance, etc.)
   - `robson-store`: PostgreSQL repositories, migrations
   - `robsond`: HTTP API, configuration, runtime
   - `robson-sim`: Backtesting, simulation, performance metrics

---

## Common Commands

### Rust Development

```bash
# Format code
cargo fmt --all

# Check formatting (CI-friendly)
cargo fmt --all --check

# Lint with Clippy (strict)
cargo clippy --all-targets -- -D warnings

# Run tests
cargo test --all

# Run specific test
cargo test test_position_size_calculation

# Build in release mode
cargo build --release

# Check code without building
cargo check --all-targets

# Update dependencies
cargo update

# Audit dependencies for security issues
cargo audit
```

### CLI Development (Bun/TypeScript)

```bash
# Install dependencies
cd cli && bun install

# Run CLI in development mode
bun run dev

# Build CLI
bun run build

# Run tests
bun test

# Type check
bun run tsc --noEmit
```

### Database (SQLx)

```bash
# Create new migration
sqlx migrate add create_orders_table

# Run migrations
sqlx migrate run

# Revert last migration
sqlx migrate revert

# Check database is up-to-date
sqlx migrate info
```

---

## Code Patterns

### Domain Entity

```rust
// robson-domain/src/entities.rs
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Order {
    pub id: Uuid,
    pub symbol: Symbol,
    pub side: OrderSide,
    pub quantity: Quantity,
    pub price: Price,
    pub status: OrderStatus,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl Order {
    pub fn new(symbol: Symbol, side: OrderSide, quantity: Quantity, price: Price) -> Self {
        Self {
            id: Uuid::now_v7(),  // Time-ordered UUIDs
            symbol,
            side,
            quantity,
            price,
            status: OrderStatus::Pending,
            created_at: chrono::Utc::now(),
        }
    }

    pub fn execute(&mut self) -> Result<(), DomainError> {
        match self.status {
            OrderStatus::Pending => {
                self.status = OrderStatus::Executed;
                Ok(())
            }
            _ => Err(DomainError::InvalidOrderState),
        }
    }
}
```

### Use Case (Engine)

```rust
// robson-engine/src/use_cases.rs
use robson_domain::{Order, DomainError};

pub struct CreateOrderUseCase<R: OrderRepository> {
    repository: R,
}

impl<R: OrderRepository> CreateOrderUseCase<R> {
    pub fn new(repository: R) -> Self {
        Self { repository }
    }

    pub async fn execute(&self, command: CreateOrderCommand) -> Result<Order, DomainError> {
        // 1. Validate
        command.validate()?;

        // 2. Create domain entity
        let order = Order::new(
            command.symbol,
            command.side,
            command.quantity,
            command.price,
        );

        // 3. Persist
        self.repository.save(&order).await?;

        // 4. Return
        Ok(order)
    }
}
```

### Port Definition (Exec)

```rust
// robson-exec/src/ports.rs
use async_trait::async_trait;
use robson_domain::{Order, DomainError};

#[async_trait]
pub trait OrderRepository {
    async fn save(&self, order: &Order) -> Result<(), DomainError>;
    async fn find_by_id(&self, id: Uuid) -> Result<Option<Order>, DomainError>;
    async fn list_active(&self) -> Result<Vec<Order>, DomainError>;
}

#[async_trait]
pub trait ExchangeClient {
    async fn place_order(&self, order: &Order) -> Result<String, DomainError>;
    async fn cancel_order(&self, exchange_id: &str) -> Result<(), DomainError>;
    async fn get_price(&self, symbol: &str) -> Result<Decimal, DomainError>;
}
```

### Adapter Implementation (Connectors)

```rust
// robson-connectors/src/binance.rs
use async_trait::async_trait;
use robson_domain::DomainError;
use robson_exec::ExchangeClient;

pub struct BinanceAdapter {
    client: reqwest::Client,
    api_key: String,
}

#[async_trait]
impl ExchangeClient for BinanceAdapter {
    async fn place_order(&self, order: &Order) -> Result<String, DomainError> {
        let response = self.client
            .post("https://api.binance.com/api/v3/order")
            .header("X-MBX-APIKEY", &self.api_key)
            .json(&order)
            .send()
            .await
            .map_err(|e| DomainError::ExchangeError(e.to_string()))?;

        let data: BinanceOrderResponse = response.json().await
            .map_err(|e| DomainError::ExchangeError(e.to_string()))?;

        Ok(data.order_id)
    }
}
```

### HTTP API (Daemon)

```rust
// robsond/src/api.rs
use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::post,
    Json, Router,
};

pub fn create_router(state: AppState) -> Router {
    Router::new()
        .route("/api/orders", post(create_order))
        .with_state(state)
}

async fn create_order(
    State(state): State<AppState>,
    Json(payload): Json<CreateOrderRequest>,
) -> impl IntoResponse {
    let command = CreateOrderCommand {
        symbol: payload.symbol,
        side: payload.side,
        quantity: payload.quantity,
        price: payload.price,
    };

    match state.create_order_use_case.execute(command).await {
        Ok(order) => (StatusCode::CREATED, Json(order)),
        Err(e) => (StatusCode::BAD_REQUEST, Json(e.to_string())),
    }
}
```

---

## Testing Patterns

### Unit Test (Domain Logic)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_order_creation() {
        let order = Order::new(
            Symbol::from("BTCUSDT"),
            OrderSide::Buy,
            Quantity::new(dec!(0.1)),
            Price::new(dec!(50000)),
        );

        assert_eq!(order.status, OrderStatus::Pending);
        assert_eq!(order.quantity.value(), dec!(0.1));
    }

    #[test]
    fn test_order_execution() {
        let mut order = Order::new(/* ... */);
        let result = order.execute();

        assert!(result.is_ok());
        assert_eq!(order.status, OrderStatus::Executed);
    }

    #[test]
    fn test_invalid_state_transition() {
        let mut order = Order::new(/* ... */);
        order.execute().unwrap();  // OK in tests

        let result = order.execute();  // Try to execute again
        assert!(result.is_err());
    }
}
```

### Async Test (Use Case)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_create_order_use_case() {
        // Arrange
        let mock_repo = MockOrderRepository::new();
        let use_case = CreateOrderUseCase::new(mock_repo);
        let command = CreateOrderCommand { /* ... */ };

        // Act
        let result = use_case.execute(command).await;

        // Assert
        assert!(result.is_ok());
    }
}
```

---

## Troubleshooting

### Common Issues

**Issue**: `cargo build` fails with "cannot find type `Decimal` in this scope"
**Solution**: Add `use rust_decimal::Decimal;` at the top of the file

**Issue**: Tests fail with "PanicException: attempt to divide by zero"
**Solution**: Add validation to check for zero before division, return `Result<T, DomainError>`

**Issue**: SQLx compile error "query was not checked against the database"
**Solution**: Run `sqlx migrate run` to apply migrations, then `cargo sqlx prepare`

**Issue**: Clippy warnings about `unwrap()` in production code
**Solution**: Replace `unwrap()` with `?` operator and return `Result`

---

## Key Dependencies

**Rust Crates**:
- `tokio` 1.42 - Async runtime
- `serde` 1.0 - Serialization
- `rust_decimal` 1.37 - Financial precision
- `sqlx` 0.8 - Database with compile-time checks
- `axum` 0.8 - HTTP server framework
- `reqwest` 0.12 - HTTP client
- `uuid` 1.11 - UUID v7 (time-ordered)
- `chrono` 0.4 - Date/time handling
- `tracing` 0.1 - Structured logging
- `thiserror` 2.0 - Error boilerplate

**CLI Dependencies** (Bun):
- `commander` - CLI framework
- `axios` - HTTP client
- `chalk` - Terminal colors
- `cli-table3` - Table formatting

---

## When to Reference Full Documentation

For comprehensive context:
- **[../docs/v2/ARCHITECTURE.md](../docs/v2/ARCHITECTURE.md)** - System design
- **[../docs/v2/DOMAIN.md](../docs/v2/DOMAIN.md)** - Domain model
- **[../docs/v2/RELIABILITY.md](../docs/v2/RELIABILITY.md)** - HA/failover
- **[../docs/v2/CLI.md](../docs/v2/CLI.md)** - Command reference
- **[../docs/v2/EXECUTION-PLAN.md](../docs/v2/EXECUTION-PLAN.md)** - Implementation roadmap

---

## Architecture Decisions

**Why Rust for v2?**
- Performance (microsecond latency for decision engine)
- Safety (no null pointers, no data races)
- Financial precision (`rust_decimal` vs floating-point)
- Compile-time guarantees (SQLx query checking)

**Why separate CLI (Bun) from daemon (Rust)?**
- CLI needs rapid iteration, developer-friendly syntax (TypeScript)
- Daemon needs reliability, performance (Rust)
- CLI communicates with daemon via HTTP API

**Why Hexagonal Architecture?**
- Testability (mock adapters easily)
- Framework independence (switch from Binance to another exchange)
- Domain-driven design (business logic stays pure)

---

## Quick Reference: File Locations

| Task | File Path |
|------|-----------|
| Domain entity | `v2/robson-domain/src/entities.rs` |
| Value object | `v2/robson-domain/src/value_objects.rs` |
| Domain error | `v2/robson-domain/src/errors.rs` |
| Use case | `v2/robson-engine/src/use_cases.rs` |
| Port definition | `v2/robson-exec/src/ports.rs` |
| Binance adapter | `v2/robson-connectors/src/binance.rs` |
| Repository | `v2/robson-store/src/repositories.rs` |
| HTTP API | `v2/robsond/src/api.rs` |
| CLI command | `v2/cli/src/commands/*.ts` |
| Migration | `v2/robson-store/migrations/*.sql` |
| Tests | `v2/*/tests/*.rs` or `v2/*/src/lib.rs` (inline) |

---

**Version**: 1.0
**Last Updated**: 2026-01-15
**Repository**: https://github.com/ldamasio/robson (v2 branch)

---

**Remember**: Small, incremental, safe changes. Always validate. Always test. Always document. English only.
