# Changelog

All notable changes to Robson Bot will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added - BTC Portfolio Tracking (2025-12-26)

#### Backend
- **BTC-Portfolio Tracking**: Complete portfolio valuation and profit tracking in BTC terms
  - New transaction types: `DEPOSIT` and `WITHDRAWAL` for external flows
  - New movement category: `EXTERNAL` for deposits/withdrawals
  - BTC fields in `BalanceSnapshot`: `total_equity_btc`, `spot_btc_value`, `margin_btc_value`

- **BTCConversionService**: Price discovery and conversion service
  - Multi-route price discovery (direct pair, USDT, BUSD)
  - 60-second price caching to avoid rate limits
  - Converts any asset balance to BTC
  - File: `apps/backend/monolith/api/services/btc_conversion_service.py` (250 lines)

- **PortfolioBTCService**: Portfolio calculation and profit tracking
  - Total portfolio value denominated in BTC
  - Profit formula: `Current Balance (BTC) + Withdrawals (BTC) - Deposits (BTC)`
  - Historical BTC value tracking
  - File: `apps/backend/monolith/api/services/portfolio_btc_service.py` (358 lines)

- **Binance Sync**: Automatic deposit/withdrawal synchronization
  - Syncs from Binance API using `get_deposit_history()` and `get_withdraw_history()`
  - Deduplicates by `binance_order_id`
  - Only syncs successful transactions (status=6)
  - Extended `AuditService` with `sync_deposits_and_withdrawals()` method

- **REST API Endpoints**:
  - `GET /api/portfolio/btc/total/` - Current portfolio value in BTC
  - `GET /api/portfolio/btc/profit/` - Profit in BTC with breakdown
  - `GET /api/portfolio/btc/history/` - Historical BTC value over time
  - `GET /api/portfolio/deposits-withdrawals/` - List of deposits/withdrawals

- **CLI Commands**:
  - `python manage.py portfolio_btc` - Show portfolio in BTC
  - `python manage.py portfolio_btc --profit` - Show profit since inception
  - `python manage.py sync_deposits --days-back 30` - Sync deposits/withdrawals

#### Frontend
- **BTCPortfolioDashboard Component**: Complete portfolio dashboard with tabbed navigation
  - **Overview Tab**: Total value, profit metrics, account breakdown
  - **History Tab**: Interactive chart using Recharts with timeline filtering
  - **Transactions Tab**: Filterable table of deposits/withdrawals
  - Auto-refresh every 60 seconds
  - File: `apps/frontend/src/components/logged/BTCPortfolioDashboard.jsx`

- **Patrimony Component Updated**: Now displays BTC-denominated portfolio
  - Shows total portfolio value in BTC
  - Displays profit/loss in BTC with color coding
  - Account and transaction summaries
  - File: `apps/frontend/src/components/logged/Patrimony.jsx`

- **Footer Updated**:
  - Changed "Designed by Deepmind Team" → "Designed by RBX Robótica"
  - Removed debug text about application mode and BACKEND_URL

#### Testing
- **Unit Tests**: `test_btc_conversion_service.py`
  - Price discovery edge cases
  - Zero division handling
  - Invalid asset handling
  - Cache behavior verification

- **Integration Tests**: `test_btc_portfolio_endpoints.py`
  - Profit calculation correctness
  - API error handling
  - Transaction filtering
  - Empty data handling

- **Frontend Tests**: `BTCPortfolioDashboard.test.js`
  - Loading and error states
  - Tab navigation
  - Data rendering
  - Auto-refresh functionality

#### Database
- **Migration 0019**: Added BTC portfolio tracking fields
  - New transaction types: `DEPOSIT`, `WITHDRAWAL`
  - New category: `EXTERNAL`
  - New BalanceSnapshot fields: `total_equity_btc`, `spot_btc_value`, `margin_btc_value`

### Changed
- Updated `AuditService` to support external flow synchronization
- Extended `main_urls.py` with BTC portfolio routes
- Updated `LoggedHomeScreen.jsx` to include BTCPortfolioDashboard

### Technical Details
- **Architecture**: Hexagonal (Ports & Adapters)
- **Language**: 100% English (code, comments, docs)
- **Testing**: pytest (backend), vitest (frontend)
- **Frontend Library**: Recharts 2.12.7 (already installed)
- **Backend**: Django 5.2, Python 3.12

### Migration Guide
```bash
# Apply database migration
python manage.py migrate api

# Sync historical deposits (last 90 days)
python manage.py sync_deposits --days-back 90

# Verify portfolio in BTC
python manage.py portfolio_btc --profit
```

---

## Previous Versions

For versions prior to 2025-12-26, please refer to git history.
