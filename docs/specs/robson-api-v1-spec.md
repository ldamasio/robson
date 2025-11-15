# Robson Bot API v1 - Behavioral Specification

**Purpose**: Narrative specification bridging requirements to OpenAPI implementation. Describes API flows, preconditions, postconditions, and error handling for all REST endpoints.

**Last Updated**: 2025-11-14

**Version**: 1.0 (Current Implementation)

---

## Table of Contents

1. [Overview](#1-overview)
2. [Authentication Flows](#2-authentication-flows)
3. [Trading Flows](#3-trading-flows)
4. [Market Data Flows](#4-market-data-flows)
5. [Error Handling](#5-error-handling)
6. [Future Planned Features](#6-future-planned-features)

---

## 1. Overview

### 1.1 API Design Principles

**RESTful Architecture**:
- Resources identified by URLs
- HTTP methods indicate operations (GET, POST, PATCH, DELETE)
- Stateless requests (JWT token carries identity)

**JSON-First**:
- All requests and responses use `application/json`
- Decimal precision maintained as strings
- Timestamps in ISO 8601 format

**Multi-Tenant**:
- All endpoints filter by authenticated user's client
- Cross-client access returns 404 (not 403)

### 1.2 Base URL

**Production**: `https://api.robsonbot.com/api/v1/`
**Development**: `http://localhost:8000/api/v1/`
**Current Implementation**: Prefix may be `/api/` or `/api/v1/` (see [Known Gap](#71-url-structure-inconsistency))

### 1.3 Global Headers

**Request Headers**:
```
Authorization: Bearer <access_token>
Content-Type: application/json
Accept: application/json
```

**Response Headers**:
```
Content-Type: application/json
Access-Control-Allow-Origin: <configured-origins>
```

---

## 2. Authentication Flows

**References**: REQ-CUR-API-001, REQ-CUR-API-002, REQ-CUR-API-003, REQ-CUR-CORE-012

### 2.1 User Login

**Endpoint**: `POST /auth/login`

**Purpose**: Authenticate user and obtain JWT tokens.

**Request Body**:
```json
{
  "email": "trader@example.com",
  "password": "SecureP@ssw0rd"
}
```

**Success Response** (200 OK):
```json
{
  "access": "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJ1c2VyX2lkIjoxLCJleHAiOjE3MzE1ODU5MDB9.abc123",
  "refresh": "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJ1c2VyX2lkIjoxLCJleHAiOjE3MzIxNzU1MDB9.def456"
}
```

**Token Lifetimes**:
- Access token: 15 minutes (900 seconds)
- Refresh token: 7 days (604800 seconds)

**Error Responses**:

| Status | Error Code | Description |
|--------|------------|-------------|
| 400 | `validation_error` | Missing or invalid email/password format |
| 401 | `invalid_credentials` | Email or password incorrect |

**Preconditions**:
- User account exists in database
- User account is active (not suspended)

**Postconditions**:
- Client stores access token for API requests
- Client stores refresh token for token renewal

**Flow Diagram**:
```
Client                    API                     Database
  |                        |                          |
  |--POST /auth/login----->|                          |
  |   {email, password}    |                          |
  |                        |---Query User------------>|
  |                        |<--User record------------|
  |                        |--Verify password         |
  |                        |--Generate JWT tokens     |
  |<--200 OK {tokens}------|                          |
  |  Store tokens          |                          |
```

---

### 2.2 Token Refresh

**Endpoint**: `POST /auth/refresh`

**Purpose**: Obtain new access token using refresh token (avoid re-login).

**Request Body**:
```json
{
  "refresh": "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9..."
}
```

**Success Response** (200 OK):
```json
{
  "access": "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9..."
}
```

**Error Responses**:

| Status | Error Code | Description |
|--------|------------|-------------|
| 400 | `validation_error` | Missing refresh token |
| 401 | `invalid_token` | Refresh token invalid or expired |

**Preconditions**:
- Refresh token was previously issued via login
- Refresh token not expired
- User account still active

**Postconditions**:
- Client replaces old access token with new one
- Original refresh token remains valid (not rotated)

**Note**: Token rotation is **not currently implemented** (future enhancement).

---

### 2.3 Authorization Flow

**References**: REQ-CUR-API-003, REQ-CUR-API-004

**Protected Endpoints**: All endpoints except `/auth/login` and `/auth/refresh`

**Authorization Process**:

1. **Extract Token**:
   - Client includes `Authorization: Bearer <access_token>` header
   - API extracts token from header

2. **Validate Token**:
   - Verify JWT signature
   - Check expiration timestamp
   - Decode user identity from payload

3. **Check User**:
   - Query user from database by ID
   - Verify user is active
   - Load user's associated client

4. **Scope Data**:
   - All queries automatically filter by `client_id`
   - User cannot access other clients' data

**Error Response** (401 Unauthorized):
```json
{
  "error": "unauthorized",
  "message": "Authentication credentials were not provided or are invalid",
  "details": {}
}
```

**Triggers**:
- Missing Authorization header
- Malformed token
- Expired token
- Invalid signature
- User account suspended

---

## 3. Trading Flows

### 3.1 List Orders

**Endpoint**: `GET /orders/`

**References**: REQ-CUR-API-005

**Purpose**: Retrieve paginated list of user's orders with optional filtering.

**Query Parameters**:

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `limit` | int | No | 20 | Number of results per page (max 100) |
| `offset` | int | No | 0 | Pagination offset |
| `status` | string | No | - | Filter by status (PENDING, PARTIALLY_FILLED, FILLED, CANCELLED, REJECTED) |
| `symbol` | string | No | - | Filter by symbol name (e.g., BTCUSDT) |

**Success Response** (200 OK):
```json
{
  "count": 42,
  "next": "https://api.robsonbot.com/api/v1/orders/?limit=20&offset=20",
  "previous": null,
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
      "avg_fill_price": "50125.50",
      "created_at": "2025-11-14T10:30:00Z",
      "updated_at": "2025-11-14T10:31:00Z"
    }
  ]
}
```

**Preconditions**:
- User authenticated with valid access token
- User has associated client

**Postconditions**:
- Returns only orders belonging to user's client
- Orders sorted by `created_at` descending (newest first)

**Behavior**:
- Empty result set returns `{"count": 0, "results": []}`
- Invalid status filter ignored (returns all statuses)
- Pagination links are absolute URLs

---

### 3.2 Create Order

**Endpoint**: `POST /orders/`

**References**: REQ-CUR-API-006, REQ-CUR-DOMAIN-005

**Purpose**: Place new trading order with validation.

**Request Body**:
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

**Field Validation**:

| Field | Type | Required | Constraints |
|-------|------|----------|-------------|
| `symbol` | string | Yes | Must exist in database, min_qty/max_qty enforced |
| `side` | string | Yes | Must be "BUY" or "SELL" |
| `type` | string | Yes | Must be "MARKET" or "LIMIT" |
| `quantity` | decimal | Yes | Must be > 0, within symbol min/max |
| `price` | decimal | Conditional | Required for LIMIT, prohibited for MARKET |
| `strategy_id` | UUID | No | Must exist and belong to user's client |

**Success Response** (201 Created):
```json
{
  "id": "550e8400-e29b-41d4-a716-446655440000",
  "symbol": "BTCUSDT",
  "side": "BUY",
  "type": "LIMIT",
  "quantity": "0.1",
  "price": "50000.00",
  "status": "PENDING",
  "filled_quantity": "0.0",
  "avg_fill_price": null,
  "created_at": "2025-11-14T10:30:00Z",
  "updated_at": "2025-11-14T10:30:00Z"
}
```

**Error Responses**:

| Status | Error Code | Description | Details Example |
|--------|------------|-------------|-----------------|
| 400 | `validation_error` | Field validation failed | `{"quantity": ["Must be greater than 0"]}` |
| 404 | `not_found` | Symbol not found | `{"symbol": ["Symbol INVALID not found"]}` |

**Preconditions**:
- User authenticated
- Symbol exists in database
- If strategy_id provided, strategy exists and belongs to client

**Postconditions**:
- Order created with status PENDING
- Order associated with user's client
- Order associated with strategy (if provided)
- Order execution triggered (see [Known Gap](#72-order-execution-flow))

**Flow Diagram**:
```
Client            API              Database         Exchange
  |                |                   |                |
  |--POST /orders->|                   |                |
  |                |--Validate fields  |                |
  |                |--Query symbol---->|                |
  |                |<--Symbol record---|                |
  |                |--Create order---->|                |
  |                |<--Order saved-----|                |
  |<--201 Created--|                   |                |
  |                |--Submit order (async)------------->|
  |                |                   |                |
```

**Note**: Order execution is **asynchronous**. Order is created with PENDING status, then submitted to exchange in background. Status updates occur via polling or WebSocket (not yet specified).

---

### 3.3 Get Order by ID

**Endpoint**: `GET /orders/{order_id}/`

**References**: REQ-CUR-API-007

**Purpose**: Retrieve specific order details.

**Path Parameters**:
- `order_id`: UUID of the order

**Success Response** (200 OK):
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
  "avg_fill_price": "50125.50",
  "created_at": "2025-11-14T10:30:00Z",
  "updated_at": "2025-11-14T10:31:00Z"
}
```

**Error Responses**:

| Status | Error Code | Description |
|--------|------------|-------------|
| 404 | `not_found` | Order not found or not owned by user's client |

**Preconditions**:
- User authenticated
- Order ID is valid UUID

**Postconditions**:
- Returns order only if owned by user's client
- **Multi-tenant isolation**: Returns 404 (not 403) for other clients' orders

**Security Note**: Using 404 instead of 403 prevents information leakage about existence of orders.

---

### 3.4 Cancel Order

**Endpoint**: `DELETE /orders/{order_id}/`

**References**: REQ-CUR-API-008, REQ-CUR-DOMAIN-006

**Purpose**: Cancel pending or partially filled order.

**Path Parameters**:
- `order_id`: UUID of the order

**Success Response** (204 No Content)

**Error Responses**:

| Status | Error Code | Description |
|--------|------------|-------------|
| 400 | `invalid_state` | Order cannot be cancelled (already FILLED or CANCELLED) |
| 404 | `not_found` | Order not found or not owned by user's client |

**Preconditions**:
- User authenticated
- Order exists and belongs to user's client
- Order status is PENDING or PARTIALLY_FILLED

**Postconditions**:
- Order status changed to CANCELLED
- Exchange cancellation request submitted
- Filled portion remains filled (if PARTIALLY_FILLED)

**State Transition Rules**:

| From Status | Can Cancel? | To Status |
|-------------|-------------|-----------|
| PENDING | ✓ Yes | CANCELLED |
| PARTIALLY_FILLED | ✓ Yes | CANCELLED |
| FILLED | ✗ No | - |
| CANCELLED | ✗ No | - |
| REJECTED | ✗ No | - |

**Flow Diagram**:
```
Client            API              Database         Exchange
  |                |                   |                |
  |--DELETE /orders/{id}-------------->|                |
  |                |--Query order----->|                |
  |                |<--Order record----|                |
  |                |--Validate status  |                |
  |                |--Update status--->|                |
  |<--204 No Content                   |                |
  |                |--Cancel order (async)------------->|
```

---

## 4. Market Data Flows

### 4.1 Get Current Price

**Endpoint**: `GET /market/price/{symbol}/`

**References**: REQ-CUR-API-009

**Purpose**: Retrieve current market price for symbol.

**Path Parameters**:
- `symbol`: Trading pair name (e.g., BTCUSDT)

**Success Response** (200 OK):
```json
{
  "symbol": "BTCUSDT",
  "price": "50000.00",
  "timestamp": "2025-11-14T10:30:00Z"
}
```

**Error Responses**:

| Status | Error Code | Description |
|--------|------------|-------------|
| 404 | `not_found` | Symbol not found |

**Preconditions**:
- User authenticated
- Symbol exists in system

**Postconditions**:
- Returns latest price from Binance
- Price timestamp included

**Data Source**:
- **Current**: Direct Binance API call (no caching)
- **Future**: 1-second cache (REQ-FUT-CORE-009)

**Performance**:
- Latency: 100-500ms (depends on Binance API)
- Rate limit: Binance limits apply (see [Known Gap](#73-rate-limiting))

---

## 5. Error Handling

**References**: REQ-CUR-API-010

### 5.1 Error Response Format

**All error responses use consistent JSON structure**:

```json
{
  "error": "error_code",
  "message": "Human-readable error message",
  "details": {
    "field_name": ["Specific validation error"]
  }
}
```

### 5.2 Standard Error Codes

| HTTP Status | Error Code | Usage |
|-------------|------------|-------|
| 400 | `validation_error` | Request validation failed |
| 400 | `invalid_state` | Operation not allowed in current state |
| 401 | `unauthorized` | Missing or invalid authentication |
| 401 | `invalid_credentials` | Login credentials incorrect |
| 401 | `invalid_token` | JWT token invalid or expired |
| 404 | `not_found` | Resource not found or not accessible |
| 500 | `internal_error` | Server error (unexpected) |

### 5.3 Validation Error Example

**Request**:
```json
POST /orders/
{
  "symbol": "BTCUSDT",
  "side": "BUY",
  "type": "LIMIT",
  "quantity": "-0.1",
  "price": null
}
```

**Response** (400 Bad Request):
```json
{
  "error": "validation_error",
  "message": "Invalid request parameters",
  "details": {
    "quantity": ["Must be greater than 0"],
    "price": ["This field is required for LIMIT orders"]
  }
}
```

### 5.4 Authentication Error Example

**Request** (missing Authorization header):
```
GET /orders/
```

**Response** (401 Unauthorized):
```json
{
  "error": "unauthorized",
  "message": "Authentication credentials were not provided",
  "details": {}
}
```

### 5.5 Not Found Error Example

**Request**:
```
GET /orders/invalid-uuid/
```

**Response** (404 Not Found):
```json
{
  "error": "not_found",
  "message": "Order not found",
  "details": {}
}
```

---

## 6. Future Planned Features

**This section describes API features NOT YET IMPLEMENTED.**

### 6.1 Strategy CRUD Endpoints (REQ-FUT-API-001)

**Status**: Planned (Priority: High)

**Endpoints**:
- `GET /strategies/` - List strategies
- `POST /strategies/` - Create strategy
- `GET /strategies/{id}/` - Get strategy
- `PATCH /strategies/{id}/` - Update strategy
- `DELETE /strategies/{id}/` - Delete strategy
- `POST /strategies/{id}/activate/` - Activate strategy
- `POST /strategies/{id}/deactivate/` - Deactivate strategy

**Example Request** (when implemented):
```json
POST /strategies/
{
  "name": "BTC Trend Follower",
  "description": "Follow 50-day MA trend",
  "config": {
    "indicators": ["MA"],
    "entry_condition": "price > ma_50",
    "exit_condition": "price < ma_50"
  },
  "risk_config": {
    "max_position_size": "1000.00",
    "stop_loss_percent": "2.00"
  }
}
```

---

### 6.2 Position CRUD Endpoints (REQ-FUT-API-002)

**Status**: Planned (Priority: High)

**Endpoints**:
- `GET /positions/` - List open positions
- `GET /positions/{id}/` - Get position details
- `POST /positions/{id}/close/` - Manually close position

**Example Response** (when implemented):
```json
GET /positions/
{
  "count": 3,
  "results": [
    {
      "id": "uuid",
      "symbol": "BTCUSDT",
      "side": "LONG",
      "quantity": "0.1",
      "avg_entry_price": "50000.00",
      "current_price": "51000.00",
      "unrealized_pnl": "100.00",
      "unrealized_pnl_percent": "2.00",
      "opened_at": "2025-11-14T10:00:00Z"
    }
  ]
}
```

---

### 6.3 Trade History Endpoints (REQ-FUT-API-003)

**Status**: Planned (Priority: Medium)

**Endpoints**:
- `GET /trades/` - List completed trades
- `GET /trades/{id}/` - Get trade details
- `GET /trades/stats/` - Get aggregate statistics

---

### 6.4 WebSocket Real-Time Updates (REQ-FUT-API-006, 007, 008)

**Status**: Planned (Priority: High)

**WebSocket Endpoint**: `wss://api.robsonbot.com/ws/`

**Events** (when implemented):
- `order.created` - New order placed
- `order.filled` - Order fully filled
- `order.partially_filled` - Partial fill
- `order.cancelled` - Order cancelled
- `price.update` - Symbol price changed
- `position.updated` - Position P&L changed

**Subscription Example** (when implemented):
```json
{
  "action": "subscribe",
  "channel": "prices",
  "symbols": ["BTCUSDT", "ETHUSDT"]
}
```

---

### 6.5 Rate Limiting (REQ-FUT-API-009)

**Status**: Planned (Priority: High)

**Limits** (when implemented):
- 100 requests per minute per user
- 1000 requests per hour per user

**Response Headers** (when implemented):
```
X-RateLimit-Limit: 100
X-RateLimit-Remaining: 87
X-RateLimit-Reset: 1731585900
```

**Error Response** (429 Too Many Requests):
```json
{
  "error": "rate_limit_exceeded",
  "message": "Rate limit exceeded. Please retry after 30 seconds",
  "details": {
    "retry_after": 30
  }
}
```

---

### 6.6 API Versioning (REQ-FUT-API-010)

**Status**: Planned (Priority: Medium)

**URL Versioning** (when implemented):
- `/api/v1/orders/` - Version 1 (current)
- `/api/v2/orders/` - Version 2 (future)

**Breaking Changes**:
- Major version bump required
- v1 maintained for minimum 12 months

---

## 7. Known Gaps and Unclear Behavior

### 7.1 URL Structure Inconsistency

**Gap**: Unclear if endpoints use `/api/` prefix or `/api/v1/`.

**Current State**: OpenAPI spec shows `/api/` prefix; actual implementation may differ.

**Recommendation**:
- Standardize on `/api/v1/` for all endpoints
- Update OpenAPI specification
- Add integration tests verifying URLs

**Tracking**: See REQ-CUR-API requirements section 3.1

---

### 7.2 Order Execution Flow

**Gap**: Order creation (POST /orders/) creates PENDING order, but **execution mechanism unclear**.

**Questions**:
- Is order execution synchronous or asynchronous?
- How does system confirm fill from exchange?
- What triggers status updates (polling vs WebSocket)?

**Current Understanding**:
- Order created with PENDING status
- Background process submits to Binance
- Status updates occur via polling (mechanism not documented)

**Recommendation**:
- Document complete order execution workflow
- Specify polling interval or WebSocket integration
- Add sequence diagrams for order lifecycle

**Tracking**: See REQ-CUR-API requirements section 3.2

---

### 7.3 Rate Limiting

**Gap**: No rate limiting implemented; **Binance API limits apply** but not enforced.

**Risk**: Excessive API calls may trigger Binance rate limits.

**Current State**: No throttling or rate limit headers.

**Recommendation**:
- Implement REQ-FUT-API-009 (rate limiting)
- Add request throttling at API gateway level
- Return 429 responses when limit exceeded

---

### 7.4 Pagination Consistency

**Gap**: Pagination format defined (REQ-CUR-API-011) but **not verified across all endpoints**.

**Current State**: DRF default pagination assumed.

**Recommendation**:
- Audit all list endpoints for pagination
- Add integration tests verifying pagination format
- Document pagination in OpenAPI spec

**Tracking**: See REQ-CUR-API requirements section 3.4

---

## 8. Traceability

### Requirements → Specification Sections

| Requirement ID | Specification Section |
|----------------|-----------------------|
| REQ-CUR-API-001 | [2.1 User Login](#21-user-login) |
| REQ-CUR-API-002 | [2.2 Token Refresh](#22-token-refresh) |
| REQ-CUR-API-003 | [2.3 Authorization Flow](#23-authorization-flow) |
| REQ-CUR-API-004 | [2.3 Authorization Flow](#23-authorization-flow) |
| REQ-CUR-API-005 | [3.1 List Orders](#31-list-orders) |
| REQ-CUR-API-006 | [3.2 Create Order](#32-create-order) |
| REQ-CUR-API-007 | [3.3 Get Order by ID](#33-get-order-by-id) |
| REQ-CUR-API-008 | [3.4 Cancel Order](#34-cancel-order) |
| REQ-CUR-API-009 | [4.1 Get Current Price](#41-get-current-price) |
| REQ-CUR-API-010 | [5. Error Handling](#5-error-handling) |
| REQ-FUT-API-001 | [6.1 Strategy CRUD](#61-strategy-crud-endpoints-req-fut-api-001) |
| REQ-FUT-API-002 | [6.2 Position CRUD](#62-position-crud-endpoints-req-fut-api-002) |
| REQ-FUT-API-003 | [6.3 Trade History](#63-trade-history-endpoints-req-fut-api-003) |
| REQ-FUT-API-006-008 | [6.4 WebSocket Updates](#64-websocket-real-time-updates-req-fut-api-006-007-008) |
| REQ-FUT-API-009 | [6.5 Rate Limiting](#65-rate-limiting-req-fut-api-009) |
| REQ-FUT-API-010 | [6.6 API Versioning](#66-api-versioning-req-fut-api-010) |

### Specification → OpenAPI

This specification will be implemented in `docs/specs/api/openapi.yaml`:

| Specification Section | OpenAPI Path | HTTP Method |
|-----------------------|--------------|-------------|
| 2.1 User Login | `/auth/login` | POST |
| 2.2 Token Refresh | `/auth/refresh` | POST |
| 3.1 List Orders | `/orders` | GET |
| 3.2 Create Order | `/orders` | POST |
| 3.3 Get Order by ID | `/orders/{order_id}` | GET |
| 3.4 Cancel Order | `/orders/{order_id}` | DELETE |
| 4.1 Get Current Price | `/market/price/{symbol}` | GET |

---

**End of Document**
