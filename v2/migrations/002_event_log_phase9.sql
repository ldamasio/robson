-- Migration: Phase 9 - Event Log + Projections + Snapshots
-- Version: 002
-- Created: 2024-01-15
-- Description: Implements append-only event log with projections for audit trail,
--              replay capability, and deterministic state reconstruction.
--
-- Key Features:
-- - Partitioned event log (monthly by ingested_at)
-- - Projection tables for current state (orders, positions, balances, risk, strategy)
-- - Snapshot system for optimization
-- - Idempotency via semantic payload hashing
-- - Multi-tenant isolation
-- - S3 archival support

BEGIN;

-- =============================================================================
-- 1. EVENT LOG (Append-Only, Partitioned)
-- =============================================================================

CREATE TABLE event_log (
    -- Identity
    event_id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID NOT NULL,

    -- Stream Partitioning
    stream_key TEXT NOT NULL,  -- "account:{uuid}" | "position:{uuid}" | "order:{uuid}" | "strategy:{uuid}"
    seq BIGINT NOT NULL,       -- Monotonic per stream_key

    -- Event Type & Data
    event_type VARCHAR(100) NOT NULL,
    payload JSONB NOT NULL,
    payload_schema_version INT NOT NULL DEFAULT 1,

    -- Temporal
    occurred_at TIMESTAMPTZ NOT NULL,
    ingested_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    -- Idempotency
    idempotency_key TEXT NOT NULL,

    -- Correlation
    trace_id UUID,
    causation_id UUID,
    command_id UUID,
    workflow_id UUID,

    -- Actor
    actor_type VARCHAR(20),
    actor_id TEXT,

    -- Audit (optional - for tamper-evident chain)
    prev_hash TEXT,
    hash TEXT,

    -- Constraints
    CONSTRAINT uk_event_log_stream_seq UNIQUE (stream_key, seq),
    CONSTRAINT uk_event_log_idempotency_key UNIQUE (idempotency_key),
    CONSTRAINT chk_actor_type CHECK (actor_type IN ('CLI', 'Daemon', 'System', 'Exchange'))
) PARTITION BY RANGE (ingested_at);

-- Indexes (applied to all partitions)
CREATE INDEX idx_event_log_tenant_occurred ON event_log(tenant_id, occurred_at DESC);
CREATE INDEX idx_event_log_stream_seq ON event_log(stream_key, seq DESC);
CREATE INDEX idx_event_log_event_type ON event_log(tenant_id, event_type, occurred_at DESC);
CREATE INDEX idx_event_log_trace_id ON event_log(trace_id) WHERE trace_id IS NOT NULL;
CREATE INDEX idx_event_log_command_id ON event_log(command_id) WHERE command_id IS NOT NULL;
CREATE INDEX idx_event_log_workflow_id ON event_log(workflow_id) WHERE workflow_id IS NOT NULL;

-- GIN index for JSONB payload search
CREATE INDEX idx_event_log_payload_gin ON event_log USING GIN (payload jsonb_path_ops);

-- Composite indexes for common queries
CREATE INDEX idx_event_log_position_events ON event_log(tenant_id, (payload->>'position_id'), occurred_at DESC)
    WHERE payload ? 'position_id';
CREATE INDEX idx_event_log_order_events ON event_log(tenant_id, (payload->>'order_id'), occurred_at DESC)
    WHERE payload ? 'order_id';

-- Create initial partitions (current month + 2 future months)
CREATE TABLE event_log_2024_01 PARTITION OF event_log
    FOR VALUES FROM ('2024-01-01') TO ('2024-02-01');
CREATE TABLE event_log_2024_02 PARTITION OF event_log
    FOR VALUES FROM ('2024-02-01') TO ('2024-03-01');
CREATE TABLE event_log_2024_03 PARTITION OF event_log
    FOR VALUES FROM ('2024-03-01') TO ('2024-04-01');

COMMENT ON TABLE event_log IS 'Append-only event log for audit trail and state reconstruction';
COMMENT ON COLUMN event_log.stream_key IS 'Logical partition key (e.g., position:{uuid})';
COMMENT ON COLUMN event_log.seq IS 'Monotonic sequence number per stream_key for ordering';
COMMENT ON COLUMN event_log.idempotency_key IS 'Hash of semantic payload for deduplication';

