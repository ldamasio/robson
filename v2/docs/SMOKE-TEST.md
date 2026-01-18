# Operational Smoke Test - Robson v2 MVP

**Version**: 2.0.0-alpha
**Last Updated**: 2026-01-17
**Scope**: Local MVP validation (StubExchange + MemoryStore)

---

## Purpose

This document provides a **copy/paste operational smoke test** to validate the Robson v2 MVP functionality locally. It exercises the complete position lifecycle using the stub exchange (no real funds, no Binance connection).

**What this validates**:
- CLI ↔ Daemon communication
- Position state transitions (Armed → Entering → Active → Closed)
- Signal orchestration (detector → engine → executor)
- Margin safety validation (isolated + 10x leverage)
- Golden Rule position sizing
- Trailing stop behavior
- Panic mode

**What this does NOT validate**:
- Real Binance API integration (Phase 9/3)
- PostgreSQL persistence (Phase 3)
- WebSocket reconnection logic (Phase 9/3)
- Production deployment

---

## Prerequisites

| Requirement | Minimum Version | How to Verify |
|-------------|-----------------|---------------|
| Rust toolchain | 1.83+ | `rustc --version` |
| Bun | 1.0+ | `bun --version` |
| robsond | Built locally | `cargo build -p robsond` |
| CLI | Installed locally | `cd cli && bun install` |

**No other requirements**:
- No Binance API keys needed (uses StubExchange)
- No PostgreSQL needed (uses MemoryStore)
- No Docker/Kubernetes needed

---

## Environment Variables (Optional)

```bash
# Default values shown - only set if you need custom values
export ROBSON_DAEMON_URL="http://localhost:8080"
```

---

## Test Cases

### T1: Daemon Startup

**Objective**: Verify daemon starts successfully and API endpoints respond.

**Steps**:

```bash
# 1. Start daemon in background
cargo run -p robsond &

# 2. Wait for startup (max 10 seconds)
sleep 3

# 3. Check health endpoint
curl http://localhost:8080/health
# Expected: {"status":"healthy","version":"2.0.0-alpha"}

# 4. Check status endpoint (should be empty initially)
curl http://localhost:8080/status | jq
# Expected: {"active_positions":0,"positions":[]}

# 5. Verify logs show no errors
# (Check terminal where robsond is running)
```

**Pass Criteria**:
- [ ] HTTP 200 response from `/health`
- [ ] HTTP 200 response from `/status`
- [ ] No panic or unwrap errors in logs
- [ ] API listening on port 8080 (or configured port)

**Fail If**:
- Connection refused → daemon not started
- Panic in logs → runtime error
- Port 8080 already in use → kill existing process: `pkill -f robsond`

---

### T2: ARM → SIGNAL → ENTRY → ACTIVE

**Objective**: Validate complete position lifecycle from arming to active trading.

**Steps**:

```bash
# 1. Arm a new position (LONG BTCUSDT)
cd cli
bun run dev arm BTCUSDT --capital 1000 --risk 1 --side long

# Expected output:
# Position armed: {position_id}
# State: Armed
# Waiting for detector signal...
```

```bash
# 2. Capture the position_id from output above
# Example: POSITION_ID="0193abcd-1234-5678-9012-abcdefghijkl"

# 3. Check status to verify position is Armed
bun run dev status

# Expected: Table showing 1 position in "Armed" state
```

```bash
# 4. Inject detector signal via HTTP (simulates MA crossover)
# Replace POSITION_ID with actual value from step 2
POSITION_ID="<your-position-id-here>"

curl -X POST http://localhost:8080/positions/$POSITION_ID/signal \
  -H "Content-Type: application/json" \
  -d '{
    "position_id": "'$POSITION_ID'",
    "entry_price": 95000,
    "stop_loss": 93000
  }' | jq

# Expected:
# {
#   "status": "signal_received",
#   "position_id": "...",
#   "entry_price": 95000,
#   "stop_loss": 93000
# }
```

