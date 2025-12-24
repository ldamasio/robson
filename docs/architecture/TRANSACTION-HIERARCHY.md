# Robson Transaction Hierarchy

## Conceptual Clarity: From Strategy to Atomic Movement

This document establishes **crystal-clear definitions** of each abstraction level in Robson's trading system.

```
┌─────────────────────────────────────────────────────────────────────┐
│                         ROBSON ABSTRACTIONS                          │
│                    (Binance doesn't know these)                      │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  LEVEL 1: STRATEGY (Estratégia)                                     │
│  ══════════════════════════════                                      │
│  Definition: A trading algorithm or methodology that generates       │
│              signals and defines rules for entering/exiting trades.  │
│                                                                      │
│  Examples:                                                           │
│    - "Reversal on Support" (enter long at support levels)           │
│    - "Breakout Momentum" (enter on breakout with volume)            │
│    - "Mean Reversion RSI" (enter when RSI oversold)                 │
│                                                                      │
│  Database Model: api.Strategy                                        │
│  Contains: name, config (JSON), risk parameters, timeframe          │
│                                                                      │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  LEVEL 2: OPERATION (Operação)                                      │
│  ═════════════════════════════                                       │
│  Definition: A complete trade cycle - from entry to exit.           │
│              One Operation can contain MULTIPLE Movements.           │
│                                                                      │
│  Lifecycle:                                                          │
│    PLANNED → OPENED → MANAGING → CLOSED                             │
│                                                                      │
│  Example Operation "OP-2024-001":                                    │
│    - Entry: Buy 0.001 BTC @ $95,000                                 │
│    - Stop-Loss: Sell if price drops to $94,000                      │
│    - Take-Profit: Sell if price rises to $98,000                    │
│    - Exit: Sold 0.001 BTC @ $97,500 (profit)                        │
│                                                                      │
│  An Operation may involve:                                           │
│    - Multiple entries (scaling in)                                   │
│    - Multiple exits (scaling out)                                    │
│    - Transfers between accounts                                      │
│    - Borrowing (for margin)                                         │
│                                                                      │
│  Database Model: api.Operation                                       │
│  Contains: strategy_id, status, entry_orders[], exit_orders[]       │
│                                                                      │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  LEVEL 3: MOVEMENT (Movimentação) ← ATOMIC LEVEL                    │
│  ════════════════════════════════════════════════                    │
│  Definition: A single, atomic financial action that changes          │
│              account balances. This is what Binance knows.           │
│                                                                      │
│  CRITICAL: Movements are the ONLY thing that actually happens        │
│  on the exchange. Everything above is Robson's interpretation.       │
│                                                                      │
└─────────────────────────────────────────────────────────────────────┘
```

---

## LEVEL 3 DETAIL: Types of Movements (Movimentações)

### Category A: Trading Movements (Compra/Venda)

These change your asset holdings:

| Type | Account | Description | Example |
|------|---------|-------------|---------|
| `SPOT_BUY` | Spot | Buy asset in spot market | Buy 0.001 BTC with 95 USDC |
| `SPOT_SELL` | Spot | Sell asset in spot market | Sell 0.001 BTC for 97 USDC |
| `MARGIN_BUY` | Isolated Margin | Buy asset with borrowed funds | Buy 0.001 BTC in BTCUSDC margin |
| `MARGIN_SELL` | Isolated Margin | Sell asset in margin account | Sell 0.001 BTC in BTCUSDC margin |

### Category B: Transfer Movements (Transferências)

These move assets between accounts without trading:

| Type | From | To | Description |
|------|------|-----|-------------|
| `TRANSFER_SPOT_TO_ISOLATED` | Spot | Isolated Margin | Move collateral to margin account |
| `TRANSFER_ISOLATED_TO_SPOT` | Isolated Margin | Spot | Withdraw from margin to spot |

### Category C: Credit Movements (Crédito/Empréstimo)

These create or settle obligations:

| Type | Description | Example |
|------|-------------|---------|
| `MARGIN_BORROW` | Borrow asset from exchange | Borrow 100 USDC for leveraged position |
| `MARGIN_REPAY` | Repay borrowed amount | Repay 100 USDC + interest |
| `INTEREST_CHARGED` | Hourly interest on borrowed amount | -0.01 USDC interest charge |

### Category D: Order Lifecycle Movements

These represent order state changes:

