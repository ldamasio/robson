# Robson Bot - API Requirements

**Purpose**: API-level requirements defining REST endpoint behaviors, authentication, request/response contracts, and non-functional requirements.

**Last Updated**: 2025-11-14

---

## 1. Current Implementation Requirements

### 1.1 Authentication Endpoints

**REQ-CUR-API-001**: JWT Token Login

**Description**: API must provide login endpoint that returns JWT access and refresh tokens.

**Rationale**: Secure API access with token-based authentication.

**Source**: `apps/backend/monolith/api/views/auth.py`, `docs/specs/api/openapi.yaml:/auth/login`

**HTTP Method**: POST

**Endpoint**: `/api/auth/login` (or `/auth/login`)

**Request**:
```json
{
  "email": "trader@example.com",
  "password": "SecureP@ssw0rd"
}
```

**Response** (200 OK):
```json
{
  "access": "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9...",
  "refresh": "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9..."
}
```

**Error Responses**:
- 400: Invalid request format
- 401: Invalid credentials

**Acceptance Criteria**:
- ✓ Accepts email and password
- ✓ Returns access token (15 min TTL)
- ✓ Returns refresh token (7 days TTL)
- ✓ Returns 401 if credentials invalid
- ✓ No rate limiting enforced (future enhancement)

**Tests**: TBD (`apps/backend/monolith/api/tests/test_auth.py`)

---

**REQ-CUR-API-002**: JWT Token Refresh

**Description**: API must provide refresh endpoint to obtain new access token.

**Rationale**: Avoid re-login when access token expires.

**Source**: `apps/backend/monolith/api/views/auth.py`, `docs/specs/api/openapi.yaml:/auth/refresh`

**HTTP Method**: POST

**Endpoint**: `/api/auth/refresh` (or `/auth/refresh`)

**Request**:
```json
{
  "refresh": "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9..."
}
```

**Response** (200 OK):
```json
{
  "access": "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9..."
}
```

**Error Responses**:
- 400: Invalid request format
- 401: Invalid or expired refresh token

**Acceptance Criteria**:
- ✓ Accepts refresh token
- ✓ Returns new access token
- ✓ Returns 401 if refresh token invalid/expired
- ✓ Original refresh token remains valid (not rotated)

**Tests**: TBD (`apps/backend/monolith/api/tests/test_auth.py`)

---

### 1.2 Authorization

**REQ-CUR-API-003**: Bearer Token Authentication

**Description**: All API endpoints (except auth) must require valid Bearer token.

**Rationale**: Protect API from unauthorized access.

**Source**: `apps/backend/monolith/backend/settings.py:REST_FRAMEWORK`, DRF configuration

**Authorization Header**:
```
Authorization: Bearer <access_token>
```

**Behavior**:
- Missing token: 401 Unauthorized
- Invalid token: 401 Unauthorized
- Expired token: 401 Unauthorized
- Valid token: Request processed

**Acceptance Criteria**:
- ✓ Endpoints require Authorization header (except /auth/login)
- ✓ Bearer token validated on each request
- ✓ Expired tokens rejected
- ✓ User identity extracted from token

**Tests**: TBD (integration tests for protected endpoints)

---

**REQ-CUR-API-004**: Multi-Tenant Data Scoping

**Description**: API must automatically filter all queries by authenticated user's client.

**Rationale**: Ensure tenant data isolation at API level.

**Source**: `apps/backend/monolith/api/models/base.py:TenantManager`, DRF viewsets

**Behavior**:
- All list endpoints filter by `client_id`
- All detail endpoints verify ownership
- Cross-client access returns 404 (not 403)

**Acceptance Criteria**:
- ✓ User cannot list other clients' orders
- ✓ User cannot retrieve other clients' orders by ID
- ✓ User cannot modify other clients' resources
- ✓ Attempting cross-client access returns 404

**Tests**: TBD (multi-tenant isolation tests)

---

### 1.3 Trading Endpoints (Existing)

**REQ-CUR-API-005**: List Orders

**Description**: API must provide endpoint to list user's orders with filtering and pagination.

**Rationale**: Users need to view their order history.

**Source**: `apps/backend/monolith/api/views/trading.py` (inferred), `docs/specs/api/openapi.yaml:/orders`

**HTTP Method**: GET

