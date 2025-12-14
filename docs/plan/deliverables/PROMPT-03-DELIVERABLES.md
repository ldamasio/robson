# PROMPT 03 DELIVERABLES - Business and Risk Validation

## Summary

Successfully implemented `robson validate` - the "paper trading" stage of the agentic workflow that validates correctness and risk WITHOUT production impact.

**Principle**: Just as paper trading validates operational reality without capital risk, validation confirms correctness without production impact.

**Purpose**:
- Reduce uncertainty
- Expose hidden risks
- Block unsafe execution
- Increase confidence

**This is NOT developer CI. This is operational and financial validation.**

---

## What Was Implemented

### Validation Framework

**Location**: `apps/backend/monolith/api/application/validation.py`

**Core Components**:
1. **ValidationStatus** - Enum (PASS, FAIL, WARNING)
2. **ValidationIssue** - Single validation issue
3. **ValidationReport** - Comprehensive validation report
4. **Validator Protocol** - Interface for validators
5. **Concrete Validators**:
   - TenantIsolationValidator
   - RiskConfigurationValidator
   - OperationValidator
6. **ValidatePlanUseCase** - Orchestrates all validation

---

## Files Created

### 1. `api/application/validation.py`
**Lines**: ~650
**Purpose**: Complete validation framework

**Classes**:

#### ValidationStatus (Enum)
```python
class ValidationStatus(Enum):
    PASS = "PASS"
    FAIL = "FAIL"
    WARNING = "WARNING"
```

#### ValidationIssue (Dataclass)
```python
@dataclass
class ValidationIssue:
    code: str                    # e.g., "TENANT_MISSING"
    severity: ValidationStatus   # PASS/FAIL/WARNING
    message: str                 # Human-readable message
    details: dict                # Additional context
```

#### ValidationReport (Dataclass)
```python
@dataclass
class ValidationReport:
    status: ValidationStatus
    issues: list[ValidationIssue]
    metadata: dict

    def to_dict() -> dict           # JSON serialization
    def to_human_readable() -> str  # Text report
    def has_failures() -> bool
    def has_warnings() -> bool
```

#### TenantIsolationValidator
**Validates**: Tenant context is present and valid

**Rules**:
1. `client_id` must be present
2. `client_id` must be positive integer
3. All operations explicitly scoped to tenant

**Why Critical**: Prevents operations affecting wrong accounts in multi-tenant system

**Error Codes**:
- `TENANT_MISSING` - No client_id provided
- `TENANT_INVALID` - client_id <= 0
- `TENANT_INVALID_FORMAT` - client_id not an integer

#### RiskConfigurationValidator
**Validates**: Risk limits are defined and sane

**Rules**:
1. `risk_config` must exist
2. `max_drawdown_percent` must be defined (0-100)
   - WARNING if > 50%
3. `stop_loss_percent` must be defined (0-100)
   - WARNING if > 10%
4. `max_position_size_percent` recommended (0-100)
   - WARNING if > 50%

**Why Critical**: Enforces risk management before live execution

**Error Codes**:
- `RISK_CONFIG_MISSING` - No risk configuration
- `MAX_DRAWDOWN_MISSING` - No max drawdown defined
- `MAX_DRAWDOWN_INVALID` - Invalid value
- `MAX_DRAWDOWN_HIGH` - Warning: >50%
- `STOP_LOSS_MISSING` - No stop loss defined
- `STOP_LOSS_INVALID` - Invalid value
- `STOP_LOSS_HIGH` - Warning: >10%
- `MAX_POSITION_MISSING` - Warning: No max position
- `MAX_POSITION_INVALID` - Invalid value
- `MAX_POSITION_HIGH` - Warning: >50%

#### OperationValidator
**Validates**: Operation parameters are sane

**Rules**:
1. Operation type must be specified (buy/sell/cancel)
2. For buy/sell:
   - Symbol must be present
   - Quantity must be positive
   - Price must be positive (if limit order)

