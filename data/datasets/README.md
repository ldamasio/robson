# Data Datasets

**Bronze/Silver/Gold layer datasets**

This directory contains the data lake layer structure with sample data and documentation.

---

## Directory Structure

```
data/datasets/
├── bronze/                    (Raw, append-only, immutable)
│   ├── events/
│   │   └── date=2024-12-27/
│   │       ├── _metadata/
│   │       │   └── README.md  (this file)
│   │       ├── part-0001.parquet
│   │       └── part-0002.parquet
│   ├── orders/
│   │   └── client_id=1/
│   │       └── date=2024-12-27/
│   └── trades/
│       └── date=2024-12-27/
│
├── silver/                    (Cleaned, typed, validated)
│   ├── stop_executions/
│   │   └── client_id=1/
│   │       └── date=2024-12-27/
│   ├── operations/
│   │   └── strategy_id=1/
│   │       └── date=2024-12-27/
│   └── portfolio_snapshots/
│       └── client_id=1/
│           └── date=2024-12-27/
│
└── gold/                      (ML-ready, feature-engineered)
    ├── training_sets/
    │   ├── stop_outcome/
    │   │   └── v1.0.0/
    │   │       ├── train/
    │   │       │   └── date=2024-12-27/
    │   │       ├── test/
    │   │       │   └── date=2024-12-27/
    │   │       └── features.json
    │   └── slippage_prediction/
    │       └── v1.0.0/
    └── features/              (Online store for inference)
        └── stop_execution_features/
            └── client_id=1/
                └── date=2024-12-27/
```

---

## Bronze Layer

### `/events/`

**Source**: `api.models.event_sourcing.StopEvent`

**Ingestion**: `bronze-ingest` CronJob (hourly)

**Schema**: See `data/schemas/README.md`

**Partitioning**: `date=YYYY-MM-DD` (daily)

**Sample Data** (first 10 rows):
```bash
# Query sample
spark-sql --master k8s://https://kubernetes.default.svc \
    -e "SELECT * FROM robson_datalake.bronze.events WHERE date='2024-12-27' LIMIT 10;"
```

**Retention**: 90 days

**Use Cases**:
- Event replay for bug fixes
- Backfill silver layer with new logic
- Audit trail for compliance

---

### `/orders/`

**Source**: `api.models.audit.AuditTransaction` (transaction_type in SPOT/MARGIN orders)

**Ingestion**: `bronze-ingest` CronJob (hourly)

**Schema**: See `data/schemas/README.md`

**Partitioning**: `client_id=XX/date=YYYY-MM-DD` (multi-tenant)

**Retention**: 90 days

**Use Cases**:
- Order history analysis
- Cost basis tracking
- Tax reporting

---

### `/trades/`

**Source**: Binance API sync via `python manage.py sync_transactions`

**Ingestion**: `bronze-ingest` CronJob (hourly)

**Schema**: Binance trade format (see `python-binance` docs)

**Partitioning**: `date=YYYY-MM-DD` (daily)

**Retention**: 90 days

**Use Cases**:
- Trade reconciliation
- Exchange fee analysis
- Slippage analysis

---

## Silver Layer

### `/stop_executions/`

**Source**: Materialized view from `bronze.events` via event replay

**Transformation**: `silver-transform` CronJob (daily)

**Schema**: See `data/schemas/README.md`

**Partitioning**: `client_id=XX/date=YYYY-MM-DD`

**Data Quality**:
- No nulls in critical columns (execution_id, operation_id, status)
- Slippage within ±50%
- Executed orders must have fill_price

**Retention**: 365 days

**Use Cases**:
- Analytical queries (Spark SQL, Presto)
- Dashboard data source
- Feature engineering for gold layer

