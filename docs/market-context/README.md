# Market Research & Context Engine

**Robson's Second Core: Context Interpretation for Risk-Aware Trading**

---

## Overview

The **Market Research & Context Engine** is Robson Bot's second core capability (alongside **Position Sizing & Risk Management**). It transforms raw market data (derivatives + selected on-chain signals) into structured, explainable market context that helps users make informed trading decisions.

**Think of it as a weather forecast for crypto markets.**

Just as a weather forecast tells you "high chance of rain, bring an umbrella," this engine tells you:
- "Market in SQUEEZE_RISK, consider smaller positions"
- "High volatility detected, wider stops recommended"
- "Funding extremely negative, watch for short squeeze"

**CRITICAL**: This engine provides **CONTEXT**, not trading signals. Users still decide when to trade and which strategy to follow (see [ADR-0007](../adr/ADR-0007-robson-is-risk-assistant-not-autotrader.md)).

---

## Goals

### What This Engine Does

✅ **Normalizes** derivatives data (funding rate, open interest) and on-chain signals (TVL trends)
✅ **Computes** features (z-scores, moving averages, deltas, extremes)
✅ **Classifies** market regime (NORMAL, CHOP_RISK, SQUEEZE_RISK, HIGH_VOL)
✅ **Provides** risk guidance (CONSERVATIVE, NEUTRAL, AGGRESSIVE)
✅ **Detects** stop vulnerability (clustered stops = danger)
✅ **Explains** all outputs (sources, thresholds, logic version)

### What This Engine Does NOT Do

❌ **Does NOT generate buy/sell signals** (you decide when to trade)
❌ **Does NOT auto-execute trades** (you confirm all orders)
❌ **Does NOT replace your analysis** (you remain in control)

---

## Core Concepts

### 1. Metric Points (Normalized Inputs)

**Definition**: Atomic, normalized market data points with timestamp, symbol, metric name, value, and source.

**Examples**:
```json
{
  "timestamp": "2025-12-28T10:00:00Z",
  "symbol": "BTCUSDT",
  "metric_name": "funding_rate",
  "value": 0.0005,
  "source": "binance_futures",
  "tags": {"contract": "perpetual", "interval": "8h"}
}
```

**Sources**:
- `binance_futures`: Funding rate, open interest, mark price
- `defillama`: Total Value Locked (TVL) trends (future)
- `glassnode`: On-chain metrics (future)

**Storage**: Persisted in `MetricPoint` table with **idempotency** on `(source, symbol, metric_name, timestamp)`.

---

### 2. Feature Vectors (Computed Indicators)

**Definition**: Derived metrics computed from raw `MetricPoint` data using statistical transforms (z-scores, moving averages, deltas).

**Examples**:
- `funding_zscore_24h`: How extreme is current funding vs last 24 hours?
- `oi_delta_4h`: % change in open interest over 4 hours
- `realized_vol_1h`: 1-hour price volatility (mark price)
- `squeeze_risk_score`: Composite heuristic (high funding + rising OI + flat price)

**Traceability**: Each `FeatureVector` includes:
- `source_metrics`: List of raw metrics used
- `computation_version`: Logic version (e.g., "v1.0.0")

---

### 3. Market Context Snapshots (Final Outputs)

**Definition**: Periodic classification of market state with risk guidance and explainable reasoning.

**Structure**:
```json
{
  "snapshot_id": "550e8400-e29b-41d4-a716-446655440000",
  "timestamp": "2025-12-28T10:05:00Z",
  "symbol": "BTCUSDT",
  "market_regime": "SQUEEZE_RISK",
  "risk_bias": "CONSERVATIVE",
  "stop_vulnerability": "HIGH",
  "recommended_posture": "REDUCE",
  "explanation_payload": {
    "funding_zscore_24h": 2.8,
    "oi_delta_4h": 0.15,
    "realized_vol_1h": 0.25,
    "logic_version": "v1.0.0",
    "reasoning": "High funding + rising OI + low vol = squeeze risk"
  },
  "sources_used": ["binance_futures"]
}
```

