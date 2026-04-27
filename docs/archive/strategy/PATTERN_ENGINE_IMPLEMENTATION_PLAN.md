# Pattern Detection Engine - Implementation Plan

**Version:** 1.0.0
**Date:** 2025-12-28
**Status:** PHASE 0 (Planning)

---

## Overview

This document defines the implementation milestones, file paths, and test plan for the Pattern Detection Engine v1.0.

**CRITICAL**: This is PHASE 0 only. No code implementation until plan is validated.

---

## Milestone Checklist

### Milestone 0: Validation (This Document)

- [x] ADR-0018 created
- [x] PATTERN_ENGINE_V1.md specification created
- [x] Implementation plan created
- [x] Test plan defined
- [ ] **USER VALIDATION REQUIRED** ✋

**Do NOT proceed to Milestone 1 until this plan is approved.**

---

### Milestone 1: Core Infrastructure (Week 1)

**Goal**: Set up hexagonal module structure + configuration.

#### 1.1 Module Structure

Create directory structure:

```bash
mkdir -p apps/backend/monolith/api/application/pattern_engine
mkdir -p apps/backend/monolith/api/application/pattern_engine/detectors
```

#### 1.2 Files to Create

- [ ] `apps/backend/monolith/api/application/pattern_engine/__init__.py`
  - Exports: `ScanForPatternsUseCase`, detector configs
- [ ] `apps/backend/monolith/api/application/pattern_engine/domain.py`
  - Entities: `CandleWindow`, `PatternSignature`, `PatternLifecycleEvent`, `PivotPoint`, `OHLCV`
  - Pure Python, NO Django imports
- [ ] `apps/backend/monolith/api/application/pattern_engine/ports.py`
  - Protocols: `CandleProvider`, `PatternRepository`, `PatternDetector`
- [ ] `apps/backend/monolith/api/application/pattern_engine/config.py`
  - Dataclasses: `HammerConfig`, `EngulfingConfig`, `HNSConfig`, `PivotConfig`, etc.
  - Default configurations (constants)
- [ ] `apps/backend/monolith/api/application/pattern_engine/helpers.py`
  - Pure functions: `compute_candle_metrics()`, `find_pivots()`, `compute_line_fit()`

**Acceptance Criteria**:

- [x] All files created
- [x] Domain entities pass type checking (mypy)
- [x] No Django imports in `domain.py`, `config.py`, `helpers.py`

**Estimated Time**: 1 day

---

### Milestone 2: Adapters (Week 1)

**Goal**: Implement Django adapters for candle fetching and persistence.

#### 2.1 Files to Create

- [ ] `apps/backend/monolith/api/application/pattern_engine/adapters.py`
  - Classes:
    - `BinanceCandleProvider` (uses `MarketDataService.get_historical_data()`)
    - `DjangoPatternRepository` (uses existing pattern models)

#### 2.2 Integration Points

**Candle Fetching**:

```python
class BinanceCandleProvider:
    """Adapter: Fetch candles from existing MarketDataService."""

    def __init__(self):
        from api.services.market_data_service import MarketDataService
        self.market_data = MarketDataService()

    def get_candles(self, symbol: str, timeframe: str, limit: int) -> CandleWindow:
        """
        Fetch OHLCV candles.

        Uses existing MarketDataService.get_historical_data().
        Converts JSON response to CandleWindow domain object.
        """
        # Implementation here
        pass
```

**Pattern Persistence**:

```python
class DjangoPatternRepository:
    """Adapter: Persist patterns to Django models."""

    def get_or_create_instance(self, signature: PatternSignature) -> PatternInstance:
        """
        Idempotent instance creation.

        Uniqueness key: (pattern_code, symbol, timeframe, start_ts)
        """
        # Implementation here
        pass

    def update_status(self, instance_id: int, status: str, evidence: dict) -> None:
        """Update instance status with evidence."""
        # Implementation here
        pass

    def emit_alert(self, instance_id: int, alert_type: str, confidence: Decimal, payload: dict) -> None:
        """
        Idempotent alert emission.

        Uniqueness key: (instance_id, alert_type, alert_ts from payload)
        """
        # Implementation here
        pass

    def store_candlestick_detail(self, instance: PatternInstance, metrics: dict) -> None:
        """Create CandlestickPatternDetail record."""
        # Implementation here
        pass

    def store_chart_detail(self, instance: PatternInstance, metrics: dict) -> None:
        """Create ChartPatternDetail record."""
        # Implementation here
        pass

    def store_pattern_points(self, instance: PatternInstance, points: list[PivotPoint]) -> None:
        """Create PatternPoint records (for chart patterns)."""
        # Implementation here
        pass
```

