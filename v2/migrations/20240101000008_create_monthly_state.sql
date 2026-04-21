-- Month boundary state tracking (MIG-v3#12)

CREATE TABLE monthly_state (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    year          SMALLINT NOT NULL,
    month         SMALLINT NOT NULL CHECK (month BETWEEN 1 AND 12),
    capital_base  NUMERIC(20,8) NOT NULL CHECK (capital_base >= 0),
    carried_risk  NUMERIC(20,8) NOT NULL DEFAULT 0 CHECK (carried_risk >= 0),
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (year, month)
);

CREATE INDEX idx_monthly_state_year_month ON monthly_state(year, month);
