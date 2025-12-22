# Critical Requirements: Production Dashboard

**Status**: Bot in PRODUCTION - Operation #1 ACTIVE
**Priority**: P0 (CRITICAL)
**Date**: 2025-12-22

---

## Business Context

**Robson Bot is NOW IN PRODUCTION** with a live BTC position:
- **Operation #1**: 0.00033 BTC @ $88,837.92 (LONG)
- **Stop-Loss**: $87,061.16 (-2%)
- **Take-Profit**: $92,391.44 (+4%)
- **Status**: ACTIVE, monitored by K8s CronJob

**PROBLEM**: User cannot see their position or monitor it visually!

---

## Critical User Needs (Must Have)

### 1. Position Visibility (P0)
**Requirement**: User MUST be able to see active positions
**Acceptance Criteria**:
- Display entry price, current price, quantity
- Show unrealized P&L (both $ and %)
- Update automatically (WebSocket or polling)
- Show stop-loss and take-profit levels
- Visual distance to stop (e.g., "15.3% to stop")

**Rationale**: User has REAL MONEY at risk and needs visibility

**Implementation**:
- CLI: `robson positions` command
- Frontend: Position component with live updates
- Backend: `GET /api/portfolio/positions/` endpoint

---

### 2. Real-Time Price Updates (P0)
**Requirement**: User MUST see current market price
**Acceptance Criteria**:
- Display current BTC/USDC price
- Update every second (or real-time via WebSocket)
- Show bid/ask spread
- Format properly (e.g., "$89,245.50")
- Visual indicator if price is moving

**Rationale**: Critical for manual decision-making

**Implementation**:
- CLI: `robson price BTCUSDC` command
- Frontend: Real-time price ticker component
- Backend: WebSocket `/ws/prices/` + REST fallback

---

### 3. Chart Visualization (P0)
**Requirement**: User MUST see 15min candlestick chart
**Acceptance Criteria**:
- Display last 100 candles (15min timeframe)
- Show entry price line
- Show stop-loss level line
- Show take-profit level line
- Optional: Show support/resistance levels

**Rationale**: Technical analysis is CORE to stop calculation rule

**Implementation**:
- Frontend: TradingView Lightweight Charts
- Backend: `GET /api/market/klines/<symbol>/` endpoint
- Data source: Binance get_klines() method (already implemented)

---

### 4. Operations History (P1)
**Requirement**: User SHOULD see all past operations
**Acceptance Criteria**:
- Table with all operations (active + closed)
- Filters: date range, symbol, side, status
- Sortable columns
- Pagination (if >50 operations)
- Export to CSV

**Rationale**: Audit trail and performance analysis

**Implementation**:
- CLI: `robson operations list` command
- Frontend: Operations tab with table
- Backend: `GET /api/operations/history/` endpoint

---

### 5. Account Overview (P1)
**Requirement**: User SHOULD see account summary
**Acceptance Criteria**:
- Total balance (USDC)
- Total portfolio value (all assets)
- Available balance (not in positions)
- Total P&L (realized + unrealized)
- Number of active positions

**Rationale**: Portfolio management and risk awareness

**Implementation**:
- CLI: `robson account` command
- Frontend: Balance + Patrimony components (already exist, enhance)
- Backend: `GET /api/portfolio/summary/` endpoint

---

## CORE Business Rules (MUST ENFORCE)

### Rule #1: Technical Stop Calculation
**Rule**: ALL future operations MUST calculate stop from 2nd technical event (15min chart)

**Enforcement Points**:
1. CLI `plan` command auto-calculates stop
2. Frontend entry form shows calculated stop
3. Backend validation REJECTS if stop not set
4. Database constraint: stop_loss_price NOT NULL

**Failure Mode**: REJECT operation creation if stop not calculated

---

### Rule #2: Position Size from Stop Distance
**Rule**: Position size MUST be calculated from stop distance (1% risk)

