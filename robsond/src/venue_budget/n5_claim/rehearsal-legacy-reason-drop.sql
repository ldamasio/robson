DO $migration38$
BEGIN
    LOCK TABLE public.recovery_incidents IN ACCESS EXCLUSIVE MODE;
    IF NOT EXISTS (
        SELECT 1
          FROM pg_catalog.pg_constraint c
         WHERE c.conrelid = 'public.recovery_incidents'::regclass
           AND c.conname = 'recovery_incidents_reason_check'
           AND c.oid::text = current_setting('robson_cut.old_oid')
           AND c.contype = 'c' AND c.convalidated
           AND pg_catalog.pg_get_constraintdef(c.oid) = current_setting('robson_cut.old_def')
    ) THEN
        RAISE EXCEPTION USING ERRCODE = 'RBC01', MESSAGE = 'old reason CHECK mismatch';
    END IF;
    IF NOT EXISTS (
        SELECT 1
          FROM pg_catalog.pg_constraint c
         WHERE c.conrelid = 'public.recovery_incidents'::regclass
           AND c.conname = 'recovery_incidents_reason_v38'
           AND c.oid::text = current_setting('robson_cut.new_oid')
           AND c.contype = 'c' AND c.convalidated
           AND pg_catalog.pg_get_constraintdef(c.oid) = current_setting('robson_cut.new_def')
    ) THEN
        RAISE EXCEPTION USING ERRCODE = 'RBC02', MESSAGE = 'expanded reason CHECK mismatch';
    END IF;
    ALTER TABLE public.recovery_incidents DROP CONSTRAINT recovery_incidents_reason_check RESTRICT;
END;
$migration38$;