-- =============================================================================
-- 2. PROJECTION TABLES (Current State)
-- =============================================================================

-- 2.1 Orders Current
CREATE TABLE orders_current (
    -- Identity
    order_id UUID PRIMARY KEY,
    tenant_id UUID NOT NULL,
    account_id UUID NOT NULL,
    position_id UUID,

    -- Exchange Mapping
    exchange_order_id TEXT,
    client_order_id TEXT NOT NULL,
    exchange_trade_ids TEXT[] DEFAULT ARRAY[]::TEXT[],

    -- Order Details
    symbol VARCHAR(20) NOT NULL,
    side VARCHAR(10) NOT NULL CHECK (side IN ('buy', 'sell')),
    order_type VARCHAR(20) NOT NULL CHECK (order_type IN ('market', 'limit', 'stop_loss', 'stop_loss_limit')),

    -- Quantities & Prices
    quantity DECIMAL(20, 8) NOT NULL,
    price DECIMAL(20, 8),
    stop_price DECIMAL(20, 8),

    -- Fills
    filled_quantity DECIMAL(20, 8) DEFAULT 0,
    average_fill_price DECIMAL(20, 8),
    total_fee DECIMAL(20, 8) DEFAULT 0,
    fee_asset VARCHAR(10),

    -- Status
    status VARCHAR(20) NOT NULL CHECK (status IN ('pending', 'submitted', 'acknowledged', 'partial', 'filled', 'canceled', 'rejected', 'expired')),

    -- Event Sourcing
    last_event_id UUID NOT NULL,
    last_seq BIGINT NOT NULL,

    -- Audit
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    filled_at TIMESTAMPTZ,

    CONSTRAINT uk_orders_client_order_id UNIQUE (client_order_id),
    CONSTRAINT uk_orders_exchange_order_id UNIQUE (exchange_order_id) WHERE exchange_order_id IS NOT NULL
);

CREATE INDEX idx_orders_tenant_account ON orders_current(tenant_id, account_id);
CREATE INDEX idx_orders_position ON orders_current(position_id) WHERE position_id IS NOT NULL;
CREATE INDEX idx_orders_status ON orders_current(status);
CREATE INDEX idx_orders_symbol ON orders_current(symbol);
CREATE INDEX idx_orders_updated ON orders_current(updated_at DESC);

COMMENT ON TABLE orders_current IS 'Projection: Current state of all orders';

-- 2.2 Positions Current
CREATE TABLE positions_current (
    -- Identity
    position_id UUID PRIMARY KEY,
    tenant_id UUID NOT NULL,
    account_id UUID NOT NULL,
    strategy_id UUID,

    -- Position Details
    symbol VARCHAR(20) NOT NULL,
    side VARCHAR(10) NOT NULL CHECK (side IN ('long', 'short')),

    -- Entry
    entry_price DECIMAL(20, 8),
    entry_quantity DECIMAL(20, 8),
    entry_filled_at TIMESTAMPTZ,

    -- Technical Stop (Golden Rule)
    technical_stop_price DECIMAL(20, 8),
    technical_stop_distance DECIMAL(20, 8),

    -- Current State
    current_quantity DECIMAL(20, 8) NOT NULL,
    current_price DECIMAL(20, 8),
    trailing_stop_price DECIMAL(20, 8),

    -- P&L
    unrealized_pnl DECIMAL(20, 8),
    realized_pnl DECIMAL(20, 8) DEFAULT 0,
    total_fees DECIMAL(20, 8) DEFAULT 0,

    -- Status
    state VARCHAR(20) NOT NULL CHECK (state IN ('armed', 'entering', 'active', 'exiting', 'closed', 'error')),

    -- Order References
    entry_order_id UUID,
    exit_order_id UUID,
    stop_loss_order_id UUID,

    -- Event Sourcing
    last_event_id UUID NOT NULL,
    last_seq BIGINT NOT NULL,

    -- Audit
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    closed_at TIMESTAMPTZ,

    CONSTRAINT fk_positions_entry_order FOREIGN KEY (entry_order_id) REFERENCES orders_current(order_id) ON DELETE SET NULL,
    CONSTRAINT fk_positions_exit_order FOREIGN KEY (exit_order_id) REFERENCES orders_current(order_id) ON DELETE SET NULL
);

