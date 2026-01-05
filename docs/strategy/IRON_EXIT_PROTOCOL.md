# Iron Exit Protocol

## Summary

Iron Exit Protocol is a **short-only isolated margin strategy** that enforces
Robson's core risk rule: **position size is derived from a technical stop**.

Key properties:
- Account type: **isolated margin**
- Side: **SELL (short)**
- Technical stop: **15m chart, level 2 (second resistance)**
- Risk limit: **max 1% of capital**
- Stop execution: **Robson internal monitor, market order**

This strategy is designed for **robust execution over precision**:
the stop is a **market exit triggered by Robson** (not a pre-placed
exchange stop-limit). Slippage is acceptable; missed exits are not.

---

## Strategy Contract

**User input**
- Symbol (e.g., BTCUSDC)
- Strategy = Iron Exit Protocol

**Robson responsibilities**
1. Determine side (SELL) from strategy bias.
2. Calculate technical stop from 15m chart (level 2 resistance).
3. Compute position size so max loss <= 1% of capital.
4. Inspect wallet + isolated margin balances.
5. Transfer collateral from Spot to Isolated Margin if needed.
6. Borrow base asset (BTC) and place MARKET SELL on isolated margin.
7. Create Operation + Movement records.
8. Monitor stop internally and execute MARKET BUY at stop trigger.

---

## Stop Execution Policy (CRITICAL)

Iron Exit Protocol **does not place STOP_LOSS_LIMIT orders on Binance**.

Instead:
- The stop price is stored on the Operation.
- Robson's stop monitor detects stop trigger.
- Robson executes a **market order** to close the position.

This is the canonical behavior for this strategy.

---

## Configuration (Strategy.config)

```json
{
  "account_type": "isolated_margin",
  "capital_mode": "balance",
  "capital_balance_percent": "100",
  "technical_stop": {
    "timeframe": "15m",
    "level": 2,
    "side": "SELL"
  },
  "stop_execution": "robson_market",
  "risk_percent": 1.0
}
```

**Note**: `capital_mode="balance"` uses isolated margin equity for the symbol.

---

## Execution Trace (Strategy -> Operation -> Movement)

```
Strategy: Iron Exit Protocol
  -> Operation (short BTCUSDC)
       -> Movement: TRANSFER_SPOT_TO_ISOLATED (if needed)
       -> Movement: MARGIN_BORROW (borrow BTC)
       -> Movement: MARGIN_SELL (entry at market)
       -> Movement: STOP_LOSS_PLACED (internal, no Binance order)
       -> Movement: STOP_LOSS_TRIGGERED (internal)
       -> Movement: MARGIN_BUY (market close with AUTO_REPAY)
```

---

## References

- docs/architecture/TRANSACTION-HIERARCHY.md
- docs/requirements/POSITION-SIZING-GOLDEN-RULE.md
- apps/backend/monolith/api/application/stop_monitor.py