**Endpoint**: `/api/orders/`

**Query Parameters**:
- `limit`: int (default 20, max 100) - pagination limit
- `offset`: int (default 0) - pagination offset
- `status`: str (optional) - filter by status (PENDING, FILLED, CANCELLED, REJECTED)
- `symbol`: str (optional) - filter by symbol name

**Response** (200 OK):
```json
{
  "count": 42,
  "results": [
    {
      "id": "550e8400-e29b-41d4-a716-446655440000",
      "symbol": "BTCUSDT",
      "side": "BUY",
      "type": "LIMIT",
      "quantity": "0.1",
      "price": "50000.00",
      "status": "FILLED",
      "filled_quantity": "0.1",
      "average_price": "50125.50",
      "created_at": "2025-11-14T10:30:00Z",
      "updated_at": "2025-11-14T10:31:00Z"
    }
  ]
}
```

**Acceptance Criteria**:
- ✓ Returns paginated list of orders
- ✓ Filters by authenticated user's client
- ✓ Supports status filter
- ✓ Supports symbol filter
- ✓ Orders sorted by created_at descending

**Tests**: TBD (`apps/backend/monolith/api/tests/test_trading.py::test_list_orders`)

---

**REQ-CUR-API-006**: Create Order

**Description**: API must provide endpoint to create new trading order.

**Rationale**: Core functionality for placing trades.

**Source**: `apps/backend/monolith/api/views/trading.py` (inferred), `docs/specs/api/openapi.yaml:/orders`

**HTTP Method**: POST

**Endpoint**: `/api/orders/`

**Request**:
```json
{
  "symbol": "BTCUSDT",
  "side": "BUY",
  "type": "LIMIT",
  "quantity": "0.1",
  "price": "50000.00",
  "strategy_id": "uuid-optional"
}
```

**Response** (201 Created):
```json
{
  "id": "550e8400-e29b-41d4-a716-446655440000",
  "symbol": "BTCUSDT",
  "side": "BUY",
  "type": "LIMIT",
  "quantity": "0.1",
  "price": "50000.00",
  "status": "PENDING",
  "created_at": "2025-11-14T10:30:00Z"
}
```

**Error Responses**:
- 400: Invalid request (validation errors)
- 404: Symbol not found

**Validation**:
- symbol: required, must exist
- side: required, must be BUY or SELL
- type: required, must be MARKET or LIMIT
- quantity: required, must be > 0
- price: required for LIMIT, prohibited for MARKET
- strategy_id: optional UUID

**Acceptance Criteria**:
- ✓ Creates order with PENDING status
- ✓ Associates with authenticated user's client
- ✓ Validates all required fields
- ✓ Returns 400 for validation errors
- ✓ Returns 201 with created order

**Tests**: TBD (`apps/backend/monolith/api/tests/test_trading.py::test_create_order`)

---

**REQ-CUR-API-007**: Get Order by ID

**Description**: API must provide endpoint to retrieve specific order by ID.

**Rationale**: Users need order details for tracking.

**Source**: `apps/backend/monolith/api/views/trading.py` (inferred), `docs/specs/api/openapi.yaml:/orders/{order_id}`

**HTTP Method**: GET

**Endpoint**: `/api/orders/{order_id}/`

**Response** (200 OK):
```json
{
  "id": "550e8400-e29b-41d4-a716-446655440000",
  "symbol": "BTCUSDT",
  "side": "BUY",
  "type": "LIMIT",
  "quantity": "0.1",
  "price": "50000.00",
  "status": "FILLED",
  "filled_quantity": "0.1",
  "average_price": "50125.50",
  "created_at": "2025-11-14T10:30:00Z",
  "updated_at": "2025-11-14T10:31:00Z"
}
```

**Error Responses**:
- 404: Order not found or not owned by user

**Acceptance Criteria**:
- ✓ Returns order if owned by user
- ✓ Returns 404 if not found
- ✓ Returns 404 if owned by different client (not 403)

**Tests**: TBD (`apps/backend/monolith/api/tests/test_trading.py::test_get_order`)

---

**REQ-CUR-API-008**: Cancel Order

**Description**: API must provide endpoint to cancel pending order.

**Rationale**: Users need ability to cancel unfilled orders.