CREATE INDEX idx_positions_tenant_account ON positions_current(tenant_id, account_id);
CREATE INDEX idx_positions_strategy ON positions_current(strategy_id) WHERE strategy_id IS NOT NULL;
CREATE INDEX idx_positions_state ON positions_current(state);
CREATE INDEX idx_positions_symbol ON positions_current(symbol);
CREATE INDEX idx_positions_updated ON positions_current(updated_at DESC);
CREATE INDEX idx_positions_active ON positions_current(tenant_id, state, symbol)
    WHERE state IN ('armed', 'entering', 'active', 'exiting');

COMMENT ON TABLE positions_current IS 'Projection: Current state of all positions';

-- 2.3 Balances Current
CREATE TABLE balances_current (
    -- Identity
    balance_id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID NOT NULL,
    account_id UUID NOT NULL,

    -- Asset
    asset VARCHAR(10) NOT NULL,

    -- Balances
    free DECIMAL(20, 8) NOT NULL,
    locked DECIMAL(20, 8) NOT NULL,
    total DECIMAL(20, 8) GENERATED ALWAYS AS (free + locked) STORED,

    -- Event Sourcing
    last_event_id UUID NOT NULL,
    last_seq BIGINT NOT NULL,

    -- Audit
    sampled_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,

    CONSTRAINT uk_balances_account_asset UNIQUE (account_id, asset)
);

CREATE INDEX idx_balances_tenant ON balances_current(tenant_id);
CREATE INDEX idx_balances_account ON balances_current(account_id);
CREATE INDEX idx_balances_updated ON balances_current(updated_at DESC);

COMMENT ON TABLE balances_current IS 'Projection: Latest sampled balances from exchange';

-- 2.4 Risk State Current
CREATE TABLE risk_state_current (
    -- Identity
    risk_id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID NOT NULL,
    account_id UUID NOT NULL,
    strategy_id UUID,

    -- Risk Metrics
    total_exposure DECIMAL(20, 8) NOT NULL DEFAULT 0,
    max_exposure DECIMAL(20, 8) NOT NULL,

    daily_pnl DECIMAL(20, 8) NOT NULL DEFAULT 0,
    daily_loss_limit DECIMAL(20, 8) NOT NULL,

    drawdown DECIMAL(8, 4) NOT NULL DEFAULT 0,
    max_drawdown DECIMAL(8, 4) NOT NULL,

    -- Violations
    is_violated BOOLEAN DEFAULT FALSE,
    violation_reason TEXT,
    violated_at TIMESTAMPTZ,

    -- Event Sourcing
    last_event_id UUID NOT NULL,
    last_seq BIGINT NOT NULL,

    -- Audit
    calculated_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,

    CONSTRAINT uk_risk_account_strategy UNIQUE (account_id, strategy_id)
);

CREATE INDEX idx_risk_tenant ON risk_state_current(tenant_id);
CREATE INDEX idx_risk_account ON risk_state_current(account_id);
CREATE INDEX idx_risk_strategy ON risk_state_current(strategy_id) WHERE strategy_id IS NOT NULL;
CREATE INDEX idx_risk_violated ON risk_state_current(is_violated) WHERE is_violated = TRUE;

COMMENT ON TABLE risk_state_current IS 'Projection: Current risk state per account/strategy';

