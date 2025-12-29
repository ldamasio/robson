# Market Research & Context Engine - Implementation Plan

**Status**: Phase 0 Revised (Discovery-Driven)
**Ready for**: Phase 0.5 Discovery (Code Scanning)
**Version**: 1.1.0
**Date**: 2025-12-28

---

## Items to Validate (Before Milestone 1)

**Note**: These are validation checkpoints, not hard blockers. Discovery-driven approach allows iteration.

### Derivatives Data Infrastructure (Discovery Task)
- [ ] Locate existing exchange integrations that might expose derivatives data
- [ ] Identify if funding rate, open interest, mark price are already accessible
- [ ] Determine if we can wrap existing collectors or need minimal new adapter
- [ ] Confirm Binance API keys are configured (if needed for new adapter)

### Threshold Calibration (Initial Heuristics)
- [ ] Set initial z-score levels, funding extremes, OI deltas as **heuristics**
- [ ] Backtest recommended but not blocking (iterate after initial deployment)
- [ ] Thresholds are NOT correctness guarantees; expect tuning iterations

### Database & Deployment Patterns (Validation)
- [ ] Confirm Django migrations can run on target database
- [ ] Identify standard deployment patterns for scheduled tasks in this repo:
  - Kubernetes CronJobs (if already standard), OR
  - Alternative schedulers (cron, systemd timers, etc.)
- [ ] Local validation is sufficient for Phase 1 (production deployment optional)

---

## File Structure

### New Files to Create

```
apps/backend/monolith/api/
├── models/
│   └── market_context.py                    # NEW: Django models (MetricPoint, FeatureVector, MarketContextSnapshot)
├── application/
│   └── market_context/                      # NEW: Hexagonal core for market context
│       ├── __init__.py
│       ├── domain.py                        # NEW: Domain entities (MetricPoint, FeatureVector, MarketContextSnapshot)
│       ├── ports.py                         # NEW: Interfaces (MetricRepository, FeatureBuilder, RegimeClassifier)
│       ├── use_cases.py                     # NEW: Use cases (CollectDerivativesMetrics, GenerateMarketContextSnapshot)
│       ├── adapters.py                      # NEW: Adapters (DjangoMetricRepository, DerivativesMetricCollector)
│       ├── feature_builders.py              # NEW: Feature computation logic (z-scores, MAs, deltas)
│       └── classifiers.py                   # NEW: Regime classification logic (HeuristicRegimeClassifier)
├── management/commands/
│   ├── collect_derivatives_metrics.py       # NEW: Django command to collect metrics
│   ├── generate_market_context.py           # NEW: Django command to generate snapshots
│   └── monitor_context_freshness.py         # NEW: Django command to check freshness
└── tests/
    ├── test_market_context.py               # NEW: Unit tests
    └── test_market_context_integration.py   # NEW: Integration tests

docs/
├── adr/
│   └── ADR-0017-market-research-context-engine.md  # CREATED
└── market-context/
    ├── README.md                            # CREATED
    └── IMPLEMENTATION-PLAN.md               # CREATED (this file)

infra/k8s/overlays/production/  # OPTIONAL (only if Kubernetes is standard deployment)
└── cronjobs/
    ├── collect-derivatives-metrics.yaml     # OPTIONAL: CronJob for metrics collection
    ├── generate-market-context.yaml         # OPTIONAL: CronJob for context generation
    └── monitor-context-freshness.yaml       # OPTIONAL: CronJob for freshness monitoring
```

---

## Implementation Milestones

### PHASE 0.5: Code Discovery & Validation (Before Milestone 1)

**Goal**: Locate existing derivatives infrastructure and determine integration approach.

**Tasks**:

1. **Scan for Exchange Integrations**
   - Search for `BinanceService`, `binance_service.py`, or similar adapters
   - Check for existing methods: `get_funding_rate()`, `get_open_interest()`, `get_mark_price()`
   - Document file paths and current usage patterns

2. **Identify Persistence Patterns**
   - Check if Django models already exist for market data (funding, OI, etc.)
   - Look for historical data storage (TimescaleDB, raw logs, etc.)
   - Determine if data is ephemeral (API calls only) or persisted

3. **Evaluate Integration Options**
   - **Option A**: Wrap existing collector (if metrics already accessible)
   - **Option B**: Extend existing adapter with new methods
   - **Option C**: Create minimal new adapter (if no existing infrastructure)

4. **Verify Deployment Patterns**
   - Check for existing CronJobs in `infra/k8s/` (precedent for scheduled tasks)
   - Identify alternative schedulers if Kubernetes not standard
   - Document current patterns for scheduled/background jobs

5. **Document Findings**
   - Create `docs/market-context/DISCOVERY-FINDINGS.md` with:
     - File paths for existing exchange code
     - Availability of derivatives metrics (funding, OI, mark price)
     - Recommended integration approach (Option A/B/C)
     - Deployment pattern recommendation