**Acceptance Criteria**:

- [x] `BinanceCandleProvider` returns valid `CandleWindow` objects
- [x] `DjangoPatternRepository.get_or_create_instance()` is idempotent (no duplicates on re-run)
- [x] `DjangoPatternRepository.emit_alert()` is idempotent (no duplicate alerts)
- [x] Integration test: fetch 100 candles, create instance, re-run → no duplicates

**Estimated Time**: 2 days

---

### Milestone 3: Candlestick Pattern Detectors (Week 2)

**Goal**: Implement 4 candlestick pattern detectors.

#### 3.1 Files to Create

- [ ] `apps/backend/monolith/api/application/pattern_engine/detectors/__init__.py`
  - Exports: `HammerDetector`, `InvertedHammerDetector`, `EngulfingDetector`
- [ ] `apps/backend/monolith/api/application/pattern_engine/detectors/base.py`
  - Abstract class: `PatternDetectorBase` (implements `PatternDetector` protocol)
- [ ] `apps/backend/monolith/api/application/pattern_engine/detectors/candlestick.py`
  - Classes:
    - `HammerDetector` (detect + confirmation + invalidation logic)
    - `InvertedHammerDetector`
    - `BullishEngulfingDetector`
    - `BearishEngulfingDetector`

#### 3.2 Implementation Requirements

Each detector must implement:

```python
class HammerDetector(PatternDetectorBase):
    pattern_code = CandlestickPatternCode.HAMMER

    def __init__(self, config: HammerConfig):
        self.config = config

    def detect(self, window: CandleWindow) -> list[PatternSignature]:
        """
        Detect hammer patterns in candle window.

        Returns list of signatures (usually 0 or 1 for candlestick patterns).
        """
        # Implementation: iterate candles, compute metrics, check thresholds
        pass

    def check_confirmation(self, instance: PatternInstance, window: CandleWindow) -> Optional[dict]:
        """
        Check if pattern is confirmed.

        Returns evidence dict if confirmed, None otherwise.
        """
        # Implementation: check next candles for confirmation rules
        pass

    def check_invalidation(self, instance: PatternInstance, window: CandleWindow) -> Optional[dict]:
        """
        Check if pattern is invalidated.

        Returns evidence dict if invalidated, None otherwise.
        """
        # Implementation: check if price broke invalidation level
        pass
```

**Acceptance Criteria**:

- [x] All 4 detectors implemented
- [x] Unit tests pass (see Test Plan below)
- [x] Detectors use configured thresholds (not hardcoded)
- [x] Evidence payload includes thresholds for auditability

**Estimated Time**: 3 days

---

### Milestone 4: Chart Pattern Detectors (Week 2)

**Goal**: Implement HNS and IHNS detectors.

#### 4.1 Files to Create

- [ ] `apps/backend/monolith/api/application/pattern_engine/detectors/chart.py`
  - Classes:
    - `HeadAndShouldersDetector`
    - `InvertedHeadAndShouldersDetector`

#### 4.2 Implementation Requirements

```python
class HeadAndShouldersDetector(PatternDetectorBase):
    pattern_code = ChartPatternCode.HEAD_AND_SHOULDERS

    def __init__(self, pivot_config: PivotConfig, hns_config: HNSConfig):
        self.pivot_config = pivot_config
        self.hns_config = hns_config

    def detect(self, window: CandleWindow) -> list[PatternSignature]:
        """
        Detect HNS patterns using pivot analysis.

        Steps:
        1. Find pivots (fractal window)
        2. Check last 3 highs for HNS structure
        3. Compute neckline
        4. Return signature with pivot points
        """
        # Implementation here
        pass

    def check_confirmation(self, instance: PatternInstance, window: CandleWindow) -> Optional[dict]:
        """Check neckline break."""
        # Implementation: price < neckline by threshold
        pass

    def check_invalidation(self, instance: PatternInstance, window: CandleWindow) -> Optional[dict]:
        """Check if price broke above RS high (or HEAD high)."""
        # Implementation here
        pass
```