---

## Market Regimes (Enums)

### `MarketRegime`

| Regime | Description | When It Occurs |
|--------|-------------|----------------|
| **NORMAL** | Healthy market conditions | Default state, no extremes detected |
| **CHOP_RISK** | Choppy, range-bound market | High vol + flat OI = whipsaws likely |
| **SQUEEZE_RISK** | Potential liquidation cascade | High funding + rising OI + low vol = squeeze setup |
| **HIGH_VOL** | Extreme volatility | Realized vol spike (>80% annualized) |

---

### `RiskBias`

| Bias | Description | Recommended Action |
|------|-------------|-------------------|
| **CONSERVATIVE** | Reduce exposure, be cautious | Smaller positions, wider stops |
| **NEUTRAL** | Normal risk tolerance | Standard position sizing |
| **AGGRESSIVE** | Favorable conditions detected | Normal or slightly larger positions (still within 1% risk) |

---

### `StopVulnerability`

| Level | Description | Implication |
|-------|-------------|-------------|
| **LOW** | Stops well-distributed | Standard stop placement OK |
| **MEDIUM** | Some clustering detected | Consider adjusting stop levels |
| **HIGH** | Heavy stop clustering | Wider stops or wait for better entry |

---

### `TradingPosture`

| Posture | Description | User Action |
|---------|-------------|-------------|
| **REDUCE** | Consider closing or reducing positions | Review open positions, tighten stops |
| **HOLD** | Maintain current exposure | No immediate action needed |
| **NORMAL** | Standard trading conditions | Follow normal strategy rules |
| **AGGRESSIVE** | Favorable setup detected | Consider normal or slightly larger size (within risk limits) |

---

## Signals (Phase 1: BTC/USDT)

### Derivatives Signals (Binance Futures)

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

### On-Chain Signals (Future - Phase 2)

**DeFiLlama**:
- `tvl_delta_7d`: % change in Total Value Locked over 7 days
- `tvl_trend`: Trend direction (UP, DOWN, FLAT)

**Glassnode (Future)**:
- `exchange_netflow_7d`: Net BTC flow to/from exchanges
- `realized_cap_delta_30d`: Change in realized capitalization

**Dune Analytics (Future)**:
- `dex_volume_7d`: DEX trading volume trend
- `stablecoin_supply_delta_7d`: Net change in stablecoin supply

---

## How It Works

### Data Flow

```
1. COLLECT
   ├─ Binance Futures API → MetricPoint (funding_rate, open_interest, mark_price)
   ├─ DeFiLlama API → MetricPoint (tvl_total, tvl_btc) [future]
   └─ Persist to database (idempotency on timestamp + metric + source)

2. COMPUTE
   ├─ Load recent MetricPoint records (e.g., last 24h)
   ├─ Calculate FeatureVector (z-scores, MAs, deltas)
   └─ Persist FeatureVector with traceability

3. CLASSIFY
   ├─ Load recent FeatureVector records
   ├─ Apply regime classification logic (heuristic or ML)
   ├─ Generate MarketContextSnapshot with explanation
   └─ Persist snapshot

4. CONSUME
   ├─ EntryGate queries latest snapshot before order creation
   ├─ PositionManager monitors regime changes for open positions
   └─ User reviews context in dashboard
```

---

### Regime Classification Logic (Initial Heuristics)

**SQUEEZE_RISK** (Liquidation cascade setup):
```python
if (funding_zscore_24h > 2.0 and
    oi_delta_4h > 0.10 and
    realized_vol_1h < 0.30):
    return SQUEEZE_RISK
```
**Reasoning**: High funding + rising OI + low vol = overleveraged positions building up

---

