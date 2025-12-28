# Data Contracts

**Formal data contracts for bronze/silver/gold layers**

This directory defines data contracts between producers and consumers in the data lake.

---

## Purpose

Data contracts provide:
1. **Expectations**: What data consumers can expect from producers
2. **Validation Rules**: Data quality checks (null checks, ranges, patterns)
3. **SLAs**: Freshness, completeness, availability guarantees
4. **Ownership**: Who is responsible for each dataset

---

## Bronze Layer Contracts

### Contract: `bronze.events`

**Producer**: Django Outbox (via bronze-ingest job)
**Consumers**: Silver transformation jobs, data scientists

**Expectations**:
```yaml
dataset_id: bronze.events
description: Raw stop-loss events from Django Outbox
owner: platform-team
contact: ldamasio@gmail.com

schema:
  - name: event_id
    type: string (uuid)
    nullable: false
    description: Unique event identifier

  - name: event_seq
    type: integer
    nullable: false
    description: Global sequence number for ordering

  - name: occurred_at
    type: timestamp
    nullable: false
    description: Event timestamp

  - name: operation_id
    type: integer
    nullable: false
    description: Related trading operation

  - name: client_id
    type: integer
    nullable: false
    description: Tenant ID (multi-tenant isolation)

  - name: symbol
    type: string
    nullable: false
    pattern: "^[A-Z]+USDC?$"
    description: Trading pair

  - name: event_type
    type: string
    nullable: false
    enum:
      - STOP_TRIGGERED
      - EXECUTED
      - FAILED
      - BLOCKED
    description: Type of event

partitioning:
  - date (daily)

data_quality:
  - uniqueness:
      - column: event_id
      - constraint: UNIQUE

  - no_nulls:
      - columns: [event_id, event_seq, occurred_at, client_id]

  - freshness:
      - max_lag: 2 hours
      - description: "Data must be available within 2 hours of event occurrence"

sla:
  availability: 99.9% (monthly)
  freshness: 2 hours (max lag)
  completeness: 100% (no missing events)
```

**Validation SQL**:
```sql
-- Check uniqueness
SELECT event_id, COUNT(*) as cnt
FROM bronze.events
WHERE date = '2024-12-27'
GROUP BY event_id
HAVING cnt > 1;

-- Check no nulls in critical columns
SELECT
    COUNT(*) as total_rows,
    COUNT(event_id) as non_null_ids,
    COUNT(event_seq) as non_null_seqs
FROM bronze.events
WHERE date = '2024-12-27';

-- Check freshness
SELECT
    MAX(occurred_at) as latest_event,
    CURRENT_TIMESTAMP - MAX(occurred_at) as lag
FROM bronze.events
WHERE date = '2024-12-27';
```

---

## Silver Layer Contracts

### Contract: `silver.stop_executions`

**Producer**: Silver transformation job
**Consumers**: Gold feature jobs, analytics dashboards

**Expectations**:
```yaml
dataset_id: silver.stop_executions
description: Cleaned and typed stop execution records
owner: analytics-team
contact: ldamasio@gmail.com

schema:
  - name: execution_id
    type: string (uuid)
    nullable: false
    description: Unique execution identifier

  - name: operation_id
    type: integer
    nullable: false
    description: Related trading operation

  - name: client_id
    type: integer
    nullable: false
    description: Tenant ID

  - name: symbol
    type: string
    nullable: false
    description: Trading pair

  - name: status
    type: string
    nullable: false
    enum: [PENDING, SUBMITTED, EXECUTED, FAILED, BLOCKED]
    description: Execution status

  - name: slippage_pct
    type: decimal(10,4)
    nullable: true
    description: Calculated slippage percentage

partitioning:
  - client_id (multi-tenant isolation)
  - date (daily)

data_quality:
  - referential_integrity:
      - column: operation_id
      - references: bronze.operations (operation_id)

  - range_check:
      - column: slippage_pct
      - min: -50.0
      - max: 50.0
      - description: "Slippage should be within ±50%"

  - consistency:
      - rule: "IF status = 'EXECUTED' THEN fill_price IS NOT NULL"
      - description: "Executed orders must have fill price"

sla:
  availability: 99.5% (monthly)
  freshness: 24 hours (daily job)
  accuracy: >95% (via sampling)
```

