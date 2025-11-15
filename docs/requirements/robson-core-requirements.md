# Robson Bot - Core Requirements

**Purpose**: Product-level requirements defining what Robson Bot is, who uses it, and what core capabilities it provides.

**Last Updated**: 2025-11-14

---

## 1. Current Implementation Requirements

### 1.1 Product Identity

**REQ-CUR-CORE-001**: Cryptocurrency Trading Platform

**Description**: Robson Bot is an open-source cryptocurrency trading platform that enables automated trading with risk management.

**Rationale**: Democratize algorithmic trading for cryptocurrency markets.

**Source**: `README.md`, overall system architecture

**Constraints**:
- Currently supports Binance exchange only
- Focuses on spot trading (no futures/derivatives)
- Python 3.12+ backend, React 18 frontend

**Acceptance Criteria**:
- ✓ Users can connect to Binance via API keys
- ✓ Users can configure trading strategies
- ✓ System executes trades automatically based on strategies
- ✓ System calculates and displays P&L

---

**REQ-CUR-CORE-002**: Multi-Tenant Architecture

**Description**: System must support multiple independent clients (tenants) with complete data isolation.

**Rationale**: Enable platform-as-a-service model where multiple users can trade independently.

**Source**: `apps/backend/monolith/api/models/base.py:TenantMixin`, all models with `client` field

**Constraints**:
- All domain entities scoped to `Client` via foreign key
- No cross-client data leakage
- Each client has independent: symbols, strategies, orders, positions

**Acceptance Criteria**:
- ✓ Client A cannot access Client B's orders
- ✓ Client A cannot see Client B's positions
- ✓ Client A cannot modify Client B's strategies
- ✓ Database queries automatically filter by client_id

**Tests**: `apps/backend/monolith/api/tests/test_models.py` (multi-tenant isolation tests)

---

**REQ-CUR-CORE-003**: Risk Management System

**Description**: System must provide configurable risk management rules to limit capital exposure.

**Rationale**: Protect users from excessive losses through enforced risk rules.

**Source**: `apps/backend/monolith/api/models/risk.py`

**Constraints**:
- Risk rules based on percentage of capital
- Currently implements 1% and 4% risk rules
- Risk rules are advisory (not yet enforced at order placement)

**Acceptance Criteria**:
- ✓ System defines BaseRiskRule abstraction
- ✓ OnePercentOfCapital rule configured at 1.00%
- ✓ JustBet4percent rule configured at 4.00%
- ✓ Risk rules have optional max_capital_amount caps

**Tests**: TBD (risk enforcement not yet tested)

---

**REQ-CUR-CORE-004**: Technical Analysis Support

**Description**: System must calculate and store technical indicators for trading decision support.

**Rationale**: Technical analysis is core to algorithmic trading strategies.

**Source**: `apps/backend/monolith/api/models/indicators.py`

**Constraints**:
- Indicators calculated per symbol and timeframe
- Currently supports: MA, RSI, MACD, Bollinger Bands, Stochastic
- Indicator values stored in database (not real-time calculation)

**Acceptance Criteria**:
- ✓ System can store Moving Average values
- ✓ System can store RSI values
- ✓ System can store MACD (macd, signal, histogram) values
- ✓ System can store Bollinger Bands (upper, middle, lower) values
- ✓ System can store Stochastic Oscillator (%K, %D) values

**Tests**: TBD (indicator calculation tests)

---

**REQ-CUR-CORE-005**: Strategy Configuration

**Description**: System must allow users to define and configure trading strategies with flexible parameters.

**Rationale**: Different trading approaches require different strategy configurations.

**Source**: `apps/backend/monolith/api/models/trading.py:Strategy`

**Constraints**:
- Strategy configuration stored as JSON (flexible schema)
- Strategies can be activated/deactivated
- No built-in strategy execution engine (manual or external trigger)

