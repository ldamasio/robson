# Production Trading Guide

This document describes how to enable and use Robson's production trading features.

## Overview

After 6+ years of development, Robson is finally ready for production trading! This guide covers:

1. Configuration for production trading
2. API endpoints for executing trades
3. P&L tracking and performance monitoring
4. Multi-tenant credential management

## Configuration

### Environment Variables

Set these in your K8s deployment or `.env` file:

```bash
# Enable production trading
BINANCE_USE_TESTNET=False
TRADING_ENABLED=True

# Production Binance credentials (from K8s secrets)
RBS_BINANCE_API_KEY_PROD=your_api_key
RBS_BINANCE_SECRET_KEY_PROD=your_secret_key

# Testnet credentials (for development)
RBS_BINANCE_API_KEY_TEST=testnet_api_key
RBS_BINANCE_SECRET_KEY_TEST=testnet_secret_key

# Optional: Encryption key for client credentials in database
RBS_CREDENTIAL_ENCRYPTION_KEY=your_32_byte_key
```

### K8s Deployment

The production deployment at `infra/k8s/prod/rbs-backend-monolith-prod-deploy.yml` includes:

```yaml
- name: BINANCE_USE_TESTNET
  value: "False"
- name: TRADING_ENABLED
  value: "True"
```

## Multi-Tenant Architecture

### Admin/Operator Credentials (K8s Secrets)

Your credentials stored in K8s secrets (`rbs-django-secret`) are used for:

- System-level operations
- Your personal trading account (as the platform operator)
- Fallback when client credentials are not configured

### Client Credentials (Database)

Each `Client` (tenant) can have their own Binance credentials:

```python
from clients.models import Client

client = Client.objects.get(email='user@example.com')

# Set encrypted credentials
client.set_credentials(
    api_key='client_api_key',
    secret_key='client_secret_key'
)
client.save()

# Retrieve decrypted credentials
api_key = client.get_api_key()
secret_key = client.get_secret_key()

# Check if credentials are configured
if client.has_credentials():
    print(f"Client has credentials: {client.masked_api_key}")
```

## API Endpoints

### Trading Status

Check current trading configuration:

```http
GET /api/trade/status/
Authorization: Bearer <token>
```

Response:

```json
{
    "trading_enabled": true,
    "environment": "production",
    "has_credentials": true,
    "can_trade": true,
    "user": "admin",
    "timestamp": "2025-12-21T12:00:00Z"
}
```

### Account Balance

Get current account balance:

```http
GET /api/trade/balance/?asset=USDC
Authorization: Bearer <token>
```

Response:

```json
{
    "success": true,
    "environment": "production",
    "data": {
        "asset": "USDC",
        "free": "30.00000000",
        "locked": "0.00000000"
    }
}
```

### Buy BTC (First Production Trade!)

Execute a market buy order for BTC:

```http
POST /api/trade/buy-btc/
Authorization: Bearer <token>
Content-Type: application/json

{
    "amount": 30.0  // Optional: USDC amount to spend (defaults to all available)
}
```

Or specify exact BTC quantity:

```http
POST /api/trade/buy-btc/
Authorization: Bearer <token>
Content-Type: application/json

{
    "quantity": 0.0003  // BTC quantity to buy
}
```

Response:

```json
{
    "success": true,
    "message": "🎉 Historic first production trade executed!",
    "environment": "production",
    "trade": {
        "id": 1,
        "symbol": "BTCUSDC",
        "side": "BUY",
        "quantity": "0.00030000",
        "price": "98500.00000000",
        "total_cost": "29.55000000",
        "fee": "0.02955000",
        "binance_order_id": "12345678",
        "timestamp": "2025-12-21T12:00:00Z"
    }
}
```

### Sell BTC

Execute a market sell order:

```http
POST /api/trade/sell-btc/
Authorization: Bearer <token>
Content-Type: application/json

{
    "quantity": 0.0003  // Optional: defaults to all available BTC
}
```

### Trade History

Get trade history with P&L:

```http
GET /api/trade/history/?days=30&limit=100
Authorization: Bearer <token>
```

Response:

```json
{
    "trades": [
        {
            "id": 1,
            "symbol": "BTCUSDC",
            "side": "BUY",
            "quantity": "0.00030000",
            "entry_price": "98500.00000000",
            "exit_price": "99000.00000000",
            "pnl": "0.15000000",
            "pnl_percentage": "0.51",
            "is_winner": true,
            "is_closed": true,
            "duration_hours": 4.5
        }
    ],
    "summary": {
        "total_trades": 10,
        "closed_trades": 8,
        "open_trades": 2,
        "total_pnl": "1.25000000",
        "winners": 6,
        "losers": 2,
        "win_rate": "75.0%"
    }
}
```

### P&L Summary

Get P&L summary by period:

```http
GET /api/trade/pnl/?period=monthly&months=12
Authorization: Bearer <token>
```

Response:

```json
{
    "period_type": "monthly",
    "periods": [
        {
            "period": "2025-12",
            "pnl": "1.25000000",
            "cumulative_pnl": "1.25000000",
            "trades": 10,
            "winners": 6,
            "losers": 4,
            "win_rate": "60.0%",
            "volume": "500.00000000"
        }
    ],
    "overall": {
        "total_pnl": "1.25000000",
        "total_trades": 10,
        "winners": 6,
        "losers": 4,
        "win_rate": "60.0%"
    }
}
```

## Database Migrations

Run these migrations before deploying:

```bash
cd apps/backend/monolith
python manage.py migrate api 0009_order_binance_order_id
python manage.py migrate clients 0002_client_is_active
```

## Security Considerations

### API Key IP Restrictions

The production Binance API key is **restricted to Contabo VPS IPs only**:

| Server | IP Address | Role |
|--------|------------|------|
| tiger | 158.220.116.31 | K3s Master |
| bengal | 164.68.96.68 | K3s Agent |
| pantera | 149.102.139.33 | K3s Agent |
| eagle | 167.86.92.97 | K3s Agent |

⚠️ **IMPORTANT**: Trading commands can ONLY be executed from within the K8s cluster.
Local development machines cannot execute real trades (API will return `code=-2015`).

To execute trades in production:

```bash
# SSH to master node
ssh root@158.220.116.31

# Execute command in backend pod
kubectl exec -n robson <pod-name> -- python manage.py <command>
```

### Additional Security Measures

1. **Credentials Encryption**: Client credentials are encrypted using Fernet symmetric encryption
2. **API Key Permissions**: Use Binance API keys with only spot trading permissions (no withdrawals)
3. **Rate Limiting**: The API has built-in rate limiting (1000 requests/hour for authenticated users)
4. **Multi-Tenant Isolation**: Each client's trades are isolated by tenant
5. **IP Whitelist**: Production API keys are restricted to Contabo VPS IPs only

## ⚠️ MANDATORY: Risk Management Rules

### The 1% Rule (NON-NEGOTIABLE)

**No trade may be executed without proper risk management.**

Every order MUST include:

1. **Stop-Loss Price**: Where the trade is invalidated
2. **Entry Price**: Planned entry point
3. **Position Size**: Calculated to risk maximum 1% of capital

### Position Sizing Formula

```
Risk Amount = Total Capital × 1%
Stop Distance = |Entry Price - Stop Price|
Position Size = Risk Amount / Stop Distance
```

**Example:**

- Capital: $1,000
- Risk Amount: $10 (1%)
- Entry: $100,000
- Stop: $98,000
- Stop Distance: $2,000
- Position Size: $10 / $2,000 = 0.005 BTC

### Monthly Drawdown Limit (4% Rule)

If monthly losses exceed 4% of capital:

- Trading is automatically PAUSED
- Review required before resuming
- PolicyState tracks this in database

### Enforcement

The system BLOCKS orders that:

- ❌ Have no stop-loss defined
- ❌ Risk more than 1% of capital
- ❌ Exceed monthly drawdown limit
- ❌ Skip the PLAN → VALIDATE → EXECUTE workflow

Use these endpoints for risk-managed trading:

- `POST /api/margin/position/calculate/` - Calculate safe position size
- `POST /api/margin/position/open/` - Open with automatic stop-loss
- `POST /api/guard/analyze/` - Check for emotional trading patterns

## Troubleshooting

### Trading Disabled Error

```json
{"success": false, "error": "Trading is disabled. Set TRADING_ENABLED=True to enable."}
```

Solution: Set `TRADING_ENABLED=True` in environment variables.

### No Credentials Error

```json
{"success": false, "error": "Binance API credentials not configured for production mode"}
```

Solution: Ensure `RBS_BINANCE_API_KEY_PROD` and `RBS_BINANCE_SECRET_KEY_PROD` are set.