-- 2.5 Strategy State Current
CREATE TABLE strategy_state_current (
    -- Identity
    strategy_id UUID PRIMARY KEY,
    tenant_id UUID NOT NULL,
    account_id UUID NOT NULL,

    -- Strategy Config
    strategy_name TEXT NOT NULL,
    strategy_type VARCHAR(50) NOT NULL,
    detector_config JSONB,
    risk_config JSONB NOT NULL,

    -- State
    is_enabled BOOLEAN DEFAULT FALSE,
    enabled_at TIMESTAMPTZ,
    disabled_at TIMESTAMPTZ,
    disabled_reason TEXT,

    -- Performance Metrics
    total_signals INT DEFAULT 0,
    total_positions INT DEFAULT 0,
    open_positions INT DEFAULT 0,

    total_pnl DECIMAL(20, 8) DEFAULT 0,
    win_rate DECIMAL(5, 4),

    -- Event Sourcing
    last_event_id UUID NOT NULL,
    last_seq BIGINT NOT NULL,

    -- Audit
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL
);

CREATE INDEX idx_strategy_tenant ON strategy_state_current(tenant_id);
CREATE INDEX idx_strategy_account ON strategy_state_current(account_id);
CREATE INDEX idx_strategy_enabled ON strategy_state_current(is_enabled) WHERE is_enabled = TRUE;
CREATE INDEX idx_strategy_updated ON strategy_state_current(updated_at DESC);

COMMENT ON TABLE strategy_state_current IS 'Projection: Current state of all strategies';

-- =============================================================================
-- 3. SNAPSHOTS (Optimization)
-- =============================================================================

CREATE TABLE snapshots (
    -- Identity
    snapshot_id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID NOT NULL,

    -- Scope
    snapshot_scope VARCHAR(20) NOT NULL CHECK (snapshot_scope IN ('account', 'strategy', 'position')),
    scope_id UUID NOT NULL,

    -- Event Sourcing Position
    as_of_event_id UUID NOT NULL,
    as_of_seq BIGINT NOT NULL,
    as_of_time TIMESTAMPTZ NOT NULL,

    -- Snapshot Data
    snapshot_type VARCHAR(50) NOT NULL,
    snapshot_payload JSONB NOT NULL,

    -- Compression (optional)
    is_compressed BOOLEAN DEFAULT FALSE,
    compression_algo VARCHAR(20),

    -- Audit
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    CONSTRAINT uk_snapshots_scope_time UNIQUE (snapshot_scope, scope_id, as_of_time)
) PARTITION BY RANGE (created_at);

CREATE INDEX idx_snapshots_tenant ON snapshots(tenant_id);
CREATE INDEX idx_snapshots_scope ON snapshots(snapshot_scope, scope_id, as_of_time DESC);
CREATE INDEX idx_snapshots_event ON snapshots(as_of_event_id);
CREATE INDEX idx_snapshots_created ON snapshots(created_at DESC);

-- Create initial snapshot partitions
CREATE TABLE snapshots_2024_01 PARTITION OF snapshots
    FOR VALUES FROM ('2024-01-01') TO ('2024-02-01');
CREATE TABLE snapshots_2024_02 PARTITION OF snapshots
    FOR VALUES FROM ('2024-02-01') TO ('2024-03-01');
CREATE TABLE snapshots_2024_03 PARTITION OF snapshots
    FOR VALUES FROM ('2024-03-01') TO ('2024-04-01');

COMMENT ON TABLE snapshots IS 'Periodic snapshots for optimization of replay';

-- =============================================================================
-- 4. SUPPORTING TABLES
-- =============================================================================