**Sample Query**:
```sql
-- Success rate by client
SELECT
    client_id,
    COUNT(*) as total_executions,
    SUM(CASE WHEN status = 'EXECUTED' THEN 1 ELSE 0 END) as successful,
    SUM(CASE WHEN status = 'EXECUTED' THEN 1 ELSE 0 END) * 100.0 / COUNT(*) as success_rate_pct
FROM silver.stop_executions
WHERE date BETWEEN '2024-01-01' AND '2024-12-27'
GROUP BY client_id
ORDER BY success_rate_pct DESC;

-- Average slippage by symbol
SELECT
    symbol,
    AVG(slippage_pct) as avg_slippage_pct,
    PERCENTILE(slippage_pct, 0.5) as median_slippage_pct,
    PERCENTILE(slippage_pct, 0.95) as p95_slippage_pct
FROM silver.stop_executions
WHERE date BETWEEN '2024-01-01' AND '2024-12-27'
  AND slippage_pct IS NOT NULL
GROUP BY symbol
ORDER BY avg_slippage_pct ASC;
```

---

### `/operations/`

**Source**: Aggregated from `bronze.orders` + `silver.stop_executions`

**Transformation**: `silver-transform` CronJob (daily)

**Schema**: Operation-level summary (position size, entry/exit, PnL)

**Partitioning**: `strategy_id=XX/date=YYYY-MM-DD`

**Retention**: 365 days

**Use Cases**:
- Strategy performance analysis
- Risk metrics (drawdown, exposure)
- PnL calculation

---

### `/portfolio_snapshots/`

**Source**: Aggregated from `api.models.portfolio.Portfolio`

**Transformation**: `silver-transform` CronJob (daily)

**Schema**: Portfolio value per client per timestamp

**Partitioning**: `client_id=XX/date=YYYY-MM-DD`

**Retention**: 365 days

**Use Cases**:
- Portfolio growth tracking
- Return on investment (ROI)
- Risk-adjusted returns (Sharpe ratio)

---

## Gold Layer

### `/training_sets/stop_outcome/`

**Source**: Feature engineering from `silver.stop_executions`

**Transformation**: `gold-features` CronJob (weekly)

**Schema**: See `data/schemas/README.md`

**Versioning**: `v1.0.0`, `v1.1.0`, etc. (immutable)

**Partitioning**: `vVERSION/train|test/date=YYYY-MM-DD`

**Features**:
- `stop_distance_pct`: Distance from entry to stop
- `time_to_trigger_sec`: Time to stop trigger
- `volatility_15m`: Price volatility before trigger
- `volume_24h`: Trading volume
- `hour_of_day`, `day_of_week`: Temporal features

**Labels**:
- `execution_success`: Boolean (target variable)
- `slippage_breach`: Boolean (auxiliary label)
- `execution_time_sec`: Regression label

**Retention**: Indefinite (versioned)

**Use Cases**:
- ML model training (classification, regression)
- Backtesting with historical data
- Model comparison (v1.0.0 vs v1.1.0)

**Sample Training Script**:
```python
import pandas as pd
from pyspark.sql import SparkSession

# Load training data
spark = SparkSession.builder.appName("StopOutcomeModel").getOrCreate()

train_df = spark.read.parquet(
    "s3a://robson-datalake/gold/training_sets/stop_outcome/v1.0.0/train/"
)

test_df = spark.read.parquet(
    "s3a://robson-datalake/gold/training_sets/stop_outcome/v1.0.0/test/"
)

# Train model (example: XGBoost)
# ...

# Evaluate
# ...

# Save model
# model.write().overwrite().save("s3a://robson-datalake/gold/models/stop_outcome/v1.0.0/")
```

---

### `/features/` (Online Feature Store)

**Source**: Real-time feature computation from `silver.stop_executions`

**Purpose**: Online inference (low-latency feature lookup)

**Format**: Parquet + feature metadata JSON

**Partitioning**: `client_id=XX/date=YYYY-MM-DD`

**Retention**: 90 days (online features are ephemeral)

**Use Cases**:
- Real-time model inference
- Feature serving via API
- A/B testing new features

