# Phase 0.5: Code Discovery Findings

**Date**: 2025-12-28
**Scope**: Derivatives infrastructure for Market Research & Context Engine (Core 2)
**Status**: COMPLETE

---

## Executive Summary

**Finding**: No existing derivatives data collection infrastructure found. BinanceService exists but lacks methods for funding rate, open interest, or mark price. Django models for derivatives metrics do not exist.

**Recommendation**: **Option C - Create Minimal New Adapter** (extend existing BinanceService pattern)

**Rationale**:
- BinanceService provides singleton client management and testnet/production mode switching
- MarketDataService demonstrates the pattern: wrap BinanceService for domain-specific data
- No refactoring required; additive approach aligns with existing architecture
- Minimal disruption; follows established conventions

---

## Discovery Results

### 1. Exchange Integration Layer

#### BinanceService (Singleton Client Wrapper)

**Location**: `apps/backend/monolith/api/services/binance_service.py`

**What Exists**:
- ✅ Singleton pattern for Binance client management (ADR-0001)
- ✅ Testnet/production mode switching (`BINANCE_USE_TESTNET` setting)
- ✅ Credential management (K8s secrets or environment variables)
- ✅ Basic methods: `ping()`, `get_server_time()`, `get_account_info()`

**What Does NOT Exist**:
- ❌ No `get_funding_rate()` method
- ❌ No `get_open_interest()` method
- ❌ No `get_mark_price()` method
- ❌ No futures/derivatives-specific methods

**Code Snippet** (existing structure):
```python
class BinanceService:
    """Singleton service for Binance API interaction."""
    _instance = None
    _client = None
    _current_testnet_mode = None

    def __init__(self, use_testnet: bool = None):
        # Lazy-initialize client with testnet/production mode
        pass

    @property
    def client(self):
        """Returns python-binance Client instance."""
        # Returns: binance.client.Client
        pass
```

**Conclusion**: BinanceService provides the infrastructure (client, credentials, mode switching) but lacks derivatives data methods.

---

#### BinanceMarginAdapter (Order Execution Only)

**Location**: `apps/backend/monolith/api/application/margin_adapters.py`

**What Exists**:
- ✅ Isolated margin order execution (transfers, buy/sell orders)
- ✅ Implements `MarginExecutionPort` (hexagonal adapter)
- ✅ Production/testnet mode awareness

**What Does NOT Exist**:
- ❌ No market data collection methods
- ❌ Focused on order execution, not data fetching

**Conclusion**: BinanceMarginAdapter is for trading execution, not data collection.

---

#### MarketDataService (Reference Pattern)

**Location**: `apps/backend/monolith/api/services/market_data_service.py`

**What Exists**:
- ✅ Wraps BinanceService for historical klines (OHLCV data)
- ✅ Uses `self.binance.client.get_historical_klines()`
- ✅ Implements caching (5-minute TTL)
- ✅ Returns data as pandas DataFrame → JSON

**Pattern Demonstrated**:
```python
class MarketDataService:
    def __init__(self):
        self.binance = BinanceService()  # Wrap singleton

    def get_historical_data(self, symbol, interval, days):
        # Call binance.client.get_historical_klines()
        # Process with pandas
        # Cache and return
        pass
```

**Conclusion**: This is the **established pattern** for adding new data collection services. Extend this approach for derivatives data.

---

### 2. Persistence Layer (Django Models)

#### Existing Models

**Location**: `apps/backend/monolith/api/models/`

**What Exists**:
- ✅ `MarginPosition` (`margin.py`) - Tracks open margin positions
- ✅ `AuditTransaction` (`audit.py`) - Audit trail for all financial movements
- ✅ `StopEvent` (`event_sourcing.py`) - Event sourcing for stop-loss executions
- ✅ `Symbol`, `Strategy`, `Operation`, `Trade` (`trading.py`) - Core trading entities

**What Does NOT Exist**:
- ❌ No `MetricPoint` or similar model for time-series market data
- ❌ No `FundingRate`, `OpenInterest`, `MarkPrice` models
- ❌ No derivatives data persistence

**Conclusion**: Need to create new Django models for derivatives metrics (MetricPoint, FeatureVector, MarketContextSnapshot).

---