-- 4.1 Stream State (Sequence Generation)
CREATE TABLE stream_state (
    stream_key TEXT PRIMARY KEY,
    tenant_id UUID NOT NULL,
    last_seq BIGINT NOT NULL DEFAULT 0,
    last_event_id UUID,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_stream_state_tenant ON stream_state(tenant_id);

COMMENT ON TABLE stream_state IS 'Tracks last sequence number per stream for ordering';

-- 4.2 Commands (Idempotency)
CREATE TABLE commands (
    command_id UUID PRIMARY KEY,
    tenant_id UUID NOT NULL,
    command_type VARCHAR(50) NOT NULL,
    payload JSONB NOT NULL,

    -- Idempotency
    idempotency_hash TEXT GENERATED ALWAYS AS (
        md5(tenant_id::text || command_type || payload::text)
    ) STORED,

    -- Status
    status VARCHAR(20) NOT NULL CHECK (status IN ('pending', 'processing', 'completed', 'failed')),
    result JSONB,
    error TEXT,

    -- Correlation
    trace_id UUID NOT NULL,

    -- Audit
    issued_at TIMESTAMPTZ NOT NULL,
    started_at TIMESTAMPTZ,
    completed_at TIMESTAMPTZ,

    CONSTRAINT uk_commands_idempotency UNIQUE (tenant_id, idempotency_hash)
);

CREATE INDEX idx_commands_tenant ON commands(tenant_id);
CREATE INDEX idx_commands_status ON commands(status);
CREATE INDEX idx_commands_trace ON commands(trace_id);
CREATE INDEX idx_commands_issued ON commands(issued_at DESC);

COMMENT ON TABLE commands IS 'CLI commands with idempotency deduplication';

-- 4.3 Fills (Exchange Trade Deduplication)
CREATE TABLE fills (
    fill_id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID NOT NULL,
    account_id UUID NOT NULL,
    order_id UUID NOT NULL REFERENCES orders_current(order_id) ON DELETE CASCADE,

    -- Exchange Mapping
    exchange_order_id TEXT NOT NULL,
    exchange_trade_id TEXT NOT NULL,

    -- Fill Details
    symbol VARCHAR(20) NOT NULL,
    side VARCHAR(10) NOT NULL,
    fill_price DECIMAL(20, 8) NOT NULL,
    fill_quantity DECIMAL(20, 8) NOT NULL,

    -- Fees
    fee DECIMAL(20, 8) NOT NULL,
    fee_asset VARCHAR(10) NOT NULL,

    -- Maker/Taker
    is_maker BOOLEAN NOT NULL,

    -- Audit
    filled_at TIMESTAMPTZ NOT NULL,
    ingested_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    CONSTRAINT uk_fills_exchange_trade_id UNIQUE (tenant_id, exchange_trade_id)
);

CREATE INDEX idx_fills_tenant_account ON fills(tenant_id, account_id);
CREATE INDEX idx_fills_order ON fills(order_id);
CREATE INDEX idx_fills_symbol ON fills(symbol);
CREATE INDEX idx_fills_filled_at ON fills(filled_at DESC);

COMMENT ON TABLE fills IS 'Exchange trade fills with deduplication by exchange_trade_id';

-- =============================================================================
-- 5. FUNCTIONS
-- =============================================================================

-- 5.1 Next Sequence Number
CREATE OR REPLACE FUNCTION next_seq(p_stream_key TEXT, p_tenant_id UUID)
RETURNS BIGINT AS $$
DECLARE
    new_seq BIGINT;
BEGIN
    -- Atomic increment with lock
    UPDATE stream_state
    SET last_seq = last_seq + 1, updated_at = NOW()
    WHERE stream_key = p_stream_key
    RETURNING last_seq INTO new_seq;

    -- Initialize if not exists
    IF new_seq IS NULL THEN
        INSERT INTO stream_state (stream_key, tenant_id, last_seq)
        VALUES (p_stream_key, p_tenant_id, 1)
        ON CONFLICT (stream_key) DO UPDATE SET last_seq = stream_state.last_seq + 1
        RETURNING last_seq INTO new_seq;
    END IF;

    RETURN new_seq;
END;
$$ LANGUAGE plpgsql;

COMMENT ON FUNCTION next_seq IS 'Get next sequence number for stream (atomic)';

-- 5.2 Create Event Log Partitions
CREATE OR REPLACE FUNCTION create_event_log_partitions(months_ahead INT)
RETURNS VOID AS $$
DECLARE
    start_date DATE;
    end_date DATE;
    partition_name TEXT;
BEGIN
    FOR i IN 1..months_ahead LOOP
        start_date := DATE_TRUNC('month', NOW() + (i || ' months')::INTERVAL)::DATE;
        end_date := (start_date + INTERVAL '1 month')::DATE;
        partition_name := 'event_log_' || TO_CHAR(start_date, 'YYYY_MM');

        IF NOT EXISTS (
            SELECT 1 FROM pg_class WHERE relname = partition_name
        ) THEN
            EXECUTE format(
                'CREATE TABLE %I PARTITION OF event_log FOR VALUES FROM (%L) TO (%L)',
                partition_name, start_date, end_date
            );

            RAISE NOTICE 'Created partition: %', partition_name;
        END IF;
    END LOOP;
END;
$$ LANGUAGE plpgsql;

COMMENT ON FUNCTION create_event_log_partitions IS 'Auto-create future event_log partitions';

-- 5.3 Create Snapshot Partitions
CREATE OR REPLACE FUNCTION create_snapshot_partitions(months_ahead INT)
RETURNS VOID AS $$
DECLARE
    start_date DATE;
    end_date DATE;
    partition_name TEXT;
BEGIN
    FOR i IN 1..months_ahead LOOP
        start_date := DATE_TRUNC('month', NOW() + (i || ' months')::INTERVAL)::DATE;
        end_date := (start_date + INTERVAL '1 month')::DATE;
        partition_name := 'snapshots_' || TO_CHAR(start_date, 'YYYY_MM');

        IF NOT EXISTS (
            SELECT 1 FROM pg_class WHERE relname = partition_name
        ) THEN
            EXECUTE format(
                'CREATE TABLE %I PARTITION OF snapshots FOR VALUES FROM (%L) TO (%L)',
                partition_name, start_date, end_date
            );

            RAISE NOTICE 'Created partition: %', partition_name;
        END IF;
    END LOOP;
END;
$$ LANGUAGE plpgsql;

COMMENT ON FUNCTION create_snapshot_partitions IS 'Auto-create future snapshot partitions';

-- 5.4 Check Partition Coverage
CREATE OR REPLACE FUNCTION check_partition_coverage()
RETURNS TABLE(table_name TEXT, months_ahead INT, status TEXT) AS $$
BEGIN
    RETURN QUERY
    SELECT
        'event_log'::TEXT,
        COUNT(*)::INT,
        CASE WHEN COUNT(*) >= 2 THEN 'OK' ELSE 'WARNING' END
    FROM pg_class
    WHERE relname LIKE 'event_log_%'
    AND relname > 'event_log_' || TO_CHAR(NOW(), 'YYYY_MM')

    UNION ALL

    SELECT
        'snapshots'::TEXT,
        COUNT(*)::INT,
        CASE WHEN COUNT(*) >= 2 THEN 'OK' ELSE 'WARNING' END
    FROM pg_class
    WHERE relname LIKE 'snapshots_%'
    AND relname > 'snapshots_' || TO_CHAR(NOW(), 'YYYY_MM');
END;
$$ LANGUAGE plpgsql;

COMMENT ON FUNCTION check_partition_coverage IS 'Monitor partition coverage (should have 2+ future months)';

-- =============================================================================
-- 6. TRIGGERS
-- =============================================================================

-- Auto-update updated_at on projection tables
CREATE OR REPLACE FUNCTION update_updated_at_column()
RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at = NOW();
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER orders_updated_at BEFORE UPDATE ON orders_current
    FOR EACH ROW EXECUTE FUNCTION update_updated_at_column();

CREATE TRIGGER positions_updated_at BEFORE UPDATE ON positions_current
    FOR EACH ROW EXECUTE FUNCTION update_updated_at_column();

CREATE TRIGGER balances_updated_at BEFORE UPDATE ON balances_current
    FOR EACH ROW EXECUTE FUNCTION update_updated_at_column();

CREATE TRIGGER risk_updated_at BEFORE UPDATE ON risk_state_current
    FOR EACH ROW EXECUTE FUNCTION update_updated_at_column();

CREATE TRIGGER strategy_updated_at BEFORE UPDATE ON strategy_state_current
    FOR EACH ROW EXECUTE FUNCTION update_updated_at_column();

COMMIT;

-- Post-migration notes:
-- 1. Run: SELECT create_event_log_partitions(3); to create more future partitions
-- 2. Schedule monthly cron: psql -c "SELECT create_event_log_partitions(3);"
-- 3. Monitor partition coverage: SELECT * FROM check_partition_coverage();