| Type | Description |
|------|-------------|
| `STOP_LOSS_PLACED` | Stop-loss order created (pending) |
| `STOP_LOSS_TRIGGERED` | Stop-loss order executed (became a SELL) |
| `STOP_LOSS_CANCELLED` | Stop-loss order cancelled |
| `TAKE_PROFIT_PLACED` | Take-profit order created |
| `TAKE_PROFIT_TRIGGERED` | Take-profit order executed |
| `LIMIT_ORDER_PLACED` | Limit order created |
| `LIMIT_ORDER_FILLED` | Limit order executed |
| `LIMIT_ORDER_CANCELLED` | Limit order cancelled |

### Category E: Risk Events

| Type | Description |
|------|-------------|
| `LIQUIDATION` | Position forcibly closed due to insufficient margin |
| `MARGIN_CALL` | Warning that margin level is low |

### Category F: Fee Movements

| Type | Description |
|------|-------------|
| `TRADING_FEE` | Commission paid on trade execution |
| `FUNDING_FEE` | Periodic funding payment (futures) |

---

## Complete Example: One Operation, Many Movements

**Scenario**: User wants to open a 3x leveraged long position on BTC

### Strategy: "Leverage Breakout"
### Operation: OP-2024-12-24-001

```
┌─────────────────────────────────────────────────────────────────┐
│  OPERATION: OP-2024-12-24-001                                   │
│  Strategy: Leverage Breakout                                     │
│  Symbol: BTCUSDC                                                │
│  Side: LONG                                                     │
│  Target: 3x leverage                                            │
│  Capital: $30                                                   │
│  Entry: ~$95,000                                                │
│  Stop: $93,000 (2.1% below entry)                              │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│  MOVEMENTS (in chronological order):                            │
│                                                                  │
│  #1 TRANSFER_SPOT_TO_ISOLATED                                   │
│     Amount: 30 USDC                                             │
│     From: Spot Account                                          │
│     To: BTCUSDC Isolated Margin                                 │
│     Binance TxID: 337322126907                                  │
│     Purpose: Collateral for leveraged position                  │
│                                                                  │
│  #2 MARGIN_BORROW                                               │
│     Amount: 60 USDC                                             │
│     Account: BTCUSDC Isolated Margin                            │
│     Binance TxID: 337322126908                                  │
│     Purpose: Borrow to achieve 3x leverage ($30 + $60 = $90)    │
│                                                                  │
│  #3 MARGIN_BUY                                                  │
│     Quantity: 0.000947 BTC                                      │
│     Price: $95,000                                              │
│     Total: ~$90 USDC                                            │
│     Account: BTCUSDC Isolated Margin                            │
│     Binance OrderID: 7634794718                                 │
│     Purpose: Open long position                                 │
│                                                                  │
│  #4 TRADING_FEE                                                 │
│     Amount: 0.09 USDC (0.1% of $90)                            │
│     Account: BTCUSDC Isolated Margin                            │
│                                                                  │
│  #5 STOP_LOSS_PLACED                                            │
│     Type: STOP_LOSS_LIMIT                                       │
│     Quantity: 0.000947 BTC                                      │
│     Trigger Price: $93,000                                      │
│     Limit Price: $93,000                                        │
│     Account: BTCUSDC Isolated Margin                            │
│     Binance OrderID: 7634794756                                 │
│     Purpose: Risk management - limit loss to 1% of capital      │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

### Later: Closing the Operation

```
┌─────────────────────────────────────────────────────────────────┐
│  CLOSING OPERATION: OP-2024-12-24-001                           │
│  Outcome: PROFIT (price went up)                                │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│  #6 STOP_LOSS_CANCELLED                                         │
│     OrderID: 7634794756                                         │
│     Reason: Manual close (take profit)                          │
│                                                                  │
│  #7 MARGIN_SELL                                                 │
│     Quantity: 0.000947 BTC                                      │
│     Price: $98,000                                              │
│     Total: ~$92.80 USDC                                         │
│     Account: BTCUSDC Isolated Margin                            │
│     Binance OrderID: 7634799999                                 │
│     Profit: +$2.80 (before fees and interest)                   │
│                                                                  │
│  #8 TRADING_FEE                                                 │
│     Amount: 0.093 USDC                                          │
│                                                                  │
│  #9 INTEREST_CHARGED                                            │
│     Amount: 0.02 USDC (hourly rate × time held)                │
│     Asset: USDC                                                 │
│                                                                  │
│  #10 MARGIN_REPAY                                               │
│      Amount: 60 USDC                                            │
│      Purpose: Repay borrowed amount                             │
│                                                                  │
│  #11 TRANSFER_ISOLATED_TO_SPOT                                  │
│      Amount: 32.68 USDC                                         │
│      From: BTCUSDC Isolated Margin                              │
│      To: Spot Account                                           │
│      Note: Original $30 + $2.68 profit (after fees/interest)    │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

