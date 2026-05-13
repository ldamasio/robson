-- Robson v2 Initial Schema
-- Event sourcing with position snapshots
--
-- Key design decisions:
-- - Fixed 10x leverage (not stored)
-- - Trailing stop based on technical stop distance
-- - Events are immutable audit log
-- - Positions are mutable snapshots (rebuilt from events if needed)

-- =============================================================================
-- Positions (Snapshots)
-- =============================================================================

CREATE TABLE positions (
    id UUID PRIMARY KEY,
    account_id UUID NOT NULL,
    symbol VARCHAR(20) NOT NULL,
    side VARCHAR(10) NOT NULL CHECK (side IN ('long', 'short')),

    -- State machine
    state VARCHAR(20) NOT NULL CHECK (state IN (
        'armed', 'entering', 'active', 'exiting', 'closed', 'error'
    )),
    state_data JSONB NOT NULL DEFAULT '{}',

    -- Entry
    entry_price DECIMAL(20, 8),
    entry_filled_at TIMESTAMPTZ,

    -- Technical stop distance (the golden rule)
    tech_stop_distance DECIMAL(20, 8),
    tech_stop_distance_pct DECIMAL(10, 6),

    -- Position sizing (derived from tech stop + 1% risk)
    quantity DECIMAL(20, 8) NOT NULL DEFAULT 0,

    -- P&L
    realized_pnl DECIMAL(20, 8) NOT NULL DEFAULT 0,
    fees_paid DECIMAL(20, 8) NOT NULL DEFAULT 0,

    -- Associated orders
    entry_order_id UUID,
    exit_order_id UUID,

    -- Audit
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    closed_at TIMESTAMPTZ
);

CREATE INDEX idx_positions_account_id ON positions(account_id);
CREATE INDEX idx_positions_symbol ON positions(symbol);
CREATE INDEX idx_positions_state ON positions(state);
CREATE INDEX idx_positions_created_at ON positions(created_at);

-- =============================================================================
-- Events (Immutable Audit Log)
-- =============================================================================

CREATE TABLE events (
    id BIGSERIAL PRIMARY KEY,
    position_id UUID NOT NULL,
    event_type VARCHAR(50) NOT NULL,
    event_data JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_events_position_id ON events(position_id);
CREATE INDEX idx_events_event_type ON events(event_type);
CREATE INDEX idx_events_created_at ON events(created_at);

-- =============================================================================
-- Orders
-- =============================================================================

CREATE TABLE orders (
    id UUID PRIMARY KEY,
    position_id UUID NOT NULL,
    exchange_order_id VARCHAR(100),
    client_order_id VARCHAR(100) NOT NULL,

    symbol VARCHAR(20) NOT NULL,
    side VARCHAR(10) NOT NULL CHECK (side IN ('buy', 'sell')),
    order_type VARCHAR(20) NOT NULL CHECK (order_type IN (
        'market', 'limit', 'stop_loss_limit'
    )),

    quantity DECIMAL(20, 8) NOT NULL,
    price DECIMAL(20, 8),  -- NULL for market orders
    stop_price DECIMAL(20, 8),  -- For stop orders

    status VARCHAR(20) NOT NULL CHECK (status IN (
        'pending', 'submitted', 'partial', 'filled', 'cancelled', 'rejected'
    )),

    filled_quantity DECIMAL(20, 8),
    fill_price DECIMAL(20, 8),
    filled_at TIMESTAMPTZ,
    fee_paid DECIMAL(20, 8),

    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    CONSTRAINT fk_orders_position FOREIGN KEY (position_id)
        REFERENCES positions(id) ON DELETE CASCADE
);

CREATE INDEX idx_orders_position_id ON orders(position_id);
CREATE INDEX idx_orders_exchange_order_id ON orders(exchange_order_id);
CREATE INDEX idx_orders_client_order_id ON orders(client_order_id);
CREATE INDEX idx_orders_status ON orders(status);

-- =============================================================================
-- Intents (Idempotency)
-- =============================================================================

CREATE TABLE intents (
    id UUID PRIMARY KEY,
    position_id UUID NOT NULL,
    intent_type VARCHAR(50) NOT NULL,
    intent_data JSONB NOT NULL,

    status VARCHAR(20) NOT NULL CHECK (status IN (
        'pending', 'processing', 'completed', 'failed'
    )),
    result JSONB,
    error TEXT,

    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    completed_at TIMESTAMPTZ,

    CONSTRAINT fk_intents_position FOREIGN KEY (position_id)
        REFERENCES positions(id) ON DELETE CASCADE
);

CREATE INDEX idx_intents_position_id ON intents(position_id);
CREATE INDEX idx_intents_status ON intents(status);
CREATE UNIQUE INDEX idx_intents_idempotency ON intents(id, position_id);

-- =============================================================================
-- Trigger for updated_at
-- =============================================================================

CREATE OR REPLACE FUNCTION update_updated_at_column()
RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at = NOW();
    RETURN NEW;
END;
$$ language 'plpgsql';

CREATE TRIGGER positions_updated_at
    BEFORE UPDATE ON positions
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_at_column();
