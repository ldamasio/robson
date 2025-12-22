# Phase 0: Emergency Dashboard - Architecture-First Implementation Plan

**Status**: Ready for execution
**Priority**: P0 (CRITICAL)
**Timeline**: 10-12 developer-days
**Goal**: Give trader visibility of active position (Operation #1)

---

## Problem Statement

**Robson Bot is in PRODUCTION** with real money at risk:
- Operation #1: 0.00033 BTC @ $88,837.92 (LONG)
- Stop-loss: $87,061.16 (-2%)
- Take-profit: $92,391.44 (+4%)
- Stop Monitor: K8s CronJob running every 1 minute

**CRITICAL ISSUE**: Trader cannot see their position, current price, or P&L!

---

## User Workflows

### Power User (CLI)
```bash
# Check positions
robson positions
# Output: Table with symbol, side, qty, entry, current, P&L, stop, target

# Get current price
robson price BTCUSDC
# Output: Bid/Ask/Spread formatted

# View account summary
robson account
# Output: Balance, positions value, available, exposure
```

**Authentication**: JWT token (client_id embedded) sent to server

---

### Web User (Frontend)
1. **Login** → JWT token stored (contains client_id for multi-tenant)
2. **Dashboard** → See active positions automatically
3. **Strategy Selection Flow** (Future - Phase 2):
   - Select strategy from list (e.g., "BTC Spot Manual")
   - Choose symbol (e.g., BTCUSDC)
   - Click "Activate Monitoring" button
   - System calculates technical stop (2nd support) + position size
   - User confirms entry
   - Operation becomes ACTIVE, monitored by CronJob

**Current Phase 0**: Focus on monitoring EXISTING Operation #1

---

## Architecture Decisions

### 1. WebSocket Strategy
**Decision**: **Defer to Phase 1, use HTTP polling (5s interval) for Phase 0**

**Rationale**:
- `backend/asgi.py` has no Django Channels configuration
- Adding Channels requires: package installation, ASGI routing, Redis channel layer
- Phase 0 is emergency - fastest path to value
- Polling acceptable for single user monitoring
- WebSocket migration in Phase 1 won't break existing components

**Polling Strategy**:
- Frontend: `setInterval(fetchPositions, 5000)` for positions
- Frontend: `setInterval(fetchPrice, 1000)` for current price
- Backend: Endpoints cached with Redis (1s TTL)

**Migration Path**: Replace polling with WebSocket in Phase 1 without component changes (just swap data fetching hook)

---

### 2. Redis Role in Architecture
**Decision**: **Redis for market price caching ONLY (1s TTL)**

**Problem Redis Solves**:
- Binance API has rate limits (~1200 requests/minute)
- Without cache: Every price poll = 1 Binance API call
- With 3 components polling (Position, ActualPrice, Chart) at 1s interval = 3 req/s = 180 req/min per user
- 10 concurrent users = 1800 req/min → **Rate limit exceeded!**

**Solution**:
- Cache market price with 1-second TTL
- Multiple requests within same second = 1 Binance API call (cache hit)
- Cache key: `market_price:{symbol}:{timestamp_floor_to_second}`

**Configuration**:
```python
# Production: Redis
CACHES = {
    'default': {
        'BACKEND': 'django_redis.cache.RedisCache',
        'LOCATION': os.getenv('REDIS_URL', 'redis://127.0.0.1:6379/1'),
        'TIMEOUT': 1,  # 1 second TTL
    }
}

# Development fallback: In-memory (single process only)
CACHES = {
    'default': {
        'BACKEND': 'django.core.cache.backends.locmem.LocMemCache',
        'TIMEOUT': 1,
    }
}
```

**NOT used for** (in Phase 0):
- Session storage (Django default session backend)
- Task queue (not needed yet)
- WebSocket channel layer (deferred to Phase 1)

**Phase 1**: Redis will also serve as Django Channels layer for WebSocket pub/sub

---

### 3. Active Positions Data Model
**Decision**: **Query Operation model, join with Order for entry price**

**Multi-Tenant Filtering**:
```python
# ALWAYS filter by client_id from JWT token
client_id = request.user.client_id

operations = Operation.objects.filter(
    client_id=client_id,  # <-- Multi-tenant isolation
    status='ACTIVE'
).select_related('symbol').prefetch_related('entry_orders')
```

**Data Flow**:
```
1. Frontend/CLI → HTTP GET /api/portfolio/positions/
2. Backend extracts client_id from JWT token
3. Query Operation model (filtered by client_id + status=ACTIVE)
4. For each operation:
   - Get entry order (first FILLED order from entry_orders M2M)
   - Get current price from Redis cache (BinanceMarketData.best_bid)
   - Calculate P&L: (current_price - entry_price) * quantity
   - Calculate distances to stop/target
5. Return JSON with all positions
```

**P&L Calculation**:
```python
from decimal import Decimal, ROUND_HALF_UP

# Get data
entry_price = Decimal('88837.92')  # From entry_order.avg_fill_price
current_price = Decimal('89245.50')  # From BinanceMarketData (cached)
quantity = Decimal('0.00033')  # From entry_order.filled_quantity

# Calculate (full precision internally)
if operation.side == 'BUY':
    pnl = (current_price - entry_price) * quantity
else:  # SELL
    pnl = (entry_price - current_price) * quantity

pnl_percent = (pnl / (entry_price * quantity)) * 100

# Round for JSON output
pnl_rounded = pnl.quantize(Decimal('0.01'), rounding=ROUND_HALF_UP)
pnl_percent_rounded = pnl_percent.quantize(Decimal('0.01'), rounding=ROUND_HALF_UP)

# Convert to string (avoid float precision issues)
response = {
    'unrealized_pnl': str(pnl_rounded),  # "134.50"
    'unrealized_pnl_percent': str(pnl_percent_rounded),  # "0.46"
}
```

---

### 4. API Contracts

#### GET /api/portfolio/positions/
**Authentication**: JWT (client_id extracted from token)

**Response**:
```json
{
  "positions": [
    {
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

**Notes**:
- All prices/amounts: strings with Decimal precision (2 decimals for USD, 8 for crypto)
- `distance_to_stop_percent`: negative = stop is below current (safe for LONG)
- Multi-tenant: Returns only positions for authenticated client_id

---

#### GET /api/market/price/{symbol}/
**Authentication**: JWT (for rate limiting per client)

**Response**:
```json
{
  "symbol": "BTCUSDC",
  "bid": "89245.00",
  "ask": "89246.00",
  "last": "89245.50",
  "timestamp": 1703251200
}
```

**Caching**:
- Redis cache key: `market_price:BTCUSDC:1703251200`
- TTL: 1 second
- Cache decorator: `@cache_page(1)`

---

## Implementation Tasks

### Backend (3 days)

#### Task 1: Positions Endpoint
**File**: `apps/backend/monolith/api/views/portfolio.py` (NEW)

**Checklist**:
- [x] Create view function `active_positions(request)`
- [x] Extract `client_id` from `request.user.client_id`
- [x] Query `Operation.objects.filter(client_id=..., status='ACTIVE')`
- [x] For each operation:
  - [x] Get entry order via `entry_orders.filter(status='FILLED').first()`
  - [x] Get current price from `BinanceMarketData().best_bid(symbol)` (uses cache)
  - [x] Calculate P&L using Decimal arithmetic
  - [x] Calculate stop/target distances
- [x] Return JSON (all values as strings)
- [x] Add `@permission_classes([IsAuthenticated])`
- [x] Add URL route: `path('portfolio/positions/', views.active_positions)`
- [x] Write tests (see Task 12)

**Dependencies**: None (uses existing models)

---

#### Task 2: Price Endpoint
**File**: `apps/backend/monolith/api/views/market_views.py` (EXISTS - add endpoint)

**Checklist**:
- [x] Create view function `current_price(request, symbol)`
- [x] Use `BinanceMarketData().best_bid(symbol)` and `.best_ask(symbol)`
- [x] Add `@cache_page(1)` decorator for Redis caching
- [x] Return JSON with bid/ask/last/timestamp
- [x] Handle errors (invalid symbol, Binance API down)
- [x] Add URL route: `path('market/price/<str:symbol>/', views.current_price)`
- [x] Write tests (see Task 12)

**Dependencies**:
- [x] Add `django-redis==5.4.0` to `requirements.txt`
- [x] Configure `CACHES` in `backend/settings.py`
- [x] Update `.env.example` with `REDIS_URL` (optional for dev)

---

### CLI (2 days)

#### Task 3: `positions` Command
**File**: `cli/cmd/monitoring.go` (NEW)

**Checklist**:
- [x] Create `positionsCmd` using Cobra pattern
- [x] HTTP GET `/api/portfolio/positions/` with JWT auth
- [x] Parse JSON response
- [x] Format table output:
  ```
  ╔════════════════════════════════════╗
  ║        ACTIVE POSITIONS            ║
  ╠════════════════════════════════════╣
  ║ Symbol: BTCUSDC                    ║
  ║ Side: LONG                         ║
  ║ P&L: +$134.50 (+0.46%)             ║
  ╚════════════════════════════════════╝
  ```
- [x] Color P&L (green if >0, red if <0)
- [x] Support `--json` flag for raw output
- [x] Add to `rootCmd` in `cmd/root.go`
- [x] Write tests (see Task 13)

---

#### Task 4: `price` Command
**File**: `cli/cmd/monitoring.go`

**Checklist**:
- [x] Create `priceCmd` with symbol argument
- [x] HTTP GET `/api/market/price/{symbol}/`
- [x] Format output: `BTC/USDC: Bid $89,245.00 | Ask $89,246.00 | Spread $1.00`
- [x] Support `--watch` flag (poll every 1s, clear screen)
- [x] Support `--json` flag
- [x] Write tests

---

#### Task 5: `account` Command
**File**: `cli/cmd/monitoring.go`

**Checklist**:
- [x] HTTP GET `/api/account/balance/` (existing endpoint)
- [x] HTTP GET `/api/portfolio/patrimony/` (existing endpoint)
- [x] HTTP GET `/api/portfolio/positions/` (Task 1)
- [x] Calculate: `available = balance - sum(positions_value)`
- [x] Display summary table
- [x] Support `--json` flag
- [x] Write tests

---

### Frontend (5 days)

#### Task 6: Position Component (Polling)
**File**: `apps/frontend/src/components/logged/Position.jsx`

**Checklist**:
- [x] Create `useEffect` with `setInterval(fetchPositions, 5000)`
- [x] Fetch `/api/portfolio/positions/` with JWT from AuthContext
- [x] Parse and store in state: `const [positions, setPositions] = useState([])`
- [x] Render Bootstrap card for each position:
  - Symbol, side, quantity
  - Entry price, current price
  - P&L with badge (green/red)
  - Stop-loss and take-profit with distance
- [x] Add loading spinner (`LoadingSpinner` component)
- [x] Handle errors with `ErrorBoundary`
- [x] Cleanup interval on unmount
- [x] Write tests (see Task 14)

**Code Snippet**:
```javascript
useEffect(() => {
  const fetchPositions = async () => {
    try {
      const response = await axios.get('/api/portfolio/positions/', {
        headers: { Authorization: `Bearer ${authContext.token}` }
      });
      setPositions(response.data.positions);
    } catch (error) {
      console.error('Failed to fetch positions:', error);
    }
  };

  fetchPositions(); // Initial
  const interval = setInterval(fetchPositions, 5000);
  return () => clearInterval(interval);
}, [authContext.token]);
```

---

#### Task 7: ActualPrice Component (Polling)
**File**: `apps/frontend/src/components/logged/ActualPrice.jsx`

**Checklist**:
- [x] Poll `/api/market/price/BTCUSDC/` every 1 second
- [x] Format price: `$89,245.50` (comma thousands separator)
- [x] Show bid/ask spread: `Spread: $1.00 (0.001%)`
- [x] Track previous price to show direction: ↑ (green) or ↓ (red)
- [x] Cleanup interval on unmount
- [x] Write tests

---

#### Task 8: Chart Component (Recharts)
**File**: `apps/frontend/src/components/logged/Chart.jsx`

**Checklist**:
- [x] Install: `npm install recharts`
- [x] Fetch `/api/historical-data/?symbol=BTCUSDC&interval=15m&days=7`
- [x] Parse OHLCV data into Recharts format
- [x] Render candlestick chart (use Recharts `CandlestickChart` or custom)
- [x] Add reference lines:
  - Entry: `$88,837.92` (blue)
  - Stop: `$87,061.16` (red)
  - Target: `$92,391.44` (green)
- [x] Update chart every 15 minutes (new candle)
- [x] Write tests

**Note**: Phase 2 can upgrade to TradingView Lightweight Charts for better performance

---

#### Task 9: Error Handling Components
**Files**:
- `apps/frontend/src/components/common/ErrorBoundary.jsx` (NEW)
- `apps/frontend/src/components/common/LoadingSpinner.jsx` (NEW)

**Checklist**:
- [x] `ErrorBoundary`: React class component with `componentDidCatch`
- [x] `LoadingSpinner`: Bootstrap spinner component
- [x] Install: `npm install react-toastify` for error notifications
- [x] Wrap Position, ActualPrice, Chart with `ErrorBoundary`
- [x] Show `LoadingSpinner` while fetching data
- [x] Write tests

---

### Testing (2 days)

#### Task 10: Backend Tests
**File**: `apps/backend/monolith/api/tests/test_portfolio.py` (NEW)

**Tests**:
```python
@pytest.mark.django_db
def test_positions_endpoint_returns_correct_data():
    # Create test operation with client_id=1
    # Mock BinanceMarketData.best_bid() to return known price
    # Call endpoint with JWT for client_id=1
    # Assert JSON structure and P&L calculation

@pytest.mark.django_db
def test_positions_multi_tenant_isolation():
    # Create operation for client_id=1 and client_id=2
    # Request as client_id=1
    # Assert only client_id=1 positions returned

@pytest.mark.django_db
def test_market_price_endpoint():
    # Mock Binance API
    # Call endpoint twice within 1 second
    # Assert second call is cache hit (no Binance call)

def test_pnl_calculation_accuracy():
    # Unit test P&L formula
    # Test LONG: (current - entry) * qty
    # Test SHORT: (entry - current) * qty
    # Verify Decimal precision
```

**Coverage Target**: 80%

---

#### Task 11: CLI Tests
**File**: `cli/cmd/monitoring_test.go` (NEW)

**Tests**:
```go
func TestPositionsCommand(t *testing.T) {
    // Mock HTTP server returning positions JSON
    // Run `positions` command
    // Assert table output format
}

func TestPriceCommand(t *testing.T) {
    // Mock price endpoint
    // Run `price BTCUSDC`
    // Assert formatted output
}

func TestJSONFlag(t *testing.T) {
    // Run `positions --json`
    // Assert valid JSON output
}
```

**Coverage Target**: 70%

---

#### Task 12: Frontend Tests
**File**: `apps/frontend/tests/Position.test.jsx` (NEW, etc.)

**Tests**:
```javascript
import { render, waitFor } from '@testing-library/react';
import Position from '../components/logged/Position';

test('Position component fetches and displays data', async () => {
  // Mock axios.get to return positions
  // Render component
  // Assert data is displayed
});

test('Position component polls every 5 seconds', async () => {
  // Use fake timers
  // Mock axios.get
  // Advance timer by 5s
  // Assert second fetch happened
});
```

**Coverage Target**: 60%

---

## Dependencies

### Backend
- `django-redis==5.4.0` (Redis cache backend)

### Frontend
- `recharts` (chart library)
- `react-toastify` (error notifications)

### Infrastructure
- **Dev**: Redis (optional, falls back to in-memory)
- **Prod**: Redis instance (K8s or managed service)

**Redis Setup (Dev)**:
```bash
# Docker
docker run -d -p 6379:6379 redis

# Or docker-compose.yml
services:
  redis:
    image: redis:7-alpine
    ports:
      - "6379:6379"
```

---

## Risks & Mitigations

| Risk | Impact | Mitigation |
|------|--------|------------|
| Redis not available in dev | Can't test caching | Fallback to in-memory cache (functional but slower) |
| Binance API rate limits | Price fetching fails | Redis cache reduces calls by 95%+ |
| P&L calculation errors | Wrong numbers shown | Extensive testing with known values, Decimal precision |
| Polling inefficiency at scale | High server load | Acceptable for Phase 0 (1-10 users), WebSocket in Phase 1 |

---

## Deployment Checklist

### Environment Variables
```bash
# Production (required)
REDIS_URL=redis://redis-service:6379/1

# Development (optional)
REDIS_URL=redis://localhost:6379/1
```

### Configuration Files
- [ ] Update `backend/settings.py` with `CACHES` config
- [ ] Update `.env.example` with `REDIS_URL`
- [ ] Add Redis service to `docker-compose.yml` (dev)
- [ ] Add Redis to K8s manifests (prod)

### Deployment Order
1. Deploy Redis (if not running)
2. Deploy backend (new endpoints)
3. Deploy CLI (new commands)
4. Deploy frontend (new components)

### Smoke Tests
```bash
# API
curl -H "Authorization: Bearer $TOKEN" https://api.robson/api/portfolio/positions/

# CLI
robson positions

# Frontend
# Login → Dashboard → Verify positions visible
```

---

## Success Criteria

### Functionality
- [ ] CLI: `robson positions` shows Operation #1 with accurate P&L
- [ ] CLI: `robson price BTCUSDC` returns current price in <1s
- [ ] Frontend: Position component displays and updates every 5s
- [ ] Frontend: Price updates every 1s
- [ ] Frontend: Chart shows 15min candles with entry/stop/target

### Performance
- [ ] API response time <500ms (p95)
- [ ] Redis cache hit rate >95% for price endpoint
- [ ] Chart renders in <200ms
- [ ] Frontend loads in <3s

### Quality
- [ ] Backend test coverage ≥80%
- [ ] CLI test coverage ≥70%
- [ ] Frontend test coverage ≥60%
- [ ] Zero lint errors (Pylint, ESLint, golint)
- [ ] All CI checks passing

---

## Timeline

**Total**: 10-12 developer-days

**Day 1-3**: Backend (Tasks 1-2 + dependencies + tests)
**Day 4-5**: CLI (Tasks 3-5 + tests)
**Day 6-10**: Frontend (Tasks 6-9 + tests)
**Day 11-12**: Integration testing + deployment

**Critical Path**: Backend → CLI → Frontend (sequential)

**Parallelization**: Frontend can start after Backend Day 2 (API contracts frozen)

---

## Migration to Phase 1 (WebSocket)

**Changes Required**:
- [ ] Install: `channels`, `daphne`, `channels-redis`
- [ ] Configure `backend/asgi.py` with WebSocket routing
- [ ] Implement `/ws/prices/` consumer
- [ ] Add Redis channel layer config
- [ ] Frontend: Replace polling with `usePriceWebSocket` hook
- [ ] No component UI changes needed

**Backward Compatibility**: Keep HTTP polling endpoints, deprecate in Phase 2

---

## Next Steps

1. **Review this plan** with stakeholders
2. **Set up Redis** (dev + prod environments)
3. **Create GitHub issues** for 12 tasks
4. **Start Task 1** (Positions endpoint)
5. **Deploy to staging** after backend completion
6. **Final integration test** before production

---

**Plan Status**: ✅ Ready for execution
**Last Updated**: 2025-12-22
**Owner**: Development Team