**Source**: `apps/backend/monolith/api/views/trading.py` (inferred), `docs/specs/api/openapi.yaml:/orders/{order_id}`

**HTTP Method**: DELETE

**Endpoint**: `/api/orders/{order_id}/`

**Response** (204 No Content)

**Error Responses**:
- 400: Order cannot be cancelled (already filled/cancelled)
- 404: Order not found or not owned by user

**Acceptance Criteria**:
- ✓ Cancels order if status is PENDING or PARTIALLY_FILLED
- ✓ Returns 400 if order already FILLED or CANCELLED
- ✓ Returns 404 if not found or not owned
- ✓ Updates order status to CANCELLED

**Tests**: TBD (`apps/backend/monolith/api/tests/test_trading.py::test_cancel_order`)

---

### 1.4 Market Data Endpoints (Existing)

**REQ-CUR-API-009**: Get Current Price

**Description**: API must provide endpoint to get current market price for symbol.

**Rationale**: Users need real-time prices for decision making.

**Source**: `apps/backend/monolith/api/views/market.py` (inferred), `docs/specs/api/openapi.yaml:/market/price/{symbol}`

**HTTP Method**: GET

**Endpoint**: `/api/market/price/{symbol}/`

**Path Parameter**:
- `symbol`: str - trading pair (e.g., BTCUSDT)

**Response** (200 OK):
```json
{
  "symbol": "BTCUSDT",
  "price": "50000.00",
  "timestamp": "2025-11-14T10:30:00Z"
}
```

**Error Responses**:
- 404: Symbol not found

**Acceptance Criteria**:
- ✓ Returns current price from Binance
- ✓ Returns timestamp of price
- ✓ Returns 404 if symbol invalid
- ✓ Price cached for 1 second (future)

**Tests**: TBD (`apps/backend/monolith/api/tests/test_market.py::test_get_current_price`)

---

### 1.5 Response Format Standards

**REQ-CUR-API-010**: Consistent Error Response Format

**Description**: All API errors must follow consistent JSON error format.

**Rationale**: Predictable error handling for clients.

**Source**: DRF default error handling

**Error Response Format**:
```json
{
  "error": "validation_error",
  "message": "Invalid request parameters",
  "details": {
    "quantity": ["Must be greater than 0"]
  }
}
```

**Fields**:
- `error`: string - error code/type
- `message`: string - human-readable message
- `details`: object (optional) - field-level errors

**Acceptance Criteria**:
- ✓ All 4xx/5xx responses use this format
- ✓ Field validation errors included in details
- ✓ Error types are consistent (validation_error, not_found, unauthorized, etc.)

**Tests**: TBD (error response format tests)

---

**REQ-CUR-API-011**: Pagination Format

**Description**: All list endpoints must use consistent pagination format.

**Rationale**: Standardize pagination across all resources.

**Source**: DRF pagination settings

**Response Format**:
```json
{
  "count": 100,
  "next": "https://api.example.com/api/orders/?limit=20&offset=20",
  "previous": null,
  "results": [...]
}
```

**Query Parameters**:
- `limit`: int (default 20, max 100)
- `offset`: int (default 0)

**Acceptance Criteria**:
- ✓ All list endpoints return count, next, previous, results
- ✓ next/previous are absolute URLs or null
- ✓ Default limit is 20
- ✓ Maximum limit is 100

**Tests**: TBD (pagination tests)

---

### 1.6 Non-Functional Requirements

**REQ-CUR-API-012**: Decimal Precision

**Description**: All price and quantity fields must use string representation of decimals.

**Rationale**: Prevent floating-point precision errors in financial calculations.

**Source**: Django Decimal field serialization

**Format**:
- Prices: string with up to 8 decimal places (e.g., "50000.12345678")
- Quantities: string with up to 8 decimal places (e.g., "0.12345678")
- P&L: string with up to 8 decimal places

**Acceptance Criteria**:
- ✓ No floating-point numbers in JSON responses
- ✓ All decimal fields serialized as strings
- ✓ Client can parse as Decimal/BigDecimal
- ✓ Precision maintained through round-trip

**Tests**: TBD (decimal serialization tests)

---

**REQ-CUR-API-013**: Timestamp Format

**Description**: All timestamps must use ISO 8601 format with timezone.

**Rationale**: Unambiguous datetime representation.

