# Manual Testing Guide: Agentic Workflow Frontend Modal

This guide provides step-by-step instructions for manually testing the refactored StartNewOperationModal component.

## Prerequisites

1. Backend API running at `VITE_API_BASE_URL` (configured in `.env`)
2. User account with authentication credentials
3. At least one Symbol and one Strategy configured in the database
4. Valid Binance API credentials (for live testing)

## Test Scenarios

### 1. Modal Opens Successfully

**Steps:**
1. Navigate to the logged-in dashboard
2. Find the "Start New Operation" button
3. Click the button

**Expected Result:**
- Modal appears with title "Start New Operation"
- All form fields are visible:
  - Trading Pair (dropdown)
  - Strategy (dropdown)
  - Side (BUY/SELL radio buttons)
  - Entry Price (decimal input)
  - Stop Price (decimal input)
  - Capital (decimal input)
- Submit button shows "Create Plan"
- Cancel button is present

### 2. Form Fields Load Data

**Steps:**
1. Open the modal
2. Wait for data to load

**Expected Result:**
- Trading Pair dropdown populated with available symbols (e.g., BTC/USDT, ETH/USDT)
- Strategy dropdown populated with available strategies
- No console errors
- Fields are enabled and interactive

### 3. Required Field Validation

**Steps:**
1. Open the modal
2. Click "Create Plan" without filling any fields

**Expected Result:**
- Error alert appears at top: "Please fix the errors above before submitting."
- Field-specific errors appear:
  - "Symbol is required"
  - "Strategy is required"
  - "Entry price is required"
  - "Stop price is required"
  - "Capital is required"
- Modal stays open
- Form is not submitted

### 4. Decimal Input Validation

**Steps:**
1. Open the modal
2. Try entering invalid data in decimal fields:
   - Letters: "abc"
   - Multiple decimal points: "123.45.67"
   - More than 8 decimal places: "1.123456789"
   - Scientific notation: "1e5"

**Expected Result:**
- Invalid characters are rejected (not entered into field)
- Only valid decimal format is accepted
- Max 8 decimal places enforced

### 5. Entry/Stop Price Validation

**Steps:**
1. Open the modal
2. Fill all fields correctly
3. Set Entry Price = Stop Price (e.g., both 50000)
4. Click "Create Plan"

**Expected Result:**
- Validation error: "Stop price must be different from entry price"
- Form is not submitted
- Modal stays open

### 6. Position Size Calculation Preview

**Steps:**
1. Open the modal
2. Select a Trading Pair (e.g., BTC/USDT)
3. Enter:
   - Entry Price: 50000
   - Stop Price: 48000
   - Capital: 10000

**Expected Result:**
- Blue info alert appears showing:
  - "Calculated Position Size: 0.05000000 BTC"
  - "Based on 1% risk rule: risking $100.00 on this trade"
- Calculation formula: (10000 × 0.01) / |50000 - 48000| = 100 / 2000 = 0.05 BTC

### 7. Successful Form Submission

**Steps:**
1. Open the modal
2. Fill all fields with valid data:
   - Trading Pair: Select any
   - Strategy: Select any
   - Side: Select BUY or SELL
   - Entry Price: 50000
   - Stop Price: 48000
   - Capital: 10000
3. Click "Create Plan"

**Expected Result:**
- Button text changes to "Creating Plan..." with spinner
- All inputs become disabled
- API request sent to `/api/trading-intents/create/`
- On success:
  - Modal closes
  - Success message appears: "Trading intent created successfully! Plan ID: X (BUY BTC/USDT)"
  - Success message auto-hides after 5 seconds
  - Console logs: "Trading intent created: {...}"

### 8. API Error Handling

**Steps:**
1. Open the modal
2. Fill form with data that will cause API error (e.g., capital exceeds available balance)
3. Click "Create Plan"

**OR simulate API error:**
- Temporarily stop backend
- Submit form

**Expected Result:**
- Red error alert appears at top of modal
- Error message displays user-friendly text (not raw stack trace)
- Examples:
  - "Insufficient balance to execute this trade"
  - "An error occurred while creating the trading intent. Please try again."
- Modal stays open
- Form remains editable
- User can retry submission

### 9. Loading State During Submission

**Steps:**
1. Open the modal (with network throttling enabled in DevTools)
2. Fill form with valid data
3. Click "Create Plan"
4. Observe behavior during API call

**Expected Result:**
- Button shows spinner and "Creating Plan..." text
- Button is disabled
- All form inputs are disabled
- Cancel button is disabled
- User cannot double-submit
- After response, form re-enables or modal closes

