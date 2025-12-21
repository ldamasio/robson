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

1. **Credentials Encryption**: Client credentials are encrypted using Fernet symmetric encryption
2. **API Key Permissions**: Use Binance API keys with only spot trading permissions (no withdrawals)
3. **Rate Limiting**: The API has built-in rate limiting (1000 requests/hour for authenticated users)
4. **Multi-Tenant Isolation**: Each client's trades are isolated by tenant

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