**HIGH_VOL** (Volatility spike):
```python
if realized_vol_1h > 0.80:
    return HIGH_VOL
```
**Reasoning**: Extreme volatility = higher risk of stop-outs and slippage

---

**CHOP_RISK** (Range-bound whipsaw):
```python
if (realized_vol_4h > 0.50 and
    abs(oi_delta_1h) < 0.02):
    return CHOP_RISK
```
**Reasoning**: High vol + flat OI = noise, not trend

---

**NORMAL** (Default):
```python
return NORMAL
```
**Reasoning**: No extremes detected, standard market conditions

---

### Freshness Monitoring

**Problem**: If derivatives data is stale (e.g., API down, network issue), context becomes unreliable.

**Solution**: Before generating snapshot, check `MetricPoint.timestamp`:
- If `now - latest_timestamp > 5 minutes`:
  - Emit warning log
  - Set `risk_bias = CONSERVATIVE`
  - Set `recommended_posture = HOLD`
  - Add to `explanation_payload`: `{"freshness_warning": "Data stale (6 minutes old)"}`

---

## How to Run

### 1. Collect Derivatives Metrics

**Single run** (fetch latest data):
```bash
python manage.py collect_derivatives_metrics --symbol BTCUSDT
```

**Continuous monitoring** (every 60 seconds):
```bash
python manage.py collect_derivatives_metrics --symbol BTCUSDT --continuous --interval 60
```

**Deploy as CronJob** (Kubernetes):
```yaml
apiVersion: batch/v1
kind: CronJob
metadata:
  name: collect-derivatives-metrics
  namespace: robson
spec:
  schedule: "*/1 * * * *"  # Every minute
  jobTemplate:
    spec:
      template:
        spec:
          containers:
          - name: collector
            image: robson-backend:latest
            command:
            - python
            - manage.py
            - collect_derivatives_metrics
            - --symbol
            - BTCUSDT
```

---

### 2. Generate Market Context Snapshots

**Single run** (compute latest snapshot):
```bash
python manage.py generate_market_context --symbol BTCUSDT
```

**Continuous monitoring** (every 5 minutes):
```bash
python manage.py generate_market_context --symbol BTCUSDT --continuous --interval 300
```

**Deploy as CronJob** (Kubernetes):
```yaml
apiVersion: batch/v1
kind: CronJob
metadata:
  name: generate-market-context
  namespace: robson
spec:
  schedule: "*/5 * * * *"  # Every 5 minutes
  jobTemplate:
    spec:
      template:
        spec:
          containers:
          - name: context-engine
            image: robson-backend:latest
            command:
            - python
            - manage.py
            - generate_market_context
            - --symbol
            - BTCUSDT
```

---

### 3. Monitor Context Freshness

**Single check** (exit with error if stale):
```bash
python manage.py monitor_context_freshness --threshold 300
```

**Continuous monitoring** (every 60 seconds):
```bash
python manage.py monitor_context_freshness --threshold 300 --continuous --interval 60
```

**Deploy as sidecar** (Kubernetes):
```yaml
containers:
- name: freshness-monitor
  image: robson-backend:latest
  command:
  - python
  - manage.py
  - monitor_context_freshness
  - --threshold
  - "300"
  - --continuous
  - --interval
  - "60"
```

---

## Integration with EntryGate

**Scenario**: User wants to open a new BTC long position.

**Flow**:
1. User provides intent (symbol, side, entry, stop, strategy)
2. **EntryGate** queries latest `MarketContextSnapshot` for BTCUSDT
3. If snapshot shows:
   ```json
   {
     "market_regime": "SQUEEZE_RISK",
     "risk_bias": "CONSERVATIVE",
     "stop_vulnerability": "HIGH",
     "recommended_posture": "REDUCE"
   }
   ```
4. **EntryGate displays warnings**:
   - ⚠️ "Market in SQUEEZE_RISK: Liquidation cascade possible"
   - ⚠️ "High stop clustering detected: Consider wider stop or wait"
   - ⚠️ "Recommended posture: REDUCE or HOLD"