### 3. Job Execution Patterns

#### Django Management Commands (Standard Pattern)

**Location**: `apps/backend/monolith/api/management/commands/`

**What Exists**:
- ✅ 20+ management commands for various tasks:
  - `monitor_stops.py` - Continuous stop-loss monitoring (cronjob-ready)
  - `sync_transactions.py` - Sync trades from Binance
  - `isolated_margin_buy.py` - Execute margin trades
  - `status.py` - Account status checks
  - `positions.py` - List margin positions

**Pattern Demonstrated**:
```python
class Command(BaseCommand):
    def add_arguments(self, parser):
        parser.add_argument('--continuous', action='store_true')
        parser.add_argument('--interval', type=int, default=60)

    def handle(self, *args, **options):
        if options['continuous']:
            while True:
                # Do work
                time.sleep(options['interval'])
        else:
            # Single run
            pass
```

**Conclusion**: Django management commands are the standard pattern for scheduled/background tasks.

---

#### Kubernetes CronJobs (NOT Found)

**Search Results**:
```bash
find infra/k8s/**/cronjobs/*.yaml
# Result: No files found
```

**Conclusion**: Kubernetes CronJobs are NOT the current deployment pattern. Management commands are run via alternative schedulers (likely cron, systemd timers, or manual invocation).

---

### 4. Hexagonal Architecture Patterns

#### Application Layer Structure

**Location**: `apps/backend/monolith/api/application/`

**What Exists**:
- ✅ `domain.py` - Lightweight domain entities (Symbol value object)
- ✅ `ports.py` - Protocol interfaces (MarginExecutionPort, etc.)
- ✅ `use_cases.py` - Business logic (use cases)
- ✅ `adapters.py` - Django/Binance implementations
- ✅ `wiring.py` - Dependency injection

**Pattern for New Feature**:
1. Define domain entities in `domain.py` (NO Django dependencies)
2. Define ports in `ports.py` (Protocol interfaces)
3. Implement use cases in `use_cases.py` (business logic)
4. Implement adapters in `adapters.py` (Django models, Binance calls)

**Conclusion**: Follow established hexagonal pattern. Create `api/application/market_context/` subdirectory for new core.

---

## Recommended Integration Approach

### **Option C: Create Minimal New Adapter** (SELECTED)

**Approach**: Add a new service (`DerivativesDataService`) following the `MarketDataService` pattern.

**Justification**:
1. **BinanceService exists** but lacks derivatives methods → extend via new service wrapper
2. **Established pattern** (`MarketDataService`) demonstrates how to wrap BinanceService
3. **Additive approach** → no refactoring of existing code required
4. **Follows hexagonal architecture** → service wraps adapter, adapter wraps Binance client
5. **Minimal disruption** → isolated to new files/modules

**Implementation**:
```python
# NEW: apps/backend/monolith/api/services/derivatives_data_service.py
class DerivativesDataService:
    """Service for collecting derivatives data (funding rate, OI, mark price)."""

    def __init__(self):
        self.binance = BinanceService()

    def get_funding_rate(self, symbol: str) -> dict:
        """Fetch current funding rate for perpetual futures."""
        return self.binance.client.futures_funding_rate(symbol=symbol)

    def get_open_interest(self, symbol: str) -> dict:
        """Fetch current open interest for symbol."""
        return self.binance.client.futures_open_interest(symbol=symbol)

    def get_mark_price(self, symbol: str) -> dict:
        """Fetch current mark price for perpetual futures."""
        return self.binance.client.futures_mark_price(symbol=symbol)
```

**Then wrap in hexagonal adapter**:
```python
# NEW: apps/backend/monolith/api/application/market_context/adapters.py
class BinanceDerivativesAdapter:
    """Adapter for fetching derivatives data from Binance."""

    def __init__(self):
        self.service = DerivativesDataService()

    def collect_metrics(self, symbol: str) -> list[MetricPoint]:
        """Collect all derivatives metrics and normalize to MetricPoint domain entities."""
        funding = self.service.get_funding_rate(symbol)
        oi = self.service.get_open_interest(symbol)
        mark = self.service.get_mark_price(symbol)

        # Normalize to MetricPoint domain entities
        return [
            MetricPoint(timestamp=..., metric_name="funding_rate", value=..., source="binance_futures"),
            MetricPoint(timestamp=..., metric_name="open_interest", value=..., source="binance_futures"),
            MetricPoint(timestamp=..., metric_name="mark_price", value=..., source="binance_futures"),
        ]
```

