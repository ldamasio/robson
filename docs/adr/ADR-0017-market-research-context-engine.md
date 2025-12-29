# ADR-0017: Market Research & Context Engine

**Status**: Proposed
**Date**: 2025-12-28
**Deciders**: Development Team
**Related**: ADR-0007 (Risk Management Assistant), ADR-0002 (Hexagonal Architecture)

---

## Context

Robson Bot currently excels at **position sizing** and **risk management** (the first core), but lacks structured market context awareness. Users must manually interpret:

- Market conditions (choppy vs trending, high vol vs low vol)
- Funding rate extremes (potential squeeze risk)
- Open Interest deltas (smart money positioning)
- Stop vulnerability (are stops clustered?)

This context could inform future decision boundaries (e.g., entry gates, position management) but is currently absent from the system.

### Verified Facts vs Assumptions (as of 2025-12-28)

**Verified (Known to Exist)**:
- ✅ Isolated margin trading logic exists in codebase
- ✅ Stop-loss execution logic exists (`monitor_stops.py`)
- ✅ Django models for trading operations (`Operation`, `Position`, `Trade`)
- ✅ Binance API integration exists (`BinanceService` singleton)
- ✅ Hexagonal architecture patterns established (domain, ports, adapters)

**Assumptions (Not Yet Verified)**:
- ⏳ Derivatives data collection: May exist partially, legacy, or not at all
- ⏳ Funding rate persistence: Unknown if currently stored
- ⏳ Open interest tracking: Unknown if currently stored
- ⏳ Mark price collection: Unknown if currently stored
- ⏳ Deployment patterns: Kubernetes CronJobs assumed but not confirmed for this use case

**Out of Scope for This Phase**:
- ❌ EntryGate integration (deferred to future Core 1.1)
- ❌ PositionManager integration (separate concern)
- ❌ Production deployment timelines
- ❌ On-chain data collection (optional future enhancement)

### Problem Statement

**Users need market context to make informed decisions, but Robson doesn't provide it.**

Current state:
- ❌ No funding rate analysis available to users
- ❌ No open interest tracking visible in system
- ❌ No volatility regime classification
- ❌ No squeeze risk detection
- ❌ Manual interpretation of raw derivatives data (if accessible at all)

Desired state (for this phase):
- ✅ Normalized derivatives metrics storage (canonical persistence layer)
- ✅ Computed features (z-scores, moving averages, deltas)
- ✅ Market regime classification (NORMAL, CHOP_RISK, SQUEEZE_RISK, HIGH_VOL)
- ✅ Explainable context snapshots (queryable by future consumers)

### Forces

