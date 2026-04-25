# Phase 9-10: V2 Production Readiness - Execution Plan

## Overview

This document provides step-by-step implementation guidance for bringing Robson v2 to production, resolving the coordination between Core Trading and Safety Net modalities, and completing the Binance connector integration.

**Target Completion**: 3-4 weeks  
**Related ADR**: [ADR-0014](../adr/ADR-0014-safety-net-core-trading-coordination.md)

---

## Part 1: Resolve Core vs Safety Net Conflicts (Priority: CRITICAL)

### Task 1.1: Add Core Position Repository to Safety Net

**File**: `v2/robsond/src/position_monitor.rs`

**Objective**: Enable Safety Net to query Core Trading positions.

**Steps:**

1. Add dependency in struct:

```rust
use robson_store::PositionRepository;

pub struct PositionMonitor {
    binance_client: Arc<BinanceRestClient>,
    execution_attempt_repo: Arc<dyn ExecutionAttemptRepository>,
    core_position_repo: Arc<dyn PositionRepository>,  // NEW
    detected_positions: DashMap<(Symbol, Side), DetectedPosition>,
    poll_interval: Duration,
    stop_signal: Arc<AtomicBool>,
}
```

2. Update constructor:

```rust
impl PositionMonitor {
    pub fn new(
        binance_client: Arc<BinanceRestClient>,
        execution_attempt_repo: Arc<dyn ExecutionAttemptRepository>,
        core_position_repo: Arc<dyn PositionRepository>,  // NEW
    ) -> Self {
        Self {
            binance_client,
            execution_attempt_repo,
            core_position_repo,  // NEW
            detected_positions: DashMap::new(),
            poll_interval: Duration::from_secs(20),
            stop_signal: Arc::new(AtomicBool::new(false)),
        }
    }
}
```

3. Update wire-up in `v2/robsond/src/main.rs`:

```rust
let position_monitor = PositionMonitor::new(
    binance_client.clone(),
    execution_attempt_repo.clone(),
    position_repo.clone(),  // Pass core position repo
);
```

**Validation**: Code compiles with no errors.

---

### Task 1.2: Implement Exclusion Filter

**File**: `v2/robsond/src/position_monitor.rs`

**Objective**: Add method to check if position is Core-managed.

**Steps:**

1. Add method to `PositionMonitor`:

```rust
impl PositionMonitor {
    /// Check if a position is managed by Core Trading.
    /// 
    /// Returns true if there's an active Core position for this (symbol, side).
    async fn is_core_managed(&self, symbol: &Symbol, side: Side) -> Result<bool, anyhow::Error> {
        self.core_position_repo
            .find_active_by_symbol_and_side(symbol, side)
            .await
            .map(|opt| opt.is_some())
    }
}
```

2. Update `find_active_by_symbol_and_side()` in `v2/robson-store/src/repository.rs`:

```rust
#[async_trait]
pub trait PositionRepository: Send + Sync {
    // ... existing methods
    
    /// Find active Core Trading position by symbol and side.
    /// Returns Some(position) if found in Entering, Active, or Exiting state.
    async fn find_active_by_symbol_and_side(
        &self,
        symbol: &Symbol,
        side: Side,
    ) -> Result<Option<Position>, StoreError>;
}
```

3. Implement for PostgreSQL repository:

```rust
impl PositionRepository for PostgresPositionRepository {
    async fn find_active_by_symbol_and_side(
        &self,
        symbol: &Symbol,
        side: Side,
    ) -> Result<Option<Position>, StoreError> {
        let row = sqlx::query!(
            r#"
            SELECT id, symbol, side, entry_price, quantity, state, created_at
            FROM positions
            WHERE symbol = $1 AND side = $2 
              AND state IN ('Entering', 'Active', 'Exiting')
            LIMIT 1
            "#,
            symbol.as_str(),
            side.as_str(),
        )
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| Position::from_row(r)))
    }
}
```

4. Update `poll_and_update()` to use filter:

```rust
async fn poll_and_update(&self) -> Result<(), anyhow::Error> {
    let positions = self.binance_client.get_isolated_margin_positions().await?;
    
    for binance_pos in positions {
        let symbol = Symbol::from_str(&binance_pos.symbol)?;
        let side = Side::from_str(&binance_pos.side)?;
        
        // EXCLUSION FILTER: Skip Core-managed positions
        if self.is_core_managed(&symbol, side).await? {
            info!(
                "Safety Net: Skipping {} {} (Core-managed)",
                symbol.as_str(),
                side.as_str()
            );
            continue;
        }
        
        // Rest of safety net logic...
    }
    
    Ok(())
}
```

**Validation**: 
- Code compiles
- Unit test: `test_is_core_managed_returns_true_for_active_position()`
- Unit test: `test_is_core_managed_returns_false_for_manual_position()`

---

### Task 1.3: Add binance_position_id to Core Positions

**File**: `v2/migrations/003_add_binance_position_id.sql` (NEW)

**Objective**: Link Core positions to Binance positions for reconciliation.

**Steps:**

1. Create migration file:

```sql
-- Migration: Add binance_position_id for Core <-> Binance reconciliation
-- Created: 2026-02-14

ALTER TABLE positions 
ADD COLUMN binance_position_id VARCHAR(255);

-- Index for fast lookup by Binance ID
CREATE INDEX idx_positions_binance_id 
ON positions(binance_position_id) 
WHERE binance_position_id IS NOT NULL;

-- Comments
COMMENT ON COLUMN positions.binance_position_id IS 
'Binance internal position ID for linking Core positions to exchange positions';
```