**Enforcement Points**:
1. CLI shows calculated size, user confirms
2. Frontend auto-calculates and displays
3. Backend recalculates in execution step
4. Reject if user-provided size deviates >5% from calculated

**Failure Mode**: REJECT execution if position size incorrect

---

### Rule #3: No Execution Without Validation
**Rule**: Execute step REQUIRES successful validation

**Enforcement Points**:
1. CLI: `--validated` flag required (set by validate command)
2. Backend: Check `validation_passed` flag
3. Frontend: Disable execute button until validated

**Failure Mode**: HARD ERROR if attempted without validation

---

## Technical Requirements

### Performance
- **Price updates**: <100ms latency (WebSocket)
- **Chart rendering**: <200ms for 100 candles
- **API response**: <500ms for all endpoints
- **WebSocket reconnect**: <5s with exponential backoff

### Reliability
- **Uptime**: 99.9% (allows 43min downtime/month)
- **Data accuracy**: 100% (no price/position errors)
- **WebSocket recovery**: Auto-reconnect with state preservation
- **API failover**: Fallback to HTTP polling if WebSocket fails

### Security
- **Authentication**: JWT required for all endpoints
- **Multi-tenancy**: client_id filter on all queries
- **No cross-client data**: Database-level isolation
- **Rate limiting**: 100 req/min per client

### Scalability
- **Concurrent users**: Support 100+ simultaneous
- **WebSocket connections**: 1000+ connections
- **Database queries**: <50ms avg response time
- **Chart data**: Cache klines with 1min TTL

---

## User Experience Requirements

### CLI
**Expectations**:
- Commands respond in <2 seconds
- JSON output available for all commands (`--json`)
- Human-readable default output
- Clear error messages

**Example**:
```bash
$ robson positions

╔════════════════════════════════════════════════════════════╗
║                    ACTIVE POSITIONS                        ║
╠════════════════════════════════════════════════════════════╣
║ Symbol: BTCUSDC                                            ║
║ Side: LONG                                                 ║
║ Quantity: 0.00033 BTC                                      ║
║ Entry: $88,837.92                                          ║
║ Current: $89,245.50  (+0.46%)                              ║
║ P&L: +$134.50  (+0.46%)                                    ║
║ Stop: $87,061.16  (-15.3% away)                            ║
║ Target: $92,391.44  (+3.5% to go)                          ║
╚════════════════════════════════════════════════════════════╝
```

### Frontend
**Expectations**:
- Mobile-responsive (Bootstrap grid)
- Accessibility (WCAG 2.1 AA)
- Loading states for all async operations
- Error boundaries for graceful failures
- Keyboard navigation support

**Key Screens**:
1. **Dashboard**: Overview + positions + prices
2. **Operations**: History table with filters
3. **Charts**: Full-screen chart with analysis
4. **Settings**: Risk configuration, alerts

---

## Data Model Requirements

### Position Entity
```python
class Position:
    id: int
    operation_id: int           # FK to Operation
    symbol: Symbol              # e.g., BTCUSDC
    side: str                   # BUY or SELL
    quantity: Decimal           # e.g., 0.00033
    entry_price: Decimal        # e.g., 88837.92
    current_price: Decimal      # COMPUTED (from market data)
    unrealized_pnl: Decimal     # COMPUTED
    unrealized_pnl_percent: Decimal  # COMPUTED
    stop_loss: Decimal          # e.g., 87061.16
    take_profit: Decimal        # e.g., 92391.44
    status: str                 # OPEN, CLOSED
```

### Portfolio Summary
```python
class PortfolioSummary:
    total_balance: Decimal           # All assets in USDC
    available_balance: Decimal       # Not in positions
    positions_value: Decimal         # Value of all positions
    unrealized_pnl: Decimal          # Total unrealized P&L
    realized_pnl: Decimal            # Total realized P&L
    total_pnl: Decimal               # realized + unrealized
    num_positions: int               # Count of active positions
    exposure_percent: Decimal        # positions_value / total_balance
```