---

### Alternative Options Considered (NOT Selected)

#### Option A: Wrap Existing Collector

**Status**: NOT APPLICABLE

**Reason**: No existing derivatives data collector found. BinanceService has no relevant methods.

---

#### Option B: Extend BinanceService Directly

**Status**: REJECTED

**Reasons**:
1. ❌ Violates single responsibility (BinanceService is thin client wrapper)
2. ❌ Mixing concerns (spot trading + derivatives data in same class)
3. ❌ Existing pattern (`MarketDataService`) shows separation is preferred

---

## File Paths Reference

### Existing Infrastructure

| Component | File Path |
|-----------|-----------|
| **BinanceService** | `apps/backend/monolith/api/services/binance_service.py` |
| **MarketDataService** | `apps/backend/monolith/api/services/market_data_service.py` |
| **BinanceMarginAdapter** | `apps/backend/monolith/api/application/margin_adapters.py` |
| **Management Commands** | `apps/backend/monolith/api/management/commands/` |
| **Trading Models** | `apps/backend/monolith/api/models/trading.py` |
| **Margin Models** | `apps/backend/monolith/api/models/margin.py` |
| **Audit Models** | `apps/backend/monolith/api/models/audit.py` |
| **Application Domain** | `apps/backend/monolith/api/application/domain.py` |
| **Application Ports** | `apps/backend/monolith/api/application/ports.py` |

---

### New Files to Create (Milestone 1)

| Component | File Path |
|-----------|-----------|
| **DerivativesDataService** | `apps/backend/monolith/api/services/derivatives_data_service.py` |
| **Market Context Models** | `apps/backend/monolith/api/models/market_context.py` |
| **Domain Entities** | `apps/backend/monolith/api/application/market_context/domain.py` |
| **Ports** | `apps/backend/monolith/api/application/market_context/ports.py` |
| **Adapters** | `apps/backend/monolith/api/application/market_context/adapters.py` |
| **Use Cases** | `apps/backend/monolith/api/application/market_context/use_cases.py` |
| **Management Command** | `apps/backend/monolith/api/management/commands/collect_derivatives_metrics.py` |
| **Unit Tests** | `apps/backend/monolith/api/tests/test_market_context.py` |

---

## Deployment Pattern Findings

### Current Standard: Django Management Commands

**Evidence**:
- ✅ 20+ management commands exist for scheduled tasks
- ✅ `monitor_stops.py` has `--continuous` mode for loop-based execution
- ✅ Commands support both single-run and continuous modes

**Pattern**:
```bash
# Single run
python manage.py monitor_stops

# Continuous (loop mode)
python manage.py monitor_stops --continuous --interval 60
```

**Deployment Options** (based on existing pattern):
1. **Cron** (most likely):
   ```cron
   */1 * * * * cd /app && python manage.py collect_derivatives_metrics --symbol BTCUSDT
   ```

2. **Systemd Timer** (alternative):
   ```ini
   [Unit]
   Description=Collect Derivatives Metrics

   [Timer]
   OnCalendar=*:0/1

   [Service]
   ExecStart=/app/venv/bin/python manage.py collect_derivatives_metrics --symbol BTCUSDT
   ```

3. **Kubernetes CronJob** (NOT current pattern, but possible):
   - No evidence of existing CronJobs in `infra/k8s/`
   - If added later, use APPENDIX examples from IMPLEMENTATION-PLAN.md

---

## Dependency Verification: python-binance

### Status: ✅ ACTIVE (VERIFIED)

**Version**: `python-binance==1.0.16` (declared in `apps/backend/monolith/requirements.txt:66`)

**Runtime Usage**: ✅ **ACTIVE** - Found 5 active imports in runtime code (NOT dead code):
- `apps/backend/monolith/api/application/margin_adapters.py:16` - BinanceMarginAdapter
- `apps/backend/monolith/api/application/adapters.py:30` - Active adapter
- `apps/backend/monolith/api/service.py:2` - Service module
- `apps/backend/monolith/api/views.py:5` - Views module
- `apps/backend/monolith/api/services/__init__.py:8` - Package export