2. Update `Position` entity in `v2/robson-domain/src/entities.rs`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Position {
    pub id: PositionId,
    pub symbol: Symbol,
    pub side: Side,
    pub entry_price: Price,
    pub quantity: Quantity,
    pub state: PositionState,
    pub binance_position_id: Option<String>,  // NEW
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
```

3. Update repository to store/retrieve:

```rust
impl PostgresPositionRepository {
    pub async fn save(&self, position: &Position) -> Result<(), StoreError> {
        sqlx::query!(
            r#"
            INSERT INTO positions (id, symbol, side, entry_price, quantity, state, binance_position_id, created_at, updated_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            ON CONFLICT (id) DO UPDATE SET
                state = EXCLUDED.state,
                binance_position_id = EXCLUDED.binance_position_id,
                updated_at = EXCLUDED.updated_at
            "#,
            position.id.as_str(),
            position.symbol.as_str(),
            position.side.as_str(),
            position.entry_price.as_decimal(),
            position.quantity.as_decimal(),
            position.state.as_str(),
            position.binance_position_id,  // NEW
            position.created_at,
            position.updated_at,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}
```

4. Update Engine to store Binance ID when entry order fills:

```rust
// In v2/robson-engine/src/engine.rs
// When processing OrderFilled event for entry order:

match filled_event {
    OrderType::Entry => {
        // ... existing logic to transition to Active
        
        // NEW: Store Binance position ID
        if let Some(binance_id) = order_response.position_id {
            position.binance_position_id = Some(binance_id);
            self.position_repo.save(&position).await?;
        }
    }
}
```

**Validation**:
- Migration runs successfully: `cargo run --bin robson-migrate`
- Position entity serializes/deserializes correctly
- Database stores and retrieves `binance_position_id`

---

### Task 1.4: Event Bus Coordination

**File**: `v2/robsond/src/event_bus.rs`

**Objective**: Emit events when Core positions open/close for real-time coordination.

**Steps:**

1. Add event types:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DaemonEvent {
    // ... existing events
    DetectorSignalFired {
        position_id: PositionId,
        symbol: Symbol,
        side: Side,
    },
    CorePositionOpened {
        position_id: PositionId,
        symbol: Symbol,
        side: Side,
        binance_position_id: String,
    },
    CorePositionClosed {
        position_id: PositionId,
        symbol: Symbol,
        side: Side,
    },
}
```

2. Update Engine to emit events:

```rust
// In v2/robson-engine/src/engine.rs

impl Engine {
    async fn handle_entry_filled(&mut self, position_id: &PositionId) -> Result<()> {
        // ... transition to Active
        
        // Emit event for Safety Net coordination
        if let Some(binance_id) = position.binance_position_id.as_ref() {
            self.event_bus.publish(DaemonEvent::CorePositionOpened {
                position_id: position.id.clone(),
                symbol: position.symbol.clone(),
                side: position.side,
                binance_position_id: binance_id.clone(),
            }).await?;
        }
        
        Ok(())
    }
    
    async fn handle_exit_filled(&mut self, position_id: &PositionId) -> Result<()> {
        // ... transition to Closed
        
        // Emit event
        self.event_bus.publish(DaemonEvent::CorePositionClosed {
            position_id: position.id.clone(),
            symbol: position.symbol.clone(),
            side: position.side,
        }).await?;
        
        Ok(())
    }
}
```

3. Add exclusion set to PositionMonitor:

```rust
pub struct PositionMonitor {
    // ... existing fields
    core_exclusion_set: Arc<RwLock<HashSet<(Symbol, Side)>>>,  // NEW
}
```

4. Subscribe to events:

```rust
impl PositionMonitor {
    pub async fn start(&self, event_bus: Arc<EventBus>) -> Result<()> {
        // Subscribe to Core position events
        let exclusion_set = self.core_exclusion_set.clone();
        event_bus.subscribe(move |event| {
            let exclusion_set = exclusion_set.clone();
            async move {
                match event {
                    DaemonEvent::CorePositionOpened { symbol, side, .. } => {
                        exclusion_set.write().await.insert((symbol, side));
                        info!("Safety Net: Added {} {} to exclusion set", symbol.as_str(), side.as_str());
                    }
                    DaemonEvent::CorePositionClosed { symbol, side, .. } => {
                        exclusion_set.write().await.remove(&(symbol, side));
                        info!("Safety Net: Removed {} {} from exclusion set", symbol.as_str(), side.as_str());
                    }
                    _ => {}
                }
            }
        }).await;
        
        // Start polling loop
        self.poll_loop().await
    }
}
```

5. Update `is_core_managed()` to check both DB and exclusion set:

```rust
async fn is_core_managed(&self, symbol: &Symbol, side: Side) -> Result<bool> {
    // Check in-memory cache first (fast path)
    if self.core_exclusion_set.read().await.contains(&(symbol.clone(), side)) {
        return Ok(true);
    }
    
    // Fallback to database query (slow path, but authoritative)
    self.core_position_repo
        .find_active_by_symbol_and_side(symbol, side)
        .await
        .map(|opt| opt.is_some())
}
```

**Validation**:
- Event published when position transitions to Active
- Event published when position transitions to Closed
- Safety Net receives events and updates exclusion set
- Unit test: `test_exclusion_set_updated_on_events()`

---

### Task 1.5: Integration Test for Coordination

**File**: `v2/robsond/tests/core_safety_coordination_test.rs` (NEW)

**Objective**: Verify Core and Safety Net coexist without conflicts.

**Steps:**

1. Create test file:

```rust
//! Integration tests for Core Trading + Safety Net coordination.

use robson_domain::*;
use robsond::*;
use std::time::Duration;
use tokio::time::sleep;

#[tokio::test]
async fn test_safety_net_skips_core_managed_position() {
    // Setup: In-memory DB, mock Binance
    let (db, binance_mock) = setup_test_environment().await;
    
    // 1. User arms BTCUSDT Long via CLI (Core Trading)
    let position_id = create_core_position(&db, "BTCUSDT", Side::Long).await;
    
    // 2. Simulate Binance position exists
    binance_mock.add_isolated_position("BTCUSDT", Side::Long, 100.0, 1.0);
    
    // 3. Start Safety Net
    let monitor = PositionMonitor::new(/* ... */);
    monitor.poll_and_update().await.unwrap();
    
    // 4. Assert: Safety Net did NOT create DetectedPosition
    assert_eq!(monitor.detected_positions.len(), 0);
    
    // 5. Assert: Logs show "Skipping BTCUSDT Long (Core-managed)"
    assert_logs_contain("Skipping BTCUSDT Long");
}

#[tokio::test]
async fn test_safety_net_monitors_manual_position() {
    // Setup
    let (db, binance_mock) = setup_test_environment().await;
    
    // 1. Simulate manual position opened on Binance (NO Core position)
    binance_mock.add_isolated_position("ETHUSDT", Side::Short, 2000.0, 5.0);
    
    // 2. Start Safety Net
    let monitor = PositionMonitor::new(/* ... */);
    monitor.poll_and_update().await.unwrap();
    
    // 3. Assert: Safety Net DID create DetectedPosition
    assert_eq!(monitor.detected_positions.len(), 1);
    let detected = monitor.detected_positions.get(&(Symbol::from("ETHUSDT"), Side::Short)).unwrap();
    assert_eq!(detected.entry_price.as_f64(), 2000.0);
    
    // 4. Assert: Safety stop calculated at 2% = 2040.0 (Short)
    let stop = detected.calculate_safety_stop();
    assert_eq!(stop.price.as_f64(), 2040.0);
}

#[tokio::test]
async fn test_both_modalities_different_symbols_no_conflict() {
    // Setup
    let (db, binance_mock) = setup_test_environment().await;
    
    // 1. Core Trading manages BTCUSDT
    create_core_position(&db, "BTCUSDT", Side::Long).await;
    binance_mock.add_isolated_position("BTCUSDT", Side::Long, 95000.0, 0.1);
    
    // 2. Manual position on ETHUSDT
    binance_mock.add_isolated_position("ETHUSDT", Side::Short, 2000.0, 5.0);
    
    // 3. Start Safety Net
    let monitor = PositionMonitor::new(/* ... */);
    monitor.poll_and_update().await.unwrap();
    
    // 4. Assert: Safety Net monitors ONLY ETHUSDT
    assert_eq!(monitor.detected_positions.len(), 1);
    assert!(monitor.detected_positions.contains_key(&(Symbol::from("ETHUSDT"), Side::Short)));
    assert!(!monitor.detected_positions.contains_key(&(Symbol::from("BTCUSDT"), Side::Long)));
}

#[tokio::test]
async fn test_event_bus_coordination_realtime() {
    // Setup
    let (db, binance_mock, event_bus) = setup_test_environment_with_event_bus().await;
    
    // 1. Start Safety Net (subscribes to events)
    let monitor = PositionMonitor::new(/* ... */);
    tokio::spawn(monitor.start(event_bus.clone()));
    
    // 2. Binance has manual position on BTCUSDT
    binance_mock.add_isolated_position("BTCUSDT", Side::Long, 95000.0, 0.1);
    
    // 3. Safety Net polls, detects manual position
    sleep(Duration::from_secs(21)).await;
    assert_eq!(monitor.detected_positions.len(), 1);
    
    // 4. Core Trading opens position on BTCUSDT (emits event)
    event_bus.publish(DaemonEvent::CorePositionOpened {
        position_id: PositionId::new(),
        symbol: Symbol::from("BTCUSDT"),
        side: Side::Long,
        binance_position_id: "12345".to_string(),
    }).await;
    
    // 5. Wait for event processing
    sleep(Duration::from_millis(100)).await;
    
    // 6. Assert: Safety Net removed BTCUSDT from monitoring
    assert_eq!(monitor.detected_positions.len(), 0);
}
```

**Validation**:
- All tests pass: `cargo test -p robsond --test core_safety_coordination_test`
- No race conditions observed
- Logs show correct exclusion behavior

---

## Part 2: Complete Binance Connector (Priority: CRITICAL)

### Task 2.1: Extend BinanceRestClient - Order Execution

**File**: `v2/robson-connectors/src/binance_rest.rs`

**Objective**: Add order placement, cancellation, and status querying.

**Steps:**

1. Read existing file to understand structure:

```bash
# File has 581 lines, includes:
# - Authentication (API key, signature)
# - get_isolated_margin_positions()
# - Error handling
```

2. Add order execution methods:

```rust
impl BinanceRestClient {
    /// Place a market order on isolated margin.
    /// 
    /// # Arguments
    /// * `symbol` - Trading pair (e.g., "BTCUSDT")
    /// * `side` - "BUY" or "SELL"
    /// * `quantity` - Order quantity
    /// * `client_order_id` - Unique ID for idempotency (e.g., "robson_core_{ulid}")
    /// 
    /// # Returns
    /// Order response with order_id, fills, position_id
    pub async fn place_market_order(
        &self,
        symbol: &str,
        side: &str,
        quantity: Decimal,
        client_order_id: &str,
    ) -> Result<OrderResponse, BinanceRestError> {
        let timestamp = self.get_timestamp();
        let recv_window = 5000;
        
        // Build params
        let params = format!(
            "symbol={}&side={}&type=MARKET&quantity={}&newClientOrderId={}&isIsolated=TRUE&timestamp={}&recvWindow={}",
            symbol,
            side,
            quantity,
            client_order_id,
            timestamp,
            recv_window
        );
        
        // Sign request
        let signature = self.sign(&params);
        let url = format!(
            "{}/api/v3/order?{}&signature={}",
            self.base_url,
            params,
            signature
        );
        
        // Execute
        let response = self.client
            .post(&url)
            .header("X-MBX-APIKEY", &self.api_key)
            .send()
            .await?;
        
        // Handle response
        if response.status().is_success() {
            let order_resp: OrderResponse = response.json().await?;
            Ok(order_resp)
        } else {
            let error: BinanceApiError = response.json().await?;
            Err(BinanceRestError::ApiError(error))
        }
    }
    
    /// Cancel an open order.
    pub async fn cancel_order(
        &self,
        symbol: &str,
        order_id: u64,
    ) -> Result<CancelResponse, BinanceRestError> {
        let timestamp = self.get_timestamp();
        let params = format!(
            "symbol={}&orderId={}&isIsolated=TRUE&timestamp={}",
            symbol,
            order_id,
            timestamp
        );
        
        let signature = self.sign(&params);
        let url = format!(
            "{}/api/v3/order?{}&signature={}",
            self.base_url,
            params,
            signature
        );
        
        let response = self.client
            .delete(&url)
            .header("X-MBX-APIKEY", &self.api_key)
            .send()
            .await?;
        
        if response.status().is_success() {
            Ok(response.json().await?)
        } else {
            Err(BinanceRestError::ApiError(response.json().await?))
        }
    }
    
    /// Query order status.
    pub async fn get_order_status(
        &self,
        symbol: &str,
        order_id: u64,
    ) -> Result<OrderStatusResponse, BinanceRestError> {
        let timestamp = self.get_timestamp();
        let params = format!(
            "symbol={}&orderId={}&timestamp={}",
            symbol,
            order_id,
            timestamp
        );
        
        let signature = self.sign(&params);
        let url = format!(
            "{}/api/v3/order?{}&signature={}",
            self.base_url,
            params,
            signature
        );
        
        let response = self.client
            .get(&url)
            .header("X-MBX-APIKEY", &self.api_key)
            .send()
            .await?;
        
        if response.status().is_success() {
            Ok(response.json().await?)
        } else {
            Err(BinanceRestError::ApiError(response.json().await?))
        }
    }
}
```

3. Add response types:

```rust
#[derive(Debug, Deserialize)]
pub struct OrderResponse {
    #[serde(rename = "orderId")]
    pub order_id: u64,
    #[serde(rename = "clientOrderId")]
    pub client_order_id: String,
    pub symbol: String,
    pub side: String,
    pub status: String,
    pub fills: Vec<Fill>,
    #[serde(rename = "positionId")]
    pub position_id: Option<String>,  // For linking to Binance position
}

#[derive(Debug, Deserialize)]
pub struct Fill {
    pub price: String,
    pub qty: String,
    pub commission: String,
    #[serde(rename = "commissionAsset")]
    pub commission_asset: String,
}

#[derive(Debug, Deserialize)]
pub struct CancelResponse {
    pub symbol: String,
    #[serde(rename = "orderId")]
    pub order_id: u64,
    pub status: String,
}

#[derive(Debug, Deserialize)]
pub struct OrderStatusResponse {
    pub symbol: String,
    #[serde(rename = "orderId")]
    pub order_id: u64,
    pub status: String,
    pub side: String,
    pub price: String,
    #[serde(rename = "origQty")]
    pub orig_qty: String,
    #[serde(rename = "executedQty")]
    pub executed_qty: String,
}
```

**Validation**:
- Code compiles
- Unit tests with mock server
- Integration tests with Binance testnet (see Task 2.3)

---

### Task 2.2: Add Rate Limiting and Retry Logic

**File**: `v2/robson-connectors/src/binance_rest.rs`

**Objective**: Prevent Binance API rate limit errors (HTTP 429).

**Steps:**

1. Add rate limiter dependency to `v2/robson-connectors/Cargo.toml`:

```toml
[dependencies]
# ... existing
governor = "0.6"
backoff = "0.4"
```

2. Add rate limiter to client:

```rust
use governor::{Quota, RateLimiter};
use std::num::NonZeroU32;

pub struct BinanceRestClient {
    // ... existing fields
    rate_limiter: RateLimiter<
        governor::state::direct::NotKeyed,
        governor::state::InMemoryState,
        governor::clock::DefaultClock,
    >,
}

impl BinanceRestClient {
    pub fn new(api_key: String, api_secret: String, base_url: String) -> Self {
        // Binance limit: 1200 requests/minute = 20 req/sec
        let quota = Quota::per_second(NonZeroU32::new(20).unwrap());
        let rate_limiter = RateLimiter::direct(quota);
        
        Self {
            client: reqwest::Client::new(),
            api_key,
            api_secret,
            base_url,
            rate_limiter,
        }
    }
    
    /// Wait for rate limiter before making request.
    async fn wait_for_capacity(&self) {
        self.rate_limiter.until_ready().await;
    }
}
```

3. Update all HTTP methods to use rate limiter:

```rust
pub async fn place_market_order(/* ... */) -> Result<OrderResponse, BinanceRestError> {
    self.wait_for_capacity().await;  // NEW: Wait for rate limit
    
    // ... rest of implementation
}
```

4. Add exponential backoff retry:

```rust
use backoff::{ExponentialBackoff, future::retry};

pub async fn place_market_order_with_retry(
    &self,
    symbol: &str,
    side: &str,
    quantity: Decimal,
    client_order_id: &str,
) -> Result<OrderResponse, BinanceRestError> {
    let backoff = ExponentialBackoff {
        max_elapsed_time: Some(Duration::from_secs(30)),
        ..Default::default()
    };
    
    retry(backoff, || async {
        match self.place_market_order(symbol, side, quantity, client_order_id).await {
            Ok(resp) => Ok(resp),
            Err(BinanceRestError::RateLimit) => Err(backoff::Error::Transient {
                err: BinanceRestError::RateLimit,
                retry_after: Some(Duration::from_secs(2)),
            }),
            Err(e) => Err(backoff::Error::Permanent(e)),
        }
    }).await
}
```

**Validation**:
- Rate limiter prevents more than 20 req/sec
- Retry logic handles transient failures
- Unit test: `test_rate_limiter_throttles_requests()`

---

### Task 2.3: Integration Tests with Binance Testnet

**File**: `v2/robson-connectors/tests/binance_integration_test.rs` (NEW)

**Objective**: Validate Binance connector against live testnet API.

**Steps:**

1. Create test file:

```rust
//! Integration tests for Binance REST API.
//! 
//! Requires testnet credentials:
//! export BINANCE_TESTNET_API_KEY="..."
//! export BINANCE_TESTNET_SECRET="..."
//! 
//! Run with: cargo test -p robson-connectors --ignored

use robson_connectors::*;
use std::env;

fn get_testnet_client() -> BinanceRestClient {
    let api_key = env::var("BINANCE_TESTNET_API_KEY")
        .expect("BINANCE_TESTNET_API_KEY not set");
    let api_secret = env::var("BINANCE_TESTNET_SECRET")
        .expect("BINANCE_TESTNET_SECRET not set");
    
    BinanceRestClient::new(
        api_key,
        api_secret,
        "https://testnet.binance.vision".to_string(),
    )
}

#[tokio::test]
#[ignore] // Only run with --ignored flag
async fn test_place_market_order_testnet() {
    let client = get_testnet_client();
    
    // Place small market buy order
    let result = client.place_market_order(
        "BTCUSDT",
        "BUY",
        Decimal::from_str("0.001").unwrap(),
        &format!("robson_test_{}", ulid::Ulid::new()),
    ).await;
    
    match result {
        Ok(resp) => {
            println!("Order placed: {:?}", resp);
            assert_eq!(resp.symbol, "BTCUSDT");
            assert_eq!(resp.side, "BUY");
            assert!(resp.order_id > 0);
        }
        Err(e) => {
            // Testnet might reject for insufficient balance
            println!("Order failed (expected on testnet): {:?}", e);
        }
    }
}

#[tokio::test]
#[ignore]
async fn test_get_order_status_testnet() {
    let client = get_testnet_client();
    
    // First, place an order
    let order_resp = client.place_market_order(
        "ETHUSDT",
        "SELL",
        Decimal::from_str("0.01").unwrap(),
        &format!("robson_test_{}", ulid::Ulid::new()),
    ).await.unwrap();
    
    // Then, query its status
    let status = client.get_order_status("ETHUSDT", order_resp.order_id).await.unwrap();
    
    assert_eq!(status.order_id, order_resp.order_id);
    println!("Order status: {:?}", status);
}

#[tokio::test]
#[ignore]
async fn test_rate_limiter_throttles() {
    let client = get_testnet_client();
    
    let start = Instant::now();
    
    // Fire 50 requests (should take ~2.5 seconds due to rate limit)
    for i in 0..50 {
        let _ = client.get_isolated_margin_positions().await;
    }
    
    let elapsed = start.elapsed();
    
    // Should take at least 2 seconds (50 req / 20 req/s = 2.5s)
    assert!(elapsed.as_secs() >= 2);
}
```

**Validation**:
- Run: `BINANCE_TESTNET_API_KEY=xxx BINANCE_TESTNET_SECRET=yyy cargo test -p robson-connectors --ignored`
- All tests pass or fail gracefully (testnet balance issues expected)

---

### Task 2.4: Implement WebSocket Client

**File**: `v2/robson-connectors/src/binance_ws.rs` (NEW)

**Objective**: Real-time market data and user data streams for price monitoring.

**Steps:**

1. Add WebSocket dependencies to `v2/robson-connectors/Cargo.toml`:

```toml
[dependencies]
# ... existing
tokio-tungstenite = "0.21"
futures-util = "0.3"
```

2. Create WebSocket client:

```rust
//! Binance WebSocket Client for real-time data streams.

use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use anyhow::Result;

pub struct BinanceWebSocketClient {
    base_url: String,
}

impl BinanceWebSocketClient {
    pub fn new(base_url: String) -> Self {
        Self { base_url }
    }
    
    /// Subscribe to user data stream (account updates, order updates).
    /// 
    /// Requires a listen key from REST API.
    pub async fn subscribe_user_data(&self, listen_key: &str) -> Result<WebSocketStream> {
        let url = format!("{}/ws/{}", self.base_url, listen_key);
        let (ws_stream, _) = connect_async(url).await?;
        
        Ok(WebSocketStream::new(ws_stream))
    }
    
    /// Subscribe to real-time ticker for a symbol.
    pub async fn subscribe_ticker(&self, symbol: &str) -> Result<WebSocketStream> {
        let symbol_lower = symbol.to_lowercase();
        let url = format!("{}/ws/{}@ticker", self.base_url, symbol_lower);
        let (ws_stream, _) = connect_async(url).await?;
        
        Ok(WebSocketStream::new(ws_stream))
    }
    
    /// Subscribe to aggregated trades (for price monitoring).
    pub async fn subscribe_agg_trade(&self, symbol: &str) -> Result<WebSocketStream> {
        let symbol_lower = symbol.to_lowercase();
        let url = format!("{}/ws/{}@aggTrade", self.base_url, symbol_lower);
        let (ws_stream, _) = connect_async(url).await?;
        
        Ok(WebSocketStream::new(ws_stream))
    }
}

pub struct WebSocketStream {
    inner: tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
}

impl WebSocketStream {
    fn new(inner: tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>) -> Self {
        Self { inner }
    }
    
    /// Receive next message from stream.
    pub async fn next(&mut self) -> Option<Result<WsMessage>> {
        match self.inner.next().await {
            Some(Ok(Message::Text(text))) => {
                Some(serde_json::from_str(&text).map_err(|e| e.into()))
            }
            Some(Ok(Message::Close(_))) => None,
            Some(Err(e)) => Some(Err(e.into())),
            _ => None,
        }
    }
    
    /// Send ping to keep connection alive.
    pub async fn ping(&mut self) -> Result<()> {
        self.inner.send(Message::Ping(vec![])).await?;
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "e")]
pub enum WsMessage {
    #[serde(rename = "aggTrade")]
    AggTrade(AggTradeEvent),
    #[serde(rename = "24hrTicker")]
    Ticker(TickerEvent),
    #[serde(rename = "executionReport")]
    ExecutionReport(ExecutionReportEvent),
}

#[derive(Debug, Deserialize)]
pub struct AggTradeEvent {
    #[serde(rename = "s")]
    pub symbol: String,
    #[serde(rename = "p")]
    pub price: String,
    #[serde(rename = "q")]
    pub quantity: String,
    #[serde(rename = "T")]
    pub trade_time: u64,
}

#[derive(Debug, Deserialize)]
pub struct TickerEvent {
    #[serde(rename = "s")]
    pub symbol: String,
    #[serde(rename = "c")]
    pub close_price: String,
    #[serde(rename = "h")]
    pub high_price: String,
    #[serde(rename = "l")]
    pub low_price: String,
}

#[derive(Debug, Deserialize)]
pub struct ExecutionReportEvent {
    #[serde(rename = "s")]
    pub symbol: String,
    #[serde(rename = "S")]
    pub side: String,
    #[serde(rename = "o")]
    pub order_type: String,
    #[serde(rename = "X")]
    pub order_status: String,
    #[serde(rename = "i")]
    pub order_id: u64,
}
```

3. Add listen key management to REST client:

```rust
// In v2/robson-connectors/src/binance_rest.rs

impl BinanceRestClient {
    /// Create a new listen key for user data stream.
    /// 
    /// Listen keys expire after 60 minutes and must be renewed.
    pub async fn create_listen_key(&self) -> Result<String, BinanceRestError> {
        let url = format!("{}/api/v3/userDataStream", self.base_url);
        
        let response = self.client
            .post(&url)
            .header("X-MBX-APIKEY", &self.api_key)
            .send()
            .await?;
        
        if response.status().is_success() {
            let resp: ListenKeyResponse = response.json().await?;
            Ok(resp.listen_key)
        } else {
            Err(BinanceRestError::ApiError(response.json().await?))
        }
    }
    
    /// Keep-alive for listen key (call every 30 minutes).
    pub async fn renew_listen_key(&self, listen_key: &str) -> Result<(), BinanceRestError> {
        let url = format!("{}/api/v3/userDataStream", self.base_url);
        
        self.client
            .put(&url)
            .header("X-MBX-APIKEY", &self.api_key)
            .query(&[("listenKey", listen_key)])
            .send()
            .await?;
        
        Ok(())
    }
}

#[derive(Deserialize)]
struct ListenKeyResponse {
    #[serde(rename = "listenKey")]
    listen_key: String,
}
```

**Validation**:
- WebSocket connects successfully
- Receives real-time price updates
- User data stream receives order execution events
- Unit test: `test_websocket_receives_ticker_updates()`

---

## Part 3: Kubernetes Deployment (Priority: HIGH)

### Task 3.1: Add Health Endpoints

**File**: `v2/robsond/src/api.rs`

**Objective**: Kubernetes liveness and readiness probes.

**Steps:**

1. Add health check endpoints:

```rust
use actix_web::{get, web, HttpResponse, Responder};
use serde_json::json;

/// GET /healthz - Liveness probe (is process alive?)
#[get("/healthz")]
async fn health_liveness() -> impl Responder {
    HttpResponse::Ok().json(json!({
        "status": "ok",
        "timestamp": chrono::Utc::now().to_rfc3339(),
    }))
}

/// GET /readyz - Readiness probe (is service ready to accept traffic?)
#[get("/readyz")]
async fn health_readiness(state: web::Data<AppState>) -> impl Responder {
    // Check database connection
    let db_ok = state.db_pool.acquire().await.is_ok();
    
    // Check Binance API reachable (simple ping)
    let binance_ok = state.binance_client
        .ping()
        .await
        .is_ok();
    
    if db_ok && binance_ok {
        HttpResponse::Ok().json(json!({
            "status": "ready",
            "checks": {
                "database": "ok",
                "binance_api": "ok",
            },
            "timestamp": chrono::Utc::now().to_rfc3339(),
        }))
    } else {
        HttpResponse::ServiceUnavailable().json(json!({
            "status": "not_ready",
            "checks": {
                "database": if db_ok { "ok" } else { "failed" },
                "binance_api": if binance_ok { "ok" } else { "failed" },
            },
            "timestamp": chrono::Utc::now().to_rfc3339(),
        }))
    }
}

// Register routes
pub fn configure_health_routes(cfg: &mut web::ServiceConfig) {
    cfg.service(health_liveness)
       .service(health_readiness);
}
```

2. Add ping method to BinanceRestClient:

```rust
// In v2/robson-connectors/src/binance_rest.rs

impl BinanceRestClient {
    /// Ping Binance API to check connectivity.
    pub async fn ping(&self) -> Result<(), BinanceRestError> {
        let url = format!("{}/api/v3/ping", self.base_url);
        let response = self.client.get(&url).send().await?;
        
        if response.status().is_success() {
            Ok(())
        } else {
            Err(BinanceRestError::Unreachable)
        }
    }
}
```

3. Update main server configuration:

```rust
// In v2/robsond/src/main.rs

HttpServer::new(move || {
    App::new()
        .app_data(web::Data::new(app_state.clone()))
        .configure(api::configure_health_routes)  // NEW
        .configure(api::configure_api_routes)
})
.bind(("0.0.0.0", 8080))?
.run()
.await
```

**Validation**:
- `curl http://localhost:8080/healthz` returns 200 OK
- `curl http://localhost:8080/readyz` returns 200 when DB + Binance healthy
- `curl http://localhost:8080/readyz` returns 503 when DB or Binance unhealthy

---

### Task 3.2: Create Dockerfile

**File**: `v2/Dockerfile` (NEW)

**Objective**: Multi-stage build for minimal production image.

**Steps:**

1. Create Dockerfile:

```dockerfile
# Stage 1: Build
FROM rust:1.83-slim AS builder

WORKDIR /app

# Install build dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Copy workspace files
COPY Cargo.toml Cargo.lock ./
COPY robsond/ ./robsond/
COPY robson-engine/ ./robson-engine/
COPY robson-domain/ ./robson-domain/
COPY robson-store/ ./robson-store/
COPY robson-connectors/ ./robson-connectors/
COPY robson-exec/ ./robson-exec/

# Build release binary
RUN cargo build --release -p robsond

# Stage 2: Runtime
FROM debian:bookworm-slim

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*

# Create non-root user
RUN useradd -m -u 1000 robson

# Copy binary from builder
COPY --from=builder /app/target/release/robsond /usr/local/bin/robsond

# Set ownership
RUN chown robson:robson /usr/local/bin/robsond

# Switch to non-root user
USER robson

# Expose port
EXPOSE 8080

# Health check
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
    CMD curl -f http://localhost:8080/healthz || exit 1

# Run daemon
CMD ["robsond"]
```

2. Create `.dockerignore`:

```
target/
.git/
.github/
docs/
*.md
.env
.env.*
node_modules/
```

3. Build and test:

```bash
cd v2
docker build -t robson-v2:latest .
docker run -p 8080:8080 -e DATABASE_URL=postgres://... -e BINANCE_API_KEY=... robson-v2:latest
```

**Validation**:
- Image builds successfully
- Image size < 100MB (slim runtime)
- Container runs and exposes port 8080
- Health check passes

---

### Task 3.3: Create Kubernetes Manifests

**Directory**: `v2/k8s/prod/` (NEW)

**Objective**: Deploy robsond to Kubernetes cluster.

**Steps:**

1. Create namespace manifest:

```yaml
# v2/k8s/prod/namespace.yml
apiVersion: v1
kind: Namespace
metadata:
  name: robson-v2
  labels:
    app: robson
    version: v2
```

2. Create secret template:

```yaml
# v2/k8s/prod/robsond-secret.yml.template
# WARNING: This is a TEMPLATE. Replace placeholders before applying!
apiVersion: v1
kind: Secret
metadata:
  name: robsond-secret
  namespace: robson-v2
type: Opaque
stringData:
  database-url: "postgresql://user:password@postgres-service:5432/robson_v2"
  binance-api-key: "YOUR_BINANCE_API_KEY"
  binance-api-secret: "YOUR_BINANCE_API_SECRET"
```

3. Create ConfigMap:

```yaml
# v2/k8s/prod/robsond-configmap.yml
apiVersion: v1
kind: ConfigMap
metadata:
  name: robsond-config
  namespace: robson-v2
data:
  binance-base-url: "https://api.binance.com"
  log-level: "info"
  polling-interval-seconds: "20"
```

4. Create Deployment:

```yaml
# v2/k8s/prod/robsond-deployment.yml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: robsond
  namespace: robson-v2
  labels:
    app: robsond
    version: v2
spec:
  replicas: 1  # Single instance (leader election not yet implemented)
  selector:
    matchLabels:
      app: robsond
  template:
    metadata:
      labels:
        app: robsond
        version: v2
    spec:
      containers:
      - name: robsond
        image: ghcr.io/your-org/robson-v2:latest
        imagePullPolicy: Always
        ports:
        - containerPort: 8080
          name: http
          protocol: TCP
        env:
        - name: DATABASE_URL
          valueFrom:
            secretKeyRef:
              name: robsond-secret
              key: database-url
        - name: BINANCE_API_KEY
          valueFrom:
            secretKeyRef:
              name: robsond-secret
              key: binance-api-key
        - name: BINANCE_API_SECRET
          valueFrom:
            secretKeyRef:
              name: robsond-secret
              key: binance-api-secret
        - name: BINANCE_BASE_URL
          valueFrom:
            configMapKeyRef:
              name: robsond-config
              key: binance-base-url
        - name: LOG_LEVEL
          valueFrom:
            configMapKeyRef:
              name: robsond-config
              key: log-level
        - name: RUST_LOG
          value: "robsond=$(LOG_LEVEL),robson_engine=$(LOG_LEVEL)"
        resources:
          requests:
            memory: "256Mi"
            cpu: "250m"
          limits:
            memory: "512Mi"
            cpu: "500m"
        livenessProbe:
          httpGet:
            path: /healthz
            port: 8080
          initialDelaySeconds: 10
          periodSeconds: 30
          timeoutSeconds: 3
          failureThreshold: 3
        readinessProbe:
          httpGet:
            path: /readyz
            port: 8080
          initialDelaySeconds: 5
          periodSeconds: 10
          timeoutSeconds: 3
          failureThreshold: 3
      restartPolicy: Always
```

5. Create Service:

```yaml
# v2/k8s/prod/robsond-service.yml
apiVersion: v1
kind: Service
metadata:
  name: robsond-service
  namespace: robson-v2
  labels:
    app: robsond
spec:
  type: ClusterIP
  ports:
  - port: 8080
    targetPort: 8080
    protocol: TCP
    name: http
  selector:
    app: robsond
```

6. Create kustomization file:

```yaml
# v2/k8s/prod/kustomization.yml
apiVersion: kustomize.config.k8s.io/v1beta1
kind: Kustomization

namespace: robson-v2

resources:
  - namespace.yml
  - robsond-configmap.yml
  - robsond-deployment.yml
  - robsond-service.yml
  # Note: Secret is applied separately (contains sensitive data)

images:
  - name: ghcr.io/your-org/robson-v2
    newTag: latest
```

**Validation**:
- All YAML files are valid: `kubectl apply --dry-run=client -f v2/k8s/prod/`
- Matches existing patterns from v1 manifests

---

## Part 4: E2E Testing and Deployment (Priority: HIGH)

### Task 4.1: Create E2E Test Suite

**File**: `v2/robsond/tests/e2e_production.rs` (NEW)

**Objective**: End-to-end tests for production scenarios.

**Steps:**

1. Create test file:

```rust
//! E2E tests for production readiness.
//! 
//! These tests simulate the full lifecycle of positions and verify
//! that Core Trading and Safety Net work together correctly.

use robson_domain::*;
use robsond::*;
use tokio::time::{sleep, Duration};

#[tokio::test]
async fn test_e2e_core_trading_full_lifecycle() {
    // Setup: Real DB (testnet), mock Binance
    let env = E2ETestEnvironment::new().await;
    
    // 1. User arms BTCUSDT
    let arm_result = env.cli_arm("BTCUSDT", Side::Long).await;
    assert!(arm_result.is_ok());
    
    // 2. Detector fires signal
    env.inject_ma_crossover_signal("BTCUSDT").await;
    sleep(Duration::from_secs(1)).await;
    
    // 3. Entry order placed
    let orders = env.get_binance_orders().await;
    assert_eq!(orders.len(), 1);
    assert_eq!(orders[0].side, "BUY");
    
    // 4. Simulate entry fill
    env.fill_order(orders[0].order_id).await;
    sleep(Duration::from_millis(500)).await;
    
    // 5. Position transitions to Active
    let position = env.get_core_position_by_symbol("BTCUSDT").await.unwrap();
    assert_eq!(position.state, PositionState::Active);
    
    // 6. Simulate price rise (trailing stop should update)
    env.update_price("BTCUSDT", 96000.0).await;
    sleep(Duration::from_secs(2)).await;
    
    // 7. Trailing stop should have moved up
    let position = env.get_core_position_by_symbol("BTCUSDT").await.unwrap();
    // (Check stop price via engine state)
    
    // 8. Simulate stop hit
    env.update_price("BTCUSDT", 94000.0).await;  // Below trailing stop
    sleep(Duration::from_secs(2)).await;
    
    // 9. Exit order placed
    let orders = env.get_binance_orders().await;
    let exit_order = orders.iter().find(|o| o.side == "SELL").unwrap();
    
    // 10. Simulate exit fill
    env.fill_order(exit_order.order_id).await;
    sleep(Duration::from_millis(500)).await;
    
    // 11. Position closed
    let position = env.get_core_position_by_symbol("BTCUSDT").await.unwrap();
    assert_eq!(position.state, PositionState::Closed);
}

#[tokio::test]
async fn test_e2e_safety_net_protects_manual_position() {
    let env = E2ETestEnvironment::new().await;
    
    // 1. User opens manual position on Binance
    env.create_binance_position("ETHUSDT", Side::Short, 2000.0, 5.0).await;
    
    // 2. Safety Net polls and detects
    sleep(Duration::from_secs(21)).await;  // Wait for polling interval
    
    // 3. Safety Net calculates 2% stop = 2040.0
    let detected = env.get_detected_position("ETHUSDT", Side::Short).await.unwrap();
    assert_eq!(detected.entry_price.as_f64(), 2000.0);
    let stop = detected.calculate_safety_stop();
    assert_eq!(stop.price.as_f64(), 2040.0);
    
    // 4. Price rises to stop level
    env.update_price("ETHUSDT", 2041.0).await;
    sleep(Duration::from_secs(21)).await;
    
    // 5. Safety Net executes market order
    let orders = env.get_binance_orders().await;
    let exit_order = orders.iter().find(|o| o.symbol == "ETHUSDT" && o.side == "BUY").unwrap();
    assert!(exit_order.client_order_id.starts_with("robson_safety_"));
}

#[tokio::test]
async fn test_e2e_no_double_execution() {
    let env = E2ETestEnvironment::new().await;
    
    // 1. Core Trading opens BTCUSDT
    env.cli_arm("BTCUSDT", Side::Long).await.unwrap();
    env.inject_ma_crossover_signal("BTCUSDT").await;
    sleep(Duration::from_secs(1)).await;
    
    // 2. Entry fills
    let orders = env.get_binance_orders().await;
    env.fill_order(orders[0].order_id).await;
    sleep(Duration::from_millis(500)).await;
    
    // 3. Safety Net polls (should skip Core-managed position)
    sleep(Duration::from_secs(21)).await;
    
    // 4. Safety Net did NOT create DetectedPosition
    let detected = env.get_detected_position("BTCUSDT", Side::Long).await;
    assert!(detected.is_none(), "Safety Net should NOT monitor Core position");
    
    // 5. Core stop hits
    env.update_price("BTCUSDT", 93000.0).await;
    sleep(Duration::from_secs(2)).await;
    
    // 6. Only ONE exit order exists
    let exit_orders = env.get_binance_orders().await
        .into_iter()
        .filter(|o| o.side == "SELL")
        .collect::<Vec<_>>();
    
    assert_eq!(exit_orders.len(), 1, "Should have exactly ONE exit order (no double execution)");
}

#[tokio::test]
async fn test_e2e_crash_recovery() {
    let env = E2ETestEnvironment::new().await;
    
    // 1. Core Trading opens position
    env.cli_arm("ADAUSDT", Side::Long).await.unwrap();
    env.inject_ma_crossover_signal("ADAUSDT").await;
    sleep(Duration::from_secs(1)).await;
    
    // 2. Entry fills
    let orders = env.get_binance_orders().await;
    env.fill_order(orders[0].order_id).await;
    sleep(Duration::from_millis(500)).await;
    
    // 3. Position Active
    let position = env.get_core_position_by_symbol("ADAUSDT").await.unwrap();
    assert_eq!(position.state, PositionState::Active);
    
    // 4. CRASH: Kill robsond
    env.kill_daemon().await;
    sleep(Duration::from_secs(2)).await;
    
    // 5. Restart robsond
    env.start_daemon().await;
    sleep(Duration::from_secs(3)).await;
    
    // 6. Position should still be Active (recovered from event log)
    let position = env.get_core_position_by_symbol("ADAUSDT").await.unwrap();
    assert_eq!(position.state, PositionState::Active);
    
    // 7. Trailing stop still works after recovery
    env.update_price("ADAUSDT", 0.4).await;  // Below stop
    sleep(Duration::from_secs(2)).await;
    
    // 8. Exit order placed
    let exit_orders = env.get_binance_orders().await
        .into_iter()
        .filter(|o| o.side == "SELL" && o.symbol == "ADAUSDT")
        .collect::<Vec<_>>();
    
    assert_eq!(exit_orders.len(), 1, "Exit order should be placed after recovery");
}
```

**Validation**:
- All E2E tests pass: `cargo test -p robsond --test e2e_production`
- No race conditions observed
- Crash recovery works correctly

---

## Part 5: Deployment Tasks (Operational)

### Task 5.1: Staging Deployment

**Manual Steps** (cannot be automated by AI):

1. Build and push Docker image:

```bash
cd v2
docker build -t ghcr.io/your-org/robson-v2:staging .
docker push ghcr.io/your-org/robson-v2:staging
```

2. Create staging namespace:

```bash
kubectl create namespace robson-v2-staging
```

3. Create secret (replace placeholders):

```bash
kubectl create secret generic robsond-secret \
  --namespace=robson-v2-staging \
  --from-literal=database-url="postgresql://..." \
  --from-literal=binance-api-key="..." \
  --from-literal=binance-api-secret="..."
```

4. Apply manifests:

```bash
kubectl apply -f v2/k8s/staging/
```

5. Monitor logs:

```bash
kubectl logs -f -n robson-v2-staging deployment/robsond
```

6. Validate health:

```bash
kubectl port-forward -n robson-v2-staging svc/robsond-service 8080:8080
curl http://localhost:8080/healthz
curl http://localhost:8080/readyz
```

7. Run for 48+ hours, monitor:
   - No crashes or restarts
   - Health probes passing
   - No errors in logs
   - Can arm positions via CLI
   - Safety Net monitors manual positions
   - No double execution

**Success Criteria**: 48h uptime with zero critical errors.

---

### Task 5.2: Production Deployment

**Manual Steps** (cannot be automated by AI):

1. Tag production image:

```bash
docker tag ghcr.io/your-org/robson-v2:staging ghcr.io/your-org/robson-v2:v1.0.0
docker push ghcr.io/your-org/robson-v2:v1.0.0
```

2. Create production namespace (separate from v1):

```bash
kubectl create namespace robson-v2
```

3. Create production secret:

```bash
kubectl create secret generic robsond-secret \
  --namespace=robson-v2 \
  --from-literal=database-url="postgresql://..." \
  --from-literal=binance-api-key="..." \
  --from-literal=binance-api-secret="..."
```

4. Apply production manifests:

```bash
kubectl apply -f v2/k8s/prod/
```

5. Monitor for 24 hours:

```bash
kubectl logs -f -n robson-v2 deployment/robsond
```

6. Validate:
   - All health checks passing
   - Core Trading works (arm → entry → exit)
   - Safety Net works (manual position protected)
   - No double execution
   - No errors in logs

**Success Criteria**: 24h production uptime with zero critical errors.

---

### Task 5.3: V1 Shutdown

**Manual Steps** (only after V2 is proven stable):

1. Disable V1 CronJobs (do NOT delete yet, for rollback):

```bash
kubectl scale cronjob rbs-stop-monitor-cronjob --replicas=0 -n robson
kubectl scale cronjob rbs-trailing-stop-cronjob --replicas=0 -n robson
```

2. Monitor V2 for 24 more hours with V1 disabled.

3. If stable, scale down V1 backend:

```bash
kubectl scale deployment rbs-backend-monolith-prod --replicas=0 -n robson
```

4. Keep V1 deployments for 30 days (emergency rollback).

5. After 30 days, proceed to V1 cleanup (Task 5.4).

---

### Task 5.4: V1 Cleanup

**Files to Delete** (only after 2+ weeks of stable V2 production):

```bash
# Django management commands
rm apps/backend/monolith/api/management/commands/monitor_stops.py
rm apps/backend/monolith/api/management/commands/adjust_trailing_stops.py

# Kubernetes CronJobs
rm infra/k8s/prod/rbs-stop-monitor-cronjob.yml
rm infra/k8s/prod/rbs-trailing-stop-cronjob.yml

# Commit
git add -A
git commit -m "chore: remove V1 execution code (V2 stable in production) [i:cursor-sonnet]"
git push
```

**Update Documentation**:

- `docs/ARCHITECTURE.md`: Remove references to V1 CronJobs
- `docs/DEVELOPER.md`: Update deployment section for V2
- `README.md`: Update status (V2 in production)
- `v2/docs/EXECUTION-PLAN.md`: Mark Phase 9-10 as COMPLETE

---

## Summary Checklist

Before marking Phase 9-10 as COMPLETE, verify:

- [ ] ADR-0014 created and approved
- [ ] Safety Net skips Core-managed positions (tested)
- [ ] `binance_position_id` column added to database
- [ ] Event bus coordination implemented
- [ ] BinanceRestClient: place_order, cancel_order, get_status
- [ ] BinanceWebSocketClient implemented
- [ ] Rate limiting and retry logic added
- [ ] Integration tests with testnet passing
- [ ] Health endpoints `/healthz` and `/readyz` working
- [ ] Dockerfile builds successfully (image < 100MB)
- [ ] Kubernetes manifests created and validated
- [ ] E2E tests passing (Core + Safety Net coordination)
- [ ] Staging deployment: 48h+ uptime, no errors
- [ ] Production deployment: 24h+ uptime, no errors
- [ ] V1 CronJobs disabled
- [ ] V2 monitoring all positions (Core + Manual)
- [ ] No double execution observed
- [ ] Rollback plan tested
- [ ] V1 execution code removed (after 30 days)

---

## Rollback Plan

If V2 fails in production:

1. **Immediate**: Re-enable V1 CronJobs

```bash
kubectl scale cronjob rbs-stop-monitor-cronjob --replicas=1 -n robson
kubectl scale cronjob rbs-trailing-stop-cronjob --replicas=1 -n robson
```

2. **Disable V2**:

```bash
kubectl scale deployment robsond --replicas=0 -n robson-v2
```

3. **Investigate**: Review V2 logs, identify root cause.

4. **Fix and Redeploy**: Fix issue, redeploy to staging, re-validate.

---

## Key Takeaways

1. **Safety Net Exclusion**: Three-layer mechanism (DB query, event bus, position ID linking) prevents double execution.
2. **Binance Integration**: Full REST + WebSocket implementation with rate limiting and retry.
3. **Production Ready**: Kubernetes deployment with health checks, monitoring, rollback plan.
4. **Gradual Rollout**: Staging → Production → V1 Shutdown → V1 Cleanup.
5. **Zero Downtime**: V2 deployed to separate namespace, V1 disabled only after V2 proven stable.

---

**Document Version**: 1.0  
**Last Updated**: 2026-02-14  
**Status**: Ready for Execution
