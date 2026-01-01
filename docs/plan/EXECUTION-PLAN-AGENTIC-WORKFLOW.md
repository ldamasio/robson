# Execution Plan: Agentic Workflow End-to-End Implementation

**Date**: 2026-01-01
**Status**: Ready to Execute
**Author**: AI Agent (Claude Sonnet 4.5)
**Context**: Complete PLAN → VALIDATE → EXECUTE user journey

---

## 1. Executive Summary

This plan implements the **complete end-to-end user journey** for the Agentic Workflow (PLAN → VALIDATE → EXECUTE), connecting frontend strategy selection to order execution with full transparency and safety.

### 1.1 Current State
- ✅ **Backend**: Fully functional (pattern detection, validation, execution)
- ✅ **CLI**: Works perfectly (`robson plan buy`, `validate`, `execute`)
- ❌ **Frontend**: No integration (modal just closes, no submit)
- ❌ **REST API**: Missing endpoints for plan creation/execution
- ❌ **User Experience**: Can't see PENDING → VALIDATED → EXECUTED flow

### 1.2 Target State
**Complete user journey**:
1. User selects strategy "All In" in Dashboard
2. System creates plan with technical stop calculation
3. User sees validation results (5 guards checked)
4. User confirms execution (DRY-RUN or LIVE)
5. System executes and shows order confirmation
6. **ALL transparent, ALL visible, ALL safe**

### 1.3 Business Value
- ✅ Users understand **WHY** trades happen (transparency)
- ✅ Users see **WHAT** will happen before risking capital (safety)
- ✅ System enforces 1% risk rule automatically (protection)
- ✅ Strategies ("All In", "Rescue Forces") become usable via UI

---

## 2. Architecture Overview

### 2.1 System Components

```
┌─────────────────────────────────────────────────────────────────┐
│                        FRONTEND (React)                         │
├─────────────────────────────────────────────────────────────────┤
│ StartNewOperationModal.jsx   → User selects strategy           │
│    └─ POST /api/intents/create/                                │
│                                                                 │
│ TradingIntentStatus.jsx       → Shows PENDING → EXECUTED       │
│    ├─ GET /api/intents/{id}/status/ (poll every 2s)            │
│    └─ POST /api/intents/{id}/execute/ (when user confirms)     │
│                                                                 │
│ TradingIntentResults.jsx      → Shows validation/execution     │
└─────────────────────────────────────────────────────────────────┘
                        │
                        │ HTTPS/JSON
                        ▼
┌─────────────────────────────────────────────────────────────────┐
│                     REST API (Django)                           │
├─────────────────────────────────────────────────────────────────┤
│ /api/intents/create/              → CreateTradingIntentView    │
│ /api/intents/{id}/execute/        → ExecuteTradingIntentView   │
│ /api/intents/{id}/status/         → TradingIntentStatusView    │
│ /api/intents/{id}/cancel/         → CancelTradingIntentView    │
└─────────────────────────────────────────────────────────────────┘
                        │
                        │ Use Cases
                        ▼
┌─────────────────────────────────────────────────────────────────┐
│                    USE CASES (Hexagonal)                        │
├─────────────────────────────────────────────────────────────────┤
│ CreateTradingIntentUseCase                                      │
│    ├─ Load strategy config                                     │
│    ├─ Calculate technical stop (TechnicalStopService)          │
│    ├─ Calculate position size (1% risk rule)                   │
│    ├─ Create TradingIntent (status: PENDING)                   │
│    └─ Auto-trigger validation                                  │
│                                                                 │
│ ValidatePlanUseCase (EXISTING ✅)                               │
│    ├─ Run guards (balance, limits, risk)                       │
│    ├─ Paper trading simulation                                 │
│    └─ Update status: VALIDATED                                 │
│                                                                 │
│ ExecutePlanUseCase (EXISTING ✅)                                │
│    ├─ Check status == VALIDATED                                │
│    ├─ Place orders on Binance                                  │
│    ├─ Create AuditTransaction records                          │
│    └─ Update status: EXECUTED                                  │
└─────────────────────────────────────────────────────────────────┘
                        │
                        │ Models
                        ▼
┌─────────────────────────────────────────────────────────────────┐
│                      MODELS (Django)                            │
├─────────────────────────────────────────────────────────────────┤
│ TradingIntent (EXISTING ✅)                                     │
│    ├─ intent_id, symbol, strategy, side                        │
│    ├─ quantity, entry_price, stop_price                        │
│    ├─ status: PENDING → VALIDATED → EXECUTED                   │
│    └─ validation_result, execution_result (JSONField)          │
│                                                                 │
│ Strategy (EXISTING ✅)                                          │
│ Order (EXISTING ✅)                                             │
│ AuditTransaction (EXISTING ✅)                                  │
└─────────────────────────────────────────────────────────────────┘
```