**Source**: DRF DateTimeField serialization

**Format**: `YYYY-MM-DDTHH:MM:SSZ` (UTC timezone)

**Example**: `2025-11-14T10:30:00Z`

**Acceptance Criteria**:
- ✓ All timestamps in UTC
- ✓ ISO 8601 format
- ✓ Includes timezone indicator (Z)
- ✓ Consistent across all endpoints

**Tests**: TBD (timestamp format tests)

---

**REQ-CUR-API-014**: CORS Configuration

**Description**: API must support Cross-Origin Resource Sharing for frontend.

**Rationale**: Enable React frontend to call API from different origin.

**Source**: `apps/backend/monolith/backend/settings.py:CORS_ALLOWED_ORIGINS`

**Allowed Origins** (current):
- `http://localhost:5173` (Vite dev server)
- `http://localhost:3000` (Legacy)

**Acceptance Criteria**:
- ✓ CORS headers present in responses
- ✓ Preflight OPTIONS requests handled
- ✓ Allowed origins configurable via environment

**Tests**: TBD (CORS tests)

---

## 2. Future / Planned Requirements

### 2.1 Missing CRUD Endpoints

**REQ-FUT-API-001**: Strategy CRUD Endpoints

**Description**: API should provide full CRUD for strategies.

**Rationale**: Users need to manage strategies via API.

**Dependencies**:
- REQ-CUR-DOMAIN-003 (strategy configuration)

**Priority**: High

**Estimated Complexity**: Simple

**Endpoints** (when implemented):
- GET `/api/strategies/` - List strategies
- POST `/api/strategies/` - Create strategy
- GET `/api/strategies/{id}/` - Get strategy
- PATCH `/api/strategies/{id}/` - Update strategy
- DELETE `/api/strategies/{id}/` - Delete strategy
- POST `/api/strategies/{id}/activate/` - Activate strategy
- POST `/api/strategies/{id}/deactivate/` - Deactivate strategy

---

**REQ-FUT-API-002**: Position CRUD Endpoints

**Description**: API should provide endpoints for position management.

**Rationale**: Users need to view and manage open positions.

**Dependencies**:
- REQ-CUR-DOMAIN-011 (position tracking)

**Priority**: High

**Estimated Complexity**: Simple

**Endpoints** (when implemented):
- GET `/api/positions/` - List open positions
- GET `/api/positions/{id}/` - Get position details
- POST `/api/positions/{id}/close/` - Manually close position

---

**REQ-FUT-API-003**: Trade History Endpoints

**Description**: API should provide endpoints to view trade history.

**Rationale**: Users need historical performance data.

**Dependencies**:
- REQ-CUR-DOMAIN-014 (trade tracking)

**Priority**: Medium

**Estimated Complexity**: Simple

**Endpoints** (when implemented):
- GET `/api/trades/` - List completed trades
- GET `/api/trades/{id}/` - Get trade details
- GET `/api/trades/stats/` - Get aggregate statistics

---

**REQ-FUT-API-004**: Indicator Endpoints

**Description**: API should provide endpoints to access technical indicators.

**Rationale**: Frontend needs indicator data for charts.

**Dependencies**:
- REQ-CUR-DOMAIN-021 (indicators)

**Priority**: Medium

**Estimated Complexity**: Moderate

**Endpoints** (when implemented):
- GET `/api/indicators/{symbol}/ma/` - Get MA values
- GET `/api/indicators/{symbol}/rsi/` - Get RSI values
- GET `/api/indicators/{symbol}/macd/` - Get MACD values
- GET `/api/indicators/{symbol}/bollinger/` - Get Bollinger Bands

---

**REQ-FUT-API-005**: Symbol Management Endpoints

**Description**: API should provide endpoints to manage symbols.

**Rationale**: Admin users need to add/configure symbols.

**Dependencies**:
- REQ-CUR-DOMAIN-001 (symbol model)

**Priority**: Low

**Estimated Complexity**: Simple

**Endpoints** (when implemented):
- GET `/api/symbols/` - List available symbols
- POST `/api/symbols/` - Add new symbol (admin only)
- PATCH `/api/symbols/{id}/` - Update symbol constraints (admin only)

---

### 2.2 Real-Time Features

**REQ-FUT-API-006**: WebSocket Order Updates

