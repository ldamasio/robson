-- ADR-0056 step 7, PR S: one row per daemon process, an epoch.
--
-- Inserted with a strict commit before the process sends any venue request.
-- A stop marks its epoch clean only when every operation finished in time, none
-- was aborted, the journal holds nothing unlanded and its latch is clear. A
-- start whose predecessor carries no marker, or one older than version 2 (the
-- fence PR's), is unclean; until then every start is, and it is only recorded.
--
-- Additive: an older binary ignores this table. epoch_seq orders epochs across
-- processes and feeds the fence PR's generation (epoch_seq << 32), so a start
-- refuses once it reaches 2^31.

CREATE TABLE process_epochs (
    epoch_id        UUID PRIMARY KEY,
    epoch_seq       BIGSERIAL NOT NULL UNIQUE,
    -- The daemon's tenant: a predecessor is the latest epoch of the same one.
    tenant_id       UUID NOT NULL,
    started_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    clean_stop_at   TIMESTAMPTZ,
    marker_version  SMALLINT CHECK (marker_version > 0),
    binary_version  TEXT NOT NULL,

    CONSTRAINT process_epochs_marker_is_whole
        CHECK ((clean_stop_at IS NULL) = (marker_version IS NULL))
);

CREATE INDEX idx_process_epochs_tenant_seq ON process_epochs (tenant_id, epoch_seq DESC);