**Acceptance Criteria**:
- ✓ User can create strategy with name and description
- ✓ User can store arbitrary configuration as JSON
- ✓ User can store risk configuration separately
- ✓ User can activate/deactivate strategy
- ✓ System tracks strategy performance (win rate, P&L)

**Tests**: `apps/backend/monolith/api/tests/test_models.py::test_strategy*`

---

### 1.2 Target Users (Current)

**REQ-CUR-CORE-006**: Individual Cryptocurrency Traders

**Description**: System targets individual traders who want to automate cryptocurrency trading.

**Rationale**: Primary user persona for current implementation.

**Source**: `README.md`, system design

**Constraints**:
- Single-user per client model
- No team collaboration features
- No role-based permissions

**Acceptance Criteria**:
- ✓ Trader can connect exchange account
- ✓ Trader can configure strategies
- ✓ Trader can view orders and positions
- ✓ Trader can track P&L

---

**REQ-CUR-CORE-007**: Researchers and Developers

**Description**: System serves researchers and developers exploring algorithmic trading.

**Rationale**: Open-source positioning for community contributions.

**Source**: `README.md`, `CONTRIBUTING.md`

**Constraints**:
- Code must be English-only for international collaboration
- Hexagonal architecture for testability
- Comprehensive documentation required

**Acceptance Criteria**:
- ✓ Developer can run system locally with Docker
- ✓ Developer can run tests
- ✓ Developer can contribute following CONTRIBUTING.md
- ✓ Code follows Python and JavaScript conventions

---

### 1.3 Core Capabilities (Current)

**REQ-CUR-CORE-008**: Order Lifecycle Management

**Description**: System must manage the complete lifecycle of trading orders from creation to completion.

**Rationale**: Core capability for any trading platform.

**Source**: `apps/backend/monolith/api/models/trading.py:Order`

**Constraints**:
- Order states: PENDING, PARTIALLY_FILLED, FILLED, CANCELLED, REJECTED
- Order types: MARKET, LIMIT
- Fill tracking with average fill price

**Acceptance Criteria**:
- ✓ User can create order with symbol, side, quantity, price
- ✓ System tracks order status transitions
- ✓ System tracks filled quantity and average fill price
- ✓ System calculates remaining quantity
- ✓ System marks order as filled when fully executed

**Tests**: `apps/backend/monolith/api/tests/test_models.py::test_order*`

---

**REQ-CUR-CORE-009**: Position Tracking

**Description**: System must track open positions with unrealized P&L.

**Rationale**: Traders need to see current exposure and performance.

**Source**: `apps/backend/monolith/api/models/trading.py:Position`

**Constraints**:
- Position tracks average entry price
- Unrealized P&L calculated based on current market price
- Long (BUY) and short (SELL) positions supported

**Acceptance Criteria**:
- ✓ System creates position from filled order
- ✓ System tracks quantity and average price
- ✓ System calculates unrealized P&L
- ✓ System can close position and calculate realized P&L

**Tests**: `apps/backend/monolith/api/tests/test_models.py::test_position*`

---

**REQ-CUR-CORE-010**: Trade History

**Description**: System must maintain history of completed trades with fees and duration.

**Rationale**: Historical analysis and performance tracking.

**Source**: `apps/backend/monolith/api/models/trading.py:Trade`

**Constraints**:
- Trade records entry and exit prices
- Fee tracking (entry_fee, exit_fee)
- Duration calculation (entry_time to exit_time)

**Acceptance Criteria**:
- ✓ System records trade entry price and time
- ✓ System records trade exit price and time
- ✓ System calculates P&L (gross - fees)
- ✓ System calculates P&L percentage
- ✓ System identifies winner vs loser trades
- ✓ System calculates trade duration

**Tests**: `apps/backend/monolith/api/tests/test_models.py::test_trade*`

---

**REQ-CUR-CORE-011**: Binance Integration

**Description**: System must integrate with Binance exchange for market data and order execution.

