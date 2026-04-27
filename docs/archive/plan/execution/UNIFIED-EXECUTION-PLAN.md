# Unified Execution Plan: Harmonized Trading Intent & Strategic Operations

**Date**: 2026-01-01
**Status**: Proposed (Awaiting Approval)
**Author**: AI Agent (Claude Sonnet 4.5)
**Context**: Unification of Agentic Workflow and Strategic Operations

---

## Executive Summary

This plan unifies two previously separate execution plans:
1. **Agentic Workflow Plan** (TradingIntent: PLAN → VALIDATE → EXECUTE)
2. **Strategic Operations Plan** (Operation lifecycle, monitoring, P&L, stops/targets)

### The Problem

The codebase currently has **two parallel systems** for managing trades:
- **TradingIntent**: Orchestrates the decision workflow (PLAN → VALIDATE → EXECUTE)
- **Operation**: Manages the active trading position (entry, stops, P&L, monitoring)

This creates confusion about:
- Which is the source of truth?
- When is each created?
- How do they relate to each other?
- Which one should the stop monitor use?

### The Solution

**TradingIntent is the ORCHESTRATOR. Operation is the EXECUTION ARTIFACT.**

```
User Decision (manual or pattern-triggered)
    ↓
TradingIntent (PENDING) - "What should we do?"
    ↓
Validation (VALIDATED) - "Is it safe?"
    ↓
TradingIntent (EXECUTING) - "Do it now"
    ↓
Operation CREATED + Orders Placed - "It's happening"
    ↓
TradingIntent (EXECUTED) + Operation (ACTIVE) - "It's live"
    ↓
Stop Monitor watches Operation
    ↓
Operation (CLOSED) - "It's done"
```

**Key Principles:**
1. **Single Source of Truth**: TradingIntent orchestrates the entire lifecycle
2. **Operation Created on Execution**: Operation is ONLY created when orders are actually placed
3. **Stop Monitoring on Operations**: Active positions are monitored via Operation
4. **TradingIntent ↔ Operation Link**: Bidirectional FK for full traceability
5. **User Acknowledgement for LIVE**: Execution mode policies respect safety rules

---

## 1. Repository Sweep (Harmony Audit)

### 1.1 Domain Objects

| Entity | File | Responsibility | Status Fields | Links | Issues |
|--------|------|----------------|---------------|-------|--------|
| **TradingIntent** | `api/models/trading.py:475-650` | **ORCHESTRATOR**: Captures user decision, validates, executes | `PENDING` → `VALIDATED` → `EXECUTING` → `EXECUTED` / `FAILED` / `CANCELLED` | `order` FK (single), `pattern_triggers` reverse | ❌ No link to Operation (conflict!) |
| **Operation** | `api/models/trading.py:228-337` | **EXECUTION ARTIFACT**: Active position management, P&L, monitoring | `PLANNED` → `ACTIVE` → `CLOSED` / `CANCELLED` | `entry_orders` M2M, `exit_orders` M2M | ❌ No link to TradingIntent (conflict!) |
| **Strategy** | `api/models/trading.py:71-130` | **CATALOG**: User-chosen trading approach | `is_active` bool | Referenced by TradingIntent & Operation | ✅ Clear role |
| **Order** | `api/models/trading.py:132-226` | **TRANSACTION**: Individual buy/sell order | `PENDING` → `FILLED` / `CANCELLED` / `REJECTED` | `binance_order_id`, `strategy` FK | ✅ Clear role |
| **Trade** | `api/models/trading.py:401-473` | **HISTORICAL RECORD**: Completed execution | `is_closed` property | `entry_price`, `exit_price`, `pnl` | ✅ Clear role |
| **PatternTrigger** | `api/models/trading.py:652-718` | **IDEMPOTENCY**: Prevents duplicate pattern triggers | `processed` / `failed` | `intent` FK, `pattern_event_id` unique | ✅ Clear role |

**Conflict Identified**: TradingIntent and Operation are **DISCONNECTED**. There's no FK linking them. This violates "single source of truth."

---

### 1.2 Services & Use Cases

| Service/Use Case | File | Responsibility | Current Status |
|------------------|------|----------------|----------------|
| **CreateTradingIntentUseCase** | `api/application/use_cases/trading_intent.py` | Create TradingIntent (PLAN step) | ✅ Validated |
| **ValidationFramework** | `api/application/validation.py` | VALIDATE step (guards, checks) | ✅ Validated |
| **ExecutionFramework** | `api/application/execution.py` | EXECUTE step (DRY-RUN / LIVE) | ✅ Validated |
| **PriceMonitor** | `api/application/stop_monitor.py` | Check operations for stop triggers | ⚠️ Uses Operation only (not TradingIntent) |
| **StopExecutor** | `api/application/stop_monitor.py` | Execute stop-loss/take-profit | ⚠️ Uses Operation only |
| **PositionSizingCalculator** | `api/management/commands/create_user_operation.py` | Calculate size (1% risk rule) | ✅ Exists but duplicated in CreateTradingIntentUseCase |

**Issues**:
- Position sizing logic is **duplicated** (TradingIntent use case AND create_user_operation command)
- Stop monitor only works with **Operation**, ignoring TradingIntent
- No service to **link TradingIntent → Operation** during execution

---

### 1.3 Entry Points

