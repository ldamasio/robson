# Isolated Margin SHORT (BTCUSDC) - Operation Report

**Date**: 2026-01-05 01:25 UTC  
**Operator**: psyctl via Robson Bot (production, kubectl exec)  
**Environment**: Production (Binance)  
**Status**: SUCCESS  

---

## Executive Summary

Executed a **SHORT** position on **BTCUSDC** in Isolated Margin using a **technical stop** on the 15m chart. The stop calculation did not find the second resistance level, so the system **fell back to swing point**. Position sizing was calculated after the technical stop to cap loss at **1% of capital ($0.30)**.

The command was executed on production and recorded in the database and audit trail.

---

## Position Details

| Parameter | Value |
|-----------|-------|
| **Symbol** | BTCUSDC |
| **Side** | SHORT |
| **Entry Price** | $92,837.01 |
| **Stop-Loss** | $93,058.20 |
| **Stop Distance** | $221.18701 (0.24%) |
| **Quantity** | 0.00135 BTC |
| **Position Value** | $125.33 |
| **Risk Amount** | $0.30 |
| **Risk Percent** | 1.00% |
| **Method** | swing_point (fallback) |
| **Confidence** | medium |

---

## Golden Rule Application

```
Capital: $30
Max Risk: 1% = $0.30
Stop Distance: $221.18701

Position Size = Max Risk / Stop Distance
             = 0.30 / 221.18701
             = 0.00135 BTC
```

---

## Execution Command

```bash
kubectl exec -n robson deploy/rbs-backend-monolith-prod-deploy -- \
  python manage.py isolated_margin_sell --capital 30 --symbol BTCUSDC --client-id 1 --live --confirm
```

---

## Execution Flow (Live)

### Step 1: Transfer Collateral to Isolated Margin
- Spot USDC was 0, so no transfer occurred.
- Existing isolated BTCUSDC collateral was used.

### Step 2: Borrow BTC
- **Borrowed**: 0.00135 BTC  
- **Transaction ID**: 340070951728

### Step 3: Place Entry Order (MARKET)
- **Order ID**: 7720925530  
- **Fill Price**: $92,837.01

### Step 4: Place Stop-Loss (STOP_LOSS_LIMIT, BUY)
- **Stop Order ID**: 7720925586  
- **Stop Price**: $93,058.19701

### Step 5: Record Position (Database)
- **Position ID**: 2  
- **DB Record**: c12443b3-e37c-41c8-9a30-c425d112fd04

### Step 6: Record to Audit Trail
Recorded:
- MARGIN_BORROW  
- MARGIN_SELL  
- STOP_LOSS_PLACED

---

## Technical Stop Notes

The system attempted to use the 2nd resistance level on the 15m chart, but found **0 levels** and fell back to **swing point**:

```
Warning: Only 0 levels found, need 2. Trying swing points.
Method: swing_point
Confidence: medium
```

---

## Prior Attempts (Root Cause)

Initial attempts used **BTCUSDT**, which failed with:

```
APIError(code=-11008): Exceeding the account's maximum borrowable limit.
```

Reason: **no USDT collateral existed in the BTCUSDT isolated margin account**.  
Switching to **BTCUSDC** (which had collateral) resolved the issue.

---

## Binance References

- Borrow Transaction: `340070951728`
- Entry Order: `7720925530`
- Stop Order: `7720925586`
- Position UUID: `c12443b3-e37c-41c8-9a30-c425d112fd04`

---

## Verification Commands

```bash
python manage.py positions --client-id 1 --symbol BTCUSDC
python manage.py operations --client-id 1 --open
```

---

## Recovery Steps (if needed)

```python
# 1) Cancel stop order
exec.client.cancel_margin_order(symbol="BTCUSDC", orderId="7720925586", isIsolated="TRUE")

# 2) Buy to close
exec.client.create_margin_order(
    symbol="BTCUSDC",
    side="BUY",
    type="MARKET",
    quantity="0.00135",
    isIsolated="TRUE",
    sideEffectType="AUTO_REPAY",
)
```

---

*This document records a production operation for audit and AI-first traceability.*