**Rationale**: Binance is the target exchange for current implementation.

**Source**: `apps/backend/monolith/api/services/binance_service.py`, ADR-0001

**Constraints**:
- Singleton BinanceService instance (rate limit management)
- Supports testnet for development
- Uses python-binance SDK v1.0.16

**Acceptance Criteria**:
- ✓ System connects to Binance using API keys
- ✓ System can fetch current market prices
- ✓ System can place market orders
- ✓ System can place limit orders
- ✓ System uses testnet when BINANCE_USE_TESTNET=True

**Tests**: TBD (Binance integration tests with testnet)

---

**REQ-CUR-CORE-012**: JWT Authentication

**Description**: System must authenticate users via JWT tokens for API access.

**Rationale**: Secure API access with token-based authentication.

**Source**: `apps/backend/monolith/backend/settings.py`, `requirements.txt:djangorestframework-simplejwt`

**Constraints**:
- Access token: 15 minutes TTL
- Refresh token: 7 days TTL
- Bearer token authentication header

**Acceptance Criteria**:
- ✓ User can login with email/password
- ✓ System returns access and refresh tokens
- ✓ User can refresh access token
- ✓ API endpoints require valid access token (except /auth/login)

**Tests**: TBD (authentication flow tests)

---

## 2. Future / Planned Requirements

### 2.1 Product Evolution

**REQ-FUT-CORE-001**: Multi-Exchange Support

**Description**: System should support multiple cryptocurrency exchanges beyond Binance.

**Rationale**: Users want to trade across different exchanges from single platform.

**Dependencies**:
- REQ-CUR-CORE-011 (existing Binance integration as reference)
- Exchange adapter abstraction in hexagonal core

**Priority**: High

**Estimated Complexity**: Complex

**Acceptance Criteria** (when implemented):
- [ ] System supports Coinbase Pro
- [ ] System supports Kraken
- [ ] User can configure which exchange per strategy
- [ ] Unified order and position tracking across exchanges

---

**REQ-FUT-CORE-002**: Real-Time WebSocket Market Data

**Description**: System should provide real-time market data via WebSocket subscriptions.

**Rationale**: Polling is inefficient; strategies need real-time price updates.

**Dependencies**:
- WebSocket infrastructure (partially exists)
- Market data service refactoring

**Priority**: High

**Estimated Complexity**: Moderate

**Acceptance Criteria** (when implemented):
- [ ] Client can subscribe to symbol price updates
- [ ] Client receives updates within 500ms of market change
- [ ] Client can unsubscribe from symbol
- [ ] Rate limiting per client connection

---

**REQ-FUT-CORE-003**: Backtesting Engine

**Description**: System should allow users to backtest strategies against historical data.

**Rationale**: Validate strategy performance before risking real capital.

**Dependencies**:
- Historical market data storage
- Strategy execution abstraction
- REQ-CUR-CORE-005 (strategy configuration)

**Priority**: Medium

**Estimated Complexity**: Complex

**Acceptance Criteria** (when implemented):
- [ ] User can load historical market data for symbol
- [ ] User can run strategy against historical data
- [ ] System simulates order execution with realistic fills
- [ ] System reports strategy performance metrics
- [ ] System accounts for fees and slippage

---

**REQ-FUT-CORE-004**: Role-Based Access Control (RBAC)

**Description**: System should support multiple users per client with different permission levels.

**Rationale**: Enable teams and organizations to collaborate on trading strategies.

**Dependencies**:
- REQ-CUR-CORE-002 (multi-tenant architecture)
- User model extension
- Permission framework

**Priority**: Medium

**Estimated Complexity**: Moderate

**Acceptance Criteria** (when implemented):
- [ ] Admin role can manage all resources
- [ ] Trader role can create orders and strategies
- [ ] Viewer role can view read-only data
- [ ] API enforces role-based permissions

---

**REQ-FUT-CORE-005**: Paper Trading Mode

