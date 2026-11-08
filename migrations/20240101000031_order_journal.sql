-- ADR-0056 step 7, PR J: the durable order journal.
--
-- One intent per logical order, one attempt per submission, written ahead of
-- the request. Additive: an older binary ignores these tables, but ignoring the
-- work they record is not safe. Rollback boundary B is the view at the end.
--
-- Columns the later PRs of step 7 start writing are created here so the table
-- is designed once: cancel_class beyond 'cancelled' (typed cancel classifier),
-- uncertain_generation (fence), applied_at. risk_accepted_* is also written by
-- the release of the fence PR.

CREATE TABLE order_intents (
    intent_id             UUID PRIMARY KEY,
    tenant_id             UUID NOT NULL,
    venue_id              TEXT NOT NULL,
    account_scope         TEXT NOT NULL,
    -- A cancellation's id is local and is never sent to the venue.
    client_order_id       TEXT NOT NULL,
    position_id           UUID,
    kind                  TEXT NOT NULL
        CHECK (kind IN ('entry', 'exit', 'protection', 'cancel', 'sweep_close')),
    cancels_order_id      TEXT,
    params                JSONB NOT NULL,
    -- NULL means no definite answer: uncertain, in flight or not sent.
    venue_outcome         TEXT CHECK (venue_outcome IN ('success', 'rejected', 'gone_terminal')),
    cancel_class          TEXT,
    not_sent              BOOLEAN NOT NULL DEFAULT FALSE,
    venue_evidence        JSONB,
    applied_at            TIMESTAMPTZ,
    uncertain_at          TIMESTAMPTZ,
    uncertain_generation  BIGINT,
    -- Who accepted the risk. Never set for terminal evidence.
    risk_accepted_at      TIMESTAMPTZ,
    risk_accepted_by      TEXT,
    risk_accepted_auth    TEXT CHECK (risk_accepted_auth IN ('verified', 'asserted')),
    risk_accepted_note    TEXT,
    -- A disposition is not a venue outcome and never changes venue_outcome.
    disposition_kind      TEXT CHECK (disposition_kind IN ('terminal_evidence', 'accepted_risk')),
    disposition_treatment TEXT CHECK (disposition_treatment IN ('unbooked_accepted')),
    disposition_evidence  JSONB,
    created_at            TIMESTAMPTZ NOT NULL,

    -- Rule 3: a client id is spent for good, whichever process used it.
    CONSTRAINT order_intents_client_id_unique UNIQUE (venue_id, account_scope, client_order_id),
    CONSTRAINT order_intents_disposition_has_treatment
        CHECK (disposition_kind IS NULL OR disposition_treatment IS NOT NULL),
    -- Terminal evidence names one order and its terminal state and attaches the
    -- venue's own record, which is what keeps an empty-account observation or a
    -- bare assertion from satisfying boundary B.
    CONSTRAINT order_intents_terminal_evidence_names_an_order
        CHECK (
            disposition_kind IS DISTINCT FROM 'terminal_evidence'
            OR (disposition_evidence IS NOT NULL
                AND disposition_evidence ? 'order_ref'
                AND disposition_evidence ? 'terminal_status'
                AND disposition_evidence ? 'venue_record'
                AND jsonb_typeof(disposition_evidence -> 'venue_record') = 'object'
                AND disposition_evidence -> 'venue_record' <> '{}'::jsonb)
        ),
    CONSTRAINT order_intents_accepted_risk_has_identity
        CHECK (
            disposition_kind IS DISTINCT FROM 'accepted_risk'
            OR (risk_accepted_at IS NOT NULL AND risk_accepted_by IS NOT NULL
                AND risk_accepted_auth IS NOT NULL)
        )
);

CREATE TABLE submit_attempts (
    attempt_id      UUID PRIMARY KEY,
    venue_id        TEXT NOT NULL,
    account_scope   TEXT NOT NULL,
    client_order_id TEXT NOT NULL,
    state           TEXT NOT NULL CHECK (state IN ('started', 'rejected')),
    created_at      TIMESTAMPTZ NOT NULL,
    rejected_at     TIMESTAMPTZ,

    CONSTRAINT submit_attempts_rejected_has_time
        CHECK ((state = 'rejected') = (rejected_at IS NOT NULL)),
    CONSTRAINT submit_attempts_intent_fk
        FOREIGN KEY (venue_id, account_scope, client_order_id)
        REFERENCES order_intents (venue_id, account_scope, client_order_id)
);

CREATE INDEX idx_submit_attempts_client_id
    ON submit_attempts (venue_id, account_scope, client_order_id);

-- Append-only record of operator decisions on the journal.
CREATE TABLE journal_decisions (
    decision_id UUID PRIMARY KEY,
    kind        TEXT NOT NULL CHECK (kind IN ('disposition')),
    intent_id   UUID REFERENCES order_intents (intent_id),
    actor       TEXT NOT NULL,
    auth_kind   TEXT NOT NULL CHECK (auth_kind IN ('verified', 'asserted')),
    note        TEXT NOT NULL CHECK (note <> ''),
    treatment   TEXT,
    evidence    JSONB,
    payload     JSONB NOT NULL,
    decided_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_order_intents_no_outcome
    ON order_intents (created_at)
    WHERE venue_outcome IS NULL AND NOT not_sent;

-- Rollback boundary B: every intent that blocks a rollback across this schema.
-- Outstanding is no definite venue outcome and not proven unsent. Each must
-- carry a disposition, which the constraints above tie to a treatment and, for
-- terminal evidence, to evidence naming the order.
CREATE VIEW order_intents_blocking_rollback AS
SELECT intent_id, client_order_id, kind, position_id, uncertain_at, created_at
FROM order_intents
WHERE venue_outcome IS NULL
  AND NOT not_sent
  AND disposition_kind IS NULL;
