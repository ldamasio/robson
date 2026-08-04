-- ADR-0052: one new-position stop policy with an immutable executable span.
--
-- Migration 000024 is already-applied history and remains untouched. Legacy
-- rows retain their provenance stamp and NULL admission-plan fields. New
-- executable_span rows receive these fields from entry_signal_received before
-- an entry order is submitted.

ALTER TABLE positions_current
    DROP CONSTRAINT IF EXISTS chk_positions_stop_policy;

ALTER TABLE positions_current
    ADD CONSTRAINT chk_positions_stop_policy
    CHECK (stop_policy IN ('legacy_uncapped', 'executable_span'));

ALTER TABLE positions_current
    ADD COLUMN IF NOT EXISTS initial_executable_stop DECIMAL(20, 8),
    ADD COLUMN IF NOT EXISTS executable_span DECIMAL(20, 8),
    ADD COLUMN IF NOT EXISTS cap_basis_distance DECIMAL(20, 8),
    ADD COLUMN IF NOT EXISTS tick_size_at_admission DECIMAL(20, 8);

COMMENT ON COLUMN positions_current.stop_policy IS
    'Stop-policy provenance pinned at arm: legacy_uncapped (historical derivation) or executable_span (ADR-0052). Never changes after arm.';
COMMENT ON COLUMN positions_current.initial_executable_stop IS
    'ADR-0052 admission-time executable trigger resolved before entry submission; NULL for legacy rows';
COMMENT ON COLUMN positions_current.executable_span IS
    'ADR-0052 immutable buffer-inclusive executable span S; NULL for legacy rows and never re-derived during replay';
COMMENT ON COLUMN positions_current.cap_basis_distance IS
    'ADR-0052 admission-time distance used for the 0.25x executable-stop buffer cap; NULL for legacy rows';
COMMENT ON COLUMN positions_current.tick_size_at_admission IS
    'Exchange tick size used to adversely quantize the ADR-0052 admission trigger; NULL for legacy rows';

-- Structural write-once barrier. Projection replay may fill a NULL column and
-- may repeat the same value idempotently, but no writer may replace immutable
-- admission evidence after it has been recorded.
CREATE OR REPLACE FUNCTION reject_adr0052_admission_evidence_rewrite()
RETURNS TRIGGER AS $$
BEGIN
    IF OLD.initial_executable_stop IS NOT NULL
       AND NEW.initial_executable_stop IS DISTINCT FROM OLD.initial_executable_stop THEN
        RAISE EXCEPTION 'initial_executable_stop is immutable once set';
    END IF;
    IF OLD.executable_span IS NOT NULL
       AND NEW.executable_span IS DISTINCT FROM OLD.executable_span THEN
        RAISE EXCEPTION 'executable_span is immutable once set';
    END IF;
    IF OLD.cap_basis_distance IS NOT NULL
       AND NEW.cap_basis_distance IS DISTINCT FROM OLD.cap_basis_distance THEN
        RAISE EXCEPTION 'cap_basis_distance is immutable once set';
    END IF;
    IF OLD.tick_size_at_admission IS NOT NULL
       AND NEW.tick_size_at_admission IS DISTINCT FROM OLD.tick_size_at_admission THEN
        RAISE EXCEPTION 'tick_size_at_admission is immutable once set';
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS positions_current_adr0052_admission_evidence_write_once
    ON positions_current;
CREATE TRIGGER positions_current_adr0052_admission_evidence_write_once
BEFORE UPDATE OF
    initial_executable_stop,
    executable_span,
    cap_basis_distance,
    tick_size_at_admission
ON positions_current
FOR EACH ROW
EXECUTE FUNCTION reject_adr0052_admission_evidence_rewrite();
