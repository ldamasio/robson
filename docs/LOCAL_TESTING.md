# Local Development Testing Guide

Quick guide for testing Robson API locally.

## Prerequisites

1. PostgreSQL running on port 5450
2. Virtual environment activated
3. Migrations applied

## Quick Start

```bash
cd apps/backend/monolith
source .venv/Scripts/activate  # Windows Git Bash
# or: .venv\Scripts\activate   # Windows CMD
# or: source .venv/bin/activate # Linux/Mac

python manage.py runserver
```

## Authentication

### 1. Create a Test User (first time only)

```bash
python manage.py createsuperuser
# Username: admin
# Email: admin@robsonbot.com
# Password: admin123
```

Or via Python:
```bash
python manage.py shell -c "
from django.contrib.auth import get_user_model
User = get_user_model()
User.objects.create_superuser('admin', 'admin@robsonbot.com', 'admin123')
"
```

### 2. Get JWT Token

```bash
# Get token
curl -X POST http://localhost:8000/api/token/ \
  -H "Content-Type: application/json" \
  -d '{"username":"admin","password":"admin123"}'
```

Response:
```json
{
  "refresh": "eyJ...",
  "access": "eyJ..."
}
```

### 3. Use Token in Requests

```bash
# Save token to variable
TOKEN=$(curl -s -X POST http://localhost:8000/api/token/ \
  -H "Content-Type: application/json" \
  -d '{"username":"admin","password":"admin123"}' \
  | grep -o '"access":"[^"]*"' | cut -d'"' -f4)

# Use in requests
curl http://localhost:8000/api/trade/status/ \
  -H "Authorization: Bearer $TOKEN"
```

## Trading Endpoints

### Check Trading Status
```bash
curl http://localhost:8000/api/trade/status/ \
  -H "Authorization: Bearer $TOKEN"
```

### Get Account Balance
```bash
curl http://localhost:8000/api/trade/balance/ \
  -H "Authorization: Bearer $TOKEN"
```

### Buy BTC (requires TRADING_ENABLED=True)
```bash
curl -X POST http://localhost:8000/api/trade/buy-btc/ \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"amount": 30}'
```

### Trade History
```bash
curl "http://localhost:8000/api/trade/history/?days=30" \
  -H "Authorization: Bearer $TOKEN"
```

### P&L Summary
```bash
curl "http://localhost:8000/api/trade/pnl/?period=monthly" \
  -H "Authorization: Bearer $TOKEN"
```

## Environment Variables

For local testing, create `.env` file in `apps/backend/monolith/`:

```env
# Django
DEBUG=True
RBS_SECRET_KEY=your-secret-key-here

# Database (local PostgreSQL)
RBS_PG_DATABASE=robson
RBS_PG_USER=postgres
RBS_PG_PASSWORD=postgres
RBS_PG_HOST=localhost
RBS_PG_PORT=5450

# Binance Testnet (for safe testing)
RBS_BINANCE_API_KEY_TEST=your-testnet-api-key
RBS_BINANCE_SECRET_KEY_TEST=your-testnet-secret-key

# Trading (disabled by default for safety)
BINANCE_USE_TESTNET=True
TRADING_ENABLED=False
```

## Enable Trading Locally

⚠️ **Warning**: Only enable with testnet credentials for safe testing.

```env
BINANCE_USE_TESTNET=True
TRADING_ENABLED=True
```

## One-Liner Test Script

```bash
# Full test script
cd apps/backend/monolith && \
TOKEN=$(curl -s -X POST http://localhost:8000/api/token/ \
  -H "Content-Type: application/json" \
  -d '{"username":"admin","password":"admin123"}' \
  | grep -o '"access":"[^"]*"' | cut -d'"' -f4) && \
echo "Trading Status:" && \
curl -s http://localhost:8000/api/trade/status/ \
  -H "Authorization: Bearer $TOKEN" | python -m json.tool
```

## Useful Endpoints

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/api/token/` | POST | Get JWT token |
| `/api/token/refresh/` | POST | Refresh token |
| `/api/trade/status/` | GET | Trading config status |
| `/api/trade/balance/` | GET | Account balance |
| `/api/trade/buy-btc/` | POST | Buy BTC |
| `/api/trade/sell-btc/` | POST | Sell BTC |
| `/api/trade/history/` | GET | Trade history |
| `/api/trade/pnl/` | GET | P&L summary |
| `/api/ping/` | GET | Binance connectivity |
| `/api/account/balance/` | GET | Legacy balance endpoint |
| `/health/` | GET | Health check |

## Credentials for Local Testing

```
Username: admin
Password: admin123
```

**Never use these credentials in production!**