**Description**: System should support paper trading (simulated trading without real money).

**Rationale**: Users want to test strategies risk-free before live trading.

**Dependencies**:
- REQ-CUR-CORE-011 (Binance integration)
- Simulated order execution engine

**Priority**: High

**Estimated Complexity**: Moderate

**Acceptance Criteria** (when implemented):
- [ ] User can enable paper trading mode per strategy
- [ ] System simulates order fills using real market prices
- [ ] System tracks paper trading P&L separately
- [ ] User can compare paper vs live performance

---

**REQ-FUT-CORE-006**: Alert and Notification System

**Description**: System should notify users of important events (filled orders, stop-loss triggers, etc.).

**Rationale**: Users need timely notifications to react to market events.

**Dependencies**:
- Event bus infrastructure
- Notification service (email, SMS, push)

**Priority**: Medium

**Estimated Complexity**: Moderate

**Acceptance Criteria** (when implemented):
- [ ] User can configure notification preferences
- [ ] System sends notification on order filled
- [ ] System sends notification on stop-loss triggered
- [ ] System sends notification on strategy error
- [ ] Multiple notification channels supported

---

**REQ-FUT-CORE-007**: Portfolio Analytics Dashboard

**Description**: System should provide comprehensive portfolio analytics and performance metrics.

**Rationale**: Users need insights into overall portfolio performance.

**Dependencies**:
- REQ-CUR-CORE-009 (position tracking)
- REQ-CUR-CORE-010 (trade history)
- Analytics calculation service

**Priority**: Low

**Estimated Complexity**: Moderate

**Acceptance Criteria** (when implemented):
- [ ] Dashboard shows total portfolio value
- [ ] Dashboard shows total P&L (realized + unrealized)
- [ ] Dashboard shows win rate across all strategies
- [ ] Dashboard shows Sharpe ratio
- [ ] Dashboard shows maximum drawdown
- [ ] Dashboard shows portfolio composition

---

### 2.2 Scalability and Performance

**REQ-FUT-CORE-008**: Horizontal Scalability

**Description**: System should scale horizontally to handle increased load.

**Rationale**: As user base grows, system must handle more concurrent users and orders.

**Dependencies**:
- Stateless backend services
- Database connection pooling
- Caching layer (Redis)

**Priority**: Medium

**Estimated Complexity**: Complex

**Acceptance Criteria** (when implemented):
- [ ] Backend can run multiple replicas
- [ ] Load balancing distributes requests
- [ ] No shared state between backend instances
- [ ] Session data stored in Redis (not in-memory)

---

**REQ-FUT-CORE-009**: Market Data Caching

**Description**: System should cache frequently accessed market data to reduce API calls.

**Rationale**: Exchange API rate limits require caching for scalability.

**Dependencies**:
- REQ-FUT-CORE-008 (Redis caching layer)
- Market data service

**Priority**: High

**Estimated Complexity**: Simple

**Acceptance Criteria** (when implemented):
- [ ] Symbol prices cached for 1 second
- [ ] Cache invalidation on price update via WebSocket
- [ ] Cache hit rate > 80% for price lookups
- [ ] Cache misses trigger fresh API call

---

## 3. Known Gaps or Unclear Behavior

### 3.1 Risk Management Enforcement

**Gap**: Risk rules are defined (REQ-CUR-CORE-003) but **not enforced** at order placement.

**Impact**: Users can place orders exceeding risk limits.

**Recommendation**:
- Implement order validation against active risk rules
- Create REQ-FUT-CORE-010 for risk enforcement
- Add tests for risk rule violations

---

### 3.2 Strategy Execution Trigger

**Gap**: Strategy configuration exists (REQ-CUR-CORE-005) but **no built-in execution engine**.

**Impact**: Unclear how strategies are triggered (manual, cronjob, event-driven?).

**Current State**: `apps/backend/cronjob/` suggests scheduled execution but not specified.

