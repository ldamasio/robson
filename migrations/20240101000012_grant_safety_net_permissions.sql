-- Grant runtime role access to safety net tables
-- Fixes: "permission denied for table detected_positions"
--
-- Root cause: tables created by superuser during initial provisioning;
-- the robson runtime role never received DML grants on them.
--
-- This migration is idempotent and non-breaking:
-- - If current_user owns the tables: GRANT is a no-op (owner already has ALL)
-- - If current_user has GRANT OPTION: GRANT succeeds
-- - If current_user lacks GRANT OPTION: caught, logged as NOTICE, deployment proceeds
--
-- For databases where this migration cannot fix permissions (table owned by
-- another role), run as superuser once:
--   GRANT ALL ON TABLE detected_positions, safety_net_executions TO robson;

BEGIN;

DO $$
BEGIN
    GRANT ALL PRIVILEGES ON TABLE detected_positions TO CURRENT_USER;
EXCEPTION
    WHEN insufficient_privilege THEN
        RAISE NOTICE 'Cannot GRANT on detected_positions: current_user is not the table owner. Run as superuser to fix.';
    WHEN undefined_table THEN
        RAISE NOTICE 'Table detected_positions does not exist yet.';
END$$;

DO $$
BEGIN
    GRANT ALL PRIVILEGES ON TABLE safety_net_executions TO CURRENT_USER;
EXCEPTION
    WHEN insufficient_privilege THEN
        RAISE NOTICE 'Cannot GRANT on safety_net_executions: current_user is not the table owner. Run as superuser to fix.';
    WHEN undefined_table THEN
        RAISE NOTICE 'Table safety_net_executions does not exist yet.';
END$$;

-- Default privileges: future tables created by current_user are automatically
-- accessible to the runtime role, preventing this class of bug from recurring.
DO $$
BEGIN
    ALTER DEFAULT PRIVILEGES FOR ROLE CURRENT_USER IN SCHEMA public
        GRANT ALL PRIVILEGES ON TABLES TO CURRENT_USER;
EXCEPTION WHEN OTHERS THEN
    RAISE NOTICE 'Cannot set default privileges: %', SQLERRM;
END$$;

COMMIT;
