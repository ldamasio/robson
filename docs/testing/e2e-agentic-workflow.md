# E2E Test: Agentic Workflow (PLAN → VALIDATE → EXECUTE)

**Date:** 2026-01-01
**Phase:** 4 - Frontend Integration & UX
**Purpose:** Manual testing checklist for complete agentic workflow

---

## Prerequisites

1. **Test Account Setup**
   - Binance account connected (testnet or production)
   - Sufficient balance for test trades
   - At least one strategy configured (e.g., "All In")

2. **Access**
   - Logged in to Robson Dashboard
   - Navigate to `/dashboard`

---

## Test Scenarios

### Scenario 1: HAPPY PATH - Dry-Run Execution

**Purpose:** Verify complete flow from plan creation to dry-run execution.

| Step | Action | Expected Result |
|------|--------|-----------------|
| 1 | Click "Start New Operation" button | Modal opens with form fields |
| 2 | Select Symbol: `BTCUSDC` | Dropdown shows available symbols |
| 3 | Select Strategy: "All In" | Dropdown shows available strategies |
| 4 | Select Side: `BUY (Long)` | Radio button selected |
| 5 | Enter Entry Price: `95000` | Value accepted |
| 6 | Enter Stop Price: `93500` | Value accepted, different from entry |
| 7 | Enter Capital: `100` | Value accepted, position size calculated |
| 8 | Verify "Calculated Position Size" preview | Shows: ~0.0667 BTC, risking $1.00 (1%) |
| 9 | Click "Create Plan" button | Button shows loading state |
| 10 | Verify: Success toast appears | Toast: "Trading plan created successfully!" |
| 11 | Verify: Navigation to status screen | URL: `/trading-intent/{intentId}` |
| 12 | Verify: Intent status is `PENDING` | Yellow badge: "PENDING" |
| 13 | Verify: Trade details displayed | Symbol, Strategy, Side, Entry, Stop, Quantity shown |
| 14 | Click "Validate Now" button | Button shows loading, then completes |
| 15 | Verify: Validation toast appears | Toast: "Validation passed! Ready to execute." |
| 16 | Verify: Validation section expands | Shows guards with PASS/FAIL icons |
| 17 | Verify: All guards show `✓ PASS` | Green checkmarks for all 5 guards |
| 18 | Click "Dry-Run" mode button | Button becomes primary (selected) |
| 19 | Click "Execute (DRY-RUN)" button | Button shows loading: "Executing..." |
| 20 | Verify: Execution toast appears | Toast: "Dry-run completed. No real orders placed." |
| 21 | Verify: Execution section expands | Shows actions table with simulated results |
| 22 | Verify: Intent status changes to `EXECUTED` | Green badge: "EXECUTED" |
| 23 | Verify: "View in Binance" button hidden | No link (dry-run doesn't place real orders) |

**Result:** ✅ PASS if all steps succeed

---

### Scenario 2: LIVE EXECUTION (CAUTION!)

**Purpose:** Verify live execution flow with safety confirmations.

⚠️ **WARNING:** This will place REAL orders on Binance with REAL money!

| Step | Action | Expected Result |
|------|--------|-----------------|
| 1-10 | Complete Steps 1-10 from Scenario 1 | Intent created successfully |
| 11-16 | Complete Steps 11-16 from Scenario 1 | Validation passed |
| 17 | Click "Live" mode button | Button becomes danger (red, selected) |
| 18 | Verify: Warning toast appears | Toast: "LIVE execution mode: Real orders will be placed!" |
| 19 | Click "Execute (LIVE)" button | Confirmation prompt appears |
| 20 | Verify: Typed confirmation dialog | Prompt: "Type 'CONFIRM' to proceed..." |
| 21 | Type: `CANCEL` (or click Cancel) | Dialog closes, no execution |
| 22 | Verify: Cancellation toast appears | Toast: "Live execution cancelled." |
| 23 | Verify: Intent still in `VALIDATED` state | Status unchanged, can retry |
| 24 | Click "Execute (LIVE)" button again | Confirmation prompt appears |
| 25 | Type: `CONFIRM` | Dialog closes |
| 26 | Verify: Execution toast appears | Toast: "Live execution successful! Orders placed on Binance." |
| 27 | Verify: Execution section expands | Shows actions with real order IDs |
| 28 | Verify: Intent status changes to `EXECUTED` | Green badge: "EXECUTED" |
| 29 | Click "View in Binance" button | Opens Binance order page in new tab |
| 30 | Verify: Order appears on Binance | Real order visible in Binance account |

**Result:** ✅ PASS if all steps succeed (and verify order on Binance)

**Cleanup:** Cancel the order on Binance after testing

---

### Scenario 3: ERROR PATH - Validation Failures

**Purpose:** Verify error handling when validation fails.

| Step | Action | Expected Result |
|------|--------|-----------------|
| 1-10 | Complete Steps 1-10 from Scenario 1 | Intent created successfully |
| 11 | Click "Validate Now" button | Button shows loading, then completes |
| 12 | Verify: Validation section expands | Shows guards with PASS/FAIL icons |
| 13 | Look for FAILED guards (red ✗) | At least one guard shows FAIL status |
| 14 | Verify: Error toast appears (if critical failure) | Toast: "Validation failed. Check details below." |
| 15 | Verify: "Execute" buttons disabled or hidden | Cannot execute failed validation |
| 16 | Read failure reason from guard message | Clear explanation of why validation failed |

**Common Failures:**
- Balance Check FAIL: "Insufficient balance for this trade"
- Risk Limit FAIL: "Monthly risk exceeded (5.2% > 5.0%)"
- Daily Loss FAIL: "Daily loss limit reached"

**Result:** ✅ PASS if errors are clearly communicated

---

### Scenario 4: ERROR PATH - Client-Side Validation

**Purpose:** Verify form validation prevents invalid submissions.

| Step | Action | Expected Result |
|------|--------|-----------------|
| 1 | Click "Start New Operation" button | Modal opens with form fields |
| 2 | Leave all fields empty | N/A |
| 3 | Click "Create Plan" button | Form validation errors appear |
| 4 | Verify: Error messages for required fields | "Symbol is required", "Strategy is required", etc. |
| 5 | Set Entry Price = Stop Price = `50000` | Both fields have same value |
| 6 | Click "Create Plan" button | Validation error: "Stop price must be different from entry price" |
| 7 | Fix: Set Entry Price = `50000`, Stop Price = `49000` | Validation error clears |
| 8 | Set Capital = `-100` | Negative value |
| 9 | Click "Create Plan" button | Validation error: "Capital must be greater than 0" |

**Result:** ✅ PASS if all client-side validations work

---

### Scenario 5: DASHBOARD INTEGRATION

**Purpose:** Verify Trading Plans section on Dashboard.

| Step | Action | Expected Result |
|------|--------|-----------------|
| 1 | Navigate to `/dashboard` | Dashboard loads |
| 2 | Scroll to "Trading Plans" section | Section visible |
| 3 | Verify: Section shows recent intents | Cards with PENDING/VALIDATED intents |
| 4 | Verify: "Refresh" button | Reloads intent list |
| 5 | Verify: "Auto-refresh On/Off" toggle | Toggles auto-refresh every 30s |
| 6 | Click "View Details" on an intent card | Navigates to intent status screen |
| 7 | Click "Validate" button on PENDING intent | Navigates to intent and validates |
| 8 | Click "Execute" button on VALIDATED intent | Navigates to intent and shows execute options |
| 9 | Verify: Empty state (if no intents) | Shows: "No Trading Plans Yet" + CTA button |

**Result:** ✅ PASS if Dashboard integration works smoothly

---

### Scenario 6: MOBILE RESPONSIVENESS

**Purpose:** Verify UI works on mobile devices.

| Step | Action | Expected Result |
|------|--------|-----------------|
| 1 | Resize browser to mobile width (375px) | Layout adapts |
| 2 | Open "Start New Operation" modal | Modal fills screen on mobile |
| 3 | Verify: Form fields stack vertically | Single column layout |
| 4 | Verify: Action buttons full-width | Buttons span full width |
| 5 | Navigate to intent status screen | Content readable on mobile |
| 6 | Verify: TradingIntentStatus component | All sections collapsible/expandable |
| 7 | Verify: Action buttons accessible | Buttons large enough for touch (44px min) |

**Result:** ✅ PASS if mobile experience is smooth

---

### Scenario 7: ERROR RECOVERY

**Purpose:** Verify graceful error recovery.

| Step | Action | Expected Result |
|------|--------|-----------------|
| 1 | Create intent (Scenario 1, Steps 1-10) | Intent created |
| 2 | Disconnect internet (offline mode) | Network unavailable |
| 3 | Click "Validate Now" button | Error toast: "Validation failed. Please try again." |
| 4 | Reconnect internet | Network restored |
| 5 | Click "Refresh" button | Intent list reloads successfully |
| 6 | Click "Validate Now" button | Validation succeeds |

**Result:** ✅ PASS if errors are recoverable with clear messages

---

### Scenario 8: POLLING BEHAVIOR

**Purpose:** Verify automatic polling for status updates.

| Step | Action | Expected Result |
|------|--------|-----------------|
| 1 | Create intent (Scenario 1, Steps 1-10) | Intent created |
| 2 | Navigate to intent status screen | Status: PENDING |
| 3 | Verify: "Live updates enabled" indicator | Visible at top of screen |
| 4 | Click "Validate Now" button | Validation completes |
| 5 | Verify: Status updates automatically | Status changes from PENDING → VALIDATED without refresh |
| 6 | Wait 5+ seconds | No further polling (status is stable) |
| 7 | Click "Execute (Dry-Run)" | Execution completes |
| 8 | Verify: Status updates automatically | Status changes to EXECUTED |
| 9 | Verify: "Refresh now" button | Manual refresh works |

**Result:** ✅ PASS if polling works correctly

---

## Success Criteria

- ✅ All 8 scenarios pass
- ✅ No console errors or warnings
- ✅ All toast notifications clear and actionable
- ✅ Loading states prevent double-submission
- ✅ Mobile experience smooth (no horizontal scroll)
- ✅ Error messages user-friendly (not technical jargon)

---

## Known Issues to Track

1. **Backend API URL routing** (from Phase 1): Test environment URL resolution issue
2. **Vitest jsdom config** (from Phase 2): Test environment configuration needed

---

## Test Environment

- **Browser:** Chrome 120+ (or latest)
- **Screen Sizes:** Desktop (1920x1080), Tablet (768x1024), Mobile (375x667)
- **Network:** Normal 4G throttling (for loading states)
- **Binance Mode:** Testnet (recommended for testing)

---

**Last Updated:** 2026-01-01
**Phase:** 4 - Frontend Integration & UX
**Status:** Ready for testing