**Acceptance Criteria**:

- [x] Both detectors implemented
- [x] Pivot detection works correctly (fractal window k=3)
- [x] PatternPoint records created for LS, HEAD, RS, neckline lows
- [x] ChartPatternDetail populated with neckline_slope, head_prominence_pct, shoulder_symmetry
- [x] Unit tests pass (see Test Plan)

**Estimated Time**: 3 days

---

### Milestone 5: Use Case Orchestrator (Week 3)

**Goal**: Implement main orchestration logic.

#### 5.1 Files to Create

- [ ] `apps/backend/monolith/api/application/pattern_engine/use_cases.py`
  - Classes:
    - `ScanForPatternsUseCase` (main orchestrator)

#### 5.2 Implementation

```python
class ScanForPatternsUseCase:
    """
    Main orchestrator for pattern detection.

    Workflow:
    1. Fetch candle window
    2. Run all detectors (candlestick + chart)
    3. For each signature: get_or_create_instance (idempotent)
    4. For FORMING instances: check confirmation/invalidation
    5. Update status + emit alerts
    6. Store detail models + pattern points
    7. Return lifecycle events
    """

    def __init__(
        self,
        candle_provider: CandleProvider,
        pattern_repository: PatternRepository,
        detectors: list[PatternDetector],
    ):
        self.candle_provider = candle_provider
        self.pattern_repository = pattern_repository
        self.detectors = detectors

    def execute(self, symbol: str, timeframe: str, candle_limit: int = 100) -> list[PatternLifecycleEvent]:
        """
        Execute pattern scanning.

        Returns list of lifecycle events (for audit trail).
        """
        # Implementation here
        pass
```

**Acceptance Criteria**:

- [x] Use case runs all detectors
- [x] Idempotency: re-running produces no duplicates
- [x] FORMING → CONFIRMED transitions work
- [x] FORMING → FAILED/INVALIDATED transitions work
- [x] Integration test passes (see Test Plan)

**Estimated Time**: 2 days

---

### Milestone 6: Management Command (Week 3)

**Goal**: Create CLI command for manual/scheduled execution.

#### 6.1 Files to Create

- [ ] `apps/backend/monolith/api/management/commands/scan_patterns.py`

#### 6.2 Implementation

```python
from django.core.management.base import BaseCommand
from api.application.pattern_engine import ScanForPatternsUseCase
from api.application.pattern_engine.adapters import BinanceCandleProvider, DjangoPatternRepository
from api.application.pattern_engine.detectors import (
    HammerDetector,
    InvertedHammerDetector,
    BullishEngulfingDetector,
    BearishEngulfingDetector,
    HeadAndShouldersDetector,
    InvertedHeadAndShouldersDetector,
)
from api.application.pattern_engine.config import (
    DEFAULT_HAMMER_CONFIG,
    DEFAULT_ENGULFING_CONFIG,
    DEFAULT_HNS_CONFIG,
    DEFAULT_PIVOT_CONFIG,
)

class Command(BaseCommand):
    help = "Scan for technical patterns on BTC/USDT 15m"

    def add_arguments(self, parser):
        parser.add_argument("--symbol", default="BTCUSDT")
        parser.add_argument("--timeframe", default="15m")
        parser.add_argument("--candle-limit", type=int, default=100)
        parser.add_argument("--continuous", action="store_true")
        parser.add_argument("--interval", type=int, default=900)  # 15 minutes

    def handle(self, *args, **options):
        # Setup dependencies
        candle_provider = BinanceCandleProvider()
        repository = DjangoPatternRepository()

        # Setup detectors
        detectors = [
            HammerDetector(DEFAULT_HAMMER_CONFIG),
            InvertedHammerDetector(DEFAULT_HAMMER_CONFIG),  # Same config
            BullishEngulfingDetector(DEFAULT_ENGULFING_CONFIG),
            BearishEngulfingDetector(DEFAULT_ENGULFING_CONFIG),
            HeadAndShouldersDetector(DEFAULT_PIVOT_CONFIG, DEFAULT_HNS_CONFIG),
            InvertedHeadAndShouldersDetector(DEFAULT_PIVOT_CONFIG, DEFAULT_HNS_CONFIG),
        ]

        # Create use case
        use_case = ScanForPatternsUseCase(candle_provider, repository, detectors)

        # Execute
        symbol = options["symbol"]
        timeframe = options["timeframe"]
        candle_limit = options["candle_limit"]

        if options["continuous"]:
            # Continuous mode (loop)
            import time
            interval = options["interval"]
            while True:
                events = use_case.execute(symbol, timeframe, candle_limit)
                self.stdout.write(f"Scanned {symbol} {timeframe}: {len(events)} events")
                time.sleep(interval)
        else:
            # Single run
            events = use_case.execute(symbol, timeframe, candle_limit)
            self.stdout.write(f"Scanned {symbol} {timeframe}: {len(events)} events")
            for event in events:
                self.stdout.write(f"  - {event.event_type}: {event.instance_id}")
```

