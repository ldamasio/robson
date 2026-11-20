-- F06 $1 new fixture initialization UUID, $2 actual F05 audit UUID.
-- Exactly one row/result. The real trigger checks incomplete->complete,
-- unchanged identity/version, and monotone generation. No source rows created.
UPDATE budget_venue_state s
SET initialization_state='complete',initialization_id=$1::uuid,
    audit_id=a.audit_id,quiescence_generation=1,
    recovery_status='comparable',epoch_count_60=1,epoch_count_10=1
FROM budget_source_audits a
WHERE s.venue_id='binance:spot' AND s.initialization_state='incomplete'
  AND s.initialization_id IS NULL AND s.audit_id IS NULL
  AND s.quiescence_generation=0 AND s.schema_version=38 AND s.proof_version=1
  AND s.recovery_generation=0 AND s.cohort_id IS NULL AND s.source_high_water IS NULL
  AND s.recovery_status='unknown' AND s.epoch_count_60=9 AND s.epoch_count_10=3
  AND a.audit_id=$2::uuid AND a.venue_id=s.venue_id
  AND a.quiescence_generation=1 AND a.status='complete'
  AND a.high_water IS NULL AND a.cursor IS NULL AND a.examined_count=0
  AND a.completed_at IS NOT NULL AND a.failure_reason IS NULL
  AND a.schema_version=38 AND a.proof_generation=1
  AND NOT EXISTS(SELECT 1 FROM submit_attempts)
  AND NOT EXISTS(SELECT 1 FROM order_intents)
  AND NOT EXISTS(SELECT 1 FROM spot_intent_clock)
  AND NOT EXISTS(SELECT 1 FROM budget_daily_charges)
  AND NOT EXISTS(SELECT 1 FROM budget_epochs)
  AND NOT EXISTS(SELECT 1 FROM venue_pauses)
  AND NOT EXISTS(SELECT 1 FROM budget_recovery_cohorts)
  AND NOT EXISTS(SELECT 1 FROM budget_expiry_proofs)
  AND NOT EXISTS(SELECT 1 FROM budget_source_audits
                 WHERE audit_id<>a.audit_id)
RETURNING s.venue_id,s.initialization_state,s.initialization_id,s.audit_id,
          s.quiescence_generation,s.schema_version,s.proof_version,
          s.recovery_generation,s.recovery_status,s.cohort_id,s.source_high_water,
          s.epoch_count_60,s.epoch_count_10,s.recorded_at::text AS recorded_at_text;