5. User reviews warnings and context explanation
6. User decides:
   - Option A: Proceed with smaller position size
   - Option B: Adjust stop price to avoid clustering
   - Option C: Wait for regime to return to NORMAL
7. Robson calculates position size, user confirms, order placed

**Key**: User always makes the final decision. Robson provides context, not commands.

---

## Integration with PositionManager

**Scenario**: Monitor open positions for regime changes.

**Flow**:
1. PositionManager runs every 5 minutes (CronJob)
2. For each open position, fetch latest `MarketContextSnapshot`
3. If regime changed from `NORMAL` → `SQUEEZE_RISK`:
   - Emit notification: "Market regime changed to SQUEEZE_RISK, consider tightening stop"
   - Flag position in dashboard (yellow warning indicator)
4. If `risk_bias == CONSERVATIVE`:
   - Update position card: "Market context: CONSERVATIVE (high caution)"
5. User reviews flagged positions and decides next action

---

## API Endpoints (Future)

**Get Latest Context Snapshot**:
```http
GET /api/market-context/snapshot/?symbol=BTCUSDT

Response:
{
  "snapshot_id": "550e8400-e29b-41d4-a716-446655440000",
  "timestamp": "2025-12-28T10:05:00Z",
  "symbol": "BTCUSDT",
  "market_regime": "NORMAL",
  "risk_bias": "NEUTRAL",
  "stop_vulnerability": "LOW",
  "recommended_posture": "NORMAL",
  "explanation_payload": {
    "funding_zscore_24h": 0.5,
    "oi_delta_4h": 0.02,
    "realized_vol_1h": 0.40,
    "logic_version": "v1.0.0"
  },
  "sources_used": ["binance_futures"]
}
```

---

**Get Historical Context**:
```http
GET /api/market-context/history/?symbol=BTCUSDT&start=2025-12-27T00:00:00Z&end=2025-12-28T00:00:00Z

Response:
[
  { "timestamp": "2025-12-27T00:05:00Z", "market_regime": "NORMAL", ... },
  { "timestamp": "2025-12-27T00:10:00Z", "market_regime": "CHOP_RISK", ... },
  ...
]
```

---

**Get Raw Metrics**:
```http
GET /api/market-context/metrics/?symbol=BTCUSDT&metric_name=funding_rate&start=2025-12-27T00:00:00Z

Response:
[
  { "timestamp": "2025-12-27T00:00:00Z", "value": 0.0003, "source": "binance_futures" },
  { "timestamp": "2025-12-27T08:00:00Z", "value": 0.0005, "source": "binance_futures" },
  ...
]
```

---

## Testing

### Unit Tests

**Location**: `apps/backend/monolith/api/tests/test_market_context.py`

**Coverage**:
- Z-score calculation with sample data
- Moving average computation
- Delta calculation (1h, 4h, 24h)
- Regime classification with known inputs
- Idempotency of `MetricPoint` upsert

**Example**:
```python
def test_funding_zscore_calculation():
    """Test z-score calculation for funding rate."""
    metrics = [
        MetricPoint(timestamp=t, value=0.0001) for t in timestamps[:-1]
    ]
    metrics.append(MetricPoint(timestamp=now, value=0.0010))  # Extreme value

    builder = FundingFeatureBuilder()
    features = builder.compute_features(metrics)

    assert features["funding_zscore_24h"] > 2.0  # Should detect extreme
```

---

### Integration Tests

**Location**: `apps/backend/monolith/api/tests/test_market_context_integration.py`

**Coverage**:
- Full flow: collect metrics → compute features → classify regime
- Freshness monitor with stale data
- EntryGate warning logic with different regimes
- PositionManager regime change detection