**Usage**:

```bash
# Single run
python manage.py scan_patterns

# Specific symbol/timeframe
python manage.py scan_patterns --symbol ETHUSDT --timeframe 1h

# Continuous mode (runs every 15 minutes)
python manage.py scan_patterns --continuous --interval 900

# Fetch more candles for chart patterns
python manage.py scan_patterns --candle-limit 200
```

**Acceptance Criteria**:

- [x] Command runs without errors
- [x] Single-run mode works
- [x] Continuous mode works (Ctrl+C to stop)
- [x] Output shows detected patterns and alerts

**Estimated Time**: 1 day

---

### Milestone 7: Tests (Week 4)

**Goal**: Comprehensive test coverage (see Test Plan below).

#### 7.1 Files to Create

- [ ] `apps/backend/monolith/api/tests/test_pattern_engine.py` (unit tests)
- [ ] `apps/backend/monolith/api/tests/test_pattern_detectors.py` (detector tests)
- [ ] `apps/backend/monolith/api/tests/test_pattern_idempotency.py` (idempotency tests)

**Acceptance Criteria**:

- [x] All tests pass
- [x] Coverage >= 80% for pattern_engine module
- [x] Property tests verify monotonic guarantees

**Estimated Time**: 3 days

---

### Milestone 8: Documentation & Validation (Week 4)

**Goal**: Finalize docs and get production approval.

#### 8.1 Tasks

- [ ] Update CLAUDE.md with pattern engine context
- [ ] Create example notebooks (optional)
- [ ] Run backtest on historical data (recommended, not required)
- [ ] User acceptance testing
- [ ] Production deployment plan (Kubernetes CronJob)

**Acceptance Criteria**:

- [x] Documentation complete
- [x] User validation passed
- [x] Ready for production deployment

**Estimated Time**: 2 days

---

## Test Plan Checklist

### Unit Tests (Pure Functions)

#### Test Suite: `test_pattern_engine.py`

**Helper Functions**:

- [ ] `test_compute_candle_metrics_normal`
  - Input: OHLCV with normal values
  - Expected: Correct body_pct, wick_pcts
- [ ] `test_compute_candle_metrics_degenerate`
  - Input: OHLCV with range=0 (high=low)
  - Expected: Returns None
- [ ] `test_find_pivots_fractal_window_k3`
  - Input: 20 candles with known highs/lows
  - Expected: Correct pivot points identified
- [ ] `test_find_pivots_insufficient_data`
  - Input: 5 candles (< 2*k+1)
  - Expected: Returns empty list

**Config Tests**:

- [ ] `test_default_configs_load`
  - Expected: All default configs load without errors

---

### Detector Tests (Golden Sequences)

#### Test Suite: `test_pattern_detectors.py`

**Hammer Detector**:

- [ ] `test_hammer_detect_perfect_hammer`
  - Input: OHLCV with lower_wick_pct=0.70, body_pct=0.25, upper_wick_pct=0.05
  - Expected: `detect() == True`
- [ ] `test_hammer_detect_borderline_pass`
  - Input: OHLCV with lower_wick_pct=0.60 (exactly at threshold)
  - Expected: `detect() == True`
