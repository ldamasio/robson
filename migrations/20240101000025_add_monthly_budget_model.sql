-- ADR-0051 rollout step 1: dormant persisted monthly budget model.

ALTER TABLE monthly_state
  ADD COLUMN monthly_budget_model TEXT NOT NULL DEFAULT 'hwm_v1'
  CHECK (monthly_budget_model IN ('hwm_v1', 'net_from_start_non_expanding_v1'));
