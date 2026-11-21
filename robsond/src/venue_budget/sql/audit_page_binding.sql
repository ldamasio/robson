-- A02: add the active binding to the original bounded global source page.
WITH binding AS MATERIALIZED (
    SELECT s.audit_id,s.quiescence_generation,s.high_water,s.cursor,s.examined_count,s.status
    FROM budget_venue_state v
    JOIN budget_source_audits s ON s.audit_id=v.audit_id AND s.venue_id=v.venue_id
     AND s.quiescence_generation=v.quiescence_generation
     AND s.high_water IS NOT DISTINCT FROM v.source_high_water
    WHERE v.venue_id='binance:spot'
)
SELECT source.*,
       (SELECT count(*) FROM binding) AS bindings,
       (SELECT audit_id FROM binding) AS bound_audit,
       (SELECT quiescence_generation FROM binding) AS bound_generation,
       (SELECT high_water FROM binding) AS bound_high_water,
       (SELECT cursor FROM binding) AS bound_cursor,
       (SELECT examined_count FROM binding) AS bound_count,
       (SELECT status FROM binding) AS bound_status
FROM (
