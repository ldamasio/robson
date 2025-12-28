# Data Schemas

**Schema definitions for Robson Bot data lake layers (bronze/silver/gold)**

This directory contains schema definitions, contracts, and validation rules for all data lake tables.

---

## Schema Format

**Choice**: JSON Schema

**Rationale**:
- Human-readable and Git-friendly
- Language-agnostic validation (Python, JavaScript, Go)
- Integrates with Great Expectations (data quality testing)
- Supports schema evolution (backward compatibility)

**Alternative Considered**:
- **Protobuf**: More compact, but requires code generation and .proto files
- **Avro**: Good for streaming, but more complex than JSON Schema for batch use case
- **Arrow IPC**: Excellent for in-memory, but overkill for storage schema

---

## Bronze Layer Schemas (Raw Events)

### StopEvent Schema

**Source**: `apps/backend/monolith/api/models/event_sourcing.py`

**File**: `stop_event.json`

```json
{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "title": "StopEvent",
  "description": "Immutable event representing a stop-loss state transition",
  "type": "object",
  "properties": {
    "event_id": {
      "type": "string",
      "format": "uuid",
      "description": "Unique event identifier"
    },
    "event_seq": {
      "type": "integer",
      "minimum": 1,
      "description": "Global sequence number for event ordering"
    },
    "occurred_at": {
      "type": "string",
      "format": "date-time",
      "description": "When the event occurred"
    },
    "operation_id": {
      "type": "integer",
      "description": "Related trading operation ID"
    },
    "client_id": {
      "type": "integer",
      "description": "Tenant/client ID for multi-tenant isolation"
    },
    "symbol": {
      "type": "string",
      "pattern": "^[A-Z]+USDC?$",
      "description": "Trading pair (e.g., BTCUSDC)"
    },
    "event_type": {
      "type": "string",
      "enum": [
        "STOP_TRIGGERED",
        "EXECUTION_SUBMITTED",
        "EXECUTED",
        "FAILED",
        "BLOCKED",
        "STALE_PRICE",
        "KILL_SWITCH",
        "SLIPPAGE_BREACH",
        "CIRCUIT_BREAKER"
      ],
      "description": "Type of event"
    },
    "trigger_price": {
      "type": "number",
      "description": "Price that triggered the stop"
    },
    "stop_price": {
      "type": "number",
      "description": "Configured stop level (absolute price)"
    },
    "quantity": {
      "type": "number",
      "minimum": 0,
      "description": "Quantity to close"
    },
    "side": {
      "type": "string",
      "enum": ["BUY", "SELL"],
      "description": "Order side (closing direction)"
    },
    "execution_token": {
      "type": "string",
      "description": "Global idempotency token"
    },
    "payload_json": {
      "type": "object",
      "description": "Complete event context"
    },
    "exchange_order_id": {
      "type": ["string", "null"],
      "description": "Binance order ID (if executed)"
    },
    "fill_price": {
      "type": ["number", "null"],
      "description": "Actual fill price from exchange"
    },
    "slippage_pct": {
      "type": ["number", "null"],
      "description": "Calculated slippage percentage"
    },
    "source": {
      "type": "string",
      "enum": ["ws", "cron", "manual"],
      "description": "Which component emitted this event"
    },
    "error_message": {
      "type": ["string", "null"],
      "description": "Error details if execution failed"
    },
    "retry_count": {
      "type": "integer",
      "minimum": 0,
      "description": "Number of retry attempts"
    }
  },
  "required": [
    "event_id",
    "event_seq",
    "occurred_at",
    "operation_id",
    "client_id",
    "symbol",
    "event_type"
  ],
  "additionalProperties": false
}
```

### AuditTransaction Schema

**Source**: `apps/backend/monolith/api/models/audit.py`

**File**: `audit_transaction.json`