**Why Critical**: Prevents malformed orders

**Error Codes**:
- `OPERATION_TYPE_MISSING` - No operation type
- `SYMBOL_MISSING` - No symbol
- `QUANTITY_MISSING` - No quantity
- `QUANTITY_INVALID` - Quantity <= 0
- `QUANTITY_INVALID_FORMAT` - Not numeric
- `PRICE_INVALID` - Price <= 0
- `PRICE_INVALID_FORMAT` - Not numeric

#### ValidatePlanUseCase
**Orchestrates**: All validators

**Usage**:
```python
use_case = ValidatePlanUseCase()
report = use_case.execute(context)

if report.has_failures():
    print("EXECUTION BLOCKED")
else:
    print("SAFE TO EXECUTE")
```

### 2. `api/management/commands/validate_plan.py`
**Lines**: ~130
**Purpose**: Django management command for validation

**Usage**:
```bash
python manage.py validate_plan \
    --plan-id abc123 \
    --client-id 1 \
    --strategy-id 5 \
    --operation-type buy \
    --symbol BTCUSDT \
    --quantity 0.001 \
    --price 50000 \
    --json
```

**Arguments**:
- `--plan-id` (required) - Plan identifier
- `--client-id` (required) - Tenant ID (**MANDATORY**)
- `--strategy-id` (optional) - Load risk config from strategy
- `--operation-type` (optional) - buy/sell/cancel
- `--symbol` (optional) - Trading symbol
- `--quantity` (optional) - Order quantity
- `--price` (optional) - Limit price
- `--json` (optional) - JSON output

**Exit Codes**:
- `0` - Validation passed
- `1` - Validation failed

**Behavior**:
1. Loads risk_config from Strategy (if --strategy-id provided)
2. Builds validation context
3. Executes ValidatePlanUseCase
4. Outputs report (human-readable or JSON)
5. Exits with appropriate code

### 3. Updated `cli/cmd/agentic.go`
**Lines Added**: ~110
**Purpose**: Updated validate command to invoke Django

**New Functions**:
- `invokeDjangoValidation()` - Invokes Django management command
- `findDjangoManagePy()` - Locates Django manage.py

**Updated validateCmd**:
```go
robson validate <plan-id> --client-id <id> [options]
```

**New Flags**:
- `--client-id` (required) - Tenant ID
- `--strategy-id` (optional) - Strategy ID
- `--operation-type` (optional) - buy/sell/cancel
- `--symbol` (optional) - Trading symbol
- `--quantity` (optional) - Order quantity
- `--price` (optional) - Limit price

**Implementation**:
- Builds Django command with all arguments
- Executes via `python manage.py validate_plan`
- Passes through stdout/stderr
- Returns exit code from Django

### 4. `api/tests/test_validation.py`
**Lines**: ~400
**Purpose**: Comprehensive validation tests

**Test Classes**:
1. **ValidationReportTests** - Report creation and serialization
2. **TenantIsolationValidatorTests** - Tenant validation rules
3. **RiskConfigurationValidatorTests** - Risk validation rules
4. **OperationValidatorTests** - Operation validation rules
5. **ValidatePlanUseCaseTests** - End-to-end validation

**Test Coverage**:
- ✅ Empty reports
- ✅ Adding issues
- ✅ Status transitions
- ✅ Serialization (dict, human-readable)
- ✅ Missing tenant
- ✅ Invalid tenant formats
- ✅ Missing risk config
- ✅ Invalid risk values
- ✅ High risk warnings
- ✅ Missing operation params
- ✅ Invalid operation values
- ✅ Complete valid plans
- ✅ Multiple failures aggregation

---

## Files Modified

### 1. `api/application/__init__.py`
**Added Exports**:
```python
from .validation import (
    ValidationStatus,
    ValidationIssue,
    ValidationReport,
    Validator,
    TenantIsolationValidator,
    RiskConfigurationValidator,
    OperationValidator,
    ValidatePlanUseCase,
)
```

