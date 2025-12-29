# ADR-0018: Pattern Detection Engine

**Status**: Proposed
**Date**: 2025-12-28
**Deciders**: Development Team
**Related**: ADR-0002 (Hexagonal Architecture), ADR-0007 (Risk Assistant), ADR-0017 (Market Context)

---

## Context

Robson Bot currently has three cores:

- **CORE 1.1**: Hand-Span Trailing Stop (manages stop updates post-entry)
- **CORE 1.2**: Entry Gate (decides if entry is allowed based on risk limits + context)
- **CORE 2**: Market Context Engine (produces market regime snapshots)

However, **there is no automated pattern detection** to identify technical opportunities (candlestick patterns, chart patterns) that could feed into the Entry Gate as "opportunity signals".

### Current State

- ✅ Pattern data models exist in production (`PatternCatalog`, `PatternInstance`, `PatternPoint`, `PatternAlert`, detail models)
- ✅ Candle data available via `MarketDataService.get_historical_data()` (Binance klines)
- ❌ No detection logic implemented (models are empty)
- ❌ No lifecycle management (FORMING → CONFIRMED → FAILED/INVALIDATED)
- ❌ No alerts emitted for downstream consumption

### Problem Statement

**Users need automated pattern detection to identify technical opportunities, but Robson doesn't provide it.**

Desired state:

- ✅ Continuous scanning of BTC/USDT 15m candles for defined patterns
- ✅ Deterministic, reproducible pattern detection (no subjective interpretation)
- ✅ Lifecycle management (FORMING → CONFIRMED → FAILED → INVALIDATED → TARGET_HIT)
- ✅ Structured alerts (FORMING, CONFIRM, INVALIDATE, TARGET_HIT) for downstream cores
- ✅ Auditable: every detection includes version, thresholds, evidence

### Forces