**Recommendation**:
- Document current strategy execution mechanism
- Create REQ-FUT-CORE-011 for strategy execution engine
- Specify trigger conditions (time-based, event-based, signal-based)

---

### 3.3 Order Execution Confirmation

**Gap**: Unclear how system confirms order execution with exchange.

**Impact**: Potential for order state inconsistencies.

**Current State**: `Order.mark_as_filled()` method exists but caller not specified.

**Recommendation**:
- Document order reconciliation workflow
- Specify polling vs WebSocket for order status updates
- Add tests for order state synchronization

---

### 3.4 Multi-Tenant User Management

**Gap**: Client isolation exists but **user-to-client mapping** not clear.

**Impact**: Unclear if one user can belong to multiple clients.

**Current State**: `clients.models.Client` exists but relationship to `User` not documented.

**Recommendation**:
- Clarify User ↔ Client relationship (1:1 or 1:many or many:many)
- Document tenant selection mechanism in API
- Add tests for multi-client user scenarios

---

### 3.5 Position Lifecycle

**Gap**: Unclear when positions are **automatically closed** vs manually closed.

**Impact**: Risk of stale open positions.

**Current State**: `Position.close_position()` method exists but trigger not specified.

**Recommendation**:
- Document position lifecycle (creation, update, closure)
- Specify auto-close conditions (fully exited, stop-loss, take-profit)
- Add tests for position auto-closure

---

## 4. Traceability

### Current Requirements → Code

| Requirement ID       | Primary Code Reference                            |
|----------------------|---------------------------------------------------|
| REQ-CUR-CORE-001     | Overall system, `README.md`                       |
| REQ-CUR-CORE-002     | `apps/backend/monolith/api/models/base.py:TenantMixin` |
| REQ-CUR-CORE-003     | `apps/backend/monolith/api/models/risk.py`        |
| REQ-CUR-CORE-004     | `apps/backend/monolith/api/models/indicators.py`  |
| REQ-CUR-CORE-005     | `apps/backend/monolith/api/models/trading.py:Strategy` |
| REQ-CUR-CORE-006     | System design, `README.md`                        |
| REQ-CUR-CORE-007     | `CONTRIBUTING.md`, `docs/DEVELOPER.md`            |
| REQ-CUR-CORE-008     | `apps/backend/monolith/api/models/trading.py:Order` |
| REQ-CUR-CORE-009     | `apps/backend/monolith/api/models/trading.py:Position` |
| REQ-CUR-CORE-010     | `apps/backend/monolith/api/models/trading.py:Trade` |
| REQ-CUR-CORE-011     | `apps/backend/monolith/api/services/binance_service.py`, ADR-0001 |
| REQ-CUR-CORE-012     | `apps/backend/monolith/backend/settings.py`, DRF JWT config |

### Current Requirements → Tests

| Requirement ID       | Primary Test Reference                            |
|----------------------|---------------------------------------------------|
| REQ-CUR-CORE-005     | `apps/backend/monolith/api/tests/test_models.py::test_strategy*` |
| REQ-CUR-CORE-008     | `apps/backend/monolith/api/tests/test_models.py::test_order*` |
| REQ-CUR-CORE-009     | `apps/backend/monolith/api/tests/test_models.py::test_position*` |
| REQ-CUR-CORE-010     | `apps/backend/monolith/api/tests/test_models.py::test_trade*` |

*Note: Many requirements lack explicit test coverage - see "Known Gaps" section.*

### Future Requirements → ADRs

| Requirement ID       | Related ADR                                       |
|----------------------|---------------------------------------------------|
| REQ-FUT-CORE-001     | Requires ADR for exchange adapter abstraction     |
| REQ-FUT-CORE-004     | Requires ADR for permission framework choice      |
| REQ-FUT-CORE-008     | Requires ADR for stateless architecture decisions |

---

**End of Document**