**Success Criteria**:
- [ ] Existing exchange infrastructure located and documented
- [ ] Derivatives metrics availability confirmed or gaps identified
- [ ] Integration approach selected (wrap vs extend vs new)
- [ ] Deployment pattern validated (Kubernetes vs alternative)
- [ ] Findings documented in DISCOVERY-FINDINGS.md

---

### MILESTONE 1: Derivatives Storage (Week 1)

**Goal**: Persist normalized derivatives data with idempotency, using approach determined in Phase 0.5.

**Tasks**:

1. **Create Django Model** (`api/models/market_context.py`)
   ```python
   class MetricPoint(models.Model):
       timestamp = models.DateTimeField(db_index=True)
       symbol = models.CharField(max_length=20, db_index=True)
       metric_name = models.CharField(max_length=50, db_index=True)
       value = models.DecimalField(max_digits=20, decimal_places=10)
       source = models.CharField(max_length=50)
       tags = models.JSONField(default=dict)

       class Meta:
           unique_together = ('source', 'symbol', 'metric_name', 'timestamp')
           indexes = [
               models.Index(fields=['symbol', 'metric_name', 'timestamp']),
           ]
   ```

2. **Create Domain Entity** (`api/application/market_context/domain.py`)
   ```python
   @dataclass(frozen=True)
   class MetricPoint:
       timestamp: datetime
       symbol: str
       metric_name: str
       value: Decimal
       source: str
       tags: dict
   ```

3. **Create Port** (`api/application/market_context/ports.py`)
   ```python
   class MetricRepository(Protocol):
       def save_metric(self, metric: MetricPoint) -> None: ...
       def get_metrics(self, symbol: str, metric_name: str, start: datetime, end: datetime) -> list[MetricPoint]: ...
   ```

4. **Create Django Adapter** (`api/application/market_context/adapters.py`)
   ```python
   class DjangoMetricRepository:
       def save_metric(self, metric: MetricPoint) -> None:
           # Upsert with get_or_create on unique_together constraint
           pass
   ```

5. **Create Collector Adapter** (`api/application/market_context/adapters.py`)
   ```python
   # Implementation depends on Phase 0.5 findings:
   # - If existing collector found: Wrap it
   # - If methods exist but not aggregated: Extend existing adapter
   # - If no infrastructure: Create minimal new collector

   class DerivativesMetricCollector:  # or wrap existing BinanceService
       def collect_metrics(self, symbol: str) -> list[MetricPoint]:
           # Fetch funding_rate, open_interest, mark_price
           # Normalize to MetricPoint domain entities
           # Use existing infrastructure if available
           pass
   ```

6. **Create Use Case** (`api/application/market_context/use_cases.py`)
   ```python
   class CollectDerivativesMetrics:
       def __init__(self, collector: DerivativesMetricCollector, repo: MetricRepository):
           self._collector = collector
           self._repo = repo

       def execute(self, symbol: str) -> int:
           metrics = self._collector.collect_metrics(symbol)
           for metric in metrics:
               self._repo.save_metric(metric)
           return len(metrics)
   ```

7. **Create Django Command** (`api/management/commands/collect_derivatives_metrics.py`)
   ```python
   class Command(BaseCommand):
       def handle(self, *args, **options):
           symbol = options['symbol']
           continuous = options['continuous']
           interval = options['interval']

           # Use collector determined in Phase 0.5 (wrap existing or new)
           use_case = CollectDerivativesMetrics(
               collector=DerivativesMetricCollector(),
               repo=DjangoMetricRepository(),
           )

           if continuous:
               while True:
                   count = use_case.execute(symbol)
                   self.stdout.write(f"✓ Collected {count} metrics")
                   time.sleep(interval)
           else:
               count = use_case.execute(symbol)
               self.stdout.write(f"✓ Collected {count} metrics")
   ```

8. **Create Migration**
   ```bash
   python manage.py makemigrations api --name add_metric_point_model
   python manage.py migrate
   ```

9. **Write Unit Tests** (`api/tests/test_market_context.py`)
   - Test `MetricPoint` domain entity creation
   - Test `DjangoMetricRepository.save_metric()` with duplicate timestamps (idempotency)
   - Test `DerivativesMetricCollector.collect_metrics()` with mocked exchange API (or wrapped existing collector)

10. **Manual Validation**
    ```bash
    # Single run
    python manage.py collect_derivatives_metrics --symbol BTCUSDT

    # Check database
    python manage.py shell
    >>> from api.models.market_context import MetricPoint
    >>> MetricPoint.objects.filter(symbol='BTCUSDT').count()
    3  # Should have funding_rate, open_interest, mark_price

    # Test idempotency
    python manage.py collect_derivatives_metrics --symbol BTCUSDT
    >>> MetricPoint.objects.filter(symbol='BTCUSDT').count()
    3  # Should still be 3 (no duplicates)
    ```