```json
{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "title": "AuditTransaction",
  "description": "Complete audit trail of financial movements",
  "type": "object",
  "properties": {
    "transaction_id": {
      "type": "string",
      "format": "uuid",
      "description": "Unique transaction identifier"
    },
    "operation_id": {
      "type": "integer",
      "description": "Related trading operation ID"
    },
    "client_id": {
      "type": "integer",
      "description": "Tenant/client ID"
    },
    "transaction_type": {
      "type": "string",
      "enum": [
        "SPOT_BUY",
        "SPOT_SELL",
        "MARGIN_BUY",
        "MARGIN_SELL",
        "TRANSFER_SPOT_TO_ISOLATED",
        "TRANSFER_ISOLATED_TO_SPOT",
        "MARGIN_BORROW",
        "MARGIN_REPAY",
        "STOP_LOSS_PLACED",
        "STOP_LOSS_TRIGGERED",
        "STOP_LOSS_EXECUTED",
        "TRADING_FEE",
        "INTEREST_CHARGED",
        "DEPOSIT",
        "WITHDRAWAL"
      ],
      "description": "Type of financial movement"
    },
    "asset": {
      "type": "string",
      "description": "Asset symbol (e.g., BTC, USDC)"
    },
    "quantity": {
      "type": "number",
      "description": "Quantity (positive for credit, negative for debit)"
    },
    "price": {
      "type": ["number", "null"],
      "description": "Price per unit (if applicable)"
    },
    "timestamp": {
      "type": "string",
      "format": "date-time",
      "description": "When the transaction occurred"
    },
    "exchange_ref": {
      "type": ["string", "null"],
      "description": "Exchange transaction ID (if applicable)"
    },
    "metadata": {
      "type": "object",
      "description": "Additional transaction context"
    }
  },
  "required": [
    "transaction_id",
    "operation_id",
    "client_id",
    "transaction_type",
    "asset",
    "quantity",
    "timestamp"
  ]
}
```

---

## Silver Layer Schemas (Cleaned Features)

### StopExecution Schema

**Source**: Derived from `StopEvent` via event replay

**File**: `stop_execution.json`

```json
{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "title": "StopExecution",
  "description": "Materialized view of latest execution state per operation",
  "type": "object",
  "properties": {
    "execution_id": {
      "type": "string",
      "format": "uuid",
      "description": "Unique execution identifier"
    },
    "operation_id": {
      "type": "integer",
      "description": "Related trading operation ID"
    },
    "client_id": {
      "type": "integer",
      "description": "Tenant/client ID"
    },
    "symbol": {
      "type": "string",
      "description": "Trading pair"
    },
    "stop_price": {
      "type": "number",
      "description": "Fixed technical stop level"
    },
    "trigger_price": {
      "type": ["number", "null"],
      "description": "Price at detection"
    },
    "quantity": {
      "type": "number",
      "description": "Quantity to close"
    },
    "side": {
      "type": "string",
      "enum": ["BUY", "SELL"],
      "description": "Order side"
    },
    "status": {
      "type": "string",
      "enum": ["PENDING", "SUBMITTED", "EXECUTED", "FAILED", "BLOCKED"],
      "description": "Current execution status"
    },
    "triggered_at": {
      "type": ["string", "null"],
      "format": "date-time",
      "description": "When stop was triggered"
    },
    "submitted_at": {
      "type": ["string", "null"],
      "format": "date-time",
      "description": "When order was submitted to exchange"
    },
    "executed_at": {
      "type": ["string", "null"],
      "format": "date-time",
      "description": "When order was filled"
    },
    "exchange_order_id": {
      "type": ["string", "null"],
      "description": "Binance order ID"
    },
    "fill_price": {
      "type": ["number", "null"],
      "description": "Actual fill price"
    },
    "slippage_pct": {
      "type": ["number", "null"],
      "description": "Calculated slippage"
    },
    "source": {
      "type": "string",
      "enum": ["ws", "cron", "manual"],
      "description": "Which component executed this"
    }
  },
  "required": [
    "execution_id",
    "operation_id",
    "client_id",
    "symbol",
    "stop_price",
    "quantity",
    "side",
    "status"
  ]
}
```

---

## Gold Layer Schemas (ML-Ready Features)

### Training Set Schema

**File**: `stop_outcome_training_set.json`