**Validation SQL**:
```sql
-- Check range constraint
SELECT COUNT(*) as outliers
FROM silver.stop_executions
WHERE date = '2024-12-27'
  AND slippage_pct IS NOT NULL
  AND (slippage_pct < -50.0 OR slippage_pct > 50.0);

-- Check consistency
SELECT COUNT(*) as inconsistent
FROM silver.stop_executions
WHERE date = '2024-12-27'
  AND status = 'EXECUTED'
  AND fill_price IS NULL;
```

---

## Gold Layer Contracts

### Contract: `gold.training_sets.stop_outcome`

**Producer**: Gold feature job
**Consumers**: ML training pipelines, model evaluation

**Expectations**:
```yaml
dataset_id: gold.training_sets.stop_outcome
description: ML-ready features for stop-loss outcome prediction
owner: data-science-team
contact: ldamasio@gmail.com

schema:
  - name: sample_id
    type: string (uuid)
    nullable: false
    description: Unique sample identifier

  - name: features
    type: struct
    nullable: false
    fields:
      - name: stop_distance_pct
        type: decimal(10,4)
        nullable: false

      - name: time_to_trigger_sec
        type: integer
        nullable: false

      - name: volatility_15m
        type: decimal(10,6)
        nullable: true

  - name: label
    type: struct
    nullable: false
    fields:
      - name: execution_success
        type: boolean
        nullable: false

      - name: slippage_breach
        type: boolean
        nullable: true

  - name: metadata
    type: struct
    nullable: false
    fields:
      - name: version
        type: string
        nullable: false

      - name: generated_at
        type: timestamp
        nullable: false

partitioning:
  - version (for reproducibility)
  - date (daily)

data_quality:
  - label_distribution:
      - column: label.execution_success
      - min_ratio: 0.1
      - max_ratio: 0.9
      - description: "Labels should be balanced (not extreme skew)"

  - feature_scaling:
      - features: [stop_distance_pct, volatility_15m]
      - check: "No extreme outliers (±3 std dev)"
      - description: "Features should be normalized"

sla:
  availability: 99.0% (monthly)
  freshness: 7 days (weekly job)
  reproducibility: 100% (versioned)
```

---

## Contract Enforcement

### Phase 0: Manual Validation

Data contracts are enforced via SQL queries in runbook (see `docs/runbooks/deep-storage.md`).

### Phase 1: Automated Validation

**Using Great Expectations**:

```python
import great_expectations as gx

# Get context
context = gx.get_context()

# Connect to data source
datasource = context.data_sources.add_spark_s3(
    name="datalake_s3",
    bucket="robson-datalake"
)

# Build batch for bronze.events
batch = datasource.add_batch_asset(
    name="bronze_events",
    path="silver/stop_executions/client_id=1/date=2024-12-27/"
)

# Load expectation suite
suite = context.suites.get("bronze_events_suite")

# Validate
validation_result = batch.validate(suite)

# Check results
if validation_result.success:
    print("✅ Data contract validation passed")
else:
    print("❌ Data contract validation failed")
    for result in validation_result.results:
        if not result.success:
            print(f"  - {result.expectation_config.type}: {result.result}")
```

### Phase 2: Contract as Code

**Store contracts in Git**:
```
data/contracts/
  bronze-events.yaml
  silver-stop-executions.yaml
  gold-stop-outcome.yaml
```

**CI/CD validation**:
```yaml
# .github/workflows/validate-contracts.yml
name: Validate Data Contracts

on: [push, pull_request]

jobs:
  validate:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Validate contracts
        run: |
          python data/contracts/validate_all.py
```

---

## Contract Changes

**Breaking Changes** (require consumer approval):
- Removing a column
- Changing data type
- Tightening constraints (e.g., reducing max value)

**Non-Breaking Changes** (notify consumers):
- Adding a column
- Loosening constraints
- Adding a new partition

**Approval Process**:
1. Open PR with contract change
2. Tag all consumers in PR description
3. Run validation against historical data
4. Get consumer approval
5. Merge and deploy

---

**Last Updated**: 2024-12-27
**Related**: ADR-0013, data/schemas/README.md
