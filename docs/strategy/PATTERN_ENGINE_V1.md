# Pattern Detection Engine v1.0

**Version:** 1.0.0
**Date:** 2025-12-28
**Status:** ✅ IMPLEMENTED (M1-M8 Complete)

---

## Overview

The Pattern Detection Engine is an automated scanner that identifies technical patterns (candlestick, chart) on BTC/USDT 15m candles and manages their lifecycle through:

1. **FORMING** → Initial pattern signature detected
2. **CONFIRMED** → Confirmation rules met
3. **FAILED/INVALIDATED** → Invalidation rules met
4. **TARGET_HIT** → Price targets reached (optional follow-up)

**Key Properties**:

- **Deterministic**: Same OHLC → Same patterns (no subjective interpretation)
- **Idempotent**: Re-scanning doesn't create duplicates
- **Auditable**: Every detection includes version, thresholds, evidence
- **Non-Executing**: Emits alerts, does NOT place orders

---

## Scope (v1)

### Patterns Implemented

**Candlestick** (5 patterns):

1. `HAMMER` - Bullish reversal (1 candle)
2. `INVERTED_HAMMER` - Bullish reversal (1 candle)
3. `BULLISH_ENGULFING` - Bullish reversal (2 candles)
4. `BEARISH_ENGULFING` - Bearish reversal (2 candles)
5. `MORNING_STAR` - Bullish reversal (3 candles) *Added during implementation*

**Chart** (2 patterns):
6. `HEAD_AND_SHOULDERS` - Bearish reversal
7. `INVERTED_HEAD_AND_SHOULDERS` - Bullish reversal

**Total**: 7 patterns (5 candlestick + 2 chart)

### Timeframe & Symbol

- **Timeframe**: 15m (initially)
- **Symbol**: BTC/USDT (initially)

Architecture supports multi-timeframe/multi-symbol in future versions.

---

## Candle Anatomy (Shared Metrics)

All candlestick pattern detectors use these deterministic metrics:

```python
def compute_candle_metrics(candle: OHLCV) -> dict:
    """
    Compute candle anatomy.

    Returns None for degenerate candles (range = 0).
    """
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

**Metrics**:

- `body`: `|close - open|`
- `range`: `high - low` (total candle range)
- `upper_wick`: Distance from body top to high
- `lower_wick`: Distance from body bottom to low
- `body_pct`: `body / range` (body as % of total range)
- `upper_wick_pct`: `upper_wick / range`
- `lower_wick_pct`: `lower_wick / range`

**Constraints**:

- All percentages sum to 1.0: `body_pct + upper_wick_pct + lower_wick_pct = 1.0`
- Degenerate candles (`range = 0`) return `None` and are skipped

---

## Pattern 1: Hammer (Bullish Reversal)

### Visual Description

```
      |  ← Small upper wick (≤15% of range)
      □  ← Small body (≤30% of range)
      □
      |
      |  ← Long lower wick (≥60% of range)
      |
      *
```

### Detection Rules

**Primary Signature**:

```python
@dataclass(frozen=True)
class HammerConfig:
    lower_wick_pct_min: Decimal = Decimal("0.60")  # 60%
    body_pct_max: Decimal = Decimal("0.30")        # 30%
    upper_wick_pct_max: Decimal = Decimal("0.15")  # 15%
```

**Detection Function**:

```python
def detect_hammer(candle: OHLCV, config: HammerConfig) -> bool:
    metrics = compute_candle_metrics(candle)
    if not metrics:
        return False  # Degenerate candle

    return (
        metrics["lower_wick_pct"] >= config.lower_wick_pct_min and
        metrics["body_pct"] <= config.body_pct_max and
        metrics["upper_wick_pct"] <= config.upper_wick_pct_max
    )