```json
{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "title": "StopOutcomeTrainingSet",
  "description": "ML-ready features for predicting stop-loss execution outcomes",
  "type": "object",
  "properties": {
    "sample_id": {
      "type": "string",
      "format": "uuid",
      "description": "Unique sample identifier"
    },
    "client_id": {
      "type": "integer",
      "description": "Tenant/client ID"
    },
    "symbol": {
      "type": "string",
      "description": "Trading pair"
    },
    "features": {
      "type": "object",
      "description": "Feature vector for model input",
      "properties": {
        "stop_distance_pct": {
          "type": "number",
          "description": "Distance from entry to stop (percentage)"
        },
        "time_to_trigger_sec": {
          "type": "number",
          "description": "Time from order placement to stop trigger (seconds)"
        },
        "volatility_15m": {
          "type": "number",
          "description": "Price volatility in 15-minute window before trigger"
        },
        "volume_24h": {
          "type": "number",
          "description": "Trading volume in last 24 hours"
        },
        "hour_of_day": {
          "type": "integer",
          "minimum": 0,
          "maximum": 23,
          "description": "Hour of day (0-23)"
        },
        "day_of_week": {
          "type": "integer",
          "minimum": 0,
          "maximum": 6,
          "description": "Day of week (0=Monday, 6=Sunday)"
        },
        "consecutive_failures_1h": {
          "type": "integer",
          "minimum": 0,
          "description": "Number of consecutive failures in last 1 hour"
        }
      },
      "required": [
        "stop_distance_pct",
        "time_to_trigger_sec",
        "volatility_15m",
        "volume_24h"
      ]
    },
    "label": {
      "type": "object",
      "description": "Ground truth label for supervised learning",
      "properties": {
        "execution_success": {
          "type": "boolean",
          "description": "True if stop executed successfully, False if failed"
        },
        "slippage_breach": {
          "type": "boolean",
          "description": "True if slippage exceeded 10% threshold"
        },
        "execution_time_sec": {
          "type": "number",
          "description": "Time from trigger to execution (seconds)"
        }
      },
      "required": ["execution_success"]
    },
    "metadata": {
      "type": "object",
      "description": "Metadata for reproducibility",
      "properties": {
        "version": {
          "type": "string",
          "description": "Feature set version (e.g., v1.0.0)"
        },
        "generated_at": {
          "type": "string",
          "format": "date-time",
          "description": "When this sample was generated"
        },
        "source_event_ids": {
          "type": "array",
          "items": {
            "type": "string"
          },
          "description": "List of event IDs used to generate this sample"
        }
      }
    }
  },
  "required": [
    "sample_id",
    "client_id",
    "symbol",
    "features",
    "label",
    "metadata"
  ]
}
```

---

## Schema Evolution

**Backward Compatibility Policy**:
- New fields: ✅ Allowed (additive changes)
- Removed fields: ❌ Forbidden (breaking change)
- Changed data types: ❌ Forbidden (breaking change)
- Changed constraints: ⚠️ Allowed only if looser (e.g., expand enum)

**Versioning**:
- Schema files are immutable (never edit, only create new versions)
- Schema evolution tracked via Git history
- Major version changes: `v1.0.0 → v2.0.0` (breaking)
- Minor version changes: `v1.0.0 → v1.1.0` (additive)

**Example**:
```
data/schemas/stop_event.json                    # Current version
data/schemas/v1/stop_event.json                # Version 1.0
data/schemas/v2/stop_event.json                # Version 2.0 (breaking)
```

---

## Validation

**Using JSON Schema Validator (Python)**:

```python
import json
from jsonschema import validate, ValidationError

# Load schema
with open('data/schemas/stop_event.json') as f:
    schema = json.load(f)

# Load data
with open('s3://robson-datalake/bronze/events/date=2024-12-27/part-0001.parquet') as f:
    data = json.load(f)

# Validate
try:
    validate(instance=data, schema=schema)
    print("✅ Schema validation passed")
except ValidationError as e:
    print(f"❌ Schema validation failed: {e.message}")
```

**Using Great Expectations**:

```python
import great_expectations as gx

# Define expectation suite
expectation_suite = gx.ExpectationSuite("stop_event_suite")

# Add expectations
expectation_suite.add_expectation(
    gx.expectations.ExpectColumnValuesToBeInSet(
        column="event_type",
        value_set=[
            "STOP_TRIGGERED",
            "EXECUTED",
            "FAILED",
            # ... (all valid event types)
        ]
    )
)

expectation_suite.add_expectation(
    gx.expectations.ExpectColumnValuesToNotNull(
        column="event_id"
    )
)

expectation_suite.add_expectation(
    gx.expectations.ExpectColumnValuesToMatchRegex(
        column="symbol",
        regex="^[A-Z]+USDC?$"
    )
)
```

---

**Last Updated**: 2024-12-27
**Related**: ADR-0013, deep-storage.md runbook