- [ ] `test_hammer_detect_borderline_fail`
  - Input: OHLCV with lower_wick_pct=0.59 (just below threshold)
  - Expected: `detect() == False`
- [ ] `test_hammer_confirmation_close_above_midpoint`
  - Input: Hammer + next candle closes above midpoint
  - Expected: `check_confirmation() == True`
- [ ] `test_hammer_confirmation_fail_no_follow_through`
  - Input: Hammer + 3 next candles all close below midpoint
  - Expected: `check_confirmation() == False`
- [ ] `test_hammer_invalidation_close_below_low`
  - Input: Hammer + next candle closes below hammer low
  - Expected: `check_invalidation() == True`

**Engulfing Detector**:

- [ ] `test_bullish_engulfing_detect_perfect`
  - Input: Prev bearish, Curr bullish, full engulf, ratio=1.5
  - Expected: `detect() == True`
- [ ] `test_bullish_engulfing_detect_fail_no_engulf`
  - Input: Prev bearish, Curr bullish, but no engulf (curr.close < prev.open)
  - Expected: `detect() == False`
- [ ] `test_bullish_engulfing_detect_fail_low_ratio`
  - Input: Engulf but ratio=1.1 (< 1.2 threshold)
  - Expected: `detect() == False`
- [ ] `test_bearish_engulfing_detect_symmetric`
  - Input: Prev bullish, Curr bearish, full engulf
  - Expected: `detect() == True`

**HNS Detector**:

- [ ] `test_hns_detect_perfect_hns`
  - Input: 3 highs (LS=51000, HEAD=51500, RS=50950), neckline flat
  - Expected: `detect() == True`, signature includes LS/HEAD/RS
- [ ] `test_hns_detect_fail_head_not_highest`
  - Input: 3 highs but RS > HEAD
  - Expected: `detect() == False`
- [ ] `test_hns_detect_fail_asymmetric_shoulders`
  - Input: LS=51000, HEAD=51500, RS=50500 (shoulders differ by >2%)
  - Expected: `detect() == False`
- [ ] `test_hns_detect_fail_insufficient_pivots`
  - Input: Only 2 highs
  - Expected: `detect() == False`
- [ ] `test_hns_confirmation_neckline_break`
  - Input: HNS + price closes below neckline by 0.6%
  - Expected: `check_confirmation() == True`
- [ ] `test_hns_invalidation_close_above_rs`
  - Input: HNS + price closes above RS high
  - Expected: `check_invalidation() == True`

---

### Integration Tests

#### Test Suite: `test_pattern_engine.py`

- [ ] `test_end_to_end_hammer_lifecycle`
  - Setup: Generate OHLCV sequence with hammer + confirmation
  - Execute: `ScanForPatternsUseCase.execute()`
  - Assert:
    - PatternInstance created with status=FORMING
    - PatternAlert(FORMING) emitted
    - After confirmation candles: status=CONFIRMED
    - PatternAlert(CONFIRM) emitted
    - CandlestickPatternDetail created
    - All timestamps deterministic (from candle data)

- [ ] `test_end_to_end_hns_lifecycle`
  - Setup: Generate OHLCV with HNS structure + neckline break
  - Execute: `ScanForPatternsUseCase.execute()`
  - Assert:
    - PatternInstance created with status=FORMING
    - PatternPoint records created (LS, HEAD, RS, neckline)
    - ChartPatternDetail created
    - After neckline break: status=CONFIRMED
    - PatternAlert(CONFIRM) emitted

---

### Idempotency Tests

#### Test Suite: `test_pattern_idempotency.py`

- [ ] `test_idempotent_instance_creation`
  - Setup: Run `ScanForPatternsUseCase.execute()` twice with same OHLCV
  - Assert:
    - Only 1 PatternInstance created (no duplicates)
    - Same instance_id returned both times

- [ ] `test_idempotent_alert_emission`
  - Setup: Run use case twice with same OHLCV + confirmation
  - Assert:
    - Only 1 PatternAlert(FORMING) exists
    - Only 1 PatternAlert(CONFIRM) exists
    - No duplicate alerts

- [ ] `test_idempotent_pattern_points`
  - Setup: Run HNS detector twice with same pivots
  - Assert:
    - Only 1 set of PatternPoint records
    - No duplicates

