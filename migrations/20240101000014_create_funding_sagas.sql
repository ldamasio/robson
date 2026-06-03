-- Funding treasury saga projection.
--
-- Events remain authoritative in event_log streams keyed as funding:<saga_id>.
-- This table stores the current state needed by REST GET/list handlers and
-- worker recovery scans.

CREATE TABLE IF NOT EXISTS funding_sagas (
    tenant_id      UUID        NOT NULL,
    saga_id        UUID        NOT NULL,
    state          TEXT        NOT NULL,
    quote          JSONB       NOT NULL,
    estimated_usdt NUMERIC     NOT NULL DEFAULT 0,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (tenant_id, saga_id)
);

CREATE INDEX IF NOT EXISTS idx_funding_sagas_state
    ON funding_sagas (tenant_id, state, updated_at);