**Example**:
```python
@pytest.mark.django_db
def test_full_context_pipeline():
    """Test end-to-end market context generation."""
    # 1. Collect metrics
    collector = BinanceFuturesMetricCollector()
    metrics = collector.collect_metrics("BTCUSDT")
    assert len(metrics) > 0

    # 2. Compute features
    builder = FeatureBuilder()
    features = builder.compute_features(metrics)
    assert "funding_zscore_24h" in features

    # 3. Classify regime
    classifier = HeuristicRegimeClassifier()
    snapshot = classifier.classify(features)
    assert snapshot.market_regime in [MarketRegime.NORMAL, MarketRegime.SQUEEZE_RISK, ...]
```

---

## Troubleshooting

### Issue: "No MetricPoint data found"

**Cause**: Metrics collector not running or failed.

**Solution**:
1. Check collector logs: `kubectl logs -n robson -l app=collect-derivatives-metrics`
2. Verify Binance API keys are configured
3. Run manual collection: `python manage.py collect_derivatives_metrics --symbol BTCUSDT`

---

### Issue: "Context snapshot is stale"

**Cause**: `generate_market_context` command not running.

**Solution**:
1. Check CronJob status: `kubectl get cronjobs -n robson`
2. Check job logs: `kubectl logs -n robson -l app=generate-market-context`
3. Run manual generation: `python manage.py generate_market_context --symbol BTCUSDT`

---

### Issue: "Freshness warning in explanation_payload"

**Cause**: MetricPoint data is older than 5 minutes (stale feed).

**Solution**:
1. Check if Binance Futures API is accessible
2. Check rate limits (might be hitting API limits)
3. Review `collect_derivatives_metrics` error logs
4. Temporarily increase freshness threshold if API is slow

---

## Future Enhancements

### Phase 2: On-Chain Signals

- Add DeFiLlama TVL trends
- Add Glassnode exchange netflows
- Add Dune Analytics DEX volume

---

### Phase 3: Machine Learning

- Replace heuristic classifier with ML model
- Train on labeled historical data
- Backtested regime predictions
- Confidence scores for each regime

---

### Phase 4: Multi-Symbol Support

- Extend to ETH/USDT, SOL/USDT, etc.
- Cross-symbol correlation analysis
- Portfolio-level regime classification

---

### Phase 5: User-Configurable Thresholds

- Allow users to customize z-score levels
- Adjustable volatility thresholds
- Personalized risk bias settings

---

## Related Documentation

- **[ADR-0017](../adr/ADR-0017-market-research-context-engine.md)**: Full architectural decision record
- **[ADR-0007](../adr/ADR-0007-robson-is-risk-assistant-not-autotrader.md)**: Robson's core mission (risk assistant)
- **[ADR-0002](../adr/ADR-0002-hexagonal-architecture.md)**: Hexagonal architecture pattern
- **[ARCHITECTURE.md](../ARCHITECTURE.md)**: Overall system architecture

---

## FAQ

### Q: Does this engine automatically trade for me?

**A**: No. This engine provides **context** (market regime, risk guidance). You still decide when to trade, which strategy to use, and confirm all orders.

---

### Q: How accurate are the regime classifications?

**A**: Initial implementation uses heuristic rules (not ML). Accuracy depends on threshold calibration. We recommend backtesting and validating with your own historical data.

---

### Q: Can I customize the thresholds?

**A**: Not yet (Phase 1). Currently hardcoded. Phase 5 will add user-configurable thresholds.

---

### Q: What happens if derivatives data is stale?

**A**: System automatically degrades to `CONSERVATIVE` bias and sets `recommended_posture = HOLD`. Freshness warning included in explanation payload.

---

### Q: Can I add my own signals?

**A**: Yes! Follow the `MetricPoint` schema and add your own collector adapter. Contributions welcome!

---

**Last Updated**: 2025-12-28
**Version**: 1.0.0 (Phase 1 - Derivatives only)
**Maintainers**: Robson Bot Core Team