#### CLI Commands (Go)
| Command | File | Purpose | Uses |
|---------|------|---------|------|
| `robson plan buy` | `cli/cmd/agentic.go` | Create TradingIntent | TradingIntent |
| `robson validate` | `cli/cmd/agentic.go` | Validate TradingIntent | TradingIntent |
| `robson execute` | `cli/cmd/agentic.go` | Execute TradingIntent | TradingIntent → ??? (no Operation link) |
| `robson margin-buy` | `cli/cmd/margin.go` | Margin trading | ??? (unclear) |

#### Django Management Commands
| Command | File | Purpose | Uses |
|---------|------|---------|------|
| `monitor_stops` | `api/management/commands/monitor_stops.py` | Monitor & execute stops | **Operation only** |
| `create_user_operation` | `api/management/commands/create_user_operation.py` | User-initiated operation | **Operation only** (bypasses TradingIntent!) |
| `technical_stop_buy` | `api/management/commands/technical_stop_buy.py` | Calculate technical stops | ??? |
| `operations` | `api/management/commands/operations.py` | List operations | **Operation only** |

**Conflict**: `create_user_operation` command creates Operation DIRECTLY, completely bypassing TradingIntent orchestration!

#### REST API Endpoints
| Endpoint | File | Purpose | Entity |
|----------|------|---------|--------|
| `POST /api/intents/create/` | `api/views/trading_intent_views.py` | Create TradingIntent | TradingIntent |
| `POST /api/intents/{id}/execute/` | `api/views/trading_intent_views.py` | Execute TradingIntent | TradingIntent → ??? |
| `GET /api/intents/{id}/status/` | `api/views/trading_intent_views.py` | Get intent status | TradingIntent |
| `POST /api/operations/` | `api/views/user_operations.py` | Create operation | **Operation** (bypasses TradingIntent!) |

**Conflict**: Two separate endpoints to create trades (`/api/intents/` vs `/api/operations/`). Which should users use?

#### Frontend Components
| Component | File | Purpose | Entity |
|-----------|------|---------|--------|
| `StartNewOperationModal` | `apps/frontend/src/components/logged/modals/StartNewOperationModal.jsx` | User creates new trade | Calls `/api/intents/create/` |
| `TradingIntentStatus` | `apps/frontend/src/components/logged/TradingIntentStatus.jsx` | Shows intent progress | TradingIntent |
| `TradingIntentResults` | `apps/frontend/src/components/logged/TradingIntentResults.jsx` | Shows results | TradingIntent |

**Status**: Frontend is **TradingIntent-first** (good!), but backend has dual paths (bad!).

---

### 1.4 Summary of Conflicts

| Issue | Impact | Severity |
|-------|--------|----------|
| **No TradingIntent ↔ Operation link** | Can't trace which Intent created which Operation | 🔴 **CRITICAL** |
| **Duplicate entry points** | `/api/intents/` vs `/api/operations/`, `robson plan` vs `create_user_operation` | 🔴 **CRITICAL** |
| **Stop monitor ignores TradingIntent** | Intents with stops won't be monitored | 🟡 **HIGH** |
| **Duplicated position sizing** | Two implementations of same logic | 🟡 **MEDIUM** |
| **Operation can bypass TradingIntent** | Violates orchestration model | 🔴 **CRITICAL** |

---

## 2. Plan Alignment Matrix (Salvage Map)

This table maps old plans to the unified plan:

| Old Plan Item | Source | Unified Plan Decision | Rationale |
|---------------|--------|----------------------|-----------|
| **TradingIntent workflow (PLAN → VALIDATE → EXECUTE)** | Agentic Plan | ✅ **KEEP AS-IS** (make it primary) | TradingIntent is the orchestrator |
| **CreateTradingIntentUseCase** | Agentic Plan | ✅ **KEEP AS-IS** | Core PLAN step |
| **ValidationFramework** | Agentic Plan | ✅ **KEEP AS-IS** | Core VALIDATE step |
| **ExecutionFramework** | Agentic Plan | ⚠️ **MODIFY**: Must create Operation on execution | Currently doesn't create Operation |
| **Frontend integration (Modal → Status → Results)** | Agentic Plan | ✅ **KEEP AS-IS** (enhance to show Operation link) | Good UX foundation |
| **Pattern auto-trigger** | Agentic Plan | ✅ **KEEP AS-IS** | Creates TradingIntent (correct!) |
| **Operation model** | Strategic Plan | ✅ **KEEP** (as execution artifact) | Needed for active position management |
| **Position sizing calculator** | Strategic Plan | ⚠️ **CONSOLIDATE**: Single implementation | Remove duplication |
| **Stop monitor (PriceMonitor/StopExecutor)** | Strategic Plan | ✅ **KEEP AS-IS** | Monitors Operation (correct after unification) |
| **create_user_operation command** | Strategic Plan | ⚠️ **ASSESS THEN DECIDE**: Scan usage in U5, deprecate only if minimal usage | Safer approach: assess before deprecating |
| **POST /api/operations/ endpoint** | Strategic Plan | ⚠️ **ASSESS THEN DECIDE**: Scan usage in U2, deprecate only if minimal usage | Safer approach: assess before deprecating |
| **Portfolio state tracking** | Strategic Plan | ✅ **KEEP AS-IS** (enhance for P&L) | Needed for dashboard |
| **Trailing stop loss** | Strategic Plan | ⏳ **DEFER to post-unification** | Advanced feature, not critical |
| **Margin support** | Strategic Plan | ⏳ **DEFER to post-unification** | Advanced feature, separate concern |