### 2. `cli/cmd/agentic.go`
**Added**:
- Import statements (encoding/json, os, os/exec, strconv)
- Updated validateCmd with new flags
- invokeDjangoValidation() function
- findDjangoManagePy() function

---

## Validation Flow

```
┌─────────────────────────────────────────────────┐
│ robson validate abc123 --client-id 1            │
│         --strategy-id 5                         │
└────────────────┬────────────────────────────────┘
                 │
                 ▼
┌─────────────────────────────────────────────────┐
│ robson-go (Go CLI)                              │
│ - Parses flags                                  │
│ - Builds Django command                         │
│ - Invokes: python manage.py validate_plan      │
└────────────────┬────────────────────────────────┘
                 │
                 ▼
┌─────────────────────────────────────────────────┐
│ Django Management Command                       │
│ - Loads strategy (if provided)                  │
│ - Builds validation context                     │
│ - Creates ValidatePlanUseCase                   │
└────────────────┬────────────────────────────────┘
                 │
                 ▼
┌─────────────────────────────────────────────────┐
│ ValidatePlanUseCase                             │
│ - Runs TenantIsolationValidator                 │
│ - Runs RiskConfigurationValidator               │
│ - Runs OperationValidator                       │
│ - Aggregates results                            │
└────────────────┬────────────────────────────────┘
                 │
                 ▼
┌─────────────────────────────────────────────────┐
│ ValidationReport                                │
│ - status: PASS/FAIL/WARNING                     │
│ - issues: List of ValidationIssue               │
│ - metadata: Context information                 │
└────────────────┬────────────────────────────────┘
                 │
                 ▼
┌─────────────────────────────────────────────────┐
│ Output (Human-Readable or JSON)                 │
│ - Clear PASS/FAIL result                        │
│ - Enumerated issues                             │
│ - Actionable recommendations                    │
└─────────────────────────────────────────────────┘
```

---

## Usage Examples

### Example 1: Valid Plan with Strategy

```bash
robson validate abc123 --client-id 1 --strategy-id 5
```

**Output**:
```
============================================================
VALIDATION REPORT
============================================================

✓ STATUS: PASS

Context:
  plan_id: abc123
  client_id: 1

Summary:
  Total issues: 0
  Failures: 0
  Warnings: 0

✓ SAFE TO EXECUTE

============================================================
```

**Exit Code**: 0

### Example 2: Missing Client ID (FAIL)

```bash
robson validate abc123
# Error: required flag(s) "client-id" not set
```

**Why**: Client ID is MANDATORY for tenant isolation

### Example 3: Invalid Risk Configuration (FAIL)

```bash
robson validate abc123 --client-id 1 --strategy-id 999
# (Strategy 999 has no risk_config)
```

**Output**:
```
============================================================
VALIDATION REPORT
============================================================

✗ STATUS: FAIL

Context:
  plan_id: abc123
  client_id: 1

FAILURES:
  1. [RISK_CONFIG_MISSING] risk_config is required for live execution
     requirement: Define max_drawdown, stop_loss_percent, max_position_size

Summary:
  Total issues: 1
  Failures: 1
  Warnings: 0

⚠️  EXECUTION BLOCKED
   Fix the failures above before executing.

============================================================
```

**Exit Code**: 1

### Example 4: High Risk Configuration (WARNING)

```bash
# Strategy with:
# - max_drawdown_percent: 60
# - stop_loss_percent: 15

robson validate abc123 --client-id 1 --strategy-id 10
```

**Output**:
```
============================================================
VALIDATION REPORT
============================================================

⚠ STATUS: WARNING (validation passed with concerns)

Context:
  plan_id: abc123
  client_id: 1

WARNINGS:
  1. [MAX_DRAWDOWN_HIGH] max_drawdown_percent is very high: 60%
     provided: 60
     recommendation: Consider limiting drawdown to 20-30%
  2. [STOP_LOSS_HIGH] stop_loss_percent is very high: 15%
     provided: 15
     recommendation: Consider limiting stop-loss to 2-5% per trade

Summary:
  Total issues: 2
  Failures: 0
  Warnings: 2

⚠️  PROCEED WITH CAUTION
   Review warnings before executing.

============================================================
```