```

### Context Filter (Optional, Configurable)

**Goal**: Hammer is more reliable after a downtrend.

```python
@dataclass(frozen=True)
class HammerContextConfig:
    enable_context_filter: bool = True
    prior_trend_bars: int = 10  # Check last N bars
    prior_return_threshold: Decimal = Decimal("-0.03")  # -3%
    use_ma_filter: bool = False  # Alternative: price below MA(N)
    ma_period: int = 20

def check_prior_downtrend(candles: list[OHLCV], config: HammerContextConfig) -> bool:
    """
    Check if prior N bars show downtrend.

    Returns True if:
    - return_pct over N bars <= threshold (e.g., -3%), OR
    - current price < MA(N) (if use_ma_filter=True)
    """
    if len(candles) < config.prior_trend_bars:
        return False  # Not enough history

    prior = candles[-config.prior_trend_bars-1:-1]  # Last N bars before hammer
    return_pct = (prior[-1].close - prior[0].close) / prior[0].close

    # Option A: Return threshold
    if return_pct <= config.prior_return_threshold:
        return True

    # Option B: MA filter (if enabled)
    if config.use_ma_filter:
        ma_n = sum(c.close for c in prior) / len(prior)
        if candles[-1].close < ma_n:
            return True

    return False
```

### Confirmation Rules

**Goal**: Confirm hammer with follow-through price action.

```python
@dataclass(frozen=True)
class HammerConfirmConfig:
    confirm_window_bars: int = 3  # Check next M bars
    confirm_close_above_midpoint: bool = True
    confirm_close_above_high: bool = False  # Stricter: close above hammer high

def check_hammer_confirmation(
    hammer_candle: OHLCV,
    next_candles: list[OHLCV],
    config: HammerConfirmConfig
) -> bool:
    """
    Confirm if next candle(s) close above hammer midpoint (or high).

    Returns True if ANY candle in next M bars satisfies condition.
    """
    if config.confirm_close_above_high:
        threshold = hammer_candle.high
    else:
        threshold = (hammer_candle.high + hammer_candle.low) / 2  # Midpoint

    for candle in next_candles[:config.confirm_window_bars]:
        if candle.close > threshold:
            return True

    return False
```

### Lifecycle States

| State | Trigger | Evidence Stored |
|-------|---------|-----------------|
| **FORMING** | `detect_hammer() == True` | `body_pct`, `lower_wick_pct`, `upper_wick_pct`, thresholds |
| **CONFIRMED** | `check_hammer_confirmation() == True` | Confirmation candle ts, close price |
| **FAILED** | `check_hammer_confirmation() == False` after `confirm_window_bars` | Reason: "No confirmation within 3 bars" |
| **INVALIDATED** | Price closes below hammer low | Invalidation price, ts |

### Evidence Payload (Example)

**FORMING Alert**:

```json
{
  "alert_type": "FORMING",
  "pattern_code": "HAMMER",
  "ts": "2025-12-28T10:00:00Z",
  "confidence": 0.75,
  "payload": {
    "candle": {
      "open": 50000.0,
      "high": 50200.0,
      "low": 49500.0,
      "close": 49900.0
    },
    "metrics": {
      "body_pct": 0.28,
      "lower_wick_pct": 0.64,
      "upper_wick_pct": 0.08
    },
    "thresholds": {
      "lower_wick_pct_min": 0.60,
      "body_pct_max": 0.30,
      "upper_wick_pct_max": 0.15
    },
    "context": {
      "prior_return_pct": -0.04,
      "prior_trend_bars": 10
    },
    "version": "pattern_engine_v1.0.0"
  }
}
```

---

## Pattern 2: Inverted Hammer (Bullish Reversal)

### Visual Description

```
      *
      |  ← Long upper wick (≥60% of range)
      |
      |
      □  ← Small body (≤30% of range)
      □
      |  ← Small lower wick (≤15% of range)
```

### Detection Rules

**Primary Signature**:

```python
@dataclass(frozen=True)
class InvertedHammerConfig:
    upper_wick_pct_min: Decimal = Decimal("0.60")  # 60%
    body_pct_max: Decimal = Decimal("0.30")        # 30%
    lower_wick_pct_max: Decimal = Decimal("0.15")  # 15%
