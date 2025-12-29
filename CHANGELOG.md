# Changelog

All notable changes to Robson Bot will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added - Pattern Detection Engine (2025-12-28)

#### Core Engine (CORE 1.0)

- **Pattern Detection System**: Deterministic, idempotent technical pattern recognition
  - 7 patterns implemented (5 candlestick + 2 chart):
    - Candlestick: Hammer, Inverted Hammer, Bullish Engulfing, Bearish Engulfing, Morning Star
    - Chart: Head & Shoulders, Inverted Head & Shoulders
  - Pure domain layer (NO Django dependencies in core)
  - Hexagonal architecture (ports & adapters)
  - All timestamps from exchange data (timezone-independent)
  - Idempotent persistence (stable uniqueness keys)
  - Module: `apps/backend/monolith/api/application/pattern_engine/` (11 files, 3,176 lines)

- **Pattern Detectors**:
  - `HammerDetector` - Bullish reversal (1 candle, confidence: 0.75)
  - `InvertedHammerDetector` - Bullish reversal (1 candle, confidence: 0.70)
  - `EngulfingDetector` - Bullish/Bearish reversal (2 candles, confidence: 0.80)
  - `MorningStarDetector` - Bullish reversal (3 candles, confidence: 0.85)
  - `HeadAndShouldersDetector` - Bearish reversal (multi-bar, confidence: 0.80)
  - `InvertedHeadAndShouldersDetector` - Bullish reversal (multi-bar, confidence: 0.80)

- **Pattern Lifecycle Management**:
  - States: FORMING → CONFIRMED → INVALIDATED
  - Automatic confirmation checks (e.g., Hammer confirmed on close above high)
  - Automatic invalidation checks (e.g., Hammer invalidated on close below low)
  - PatternAlert emission (idempotent, uniqueness key: instance_id + alert_type + alert_ts)

- **Management Command**:
  - `python manage.py detect_patterns SYMBOL TIMEFRAME [--all|--candlestick|--chart]`
  - Flexible detector selection (individual pattern flags: --hammer, --hns, etc.)
  - Rich terminal output with idempotency tracking
  - File: `apps/backend/monolith/api/management/commands/detect_patterns.py` (371 lines)

- **Tests**:
  - 22+ test cases (869 lines)
  - Pure unit tests (helpers, golden OHLC sequences)
  - Idempotency integration tests (CRITICAL: 2nd run creates 0 instances/alerts)
  - Property tests (optional, requires Hypothesis)
  - File: `apps/backend/monolith/api/tests/test_pattern_engine.py`

- **Documentation**:
  - ADR-0018: Architecture decision record
  - PATTERN_ENGINE_V1.md: Technical specification with detection rules
  - PATTERN_ENGINE_IMPLEMENTATION_PLAN.md: Milestones M1-M8
  - PATTERN_ENGINE_IMPLEMENTATION_SUMMARY.md: Usage guide with examples
  - PATTERN_ENGINE_SESSION_HANDOFF.md: Deployment guide

#### Key Features

- ✅ **Deterministic**: Same OHLC → Same patterns (no randomness)
- ✅ **Idempotent**: Re-scans create 0 duplicates (verified: alerts_created=0 on 2nd run)
- ✅ **Timezone-Independent**: All timestamps from exchange data (no `datetime.now()`)
- ✅ **Non-Executing**: Emits alerts only (NO order placement)
- ✅ **Zero Schema Changes**: Uses existing production models (PatternInstance, PatternAlert)

#### Integration Points

- Uses existing `MarketDataService` for candle fetching (with caching)
- Uses existing Django pattern models (no migrations required)
- Output: `PatternAlert` table ready for EntryGate (CORE 1.2) consumption
- Future: Hand-Span Trailing Stop (CORE 1.1) triggered on confirmed patterns

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
