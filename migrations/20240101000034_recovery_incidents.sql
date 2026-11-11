-- ADR-0056 step 7, PR F: durable incidents and the audited release.
--
-- An incident is what closed the admission fence: one open row per reason,
-- counting its occurrences and the generations they were recorded at. A
-- generation is epoch_seq << 32 | n, monotonic across processes through the
-- strict epoch insert. An upsert against a released row opens a new one.
--
-- A release is an operator's risk acceptance, never a venue outcome or a
-- booking: one transaction tombstones the uncertain intents stamped at or
-- below the quoted generation (order_intents.disposition_kind =
-- 'accepted_risk'), releases the incidents whose last generation is at or
-- below it, and appends its decision here, in journal_decisions.
--
-- Additive: an older binary ignores this table. Ignoring an open incident is
-- not safe, so rolling back across it needs the fence PR's condition: the
-- final, quiesced epoch stopped clean at marker_version 2, which no open
-- incident allows.

CREATE TABLE recovery_incidents (
    incident_id       UUID PRIMARY KEY,
    tenant_id         UUID NOT NULL,
    reason            TEXT NOT NULL CHECK (reason IN (
        'unclean_start', 'uncertain_outcome', 'unlanded_send', 'persistence_failure',
        'unpermitted_mutation', 'stream_suspended', 'operation_aborted',
        'generation_exhausted'
    )),
    first_at          TIMESTAMPTZ NOT NULL,
    last_at           TIMESTAMPTZ NOT NULL,
    count             BIGINT NOT NULL CHECK (count > 0),
    first_generation  BIGINT NOT NULL,
    last_generation   BIGINT NOT NULL,
    released_at       TIMESTAMPTZ,
    decision_id       UUID,

    CONSTRAINT recovery_incidents_generations_ordered
        CHECK (first_generation <= last_generation),
    CONSTRAINT recovery_incidents_release_is_whole
        CHECK ((released_at IS NULL) = (decision_id IS NULL))
);

-- At most one open incident per reason.
CREATE UNIQUE INDEX idx_recovery_incidents_open
    ON recovery_incidents (tenant_id, reason) WHERE released_at IS NULL;

-- A release appends its decision with the incidents it released, after them
-- in its transaction.
ALTER TABLE journal_decisions DROP CONSTRAINT journal_decisions_kind_check;
ALTER TABLE journal_decisions
    ADD CONSTRAINT journal_decisions_kind_check CHECK (kind IN ('disposition', 'release'));

ALTER TABLE recovery_incidents
    ADD CONSTRAINT recovery_incidents_decision_fk
        FOREIGN KEY (decision_id) REFERENCES journal_decisions (decision_id)
        DEFERRABLE INITIALLY DEFERRED;

-- The index on the intents a release tombstones is migration 35: it is built
-- concurrently, outside any transaction, so live journal writes never wait on
-- it.