```

**Detection Function**:

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

### Context + Confirmation

**Same as Hammer** (prior downtrend, next candle confirmation).

---

## Pattern 3: Bullish Engulfing

### Visual Description

```
Prev candle:     Current candle:
    □                 ■■■■
    □                 ■■■■
    ■                 ■■■■
    ■                 ■■■■
                      ■■■■
```

Prev = bearish (close < open), Current = bullish, Current body engulfs Prev body.

### Detection Rules

**Primary Signature**:

```python
@dataclass(frozen=True)
class EngulfingConfig:
    min_engulf_ratio: Decimal = Decimal("1.2")  # Current body ≥ 1.2x prev body
    require_full_engulf: bool = True  # Strict: open ≤ prev.close, close ≥ prev.open

def detect_bullish_engulfing(prev: OHLCV, curr: OHLCV, config: EngulfingConfig) -> bool:
    """
    Detect bullish engulfing pattern.

    Rules:
    1. Prev candle is bearish (close < open)
    2. Current candle is bullish (close > open)
    3. Current body engulfs prev body (open_curr ≤ close_prev AND close_curr ≥ open_prev)
    4. Engulf ratio ≥ threshold (current_body / prev_body ≥ min_engulf_ratio)
    """
    # Prev candle bearish
    if prev.close >= prev.open:
        return False

    # Current candle bullish
    if curr.close <= curr.open:
        return False

    # Full engulfment check
    if config.require_full_engulf:
        if not (curr.open <= prev.close and curr.close >= prev.open):
            return False

    # Engulf ratio
    prev_body = abs(prev.close - prev.open)
    curr_body = abs(curr.close - curr.open)
    engulf_ratio = curr_body / max(prev_body, Decimal("0.0001"))

    return engulf_ratio >= config.min_engulf_ratio
```

### Evidence Payload (Example)

```json
{
  "alert_type": "FORMING",
  "pattern_code": "ENGULFING_BULL",
  "ts": "2025-12-28T10:15:00Z",
  "confidence": 0.80,
  "payload": {
    "prev_candle": {
      "open": 50000.0,
      "close": 49800.0,
      "body": 200.0
    },
    "curr_candle": {
      "open": 49750.0,
      "close": 50100.0,
      "body": 350.0
    },
    "engulf_ratio": 1.75,
    "thresholds": {
      "min_engulf_ratio": 1.2
    },
    "version": "pattern_engine_v1.0.0"
  }
}
```

### Confirmation (Optional)

**Rule**: Next candle closes in same direction (bullish for bullish engulfing).

```python
def check_engulfing_confirmation(engulfing_candle: OHLCV, next_candle: OHLCV) -> bool:
    """Confirm if next candle is also bullish (for bullish engulfing)."""
    return next_candle.close > next_candle.open
```

---

## Pattern 4: Bearish Engulfing

### Detection Rules

**Symmetric to Bullish Engulfing**:

- Prev candle is **bullish** (close > open)
- Current candle is **bearish** (close < open)
- Current body engulfs prev body (open_curr ≥ close_prev AND close_curr ≤ open_prev)
- Engulf ratio ≥ threshold

---

## Pattern 5: Head and Shoulders (Bearish Reversal)

### Visual Description

```
            HEAD
             ▲
            / \
           /   \
          /     \
    LS   /       \   RS
     ▲  /         \  ▲
    / \/           \/ \
   /                  \
  ------------------------  Neckline
```

**Components**:

- **LS** (Left Shoulder): First pivot high
- **HEAD**: Highest pivot high
- **RS** (Right Shoulder): Third pivot high (similar height to LS)
- **Neckline**: Support line connecting lows between LS-HEAD and HEAD-RS

### Detection Rules

**Step 1: Pivot Detection** (Fractal Window)

```python
@dataclass(frozen=True)
class PivotConfig:
    fractal_window: int = 3  # k = 3 (check 3 bars left + right)

