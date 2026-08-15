-- Persist the signal entry reference used to resolve the admission stop plan.
-- positions_current.entry_price is overwritten by the exchange fill, so it
-- cannot carry this immutable pricing provenance across projection recovery.

ALTER TABLE positions_current
    ADD COLUMN IF NOT EXISTS stop_plan_entry_reference DECIMAL(20, 8);

COMMENT ON COLUMN positions_current.stop_plan_entry_reference IS
    'Signal entry reference used to resolve the executable stop plan; immutable once set and NULL on historical rows';

CREATE OR REPLACE FUNCTION reject_stop_plan_entry_reference_rewrite()
RETURNS TRIGGER AS $$
BEGIN
    IF OLD.stop_plan_entry_reference IS NOT NULL
       AND NEW.stop_plan_entry_reference IS DISTINCT FROM OLD.stop_plan_entry_reference THEN
        RAISE EXCEPTION 'stop_plan_entry_reference is immutable once set';
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER positions_current_stop_plan_entry_reference_write_once
BEFORE UPDATE OF stop_plan_entry_reference
ON positions_current
FOR EACH ROW
EXECUTE FUNCTION reject_stop_plan_entry_reference_rewrite();