---

## API Contracts

### GET /api/portfolio/positions/
**Request**: None (uses JWT for client_id)
**Response**:
```json
{
  "positions": [
    {
      "id": 1,
      "operation_id": 1,
      "symbol": "BTCUSDC",
      "side": "BUY",
      "quantity": "0.00033",
      "entry_price": "88837.92",
      "current_price": "89245.50",
      "unrealized_pnl": "134.50",
      "unrealized_pnl_percent": "0.46",
      "stop_loss": "87061.16",
      "take_profit": "92391.44",
      "distance_to_stop_percent": "-15.3",
      "distance_to_target_percent": "3.5",
      "status": "OPEN"
    }
  ]
}
```

### GET /api/market/price/{symbol}/
**Request**: Path param `symbol` (e.g., BTCUSDC)
**Response**:
```json
{
  "symbol": "BTCUSDC",
  "bid": "89245.00",
  "ask": "89246.00",
  "last": "89245.50",
  "timestamp": 1703251200,
  "source": "binance"
}
```

### WS /ws/prices/
**Subscribe Message**:
```json
{
  "action": "subscribe",
  "symbols": ["BTCUSDC"]
}
```

**Price Update Message**:
```json
{
  "type": "price_update",
  "symbol": "BTCUSDC",
  "price": "89245.50",
  "timestamp": 1703251200
}
```

---

## Testing Requirements

### Critical Paths (Must Test)
1. Position display with correct P&L calculation
2. Real-time price updates (WebSocket)
3. Chart rendering with 100+ candles
4. Technical stop calculation enforcement
5. Position size validation

### Test Coverage Targets
- **Backend**: 80% coverage (pytest)
- **Frontend**: 60% coverage (Vitest)
- **CLI**: 70% coverage (Go tests)
- **E2E**: Critical flows (Playwright)

### Test Data
- Use Operation #1 as reference (real production data)
- Mock Binance API for consistent test results
- Create fixtures for 10 historical operations

---

## Deployment Requirements

### Release Checklist
- [ ] All tests passing (CI green)
- [ ] Manual QA on staging environment
- [ ] Database migrations reviewed
- [ ] Environment variables documented
- [ ] Rollback plan prepared
- [ ] Monitoring/alerts configured

### Rollout Strategy
- **Phase 1**: Deploy backend API (non-breaking)
- **Phase 2**: Deploy CLI (backward compatible)
- **Phase 3**: Deploy frontend (feature flag if needed)
- **Monitoring**: Check error rates for 24h post-deploy

---

## Success Criteria

### Phase 0 (Week 1)
- [ ] User can view active positions (CLI + UI)
- [ ] Real-time price updates working
- [ ] Basic 15min chart visible
- [ ] P&L calculation accurate

### Phase 1 (Week 2)
- [ ] Operations history accessible
- [ ] Risk metrics displayed
- [ ] WebSocket stable (no disconnects)
- [ ] Order status tracking working

### Phase 2 (Week 3)
- [ ] Technical stop calculation integrated
- [ ] Chart shows support/resistance
- [ ] Position size auto-calculated
- [ ] 1% rule enforced

---

## Non-Functional Requirements

### Availability
- **Target**: 99.9% uptime
- **Maintenance**: Max 30min/week scheduled downtime
- **Recovery**: <5min RTO (Recovery Time Objective)

### Monitoring
- **Metrics**: Prometheus + Grafana
- **Logs**: Centralized (ELK or similar)
- **Alerts**: Slack/PagerDuty for critical errors
- **Dashboards**: System health, API latency, error rates

### Documentation
- **User Guide**: CLI usage, UI navigation
- **API Docs**: OpenAPI spec (Swagger)
- **Developer Docs**: Architecture, deployment
- **Runbooks**: Common issues, troubleshooting

---

**Status**: Ready for implementation
**Owner**: Development team
**Timeline**: 4 weeks to full production dashboard
**Dependencies**: None (all backend primitives exist)
