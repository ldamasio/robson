# Robson Bot Strategies

**Built-in playful strategies for better trading UX.**

Robson comes with pre-defined strategies designed for different market conditions. Each strategy has a **playful, memorable name** to make trading more intuitive and less intimidating.

---

## 🎯 Available Strategies

### 1. **All In** 🚀

**Go all-in with technical stop precision.**

- **Type**: Manual entry
- **Timeframe**: 15m
- **Account**: Isolated Margin (3x leverage)
- **Entry Method**: Buy maximum position size with stop at second technical support
- **Risk**: 1% of capital per trade
- **Indicators**: Support/Resistance, Technical Stop

**What it does**: Calculates the optimal position size by working backwards from the technical invalidation level (2nd support on 15m chart). If your stop is far, you get a smaller position. If it's tight, you get a larger position. **Risk stays constant at 1%.**

**Use case**: When you've identified a high-conviction entry and want to maximize position size while keeping risk controlled.

**Command example**:
```bash
# Dry-run: Analyze technical stop and position size
python manage.py technical_stop_buy --capital 100 --strategy "All In"

# Live: Execute buy with technical stop
python manage.py technical_stop_buy --capital 100 --strategy "All In" --live --confirm

# With custom timeframe (4h chart instead of 15m)
python manage.py technical_stop_buy --capital 100 --strategy "All In" --timeframe 4h --live --confirm

# Use 3rd support level instead of 2nd
python manage.py technical_stop_buy --capital 100 --strategy "All In" --level-n 3 --live --confirm
```

**Example output**:
```
=== TECHNICAL STOP ANALYSIS ===
Symbol: BTCUSDC
Current Price: $95,432.10
Technical Stop: $93,500.00 (2nd support on 15m)
Stop Distance: $1,932.10 (2.02%)

=== POSITION SIZING ===
Capital: $100.00
Risk Amount (1%): $1.00
Position Size: 0.000517 BTC
Position Value: $49.35

=== EXECUTION ===
✅ BUY 0.000517 BTC @ $95,432.10
✅ STOP placed @ $93,500.00
✅ If stopped: Loss = $1.00 (exactly 1% of capital)
```

---

### 2. **Rescue Forces** 🛡️

**Automatic rescue on bullish momentum.**

- **Type**: Automated entry
- **Timeframe**: 15m
- **Account**: Isolated Margin (3x leverage)
- **Entry Conditions**:
  - MA4 crosses above MA9
  - Short-term uptrend confirmed
  - Volume spike confirmation (optional)
- **Risk**: 1% of capital per trade
- **Indicators**: MA4, MA9, Trend, Volume

**What it does**: Automatically enters a position when fast MA (4-period) crosses above slow MA (9-period) with a confirmed short-term bullish trend. The system monitors the market 24/7 and executes when conditions align.

**Use case**: For traders who want to catch early bullish momentum without watching charts all day. Perfect for scalping or short-term trend following.

**How it works**:
1. Pattern detector scans BTCUSDT on 15m timeframe
2. Detects MA4/MA9 crossover
3. Validates short-term uptrend
4. Checks volume spike (if enabled)
5. Calculates position size (1% risk, stop below MA9)
6. Executes buy order automatically
7. Places stop-loss below MA9

**Setup** (via Opportunity Detector):
1. Go to **Opportunity Detector** → **Strategy Configuration**
2. Click **New Configuration**
3. Select **Strategy**: "Rescue Forces"
4. Select **Pattern**: "MA Crossover" (or create custom)
5. Configure:
   - Auto-entry: Enabled
   - Min Confidence: 0.75
   - Timeframes: 15m
   - Symbols: BTCUSDT, ETHUSDT
6. Save

**Manual trigger** (for testing):
```bash
# Scan for Rescue Forces setups
python manage.py scan_patterns --strategy "Rescue Forces" --timeframe 15m

# Execute pending Rescue Forces signals
python manage.py detect_patterns --strategy "Rescue Forces" --auto-execute --live --confirm
```

---

### 3. **Smooth Sailing** ⛵

**Ride the calm waves of trending markets.**

- **Type**: Trend following
- **Timeframe**: 1h
- **Account**: Spot
- **Entry Conditions**: MA50 crosses above MA200 (Golden Cross)
- **Risk**: 0.5% of capital per trade
- **Indicators**: MA50, MA200

**What it does**: Classic long-term trend following. Enters when the 50-period MA crosses above the 200-period MA (Golden Cross), indicating a strong uptrend.