**Success Criteria**:
- [ ] MetricPoint model created with unique constraint
- [ ] Migration applied successfully
- [ ] Metrics collected from Binance Futures API
- [ ] Idempotency enforced (duplicate timestamps ignored)
- [ ] Unit tests pass (100% coverage for domain + repository)

---

### MILESTONE 2: Feature Engineering (Week 2)

**Goal**: Compute statistical features (z-scores, moving averages, deltas) from raw metrics.

**Tasks**:

1. **Create Django Model** (`api/models/market_context.py`)
   ```python
   class FeatureVector(models.Model):
       timestamp = models.DateTimeField(db_index=True)
       symbol = models.CharField(max_length=20, db_index=True)
       feature_name = models.CharField(max_length=50, db_index=True)
       value = models.DecimalField(max_digits=20, decimal_places=10)
       source_metrics = models.JSONField(default=list)
       computation_version = models.CharField(max_length=20, default="v1.0.0")

       class Meta:
           unique_together = ('symbol', 'feature_name', 'timestamp', 'computation_version')
   ```

2. **Create Domain Entity** (`api/application/market_context/domain.py`)
   ```python
   @dataclass(frozen=True)
   class FeatureVector:
       timestamp: datetime
       symbol: str
       feature_name: str
       value: Decimal
       source_metrics: list[str]
       computation_version: str
   ```

3. **Create Feature Builders** (`api/application/market_context/feature_builders.py`)
   ```python
   class FundingFeatureBuilder:
       def compute_zscore_24h(self, metrics: list[MetricPoint]) -> FeatureVector:
           # Calculate z-score of latest funding vs last 24h
           pass

       def compute_ma_8h(self, metrics: list[MetricPoint]) -> FeatureVector:
           # Calculate 8-hour moving average of funding
           pass

   class OpenInterestFeatureBuilder:
       def compute_delta_1h(self, metrics: list[MetricPoint]) -> FeatureVector:
           # Calculate % change in OI over 1 hour
           pass

       def compute_zscore_7d(self, metrics: list[MetricPoint]) -> FeatureVector:
           # Calculate z-score of current OI vs last 7 days
           pass

   class VolatilityFeatureBuilder:
       def compute_realized_vol_1h(self, metrics: list[MetricPoint]) -> FeatureVector:
           # Calculate 1-hour realized volatility from mark_price
           pass
   ```

4. **Create Port** (`api/application/market_context/ports.py`)
   ```python
   class FeatureBuilder(Protocol):
       def compute_features(self, metrics: list[MetricPoint]) -> list[FeatureVector]: ...
   ```

5. **Create Composite Builder** (`api/application/market_context/feature_builders.py`)
   ```python
   class CompositeFeatureBuilder:
       def __init__(self):
           self.funding_builder = FundingFeatureBuilder()
           self.oi_builder = OpenInterestFeatureBuilder()
           self.vol_builder = VolatilityFeatureBuilder()

       def compute_features(self, metrics: list[MetricPoint]) -> list[FeatureVector]:
           features = []
           features.extend(self.funding_builder.compute_all(metrics))
           features.extend(self.oi_builder.compute_all(metrics))
           features.extend(self.vol_builder.compute_all(metrics))
           return features
   ```

6. **Create Migration**
   ```bash
   python manage.py makemigrations api --name add_feature_vector_model
   python manage.py migrate
   ```

7. **Write Unit Tests** (`api/tests/test_market_context.py`)
   - Test z-score calculation with known data (mean=0, std=1)
   - Test moving average calculation (simple MA)
   - Test delta calculation (% change)
   - Test edge cases (insufficient data, zero values)

8. **Manual Validation**
   ```bash
   # Compute features for collected metrics
   python manage.py shell
   >>> from api.application.market_context.use_cases import ComputeFeatures
   >>> from api.models.market_context import MetricPoint, FeatureVector
   >>> metrics = MetricPoint.objects.filter(symbol='BTCUSDT', metric_name='funding_rate').order_by('timestamp')[:100]
   >>> use_case = ComputeFeatures()
   >>> features = use_case.execute('BTCUSDT')
   >>> FeatureVector.objects.filter(symbol='BTCUSDT').count()
   10  # Should have funding_zscore_24h, funding_ma_8h, oi_delta_1h, etc.
   ```

**Success Criteria**:
- [ ] FeatureVector model created
- [ ] Z-score, MA, delta builders implemented
- [ ] Unit tests pass (100% coverage for feature builders)
- [ ] Features computed from real Binance data
- [ ] Traceability enforced (source_metrics field populated)

---

### MILESTONE 3: Regime Classifier (Week 3)

**Goal**: Classify market regime based on computed features using heuristic rules.

**Tasks**:

1. **Create Django Model** (`api/models/market_context.py`)
   ```python
   class MarketContextSnapshot(models.Model):
       snapshot_id = models.UUIDField(primary_key=True, default=uuid.uuid4)
       timestamp = models.DateTimeField(db_index=True)
       symbol = models.CharField(max_length=20, db_index=True)
       market_regime = models.CharField(max_length=20)  # NORMAL, CHOP_RISK, SQUEEZE_RISK, HIGH_VOL
       risk_bias = models.CharField(max_length=20)  # CONSERVATIVE, NEUTRAL, AGGRESSIVE
       stop_vulnerability = models.CharField(max_length=20)  # LOW, MEDIUM, HIGH
       recommended_posture = models.CharField(max_length=20)  # REDUCE, HOLD, NORMAL, AGGRESSIVE
       explanation_payload = models.JSONField()
       sources_used = models.JSONField(default=list)

       class Meta:
           indexes = [
               models.Index(fields=['symbol', '-timestamp']),
           ]
   ```

2. **Create Domain Entity** (`api/application/market_context/domain.py`)
   ```python
   class MarketRegime(Enum):
       NORMAL = "NORMAL"
       CHOP_RISK = "CHOP_RISK"
       SQUEEZE_RISK = "SQUEEZE_RISK"
       HIGH_VOL = "HIGH_VOL"

   @dataclass
   class MarketContextSnapshot:
       snapshot_id: str
       timestamp: datetime
       symbol: str
       market_regime: MarketRegime
       risk_bias: RiskBias
       stop_vulnerability: StopVulnerability
       recommended_posture: TradingPosture
       explanation_payload: dict
       sources_used: list[str]
   ```

3. **Create Classifier** (`api/application/market_context/classifiers.py`)
   ```python
   class HeuristicRegimeClassifier:
       THRESHOLDS = {
           'funding_zscore_extreme': 2.0,
           'oi_delta_significant': 0.10,
           'realized_vol_low': 0.30,
           'realized_vol_extreme': 0.80,
           'realized_vol_high': 0.50,
       }

       def classify(self, features: dict[str, Decimal]) -> MarketContextSnapshot:
           regime = self._classify_regime(features)
           risk_bias = self._determine_risk_bias(regime, features)
           stop_vuln = self._assess_stop_vulnerability(features)
           posture = self._recommend_posture(regime, risk_bias)

           explanation = {
               'logic_version': 'v1.0.0',
               'thresholds_used': self.THRESHOLDS,
               'feature_values': {k: str(v) for k, v in features.items()},
               'reasoning': self._explain_reasoning(regime, features),
           }

           return MarketContextSnapshot(
               snapshot_id=str(uuid.uuid4()),
               timestamp=datetime.now(timezone.utc),
               symbol=features['symbol'],
               market_regime=regime,
               risk_bias=risk_bias,
               stop_vulnerability=stop_vuln,
               recommended_posture=posture,
               explanation_payload=explanation,
               sources_used=['binance_futures'],
           )

       def _classify_regime(self, features: dict) -> MarketRegime:
           # SQUEEZE_RISK: High funding + rising OI + low vol
           if (features.get('funding_zscore_24h', 0) > self.THRESHOLDS['funding_zscore_extreme'] and
               features.get('oi_delta_4h', 0) > self.THRESHOLDS['oi_delta_significant'] and
               features.get('realized_vol_1h', 0) < self.THRESHOLDS['realized_vol_low']):
               return MarketRegime.SQUEEZE_RISK

           # HIGH_VOL: Realized vol spike
           if features.get('realized_vol_1h', 0) > self.THRESHOLDS['realized_vol_extreme']:
               return MarketRegime.HIGH_VOL

           # CHOP_RISK: High vol + flat OI
           if (features.get('realized_vol_4h', 0) > self.THRESHOLDS['realized_vol_high'] and
               abs(features.get('oi_delta_1h', 0)) < 0.02):
               return MarketRegime.CHOP_RISK

           # NORMAL: Default
           return MarketRegime.NORMAL
   ```

4. **Create Use Case** (`api/application/market_context/use_cases.py`)
   ```python
   class GenerateMarketContextSnapshot:
       def __init__(self, feature_repo, classifier, snapshot_repo):
           self._feature_repo = feature_repo
           self._classifier = classifier
           self._snapshot_repo = snapshot_repo

       def execute(self, symbol: str) -> MarketContextSnapshot:
           # Get latest features
           features = self._feature_repo.get_latest_features(symbol)

           # Classify regime
           snapshot = self._classifier.classify(features)

           # Persist snapshot
           self._snapshot_repo.save_snapshot(snapshot)

           return snapshot
   ```

5. **Create Django Command** (`api/management/commands/generate_market_context.py`)
   ```python
   class Command(BaseCommand):
       def handle(self, *args, **options):
           symbol = options['symbol']
           continuous = options['continuous']
           interval = options['interval']

           use_case = GenerateMarketContextSnapshot(...)

           if continuous:
               while True:
                   snapshot = use_case.execute(symbol)
                   self.stdout.write(f"✓ Generated snapshot: {snapshot.market_regime}")
                   time.sleep(interval)
           else:
               snapshot = use_case.execute(symbol)
               self.stdout.write(f"✓ Generated snapshot: {snapshot.market_regime}")
   ```

