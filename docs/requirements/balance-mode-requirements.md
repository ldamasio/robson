# BALANCE Mode Requirements

## Overview

BALANCE mode is a capital calculation feature that derives trade capital from the user's available exchange balance (SPOT) rather than using a fixed amount. This enables dynamic position sizing based on actual account equity.

**Scope**: SPOT accounts only (does NOT support margin or isolated margin accounts yet).

**Location**: `api/application/use_cases/auto_calculate_trading_parameters.py`

---

## 1. What BALANCE Mode Is

### Definition

BALANCE mode calculates trade capital as a percentage of the user's **available (free) quote asset balance** on the exchange:

```
capital = available_quote_balance × capital_balance_percent
```

Where:
- `available_quote_balance` comes from Binance SPOT account (e.g., free USDT)
- `capital_balance_percent` is configured in Strategy.config (0-100, clamped)
- `capital_source` in response is set to `"BALANCE"` for audit trail

### Canonical Quote Asset

The quote asset is sourced from `Symbol.quote_asset` (the canonical model attribute), not derived from the trading pair name.

**Example**: For symbol `BTCUSDT`, the quote asset is `USDT`.

### Determinism Guarantee

Preview quantity (from auto-calc endpoint) must equal persisted PLAN quantity exactly. Both are quantized to 8 decimal places using `_quantize_quantity()`.

---

## 2. How to Enable (Per-Strategy Configuration)

Configure BALANCE mode in `Strategy.config`:

```json
{
  "capital_mode": "balance",
  "capital_balance_percent": 50,
  "capital_fixed": "1000.00"
}
```

### Config Keys

| Key | Type | Required | Default | Description |
|-----|------|----------|---------|-------------|
| `capital_mode` | string | No | `"fixed"` | Capital calculation mode: `"fixed"` or `"balance"` (case-insensitive) |
| `capital_balance_percent` | number | No | `"100"` | Percentage of available balance to use (0-100) |
| `capital_fixed` | string | No | `"1000.00"` | Fallback capital when BALANCE mode fails |
| `account_type` | string | No | `"spot"` | Account type (only `"spot"` supported currently) |

### Validation

- `capital_balance_percent` is clamped to `[0, 100]` with warnings
- Invalid values (non-numeric, negative, >100) fall back to 100% with warning
- Values below 0 are clamped to 0% (no capital allocated)
- Values above 100 are clamped to 100%

---

## 3. Warnings Behavior

BALANCE mode uses **safe fallbacks** - intent creation always succeeds, even when balance fetch fails.

### Fallback Conditions

| Condition | Behavior | `capital_source` | Warning |
|-----------|----------|------------------|---------|
| Balance provider not configured | Use `capital_fixed` | `"FALLBACK"` | "Balance provider not configured..." |
| `client_id` not available | Use `capital_fixed` | `"FALLBACK"` | "client_id not available..." |
| Available balance <= 0 | Use `capital_fixed` | `"FALLBACK"` | "Available balance is <= 0..." |
| Exchange API timeout/connection error | Use `capital_fixed` | `"FALLBACK"` | "Exchange API timeout/connection error..." |
| Other API error | Use `capital_fixed` | `"FALLBACK"` | "Unable to fetch exchange balance..." |
| `capital_mode` unknown | Use `capital_fixed` | `"FALLBACK"` | "Unknown capital mode 'X'..." |

### Warning Format

Warnings are user-friendly, omitting sensitive details:
- "Your intent was created successfully; balance retrieval will retry."
- "Using fixed capital fallback."

### Combined Warnings

Stop warnings from technical stop calculation are merged into the response `warnings` list for unified display.

---

## 4. Contracts

### Use Case: `AutoCalculateTradingParametersUseCase`

**File**: `api/application/use_cases/auto_calculate_trading_parameters.py`

**Input**:
```python
execute(symbol_obj, strategy_obj, client_id: int | None = None) -> dict
```

**Output**:
```python
{
    "side": "BUY" | "SELL",
    "entry_price": Decimal,
    "stop_price": Decimal,
    "capital": Decimal,
    "capital_used": Decimal,          # Same as capital
    "capital_source": "FIXED" | "BALANCE" | "FALLBACK",
    "quantity": Decimal,              # Quantized to 8 decimals
    "risk_amount": Decimal,
    "position_value": Decimal,
    "timeframe": str,
    "method_used": str,
    "confidence": "HIGH" | "MEDIUM" | "LOW",
    "confidence_float": Decimal,      # 0.8, 0.6, or 0.4
    "side_source": str,
    "warnings": list[str],
    "stop_result": TechnicalStopResult
}
```

### Port: `AccountBalancePort`

**File**: `api/application/ports.py`

