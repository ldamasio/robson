-- F03 no binds; require exactly one pristine incomplete row (not a completed
-- initialization), qg=0, null init/audit/cohort/highwater, default schema/proof.
INSERT INTO budget_venue_state(venue_id) VALUES ('binance:spot')
ON CONFLICT(venue_id) DO UPDATE SET venue_id=EXCLUDED.venue_id
RETURNING venue_id,initialization_state,initialization_id,audit_id,
          quiescence_generation,schema_version,proof_version,
          recovery_generation,recovery_status,cohort_id,source_high_water,
          epoch_count_60,epoch_count_10;