---

### Property Tests (Hypothesis)

- [ ] `test_property_no_duplicates_on_rescan`
  - Property: For any OHLCV sequence, re-scanning produces same DB state
  - Strategy: Generate random OHLCV sequences, scan twice, compare DB counts

- [ ] `test_property_timestamps_deterministic`
  - Property: alert_ts always comes from candle data, never datetime.now()
  - Strategy: Mock datetime.now(), verify alerts use candle timestamps

- [ ] `test_property_status_monotonic`
  - Property: Status transitions never go backwards (e.g., CONFIRMED → FORMING)
  - Strategy: Generate lifecycle sequences, verify state machine correctness

---

## File Path Summary

### New Files (to be created in PHASE 1)

```
apps/backend/monolith/api/application/pattern_engine/
├── __init__.py
├── domain.py
├── ports.py
├── config.py
├── helpers.py
├── adapters.py
├── use_cases.py
├── detectors/
│   ├── __init__.py
│   ├── base.py
│   ├── candlestick.py
│   └── chart.py

apps/backend/monolith/api/management/commands/
└── scan_patterns.py

apps/backend/monolith/api/tests/
├── test_pattern_engine.py
├── test_pattern_detectors.py
└── test_pattern_idempotency.py
```

### Existing Files (to use, NOT modify)

```
apps/backend/monolith/api/models/patterns/
├── base.py                    # PatternInstance, PatternAlert, etc.
├── candlestick.py             # CandlestickPatternDetail
└── chart.py                   # ChartPatternDetail

apps/backend/monolith/api/services/
└── market_data_service.py     # MarketDataService.get_historical_data()
```

---

## Risk Mitigation

### Risk 1: Candle Data Staleness

**Mitigation**:

- Implement freshness check in `BinanceCandleProvider`
- Log warning if candles > 20 minutes old
- Optionally skip detection if data too stale

### Risk 2: Detector Configuration Uncertainty

**Mitigation**:

- Start with conservative thresholds from literature
- Log all thresholds in evidence payload
- Plan for backtest-driven tuning in future

### Risk 3: Database Performance (Large Pattern Counts)

**Mitigation**:

- Use indexes on (symbol, status, breakout_ts)
- Use `select_related()` / `prefetch_related()` in queries
- Monitor query performance with Django debug toolbar

### Risk 4: Integration with Entry Gate Not Clear

**Mitigation**:

- Pattern Engine is standalone (emits data only)
- Entry Gate integration is Phase 2 (out of scope v1)
- Provide clear query examples in docs

---

## Success Criteria (PHASE 1 Complete)

- [x] All milestones complete
- [x] All tests pass (coverage >= 80%)
- [x] Management command runs successfully
- [x] No duplicate instances/alerts on re-scan
- [x] Documentation complete
- [x] Code review approved

---

## Timeline Summary

| Milestone | Duration | Cumulative |
|-----------|----------|------------|
| M1: Infrastructure | 1 day | Day 1 |
| M2: Adapters | 2 days | Day 3 |
| M3: Candlestick Detectors | 3 days | Day 6 |
| M4: Chart Detectors | 3 days | Day 9 |
| M5: Use Case | 2 days | Day 11 |
| M6: Management Command | 1 day | Day 12 |
| M7: Tests | 3 days | Day 15 |
| M8: Docs & Validation | 2 days | Day 17 |

**Total**: ~17 working days (~3.5 weeks)

---

## PHASE 0 Outputs (Complete)

1. ✅ **ADR**: `docs/adr/ADR-0018-pattern-detection-engine.md`
2. ✅ **Specification**: `docs/strategy/PATTERN_ENGINE_V1.md`
3. ✅ **Milestone Checklist**: This document (Milestones 1-8)
4. ✅ **Test Plan Checklist**: This document (Unit, Integration, Property tests)

---

**Next Step**: **USER VALIDATION REQUIRED** ✋

Do NOT proceed to PHASE 1 (implementation) until:

- [ ] ADR-0018 reviewed and approved
- [ ] PATTERN_ENGINE_V1.md reviewed and approved
- [ ] This implementation plan reviewed and approved
- [ ] Threshold configs validated (or accepted as heuristics)

---

**Last Updated**: 2025-12-28
