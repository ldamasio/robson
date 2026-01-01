# User Journey: Agentic Workflow (PLAN → VALIDATE → EXECUTE)

**End-to-end flow from strategy selection to order execution.**

---

## 📊 Current State Analysis

### ✅ What EXISTS

1. **Pattern Detection Engine** ✅
   - Location: `api/application/pattern_engine/`
   - Detects technical patterns (MA crossover, chart patterns, etc.)
   - Creates `PatternInstance` with PENDING → DETECTED → CONFIRMED status

2. **Pattern → Plan Bridge** ✅
   - Location: `api/application/pattern_engine/pattern_to_plan.py`
   - `PatternToPlanUseCase`: Converts CONFIRMED patterns to `TradingIntent`
   - Checks `StrategyPatternConfig` for auto-entry rules
   - Creates intents when conditions match

3. **TradingIntent Model** ✅
   - Location: `api/models/trading.py`
   - Stores trading decisions with full context
   - Status: PENDING → VALIDATED → EXECUTING → EXECUTED
   - Tracks WHY decision was made (regime, confidence, reason)

4. **Validation Framework** ✅
   - Location: `api/application/validation.py`
   - Command: `python manage.py validate_plan`
   - Validates operational + financial constraints
   - Returns PASS/FAIL/WARNING

5. **Execution Framework** ✅
   - Location: `api/application/execution.py`
   - Command: `python manage.py execute_plan`
   - SAFE BY DEFAULT (DRY-RUN default, LIVE requires --acknowledge-risk)
   - Guards: Pre-checks before execution

6. **Strategy Model** ✅
   - Pre-defined strategies: "All In", "Rescue Forces", etc.
   - `config`: Trading parameters
   - `risk_config`: Risk management rules

### ❌ What's MISSING (GAPS)

1. **Frontend → Backend Integration** ❌
   - `StartNewOperationModal`: No submit logic!
   - Button just closes modal, doesn't create plan
   - **GAP**: Need to POST to backend to create `TradingIntent`

2. **REST API for Plan Creation** ❌
   - No `/api/plans/create/` or `/api/intents/create/` endpoint
   - Frontend has nowhere to send the data
   - **GAP**: Need API endpoint

3. **Plan Persistence** ❓ (Unclear)
   - Commands reference `plan_id` but no `ExecutionPlan` model found
   - `TradingIntent` might BE the plan, but unclear
   - **GAP**: Need clear plan storage/retrieval

4. **Frontend Plan Status Tracking** ❌
   - No UI to show "Plan created → Validating → Executing → Done"
   - **GAP**: Need status component

---

## 🎯 Complete End-to-End Flow (HOW IT SHOULD WORK)

### Path 1: **Manual Entry** (User selects strategy in frontend)

