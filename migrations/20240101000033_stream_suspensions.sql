-- ADR-0056 step 7, PR P: event-log streams that refuse appends.
--
-- A failed append or projection apply suspends its stream for the life of the
-- process: nothing more is appended to it, so its projection lags its log by
-- exactly the suffix that did not apply. The suspension is recorded here
-- through the journal's retry set and read back at the next start. A position
-- stream is then repaired, its suffix validated and applied in pages, and
-- cleared; a system stream stays suspended until step 8 delivers a replay-safe
-- repair. The repair's evidence is this row: repaired_through_seq per page,
-- repair_error, cleared_at.
--
-- Additive: an older binary ignores this table. Ignoring a suspension is not
-- safe, so a rollback needs no open row here.

CREATE TABLE stream_suspensions (
    suspension_id         UUID PRIMARY KEY,
    tenant_id             UUID NOT NULL,
    stream_key            TEXT NOT NULL,
    stream_kind           TEXT NOT NULL CHECK (stream_kind IN ('position', 'system')),
    -- The seq that did not apply; NULL when the append failed, or landed with
    -- its seq unknown.
    failed_seq            BIGINT,
    -- Position streams: the last seq the startup repair applied.
    repaired_through_seq  BIGINT,
    -- System streams: the month, as its first day.
    month                 DATE,
    error                 TEXT NOT NULL,
    -- Position streams: why the last startup repair did not clear it.
    repair_error          TEXT,
    -- The process that suspended it.
    epoch_id              UUID,
    suspended_at          TIMESTAMPTZ NOT NULL,
    cleared_at            TIMESTAMPTZ,

    CONSTRAINT stream_suspensions_month_is_the_system_kind
        CHECK ((stream_kind = 'system') = (month IS NOT NULL)),
    CONSTRAINT stream_suspensions_month_is_a_first_day
        CHECK (month IS NULL OR EXTRACT(DAY FROM month) = 1),
    CONSTRAINT stream_suspensions_repair_is_position_only
        CHECK (stream_kind = 'position'
               OR (repaired_through_seq IS NULL AND repair_error IS NULL))
);

-- One open suspension per stream.
CREATE UNIQUE INDEX idx_stream_suspensions_open
    ON stream_suspensions (tenant_id, stream_key) WHERE cleared_at IS NULL;