def find_pivots(candles: list[OHLCV], config: PivotConfig) -> list[PivotPoint]:
    """
    Find swing highs and lows using fractal window.

    A pivot high at i exists if:
    - high[i] >= high[i-k] ... high[i+k]

    A pivot low at i exists if:
    - low[i] <= low[i-k] ... low[i+k]
    """
    k = config.fractal_window
    pivots = []

    for i in range(k, len(candles) - k):
        window = candles[i-k:i+k+1]

        # Pivot high check
        is_high = all(candles[i].high >= c.high for c in window)
        if is_high:
            pivots.append(PivotPoint(
                ts=candles[i].ts,
                price=candles[i].high,
                type="HIGH",
                bar_index=i,
            ))

        # Pivot low check
        is_low = all(candles[i].low <= c.low for c in window)
        if is_low:
            pivots.append(PivotPoint(
                ts=candles[i].ts,
                price=candles[i].low,
                type="LOW",
                bar_index=i,
            ))

    return pivots
```

**Step 2: HNS Pattern Recognition**

```python
@dataclass(frozen=True)
class HNSConfig:
    min_pivots_required: int = 3  # Need at least 3 highs
    shoulder_tolerance_pct: Decimal = Decimal("0.02")  # Shoulders within 2% of each other
    head_prominence_min_pct: Decimal = Decimal("0.05")  # Head must be 5% higher than shoulders
    neckline_slope_tolerance: Decimal = Decimal("0.01")  # Neckline slope ≤ 1% (relatively flat)

def detect_head_and_shoulders(pivots: list[PivotPoint], config: HNSConfig) -> Optional[HNSSignature]:
    """
    Detect HNS pattern from pivot points.

    Returns HNSSignature if detected, None otherwise.
    """
    highs = [p for p in pivots if p.type == "HIGH"]
    lows = [p for p in pivots if p.type == "LOW"]

    if len(highs) < config.min_pivots_required:
        return None

    # Get last 3 highs: LS, HEAD, RS
    ls, head, rs = highs[-3:]

    # Rule 1: Head must be highest
    if not (head.price > ls.price and head.price > rs.price):
        return None

    # Rule 2: Head prominence
    avg_shoulder = (ls.price + rs.price) / 2
    head_prominence_pct = (head.price - avg_shoulder) / avg_shoulder

    if head_prominence_pct < config.head_prominence_min_pct:
        return None

    # Rule 3: Shoulders within tolerance
    shoulder_diff = abs(ls.price - rs.price)
    shoulder_tol = config.shoulder_tolerance_pct * avg_shoulder

    if shoulder_diff > shoulder_tol:
        return None

    # Rule 4: Find neckline (lows between LS-HEAD and HEAD-RS)
    neckline_lows = [
        p for p in lows
        if ls.bar_index < p.bar_index < head.bar_index or
           head.bar_index < p.bar_index < rs.bar_index
    ]

    if len(neckline_lows) < 2:
        return None  # Need at least 2 lows for neckline

    # Compute neckline (linear fit of neckline lows)
    neckline_slope, neckline_intercept = compute_line_fit(neckline_lows)

    # Rule 5: Neckline relatively flat (slope tolerance)
    if abs(neckline_slope) > config.neckline_slope_tolerance:
        return None

    # Compute confidence (0-1)
    confidence = compute_hns_confidence(ls, head, rs, neckline_slope, config)

    return HNSSignature(
        ls=ls,
        head=head,
        rs=rs,
        neckline_slope=neckline_slope,
        neckline_intercept=neckline_intercept,
        neckline_lows=neckline_lows,
        confidence=confidence,
    )
```

### Confirmation Rules

**Neckline Break**:

```python
@dataclass(frozen=True)
class HNSConfirmConfig:
    break_tolerance_pct: Decimal = Decimal("0.005")  # 0.5% below neckline