**Summary**:
- **Keep**: 8 items (TradingIntent workflow, validation, execution, stop monitor, etc.)
- **Modify**: 2 items (ExecutionFramework to create Operation, position sizing consolidation)
- **Assess Then Decide**: 2 items (create_user_operation command, /api/operations/ endpoint - usage scans in U2/U5, deprecation only if minimal usage found)
- **Defer**: 2 items (trailing stops, margin)

**Note**: The "Assess Then Decide" approach ensures we don't break existing usage. Usage scans in U2 and U5 will inform deprecation decisions in U6.

---

## 3. Architecture Decision Record (ADR Proposal)

### ADR: Unified Trading Intent & Operation Model

**Status**: Proposed
**Date**: 2026-01-01
**Context**: Harmonization of TradingIntent (agentic workflow) and Operation (strategic operations)

---

#### Decision A: Source of Truth

**Decision**: TradingIntent is the **ORCHESTRATOR** and **single source of truth** for the trading lifecycle.

**Rationale**:
- TradingIntent captures **user's decision** (what, when, why)
- TradingIntent enforces **validation** before execution
- TradingIntent provides **audit trail** of decision-making
- Operation is an **execution artifact**, created ONLY when execution happens

**Alternatives Considered**:
1. ❌ **Operation as source of truth**: Loses decision context (regime, confidence, reason)
2. ❌ **Dual sources of truth**: Current state, causes conflicts and confusion
3. ✅ **TradingIntent as orchestrator**: Clear hierarchy, single entry point

**Consequences**:
- All user-initiated trades go through TradingIntent
- Direct Operation creation is deprecated
- Operation becomes a **child** of TradingIntent (via FK)

---

#### Decision B: Execution Policy (User-Initiated vs System-Initiated)

**Decision**: Enforce **explicit user acknowledgement** for LIVE execution, with distinct policies for user-initiated vs system-initiated intents.

| Flow Type | Who Creates Intent | Auto-Validate | Auto-Execute | LIVE Requires |
|-----------|-------------------|---------------|--------------|---------------|
| **User-Initiated (Manual)** | User via UI/CLI | ❌ No (user triggers) | ❌ No (user triggers) | Typed "CONFIRM" + `--acknowledge-risk` |
| **Pattern-Triggered (MVP)** | Pattern engine | ✅ Yes (auto-validates to DRY-RUN) | ❌ **BLOCKED** (hard block on LIVE) | N/A (LIVE disabled) |
| **Pattern-Triggered (Future)** | Pattern engine | ✅ Yes | ⚠️ **Opt-in only** (rate-limited) | Typed "CONFIRM" + rate limits |

**Rationale** (per ADR-0007 & ADR-0019):
- Robson is a **risk assistant**, not an auto-trader
- User must approve LIVE execution (regulatory & trust)
- Pattern triggers default to **DRY-RUN** (safe by default)
- LIVE auto-execution is **future feature**, requires strict guardrails

**Safety Rules**:
1. **DRY-RUN is default**: No `--live` flag = simulation only
2. **LIVE requires acknowledgement**: Must type "CONFIRM" or pass `--acknowledge-risk`
3. **Pattern triggers blocked from LIVE**: Hard 400 error in MVP (ADR-0019)
4. **Rate limits**: Max 10 auto-executions/day per user (post-MVP)

---

#### Decision C: Single Risk Boundary (Position Sizing & Validation)

**Decision**: Consolidate position sizing and risk validation into a **single service** called by all entry points.

**Current State** (duplicated):
```python
# In CreateTradingIntentUseCase
quantity = (capital * 0.01) / stop_distance  # Duplicated logic

# In create_user_operation command
quantity = PositionSizingCalculator.calculate(...)  # Duplicated logic
```

**Target State** (unified):
```python
# Single service (new location)
# apps/backend/core/domain/risk/position_sizing.py

class PositionSizingService:
    """Calculates position size using 1% risk rule."""

    def calculate(
        self,
        capital: Decimal,
        entry_price: Decimal,
        stop_price: Decimal,
        side: str,
    ) -> PositionSizeResult:
        """
        Calculate optimal position size (1% risk rule).

        Returns PositionSizeResult with:
        - quantity: Calculated position size
        - position_value: Total value
        - risk_amount: Max loss (USDC)
        - risk_percent: Actual risk %
        """
        stop_distance = abs(entry_price - stop_price)
        risk_amount = capital * Decimal("0.01")  # 1%
        quantity = risk_amount / stop_distance

        return PositionSizeResult(
            quantity=quantity,
            position_value=entry_price * quantity,
            risk_amount=risk_amount,
            risk_percent=(stop_distance / entry_price) * Decimal("100"),
        )
```

**Called by**:
- CreateTradingIntentUseCase (PLAN step)
- Validation guards (risk checks)
- Execution framework (final verification)