---

## 3. Implementation Phases

### Phase 1: Backend REST API (Priority: P0)
**Goal**: Create API endpoints for frontend integration

**Deliverables**:
- `apps/backend/monolith/api/views/trading_intent_views.py` (NEW)
  - `create_trading_intent` (POST /api/intents/create/)
  - `execute_trading_intent` (POST /api/intents/{id}/execute/)
  - `get_trading_intent_status` (GET /api/intents/{id}/status/)
  - `cancel_trading_intent` (POST /api/intents/{id}/cancel/)

- `apps/backend/monolith/api/serializers/trading_intent_serializers.py` (NEW)
  - `CreateTradingIntentSerializer`
  - `TradingIntentSerializer`
  - `ExecuteIntentSerializer`

- `apps/backend/monolith/api/application/use_cases/trading_intent.py` (NEW)
  - `CreateTradingIntentUseCase`
  - Integrates with existing validation/execution

- URL mappings in `api/main_urls.py`

**See**: `docs/plan/prompts/agentic-workflow/01-backend-api.prompt`

---

### Phase 2: Frontend Strategy Selection (Priority: P0)
**Goal**: Make StartNewOperationModal functional

**Deliverables**:
- Update `apps/frontend/src/components/logged/modals/StartNewOperationModal.jsx`
  - Add form state management (strategy, symbol, capital, timeframe)
  - Add validation
  - Add submit handler → POST /api/intents/create/
  - Show loading/error states
  - Navigate to status view after creation

- Update modal styling for better UX

**See**: `docs/plan/prompts/agentic-workflow/02-frontend-modal.prompt`

---

### Phase 3: Frontend Status Tracking (Priority: P0)
**Goal**: Show PENDING → VALIDATED → EXECUTED progression

**Deliverables**:
- `apps/frontend/src/components/logged/TradingIntentStatus.jsx` (NEW)
  - Poll /api/intents/{id}/status/ every 2 seconds
  - Timeline/stepper UI showing progress
  - Show validation results (guards passed/failed)
  - Show execution buttons (DRY-RUN / LIVE)
  - Show execution results (order ID, confirmation)

- `apps/frontend/src/components/logged/TradingIntentResults.jsx` (NEW)
  - Detailed validation results display
  - Guard status indicators
  - Risk calculations display

- Add route in router

**See**: `docs/plan/prompts/agentic-workflow/03-frontend-status.prompt`

---

### Phase 4: Frontend Integration (Priority: P1)
**Goal**: Connect all pieces and improve UX

**Deliverables**:
- Update Dashboard to show recent intents
- Add notifications/toasts for status changes
- Add intent history view
- Update navigation to highlight agentic workflow
- Improve mobile responsiveness

**See**: `docs/plan/prompts/agentic-workflow/04-frontend-integration.prompt`

---

### Phase 5: Pattern Detection Auto-Trigger (Priority: P1)
**Goal**: Connect pattern detection to agentic workflow

**Deliverables**:
- Update `PatternToPlanUseCase` to auto-trigger validation
- Add background worker to process confirmed patterns
- Add configuration for auto-execute vs manual-confirm
- Add user notifications for pattern-triggered intents

**See**: `docs/plan/prompts/agentic-workflow/05-pattern-auto-trigger.prompt`

---

### Phase 6: Testing & Documentation (Priority: P1)
**Goal**: Ensure quality and maintainability