def check_hns_confirmation(hns: HNSSignature, current_price: Decimal, current_ts: datetime, config: HNSConfirmConfig) -> bool:
    """
    Confirm HNS if price breaks neckline by threshold.

    Neckline level at current_ts = neckline_slope * time_offset + neckline_intercept
    (Simplified: use neckline at RS timestamp)
    """
    neckline_level = hns.neckline_slope * (current_ts - hns.rs.ts).total_seconds() + hns.neckline_intercept
    break_threshold = neckline_level * (Decimal("1") - config.break_tolerance_pct)

    return current_price < break_threshold  # Bearish break
```

### Invalidation Rules

```python
@dataclass(frozen=True)
class HNSInvalidationConfig:
    invalidation_level: str = "RS_HIGH"  # "RS_HIGH" or "HEAD_HIGH"

def check_hns_invalidation(hns: HNSSignature, current_price: Decimal, config: HNSInvalidationConfig) -> bool:
    """
    Invalidate HNS if price closes above invalidation level.

    Options:
    - "RS_HIGH": price > RS high (lenient)
    - "HEAD_HIGH": price > HEAD high (strict)
    """
    if config.invalidation_level == "HEAD_HIGH":
        invalidation_price = hns.head.price
    else:  # Default: RS_HIGH
        invalidation_price = hns.rs.price

    return current_price > invalidation_price
```

### PatternPoint Storage (CRITICAL)

**For chart patterns like HNS, `PatternPoint` records MUST be created for reproducibility.**

```python
def store_hns_points(instance: PatternInstance, hns: HNSSignature) -> None:
    """Store HNS pivot points for audit trail."""
    PatternPoint.objects.bulk_create([
        PatternPoint(
            instance=instance,
            label="LS",
            ts=hns.ls.ts,
            price=hns.ls.price,
            bar_index_offset=(hns.ls.ts - instance.start_ts).total_seconds() / (15 * 60),
            role="LEFT_SHOULDER",
        ),
        PatternPoint(
            instance=instance,
            label="HEAD",
            ts=hns.head.ts,
            price=hns.head.price,
            bar_index_offset=(hns.head.ts - instance.start_ts).total_seconds() / (15 * 60),
            role="HEAD",
        ),
        PatternPoint(
            instance=instance,
            label="RS",
            ts=hns.rs.ts,
            price=hns.rs.price,
            bar_index_offset=(hns.rs.ts - instance.start_ts).total_seconds() / (15 * 60),
            role="RIGHT_SHOULDER",
        ),
        # Neckline lows
        *[
            PatternPoint(
                instance=instance,
                label=f"NECKLINE_{i}",
                ts=low.ts,
                price=low.price,
                bar_index_offset=(low.ts - instance.start_ts).total_seconds() / (15 * 60),
                role="NECKLINE",
            )
            for i, low in enumerate(hns.neckline_lows)
        ],
    ])
```

### ChartPatternDetail Storage

```python
def store_hns_detail(instance: PatternInstance, hns: HNSSignature) -> None:
    """Store HNS-specific metrics in ChartPatternDetail."""
    ChartPatternDetail.objects.create(
        instance=instance,
        neckline_slope=hns.neckline_slope,
        head_prominence_pct=(hns.head.price - (hns.ls.price + hns.rs.price)/2) / ((hns.ls.price + hns.rs.price)/2) * 100,
        shoulder_symmetry=abs(hns.ls.price - hns.rs.price) / ((hns.ls.price + hns.rs.price)/2) * 100,
        width_bars=(hns.rs.ts - hns.ls.ts).total_seconds() / (15 * 60),
        height_pct=(hns.head.price - hns.neckline_intercept) / hns.head.price * 100,
    )
