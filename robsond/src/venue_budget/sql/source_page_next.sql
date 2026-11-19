-- One bounded descriptive page, not audit completion or writer authority.
-- $1 high-water UUID; next-page variant additionally binds $2 actual cursor.
WITH page AS MATERIALIZED (
    SELECT a.attempt_id,a.venue_id,a.account_scope,a.client_order_id,a.created_at
    FROM submit_attempts a
    WHERE a.attempt_id <= $1::uuid AND a.attempt_id > $2::uuid
    ORDER BY a.attempt_id LIMIT 1000
), checked AS MATERIALIZED (
    SELECT p.attempt_id,
           i.intent_id IS NULL AS no_intent,
           c.attempt_id IS NULL AS no_clock,
           d.attempt_id IS NULL AS no_charge,
           i.kind='cancel' AS is_cancel,
           c.attempt_id IS NOT NULL AND c.budget_clock_domain IS NOT DISTINCT FROM NULL::budget_clock_domain_v38
             AND c.budget_created_tick IS NULL AS legacy_null,
           i.intent_id IS NOT NULL AND c.attempt_id IS NOT NULL AND d.attempt_id IS NOT NULL
             AND (ROW(c.intent_id,c.tenant_id,c.venue_id,c.attempt_created_at)
                   IS DISTINCT FROM ROW(i.intent_id,i.tenant_id,p.venue_id,p.created_at)
               OR ROW(d.venue_id,d.tenant_id,d.intent_id,d.account_scope,d.client_order_id,d.kind,
                      d.source_created_at,d.budget_clock_domain,d.budget_created_tick,d.budget_recorded_at,d.daily_units)
                   IS DISTINCT FROM ROW(p.venue_id,i.tenant_id,i.intent_id,p.account_scope,p.client_order_id,i.kind,
                      p.created_at,c.budget_clock_domain,c.budget_created_tick,c.budget_recorded_at,
                      CASE WHEN i.kind='cancel' THEN 0::smallint ELSE 1::smallint END)) AS mismatch
    FROM page p
    LEFT JOIN order_intents i ON (i.venue_id,i.account_scope,i.client_order_id)
                               =(p.venue_id,p.account_scope,p.client_order_id)
    LEFT JOIN spot_intent_clock c ON c.attempt_id=p.attempt_id
    LEFT JOIN budget_daily_charges d ON d.attempt_id=c.attempt_id
    WHERE p.venue_id='binance:spot'
)
SELECT ARRAY(SELECT attempt_id FROM page ORDER BY attempt_id) AS examined,
       ARRAY(SELECT attempt_id FROM checked ORDER BY attempt_id) AS spot,
       ARRAY(SELECT attempt_id FROM checked WHERE no_intent ORDER BY attempt_id) AS missing_intent,
       ARRAY(SELECT attempt_id FROM checked WHERE no_clock ORDER BY attempt_id) AS missing_clock,
       ARRAY(SELECT attempt_id FROM checked WHERE no_charge ORDER BY attempt_id) AS missing_charge,
       ARRAY(SELECT attempt_id FROM checked WHERE mismatch ORDER BY attempt_id) AS mismatched,
       ARRAY(SELECT attempt_id FROM checked WHERE is_cancel ORDER BY attempt_id) AS cancels,
       ARRAY(SELECT attempt_id FROM checked WHERE legacy_null ORDER BY attempt_id) AS legacy_null
