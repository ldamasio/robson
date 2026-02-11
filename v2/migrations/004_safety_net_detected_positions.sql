-- Migration 004: Safety Net Detected Positions
-- Adds persistence for the safety net's detected rogue positions
--
-- This allows the position monitor to recover its state after restarts,
-- ensuring that detected positions and their execution attempts are not lost.

BEGIN;

-- =============================================================================
-- DETECTED POSITIONS (Safety Net)
-- =============================================================================

CREATE TABLE detected_positions (
    -- Identity
    position_id TEXT PRIMARY KEY,  -- Format: "{symbol}:{side}" e.g., "BTCUSDT:long"

    -- Position Details
    symbol VARCHAR(20) NOT NULL,
    side VARCHAR(10) NOT NULL CHECK (side IN ('long', 'short')),

    -- Entry
    entry_price DECIMAL(20, 8) NOT NULL,
    quantity DECIMAL(20, 8) NOT NULL,

    -- Safety Stop
    stop_price DECIMAL(20, 8) NOT NULL,
    stop_distance DECIMAL(20, 8) NOT NULL,
    stop_distance_pct DECIMAL(10, 6) NOT NULL,

    -- Tracking
    detected_at TIMESTAMPTZ NOT DEFAULT NOW(),
    verified_at TIMESTAMPTZ,  -- Last time position was verified on Binance
    closed_at TIMESTAMPTZ,     -- When position was closed/removed
    is_active BOOLEAN NOT NULL DEFAULT TRUE,

    -- Execution Attempts (Idempotency)
    last_execution_attempt_at TIMESTAMPTZ,
    consecutive_failures INT NOT NULL DEFAULT 0,
    is_panic_mode BOOLEAN NOT NULL DEFAULT FALSE,
    last_error TEXT,

    -- Metadata
    stop_method VARCHAR(20) NOT NULL CHECK (stop_method IN ('fixed_2pct', 'technical'))
);

CREATE INDEX idx_detected_positions_symbol ON detected_positions(symbol);
CREATE INDEX idx_detected_positions_active ON detected_positions(is_active, symbol);
CREATE INDEX idx_detected_positions_detected_at ON detected_positions(detected_at DESC);
CREATE INDEX idx_detected_positions_panic_mode ON detected_positions(is_panic_mode, is_active)
    WHERE is_panic_mode = TRUE;

COMMENT ON TABLE detected_positions IS 'Safety net: Detected rogue positions with calculated safety stops';
COMMENT ON COLUMN detected_positions.position_id IS 'Composite ID: symbol:side';
COMMENT ON COLUMN detected_positions.stop_method IS 'How the stop was calculated (fixed_2pct for safety net)';
COMMENT ON COLUMN detected_positions.is_panic_mode IS 'True if 3+ consecutive execution failures';

-- =============================================================================
-- SAFETY NET EXECUTION LOG
-- =============================================================================

CREATE TABLE safety_net_executions (
    -- Identity
    execution_id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    position_id TEXT NOT NULL,

    -- Execution Details
    symbol VARCHAR(20) NOT NULL,
    side VARCHAR(10) NOT NULL,
    exit_side VARCHAR(10) NOT NULL,

    -- Quantities & Prices
    quantity DECIMAL(20, 8) NOT NULL,
    expected_stop_price DECIMAL(20, 8) NOT NULL,
    execution_price DECIMAL(20, 8),

    -- Order
    exchange_order_id TEXT,

    -- Result
    status VARCHAR(20) NOT NULL CHECK (status IN ('pending', 'success', 'failed', 'panic')),
    error_message TEXT,

    -- Retry Tracking
    attempt_number INT NOT NULL DEFAULT 1,
    is_retry BOOLEAN NOT NULL DEFAULT FALSE,

    -- Audit
    attempted_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    completed_at TIMESTAMPTZ,

    CONSTRAINT fk_safety_executions_position
        FOREIGN KEY (position_id)
        REFERENCES detected_positions(position_id)
        ON DELETE CASCADE
);

CREATE INDEX idx_safety_executions_position ON safety_net_executions(position_id, attempted_at DESC);
CREATE INDEX idx_safety_executions_symbol ON safety_net_executions(symbol, attempted_at DESC);
CREATE INDEX idx_safety_executions_status ON safety_net_executions(status, attempted_at DESC);

COMMENT ON TABLE safety_net_executions IS 'Safety net execution log for audit trail';
COMMENT ON COLUMN safety_net_executions.attempt_number IS 'Retry attempt number (1, 2, 3...)';

COMMIT;