### Insufficient Balance

```json
{"success": false, "error": "No USDC available for trading"}
```

Solution: Ensure your Binance account has sufficient USDC balance.

## First Trade Celebration! 🎉

After 6+ years of development, when you execute your first production trade, Robson will return:

```json
{
    "success": true,
    "message": "🎉 Historic first production trade executed!",
    ...
}
```

Congratulations on this milestone!

---

## Stop Monitoring & Trailing Stops

### CronJobs Architecture

Two separate CronJobs handle stop management in production:

| CronJob | Schedule | Purpose |
|---------|----------|---------|
| `rbs-stop-monitor-cronjob` | Every minute | Execute stop-loss and take-profit orders |
| `rbs-trailing-stop-cronjob` | Every minute | Adjust trailing stop prices |

### Stop Monitor CronJob

**File**: `infra/k8s/prod/rbs-stop-monitor-cronjob.yml`

Monitors open positions and executes:

- **Stop-Loss**: When price drops below stop_price (for LONG positions)
- **Take-Profit**: When price reaches target_price

**Command**: `python manage.py monitor_stops`

> **Note**: As of 2025-12-29, the `--dry-run` flag was removed and the CronJob now executes real orders.

### Trailing Stop CronJob

**File**: `infra/k8s/prod/rbs-trailing-stop-cronjob.yml`

Implements the **Hand-Span Trailing Stop** algorithm:

1. **Span Definition**: Distance between entry price and initial stop-loss
2. **Break-Even at 1 Span**: When profit equals 1 span, move stop to entry price
3. **Trail at 2+ Spans**: Move stop by (spans - 1) × span distance
4. **Monotonic**: Stop only moves in favorable direction (never retreats)

**Example** (LONG position):

- Entry: $100
- Initial Stop: $98 (span = $2)
- Price reaches $102 (1 span profit) → Stop moves to $100 (break-even)
- Price reaches $104 (2 spans profit) → Stop moves to $102
- Price reaches $106 (3 spans profit) → Stop moves to $104

**Command**: `python manage.py adjust_trailing_stops`

### Applying CronJobs

```bash
# From the master node
kubectl apply -f infra/k8s/prod/rbs-stop-monitor-cronjob.yml
kubectl apply -f infra/k8s/prod/rbs-trailing-stop-cronjob.yml

# Verify
kubectl get cronjobs -n robson
```

---

## AI Chat Assistant (Robson AI)

### Overview

Robson includes an AI-powered chat assistant for conversational trading assistance. The assistant can:

- Provide technical analysis
- Check account balances and positions
- Calculate position sizes
- Execute trades (with confirmation)
- Offer risk management advice

### API Endpoints

**Chat with Robson**:

```http
POST /api/chat/
Authorization: Bearer <token>
Content-Type: application/json

{
    "message": "What is my current BTC position?",
    "conversation_id": null  // Optional: for conversation continuity
}
```

Response:

```json
{
    "success": true,
    "message": "You have an open LONG position...",
    "conversation_id": "uuid-here",
    "detected_intent": "positions",
    "requires_confirmation": false,
    "model": "llama3-8b-8192"
}
```

**Check AI Status**:

```http
GET /api/chat/status/
Authorization: Bearer <token>
```

Response:

```json
{
    "available": true,
    "model": "llama3-8b-8192",
    "provider": "Groq"
}
```

**Get Trading Context**:

```http
GET /api/chat/context/
Authorization: Bearer <token>
```

### Configuration

**Environment Variables**:

```bash
# Groq API key (from Kubernetes secret)
GROQ_API_KEY=gsk_xxx...

# Optional: Override default model
ROBSON_AI_MODEL=llama3-8b-8192

# Optional: Max tokens per response
ROBSON_AI_MAX_TOKENS=4096
```

**Kubernetes Secret**:

```bash
kubectl create secret generic rbs-groq-secret \
  --from-literal=GROQ_API_KEY=your_key_here \
  -n robson
```

### Supported Models

Currently using Groq's free tier:

- `llama3-8b-8192` (default)
- `llama3-70b-8192`
- `mixtral-8x7b-32768`

**Future**: Multi-cloud support (OpenAI, Anthropic, DeepSeek) with user-configurable API keys.

### Frontend Component

The chat is available as a floating component on the logged-in dashboard:

- Click the 🤖 button to open
- Quick actions for common queries
- Full conversation history