```
┌──────────────────────────────────────────────────────────────────┐
│ 1. USER ACTION (Frontend)                                       │
├──────────────────────────────────────────────────────────────────┤
│ Dashboard → "Start New Operation" button                        │
│ ├─ Select strategy: "All In"                                    │
│ ├─ Select symbol: BTC/USDT                                      │
│ ├─ Select timeframe: 15m                                        │
│ ├─ Set capital: $100                                            │
│ └─ Click "Start New Operation"                                  │
└──────────────────────────────────────────────────────────────────┘
                        │
                        │ POST /api/intents/create/
                        ▼
┌──────────────────────────────────────────────────────────────────┐
│ 2. PLAN CREATION (Backend)                                      │
├──────────────────────────────────────────────────────────────────┤
│ CreateTradingIntentUseCase.execute():                           │
│ ├─ Load strategy config ("All In")                              │
│ ├─ Calculate technical stop (call TechnicalStopService)         │
│ │  - Fetch 15m chart                                            │
│ │  - Identify 2nd support level                                │
│ │  - Returns: entry=$95,432, stop=$93,500                       │
│ ├─ Calculate position size (GOLDEN RULE)                        │
│ │  - Risk = $100 × 1% = $1.00                                   │
│ │  - Distance = $95,432 - $93,500 = $1,932                      │
│ │  - Size = $1.00 / $1,932 = 0.000517 BTC                       │
│ └─ Create TradingIntent record                                  │
│    - status: PENDING                                            │
│    - intent_id: "intent-abc123"                                 │
│    - symbol: BTCUSDT                                            │
│    - strategy: "All In"                                         │
│    - quantity: 0.000517                                         │
│    - entry_price: $95,432                                       │
│    - stop_price: $93,500                                        │
│    - confidence: 0.9 (high for manual)                          │
└──────────────────────────────────────────────────────────────────┘
                        │
                        │ Returns: {intent_id: "intent-abc123", status: "PENDING"}
                        ▼
┌──────────────────────────────────────────────────────────────────┐
│ 3. VALIDATION (Backend - Auto-triggered)                        │
├──────────────────────────────────────────────────────────────────┤
│ python manage.py validate_plan --plan-id intent-abc123          │
│                                                                  │
│ ValidatePlanUseCase.execute():                                  │
│ ├─ Load TradingIntent                                           │
│ ├─ Run Guards:                                                  │
│ │  [PASS] ✅ Balance sufficient ($100 available)                │
│ │  [PASS] ✅ Daily loss limit not exceeded (0% used)            │
│ │  [PASS] ✅ Max open positions OK (0/5 open)                   │
│ │  [PASS] ✅ Risk per trade within limits (1% ≤ 2%)             │
│ │  [PASS] ✅ Stop distance reasonable (2% not too tight)        │
│ ├─ Paper Trading Simulation:                                    │
│ │  - Simulates order placement                                 │
│ │  - Checks Binance API limits/permissions                     │
│ │  - Validates symbol is tradable                              │
│ └─ Result: VALIDATION PASSED                                    │
│    - Update TradingIntent.status = VALIDATED                    │
│    - Update TradingIntent.validated_at = now()                  │
└──────────────────────────────────────────────────────────────────┘
                        │
                        │ Returns: {status: "VALIDATED", guards_passed: 5}
                        ▼
┌──────────────────────────────────────────────────────────────────┐
│ 4. USER CONFIRMATION (Frontend)                                 │
├──────────────────────────────────────────────────────────────────┤
│ Show Validation Result:                                         │
│ ┌────────────────────────────────────────────────────────────┐ │
│ │ ✅ Validation PASSED                                       │ │
│ │                                                            │ │
│ │ Entry: 0.000517 BTC @ $95,432                              │ │
│ │ Stop: $93,500 (2.02% below)                                │ │
│ │ Risk: $1.00 (exactly 1% of capital)                        │ │
│ │                                                            │ │
│ │ Guards: ✅ 5/5 passed                                       │ │
│ │                                                            │ │
│ │ [Execute DRY-RUN]  [Execute LIVE] [Cancel]                 │ │
│ └────────────────────────────────────────────────────────────┘ │
│                                                                  │
│ User clicks: [Execute LIVE]                                     │
└──────────────────────────────────────────────────────────────────┘
                        │
                        │ POST /api/intents/intent-abc123/execute/ {mode: "LIVE"}
                        ▼
┌──────────────────────────────────────────────────────────────────┐
│ 5. EXECUTION (Backend)                                          │
├──────────────────────────────────────────────────────────────────┤
│ python manage.py execute_plan --plan-id intent-abc123 \         │
│   --live --acknowledge-risk                                     │
│                                                                  │
│ ExecutePlanUseCase.execute():                                   │
│ ├─ Check TradingIntent.status == VALIDATED ✅                   │
│ ├─ Run Pre-Execution Guards:                                    │
│ │  [PASS] ✅ Still validated                                    │
│ │  [PASS] ✅ Market is open                                     │
│ │  [PASS] ✅ Symbol still tradable                              │
│ ├─ Execute Actions:                                             │
│ │  1. Place BUY order (0.000517 BTC @ market)                  │
│ │     → Binance order ID: 12345678                             │
│ │  2. Place STOP-LOSS order ($93,500)                          │
│ │     → Binance order ID: 12345679                             │
│ │  3. Create AuditTransaction records                          │
│ │  4. Create Operation record                                  │
│ │  5. Link to strategy "All In"                                │
│ └─ Result: EXECUTED                                             │
│    - Update TradingIntent.status = EXECUTED                     │
│    - Update TradingIntent.executed_at = now()                   │
│    - Update TradingIntent.exchange_order_id = 12345678          │
└──────────────────────────────────────────────────────────────────┘
                        │
                        │ Returns: {status: "EXECUTED", order_id: 12345678}
                        ▼
┌──────────────────────────────────────────────────────────────────┐
│ 6. CONFIRMATION (Frontend)                                      │
├──────────────────────────────────────────────────────────────────┤
│ Show Success:                                                    │
│ ┌────────────────────────────────────────────────────────────┐ │
│ │ 🎉 Order Executed Successfully!                            │ │
│ │                                                            │ │
│ │ Strategy: All In                                           │ │
│ │ BUY 0.000517 BTC @ $95,432                                 │ │
│ │ Stop-Loss: $93,500                                         │ │
│ │ Binance Order ID: 12345678                                 │ │
│ │                                                            │ │
│ │ [View Position] [Close]                                    │ │
│ └────────────────────────────────────────────────────────────┘ │
└──────────────────────────────────────────────────────────────────┘
```

---

### Path 2: **Auto Entry** (Pattern detected → Auto-execute)

