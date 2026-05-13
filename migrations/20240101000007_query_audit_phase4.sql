-- Migration: Query audit projection + projection worker checkpoint persistence
-- Created: 2026-04-05
-- Purpose: Persist query lifecycle snapshots and store projection worker cursor in DB.

CREATE TABLE queries_current (
    query_id UUID PRIMARY KEY,
    tenant_id UUID NOT NULL,
    stream_key TEXT NOT NULL,
    position_id UUID,
    state TEXT NOT NULL,
    started_at TIMESTAMPTZ NOT NULL,
    finished_at TIMESTAMPTZ,
    snapshot JSONB NOT NULL,
    last_event_id UUID NOT NULL,
    last_seq BIGINT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL
);

CREATE INDEX idx_queries_current_state ON queries_current(state);
CREATE INDEX idx_queries_current_position_id ON queries_current(position_id)
    WHERE position_id IS NOT NULL;
CREATE INDEX idx_queries_current_updated_at ON queries_current(updated_at DESC);

COMMENT ON TABLE queries_current IS 'Projection: latest durable lifecycle snapshot for each execution query';

CREATE TABLE projection_checkpoints (
    projection_name TEXT NOT NULL,
    tenant_id UUID NOT NULL,
    stream_key TEXT NOT NULL,
    last_seq BIGINT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    CONSTRAINT pk_projection_checkpoints PRIMARY KEY (projection_name, tenant_id, stream_key)
);

COMMENT ON TABLE projection_checkpoints IS 'Projection worker checkpoints persisted in PostgreSQL';