**Client Access Pattern**: ✅ **VERIFIED**
- BinanceService wraps `binance.client.Client` as singleton (line 97: `client_cls(api_key, secret_key, testnet=is_testnet)`)
- BinanceMarginAdapter successfully uses Client for margin operations (transfer, orders, account queries)
- Client supports testnet/production mode switching via `testnet` parameter

### Futures API Methods Availability

**Verification Method**: Cross-referenced python-binance package documentation + Binance official REST API endpoints

| Method | Status | Binance REST Endpoint | Verified |
|--------|--------|----------------------|----------|
| `client.futures_funding_rate(symbol, **params)` | ✅ AVAILABLE | `GET /fapi/v1/fundingRate` | YES |
| `client.futures_open_interest(symbol)` | ✅ AVAILABLE | `GET /fapi/v1/openInterest` | YES |
| `client.futures_mark_price(symbol)` | ✅ AVAILABLE | `GET /fapi/v1/premiumIndex` | YES |

**Key Finding**: The `futures_mark_price()` method returns BOTH mark price AND current funding rate in a single API call:

```python
# Response from client.futures_mark_price(symbol="BTCUSDT"):
{
    "symbol": "BTCUSDT",
    "markPrice": "95000.00",           # Current mark price
    "indexPrice": "94995.50",          # Index price
    "lastFundingRate": "0.0001",       # ⭐ Current funding rate (included!)
    "nextFundingTime": 1640000000000,  # Next funding timestamp
    "time": 1639999900000              # Response timestamp
}
```

**Optimization Opportunity**: Can fetch mark price + funding rate with a single API call instead of two separate calls.

### Existing Usage Evidence

**Margin API Methods** (verified in `margin_adapters.py`):
- ✅ `client.transfer_spot_to_isolated_margin()` (line 179)
- ✅ `client.transfer_isolated_margin_to_spot()` (line 240)
- ✅ `client.get_isolated_margin_account()` (line 292)
- ✅ `client.create_margin_order()` (line 383)
- ✅ `client.cancel_margin_order()` (line 456)
- ✅ `client.get_open_margin_orders()` (line 496)

**Conclusion**: The Client instance provided by python-binance v1.0.16 is actively used for margin trading operations. Futures methods follow the same pattern and are confirmed available via library documentation and Binance API endpoint verification.

### No Existing Futures Data Collection

**Search Results**: ❌ NO existing usage of futures methods found
```bash
grep -r "futures_funding_rate\|futures_open_interest\|futures_mark_price" apps/backend/
# Result: No matches
```

**Conclusion**: Derivatives data collection is NOT currently implemented. All futures methods are available but unused.

### ✅ RECOMMENDATION: PROCEED with DerivativesDataService Implementation

**Status**: **ALL CHECKS PASSED** - Safe to implement DerivativesDataService

**Verified Pre-Conditions**:
1. ✅ python-binance v1.0.16 is ACTIVE (not legacy/unused)
2. ✅ Client is actively used for margin trading (proven pattern)
3. ✅ Futures methods confirmed available (library + API docs)
4. ✅ BinanceService singleton provides Client access
5. ✅ No existing derivatives collection to conflict with
6. ✅ Testnet/production mode switching already implemented

**Implementation Confidence**: **HIGH** - All required infrastructure exists and is actively maintained.

**Next Steps**: Proceed to Milestone 1 implementation following the established patterns:
- Create `DerivativesDataService` wrapping `BinanceService.client`
- Use `futures_mark_price(symbol)` for mark price + funding rate (single call optimization)
- Use `futures_open_interest(symbol)` for open interest
- Follow `MarketDataService` pattern (service wrapper → adapter → use case)

---

## Python-Binance Client Methods Available

### Futures API Methods (Confirmed Available)

Based on `python-binance` v1.0.16 + Binance REST API verification:

| Method | Description | Returns |
|--------|-------------|---------|
| `client.futures_funding_rate(symbol)` | Get funding rate history | `[{"fundingRate": "0.0001", "fundingTime": 1234567890, "symbol": "BTCUSDT"}]` |
| `client.futures_open_interest(symbol)` | Get current open interest | `{"openInterest": "12345.67", "symbol": "BTCUSDT", "time": 1234567890}` |
| `client.futures_mark_price(symbol)` | Get mark price + funding rate | `{"markPrice": "95000.00", "lastFundingRate": "0.0001", "nextFundingTime": 1640000000000, "time": 1639999900000}` |

**Note**: These methods are available on the `Client` instance wrapped by `BinanceService.client`.

---

## Security & Credentials

### API Keys Configuration

**Current Pattern** (from `binance_service.py`):
```python
# Testnet credentials
BINANCE_API_KEY_TEST = settings.BINANCE_API_KEY_TEST
BINANCE_SECRET_KEY_TEST = settings.BINANCE_SECRET_KEY_TEST

# Production credentials
BINANCE_API_KEY = settings.BINANCE_API_KEY
BINANCE_SECRET_KEY = settings.BINANCE_SECRET_KEY

# Mode switch
BINANCE_USE_TESTNET = settings.BINANCE_USE_TESTNET  # Default: True
```

**Conclusion**: Credentials managed via Django settings (environment variables or K8s secrets). No additional configuration required for derivatives data collection.

---

## Next Steps (Milestone 1 Implementation)

Based on these findings, proceed with Milestone 1:

1. **Create Django Model** (`api/models/market_context.py`):
   - `MetricPoint` with unique constraint on `(source, symbol, metric_name, timestamp)`

2. **Create Domain Entity** (`api/application/market_context/domain.py`):
   - Immutable `MetricPoint` dataclass (NO Django dependencies)

3. **Create Service** (`api/services/derivatives_data_service.py`):
   - Wrap BinanceService, call `futures_funding_rate()`, `futures_open_interest()`, `futures_mark_price()`

4. **Create Adapter** (`api/application/market_context/adapters.py`):
   - `BinanceDerivativesAdapter` wraps service, normalizes to `MetricPoint` domain entities
   - `DjangoMetricRepository` persists `MetricPoint` to database (idempotent upsert)

5. **Create Use Case** (`api/application/market_context/use_cases.py`):
   - `CollectDerivativesMetrics` orchestrates: adapter → repository

6. **Create Management Command** (`api/management/commands/collect_derivatives_metrics.py`):
   - Follow `monitor_stops.py` pattern (supports `--continuous` mode)

7. **Write Unit Tests** (`api/tests/test_market_context.py`):
   - Test domain entity creation
   - Test repository idempotency
   - Test adapter normalization (mocked Binance API)

8. **Manual Validation**:
   ```bash
   # Single run
   python manage.py collect_derivatives_metrics --symbol BTCUSDT

   # Verify in database
   python manage.py shell
   >>> from api.models.market_context import MetricPoint
   >>> MetricPoint.objects.filter(symbol='BTCUSDT').count()
   3  # funding_rate, open_interest, mark_price
   ```

---

## Risk Assessment

### Low Risk

✅ **Additive approach**: No refactoring of existing code
✅ **Follows established patterns**: MarketDataService, management commands, hexagonal architecture
✅ **Isolated to new modules**: No changes to BinanceService, BinanceMarginAdapter, or core trading logic
✅ **Testnet support**: Can test with testnet before production deployment

### Medium Risk

⚠️ **Binance API rate limits**: Futures API has different rate limits than spot API (need to validate)
⚠️ **Data freshness**: If collection fails, stale data could affect context snapshots (mitigated by freshness monitor in Milestone 4)

### Mitigations

- Start with BTC/USDT only (single symbol)
- Collect every 60 seconds (well below rate limits)
- Implement exponential backoff on API errors
- Add freshness monitoring (Milestone 4)

---

## Conclusion

**Status**: Phase 0.5 COMPLETE

**Recommendation**: **Proceed to Milestone 1** with Option C (Create Minimal New Adapter)

**Confidence**: HIGH (all required infrastructure exists; established patterns identified; no blockers)

**Next Action**: Implement Milestone 1 tasks as outlined in IMPLEMENTATION-PLAN.md

---

**Prepared by**: Claude Code
**Date**: 2025-12-28
**Version**: 1.0.0