**Exit Code**: 0 (warnings don't block execution, but alert user)

### Example 5: JSON Output

```bash
robson validate abc123 --client-id 1 --strategy-id 5 --json
```

**Output**:
```json
{
  "status": "PASS",
  "issues": [],
  "metadata": {
    "plan_id": "abc123",
    "client_id": "1",
    "strategy_name": "Conservative BTC Strategy"
  },
  "summary": {
    "total_issues": 0,
    "failures": 0,
    "warnings": 0
  }
}
```

### Example 6: Manual Operation Validation

```bash
robson validate manual-001 \
  --client-id 1 \
  --operation-type buy \
  --symbol BTCUSDT \
  --quantity 0.001 \
  --price 50000
```

**Output**: Validates operation parameters without loading from strategy

---

## Validation Rules Summary

### Tenant Isolation

| Rule | Severity | Code | Description |
|------|----------|------|-------------|
| client_id present | FAIL | TENANT_MISSING | Client ID required |
| client_id > 0 | FAIL | TENANT_INVALID | Must be positive |
| client_id is int | FAIL | TENANT_INVALID_FORMAT | Must be integer |

### Risk Configuration

| Rule | Severity | Code | Description |
|------|----------|------|-------------|
| risk_config exists | FAIL | RISK_CONFIG_MISSING | Risk config required |
| max_drawdown defined | FAIL | MAX_DRAWDOWN_MISSING | Max drawdown required |
| max_drawdown 0-100 | FAIL | MAX_DRAWDOWN_INVALID | Must be 0-100% |
| max_drawdown <= 50 | WARNING | MAX_DRAWDOWN_HIGH | Very high drawdown |
| stop_loss defined | FAIL | STOP_LOSS_MISSING | Stop loss required |
| stop_loss 0-100 | FAIL | STOP_LOSS_INVALID | Must be 0-100% |
| stop_loss <= 10 | WARNING | STOP_LOSS_HIGH | Very high stop loss |
| max_position defined | WARNING | MAX_POSITION_MISSING | Position size recommended |
| max_position 0-100 | FAIL | MAX_POSITION_INVALID | Must be 0-100% |
| max_position <= 50 | WARNING | MAX_POSITION_HIGH | Very large position |

### Operation Parameters

| Rule | Severity | Code | Description |
|------|----------|------|-------------|
| type specified | FAIL | OPERATION_TYPE_MISSING | Type required |
| symbol present | FAIL | SYMBOL_MISSING | Symbol required |
| quantity present | FAIL | QUANTITY_MISSING | Quantity required |
| quantity > 0 | FAIL | QUANTITY_INVALID | Must be positive |
| quantity numeric | FAIL | QUANTITY_INVALID_FORMAT | Must be numeric |
| price > 0 | FAIL | PRICE_INVALID | Must be positive |
| price numeric | FAIL | PRICE_INVALID_FORMAT | Must be numeric |

---

## Testing

### Run Validation Tests

```bash
cd apps/backend/monolith
python manage.py test api.tests.test_validation -v 2
```

**Expected Output**:
```
test_adding_failure_changes_status ... ok
test_adding_warning_changes_status ... ok
test_complete_buy_passes ... ok
test_complete_risk_config_passes ... ok
test_complete_valid_plan_passes ... ok
test_empty_report_passes ... ok
...
----------------------------------------------------------------------
Ran 30 tests in 0.123s

OK
```

### Test Coverage

```bash
# All validation tests
python manage.py test api.tests.test_validation

# Specific test class
python manage.py test api.tests.test_validation.TenantIsolationValidatorTests

# Specific test
python manage.py test api.tests.test_validation.TenantIsolationValidatorTests.test_valid_client_id_passes
```

---

## Integration with Agentic Workflow

### Complete Workflow

```bash
# STEP 1: PLAN
robson plan buy BTCUSDT 0.001 --limit 50000
# Output: Plan ID: abc123def456

# STEP 2: VALIDATE (Paper Trading)
robson validate abc123def456 --client-id 1 --strategy-id 5

# If PASS → safe to execute
# If FAIL → fix issues and re-validate
# If WARNING → review and decide

# STEP 3: EXECUTE (only if validated)
robson execute abc123def456
```

### Validation as a Gate

```mermaid
PLAN → VALIDATE → EXECUTE
        ↓
      PASS? ──Yes→ EXECUTE
        ↓
       No
        ↓
    BLOCKED
```

**Key Principle**: You CANNOT execute without validation

---

## Benefits

### 1. Risk Mitigation
- ✅ Catches errors before they cost money
- ✅ Enforces risk limits
- ✅ Blocks unsafe operations

### 2. Tenant Isolation
- ✅ Mandatory client_id prevents cross-tenant contamination
- ✅ All operations explicitly scoped
- ✅ Multi-tenant security enforced

### 3. Operational Confidence
- ✅ Clear PASS/FAIL/WARNING status
- ✅ Enumerated, actionable issues
- ✅ Human-readable explanations

### 4. Automation-Friendly
- ✅ JSON output for agents
- ✅ Exit codes for scripting
- ✅ Machine-parseable errors

### 5. Paper Trading Analogy
- ✅ Validates without execution
- ✅ Reduces uncertainty
- ✅ Builds confidence

---

## Future Enhancements

### Short-term
- [ ] Add balance validation (check available capital)
- [ ] Add market data validation (check symbol exists, price reasonable)
- [ ] Add position limit validation (max open positions)
- [ ] Persist validation results for audit

### Long-term
- [ ] Historical validation metrics
- [ ] Validation failure analytics
- [ ] Custom validation rules per strategy
- [ ] Machine learning for anomaly detection

---

## Code Statistics

### Files Created

| File | Lines | Purpose |
|------|-------|---------|
| `api/application/validation.py` | ~650 | Validation framework |
| `api/management/commands/validate_plan.py` | ~130 | Django command |
| `api/tests/test_validation.py` | ~400 | Validation tests |

**Total New Code**: ~1,180 lines

### Files Modified

| File | Lines Changed | Purpose |
|------|---------------|---------|
| `api/application/__init__.py` | +15 | Exports |
| `cli/cmd/agentic.go` | +110 | Validate command |

**Total Modified**: ~125 lines

---

## Validation Checklist

- [x] ValidationReport with PASS/FAIL/WARNING
- [x] TenantIsolationValidator (client_id mandatory)
- [x] RiskConfigurationValidator (drawdown, stop-loss, position sizing)
- [x] OperationValidator (symbol, quantity, price)
- [x] ValidatePlanUseCase (orchestration)
- [x] Django management command
- [x] robson-go integration
- [x] Human-readable output
- [x] JSON output
- [x] Comprehensive tests
- [x] Documentation

---

## Summary

**Objective**: Implement business and risk validation
**Status**: ✅ **COMPLETE**

**Key Achievements**:
- Implemented complete validation framework
- Enforced tenant isolation (client_id mandatory)
- Validated risk configuration (drawdown, stop-loss, sizing)
- Created Django management command
- Integrated with robson-go CLI
- Added comprehensive tests
- Clear PASS/FAIL/WARNING with actionable feedback

**Principle Demonstrated**: Validation is the "paper trading" of the agentic workflow - validate correctness and risk WITHOUT production impact.

**Files Created**: 3
**Files Modified**: 2
**Tests**: 30+
**Exit Codes**: 0 (pass), 1 (fail)

**No git commits created** (per instructions).
All changes ready for manual review in Cursor.

---

**Last Updated**: 2025-12-14
**Prompt**: 03 of 04
**Status**: COMPLETE