```
┌──────────────────────────────────────────────────────────────────┐
│ 1. PATTERN DETECTION (Background Worker)                        │
├──────────────────────────────────────────────────────────────────┤
│ python manage.py scan_patterns --timeframe 15m                  │
│                                                                  │
│ PatternDetectionEngine:                                         │
│ ├─ Fetch 15m candles for BTCUSDT                                │
│ ├─ Calculate MA4, MA9                                           │
│ ├─ Detect: MA4 crossed above MA9 ✅                              │
│ ├─ Validate: Short-term uptrend ✅                               │
│ ├─ Create PatternInstance:                                      │
│ │  - pattern: "MA_CROSS_BULLISH"                               │
│ │  - status: DETECTED                                          │
│ │  - confidence: 0.82                                          │
│ └─ Check confirmation criteria... [wait for next candle]        │
└──────────────────────────────────────────────────────────────────┘
                        │
                        │ [Next candle confirms pattern]
                        ▼
┌──────────────────────────────────────────────────────────────────┐
│ 2. PATTERN CONFIRMATION                                         │
├──────────────────────────────────────────────────────────────────┤
│ PatternInstance.status → CONFIRMED                              │
│ PatternAlert created: CONFIRM                                   │
└──────────────────────────────────────────────────────────────────┘
                        │
                        │ Triggers PatternAlertProcessor
                        ▼
┌──────────────────────────────────────────────────────────────────┐
│ 3. PATTERN → PLAN CONVERSION                                    │
├──────────────────────────────────────────────────────────────────┤
│ PatternToPlanUseCase.execute():                                 │
│ ├─ Load StrategyPatternConfig for "Rescue Forces"              │
│ │  - auto_entry_enabled: true ✅                                │
│ │  - min_confidence: 0.75 (pattern has 0.82) ✅                 │
│ ├─ Extract trade parameters from pattern evidence:              │
│ │  - entry_price: $95,450 (current close)                      │
│ │  - stop_price: $95,200 (below MA9)                           │
│ │  - confidence: 0.82                                          │
│ ├─ Create TradingIntent:                                        │
│ │  - strategy: "Rescue Forces"                                 │
│ │  - symbol: BTCUSDT                                           │
│ │  - side: BUY                                                 │
│ │  - status: PENDING                                           │
│ │  - metadata.source: "pattern_detection"                      │
│ └─ Auto-trigger validation                                      │
└──────────────────────────────────────────────────────────────────┘
                        │
                        ▼
[VALIDATION → EXECUTION flow same as Path 1]

(If auto_execute enabled in config → goes straight to EXECUTE after VALIDATE)
```

---

## 🔧 Implementation Gaps & Fixes Needed

### Gap 1: Frontend → Backend Integration

**File**: `apps/frontend/src/components/logged/modals/StartNewOperationModal.jsx`

**Current**: Button just closes modal
```jsx
<Button onClick={props.onHide}>Start New Operation</Button>
```

**Needed**: Submit handler
```jsx
const handleSubmit = async () => {
  const payload = {
    strategy_id: selectedStrategy,
    symbol: selectedSymbol,
    capital: capital,
    timeframe: timeframe,
  };

  const response = await fetch(`${API_URL}/api/intents/create/`, {
    method: 'POST',
    headers: {
      'Authorization': `Bearer ${authTokens.access}`,
      'Content-Type': 'application/json',
    },
    body: JSON.stringify(payload),
  });

  const result = await response.json();
  // Show validation results
  // Allow user to Execute DRY-RUN or LIVE
};
```

---

### Gap 2: REST API Endpoints

**File**: `apps/backend/monolith/api/views/trading_intent_views.py` (NEW)

**Needed**:
```python
@api_view(['POST'])
@permission_classes([IsAuthenticated])
def create_trading_intent(request):
    """
    Create a new trading intent (PLAN step).

    Request body:
        strategy_id: int
        symbol: str (e.g., "BTCUSDT")
        capital: Decimal
        timeframe: str (e.g., "15m")
        entry_mode: str ("manual" or "auto")

    Returns:
        TradingIntent with status PENDING
    """
    # 1. Validate inputs
    # 2. Load strategy config
    # 3. Calculate technical stop (if applicable)
    # 4. Calculate position size
    # 5. Create TradingIntent record
    # 6. Auto-trigger validation
    # 7. Return intent_id + initial status
    pass

@api_view(['POST'])
@permission_classes([IsAuthenticated])
def execute_trading_intent(request, intent_id):
    """
    Execute a VALIDATED intent.

    Request body:
        mode: "DRY_RUN" or "LIVE"
        acknowledge_risk: bool (required for LIVE)

    Returns:
        Execution result with order IDs
    """
    # 1. Load intent, check status == VALIDATED
    # 2. Call execute_plan command
    # 3. Return execution results
    pass

@api_view(['GET'])
@permission_classes([IsAuthenticated])
def get_trading_intent_status(request, intent_id):
    """
    Get current status of intent.

    Returns:
        {
          status: "PENDING" | "VALIDATED" | "EXECUTING" | "EXECUTED",
          validation_result: {...},
          execution_result: {...}
        }
    """
    pass
```