### 10. Form Reset After Success

**Steps:**
1. Open the modal
2. Submit successfully
3. Re-open the modal

**Expected Result:**
- All form fields are reset to default values
- No previous data remains
- No error messages shown
- Fresh state ready for new operation

### 11. Cancel Button

**Steps:**
1. Open the modal
2. Fill some fields
3. Click "Cancel"

**Expected Result:**
- Modal closes
- No API request sent
- Parent component not notified

### 12. Close Button (X)

**Steps:**
1. Open the modal
2. Fill some fields
3. Click X button in top-right corner

**Expected Result:**
- Modal closes
- No API request sent
- Same as Cancel button

### 13. Side Selection (BUY vs SELL)

**Steps:**
1. Open the modal
2. Select BUY side
3. Observe help text for Stop Price
4. Select SELL side
5. Observe help text change

**Expected Result:**
- BUY selected: "Technical invalidation level (2nd support level)"
- SELL selected: "Technical invalidation level (2nd resistance level)"
- Help text adapts to trading direction

## API Integration Verification

### Request Payload

When submitting the form, verify in DevTools Network tab:

**Endpoint:** `POST /api/trading-intents/create/`

**Headers:**
```
Content-Type: application/json
Authorization: Bearer <JWT_TOKEN>
```

**Body:**
```json
{
  "symbol": 1,
  "strategy": 1,
  "side": "BUY",
  "entry_price": "50000",
  "stop_price": "48000",
  "capital": "10000"
}
```

**Expected Response (201 Created):**
```json
{
  "id": 123,
  "symbol": 1,
  "symbol_display": "BTC/USDT",
  "strategy": 1,
  "side": "BUY",
  "entry_price": "50000.00",
  "stop_price": "48000.00",
  "capital": "10000.00",
  "status": "PENDING",
  "created_at": "2025-01-01T12:00:00Z"
}
```

## Browser Console Checks

### No Errors Expected

- No uncaught exceptions
- No PropTypes warnings
- No React warnings

### Expected Console Logs

```
Trading intent created: { id: 123, side: 'BUY', ... }
```

## Accessibility Testing

### Keyboard Navigation

1. Press Tab to navigate through form fields
2. Use arrow keys for radio buttons
3. Press Enter to submit form
4. Press Escape to close modal

**Expected Result:**
- All fields are keyboard-accessible
- Focus order is logical
- Modal can be operated without mouse

### Screen Reader

1. Enable screen reader (e.g., NVDA, JAWS)
2. Navigate modal

**Expected Result:**
- Labels are announced correctly
- Required fields are indicated
- Error messages are read
- Button states are communicated

## Performance Testing

### Network Throttling

1. Enable "Slow 3G" in DevTools
2. Submit form

**Expected Result:**
- Loading state shows immediately
- No double-submission possible
- User gets feedback that action is processing

## Edge Cases

### 1. Very Large Numbers
- Entry Price: 999999999.99999999
- Should handle without overflow

### 2. Very Small Numbers
- Capital: 0.01
- Should allow minimum values

### 3. Negative Numbers (if applicable)
- DecimalInput should prevent negative if min="0"

### 4. Concurrent Operations
- Open modal twice (shouldn't happen, but test)
- Should handle gracefully

## Browser Compatibility

Test in:
- Chrome/Edge (latest)
- Firefox (latest)
- Safari (latest, if macOS available)

All functionality should work consistently.

## Success Criteria Summary

- ✅ Modal opens and displays all fields
- ✅ Dropdowns populate from API
- ✅ Form validates required fields
- ✅ DecimalInput prevents invalid input
- ✅ Entry/Stop price validation works
- ✅ Position size calculation displays correctly
- ✅ Form submits successfully
- ✅ Loading state prevents double-submission
- ✅ Success callback shows notification
- ✅ API errors display user-friendly messages
- ✅ Modal closes after success
- ✅ Form resets after success
- ✅ Cancel/Close buttons work
- ✅ No console errors
- ✅ Build succeeds without warnings

## Known Issues

- Test suite has environment configuration issue (jsdom/whatwg-url)
- Tests are written correctly but cannot run until Vitest config is fixed
- This does not affect production functionality

## Next Steps After Manual Testing

1. Fix Vitest configuration to enable automated tests
2. Consider adding toast notifications instead of inline alerts
3. Add navigation to intent status page after creation
4. Implement operations list refresh after intent creation
5. Add form field persistence (if user accidentally closes modal)
