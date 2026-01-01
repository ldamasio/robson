# ADR-0019: Auto-Trigger Guardrails for Pattern-Based Trading

**Status:** Accepted (Phase 5 MVP Validated ✅)
**Date:** 2026-01-01
**Validation Date:** 2026-01-01
**Context:** Phase 5 - Pattern Auto-Trigger Implementation
**Related:** [ADR-0007](ADR-0007-robson-is-risk-assistant-not-autotrader.md), [ADR-0018](ADR-0018-pattern-detection-engine.md)

---

## MVP Scope Note

**Phase 5 MVP (current implementation):**
- ✅ Trigger endpoint with idempotency
- ✅ Auto-validate (dry-run only)
- ✅ Pattern metadata visibility in UI
- ✅ Hard block on LIVE auto-execution
- ⏳ Rate limits, kill switch, full audit: **TODO (post-MVP)**

This ADR documents the full guardrail vision. Items marked as **TODO (post-MVP)**
are deferred to future phases after the MVP is proven safe and effective.

### Phase 5 Validation Evidence

**Validation Command:** `python manage.py validate_phase5_mvp --client-id 1`

**Validation Results (2026-01-01):**
```
VALIDATION SUMMARY
================================================================================

Total tests: 13
  ✅ PASSED:  10
  ❌ FAILED:  0
  ⏭️  SKIPPED: 3

✅ VALIDATION PASSED

All critical tests passed. Phase 5 MVP is validated.
```

**Key Tests Verified:**
- ✅ Pattern trigger endpoint creates trading intent
- ✅ Idempotency protection (duplicate pattern_event_id → ALREADY_PROCESSED)
- ✅ Auto-validation works correctly (ValidationFramework integration)
- ✅ Pattern metadata stored in TradingIntent (pattern_code, pattern_source, pattern_event_id)
- ✅ PatternTrigger model records all triggers for audit
- ✅ Decimal precision clamping (risk_percent, entry_price, stop_price)

**Test Suite:** See `api/tests/test_pattern_triggers_phase6.py` for comprehensive Phase 6 test coverage.

**Commit:** See git log for validation command implementation (commit: `176be475`)

---

## Context

Phase 5 introduces **pattern auto-trigger**, where detected patterns can automatically
validate and execute trading intents. This is a **high-risk feature** because:

1. **Real money at stake**: Auto-trigger with LIVE mode can place real orders
2. **No human in the loop**: Unlike manual flow, user isn't reviewing each trade
3. **Cascading failures**: Pattern detection bug → multiple bad trades
4. **Regulatory risk**: Automated trading requires safety measures

**Core Principle** (from ADR-0007): Robson is a **risk management assistant**, NOT an
auto-trader. Auto-trigger must respect this principle.

---

## Decision

### 1. Safe Defaults (Non-Negotiable)

**Auto-execute is OFF by default.**
```javascript
// Default configuration
const autoTriggerConfig = {
  autoValidate: false,  // User must explicitly enable
  autoExecute: false,   // User must explicitly enable
  executionMode: 'dry-run',  // DRY-RUN is default, not LIVE
};
```

**Rationale:**
- New users start with manual flow
- Existing behavior unchanged (explicit opt-in)
- Prevents accidental LIVE execution

### 2. Typed Confirmation Remains Mandatory for LIVE

Even with auto-trigger enabled, LIVE execution requires **typed confirmation**.

```javascript
// Pseudo-code for auto-trigger LIVE execution
if (autoExecute && executionMode === 'live') {
  // First attempt: Show confirmation modal with typed input
  const confirmed = await showConfirmationDialog({
    title: 'Confirm LIVE Auto-Execution',
    message: 'Type CONFIRM to proceed with LIVE execution:',
    patternName: pattern.name,
    intentDetails: intent,
  });

  if (!confirmed) {
    // User cancelled or typed wrong
    logger.info('Auto-trigger LIVE execution cancelled by user');
    return;
  }
}
```

**Rationale:**
- User must explicitly approve LIVE trades
- Prevents "fire and forget" LIVE trading
- Regulatory compliance (user affirmation)

### 3. Rate Limits / Safety Limits (Per-User Per-Day) ⏳ TODO (post-MVP)