**Use case**: Conservative spot trading for long-term holders. Lower risk, lower frequency, higher win rate.

**Example**:
```bash
# Setup monitoring for Golden Cross on 1h charts
python manage.py detect_patterns --strategy "Smooth Sailing" --timeframe 1h --symbols BTCUSDT ETHUSDT
```

---

### 4. **Bounce Back** 🎾

**Catch the bounce when price returns to mean.**

- **Type**: Mean reversion
- **Timeframe**: 30m
- **Account**: Spot
- **Entry Conditions**:
  - Price touches lower Bollinger Band
  - RSI < 30 (oversold)
- **Risk**: 0.5% of capital per trade
- **Indicators**: Bollinger Bands, RSI

**What it does**: Buys when price is oversold (RSI < 30) and touching the lower Bollinger Band, expecting a bounce back to the mean.

**Use case**: Range-bound markets. Works best when there's no strong trend and price oscillates around a mean.

**Example**:
```bash
# Scan for Bounce Back opportunities
python manage.py scan_patterns --strategy "Bounce Back" --timeframe 30m
```

---

## 🔧 Strategy Configuration

Strategies are stored in the database with rich metadata:

```python
{
    "name": "All In",
    "description": "Go all-in with technical stop precision...",
    "config": {
        "timeframe": "15m",
        "indicators": ["Support/Resistance", "Technical Stop"],
        "entry_type": "manual",
        "risk_percent": 1.0,
        "use_technical_stop": True,
        "leverage": 3,
        "account_type": "isolated_margin"
    },
    "risk_config": {
        "max_risk_per_trade": 1.0,
        "use_technical_stop": True,
        "stop_placement": "second_support_15m"
    }
}
```

---

## 📊 How to Add Strategies to Production

### Option 1: Run the seeder (recommended)

```bash
# SSH into production pod
kubectl exec -it deployment/robson-backend -n robson -- bash

# Run seeder (creates strategies + sample data)
python manage.py seed_production_data
```

**Output**:
```
Created strategy: All In
Created strategy: Rescue Forces
Created strategy: Smooth Sailing
Created strategy: Bounce Back
Successfully seeded production-like data!
```

### Option 2: Django Admin

1. Go to `/admin/api/strategy/`
2. Click **Add Strategy**
3. Fill in:
   - Name: "All In"
   - Description: "Go all-in with technical stop precision..."
   - Config: `{"timeframe": "15m", "risk_percent": 1.0, ...}`
4. Save

### Option 3: Django Shell

```bash
python manage.py shell
```

```python
from api.models import Strategy
from clients.models import Client

client = Client.objects.get(id=1)

Strategy.objects.create(
    client=client,
    name="All In",
    description="Go all-in with technical stop precision...",
    config={
        "timeframe": "15m",
        "risk_percent": 1.0,
        "use_technical_stop": True
    },
    is_active=True
)
```

---

## 🎨 Why Playful Names?

Traditional strategy names like "SMA Crossover with RSI Filter" are:
- ❌ Intimidating for beginners
- ❌ Hard to remember
- ❌ Boring and clinical

Playful names like "Rescue Forces" and "All In":
- ✅ Memorable and fun
- ✅ Convey the strategy's personality
- ✅ Make trading less stressful
- ✅ Better UX for retail traders

---

## 🚀 Quick Start

1. **Seed strategies**:
   ```bash
   python manage.py seed_production_data
   ```

2. **Use "All In" strategy**:
   ```bash
   python manage.py technical_stop_buy --capital 100 --strategy "All In" --live --confirm
   ```

3. **Setup "Rescue Forces" auto-trading**:
   - Go to **Opportunity Detector** → **Strategy Configuration**
   - Create pattern config with "Rescue Forces" strategy
   - Enable auto-entry

4. **Monitor** in the frontend:
   - Dashboard → **Start New Operation** (select strategy)
   - Opportunity Detector → **Strategy Configuration** (manage auto-trading)

---

## 📚 Related Documentation

- [Position Sizing Golden Rule](requirements/POSITION-SIZING-GOLDEN-RULE.md)
- [Technical Stop Documentation](requirements/TECHNICAL-STOP.md)
- [Risk Management](RISK-MANAGEMENT.md)
- [Pattern Detection](PATTERN-DETECTION.md)

---

**Last Updated**: 2026-01-01
**Version**: 2.0 (Playful Strategy Names)