**Description**: API should push order status updates via WebSocket.

**Rationale**: Real-time order tracking without polling.

**Dependencies**:
- WebSocket infrastructure
- REQ-CUR-DOMAIN-005 (order state machine)

**Priority**: High

**Estimated Complexity**: Moderate

**WebSocket Events** (when implemented):
- `order.created` - New order placed
- `order.filled` - Order fully filled
- `order.partially_filled` - Partial fill
- `order.cancelled` - Order cancelled

---

**REQ-FUT-API-007**: WebSocket Price Updates

**Description**: API should stream real-time price updates via WebSocket.

**Rationale**: Real-time market data for strategies.

**Dependencies**:
- WebSocket infrastructure
- Binance WebSocket integration

**Priority**: High

**Estimated Complexity**: Moderate

**WebSocket Events** (when implemented):
- `price.update` - Symbol price changed
- Subscription: client subscribes to specific symbols
- Unsubscription: client unsubscribes

---

**REQ-FUT-API-008**: WebSocket Position Updates

**Description**: API should push position P&L updates via WebSocket.

**Rationale**: Real-time portfolio value tracking.

**Dependencies**:
- REQ-FUT-API-007 (price updates)
- REQ-CUR-DOMAIN-012 (position P&L)

**Priority**: Medium

**Estimated Complexity**: Moderate

**WebSocket Events** (when implemented):
- `position.updated` - Position P&L changed
- Rate limiting: max 1 update/second per position

---

### 2.3 API Enhancements

**REQ-FUT-API-009**: Rate Limiting

**Description**: API should enforce rate limits per user.

**Rationale**: Prevent API abuse and ensure fair usage.

**Dependencies**: None

**Priority**: High

**Estimated Complexity**: Simple

**Limits** (when implemented):
- 100 requests per minute per user
- 1000 requests per hour per user
- Header: `X-RateLimit-Remaining`

---

**REQ-FUT-API-010**: API Versioning

**Description**: API should support versioning via URL prefix.

**Rationale**: Allow breaking changes without disrupting existing clients.

**Dependencies**: None

**Priority**: Medium

**Estimated Complexity**: Simple

**Format** (when implemented):
- `/api/v1/orders/` - Version 1 (current)
- `/api/v2/orders/` - Version 2 (future)

---

**REQ-FUT-API-011**: Bulk Operations

**Description**: API should support bulk order creation and cancellation.

**Rationale**: Efficiency for strategies managing many orders.

**Dependencies**:
- REQ-CUR-API-006 (create order)
- REQ-CUR-API-008 (cancel order)

**Priority**: Low

**Estimated Complexity**: Moderate

**Endpoints** (when implemented):
- POST `/api/orders/bulk/` - Create multiple orders
- DELETE `/api/orders/bulk/` - Cancel multiple orders

---

**REQ-FUT-API-012**: GraphQL API

**Description**: API should offer GraphQL endpoint as alternative to REST.

**Rationale**: Flexible querying for complex frontend requirements.

**Dependencies**: None

**Priority**: Low

**Estimated Complexity**: Complex

**Acceptance Criteria** (when implemented):
- [ ] GraphQL endpoint at `/graphql`
- [ ] Schema includes all resources (Orders, Positions, Strategies)
- [ ] Mutations for create/update/delete operations
- [ ] Subscriptions for real-time updates

---

### 2.4 Security Enhancements

**REQ-FUT-API-013**: API Key Authentication

**Description**: API should support API key authentication for programmatic access.

**Rationale**: Enable trading bots and scripts.

**Dependencies**:
- REQ-CUR-API-003 (authentication)

**Priority**: Medium

**Estimated Complexity**: Moderate

**Acceptance Criteria** (when implemented):
- [ ] User can generate API keys via web UI
- [ ] API keys have configurable permissions (read-only, trade)
- [ ] API keys can be revoked
- [ ] Header: `X-API-Key: <key>`

---

**REQ-FUT-API-014**: IP Whitelisting

**Description**: API should support IP whitelisting for API keys.

**Rationale**: Additional security for programmatic access.

**Dependencies**:
- REQ-FUT-API-013 (API keys)

**Priority**: Low

**Estimated Complexity**: Simple

**Acceptance Criteria** (when implemented):
- [ ] API key can have whitelist of allowed IPs
- [ ] Requests from non-whitelisted IPs rejected (403)
- [ ] IP whitelisting optional per API key

