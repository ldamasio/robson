-- F05 $1 actual fresh fixture audit UUID, $2 fixed PG-microsecond audit time.
-- Exactly one row/result. Zero rows is terminal nonempty/context refusal.
-- Complete status here means this controlled empty fixture only; no global
-- production fencing/audit claim. The repeated predicates are actual SQL.
INSERT INTO budget_source_audits(audit_id,venue_id,quiescence_generation,
    high_water,cursor,examined_count,status,failure_reason,started_at,
    completed_at,schema_version,proof_generation)
SELECT $1::uuid,'binance:spot',1,NULL,NULL,0,'complete',NULL,
       $2::timestamptz,$2::timestamptz,38,1
WHERE NOT EXISTS(SELECT 1 FROM submit_attempts)
  AND NOT EXISTS(SELECT 1 FROM order_intents)
  AND NOT EXISTS(SELECT 1 FROM spot_intent_clock)
  AND NOT EXISTS(SELECT 1 FROM budget_daily_charges)
  AND NOT EXISTS(SELECT 1 FROM budget_epochs)
  AND NOT EXISTS(SELECT 1 FROM budget_source_audits)
  AND NOT EXISTS(SELECT 1 FROM venue_pauses)
  AND NOT EXISTS(SELECT 1 FROM budget_recovery_cohorts)
  AND NOT EXISTS(SELECT 1 FROM budget_expiry_proofs)
RETURNING audit_id,venue_id,quiescence_generation,high_water,cursor,
          examined_count,status,failure_reason,
          (extract(epoch FROM started_at)*1000000)::bigint AS started_at_us,
          (extract(epoch FROM completed_at)*1000000)::bigint AS completed_at_us,
          schema_version,proof_generation;