---

## Database Model Hierarchy

```
┌──────────────────┐
│     Strategy     │  "How to trade"
│   (1 strategy)   │
└────────┬─────────┘
         │ has many
         ▼
┌──────────────────┐
│    Operation     │  "One complete trade cycle"
│  (N operations)  │
└────────┬─────────┘
         │ has many
         ▼
┌──────────────────┐
│    Movement      │  "Atomic financial action"
│  (M movements)   │  ← This is what we audit!
└──────────────────┘
```

### Key Relationships:

1. **Strategy → Operation**: A strategy generates many operations over time
2. **Operation → Movement**: One operation consists of many movements
3. **Movement ↔ Binance**: Movements map 1:1 to Binance transactions

---

## What We Audit (AuditTransaction Table)

The `AuditTransaction` model captures EVERY Movement:

```python
class AuditTransaction(models.Model):
    # Identity
    transaction_id = CharField(unique=True)      # Robson's UUID
    binance_order_id = CharField(null=True)      # Binance's ID (if applicable)
    
    # What type of movement
    transaction_type = CharField(choices=[
        # Trading
        'SPOT_BUY', 'SPOT_SELL',
        'MARGIN_BUY', 'MARGIN_SELL',
        # Transfers
        'TRANSFER_SPOT_TO_ISOLATED',
        'TRANSFER_ISOLATED_TO_SPOT',
        # Credit
        'MARGIN_BORROW', 'MARGIN_REPAY',
        'INTEREST_CHARGED',
        # Orders
        'STOP_LOSS_PLACED', 'STOP_LOSS_TRIGGERED', 'STOP_LOSS_CANCELLED',
        'TAKE_PROFIT_PLACED', 'TAKE_PROFIT_TRIGGERED',
        # Risk
        'LIQUIDATION',
        # Fees
        'TRADING_FEE',
    ])
    
    # Which account
    account_type = CharField(choices=['SPOT', 'ISOLATED_MARGIN', 'CROSS_MARGIN'])
    
    # Details
    symbol = CharField()           # e.g., "BTCUSDC"
    asset = CharField()            # e.g., "BTC" or "USDC"
    quantity = DecimalField()      # Amount
    price = DecimalField(null=True)
    
    # Links to Robson abstractions
    operation = ForeignKey(Operation, null=True)  # Which operation this belongs to
    strategy = ForeignKey(Strategy, null=True)    # Which strategy (via operation)
    
    # Timestamps
    created_at = DateTimeField()   # When Robson recorded it
    executed_at = DateTimeField()  # When Binance executed it
```

---

## UI/UX Clarity

The frontend should show movements grouped by operation:

```
┌─────────────────────────────────────────────────────────────────┐
│  📊 OPERATION: OP-2024-12-24-001                                │
│  Strategy: Leverage Breakout | Status: OPEN | P&L: +$2.50      │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│  📋 MOVEMENTS:                                                   │
│                                                                  │
│  12:00:01  ↔️ TRANSFER    30 USDC      Spot → Isolated         │
│  12:00:02  💰 BORROW      60 USDC      Isolated Margin          │
│  12:00:03  🟢 MARGIN_BUY  0.000947 BTC @ $95,000                │
│  12:00:03  💸 FEE         0.09 USDC                             │
│  12:00:04  🛑 STOP_LOSS   Placed @ $93,000                      │
│                                                                  │
│  [▼ Show pending movements when closed]                         │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

---

## Summary: The Three Levels

| Level | Name | Who Knows | Example |
|-------|------|-----------|---------|
| **1** | Strategy | Robson only | "Reversal on Support" |
| **2** | Operation | Robson only | Entry + Manage + Exit cycle |
| **3** | Movement | Robson + Binance | Buy 0.001 BTC @ $95,000 |

**Key Insight**: 
- Binance only knows about **Movements**
- Robson adds meaning by grouping movements into **Operations**
- Operations are generated by **Strategies**

This hierarchy enables:
1. ✅ Complete audit trail (every movement recorded)
2. ✅ Performance analysis (P&L per operation, per strategy)
3. ✅ Regulatory compliance (can trace every dollar)
4. ✅ User transparency (understand what happened and why)