6. **Create Migration**
   ```bash
   python manage.py makemigrations api --name add_market_context_snapshot_model
   python manage.py migrate
   ```

7. **Write Unit Tests** (`api/tests/test_market_context.py`)
   - Test SQUEEZE_RISK classification (high funding + rising OI + low vol)
   - Test HIGH_VOL classification (vol spike)
   - Test CHOP_RISK classification (high vol + flat OI)
   - Test NORMAL classification (no extremes)

8. **Manual Validation**
   ```bash
   # Generate snapshot
   python manage.py generate_market_context --symbol BTCUSDT

   # Check database
   python manage.py shell
   >>> from api.models.market_context import MarketContextSnapshot
   >>> snapshot = MarketContextSnapshot.objects.filter(symbol='BTCUSDT').latest('timestamp')
   >>> snapshot.market_regime
   'NORMAL'
   >>> snapshot.explanation_payload
   {'logic_version': 'v1.0.0', 'feature_values': {...}, 'reasoning': '...'}
   ```

**Success Criteria**:
- [ ] MarketContextSnapshot model created
- [ ] HeuristicRegimeClassifier implemented with all 4 regimes
- [ ] Unit tests pass (all regime paths tested)
- [ ] Snapshots generated with explanation payloads
- [ ] Integration test passes (collect → features → classify)

---

### MILESTONE 4: Freshness Monitor (Week 4)

**Goal**: Detect stale derivatives data and degrade gracefully to CONSERVATIVE mode.

**Tasks**:

1. **Create Freshness Checker** (`api/application/market_context/freshness.py`)
   ```python
   class FreshnessChecker:
       STALE_THRESHOLD_SECONDS = 300  # 5 minutes

       def check_freshness(self, symbol: str) -> dict:
           latest_metric = MetricPoint.objects.filter(symbol=symbol).latest('timestamp')
           age_seconds = (datetime.now(timezone.utc) - latest_metric.timestamp).total_seconds()

           is_stale = age_seconds > self.STALE_THRESHOLD_SECONDS

           return {
               'is_stale': is_stale,
               'age_seconds': age_seconds,
               'latest_timestamp': latest_metric.timestamp,
               'threshold_seconds': self.STALE_THRESHOLD_SECONDS,
           }
   ```

2. **Update Classifier** (`api/application/market_context/classifiers.py`)
   ```python
   class HeuristicRegimeClassifier:
       def classify(self, features: dict[str, Decimal], freshness: dict) -> MarketContextSnapshot:
           # ... existing classification logic ...

           # Override if data is stale
           if freshness['is_stale']:
               risk_bias = RiskBias.CONSERVATIVE
               posture = TradingPosture.HOLD
               explanation['freshness_warning'] = f"Data stale ({freshness['age_seconds']}s old)"

           # ... return snapshot ...
   ```

3. **Create Django Command** (`api/management/commands/monitor_context_freshness.py`)
   ```python
   class Command(BaseCommand):
       def handle(self, *args, **options):
           threshold = options['threshold']
           continuous = options['continuous']
           interval = options['interval']

           checker = FreshnessChecker()

           if continuous:
               while True:
                   freshness = checker.check_freshness('BTCUSDT')
                   if freshness['is_stale']:
                       self.stderr.write(f"⚠️  Data stale: {freshness['age_seconds']}s old")
                   else:
                       self.stdout.write("✓ Data fresh")
                   time.sleep(interval)
           else:
               freshness = checker.check_freshness('BTCUSDT')
               if freshness['is_stale']:
                   self.stderr.write(f"⚠️  Data stale: {freshness['age_seconds']}s old")
                   sys.exit(1)
   ```

4. **Write Unit Tests** (`api/tests/test_market_context.py`)
   - Test freshness check with recent data (not stale)
   - Test freshness check with old data (stale)
   - Test classifier degradation to CONSERVATIVE when stale

5. **Manual Validation**
   ```bash
   # Check freshness
   python manage.py monitor_context_freshness --threshold 300

   # Simulate stale data (stop collector for 6 minutes)
   # Wait 6 minutes...
   python manage.py monitor_context_freshness --threshold 300
   # Should exit with error code 1

   # Generate snapshot with stale data
   python manage.py generate_market_context --symbol BTCUSDT
   # Should show CONSERVATIVE bias with freshness_warning
   ```

**Success Criteria**:
- [ ] Freshness checker implemented
- [ ] Classifier degrades to CONSERVATIVE when stale
- [ ] Freshness warning added to explanation payload
- [ ] Unit tests pass (stale and fresh scenarios)
- [ ] Manual validation confirms graceful degradation

---

### MILESTONE 5: On-Chain Signals (Optional - Week 5+)

**Goal**: Add minimal on-chain data (DeFiLlama TVL trend) to enrich context.

**Tasks**:

1. **Create DeFiLlama Adapter** (`api/application/market_context/adapters.py`)
   ```python
   class DeFiLlamaMetricCollector:
       BASE_URL = "https://api.llama.fi"

       def collect_metrics(self, symbol: str) -> list[MetricPoint]:
           # Fetch TVL for Bitcoin protocol
           response = requests.get(f"{self.BASE_URL}/tvl/bitcoin")
           data = response.json()

           return [
               MetricPoint(
                   timestamp=datetime.now(timezone.utc),
                   symbol='BTCUSDT',  # Map protocol to symbol
                   metric_name='tvl_total',
                   value=Decimal(data['tvl']),
                   source='defillama',
                   tags={'protocol': 'bitcoin'},
               )
           ]
   ```

2. **Create TVL Feature Builder** (`api/application/market_context/feature_builders.py`)
   ```python
   class TVLFeatureBuilder:
       def compute_delta_7d(self, metrics: list[MetricPoint]) -> FeatureVector:
           # Calculate % change in TVL over 7 days
           pass

       def compute_trend(self, metrics: list[MetricPoint]) -> FeatureVector:
           # Classify trend as UP, DOWN, FLAT
           pass
   ```

3. **Update Classifier** (`api/application/market_context/classifiers.py`)
   ```python
   class HeuristicRegimeClassifier:
       def classify(self, features: dict, on_chain_features: dict) -> MarketContextSnapshot:
           # ... existing logic ...

           # Enhance with on-chain signals
           if on_chain_features.get('tvl_trend') == 'DOWN' and regime == MarketRegime.NORMAL:
               risk_bias = RiskBias.CONSERVATIVE
               explanation['tvl_warning'] = "TVL declining (-10% over 7d)"

           # ... return snapshot ...
   ```

4. **Write Unit Tests** (`api/tests/test_market_context.py`)
   - Test DeFiLlama collector with mocked API
   - Test TVL feature builders
   - Test classifier enhancement with TVL signals

5. **Manual Validation**
   ```bash
   # Collect TVL metrics
   python manage.py collect_onchain_metrics --source defillama --symbol BTCUSDT

   # Generate snapshot with on-chain signals
   python manage.py generate_market_context --symbol BTCUSDT --include-onchain

   # Check explanation payload for TVL reasoning
   ```

**Success Criteria**:
- [ ] DeFiLlama collector implemented
- [ ] TVL features computed
- [ ] Classifier enhanced with TVL signals
- [ ] Unit tests pass
- [ ] Manual validation confirms TVL integration

---

## Deployment Plan

### Phase 1: Local Testing

**Environment**: Developer laptop

**Steps**:
1. Run migrations
2. Collect metrics manually
3. Generate snapshots manually
4. Validate database records
5. Test API endpoints

**Commands**:
```bash
# Setup
python manage.py makemigrations
python manage.py migrate

# Collect
python manage.py collect_derivatives_metrics --symbol BTCUSDT

# Generate
python manage.py generate_market_context --symbol BTCUSDT

# Validate
python manage.py shell
>>> from api.models.market_context import *
>>> MetricPoint.objects.count()
>>> FeatureVector.objects.count()
>>> MarketContextSnapshot.objects.count()
```

---

## APPENDIX: Kubernetes Deployment Examples (OPTIONAL)

**Note**: These sections are OPTIONAL and only relevant if Kubernetes CronJobs are the existing standard deployment pattern in this repository. If Phase 0.5 discovery reveals alternative schedulers (cron, systemd timers, etc.), adapt accordingly.

---

### Phase 2: Staging Deployment (Kubernetes Example)

**Environment**: Kubernetes staging namespace (if applicable)

**Steps**:
1. Build Docker image
2. Push to registry
3. Deploy CronJobs
4. Monitor logs
5. Validate data collection

**Kubernetes Manifests**:

**File**: `infra/k8s/overlays/staging/cronjobs/collect-derivatives-metrics.yaml`
```yaml
apiVersion: batch/v1
kind: CronJob
metadata:
  name: collect-derivatives-metrics
  namespace: staging
spec:
  schedule: "*/1 * * * *"  # Every minute
  successfulJobsHistoryLimit: 3
  failedJobsHistoryLimit: 3
  jobTemplate:
    spec:
      template:
        spec:
          restartPolicy: OnFailure
          containers:
          - name: collector
            image: ghcr.io/ldamasio/rbs-backend-monolith-staging:latest
            command:
            - python
            - manage.py
            - collect_derivatives_metrics
            - --symbol
            - BTCUSDT
            envFrom:
            - secretRef:
                name: django-secrets
```

**File**: `infra/k8s/overlays/staging/cronjobs/generate-market-context.yaml`
```yaml
apiVersion: batch/v1
kind: CronJob
metadata:
  name: generate-market-context
  namespace: staging
spec:
  schedule: "*/5 * * * *"  # Every 5 minutes
  successfulJobsHistoryLimit: 3
  failedJobsHistoryLimit: 3
  jobTemplate:
    spec:
      template:
        spec:
          restartPolicy: OnFailure
          containers:
          - name: context-engine
            image: ghcr.io/ldamasio/rbs-backend-monolith-staging:latest
            command:
            - python
            - manage.py
            - generate_market_context
            - --symbol
            - BTCUSDT
            envFrom:
            - secretRef:
                name: django-secrets
```