**URL mapping** (`api/main_urls.py`):
```python
# Trading Intents (Agentic Workflow)
path('intents/create/', views.create_trading_intent, name='create_trading_intent'),
path('intents/<str:intent_id>/execute/', views.execute_trading_intent, name='execute_trading_intent'),
path('intents/<str:intent_id>/status/', views.get_trading_intent_status, name='get_trading_intent_status'),
```

---

### Gap 3: Frontend Status Tracking

**File**: `apps/frontend/src/components/logged/TradingIntentStatus.jsx` (NEW)

**Needed**: Component to show plan status
```jsx
function TradingIntentStatus({ intentId }) {
  const [status, setStatus] = useState(null);

  useEffect(() => {
    // Poll /api/intents/{intentId}/status/ every 2s
    const interval = setInterval(async () => {
      const res = await fetch(`${API_URL}/api/intents/${intentId}/status/`);
      const data = await res.json();
      setStatus(data);
    }, 2000);
    return () => clearInterval(interval);
  }, [intentId]);

  if (!status) return <LoadingSpinner />;

  return (
    <Card>
      <Card.Header>
        Trading Intent: {intentId}
      </Card.Header>
      <Card.Body>
        <Timeline>
          <Step active={status.status === 'PENDING'} completed={status.status !== 'PENDING'}>
            1. Plan Created
          </Step>
          <Step active={status.status === 'VALIDATED'} completed={status.status === 'EXECUTED'}>
            2. Validated ({status.validation_result?.guards_passed}/5 guards passed)
          </Step>
          <Step active={status.status === 'EXECUTING'}>
            3. Executing...
          </Step>
          <Step completed={status.status === 'EXECUTED'}>
            4. Executed ✅
          </Step>
        </Timeline>

        {status.status === 'VALIDATED' && (
          <div>
            <Button onClick={() => handleExecute('DRY_RUN')}>
              Execute DRY-RUN
            </Button>
            <Button onClick={() => handleExecute('LIVE')}>
              Execute LIVE
            </Button>
          </div>
        )}

        {status.status === 'EXECUTED' && (
          <Alert variant="success">
            Order executed! Binance ID: {status.execution_result.order_id}
          </Alert>
        )}
      </Card.Body>
    </Card>
  );
}
```

---

### Gap 4: CLI Integration (Already Works!)

**Current** ✅:
```bash
# Via CLI (works today)
robson plan buy BTCUSDT 0.001
robson validate <plan-id> --client-id 1
robson execute <plan-id> --client-id 1 --live --acknowledge-risk
```

**Bridge needed**: Connect CLI flow with Django commands (may already exist via robson-go)

---

## 📋 Implementation Checklist

### Backend
- [ ] Create `trading_intent_views.py` with:
  - [ ] `create_trading_intent`
  - [ ] `execute_trading_intent`
  - [ ] `get_trading_intent_status`
- [ ] Add URL mappings in `main_urls.py`
- [ ] Test APIs with Postman/curl
- [ ] Update OpenAPI spec

### Frontend
- [ ] Update `StartNewOperationModal.jsx`:
  - [ ] Add form state management
  - [ ] Add submit handler
  - [ ] Call `/api/intents/create/`
- [ ] Create `TradingIntentStatus.jsx` component
- [ ] Add status polling logic
- [ ] Add execute buttons (DRY-RUN / LIVE)
- [ ] Show validation results UI
- [ ] Show execution results UI

### Testing
- [ ] Test manual flow: Dashboard → Select strategy → Execute
- [ ] Test auto flow: Pattern detection → Auto-execute
- [ ] Test validation failures (insufficient balance, etc.)
- [ ] Test DRY-RUN mode
- [ ] Test LIVE mode with real orders

---

## 🎯 Summary: The Missing Piece

**What we have**:
- ✅ Pattern detection
- ✅ Pattern → Plan bridge
- ✅ Validation framework
- ✅ Execution framework
- ✅ Strategies with configs

**What's missing**:
- ❌ **Frontend integration** (modal doesn't submit)
- ❌ **REST API endpoints** (nowhere to POST strategy selection)
- ❌ **Status tracking UI** (can't see PENDING → VALIDATED → EXECUTED)

**The gap is the last mile**: We have the engine, but no steering wheel for users.

Once we implement the 3 gaps above, users can:
1. Select "All In" strategy in Dashboard
2. See validation results
3. Click "Execute LIVE"
4. Get order confirmation

**AND** patterns can auto-execute via "Rescue Forces" strategy.

---

**Next Steps**: Implement Gap 1, 2, and 3 to complete the user journey.