```

---

## Pattern 6: Inverted Head and Shoulders (Bullish Reversal)

### Detection Rules

**Symmetric to HNS** (inverted):

- Pivots are **lows** instead of highs
- Neckline is **resistance** instead of support
- Confirmation: price breaks **above** neckline
- Invalidation: price closes **below** RS low (or HEAD low)

---

## Lifecycle Management

### State Transitions

```
    ┌─────────┐
    │ FORMING │  (Initial detection)
    └────┬────┘
         │
    ┌────▼────────────────────┐
    │  Check Confirmation?     │
    └────┬──────────┬──────────┘
         │          │
    ┌────▼─────┐   └──────► FAILED (no confirmation within window)
    │CONFIRMED │
    └────┬─────┘
         │
    ┌────▼──────────────────┐
    │ Check Invalidation?   │
    └────┬─────────┬─────────┘
         │         │
   INVALIDATED     TARGET_HIT (if tp1/tp2/tp3 reached)
```

### Alert Emission

| Transition | Alert Type | Payload |
|------------|------------|---------|
| Detection → FORMING | `FORMING` | Pattern signature, metrics, thresholds |
| FORMING → CONFIRMED | `CONFIRM` | Confirmation candle, timestamp |
| FORMING → FAILED | None (silent failure, logged) | - |
| CONFIRMED → INVALIDATED | `INVALIDATE` | Invalidation price, timestamp |
| CONFIRMED → TARGET_HIT | `TARGET_HIT` | Target level (tp1/tp2/tp3), timestamp |

---

## Idempotency Strategy

### Instance Uniqueness

**Key**: `(pattern_code, symbol, timeframe, start_ts)`

```python
def get_or_create_instance(signature: PatternSignature) -> PatternInstance:
    """
    Idempotent instance creation.

    Running detector multiple times with same OHLC produces:
    - Same PatternInstance (no duplicates)
    - Same PatternPoint records
    - Same PatternAlert records
    """
    instance, created = PatternInstance.objects.get_or_create(
        pattern__pattern_code=signature.pattern_code,
        symbol__name=signature.symbol,
        timeframe=signature.timeframe,
        start_ts=signature.start_ts,
        defaults={
            "status": PatternStatus.FORMING,
            "detected_version": DETECTOR_VERSION,
            "features": signature.evidence,
            "end_ts": signature.end_ts,
        }
    )

    if not created and instance.status == PatternStatus.FORMING:
        # Update evidence if pattern still forming (idempotent update)
        instance.features = signature.evidence
        instance.end_ts = signature.end_ts
        instance.save(update_fields=["features", "end_ts"])

    return instance
```

### Alert Uniqueness

**Key**: `(instance_id, alert_type, alert_ts)`

Use `alert_ts` from candle timestamp (NOT `datetime.now()`) for determinism.

```python
def emit_alert(instance_id: int, alert_type: str, event_ts: datetime, confidence: Decimal, payload: dict) -> None:
    """
    Idempotent alert emission.

    Multiple calls with same (instance_id, alert_type, event_ts) produce single alert.
    """
    PatternAlert.objects.get_or_create(
        instance_id=instance_id,
        alert_type=alert_type,
        alert_ts=event_ts,  # Use candle timestamp, NOT now()
        defaults={
            "confidence": confidence,
            "payload": payload,
        }
    )
```

---

## Configuration Defaults

### Candlestick Patterns

```python
DEFAULT_HAMMER_CONFIG = HammerConfig(
    lower_wick_pct_min=Decimal("0.60"),
    body_pct_max=Decimal("0.30"),
    upper_wick_pct_max=Decimal("0.15"),
)

DEFAULT_HAMMER_CONTEXT_CONFIG = HammerContextConfig(
    enable_context_filter=True,
    prior_trend_bars=10,
    prior_return_threshold=Decimal("-0.03"),
    use_ma_filter=False,
    ma_period=20,
)

DEFAULT_HAMMER_CONFIRM_CONFIG = HammerConfirmConfig(
    confirm_window_bars=3,
    confirm_close_above_midpoint=True,
    confirm_close_above_high=False,
)