```bash
# 5. Verify position transitioned to Entering → Active
bun run dev status

# Expected: Position now shows "Active" state
# - Entry order was placed (via StubExchange)
# - Position shows: entry_price, stop_loss, quantity
```

**Pass Criteria**:
- [ ] Position created with Armed state
- [ ] Signal injection returns HTTP 200
- [ ] Position transitions to Active state
- [ ] Position shows calculated quantity (not zero)
- [ ] Logs show margin safety validation passed

**Verify Golden Rule Sizing**:

```bash
# The quantity should be calculated as:
# Capital: $1000
# Risk: 1% = $10
# Stop distance: |95000 - 93000| = $2000
# Quantity = $10 / $2000 = 0.005 BTC

# Check status output shows quantity ~0.005
bun run dev status | jq '.positions[0].quantity'
# Expected: approximately 0.005 (may vary slightly due to decimal precision)
```

**Fail If**:
- Position remains Armed after signal → event bus not working
- Signal returns 404 → invalid position_id
- Signal returns 400 → invalid payload (stop must be below entry for LONG)
- Quantity is 0 → sizing calculation failed

---

### T3: ACTIVE → EXIT (Trailing Stop)

**Objective**: Validate trailing stop behavior and position exit.

**Note**: In the current MVP, trailing stop exit is validated via the E2E test suite. Manual testing is limited because:

1. StubExchange prices are hardcoded ($95K BTC)
2. No WebSocket-based price updates in MVP
3. Trailing stop triggers on price movement (requires dynamic market data)

**Automated Validation (Recommended)**:

```bash
# Run the E2E test that validates detector → signal → entry → exit flow
cargo test -p robsond test_e2e_detector_ma_crossover_signal -- --nocapture

# Expected: All assertions pass
# - Detector emits signal on MA crossover
# - Position transitions Armed → Entering → Active
# - Detector cleanup occurs (single-shot)
```

**Manual Validation (Limited)**:

```bash
# If you have an Active position from T2, check its exit conditions
bun run dev status | jq '.positions[0]'

# Look for:
# - "trailing_stop": distance in USD (should equal |entry - stop|)
# - "exit_reason": null (still active)

# To force exit, use panic command (see T5)
```

**Pass Criteria**:
- [ ] E2E test passes without panics
- [ ] Logs show trailing stop calculation
- [ ] Trailing stop distance equals TechnicalStopDistance

**Fail If**:
- E2E test fails → signal orchestration broken
- Trailing stop distance differs from initial stop distance → calculation error

---

### T4: DISARM (Cancel Armed Position)

**Objective**: Verify cancellation of position before entry.

**Steps**:

```bash
# 1. Arm a new position (different symbol or parameters)
cd cli
bun run dev arm ETHUSDT --capital 500 --risk 1 --side long

# Capture position_id from output
POSITION_ID="<new-position-id>"
```

```bash
# 2. Verify position is Armed
bun run dev status

# Expected: Shows new position in "Armed" state
```

```bash
# 3. Disarm the position (cancel before signal)
bun run dev disarm $POSITION_ID

# Expected: "Position disarmed successfully"
```

```bash
# 4. Verify position was removed
bun run dev status

# Expected: Position no longer in list (or shows "Disarmed" state)
```

**Pass Criteria**:
- [ ] Position created successfully
- [ ] Disarm command returns success
- [ ] Position removed from active list
- [ ] No error logs about orphaned tasks

**Fail If**:
- Disarm returns 404 → position already entered or invalid ID
- Position still shows after disarm → cleanup not working
- Error about "detector not found" → race condition in task cleanup

---

### T5: PANIC (Emergency Close All)

**Objective**: Validate panic mode closes all active positions immediately.

**Steps**:

```bash
# 1. Ensure you have at least one Active position
# (From T2, or arm+signal again if needed)
bun run dev status

# Expected: At least 1 position in "Active" state
```

```bash
# 2. Show panic command (dry-run, without confirmation)
bun run dev panic

# Expected: Shows warning and count of positions to be closed
# "Would close N positions immediately"
# "Use --confirm to execute"
```

```bash
# 3. Execute panic (with confirmation)
bun run dev panic --confirm

# Expected:
# "PANIC MODE ACTIVATED"
# "Closing N positions..."
# "All positions closed"
```

```bash
# 4. Verify all positions closed
bun run dev status

# Expected:
# - No Active positions
# - Closed positions may show in history (if implemented)
# - All positions in "Closed" or "Panicked" state
```

**Pass Criteria**:
- [ ] Panic requires --confirm flag (safety check)
- [ ] All Active positions transition to Closed/Panicked
- [ ] No orders remain open (in StubExchange)
- [ ] Logs show emergency exit executed

**Fail If**:
- Panic executes without --confirm → safety violation
- Some positions still Active after panic → executor failure
- Error about "order placement failed" → exchange port issue

---

## Runtime Invariants Checklist

Verify these invariants hold true during smoke test execution.

### Leverage Invariant

**Invariant**: All positions use fixed 10x leverage (isolated margin).

**How to Verify**:

```bash
# Check daemon logs for margin validation
# Look for: "Validating margin settings (isolated + 10x)"

# Or inspect code constant:
grep -r "FIXED_LEVERAGE" robson-exec/src/executor.rs
# Output: pub const FIXED_LEVERAGE: u8 = 10;
```

**Expected**: Always 10x, never configurable in MVP.

---

### Margin Safety Invariant

**Invariant**: Margin validation occurs BEFORE entry and BEFORE exit.

**How to Verify**:

```bash
# In daemon logs during entry (T2), look for:
# "Validating margin settings (isolated + 10x)"
# "Margin safety check passed"

# During exit (T3), look for:
# "Validating margin settings for exit (isolated + 10x)"
```

**Expected**: Validation runs for every order placement, never bypassed.

---

### Golden Rule Sizing Invariant

**Invariant**: Position quantity is DERIVED from capital, risk %, and stop distance. User does NOT specify quantity.

**Formula**:
```
Position Size = (Capital × Risk %) / |Entry Price - Stop Loss|
```

**How to Verify**:

```bash
# From T2 output, calculate expected quantity:
# Example from T2:
# Capital: $1000
# Risk: 1% = $10
# Entry: $95000
# Stop: $93000
# Stop Distance: $2000
# Expected Quantity: $10 / $2000 = 0.005 BTC

bun run dev status | jq '.positions[0].quantity'
# Should be approximately 0.005
```

**Expected**: CLI `--quantity` flag does NOT exist. Size is always calculated.

---

### Trailing Stop Distance Invariant

**Invariant**: Trailing stop distance equals TechnicalStopDistance (constant, not dynamic).

**How to Verify**:

```bash
# From T2, capture initial stop distance:
# Initial: |95000 - 93000| = $2000

# Check active position's trailing stop setting:
bun run dev status | jq '.positions[0].trailing_stop_distance'
# Should equal $2000 (initial stop distance)
```

**Expected**: Trailing stop does NOT tighten automatically. Only trails upward (for LONG).

---

### Idempotency Invariant

**Invariant**: Repeating the same signal (same signal_id) is idempotent (no duplicate orders).

**How to Verify**:

```bash
# From T2, capture the signal_id or use same payload
POSITION_ID="<your-active-position-id>"

# Send signal first time
curl -X POST http://localhost:8080/positions/$POSITION_ID/signal \
  -H "Content-Type: application/json" \
  -d '{
    "position_id": "'$POSITION_ID'",
    "entry_price": 95000,
    "stop_loss": 93000,
    "signal_id": "test-idempotency-123"
  }' | jq

# Expected: First signal succeeds

# Send EXACT same signal again (same signal_id)
curl -X POST http://localhost:8080/positions/$POSITION_ID/signal \
  -H "Content-Type: application/json" \
  -d '{
    "position_id": "'$POSITION_ID'",
    "entry_price": 95000,
    "stop_loss": 93000,
    "signal_id": "test-idempotency-123"
  }' | jq

# Expected: Returns "already_processed" or similar
# NO duplicate entry orders placed
```

**Expected**: Second signal returns success without placing new orders.

---

## Troubleshooting

### Issue 1: Daemon Not Starting

**Symptoms**: `cargo run -p robsond` fails with error or panic.

**Common Causes**:

| Cause | Solution |
|-------|----------|
| Port 8080 already in use | `lsof -ti:8080 \| xargs kill -9` |
| Missing dependencies | `cargo build` first, check for errors |
| Config file invalid | Check `robsond/src/config.rs` defaults |
| Database connection error | OK in MVP (uses MemoryStore, ignore DB errors) |

**Debug Commands**:

```bash
# Check what's using port 8080
lsof -i:8080

# Try with custom port
ROBSON_API_PORT=8081 cargo run -p robsond

# Check logs for panic message
RUST_BACKTRACE=1 cargo run -p robsond
```

---

### Issue 2: CLI Cannot Find Daemon

**Symptoms**: `bun run dev status` returns "ECONNREFUSED" or similar.

**Common Causes**:

| Cause | Solution |
|-------|----------|
| Daemon not running | Start daemon: `cargo run -p robsond &` |
| Wrong port configured | `export ROBSON_DAEMON_URL="http://localhost:8080"` |
| Firewall blocking | Check local firewall (unlikely on localhost) |

**Debug Commands**:

```bash
# Verify daemon is listening
curl http://localhost:8080/health

# Check CLI's configured URL
echo $ROBSON_DAEMON_URL

# Test connection manually
curl http://localhost:8080/status
```

---

### Issue 3: Status Shows No Positions

**Symptoms**: `bun run dev status` returns empty list even after arming.

**Common Causes**:

| Cause | Solution |
|-------|----------|
| Position failed to arm | Check daemon logs for errors |
| Position already expired | Positions may auto-expire (check config) |
| Signal already processed | If using test signal, position may have moved to Active |
| MemoryStore lost data | Restarted daemon clears in-memory data |

**Debug Commands**:

```bash
# Check daemon logs for arm errors
# (In terminal where robsond is running)

# Try arming again with verbose output
bun run dev --verbose arm BTCUSDT --capital 1000 --risk 1 --side long

# Check if position exists via API directly
curl http://localhost:8080/status | jq
```

---

### Issue 4: Margin Safety Validation Error

**Symptoms**: Signal returns 400 error with "margin safety validation failed".

**Common Causes**:

| Cause | Solution |
|-------|----------|
| Stop loss above entry (for LONG) | Ensure stop < entry for LONG positions |
| Stop loss below entry (for SHORT) | Ensure stop > entry for SHORT positions |
| Insufficient capital for 10x leverage | Increase --capital amount |
| StubExchange returns invalid margin | This is a bug, report it |

**Debug Commands**:

```bash
# Verify your signal payload
# For LONG: stop_loss < entry_price
# For SHORT: stop_loss > entry_price

# Example valid LONG signal:
curl -X POST http://localhost:8080/positions/$POSITION_ID/signal \
  -H "Content-Type: application/json" \
  -d '{
    "position_id": "'$POSITION_ID'",
    "entry_price": 95000,
    "stop_loss": 93000  # BELOW entry (valid for LONG)
  }'

# Example valid SHORT signal:
curl -X POST http://localhost:8080/positions/$POSITION_ID/signal \
  -H "Content-Type: application/json" \
  -d '{
    "position_id": "'$POSITION_ID'",
    "entry_price": 95000,
    "stop_loss": 97000  # ABOVE entry (valid for SHORT)
  }'
```