**Example Feature Lookup**:
```python
# Feature store API (hypothetical)
from feature_store import get_latest_features

# Get features for client 1, symbol BTCUSDC
features = get_latest_features(
    client_id=1,
    symbol="BTCUSDC",
    feature_set="stop_execution_features"
)

print(features)
# {
#     "stop_distance_pct": 2.5,
#     "volatility_15m": 0.015,
#     "volume_24h": 15000000,
#     ...
# }
```

---

## Data Lifecycle

### Bronze → Silver Transformation

```
┌─────────────────────────────────────────────────────────────────┐
│  BRONZE (Raw Events)                                            │
│  - Append-only                                                  │
│  - JSON schema (flexible)                                       │
│  - 90-day retention                                            │
└─────────────────────────────────────────────────────────────────┘
                        │
                        ▼
┌─────────────────────────────────────────────────────────────────┐
│  SILVER (Cleaned Features)                                      │
│  - Typed, validated                                            │
│  - Parquet format (columnar)                                    │
│  - 365-day retention                                            │
│  - Data quality checks                                          │
└─────────────────────────────────────────────────────────────────┘
                        │
                        ▼
┌─────────────────────────────────────────────────────────────────┐
│  GOLD (ML-Ready)                                                │
│  - Feature-engineered                                          │
│  - Versioned (v1.0.0, v1.1.0)                                   │
│  - Indefinite retention                                         │
│  - Training/test splits                                         │
└─────────────────────────────────────────────────────────────────┘
```

### Backfill Workflow

```bash
# Re-run silver transformation for specific date range
spark-submit --class BackfillJob \
    --driver-memory 2G \
    --executor-memory 4G \
    --num-executors 1 \
    backfill.py \
        --start-date 2024-01-01 \
        --end-date 2024-12-27 \
        --input bronze.events \
        --output silver.stop_executions_v2 \
        --overwrite
```

---

## Access Patterns

### Read-Heavy (Analytics)
- **Tool**: Spark SQL, Presto/Trino
- **Pattern**: Scan large partitions, aggregate
- **Optimization**: Columnar Parquet, partition pruning

### Write-Once (Ingestion)
- **Tool**: Spark jobs (bronze/silver/gold)
- **Pattern**: Append-only, never update
- **Optimization**: Partition by date, coalesce for large files

### Read-Write (Feature Store)
- **Tool**: Custom API, Redis cache
- **Pattern**: Random access by key
- **Optimization**: Materialized views, caching

---

## Monitoring

### Data Quality Metrics

```sql
-- Bronze layer: Row count over time
SELECT date, COUNT(*) as row_count
FROM bronze.events
GROUP BY date
ORDER BY date DESC
LIMIT 30;

-- Silver layer: Null percentage
SELECT
    COUNT(*) as total_rows,
    COUNT(execution_id) as non_null_executions,
    COUNT(fill_price) as non_null_fill_prices,
    (COUNT(*) - COUNT(fill_price)) * 100.0 / COUNT(*) as null_fill_price_pct
FROM silver.stop_executions
WHERE date = '2024-12-27';

-- Gold layer: Label distribution
SELECT
    label.execution_success,
    COUNT(*) as sample_count,
    COUNT(*) * 100.0 / SUM(COUNT(*)) OVER() as percentage
FROM gold.training_sets.stop_outcome
WHERE version = 'v1.0.0'
GROUP BY label.execution_success;
```

### Freshness Checks

```sql
-- Bronze freshness (max lag)
SELECT
    CURRENT_TIMESTAMP - MAX(occurred_at) as max_lag
FROM bronze.events
WHERE date = '2024-12-27';

-- Silver freshness (job completion)
SELECT
    MAX(created_at) as last_silver_update,
    CURRENT_TIMESTAMP - MAX(created_at) as silver_lag
FROM silver.stop_executions
WHERE date = '2024-12-27';
```

---

**Last Updated**: 2024-12-27
**Related**: ADR-0013, data/schemas/README.md, data/contracts/README.md