DEFAULT_ENGULFING_CONFIG = EngulfingConfig(
    min_engulf_ratio=Decimal("1.2"),
    require_full_engulf=True,
)
```

### Chart Patterns

```python
DEFAULT_PIVOT_CONFIG = PivotConfig(
    fractal_window=3,
)

DEFAULT_HNS_CONFIG = HNSConfig(
    min_pivots_required=3,
    shoulder_tolerance_pct=Decimal("0.02"),
    head_prominence_min_pct=Decimal("0.05"),
    neckline_slope_tolerance=Decimal("0.01"),
)

DEFAULT_HNS_CONFIRM_CONFIG = HNSConfirmConfig(
    break_tolerance_pct=Decimal("0.005"),
)

DEFAULT_HNS_INVALIDATION_CONFIG = HNSInvalidationConfig(
    invalidation_level="RS_HIGH",  # Options: "RS_HIGH", "HEAD_HIGH"
)
```

---

## Example Scenarios

### Scenario 1: Hammer Detection → Confirmation

**OHLC Sequence** (BTC/USDT 15m):

| Timestamp | Open | High | Low | Close | Metrics |
|-----------|------|------|-----|-------|---------|
| 10:00 | 50500 | 50600 | 50400 | 50450 | Downtrend continues |
| 10:15 | 50450 | 50500 | 50350 | 50420 | Downtrend continues |
| 10:30 | 50420 | 50450 | 50300 | 50380 | Downtrend continues |
| **10:45** | **50380** | **50420** | **49900** | **50350** | **HAMMER** |
| 10:60 | 50350 | 50550 | 50320 | 50520 | Confirmation candle |

**Analysis**:

1. **10:45 Candle** (Hammer):
   - `body = |50350 - 50380| = 30`
   - `range = 50420 - 49900 = 520`
   - `lower_wick = 50380 - 49900 = 480`
   - `upper_wick = 50420 - 50380 = 40`
   - `lower_wick_pct = 480 / 520 = 0.923` ✅ (>= 0.60)
   - `body_pct = 30 / 520 = 0.058` ✅ (<= 0.30)
   - `upper_wick_pct = 40 / 520 = 0.077` ✅ (<= 0.15)
   - **FORMING** alert emitted

2. **Context Check**:
   - Prior 10 bars: return = (50380 - 50500) / 50500 = -0.24% (< -3% threshold)
   - Context filter PASSED ✅

3. **Confirmation Check** (10:60 candle):
   - Hammer midpoint = (50420 + 49900) / 2 = 50160
   - Close = 50520 > 50160 ✅
   - **CONFIRMED** alert emitted

**Database State**:

```sql
-- PatternInstance
id=1, pattern_code=HAMMER, symbol=BTCUSDT, timeframe=15m, start_ts=10:45, status=CONFIRMED

-- CandlestickPatternDetail
instance_id=1, body_pct_main=0.058, lower_wick_pct_main=0.923, upper_wick_pct_main=0.077

-- PatternAlert
[
  {instance_id=1, alert_type=FORMING, alert_ts=10:45, confidence=0.75},
  {instance_id=1, alert_type=CONFIRM, alert_ts=11:00, confidence=0.85}
]
```

---

### Scenario 2: Head and Shoulders Detection → Confirmation

**Pivot Sequence** (BTC/USDT 15m, simplified):

| Timestamp | Type | Price | Role |
|-----------|------|-------|------|
| 10:00 | HIGH | 51000 | LS (Left Shoulder) |
| 10:15 | LOW | 50500 | Neckline low |
| 10:30 | HIGH | 51500 | HEAD |
| 10:45 | LOW | 50450 | Neckline low |
| 11:00 | HIGH | 50950 | RS (Right Shoulder) |

**Analysis**:

1. **Pivot Detection** (fractal window k=3):
   - LS at 10:00: 51000
   - HEAD at 10:30: 51500
   - RS at 11:00: 50950

2. **HNS Detection**:
   - HEAD > LS ✅ (51500 > 51000)
   - HEAD > RS ✅ (51500 > 50950)
   - Shoulder symmetry: `|51000 - 50950| / avg(51000, 50950)` = 0.98% ✅ (< 2%)
   - Head prominence: `(51500 - 50975) / 50975` = 1.03% (may fail if < 5% threshold)

3. **Neckline Computation**:
   - Neckline lows: [50500, 50450]
   - Neckline level ≈ 50475 (linear fit)

4. **Confirmation** (next candle at 11:15):
   - If close < 50475 * 0.995 = 50225 ✅ → **CONFIRMED**

**Database State**:

```sql
-- PatternInstance
id=2, pattern_code=HNS, symbol=BTCUSDT, timeframe=15m, start_ts=10:00, status=CONFIRMED