```python
class AccountBalancePort(Protocol):
    def get_available_quote_balance(
        self,
        client_id: int,
        quote_asset: str
    ) -> Decimal:
        """Get the available (free) balance for a quote asset."""
        ...
```

### Adapter: `BinanceAccountBalanceAdapter`

**File**: `api/application/adapters.py`

**Timeout**: 5 seconds (configurable)

**Implementation**:
- Uses `python-binance` `Client.get_account()`
- Filters by `quote_asset`
- Returns `free` balance (not total)

---

## 5. Safety Limits

### MAX_CAPITAL

**Value**: `Decimal("100000.00")` (100,000)

**Behavior**: When calculated capital exceeds MAX_CAPITAL:
- Capital is capped at MAX_CAPITAL
- Warning is added: "Available balance results in capital above maximum..."

**Purpose**: Prevent runaway capital allocation from very large balances.

### Minimum Capital Warning

When computed capital < $10 USDT:
- Warning is added: "Computed capital is below typical exchange minimum..."
- Trade may fail at execution with "Filter failure: MIN_NOTIONAL"

---

## 6. Known Limitations

1. **SPOT only**: Does not support margin or isolated margin accounts
2. **Single asset**: Uses one quote asset (e.g., USDT), not multi-asset portfolio
3. **No caching**: Balance is fetched on each intent creation (could be optimized)
4. **No historical tracking**: Balance snapshots are separate (see `AuditTransaction`, `BalanceSnapshot`)

---

## 7. Examples

### Example 1: Successful BALANCE Mode

**Strategy Config**:
```json
{
  "capital_mode": "balance",
  "capital_balance_percent": 25,
  "capital_fixed": "1000.00"
}
```

**User Balance**: 5000 USDT available

**Result**:
```
capital = 5000 × 0.25 = 1250 USDT
capital_source = "BALANCE"
warnings = []
```

---

### Example 2: Balance Fetch Timeout

**Strategy Config**:
```json
{
  "capital_mode": "balance",
  "capital_balance_percent": 100,
  "capital_fixed": "500.00"
}
```

**Event**: Binance API times out after 5 seconds

**Result**:
```
capital = 500.00 (from capital_fixed)
capital_source = "FALLBACK"
warnings = [
  "Exchange API timeout while fetching USDT balance. Using fixed capital fallback."
]
```

---

### Example 3: Invalid Balance Percent

**Strategy Config**:
```json
{
  "capital_mode": "balance",
  "capital_balance_percent": 150,  // Invalid: > 100
  "capital_fixed": "1000.00"
}
```

**User Balance**: 3000 USDT available

**Result**:
```
capital = 3000 × 1.0 = 3000 USDT  // Clamped to 100%
capital_source = "BALANCE"
warnings = [
  "capital_balance_percent cannot exceed 100% (got 150%). Using 100%."
]
```

---

### Example 4: Zero Balance Fallback

**Strategy Config**:
```json
{
  "capital_mode": "balance",
  "capital_balance_percent": 100,
  "capital_fixed": "200.00"
}
```

**User Balance**: 0 USDT available (empty account)

**Result**:
```
capital = 200.00 (fallback)
capital_source = "FALLBACK"
warnings = [
  "Available USDT balance is 0. Using fixed capital fallback."
]
```

---

## 8. Testing

### Unit Tests

**File**: `api/tests/test_trading_intent_api.py`

**Test Classes**:
- `TestBalanceMode` - 4 tests covering success, timeout, zero balance, invalid percent
- `TestP0Fixes` - 3 tests for confidence_float, warnings, quantity determinism

**Key Assertions**:
```python
# BALANCE mode succeeds
assert Decimal(response.data["capital"]) == expected_capital
assert response.data["capital_source"] == "BALANCE"

# Fallback on timeout
assert response.data["capital_source"] == "FALLBACK"
assert len(response.data["warnings"]) > 0

# Determinism: preview quantity = persisted quantity
assert preview_quantity == persisted_quantity
```

---

## 9. References

### Code

- **Use Case**: `api/application/use_cases/auto_calculate_trading_parameters.py`
- **Port**: `api/application/ports.py::AccountBalancePort`
- **Adapter**: `api/application/adapters.py::BinanceAccountBalanceAdapter`
- **View**: `api/views/trading_intent_views.py::auto_calculate_parameters`
- **Tests**: `api/tests/test_trading_intent_api.py::TestBalanceMode`

### Related Documents

- [Position Sizing Golden Rule](POSITION-SIZING-GOLDEN-RULE.md) - 1% risk rule
- [Strategy Semantic Clarity](STRATEGY-SEMANTIC-CLARITY.md) - User-driven strategy selection
- [Technical Stop Requirements](technical-stop-requirements.md) - Stop-loss derivation

---

**Maintained by**: Robson Bot Core Team
**Last Updated**: 2025-01-03
**Version**: 1.0