1. **Derivatives infrastructure may exist** (isolated margin execution confirmed; data collection status unknown)
2. **Context interpretation is complex** (requires signal processing, thresholds, heuristics)
3. **Must remain explainable** (ADR-0007: users need transparency)
4. **Must NOT be an autotrader** (ADR-0007: Robson assists, doesn't decide)
5. **Must follow hexagonal architecture** (ADR-0002: framework-independent domain)
6. **Exploratory phase** (building canonical layer, not assuming clean feeds exist)

---

## Items to Validate Before Implementation

Before proceeding to Milestone 1, the following should be verified (not all are blocking):

1. **Derivatives data infrastructure** (Discovery task)
   - Locate existing exchange integrations that might expose funding rate, OI, mark price
   - Identify if any persistence already exists (Django models, raw logs, etc.)
   - Determine if we can wrap existing code or need a minimal new adapter

2. **Threshold calibration** (Initial heuristics only)
   - Initial values for z-scores, funding extremes, squeeze risk will be set as **heuristics**
   - These are NOT correctness guarantees; they require backtesting and iteration
   - Hardcoded initially, user-configurable in future phases if validated

3. **On-chain data scope** (Optional for Phase 1)
   - DeFiLlama TVL trends considered for Phase 2+ if derivatives-only proves insufficient
   - Glassnode, Dune Analytics deferred until Core 2 validates value of derivatives alone

4. **Deployment patterns** (Discovery task)
   - Verify if Kubernetes CronJobs are standard for scheduled tasks in this repo
   - If not, Django management commands can run via alternative schedulers (cron, systemd timers, etc.)

*(Validation is recommended before implementation, but this ADR is directionally approved for exploratory development.)*

---

## Decision

**We will implement a second core: Market Research & Context Engine.**

This engine will:

1. **Normalize raw market data** (derivatives initially; on-chain optional later) into `MetricPoint` records
2. **Compute features** (z-scores, moving averages, deltas, extremes) as `FeatureVector` records
3. **Classify market regime** (NORMAL, CHOP_RISK, SQUEEZE_RISK, HIGH_VOL) based on features
4. **Generate context snapshots** (`MarketContextSnapshot`) with:
   - `market_regime` (enum)
   - `risk_bias` (CONSERVATIVE, NEUTRAL, AGGRESSIVE)
   - `stop_vulnerability` (LOW, MEDIUM, HIGH)
   - `recommended_posture` (REDUCE, HOLD, NORMAL, AGGRESSIVE)
   - `explanation_payload` (JSON with contributing metrics and logic version)
5. **Persist snapshots** in queryable form (Django models) for future consumption by decision boundaries

### Core Principles

**CRITICAL**: This is a **CONTEXT ENGINE**, NOT a trading signal generator.

- ✅ Provides explainable market regime classification
- ✅ Offers risk bias guidance (conservative vs aggressive)
- ✅ Detects stop vulnerability (clustered stops = danger)
- ✅ Fully auditable (sources, timestamps, logic version)
- ❌ Does NOT generate buy/sell signals
- ❌ Does NOT auto-execute trades
- ❌ Does NOT replace user judgment

**Analogy**: This engine is like a **weather forecast** for crypto markets. Users decide whether to trade based on the forecast, but Robson doesn't decide for them.

---

## Consequences

### Positive

✅ **Enhanced User Decision-Making**: Users get structured market context instead of raw data (when exposed via future UI/API)
✅ **Risk Awareness**: Squeeze risk, funding extremes, high vol detected proactively
✅ **Explainability**: Every snapshot includes sources, thresholds, and logic version
✅ **Queryable Persistence**: Snapshots stored in Django models, ready for future consumers
✅ **Auditable**: Complete trace from raw metrics → features → regime classification
✅ **Extensible**: Easy to add new signals (on-chain, order book, social sentiment) later
✅ **Aligned with ADR-0007**: Robson assists, user decides

### Negative / Trade-offs

❌ **Complexity**: Adds signal processing, feature engineering, and regime classification logic
❌ **Calibration Overhead**: Thresholds (z-score levels, funding extremes) require tuning and backtest validation
❌ **Data Freshness Risk**: If derivatives feed is stale (>5min), system must degrade to CONSERVATIVE
❌ **Initial Scope Limited**: BTC/USDT only, on-chain signals minimal (DeFiLlama TVL trend)
❌ **Not Real-Time**: Snapshots computed periodically (e.g., every 1-5 minutes), not tick-by-tick

### Neutral

⚪ **Threshold Configuration**: Initially hardcoded, but designed for future user customization
⚪ **Machine Learning Potential**: Current heuristics can evolve to ML models later if data proves valuable

---

## Alternatives

### Alternative A: Manual User Analysis (Status Quo)

**Description**: Users continue to manually check Binance derivatives data, Glassnode, DeFiLlama.

**Why Not Chosen**:
- ❌ Time-consuming and error-prone
- ❌ No structured historical tracking
- ❌ Can't integrate with future decision boundaries (requires manual copy-paste)
- ❌ Misses subtle patterns (e.g., funding divergence + OI spike)

---

### Alternative B: Full ML-Based Regime Classifier

**Description**: Build a machine learning model to classify market regimes based on historical data.

**Why Not Chosen**:
- ❌ Requires large labeled dataset (months of regime labeling)
- ❌ Black-box decision-making conflicts with explainability (ADR-0007)
- ❌ Over-engineering for initial scope (YAGNI)
- ✅ Can revisit later if heuristics prove insufficient

---

### Alternative C: Third-Party Market Data API (Glassnode, Kaiko, CryptoQuant)

**Description**: Subscribe to a premium market data service with pre-computed indicators.

**Why Not Chosen**:
- ❌ Cost ($100-500/month per API)
- ❌ Vendor lock-in
- ❌ Limited customization (can't add Robson-specific heuristics like squeeze risk)
- ✅ May integrate later for advanced on-chain metrics

---

### Alternative D: Embed Context in Existing Risk Core

**Description**: Add market context features to the existing Risk Management core instead of creating a separate engine.

**Why Not Chosen**:
- ❌ Violates separation of concerns (risk sizing ≠ market context)
- ❌ Risk core should focus on position sizing and exposure limits
- ❌ Context engine requires different persistence patterns (time-series metrics vs transactional)
- ✅ Keeping them separate allows independent evolution

---

## Implementation Notes

### Architecture (Hexagonal)

**Domain** (`api/application/domain.py` or new `api/application/market_context/domain.py`):
```python
@dataclass(frozen=True)
class MetricPoint:
    """Normalized input metric (immutable)."""
    timestamp: datetime
    symbol: str
    metric_name: str  # e.g., "funding_rate", "open_interest"
    value: Decimal
    source: str  # "binance_futures", "defillama"
    tags: dict  # {"timeframe": "8h", "contract": "perpetual"}

@dataclass(frozen=True)
class FeatureVector:
    """Computed feature (immutable)."""
    timestamp: datetime
    symbol: str
    feature_name: str  # e.g., "funding_zscore_24h", "oi_delta_1h"
    value: Decimal
    source_metrics: list[str]  # Traceability to raw metrics
    computation_version: str  # "v1.0.0"

@dataclass
class MarketContextSnapshot:
    """Final output: market regime + risk guidance."""
    snapshot_id: str
    timestamp: datetime
    symbol: str
    market_regime: MarketRegime  # Enum: NORMAL, CHOP_RISK, SQUEEZE_RISK, HIGH_VOL
    risk_bias: RiskBias  # Enum: CONSERVATIVE, NEUTRAL, AGGRESSIVE
    stop_vulnerability: StopVulnerability  # Enum: LOW, MEDIUM, HIGH
    recommended_posture: TradingPosture  # Enum: REDUCE, HOLD, NORMAL, AGGRESSIVE
    explanation_payload: dict  # {"funding_zscore": 2.5, "oi_delta_4h": "+15%", "logic_version": "v1.0.0"}
    sources_used: list[str]  # ["binance_futures", "defillama"]
```

**Ports** (`api/application/ports.py`):
```python
class MetricRepository(Protocol):
    def save_metric(self, metric: MetricPoint) -> None: ...
    def get_metrics(self, symbol: str, metric_name: str, start: datetime, end: datetime) -> list[MetricPoint]: ...

class FeatureBuilder(Protocol):
    def compute_features(self, metrics: list[MetricPoint]) -> list[FeatureVector]: ...

class RegimeClassifier(Protocol):
    def classify(self, features: list[FeatureVector]) -> MarketContextSnapshot: ...
```

**Adapters** (`api/application/adapters.py` or new `api/application/market_context/adapters.py`):
```python
class DjangoMetricRepository:
    """Persists MetricPoint to Django model."""
    def save_metric(self, metric: MetricPoint) -> None:
        # Idempotency: upsert on (source, symbol, metric_name, timestamp)
        pass

class BinanceFuturesMetricCollector:
    """Fetches funding rate, open interest from Binance Futures API."""
    def collect_metrics(self, symbol: str) -> list[MetricPoint]:
        # Fetch from Binance, normalize to MetricPoint
        pass

class HeuristicRegimeClassifier:
    """Rule-based regime classifier (initial implementation)."""
    def classify(self, features: list[FeatureVector]) -> MarketContextSnapshot:
        # Apply thresholds, compute regime, generate explanation
        pass
```

**Use Cases** (`api/application/use_cases.py`):
```python
class CollectDerivativesMetrics:
    """Fetch and store derivatives metrics from Binance Futures."""
    def execute(self, symbol: str) -> int:
        # Fetch, normalize, persist with idempotency
        pass

class GenerateMarketContextSnapshot:
    """Compute features and classify market regime."""
    def execute(self, symbol: str) -> MarketContextSnapshot:
        # Get recent metrics, build features, classify, persist snapshot
        pass
```

---

### Data Models (Django)

**Location**: `apps/backend/monolith/api/models/market_context.py`

```python
class MetricPoint(models.Model):
    """Normalized market metric (time-series data)."""
    timestamp = models.DateTimeField(db_index=True)
    symbol = models.CharField(max_length=20, db_index=True)
    metric_name = models.CharField(max_length=50, db_index=True)
    value = models.DecimalField(max_digits=20, decimal_places=10)
    source = models.CharField(max_length=50)  # "binance_futures", "defillama"
    tags = models.JSONField(default=dict)  # Flexible metadata

    class Meta:
        unique_together = ('source', 'symbol', 'metric_name', 'timestamp')  # Idempotency
        indexes = [
            models.Index(fields=['symbol', 'metric_name', 'timestamp']),
        ]

class FeatureVector(models.Model):
    """Computed feature from raw metrics."""
    timestamp = models.DateTimeField(db_index=True)
    symbol = models.CharField(max_length=20, db_index=True)
    feature_name = models.CharField(max_length=50, db_index=True)
    value = models.DecimalField(max_digits=20, decimal_places=10)
    source_metrics = models.JSONField(default=list)  # ["funding_rate", "open_interest"]
    computation_version = models.CharField(max_length=20, default="v1.0.0")

    class Meta:
        unique_together = ('symbol', 'feature_name', 'timestamp', 'computation_version')

class MarketContextSnapshot(models.Model):
    """Final output: market regime classification."""
    snapshot_id = models.UUIDField(primary_key=True, default=uuid.uuid4)
    timestamp = models.DateTimeField(db_index=True)
    symbol = models.CharField(max_length=20, db_index=True)
    market_regime = models.CharField(max_length=20)  # NORMAL, CHOP_RISK, etc.
    risk_bias = models.CharField(max_length=20)  # CONSERVATIVE, NEUTRAL, AGGRESSIVE
    stop_vulnerability = models.CharField(max_length=20)  # LOW, MEDIUM, HIGH
    recommended_posture = models.CharField(max_length=20)  # REDUCE, HOLD, NORMAL, AGGRESSIVE
    explanation_payload = models.JSONField()  # Full context with sources, thresholds, version
    sources_used = models.JSONField(default=list)  # ["binance_futures", "defillama"]

    class Meta:
        indexes = [
            models.Index(fields=['symbol', '-timestamp']),  # Get latest snapshot
        ]
```

---

### Management Commands

**Location**: `apps/backend/monolith/api/management/commands/`

1. **`collect_derivatives_metrics.py`**:
   ```bash
   python manage.py collect_derivatives_metrics --symbol BTCUSDT
   python manage.py collect_derivatives_metrics --symbol BTCUSDT --continuous --interval 60
   ```

2. **`generate_market_context.py`**:
   ```bash
   python manage.py generate_market_context --symbol BTCUSDT
   python manage.py generate_market_context --symbol BTCUSDT --continuous --interval 300
   ```

3. **`monitor_context_freshness.py`**:
   ```bash
   # Emit warning if latest MetricPoint > 5 minutes old
   python manage.py monitor_context_freshness --threshold 300
   ```

---

### Feature Definitions (BTC/USDT - Phase 1)

**Funding Rate Features**:
- `funding_zscore_24h`: Z-score of current funding vs last 24h
- `funding_ma_8h`: 8-hour moving average of funding rate
- `funding_extreme_flag`: Boolean (funding > 0.01% or < -0.01%)

**Open Interest Features**:
- `oi_delta_1h`: % change in OI over 1 hour
- `oi_delta_4h`: % change in OI over 4 hours
- `oi_delta_24h`: % change in OI over 24 hours
- `oi_zscore_7d`: Z-score of current OI vs last 7 days

**Volatility Features**:
- `realized_vol_1h`: 1-hour realized volatility (mark price)
- `realized_vol_4h`: 4-hour realized volatility

**Composite Features**:
- `squeeze_risk_score`: Heuristic combining (OI up + funding extreme + price flat)

---

### Regime Classification Logic (Initial Heuristics)

```python
def classify_regime(features: dict) -> MarketRegime:
    """Initial rule-based classifier."""

    # SQUEEZE_RISK: High funding + rising OI + low vol
    if (features["funding_zscore_24h"] > 2.0 and
        features["oi_delta_4h"] > 0.10 and
        features["realized_vol_1h"] < 0.30):
        return MarketRegime.SQUEEZE_RISK

    # HIGH_VOL: Realized vol spike
    if features["realized_vol_1h"] > 0.80:
        return MarketRegime.HIGH_VOL

    # CHOP_RISK: Flat price + high vol
    if (features["realized_vol_4h"] > 0.50 and
        abs(features["oi_delta_1h"]) < 0.02):
        return MarketRegime.CHOP_RISK

    # NORMAL: Default
    return MarketRegime.NORMAL
```

---

### Freshness Monitor

**Goal**: Detect stale derivatives data and degrade to CONSERVATIVE mode.

**Implementation**:
1. Before generating snapshot, check latest `MetricPoint.timestamp`
2. If `now - latest_timestamp > 5 minutes`:
   - Emit warning log
   - Set `risk_bias = CONSERVATIVE`
   - Set `recommended_posture = HOLD`
   - Add to `explanation_payload`: `{"freshness_warning": "Data stale (6 minutes old)"}`

---

### Integration with Future Decision Boundaries (Out of Scope for Core 2)

**Note**: This section describes potential future usage, NOT implementation requirements for this phase.

**Example: Future EntryGate (Core 1.1)**:
- EntryGate could query latest `MarketContextSnapshot` before order creation
- Display warnings if `recommended_posture == REDUCE` or `stop_vulnerability == HIGH`
- User reviews context, decides to proceed/adjust/cancel
- Final decision remains with user (never auto-blocked)

**Example: Future PositionManager (Core 1.2+)**:
- Could monitor regime changes for open positions
- Flag positions when regime shifts (NORMAL → SQUEEZE_RISK)
- User reviews flagged positions, decides next action

**Key**: These integrations are deferred to future cores. Core 2 only provides the queryable snapshot persistence layer.

---

### Tests

**Unit Tests** (`api/tests/test_market_context.py`):
- Test z-score calculation with sample data
- Test moving average computation
- Test regime classification with known inputs
- Test idempotency of `MetricPoint` upsert

**Integration Tests** (`api/tests/test_market_context_integration.py`):
- Test full flow: collect metrics → compute features → classify regime
- Test freshness monitor with stale data
- Test snapshot persistence and querying

---

### Rollout Plan (Milestones)

**Phase 0.5: Code Discovery & Validation** (Before Milestone 1)
- [ ] Locate existing exchange integrations (file paths, adapters)
- [ ] Identify which derivatives metrics are already accessible (funding rate, OI, mark price)
- [ ] Determine if existing collectors can be wrapped or if new adapter needed
- [ ] Verify deployment patterns (Kubernetes CronJobs vs other schedulers)
- [ ] Document findings and update implementation approach accordingly

**Milestone 1: Derivatives Storage** (Week 1)
- [ ] Create `MetricPoint` model and migration
- [ ] Implement `DjangoMetricRepository` (idempotent persistence)
- [ ] Create collector adapter: wrap existing code OR minimal new adapter (based on Phase 0.5 findings)
- [ ] Create `collect_derivatives_metrics` management command (if needed)
- [ ] Test idempotency with duplicate metrics
- [ ] Validate BTC/USDT data collection (local first, deployment optional)

**Milestone 2: Feature Engineering** (Week 2)
- [ ] Create `FeatureVector` model and migration
- [ ] Implement z-score, MA, delta calculators (feature builders)
- [ ] Create `FeatureBuilder` use case
- [ ] Test with historical data (if available)
- [ ] Add unit tests for all feature builders (heuristic correctness)

**Milestone 3: Regime Classifier** (Week 3)
- [ ] Create `MarketContextSnapshot` model and migration
- [ ] Implement `HeuristicRegimeClassifier` (initial thresholds as heuristics)
- [ ] Create `generate_market_context` management command
- [ ] Test classification with sample scenarios
- [ ] Add integration test for full pipeline (collect → features → classify)

**Milestone 4: Freshness Monitor** (Week 4)
- [ ] Implement `monitor_context_freshness` command
- [ ] Add freshness check to snapshot generation (auto-degrade to CONSERVATIVE)
- [ ] Test degradation to CONSERVATIVE mode with stale data
- [ ] Add logging (warning on stale data)

**Milestone 5: On-Chain Signals** (Optional - Week 5+)
- [ ] Add DeFiLlama TVL trend as MetricPoint
- [ ] Normalize on-chain data to same schema
- [ ] Extend regime classifier to use TVL trend
- [ ] Test with historical TVL data
- [ ] Evaluate additional on-chain providers (Glassnode, Dune)

**Note**: Deployment to production (Kubernetes, CronJobs, etc.) is NOT a requirement for this phase. Local validation and staging tests are sufficient.

---

## Related Decisions

- **ADR-0007**: Robson is Risk Assistant (this engine provides context, not trading signals)
- **ADR-0002**: Hexagonal Architecture (domain-driven design, framework-independent)
- **ADR-0001**: BinanceService Singleton (rate limit handling for derivatives API)

---

## References

- Binance Futures API: https://binance-docs.github.io/apidocs/futures/en/
- DeFiLlama API: https://defillama.com/docs/api
- Market Regime Classification (academic): https://arxiv.org/abs/1906.03945
- Z-Score in Trading: https://www.investopedia.com/terms/z/zscore.asp

---

## Approval

**Deciders**: Development Team, Product Owner
**Status**: PROPOSED (directionally approved for exploratory development)
**Next Steps**:
1. Complete Phase 0.5 discovery (locate derivatives infrastructure)
2. Validate threshold values as heuristics (backtesting recommended but not blocking)
3. Proceed to Milestone 1 implementation with discovery findings

---

**Last Updated**: 2025-12-28