**Deploy Commands**:
```bash
# Build and push image
docker build -t ghcr.io/ldamasio/rbs-backend-monolith-staging:latest apps/backend/monolith
docker push ghcr.io/ldamasio/rbs-backend-monolith-staging:latest

# Apply CronJobs
kubectl apply -f infra/k8s/overlays/staging/cronjobs/collect-derivatives-metrics.yaml
kubectl apply -f infra/k8s/overlays/staging/cronjobs/generate-market-context.yaml

# Monitor logs
kubectl logs -n staging -l app=collect-derivatives-metrics --tail=50 -f
kubectl logs -n staging -l app=generate-market-context --tail=50 -f

# Check jobs
kubectl get cronjobs -n staging
kubectl get jobs -n staging
```

---

### Phase 3: Production Deployment (Kubernetes Example)

**Environment**: Kubernetes production namespace (if applicable)

**Prerequisites**:
- [ ] Staging validated for 7 days with no errors
- [ ] Database performance tested (no slow queries)
- [ ] Binance API rate limits confirmed (no 429 errors)
- [ ] Alerting configured (stale data, job failures)

**Steps**:
1. Update production manifests
2. Run migrations on production database
3. Deploy CronJobs with --dry-run first
4. Validate dry-run logs
5. Remove --dry-run flag
6. Monitor for 24 hours

**Production Manifests**: (same as staging, with `namespace: robson`)

**Deploy Commands**:
```bash
# Run migration
kubectl exec -n robson deploy/rbs-backend-monolith-prod-deploy -- python manage.py migrate

# Apply CronJobs (dry-run first)
kubectl apply -f infra/k8s/overlays/production/cronjobs/ --dry-run=client

# Apply for real
kubectl apply -f infra/k8s/overlays/production/cronjobs/

# Monitor
kubectl logs -n robson -l app=collect-derivatives-metrics --tail=100 -f
```

---

## Test Plan

### Unit Tests

**Location**: `apps/backend/monolith/api/tests/test_market_context.py`

**Coverage Approach**: Best-effort coverage for all new code (no strict % requirement)

**Test Cases**:

1. **MetricPoint Domain Entity**
   - Test creation with valid data
   - Test immutability (frozen dataclass)
   - Test field validation

2. **DjangoMetricRepository**
   - Test save_metric with new record
   - Test save_metric with duplicate (idempotency)
   - Test get_metrics with date range filtering

3. **DerivativesMetricCollector**
   - Test collect_metrics with mocked exchange API
   - Test error handling (API down, rate limit)
   - Test normalization to MetricPoint
   - If wrapping existing collector: test wrapper delegates correctly

4. **Feature Builders**
   - Test z-score calculation (known input/output)
   - Test moving average (simple MA)
   - Test delta calculation (% change)
   - Test edge cases (insufficient data, zero values)

5. **HeuristicRegimeClassifier**
   - Test SQUEEZE_RISK classification (synthetic data)
   - Test HIGH_VOL classification
   - Test CHOP_RISK classification
   - Test NORMAL classification (no extremes)

6. **FreshnessChecker**
   - Test with fresh data (< 5 min old)
   - Test with stale data (> 5 min old)
   - Test threshold configuration

**Run Tests**:
```bash
cd apps/backend/monolith
pytest api/tests/test_market_context.py -v --cov=api/application/market_context --cov-report=html
```

---

### Integration Tests

**Location**: `apps/backend/monolith/api/tests/test_market_context_integration.py`

**Coverage Requirements**: Full pipeline (collect → features → classify)

**Test Cases**:

1. **Full Pipeline**
   - Collect metrics from mocked exchange API
   - Compute features from collected metrics
   - Classify regime from features
   - Persist snapshot to database
   - Validate snapshot structure and explanation

2. **Freshness Monitor Integration**
   - Create stale metrics (old timestamp)
   - Generate snapshot
   - Validate CONSERVATIVE degradation
   - Validate freshness_warning in explanation

**Run Tests**:
```bash
cd apps/backend/monolith
pytest api/tests/test_market_context_integration.py -v --cov --cov-report=html
```

---

### Manual Testing Checklist

**Before Milestone Sign-Off**:

1. **Milestone 1: Derivatives Storage**
   - [ ] Collect metrics manually (`python manage.py collect_derivatives_metrics --symbol BTCUSDT`)
   - [ ] Verify records in database (`MetricPoint.objects.count()`)
   - [ ] Run command twice, confirm idempotency (count unchanged)
   - [ ] Check metric values match Binance API response