**Deliverables**:
- Backend tests:
  - Unit tests for use cases
  - Integration tests for API endpoints
  - Test fixtures for TradingIntent

- Frontend tests:
  - Component tests (Modal, Status, Results)
  - Integration tests (full flow)
  - E2E tests with Cypress/Playwright

- Documentation:
  - Update API documentation (OpenAPI spec)
  - User guide for agentic workflow
  - Developer guide for extending

**See**: `docs/plan/prompts/agentic-workflow/06-testing-docs.prompt`

---

## 4. Detailed Implementation: Phase 1 (Backend API)

### 4.1 File Structure
```
apps/backend/monolith/api/
├── views/
│   └── trading_intent_views.py          # NEW - REST API views
├── serializers/
│   └── trading_intent_serializers.py    # NEW - DRF serializers
├── application/
│   └── use_cases/
│       └── trading_intent.py            # NEW - Business logic
└── tests/
    └── test_trading_intent_api.py       # NEW - API tests
```

### 4.2 API Endpoints Specification

#### **POST /api/intents/create/**
**Purpose**: Create new trading intent (PLAN step)

**Request**:
```json
{
  "strategy_id": 1,
  "symbol": "BTCUSDT",
  "capital": "100.00",
  "timeframe": "15m",
  "entry_mode": "manual",
  "metadata": {
    "source": "dashboard",
    "user_note": "Strong support at 93500"
  }
}
```

**Response (201 Created)**:
```json
{
  "intent_id": "intent-abc123",
  "status": "PENDING",
  "strategy": {
    "id": 1,
    "name": "All In"
  },
  "symbol": "BTCUSDT",
  "side": "BUY",
  "quantity": "0.000517",
  "entry_price": "95432.10",
  "stop_price": "93500.00",
  "risk_amount": "1.00",
  "risk_percent": "1.0",
  "confidence": "0.9",
  "created_at": "2026-01-01T10:00:00Z"
}
```

**Errors**:
- 400: Invalid input
- 404: Strategy not found
- 500: Technical stop calculation failed

---

#### **GET /api/intents/{intent_id}/status/**
**Purpose**: Get current status and results

**Response (200 OK)**:
```json
{
  "intent_id": "intent-abc123",
  "status": "VALIDATED",
  "strategy": {"id": 1, "name": "All In"},
  "symbol": "BTCUSDT",
  "quantity": "0.000517",
  "entry_price": "95432.10",
  "stop_price": "93500.00",
  "validation_result": {
    "status": "PASS",
    "guards": [
      {"name": "balance_check", "status": "PASS", "message": "Balance sufficient"},
      {"name": "risk_limit", "status": "PASS", "message": "Risk 1% <= 2% limit"},
      {"name": "daily_loss", "status": "PASS", "message": "0% of daily limit used"},
      {"name": "position_limit", "status": "PASS", "message": "0/5 positions open"},
      {"name": "market_hours", "status": "PASS", "message": "Market is open"}
    ],
    "warnings": [],
    "validated_at": "2026-01-01T10:00:05Z"
  },
  "execution_result": null,
  "created_at": "2026-01-01T10:00:00Z",
  "validated_at": "2026-01-01T10:00:05Z",
  "executed_at": null
}
```

---

#### **POST /api/intents/{intent_id}/execute/**
**Purpose**: Execute validated intent

**Request**:
```json
{
  "mode": "LIVE",
  "acknowledge_risk": true
}
```

**Response (200 OK)**:
```json
{
  "intent_id": "intent-abc123",
  "status": "EXECUTED",
  "execution_result": {
    "status": "SUCCESS",
    "mode": "LIVE",
    "orders": [
      {
        "type": "ENTRY",
        "side": "BUY",
        "quantity": "0.000517",
        "price": "95432.10",
        "binance_order_id": "12345678",
        "status": "FILLED"
      },
      {
        "type": "STOP_LOSS",
        "side": "SELL",
        "quantity": "0.000517",
        "stop_price": "93500.00",
        "binance_order_id": "12345679",
        "status": "PENDING"
      }
    ],
    "audit_trail": [
      {"action": "SPOT_BUY", "amount": "49.35", "asset": "USDC"},
      {"action": "STOP_LOSS_PLACED", "stop_price": "93500.00"}
    ],
    "executed_at": "2026-01-01T10:05:00Z"
  }
}
```