-- ChartPatternDetail
instance_id=2, neckline_slope=0.0001, head_prominence_pct=1.03, shoulder_symmetry=0.98, width_bars=4

-- PatternPoint (4 records)
[
  {instance_id=2, label=LS, ts=10:00, price=51000, role=LEFT_SHOULDER},
  {instance_id=2, label=HEAD, ts=10:30, price=51500, role=HEAD},
  {instance_id=2, label=RS, ts=11:00, price=50950, role=RIGHT_SHOULDER},
  {instance_id=2, label=NECKLINE_0, ts=10:15, price=50500, role=NECKLINE},
  {instance_id=2, label=NECKLINE_1, ts=10:45, price=50450, role=NECKLINE},
]

-- PatternAlert
[
  {instance_id=2, alert_type=FORMING, alert_ts=11:00, confidence=0.70},
  {instance_id=2, alert_type=CONFIRM, alert_ts=11:15, confidence=0.80}
]
```

---

## Monitoring & Logging

### Log Levels

**INFO**: Normal operations

```python
logger.info(f"Scanned 100 candles for BTCUSDT 15m")
logger.info(f"HAMMER detected at {start_ts}, confidence={confidence:.2f}")
logger.info(f"Pattern {instance_id} transitioned to CONFIRMED")
```

**WARNING**: Anomalies, stale data

```python
logger.warning(f"Candle feed stale: last={last_ts}, age={age}s")
logger.warning(f"Degenerate candle (range=0) at {ts}, skipping")
logger.warning(f"HNS detection failed: not enough pivots (need 3, got {len(highs)})")
```

**ERROR**: Failures

```python
logger.error(f"Failed to fetch candles from Binance: {exc}")
logger.error(f"Database write failed for pattern {instance_id}: {exc}")
```

### Metrics to Track

**Per-Run Metrics**:

- `candles_scanned_count`: Number of candles processed
- `patterns_detected_count`: Patterns found (FORMING)
- `patterns_confirmed_count`: Patterns confirmed
- `patterns_invalidated_count`: Patterns invalidated
- `alerts_emitted_count`: Total alerts emitted
- `run_duration_ms`: Detector run time

**Cumulative Metrics**:

- `total_patterns_by_code`: Histogram (HAMMER: 50, ENGULFING_BULL: 30, etc.)
- `avg_confirmation_rate`: Confirmed / Detected ratio
- `avg_time_to_confirmation_bars`: Bars from FORMING → CONFIRMED

---

## Version History

| Version | Date | Changes |
|---------|------|---------|
| v1.0.0 | 2025-12-28 | Initial specification (PHASE 0) |

---

**Next Steps (PHASE 1)**:

1. Implement detectors in `api/application/pattern_engine/detectors/`
2. Write unit tests with golden OHLC sequences
3. Add integration tests for idempotency
4. Create management command `scan_patterns.py`

---

**Related Documents**:

- ADR-0018: Pattern Detection Engine
- CLAUDE.md: Hexagonal architecture patterns
- ADR-0007: Robson is Risk Assistant (not autotrader)