1. **Pattern models already exist** (must use existing schema, NOT redesign)
2. **Determinism required** (same OHLC → same patterns, idempotent on re-runs)
3. **Minimal scope v1** (4 candlestick + 2 chart patterns only, no harmonic/elliott/wyckoff)
4. **Integration with cores** (Entry Gate consumes alerts, but Pattern Engine does NOT place orders)
5. **Hexagonal architecture** (ADR-0002: domain logic framework-independent)
6. **Risk assistant principle** (ADR-0007: Robson assists, doesn't auto-trade)

---

## Decision

**We will implement a Pattern Detection Engine as a fourth operational component.**

This engine will:

1. **Scan OHLCV candles** (BTC/USDT 15m initially) for pattern signatures
2. **Create/update `PatternInstance` records** through full lifecycle:
   - `FORMING` when signature detected
   - `CONFIRMED` when confirmation rules met
   - `FAILED/INVALIDATED/EXPIRED` when invalidation rules met
   - `TARGET_HIT` when tp1/tp2/tp3 reached (optional follow-up)
3. **Persist evidence** in detail models:
   - `CandlestickPatternDetail` for candlestick patterns
   - `ChartPatternDetail` for chart patterns
   - `PatternPoint` for pivot/key points (required for HNS/IHNS)
4. **Emit `PatternAlert` records** at lifecycle transitions:
   - `FORMING`, `CONFIRM`, `INVALIDATE`, `TARGET_HIT`
5. **Ensure determinism + idempotency**:
   - Uniqueness: `(pattern_code, symbol, timeframe, start_ts)` for instances
   - Uniqueness: `(instance_id, alert_type, event_ts)` for alerts
   - Re-scanning same candle window produces NO duplicates

### Core Principles

**CRITICAL**: This is a **PATTERN DETECTION ENGINE**, NOT a trading signal executor.

- ✅ Provides structured opportunity signals (patterns) with evidence
- ✅ Emits alerts for downstream consumption (Entry Gate, etc.)
- ✅ Fully auditable (detected_version, thresholds in payload, pivot points stored)
- ✅ Deterministic (rule-based, no ML black boxes in v1)
- ❌ Does NOT generate buy/sell orders
- ❌ Does NOT manage positions
- ❌ Does NOT move stop-losses (that's Hand-Span Trailing Stop's job)

**Analogy**: This engine is like a **radar** that detects technical setups. Entry Gate decides whether to allow action on those setups.

---

## Scope (v1 - Strict Boundaries)

### In Scope

**Candlestick Patterns** (4 patterns):

- `HAMMER` (bullish reversal)
- `INVERTED_HAMMER` (bullish reversal)
- `ENGULFING_BULL` (bullish reversal)
- `ENGULFING_BEAR` (bearish reversal)

**Chart Patterns** (2 patterns):

- `HEAD_AND_SHOULDERS` (bearish reversal)
- `INVERTED_HEAD_AND_SHOULDERS` (bullish reversal)

**Timeframe**: 15m initially (architecture supports multi-timeframe later)

**Symbol**: BTC/USDT initially (architecture supports multi-symbol later)

### Out of Scope (v1)

The following patterns exist in `ChartPatternCode`/`CandlestickPatternCode` enums but are **NOT implemented in v1**:

- Triangles (ASCENDING, DESCENDING, SYMMETRICAL)
- Wedges (RISING, FALLING)
- Harmonic patterns (Gartley, Bat, Butterfly, etc.)
- Elliott Wave patterns
- Wyckoff patterns
- Indicator-based patterns
- Cyclical patterns
- Double/Triple tops/bottoms
- Flags, pennants, channels

**Architecture must allow adding these later without refactor.**

---

## Architecture

### Hexagonal Structure

**Domain** (`api/application/pattern_engine/domain.py`):

```python
@dataclass(frozen=True)
class CandleWindow:
    """Immutable OHLCV sequence for analysis."""
    symbol: str
    timeframe: str
    candles: list[OHLCV]  # Ordered by timestamp
    start_ts: datetime
    end_ts: datetime

@dataclass(frozen=True)
class PatternSignature:
    """Detected pattern signature (before confirmation)."""
    pattern_code: str
    start_ts: datetime
    end_ts: datetime
    confidence: Decimal  # 0-1
    evidence: dict  # Numeric metrics, NOT "looks like"
    key_points: list[PivotPoint]  # For chart patterns

@dataclass(frozen=True)
class PatternLifecycleEvent:
    """Lifecycle state transition event."""
    instance_id: int
    event_type: str  # FORMING, CONFIRMED, INVALIDATED, etc.
    event_ts: datetime
    evidence: dict
    version: str  # detector version
```

**Ports** (`api/application/pattern_engine/ports.py`):

```python
class CandleProvider(Protocol):
    """Interface for fetching candles."""
    def get_candles(self, symbol: str, timeframe: str, limit: int) -> CandleWindow: ...

class PatternRepository(Protocol):
    """Interface for pattern persistence."""
    def get_or_create_instance(self, signature: PatternSignature) -> PatternInstance: ...
    def update_status(self, instance_id: int, status: str, evidence: dict) -> None: ...
    def emit_alert(self, instance_id: int, alert_type: str, confidence: Decimal, payload: dict) -> None: ...

class PatternDetector(Protocol):
    """Interface for pattern detection logic."""
    def detect(self, window: CandleWindow) -> list[PatternSignature]: ...
    def check_confirmation(self, instance: PatternInstance, window: CandleWindow) -> bool: ...
    def check_invalidation(self, instance: PatternInstance, window: CandleWindow) -> bool: ...
```

**Use Cases** (`api/application/pattern_engine/use_cases.py`):

```python
class ScanForPatternsUseCase:
    """Main orchestrator: scan candles, create/update instances, emit alerts."""

    def execute(self, symbol: str, timeframe: str) -> list[PatternLifecycleEvent]:
        # 1. Fetch candle window
        # 2. Run all detectors (candlestick + chart)
        # 3. For each signature: get_or_create_instance (idempotent)
        # 4. For FORMING instances: check confirmation/invalidation
        # 5. Update status + emit alerts
        # 6. Return events for audit
        pass
```

**Adapters** (`api/application/pattern_engine/adapters.py`):

```python
class BinanceCandleProvider:
    """Fetches candles from MarketDataService."""
    def get_candles(self, symbol: str, timeframe: str, limit: int) -> CandleWindow:
        # Use existing MarketDataService.get_historical_data()
        pass

class DjangoPatternRepository:
    """Persists to PatternInstance, PatternPoint, PatternAlert models."""
    def get_or_create_instance(self, signature: PatternSignature) -> PatternInstance:
        # Idempotent upsert on (pattern_code, symbol, timeframe, start_ts)
        pass
```

---

## Pattern Rule Definitions (Numeric, Deterministic)

### Configuration Module

**Location**: `api/application/pattern_engine/config.py`

```python
@dataclass(frozen=True)
class HammerConfig:
    """Hammer detection thresholds."""
    lower_wick_pct_min: Decimal = Decimal("0.60")  # 60% of range
    body_pct_max: Decimal = Decimal("0.30")  # 30% of range
    upper_wick_pct_max: Decimal = Decimal("0.15")  # 15% of range
    prior_trend_bars: int = 10  # Check N bars for downtrend
    prior_return_threshold: Decimal = Decimal("-0.03")  # -3% or MA below
    confirm_window_bars: int = 3  # Confirm within M bars
    confirm_close_above_midpoint: bool = True  # Close above hammer midpoint
```

### Candle Metrics (Shared Helpers)

```python
def compute_candle_metrics(candle: OHLCV) -> dict:
    """Deterministic candle anatomy."""
    body = abs(candle.close - candle.open)
    range_ = candle.high - candle.low

    if range_ == 0:  # Degenerate case
        return None

    upper_wick = candle.high - max(candle.open, candle.close)
    lower_wick = min(candle.open, candle.close) - candle.low

    return {
        "body": body,
        "range": range_,
        "upper_wick": upper_wick,
        "lower_wick": lower_wick,
        "body_pct": body / range_,
        "upper_wick_pct": upper_wick / range_,
        "lower_wick_pct": lower_wick / range_,
    }
```

### A) Hammer (Bullish Candidate)

**Detection**:

```python
def detect_hammer(candle: OHLCV, config: HammerConfig) -> bool:
    metrics = compute_candle_metrics(candle)
    if not metrics:
        return False

    return (
        metrics["lower_wick_pct"] >= config.lower_wick_pct_min and
        metrics["body_pct"] <= config.body_pct_max and
        metrics["upper_wick_pct"] <= config.upper_wick_pct_max
    )
```

**Context Filter** (optional, configurable):

```python
def check_prior_downtrend(candles: list[OHLCV], config: HammerConfig) -> bool:
    """Check if prior N bars show downtrend."""
    if len(candles) < config.prior_trend_bars:
        return False  # Not enough history

    prior = candles[-config.prior_trend_bars:-1]
    return_pct = (prior[-1].close - prior[0].close) / prior[0].close

    return return_pct <= config.prior_return_threshold
```

**Confirmation** (configurable):

```python
def check_hammer_confirmation(hammer_candle: OHLCV, next_candles: list[OHLCV], config: HammerConfig) -> bool:
    """Confirm if next candle closes above hammer midpoint within M bars."""
    hammer_midpoint = (hammer_candle.high + hammer_candle.low) / 2

    for candle in next_candles[:config.confirm_window_bars]:
        if candle.close > hammer_midpoint:
            return True

    return False
```

### B) Inverted Hammer (Bullish Candidate)

**Detection**:

```python
def detect_inverted_hammer(candle: OHLCV, config: InvertedHammerConfig) -> bool:
    metrics = compute_candle_metrics(candle)
    if not metrics:
        return False

    return (
        metrics["upper_wick_pct"] >= config.upper_wick_pct_min and
        metrics["body_pct"] <= config.body_pct_max and
        metrics["lower_wick_pct"] <= config.lower_wick_pct_max
    )
```

### C) Engulfing (Bullish/Bearish)

**Bullish Engulfing**:

```python
def detect_bullish_engulfing(prev: OHLCV, curr: OHLCV, config: EngulfingConfig) -> bool:
    # Prev candle bearish
    if prev.close >= prev.open:
        return False

    # Current candle bullish
    if curr.close <= curr.open:
        return False

    # Current body engulfs prev body
    if not (curr.open <= prev.close and curr.close >= prev.open):
        return False

    # Engulf ratio threshold (optional)
    prev_body = abs(prev.close - prev.open)
    curr_body = abs(curr.close - curr.open)
    engulf_ratio = curr_body / max(prev_body, Decimal("0.0001"))

    return engulf_ratio >= config.min_engulf_ratio
```

**Bearish Engulfing**: Symmetric (inverse conditions).

### D) Head and Shoulders (Pivot-Based)

**Pivot Detection** (fractal window):

```python
def find_pivots(candles: list[OHLCV], k: int = 3) -> list[PivotPoint]:
    """Find swing highs/lows using fractal window k."""
    pivots = []

    for i in range(k, len(candles) - k):
        # Pivot high: high[i] is max in [i-k, i+k]
        is_high = all(candles[i].high >= candles[j].high for j in range(i-k, i+k+1))
        if is_high:
            pivots.append(PivotPoint(ts=candles[i].ts, price=candles[i].high, type="HIGH"))

        # Pivot low: low[i] is min in [i-k, i+k]
        is_low = all(candles[i].low <= candles[j].low for j in range(i-k, i+k+1))
        if is_low:
            pivots.append(PivotPoint(ts=candles[i].ts, price=candles[i].low, type="LOW"))

    return pivots
```

**HNS Pattern Recognition**:

```python
def detect_head_and_shoulders(pivots: list[PivotPoint], config: HNSConfig) -> Optional[HNSSignature]:
    """Detect HNS using last M pivot highs and intervening lows."""
    highs = [p for p in pivots if p.type == "HIGH"]
    lows = [p for p in pivots if p.type == "LOW"]

    if len(highs) < 3:
        return None

    # Last 3 highs: LS, HEAD, RS
    ls, head, rs = highs[-3:]

    # Head must be highest
    if not (head.price > ls.price and head.price > rs.price):
        return None

    # Shoulders within tolerance
    shoulder_diff = abs(ls.price - rs.price)
    shoulder_tol = config.shoulder_tolerance_pct * head.price

    if shoulder_diff > shoulder_tol:
        return None

    # Find neckline (lows between LS-Head and Head-RS)
    # Simplified: connect two lowest lows in those regions
    # (Full implementation would use linear regression)

    return HNSSignature(
        ls=ls,
        head=head,
        rs=rs,
        neckline_slope=compute_neckline_slope(lows),
        confidence=compute_hns_confidence(ls, head, rs, config),
    )
```

**Confirmation** (neckline break):

```python
def check_hns_confirmation(hns: HNSSignature, current_price: Decimal, config: HNSConfig) -> bool:
    """Confirm if price breaks neckline by threshold."""
    neckline_level = compute_neckline_at_current_ts(hns.neckline_slope, current_ts)
    break_threshold = neckline_level * (1 - config.break_tolerance_pct)

    return current_price < break_threshold  # Bearish break
```

**Invalidation**:

```python
def check_hns_invalidation(hns: HNSSignature, current_price: Decimal, config: HNSConfig) -> bool:
    """Invalidate if price closes above RS high (or head, configurable)."""
    invalidation_level = hns.rs.price  # or hns.head.price if stricter

    return current_price > invalidation_level
```

---

## Determinism + Idempotency Strategy

### Instance Uniqueness

```python
def get_or_create_instance(signature: PatternSignature) -> PatternInstance:
    """Idempotent instance creation."""
    # Uniqueness key: (pattern_code, symbol, timeframe, start_ts)
    instance, created = PatternInstance.objects.get_or_create(
        pattern__pattern_code=signature.pattern_code,
        symbol=symbol_obj,
        timeframe=signature.timeframe,
        start_ts=signature.start_ts,
        defaults={
            "status": PatternStatus.FORMING,
            "detected_version": DETECTOR_VERSION,
            "features": signature.evidence,
        }
    )

    if not created and instance.status == PatternStatus.FORMING:
        # Update evidence if still forming (idempotent)
        instance.features = signature.evidence
        instance.save()

    return instance
```

### Alert Uniqueness

```python
def emit_alert(instance_id: int, alert_type: str, event_ts: datetime, confidence: Decimal, payload: dict) -> None:
    """Idempotent alert emission."""
    # Uniqueness key: (instance_id, alert_type, event_ts)
    # Use get_or_create to prevent duplicates
    PatternAlert.objects.get_or_create(
        instance_id=instance_id,
        alert_type=alert_type,
        alert_ts=event_ts,  # Use event_ts for determinism
        defaults={
            "confidence": confidence,
            "payload": payload,
        }
    )
```

### Re-Scan Guarantees

**Property**: Running detector repeatedly over same candle history produces:

- Same `PatternInstance` records (no duplicates)
- Same `PatternAlert` records (no duplicate alerts)
- Same `PatternPoint` records (pivot points reproducible)

**Implementation**:

- Use `get_or_create` with stable uniqueness keys
- Use deterministic timestamps (from candle data, NOT `datetime.now()`)
- Store `detected_version` to allow re-detection with new rules (separate instances)

---

## Integration with Other Cores

### Entry Gate (CORE 1.2) Consumption

**How Entry Gate Uses Patterns**:

1. Entry Gate receives user intent: "BUY BTC/USDT at $50,000"
2. Entry Gate queries:

   ```python
   recent_patterns = PatternInstance.objects.filter(
       symbol__name="BTCUSDT",
       status=PatternStatus.CONFIRMED,
       pattern__direction_bias=PatternDirectionBias.BULLISH,
       breakout_ts__gte=now() - timedelta(hours=4),  # Recent
   )
   ```

3. Entry Gate checks risk limits (monthly loss quota, cooldowns, etc.)
4. Entry Gate optionally checks `MarketContextSnapshot` (CORE 2) for regime filter
5. **User reviews all info and decides** (Entry Gate does NOT auto-execute)

**Pattern Engine Does NOT**:

- ❌ Place orders
- ❌ Call Entry Gate directly
- ❌ Auto-execute on confirmed patterns

**Entry Gate Does NOT**:

- ❌ Auto-allow entries just because pattern confirmed
- ❌ Bypass user confirmation

### Hand-Span Trailing Stop (CORE 1.1)

**Separation**:

- Pattern Engine detects opportunities → `PatternInstance` created
- Entry Gate decides if entry allowed → User confirms → Order placed
- Hand-Span Trailing Stop manages stop updates AFTER position opened

**Pattern Engine Does NOT**:

- ❌ Move stops (that's CORE 1.1's job)
- ❌ Track open positions

**Optional Future Enhancement** (out of scope v1):

- Pattern Engine could populate `PatternInstance.invalidation_level` and `PatternInstance.tp1/tp2/tp3`
- Hand-Span Trailing Stop COULD use these as reference points (but maintains its own logic)

---

## Monitoring / Observability

### Logging

**INFO-level**:

- Pattern detected: `logger.info(f"HAMMER detected at {start_ts}, confidence={confidence}")`
- Status transition: `logger.info(f"Pattern {instance_id} CONFIRMED at {event_ts}")`
- Detector run summary: `logger.info(f"Scanned 100 candles, found 3 patterns, emitted 5 alerts")`

**WARNING-level**:

- Stale candle data: `logger.warning(f"Candles stale: last={last_ts}, age={age}s")`
- Degenerate candles: `logger.warning(f"Candle with zero range at {ts}, skipping")`
- Pivot detection failure: `logger.warning(f"Not enough pivots for HNS detection, need 3 highs, got {len(highs)}")`

### Freshness Monitor

**Requirement**: If candle feed is stale (no new 15m candle beyond 20 minutes), emit warning.

**Implementation**:

```python
def check_candle_freshness(window: CandleWindow, threshold_seconds: int = 1200) -> bool:
    """Check if latest candle is fresh."""
    age = (datetime.now(tz=timezone.utc) - window.end_ts).total_seconds()

    if age > threshold_seconds:
        logger.warning(f"Candle feed stale: last={window.end_ts}, age={age}s")
        return False

    return True
```

### Versioning

**Every `PatternInstance` must have**:

- `detected_version`: e.g., `"pattern_engine_v1.0.0"`
- Thresholds in `features` JSON or `payload` of alerts

**Example**:

```json
{
  "detected_version": "pattern_engine_v1.0.0",
  "features": {
    "lower_wick_pct": 0.65,
    "body_pct": 0.25,
    "thresholds": {
      "lower_wick_pct_min": 0.60,
      "body_pct_max": 0.30
    }
  }
}
```

---

## Deliverables (PHASE 0 vs PHASE 1)

### PHASE 0 (This Document)

**Outputs**:

1. ✅ This ADR
2. ✅ `docs/strategy/PATTERN_ENGINE_V1.md` (detailed rules, thresholds, examples)
3. ✅ Implementation plan with milestones
4. ✅ Test plan with exact test cases

**No code implementation yet.**

### PHASE 1 (After Validation)

**Outputs**:

1. Pattern detection module implementation
2. Unit tests (golden OHLC sequences)
3. Integration tests (idempotent instance creation)
4. Property tests (no duplicates on re-runs)
5. Management command: `python manage.py scan_patterns --symbol BTCUSDT --timeframe 15m`

---

## Consequences

### Positive

✅ **Structured Opportunity Signals**: Patterns feed into Entry Gate with evidence
✅ **Deterministic**: Same OHLC always produces same patterns
✅ **Auditable**: Full trace from candles → signature → instance → alerts
✅ **Idempotent**: Re-scanning doesn't create duplicates
✅ **Extensible**: Easy to add new patterns (triangles, etc.) later
✅ **Aligned with ADR-0007**: Robson assists (detects patterns), user decides (Entry Gate)

### Negative / Trade-offs

❌ **Limited Scope v1**: Only 6 patterns (4 candlestick + 2 chart)
❌ **Rule Calibration**: Thresholds (wick%, pivot window k) require backtesting
❌ **Candle Dependency**: Depends on candle feed freshness
❌ **No Real-Time**: Scans periodically (e.g., every 15m), not tick-by-tick

### Neutral

⚪ **No ML**: v1 uses rule-based detection; ML can be added later
⚪ **Single Timeframe**: 15m initially; multi-timeframe requires coordination (out of scope v1)

---

## Alternatives

### Alternative A: Manual Pattern Spotting (Status Quo)

**Why Not**: Time-consuming, error-prone, no audit trail, can't integrate with Entry Gate.

### Alternative B: Third-Party Pattern Scanner (TradingView, etc.)

**Why Not**: Cost, vendor lock-in, no customization, can't integrate with Robson's risk management.

### Alternative C: ML-Based Pattern Recognition

**Why Not**: Requires labeled dataset, black-box (conflicts with explainability), over-engineering for v1.

---

## Implementation Notes

### File Paths

**New Files** (to be created in PHASE 1):

```
apps/backend/monolith/api/application/pattern_engine/
├── __init__.py
├── domain.py          # Pure entities (CandleWindow, PatternSignature, etc.)
├── ports.py           # Interfaces (CandleProvider, PatternRepository, PatternDetector)
├── use_cases.py       # ScanForPatternsUseCase orchestrator
├── adapters.py        # BinanceCandleProvider, DjangoPatternRepository
├── config.py          # HammerConfig, HNSConfig, etc.
├── detectors/
│   ├── __init__.py
│   ├── base.py        # PatternDetector base interface
│   ├── candlestick.py # HammerDetector, EngulfingDetector, etc.
│   └── chart.py       # HNSDetector, IHNSDetector
└── helpers.py         # compute_candle_metrics, find_pivots, etc.

apps/backend/monolith/api/management/commands/
└── scan_patterns.py   # Management command

apps/backend/monolith/api/tests/
├── test_pattern_engine.py
├── test_pattern_detectors.py
└── test_pattern_idempotency.py

docs/strategy/
└── PATTERN_ENGINE_V1.md
```

**Existing Files** (to use, NOT modify):

- `apps/backend/monolith/api/models/patterns/base.py` (PatternInstance, etc.)
- `apps/backend/monolith/api/models/patterns/candlestick.py` (CandlestickPatternDetail)
- `apps/backend/monolith/api/models/patterns/chart.py` (ChartPatternDetail)
- `apps/backend/monolith/api/services/market_data_service.py` (candle fetching)

---

## Related Decisions

- **ADR-0002**: Hexagonal Architecture (domain-driven, framework-independent)
- **ADR-0007**: Robson is Risk Assistant (patterns detected, user decides)
- **ADR-0017**: Market Context Engine (patterns can be inputs to context if needed)

---

## References

- **Candlestick Patterns**: Steve Nison, "Japanese Candlestick Charting Techniques"
- **Chart Patterns**: Thomas Bulkowski, "Encyclopedia of Chart Patterns"
- **Head and Shoulders**: <https://www.investopedia.com/terms/h/head-shoulders.asp>
- **Pivot Points**: <https://www.investopedia.com/terms/p/pivotpoint.asp>

---

## Approval

**Deciders**: Development Team, Product Owner
**Status**: PROPOSED (PHASE 0 complete, awaiting validation)
**Next Steps**:

1. Review this ADR + `docs/strategy/PATTERN_ENGINE_V1.md`
2. Validate thresholds (HammerConfig, etc.) via backtesting (recommended)
3. Approve for PHASE 1 implementation

---

**Last Updated**: 2025-12-28