**Errors**:
- 400: Not validated yet
- 400: Live mode without acknowledge_risk
- 409: Already executed
- 500: Execution failed

---

#### **POST /api/intents/{intent_id}/cancel/**
**Purpose**: Cancel pending/validated intent

**Response (200 OK)**:
```json
{
  "intent_id": "intent-abc123",
  "status": "CANCELLED",
  "cancelled_at": "2026-01-01T10:05:00Z"
}
```

---

## 5. Detailed Implementation: Phase 2 (Frontend Modal)

### 5.1 Updated Modal Structure

```jsx
// StartNewOperationModal.jsx
import React, { useState, useEffect, useContext } from 'react';
import { useNavigate } from 'react-router-dom';
import AuthContext from '../../../context/AuthContext';

function StartNewOperationModal({ show, onHide }) {
  const navigate = useNavigate();
  const { authTokens } = useContext(AuthContext);

  // Form state
  const [formData, setFormData] = useState({
    strategy_id: '',
    symbol: 'BTCUSDT',
    capital: '100',
    timeframe: '15m',
  });

  // UI state
  const [strategies, setStrategies] = useState([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState(null);

  // Load strategies on mount
  useEffect(() => {
    if (show) {
      fetchStrategies();
    }
  }, [show]);

  const fetchStrategies = async () => {
    try {
      const response = await fetch(
        `${import.meta.env.VITE_API_BASE_URL}/api/strategies/`,
        {
          headers: {
            Authorization: `Bearer ${authTokens.access}`,
            'Content-Type': 'application/json',
          },
        }
      );
      const data = await response.json();
      setStrategies(data.results || data);
    } catch (err) {
      console.error('Failed to load strategies:', err);
    }
  };

  const handleSubmit = async (e) => {
    e.preventDefault();
    setLoading(true);
    setError(null);

    try {
      const response = await fetch(
        `${import.meta.env.VITE_API_BASE_URL}/api/intents/create/`,
        {
          method: 'POST',
          headers: {
            Authorization: `Bearer ${authTokens.access}`,
            'Content-Type': 'application/json',
          },
          body: JSON.stringify({
            ...formData,
            capital: parseFloat(formData.capital),
          }),
        }
      );

      if (!response.ok) {
        const errorData = await response.json();
        throw new Error(errorData.detail || 'Failed to create trading intent');
      }

      const result = await response.json();

      // Navigate to status view
      navigate(`/trading-intent/${result.intent_id}`);
      onHide();
    } catch (err) {
      setError(err.message);
    } finally {
      setLoading(false);
    }
  };

  return (
    <Modal show={show} onHide={onHide}>
      <Modal.Header closeButton>
        <Modal.Title>Start New Operation</Modal.Title>
      </Modal.Header>

      <Form onSubmit={handleSubmit}>
        <Modal.Body>
          {error && <Alert variant="danger">{error}</Alert>}

          <Form.Group className="mb-3">
            <Form.Label>Strategy *</Form.Label>
            <Form.Select
              value={formData.strategy_id}
              onChange={(e) =>
                setFormData({ ...formData, strategy_id: e.target.value })
              }
              required
            >
              <option value="">Select a strategy...</option>
              {strategies.map((s) => (
                <option key={s.id} value={s.id}>
                  {s.name}
                </option>
              ))}
            </Form.Select>
          </Form.Group>

          <Form.Group className="mb-3">
            <Form.Label>Symbol *</Form.Label>
            <Form.Select
              value={formData.symbol}
              onChange={(e) =>
                setFormData({ ...formData, symbol: e.target.value })
              }
            >
              <option value="BTCUSDT">BTC/USDT</option>
              <option value="ETHUSDT">ETH/USDT</option>
              <option value="SOLUSDT">SOL/USDT</option>
            </Form.Select>
          </Form.Group>

          <Row>
            <Col md={6}>
              <Form.Group className="mb-3">
                <Form.Label>Capital (USDC) *</Form.Label>
                <Form.Control
                  type="number"
                  step="0.01"
                  min="1"
                  value={formData.capital}
                  onChange={(e) =>
                    setFormData({ ...formData, capital: e.target.value })
                  }
                  required
                />
              </Form.Group>
            </Col>

            <Col md={6}>
              <Form.Group className="mb-3">
                <Form.Label>Timeframe</Form.Label>
                <Form.Select
                  value={formData.timeframe}
                  onChange={(e) =>
                    setFormData({ ...formData, timeframe: e.target.value })
                  }
                >
                  <option value="5m">5 min</option>
                  <option value="15m">15 min</option>
                  <option value="1h">1 hour</option>
                  <option value="4h">4 hours</option>
                </Form.Select>
              </Form.Group>
            </Col>
          </Row>
        </Modal.Body>

        <Modal.Footer>
          <Button variant="secondary" onClick={onHide} disabled={loading}>
            Cancel
          </Button>
          <Button variant="primary" type="submit" disabled={loading}>
            {loading ? 'Creating Plan...' : 'Create Plan'}
          </Button>
        </Modal.Footer>
      </Form>
    </Modal>
  );
}
```