---

### Issue 5: Signal Returns Invalid Payload Error

**Symptoms**: POST to `/positions/:id/signal` returns 400 with "validation error".

**Common Causes**:

| Cause | Solution |
|-------|----------|
| Missing required fields | Ensure `position_id`, `entry_price`, `stop_loss` are present |
| Invalid price format | Use numbers (not strings), e.g., `95000` not `"95000"` |
| Invalid UUID format | Ensure `position_id` is valid UUID (copy from CLI output) |
| Negative or zero prices | All prices must be positive numbers |

**Debug Commands**:

```bash
# Valid signal payload format:
{
  "position_id": "0193abcd-1234-5678-9012-abcdefghijkl",  # Valid UUID
  "entry_price": 95000,     # Positive number
  "stop_loss": 93000,       # Positive number
  "signal_id": "optional-unique-id"  # Optional, for idempotency
}

# Test with curl (replace POSITION_ID):
POSITION_ID="<valid-uuid-from-cli>"

curl -X POST http://localhost:8080/positions/$POSITION_ID/signal \
  -H "Content-Type: application/json" \
  -d '{
    "position_id": "'$POSITION_ID'",
    "entry_price": 95000,
    "stop_loss": 93000
  }' | jq
```

---

## Cleanup

After completing smoke tests:

```bash
# 1. Stop daemon
pkill -f robsond

# 2. Verify no processes remain
ps aux | grep robsond

# 3. (Optional) Clean up any build artifacts
cargo clean
```

---

## Next Step After Smoke Test

After successfully completing all smoke test cases, the MVP is validated locally. The next steps for operational reliability are:

### Option A: WebSocket Reconnect Hardening

**Scope**: Add connection resilience to market data stream.

**Changes**:
- `robsond/src/market_data.rs`: Add exponential backoff reconnection
- Track connection state (Connecting → Connected → Disconnected)
- Log lifecycle events with tracing

**Effort**: ~100 LOC
**Risk**: Low (maintains existing contracts)
**Impact**: Daemon survives temporary network issues

---

### Option B: Minimal Persistence (PostgreSQL)

**Scope**: Add PostgreSQL repository to survive daemon restarts.

**Changes**:
- `robson-store/src/migrations/`: Create positions table migration
- `robson-store/src/repositories.rs`: Implement PostgresPositionRepository
- `robsond/src/wiring.rs`: Replace MemoryStore with PostgresStore

**Effort**: ~200 LOC + migrations
**Risk**: Medium (adds external dependency)
**Impact**: Positions survive daemon restarts, can recover from crashes

---

## Appendix: Quick Reference

### CLI Commands

```bash
# Arm position
bun run dev arm <SYMBOL> --capital <USD> --risk <%> --side <long|short>

# Check status
bun run dev status

# Disarm position
bun run dev disarm <POSITION_ID>

# Panic (emergency close all)
bun run dev panic --confirm
```

### API Endpoints

```bash
# Health check
GET /health

# List all positions
GET /status

# Arm new position
POST /positions
Body: { "symbol": "BTCUSDT", "side": "long", "capital": 1000, "risk_percent": 1 }

# Inject signal (testing)
POST /positions/:id/signal
Body: { "position_id": "...", "entry_price": 95000, "stop_loss": 93000 }

# Disarm position
DELETE /positions/:id

# Panic mode
POST /panic
Body: { "confirm": true }
```

### State Machine

```
Armed → [detector signal] → Entering → [entry filled] → Active
                                                    ↓ [stop triggered]
                                                 Exiting → [exit filled] → Closed

[disarm] at any state → Closed (cancels pending orders)
[panic] at any state → Closed (emergency exit)
```

---

**Document Status**: ✅ Complete
**Validated Against**: Phase 6b MVP (commit d49d111b)
