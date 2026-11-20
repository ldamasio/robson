-- B03: $1 venue TEXT, fixed by private caller to binance:spot.
-- Exactly one row/result, rows_affected=1; caller refuses incomplete state.
INSERT INTO budget_venue_state(venue_id) VALUES ($1::text)
ON CONFLICT(venue_id) DO UPDATE SET venue_id=EXCLUDED.venue_id
RETURNING venue_id,initialization_state,initialization_id,audit_id,
          quiescence_generation,schema_version,proof_version,
          recovery_generation,recovery_status,cohort_id,source_high_water,
          epoch_count_60,epoch_count_10,recorded_at::text AS recorded_at_text;