---

## 6. Success Criteria

### 6.1 Phase 1 (Backend API)
- [ ] All 4 endpoints return correct status codes
- [ ] Request/response schemas match specification
- [ ] Integration with existing validation/execution works
- [ ] Error handling is comprehensive
- [ ] API documented in OpenAPI spec

### 6.2 Phase 2 (Frontend Modal)
- [ ] Modal loads strategies successfully
- [ ] Form validation works
- [ ] Submit creates TradingIntent
- [ ] Loading states show correctly
- [ ] Errors display clearly
- [ ] Navigation to status view works

### 6.3 Phase 3 (Status Tracking)
- [ ] Status polling works (updates every 2s)
- [ ] Timeline shows correct status
- [ ] Validation results display correctly
- [ ] Execute buttons work (DRY-RUN / LIVE)
- [ ] Execution results show order IDs

### 6.4 End-to-End
- [ ] User can select "All In" and create plan
- [ ] User sees validation results with guard status
- [ ] User can execute DRY-RUN successfully
- [ ] User can execute LIVE successfully
- [ ] Order appears in Binance
- [ ] Audit trail is complete

---

## 7. Rollback Plan

If implementation causes issues:

1. **Backend**: Remove URL mappings, delete views file
2. **Frontend**: Revert modal to previous version
3. **Database**: No migrations needed (TradingIntent already exists)

All changes are **additive** and can be rolled back without data loss.

---

## 8. Timeline

| Phase | Estimated Time | Priority |
|-------|---------------|----------|
| Phase 1: Backend API | 3-4 hours | P0 |
| Phase 2: Frontend Modal | 2-3 hours | P0 |
| Phase 3: Status Tracking | 3-4 hours | P0 |
| Phase 4: Integration | 2-3 hours | P1 |
| Phase 5: Pattern Auto-Trigger | 2-3 hours | P1 |
| Phase 6: Testing & Docs | 3-4 hours | P1 |
| **Total** | **15-21 hours** | - |

---

## 9. Next Steps

1. **Review this plan** with stakeholders
2. **Execute Phase 1** (Backend API) using prompt: `docs/plan/prompts/agentic-workflow/01-backend-api.prompt`
3. **Test Phase 1** with Postman/curl
4. **Execute Phase 2** (Frontend Modal)
5. **Execute Phase 3** (Status Tracking)
6. **Test end-to-end** with real strategy execution

---

## 10. References

- Analysis: `docs/USER-JOURNEY-AGENTIC.md`
- Strategies: `docs/STRATEGIES.md`
- Pattern Detection: `api/application/pattern_engine/pattern_to_plan.py`
- Validation: `api/application/validation.py`
- Execution: `api/application/execution.py`
- TradingIntent Model: `api/models/trading.py:TradingIntent`

---

**Status**: Ready for implementation
**Owner**: Development Team
**Start Date**: 2026-01-01