**Maximum auto-executions per user per day: 10**
```python
# Backend validation (pseudo-code)
MAX_AUTO_EXECUTIONS_PER_DAY = 10

def validate_auto_trigger_limit(client_id):
    today = datetime.now().date()
    count = AutoTriggerEvent.objects.filter(
        client_id=client_id,
        timestamp__date=today,
        event_type='execute'
    ).count()

    if count >= MAX_AUTO_EXECUTIONS_PER_DAY:
        raise AutoTriggerLimitExceeded(
            f"Daily auto-execution limit reached ({MAX_AUTO_EXECUTIONS_PER_DAY})"
        )
```

**Kill switch flag (emergency override):**
```python
# Backend setting
AUTO_TRIGGER_KILL_SWITCH = False  # Global off switch

if AUTO_TRIGGER_KILL_SWITCH:
    logger.warning("Auto-trigger kill switch engaged, blocking all auto-triggers")
    raise AutoTriggerDisabled("Auto-trigger temporarily disabled by admin")
```

**Rationale:**
- Limiting blast radius of bugs/attacks
- Emergency shutdown capability
- Per-user isolation (one user can't affect others)

### 4. Idempotency Protection (Prevent Double-Runs)

**Pattern auto-trigger must be idempotent.**
```python
# Backend validation (pseudo-code)
def validate_idempotency(pattern_id, intent_id):
    # Check if this pattern+intent was already auto-triggered
    existing = AutoTriggerEvent.objects.filter(
        pattern_id=pattern_id,
        intent_id=intent_id,
        event_type__in=['validate', 'execute']
    ).first()

    if existing:
        logger.info(f"Auto-trigger already executed for pattern={pattern_id}, intent={intent_id}")
        raise IdempotencyViolation("This intent was already processed by auto-trigger")
```

**Frontend de-duplication:**
```javascript
// Frontend: Skip if intent already processed
if (intent.auto_triggered) {
  logger.info('Intent already auto-triggered, skipping');
  return;
}
```

**Rationale:**
- Retries/restarts shouldn't double-execute
- Prevents race conditions in distributed systems
- Audit trail integrity

### 5. Audit Events (Who/What/When/Result) ⏳ TODO (post-MVP)

**MVP:** Basic idempotency tracking only (PatternTrigger model).
**Post-MVP:** Full audit logging with detailed AutoTriggerEvent model.

**All auto-trigger events must be logged with full context.**
```python
# Backend audit model
class AutoTriggerEvent(models.Model):
    event_id = models.UUIDField(primary_key=True, default=uuid.uuid4)
    client_id = models.IntegerField()  # Who
    pattern_id = models.IntegerField()  # What pattern
    intent_id = models.UUIDField()  # What intent
    event_type = models.CharField(choices=['validate', 'execute', 'cancel'])
    execution_mode = models.CharField(choices=['dry-run', 'live'])
    status = models.CharField(choices=['started', 'success', 'failed', 'cancelled'])
    error_message = models.TextField(null=True, blank=True)
    timestamp = models.DateTimeField(auto_now_add=True)

    # Risk context
    auto_validate_enabled = models.BooleanField()
    auto_execute_enabled = models.BooleanField()
    user_confirmed_live = models.BooleanField()  # For LIVE mode
```

**Audit query examples:**
```sql
-- How many auto-executions today per user?
SELECT client_id, COUNT(*)
FROM auto_trigger_event
WHERE event_type = 'execute'
  AND DATE(timestamp) = CURRENT_DATE
GROUP BY client_id;

-- Which patterns are triggering most frequently?
SELECT pattern_id, COUNT(*) as trigger_count
FROM auto_trigger_event
GROUP BY pattern_id
ORDER BY trigger_count DESC
LIMIT 10;
```

**Rationale:**
- Regulatory compliance (audit trail)
- Debugging (what happened when)
- Risk monitoring (unusual patterns)

### 6. UX Visibility (Never Silent)

**Auto-trigger activity must always be visible in the UI.**

**Banner on intent status screen (when auto-trigger enabled):**
```jsx
<Alert variant="info" className="mb-3">
  <strong>Auto-Trigger Enabled:</strong> This intent will be automatically
  validated and executed when pattern conditions are met.
  <br />
  <small>Pattern: {pattern.name} | Mode: {executionMode}</small>
</Alert>
```

**Toast notifications for each auto-trigger step:**
```javascript
// Pattern detected
showLoading(`Pattern detected: ${pattern.name}. Validating...`);

// Validation complete
updateLoadingToSuccess(toastId, 'Validation passed. Ready to execute.');

// Execution (dry-run)
updateLoadingToSuccess(toastId, 'Dry-run execution complete. No real orders placed.');

// Execution (LIVE) - requires confirmation
showWarning('LIVE auto-execution: Type CONFIRM to proceed.');
```

**Activity log on dashboard:** ⏳ TODO (post-MVP)
```jsx
<AutoTriggerActivityLog>
  <ActivityItem
    timestamp="2026-01-01 10:23:45"
    pattern="MA4/MA9 Crossover"
    intent="BTCUSDC BUY @ $95000"
    status="success"
    mode="dry-run"
  />
</AutoTriggerActivityLog>
```

**Rationale:**
- User awareness (no silent background activity)
- Trust (transparency builds confidence)
- Control (user can disable if needed)

---

## Consequences

### Positive

1. **Safety first**: Multiple layers prevent accidental LIVE trading
2. **Auditability**: Full traceability of all auto-trigger activity
3. **Compliance**: Meets regulatory requirements for automated trading
4. **User trust**: Transparency and control build confidence
5. **Limiting blast radius**: Per-user limits and kill switch limit damage

### Negative

1. **Friction**: Typed confirmation for LIVE may feel tedious
2. **Development overhead**: Audit logging adds complexity
3. **Maintenance**: Rate limits require monitoring and adjustment

### Mitigations

1. **Friction**: Only require confirmation for LIVE, not dry-run
2. **Development overhead**: Reuse existing audit infrastructure
3. **Maintenance**: Set alerts for limit breaches, review quarterly

---

## Implementation Checklist

### Backend (Django)

**MVP (Phase 5 - Current):**
- [x] Add `PatternTrigger` model for idempotency
- [x] Add idempotency check (prevent double-runs)
- [x] Add pattern trigger endpoint (`POST /api/pattern-triggers/`)
- [x] Hard block on LIVE auto-execution (400 error)
- [x] Add pattern metadata to `TradingIntent`

**Post-MVP:**
- [ ] Add `AutoTriggerEvent` model with full audit fields
- [ ] Add rate limit validation (`MAX_AUTO_EXECUTIONS_PER_DAY`)
- [ ] Add kill switch setting (`AUTO_TRIGGER_KILL_SWITCH`)
- [ ] Add auto-trigger settings endpoints

### Frontend (React)

**MVP (Phase 5 - Current):**
- [x] Add pattern metadata display to intent status screen
- [x] Add toast notifications for pattern trigger events

**Post-MVP:**
- [ ] Add activity log component to dashboard
- [ ] Add auto-trigger settings modal (enable/disable, mode selection)
- [ ] Add confirmation dialog for LIVE auto-execution

### Documentation

**MVP (Phase 5 - Current):**
- [x] Update e2e test doc with pattern trigger steps

**Post-MVP:**
- [ ] Update user docs with auto-trigger safety info
- [ ] Add operations runbook for kill switch usage
- [ ] Add monitoring dashboard for auto-trigger metrics

---

## Alternatives Considered

### Alternative 1: No Rate Limits
**Rejected:** Too risky. A bug could drain accounts in minutes.

### Alternative 2: No Typed Confirmation for LIVE
**Rejected:** Violates regulatory requirements and ADR-0007 principles.

### Alternative 3: Silent Auto-Trigger (No UI Until Complete)
**Rejected:** Users need to see what's happening. Silent execution erodes trust.

---

## References

- [ADR-0007: Robson is Risk Assistant, Not Auto-Trader](ADR-0007-robson-is-risk-assistant-not-autotrader.md)
- [ADR-0018: Pattern Detection Engine](ADR-0018-pattern-detection-engine.md)
- [Phase 5 Prompt](../plan/prompts/agentic-workflow/prompt-05-pattern-auto-trigger.txt)

---

**Approved by:** Engineering Decision Record
**Review date:** 2026-04-01 (6 months post-implementation)