**Rationale**:
- **DRY** (Don't Repeat Yourself): One implementation, one truth
- **Consistency**: All entry points use identical calculation
- **Testability**: Single test suite for position sizing
- **Evolvability**: Easy to enhance (e.g., volatility-adjusted sizing)

---

#### Decision D: State Machines & Synchronization

**Decision**: Define clear state transitions and sync rules between TradingIntent and Operation.

##### TradingIntent State Machine

```
PENDING (created)
   ↓
   ├─[validate]─→ VALIDATED (passed validation)
   │                ↓
   │             [execute]
   │                ↓
   │             EXECUTING (orders being placed)
   │                ↓
   │                ├─[success]─→ EXECUTED (orders placed, Operation created)
   │                └─[failure]─→ FAILED (execution error)
   │
   ├─[validation fails]─→ FAILED (blocked by guards)
   └─[user cancels]─→ CANCELLED (user aborted)
```

**Transitions**:
- `PENDING → VALIDATED`: Validation passes (all guards PASS)
- `VALIDATED → EXECUTING`: User confirms execution (or auto-execute in DRY-RUN)
- `EXECUTING → EXECUTED`: Orders placed successfully, Operation created
- `EXECUTING → FAILED`: Order placement fails (exchange error, balance insufficient)
- `PENDING/VALIDATED → CANCELLED`: User cancels intent

**Immutability**: Once `EXECUTED`, status cannot change (Operation tracks ongoing state)

---

##### Operation State Machine

```
(No Operation yet - TradingIntent is still PENDING/VALIDATED)
   ↓
[TradingIntent EXECUTING → orders placed]
   ↓
ACTIVE (position open, entry orders filled)
   ↓
   ├─[stop-loss triggers]─→ CLOSED (position exited)
   ├─[take-profit triggers]─→ CLOSED (position exited)
   ├─[user manually closes]─→ CLOSED (position exited)
   └─[error/emergency]─→ CANCELLED (aborted)
```

**Transitions**:
- **Creation**: Operation is created ONLY when TradingIntent transitions to `EXECUTING`
- `ACTIVE → CLOSED`: Stop monitor triggers exit, or user manually closes
- `ACTIVE → CANCELLED`: Emergency abort (rare)

**Key Rule**: Operation is **NEVER** created until orders are actually being placed.

---

##### Synchronization Rules

| TradingIntent Status | Operation Exists? | Operation Status | Notes |
|---------------------|-------------------|------------------|-------|
| `PENDING` | ❌ No | N/A | Planning stage |
| `VALIDATED` | ❌ No | N/A | Approved but not executed |
| `EXECUTING` | ✅ Yes (just created) | `ACTIVE` | Orders being placed |
| `EXECUTED` | ✅ Yes | `ACTIVE` | Position open, monitored by stop monitor |
| `EXECUTED` (later) | ✅ Yes | `CLOSED` | Position closed by stop monitor |
| `FAILED` | ❌ No | N/A | Execution failed, no Operation created |
| `CANCELLED` | ❌ No | N/A | User cancelled before execution |

**Database Schema Changes**:
```python
# apps/backend/monolith/api/models/trading.py

class TradingIntent(BaseModel):
    # ... existing fields ...

    # NEW: Link to Operation (nullable, set when executed)
    operation = models.OneToOneField(
        'Operation',
        null=True,
        blank=True,
        on_delete=models.SET_NULL,
        related_name='trading_intent',
        help_text='Operation created when this intent was executed'
    )

class Operation(BaseModel):
    # ... existing fields ...

    # NEW: Link back to TradingIntent (nullable for backwards compatibility)
    # In practice, all new Operations should have this set
    # (Only legacy Operations created before unification will have NULL)
    trading_intent = models.OneToOneField(
        'TradingIntent',
        null=True,
        blank=True,
        on_delete=models.SET_NULL,
        related_name='operation_reverse',
        help_text='TradingIntent that created this operation'
    )
```

**Migration Strategy**:
1. Add nullable FKs (backwards compatible)
2. Migrate existing data (if any orphaned Operations exist)
3. Enforce NOT NULL for new records (via application logic, not DB constraint)

---

### Open Questions

1. **Legacy Operations**: What to do with Operations created before unification?
   - **Proposal**: Leave them as-is (NULL trading_intent FK). They'll show in monitoring but without intent context.

2. **Multiple Entries**: Can a single TradingIntent create multiple Operations (e.g., DCA)?
   - **Proposal**: For MVP, 1:1 relationship. Future: 1:N with `operation_set` FK.

3. **Pattern-Triggered LIVE**: When should we enable LIVE auto-execution for patterns?
   - **Proposal**: Post-unification, with strict guardrails (ADR-0019 post-MVP items).

4. **Margin Operations**: How does margin trading fit into this model?
   - **Proposal**: Margin is a **property** of Operation (margin_type, leverage). TradingIntent is agnostic.

---

## 4. Unified Execution Plan (Phases U1-U6)

### Rollout Strategy

**Principle**: Avoid big-bang. Each phase is **independently deployable** and **incrementally valuable**.

**Safety**:
- All changes are **backwards compatible** (nullable FKs, feature flags)
- Each phase has **rollback plan**
- Each phase has **validation tests**

---

### Phase U1: Foundation (Backend Schema & Services)

**Goal**: Establish unified data model and consolidated services.

**Duration**: 1-2 days
**Priority**: P0 (blocks everything else)

#### Deliverables

1. **Database Migration**: Add bidirectional TradingIntent ↔ Operation FKs
   ```bash
   # apps/backend/monolith/api/migrations/XXXX_add_intent_operation_link.py
   ```

2. **Consolidate Position Sizing**:
   - Create `apps/backend/core/domain/risk/position_sizing.py`
   - Move logic from both locations to single service
   - Add comprehensive unit tests

3. **Update CreateTradingIntentUseCase**:
   - Use new PositionSizingService
   - Remove duplicated logic

4. **Update ExecutionFramework**:
   - Add `create_operation_from_intent()` method
   - Set TradingIntent.operation FK on execution
   - Set Operation.trading_intent FK bidirectionally

#### In-Scope
- Schema changes (migration)
- Position sizing consolidation
- Execution framework modification

#### Out-of-Scope
- API endpoints (Phase U2)
- Frontend changes (Phase U3)
- Stop monitor changes (Phase U4)

#### Exit Criteria

**Tests**:
```bash
# Migration works
python manage.py migrate

# Position sizing tests pass
pytest apps/backend/core/tests/test_position_sizing.py -v

# Intent → Operation linking works
pytest apps/backend/monolith/api/tests/test_intent_operation_link.py -v
```

**Validation Command**:
```bash
python manage.py validate_u1_foundation --client-id 1
```

Expected output:
```
✅ Schema: TradingIntent.operation FK exists
✅ Schema: Operation.trading_intent FK exists
✅ Service: PositionSizingService calculates correctly
✅ Service: ExecutionFramework creates Operation on execute
✅ Service: Bidirectional FKs are set correctly
```

#### Rollback Plan
```bash
# Rollback migration
python manage.py migrate api <previous_migration_number>

# Revert code changes via git
git revert <commit_hash>
```

---

### Phase U2: Backend API Consolidation

**Goal**: Establish `/api/intents/` as the canonical entry point with full functionality.

**Duration**: 1-2 days
**Priority**: P0

#### Deliverables

1. **Enhance `/api/intents/execute/` endpoint**:
   - Return Operation ID in response
   - Include Operation details in execution result
   - Ensure full parity with any features in `/api/operations/`

2. **Add `/api/intents/{id}/operation/` endpoint**:
   - GET operation details for a given intent
   - Returns 404 if intent not yet executed

3. **Usage Scan (BEFORE any deprecation)**:
   ```bash
   # Scan codebase for /api/operations/ usage
   grep -r "/api/operations/" apps/frontend/
   grep -r "/api/operations/" cli/
   grep -r "POST.*operations" apps/backend/

   # Check CI/CD references
   grep -r "operations" .github/workflows/
   grep -r "operations" infra/

   # Generate usage report
   python manage.py scan_operations_api_usage > /tmp/operations_usage.txt
   ```

4. **Document Migration Path**:
   - Create `docs/api/MIGRATION-OPERATIONS-TO-INTENTS.md`
   - Map old endpoints to new endpoints
   - Provide example request/response transformations

5. **Update OpenAPI spec**:
   - Document `/api/intents/` as canonical endpoint
   - Keep `/api/operations/` documented (no deprecation yet)
   - Add migration notes

#### In-Scope
- API endpoint enhancements (intents)
- Usage scanning and reporting
- Migration documentation
- OpenAPI spec updates

#### Out-of-Scope
- **NO deprecation in U2** (deferred to U6 after compatibility window)
- **NO removal of `/api/operations/`** (kept fully functional)
- Frontend changes (Phase U3)
- CLI changes (Phase U5)

#### Compatibility Window

**Important**: `/api/operations/` remains **fully functional** through all phases U2-U5.

Deprecation (if needed) happens ONLY in U6, AFTER:
- Usage scan confirms low/zero external usage
- Migration guide tested
- All internal code migrated
- 30-day advance notice given

#### Exit Criteria

**API Tests**:
```bash
# Verify /api/intents/ has full functionality
curl -X POST /api/intents/create/ -d '{ ... }'
# → Returns intent_id

curl -X POST /api/intents/{intent_id}/execute/ -d '{ "mode": "DRY_RUN" }'
# → Returns { ... "operation_id": 123 ... }

curl -X GET /api/intents/{intent_id}/operation/
# → Returns Operation details with full parity

# Verify /api/operations/ still works (backwards compatibility)
curl -X GET /api/operations/
# → Returns operations list (still functional)
```

**Usage Scan**:
```bash
# Generate and review usage report
python manage.py scan_operations_api_usage
# → Shows all references to /api/operations/
# → Identifies which need migration
```

**Validation**:
```bash
pytest apps/backend/monolith/api/tests/test_unified_api.py -v
pytest apps/backend/monolith/api/tests/test_operations_backwards_compat.py -v
```

#### Rollback Plan
- Revert endpoint enhancements (no breaking changes made)
- No deprecation occurred, so nothing to roll back
- `/api/operations/` never touched, remains fully functional

---

### Phase U3: Frontend Integration

**Goal**: Update UI to show unified TradingIntent → Operation flow.

**Duration**: 1-2 days
**Priority**: P0

#### Deliverables

1. **Update TradingIntentResults.jsx**:
   - Show Operation ID when intent is executed
   - Add "View Active Operation" link
   - Display Operation status (ACTIVE / CLOSED)

2. **Update TradingIntentStatus.jsx**:
   - Poll includes Operation status
   - Show Operation details in execution phase
   - Display P&L from Operation

3. **Create OperationDetails component** (optional):
   - Detailed view of active Operation
   - Real-time P&L, current price
   - Stop/target levels

4. **Update StartNewOperationModal**:
   - Rename to "StartNewTradingIntent" (semantic clarity)
   - Update form labels to match TradingIntent

#### In-Scope
- React component updates
- UI/UX for showing Operation link
- Polling logic enhancement

#### Out-of-Scope
- New dashboard features
- Advanced P&L charts (future)

#### Exit Criteria

**Manual Testing**:
1. Create intent via UI
2. Validate intent
3. Execute intent (DRY-RUN)
4. See Operation ID in results
5. See Operation status updates

**E2E Tests**:
```bash
npm run test:e2e -- --spec "TradingIntentUnified"
```

#### Rollback Plan
- Revert frontend code
- Old API still works (backwards compatible)

---

### Phase U4: Stop Monitor Enhancement

**Goal**: Stop monitor uses Operation (already correct), but we validate it works with unified model.

**Duration**: 1 day
**Priority**: P1

#### Deliverables

1. **Validation Tests**:
   - Verify stop monitor finds Operations created from TradingIntent
   - Verify stop execution updates both Operation and TradingIntent
   - Verify audit trail is complete

2. **Enhanced Monitoring**:
   - Log TradingIntent.intent_id in stop monitor logs
   - Include intent context in stop execution notifications

3. **Documentation**:
   - Update stop monitor docs with unified flow
   - Document how to trace intent → operation → stop execution

#### In-Scope
- Testing and validation
- Logging enhancements
- Documentation

#### Out-of-Scope
- Core stop monitor logic changes (already works)
- New stop types (trailing, etc.)

#### Exit Criteria

**Integration Test**:
```bash
# Create intent → execute → trigger stop
python manage.py validate_u4_stop_monitor --client-id 1
```

Expected:
```
✅ Created TradingIntent
✅ Executed intent → Operation created
✅ Simulated price drop → Stop triggered
✅ Stop executor closed Operation
✅ Operation status: CLOSED
✅ TradingIntent execution_result updated
```

#### Rollback Plan
- No schema changes, pure validation phase
- Revert logging changes if needed

---

### Phase U5: CLI Enhancement & Migration Planning

**Goal**: Enhance Go CLI with unified flow; assess `create_user_operation` migration path.

**Duration**: 1 day
**Priority**: P1

#### Deliverables

1. **Update Go CLI** (`cli/cmd/agentic.go`):
   - Ensure `robson plan` → `validate` → `execute` creates Operation
   - Show Operation ID in execution output
   - Add `robson operation status {operation_id}` command

2. **Usage Assessment for `create_user_operation`**:
   ```bash
   # Check if command is used in scripts/automation
   grep -r "create_user_operation" scripts/
   grep -r "create_user_operation" infra/
   grep -r "create_user_operation" docs/

   # Check historical usage (if logging available)
   # Assess impact of potential future deprecation
   ```

3. **Migration Documentation**:
   - Document `robson plan` workflow as recommended approach
   - Create side-by-side comparison guide
   - Keep `create_user_operation` fully functional (no deprecation yet)

4. **Update CLI docs**:
   - Highlight unified workflow as best practice
   - Note that `create_user_operation` remains available

#### In-Scope
- Go CLI enhancements
- Usage assessment and documentation
- Migration guide creation

#### Out-of-Scope
- **NO deprecation warnings in U5** (deferred to U6 if needed)
- **NO removal of `create_user_operation`** (kept fully functional)
- Major CLI refactoring
- New CLI features

#### Compatibility Window

**Important**: `create_user_operation` command remains **fully functional** through U5.

Any deprecation happens ONLY in U6, AFTER:
- Usage assessment confirms low/zero usage
- All internal scripts migrated
- Migration guide validated
- Users notified in advance

#### Exit Criteria

**CLI Tests**:
```bash
# New workflow (recommended)
robson plan buy BTCUSDC 0.001 --client-id 1
# → Returns plan_id

robson execute {plan_id} --client-id 1 --live --acknowledge-risk
# → Returns operation_id

# Old workflow (still works, no warnings yet)
python manage.py create_user_operation --client-id 1 ...
# → Works normally (no deprecation notice)
```

**Usage Assessment**:
```bash
# Generate usage report
./scripts/scan_create_user_operation_usage.sh
# → Shows all references to command
# → Identifies scripts that need migration
```

#### Rollback Plan
- Revert CLI enhancements (no breaking changes)
- No deprecation occurred, nothing to roll back
- `create_user_operation` never touched

---

### Phase U6: Assessment-Based Cleanup & Documentation

**Goal**: Finalize documentation and optionally deprecate based on usage assessment.

**Duration**: 1-2 days
**Priority**: P2

#### Deliverables

1. **Deprecation Decision** (based on U2/U5 usage scans):

   **IF usage scans show zero/minimal usage**:
   - Add deprecation warnings to `/api/operations/` endpoint
   - Add deprecation warnings to `create_user_operation` command
   - Set 30-day deprecation notice period
   - Document migration path prominently

   **IF usage scans show active usage**:
   - Keep endpoints/commands fully functional (NO deprecation)
   - Document both approaches as valid
   - Provide feature parity between both paths
   - Defer deprecation decision to future phase

   **Code Removal** (ONLY if deprecated AND notice period expired):
   - Behind feature flag: `ENABLE_DEPRECATED_OPERATIONS_API`
   - Behind feature flag: `ENABLE_CREATE_USER_OPERATION_COMMAND`
   - Flags default to `True` (keep enabled for safety)

2. **Comprehensive Documentation**:
   - Update CLAUDE.md with unified model
   - Create GLOSSARY.md (TradingIntent vs Operation vs Strategy)
   - Update architecture diagrams
   - Create migration guide for existing code

3. **Validation Suite**:
   - Create `validate_unified_model` command
   - Runs all phase validations
   - Generates health report

4. **Performance Review**:
   - Check query performance (FKs, indexes)
   - Add missing indexes if needed
   - Update monitoring dashboards

#### In-Scope
- Code removal (gated by flag)
- Documentation updates
- Performance optimization

#### Out-of-Scope
- New features
- Major refactoring

#### Exit Criteria

**Full Validation**:
```bash
python manage.py validate_unified_model --client-id 1 --comprehensive
```

Expected:
```
================================================================================
UNIFIED MODEL VALIDATION
================================================================================

Schema Validation:
  ✅ TradingIntent.operation FK exists
  ✅ Operation.trading_intent FK exists
  ✅ Indexes optimized

Service Validation:
  ✅ PositionSizingService (single implementation)
  ✅ ExecutionFramework creates Operation
  ✅ Stop monitor works with unified model

API Validation:
  ✅ /api/intents/ is primary endpoint
  ✅ Deprecated endpoints marked
  ✅ OpenAPI spec updated

CLI Validation:
  ✅ robson plan → validate → execute works
  ✅ Operation ID returned
  ✅ Deprecated commands marked

Frontend Validation:
  ✅ UI shows Operation link
  ✅ Polling includes Operation status
  ✅ E2E tests pass

Integration Validation:
  ✅ Full flow: Create intent → Validate → Execute → Monitor stop
  ✅ Audit trail complete
  ✅ No orphaned records

================================================================================
✅ UNIFIED MODEL VALIDATED
All components working harmoniously.
================================================================================
```

#### Rollback Plan
- Feature flags allow enabling deprecated code
- All changes backwards compatible
- Data integrity maintained

---

## 5. Documentation Updates

### 5.1 Glossary: TradingIntent vs Operation vs Strategy

**What is a TradingIntent?**

A **TradingIntent** is the **orchestrator** of a trading decision. It captures:
- **WHAT**: Symbol, side, quantity, entry/stop prices
- **WHY**: Strategy chosen, market regime, confidence, reason
- **LIFECYCLE**: PENDING → VALIDATED → EXECUTING → EXECUTED / FAILED

Think of it as: **"I want to do this trade, here's my plan, validate it, then execute it."**

**Created by**:
- User via UI (StartNewOperationModal)
- User via CLI (`robson plan buy`)
- Pattern engine (auto-trigger)

**Lives in**: Database, tracked throughout workflow

---

**What is an Operation?**

An **Operation** is the **execution artifact**—the active trading position. It contains:
- **POSITION**: Entry orders, exit orders, quantity, average price
- **RISK**: Stop price, target price (FIXED levels)
- **MONITORING**: Stop monitor checks this for triggers
- **P&L**: Unrealized/realized profit & loss

Think of it as: **"The trade is live, monitor it, show me P&L, execute stops."**

**Created by**: Execution framework ONLY (when TradingIntent is executed)

**Lives in**: Database, monitored by stop monitor

---

**What is a Strategy?**

A **Strategy** is a **catalog entry**—a user-chosen trading approach. It contains:
- **NAME**: "All In", "Rescue Forces", "Mean Reversion MA99"
- **DESCRIPTION**: User's documented trading plan
- **CONFIG**: Reference settings (NOT automation logic)
- **RISK CONFIG**: Risk parameters (max risk %, max drawdown %, etc.)
- **PERFORMANCE**: Total trades, win rate, P&L

Think of it as: **"This is how I trade, here's my track record."**

**Created by**: User (seeded strategies) or system (pre-defined)

**Lives in**: Database (catalog), selected when creating TradingIntent

---

**Relationship**:
```
User selects Strategy "All In"
    ↓
TradingIntent created (references Strategy)
    ↓
TradingIntent validated
    ↓
TradingIntent executed → Operation created (references Strategy)
    ↓
Operation monitored
    ↓
Operation closed → Update Strategy performance
```

---

### 5.2 What We Preserved from the Old Plans

**From Agentic Workflow Plan**:
- ✅ TradingIntent as orchestrator (core concept)
- ✅ PLAN → VALIDATE → EXECUTE workflow (intact)
- ✅ ValidationFramework and ExecutionFramework (reused)
- ✅ Frontend components (modal, status, results)
- ✅ Pattern auto-trigger (creates TradingIntent)
- ✅ Safety defaults (DRY-RUN, user acknowledgement)

**From Strategic Operations Plan**:
- ✅ Operation model (as execution artifact)
- ✅ Stop monitor (PriceMonitor/StopExecutor)
- ✅ Position sizing (1% risk rule)
- ✅ P&L tracking (unrealized/realized)
- ✅ Portfolio state (for dashboard)

**What Changed**:
- ❌ Operation is NO LONGER created directly (now via TradingIntent)
- ❌ Duplicate entry points removed (`create_user_operation`, `/api/operations/`)
- ❌ Duplicate position sizing removed (consolidated to single service)
- ✅ TradingIntent and Operation are LINKED (bidirectional FK)

---

## 6. Success Metrics

| Metric | Target | Measurement |
|--------|--------|-------------|
| **Single Entry Point** | 100% of trades via TradingIntent | Audit log shows 0 direct Operation creations |
| **Link Integrity** | 100% of Operations have TradingIntent FK | DB query: `SELECT COUNT(*) FROM api_operation WHERE trading_intent_id IS NULL AND created_at > '<unification_date>'` = 0 |
| **Position Sizing Accuracy** | 100% match (old vs new calc) | Unit tests + production validation |
| **Stop Monitor Coverage** | 100% of active Operations monitored | Monitor logs show all ACTIVE Operations checked |
| **API Deprecation Adoption** | 90% of clients use `/api/intents/` | API metrics show <10% traffic to `/api/operations/` |
| **Frontend UX** | Users see Operation link in 100% of executions | E2E tests + user feedback |

---

## 7. Risk Mitigation

| Risk | Impact | Mitigation |
|------|--------|------------|
| **Data Loss** | Operations orphaned during migration | Nullable FKs (backwards compatible), rollback plan |
| **API Breaking Change** | Clients using old endpoints break | Deprecation period (6 months), feature flag to disable |
| **Stop Monitor Downtime** | Positions not monitored | No schema changes to Operation, stop monitor unchanged |
| **User Confusion** | UI changes confuse existing users | Gradual rollout, documentation, support guide |
| **Performance Degradation** | New FKs slow down queries | Add indexes, query optimization, load testing |

---

## 8. Rollback Strategy

Each phase has independent rollback:

| Phase | Rollback Action | Data Impact |
|-------|----------------|-------------|
| **U1** | Revert migration | None (nullable FKs) |
| **U2** | Re-enable old endpoints | None (routing change) |
| **U3** | Revert frontend code | None (API backwards compatible) |
| **U4** | Revert logging changes | None (validation only) |
| **U5** | Remove deprecation warnings | None (commands still work) |
| **U6** | Disable feature flag | None (deprecated code re-enabled) |

**Emergency Rollback** (full unification):
```bash
# 1. Revert all code changes
git revert <start_commit>..<end_commit>

# 2. Rollback migrations
python manage.py migrate api <pre_unification_migration>

# 3. Re-enable deprecated endpoints (feature flag)
# In settings.py or environment:
ENABLE_DEPRECATED_OPERATIONS_API = True
ENABLE_CREATE_USER_OPERATION_COMMAND = True

# 4. Restart services
kubectl rollout restart deployment/robson-backend -n production
```

---

## 9. Next Steps (STOP and Ask for Approval)

**THIS IS A PLANNING DOCUMENT. NO IMPLEMENTATION YET.**

Before starting Phase U1, we need:

1. **Stakeholder Review**:
   - Product Owner approval of unified model
   - Engineering team review of architecture decisions
   - Security review of execution policies

2. **User Communication**:
   - Notify users of upcoming changes (if any breaking changes)
   - Prepare migration guide for API clients
   - Update documentation in advance

3. **Approval to Proceed**:
   - [ ] Architecture Decision Record (Section 3) approved
   - [ ] Execution Plan (Section 4) approved
   - [ ] Risk mitigation plan approved
   - [ ] Rollback strategy approved

**QUESTION FOR USER**:

Do you approve this unified execution plan? Should I proceed with Phase U1 (Foundation)?

If approved, I will:
1. Create the database migration (TradingIntent ↔ Operation FKs)
2. Consolidate position sizing into single service
3. Update ExecutionFramework to create Operation on execute
4. Write comprehensive tests
5. Create validation command for U1

If you need changes, please specify which sections to revise.

---

## 10. References

- **Old Plans**:
  - [EXECUTION-PLAN-AGENTIC-WORKFLOW.md](../EXECUTION-PLAN-AGENTIC-WORKFLOW.md) (SUPERSEDED)
  - [EXECUTION-PLAN-STRATEGIC-OPERATIONS.md](../EXECUTION-PLAN-STRATEGIC-OPERATIONS.md) (SUPERSEDED)

- **ADRs**:
  - [ADR-0007: Robson is Risk Assistant, Not Auto-Trader](../../adr/ADR-0007-robson-is-risk-assistant-not-autotrader.md)
  - [ADR-0019: Auto-Trigger Guardrails](../../adr/ADR-0019-auto-trigger-guardrails.md)

- **Domain Models**:
  - `apps/backend/monolith/api/models/trading.py`: TradingIntent, Operation, Strategy, Order
  - `apps/backend/core/domain/trading.py`: Domain entities

- **Frameworks**:
  - `apps/backend/monolith/api/application/validation.py`: ValidationFramework
  - `apps/backend/monolith/api/application/execution.py`: ExecutionFramework
  - `apps/backend/monolith/api/application/stop_monitor.py`: Stop monitor

---

**Document Version**: 1.0
**Last Updated**: 2026-01-01
**Status**: Awaiting Approval
**Next Review**: After stakeholder approval