---

## 3. Known Gaps or Unclear Behavior

### 3.1 URL Structure Inconsistency

**Gap**: Unclear if endpoints use `/api/` prefix or direct (`/auth/login` vs `/api/auth/login`).

**Impact**: Frontend must guess or handle both patterns.

**Current State**: OpenAPI spec shows `/api/` prefix; views location unclear.

**Recommendation**:
- Standardize on `/api/v1/` prefix for all endpoints
- Document in OpenAPI and API requirements
- Add integration tests verifying URL structure

---

### 3.2 Order Execution Flow

**Gap**: API has order creation (REQ-CUR-API-006) but **execution mechanism unclear**.

**Impact**: Unclear if order auto-executes or requires separate trigger.

**Current State**: Order created with PENDING status; no visible execution endpoint.

**Recommendation**:
- Document order execution flow (create → submit to exchange → callback → update status)
- Specify if execution is synchronous (blocking) or asynchronous (background)
- Add tests for order execution workflow

---

### 3.3 Error Code Standardization

**Gap**: Error response format defined (REQ-CUR-API-010) but **error codes not standardized**.

**Impact**: Clients must handle inconsistent error codes.

**Current State**: Relies on DRF defaults.

**Recommendation**:
- Define standard error code enum (validation_error, not_found, unauthorized, etc.)
- Document all possible error codes per endpoint
- Add tests verifying error codes

---

### 3.4 Pagination Consistency

**Gap**: Pagination format defined (REQ-CUR-API-011) but **not verified across endpoints**.

**Impact**: Possible inconsistency between endpoints.

**Current State**: DRF default pagination assumed.

**Recommendation**:
- Audit all list endpoints for pagination consistency
- Enforce pagination in all list views
- Add tests verifying pagination format

---

### 3.5 Multi-Tenant User Management API

**Gap**: No API endpoints for **user/client management**.

**Impact**: Unclear how users register, manage profile, or select client.

**Current State**: Authentication exists but no user CRUD.

**Recommendation**:
- Create REQ-FUT-API-015 for user management endpoints
- Specify user registration workflow
- Document multi-client user scenarios

---

### 3.6 Missing Documentation Endpoint

**Gap**: No **API documentation endpoint** (e.g., Swagger UI).

**Impact**: Developers must read OpenAPI YAML manually.

**Current State**: OpenAPI spec exists as file only.

**Recommendation**:
- Add `/api/docs/` endpoint serving Swagger UI
- Use drf-spectacular to auto-generate OpenAPI from code
- Keep OpenAPI spec in sync with implementation

---

## 4. Traceability

### Current Requirements → OpenAPI

| Requirement ID       | OpenAPI Path                          |
|----------------------|---------------------------------------|
| REQ-CUR-API-001      | `/auth/login` (POST)                  |
| REQ-CUR-API-002      | `/auth/refresh` (POST)                |
| REQ-CUR-API-005      | `/orders` (GET)                       |
| REQ-CUR-API-006      | `/orders` (POST)                      |
| REQ-CUR-API-007      | `/orders/{order_id}` (GET)            |
| REQ-CUR-API-008      | `/orders/{order_id}` (DELETE)         |
| REQ-CUR-API-009      | `/market/price/{symbol}` (GET)        |

### Current Requirements → Views (Inferred)

| Requirement ID       | View Reference (Inferred)             |
|----------------------|---------------------------------------|
| REQ-CUR-API-001-002  | `apps/backend/monolith/api/views/auth.py` |
| REQ-CUR-API-005-008  | `apps/backend/monolith/api/views/trading.py` |
| REQ-CUR-API-009      | `apps/backend/monolith/api/views/market.py` |

*Note: Actual view implementations need verification - paths inferred from file existence.*

### Current Requirements → Settings

| Requirement ID       | Settings Reference                    |
|----------------------|---------------------------------------|
| REQ-CUR-API-003      | `apps/backend/monolith/backend/settings.py:REST_FRAMEWORK.DEFAULT_AUTHENTICATION_CLASSES` |
| REQ-CUR-API-014      | `apps/backend/monolith/backend/settings.py:CORS_ALLOWED_ORIGINS` |

---

**End of Document**