2. **Milestone 2: Feature Engineering**
   - [ ] Compute features manually (`python manage.py shell` + use case)
   - [ ] Verify FeatureVector records in database
   - [ ] Validate z-score calculation (compare to manual calc)
   - [ ] Validate source_metrics traceability

3. **Milestone 3: Regime Classifier**
   - [ ] Generate snapshot manually (`python manage.py generate_market_context --symbol BTCUSDT`)
   - [ ] Verify MarketContextSnapshot in database
   - [ ] Validate explanation_payload structure
   - [ ] Test all 4 regime classifications (use synthetic data if needed)

4. **Milestone 4: Freshness Monitor**
   - [ ] Stop collector for 6 minutes
   - [ ] Run freshness monitor, confirm stale warning
   - [ ] Generate snapshot, confirm CONSERVATIVE degradation
   - [ ] Resume collector, confirm fresh status

---

## Risk Mitigation

### Risk 1: Binance API Rate Limits

**Likelihood**: Medium
**Impact**: High (data collection stops)

**Mitigation**:
- Collect metrics every 60s (well below 2400 req/min limit)
- Implement exponential backoff on 429 errors
- Cache metrics locally (reduce redundant API calls)
- Monitor rate limit headers (`X-MBX-USED-WEIGHT`)

---

### Risk 2: Database Performance (Time-Series Data)

**Likelihood**: Medium
**Impact**: Medium (slow queries)

**Mitigation**:
- Use composite indexes on `(symbol, metric_name, timestamp)`
- Partition table by date (future optimization)
- Archive old metrics (>30 days) to cold storage
- Use database-level time-series optimizations (TimescaleDB extension if needed)

---

### Risk 3: Incorrect Threshold Calibration

**Likelihood**: High
**Impact**: Medium (false positives/negatives)

**Mitigation**:
- Backtest thresholds with historical data
- Validate against known squeeze events (2021-05 BTC dump)
- Make thresholds configurable (future phase)
- Monitor false positive rate in production
- A/B test threshold variations

---

### Risk 4: Stale Data Not Detected

**Likelihood**: Low
**Impact**: High (wrong decisions based on stale context)

**Mitigation**:
- Freshness monitor as separate CronJob (runs every 60s)
- Alert on stale data (PagerDuty, Slack)
- Automatic degradation to CONSERVATIVE
- Display freshness warning prominently in UI

---

## Rollback Plan

**If issues occur**:

1. **Stop data collection** (if deployed as scheduled jobs):
   - If using Kubernetes CronJobs:
     ```bash
     kubectl patch cronjob collect-derivatives-metrics -n robson -p '{"spec": {"suspend": true}}'
     kubectl patch cronjob generate-market-context -n robson -p '{"spec": {"suspend": true}}'
     ```
   - If using alternative scheduler: disable cron/systemd timer

3. **Database Rollback** (if migration corrupts data):
   ```bash
   # Rollback last migration
   kubectl exec -n robson deploy/rbs-backend-monolith-prod-deploy -- python manage.py migrate api <previous_migration>
   ```

4. **Delete Snapshots** (if classifications are incorrect):
   ```bash
   kubectl exec -n robson deploy/rbs-backend-monolith-prod-deploy -- python manage.py shell
   >>> from api.models.market_context import MarketContextSnapshot
   >>> MarketContextSnapshot.objects.all().delete()
   ```

---

## Success Metrics

**Phase 1 (4-6 weeks)** - Core derivatives context:
- [ ] 100% uptime for metrics collection (no job failures)
- [ ] <1% stale data incidents (freshness monitor catches)
- [ ] Test coverage for market context modules (best-effort, not strict %)
- [ ] <100ms snapshot query latency

**Phase 2 (Future)** - Enhancements:
- [ ] Regime classification accuracy validated against known events (backtesting)
- [ ] On-chain signals integrated (DeFiLlama TVL trend)
- [ ] Multi-symbol support (ETH, SOL, etc.)
- [ ] Integration with decision boundaries (EntryGate in Core 1.1)

---

## Next Steps (After Plan Validation)

1. **Phase 0.5: Code Discovery** (priority task)
   - Locate existing exchange integrations (BinanceService, adapters)
   - Identify derivatives metrics availability (funding, OI, mark price)
   - Determine integration approach (wrap vs extend vs new)
   - Document findings in `docs/market-context/DISCOVERY-FINDINGS.md`

2. **Milestone 1: Implementation** (after Phase 0.5 complete)
   - Create `MetricPoint` model and migration
   - Implement collector (based on Phase 0.5 findings)
   - Write unit tests (heuristic correctness)
   - Validate locally (production deployment optional)

3. **Iteration Checkpoints**
   - Review progress against milestones
   - Address blockers discovered during implementation
   - Adjust thresholds based on real data (iterate heuristics)
   - Update plan if discovery reveals major gaps

---

**Plan Status**: REVISED (discovery-driven approach, EntryGate out of scope)
**Author**: Claude Code
**Date**: 2025-12-28
**Version**: 1.1.0 (Revised for Option B feedback)
