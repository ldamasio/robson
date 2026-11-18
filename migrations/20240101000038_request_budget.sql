DO $migration38$
BEGIN
    -- Additive only: no backfill, validation, worker or readiness grant.

    -- ddl 01: budget_clock_domain_v38
CREATE TYPE budget_clock_domain_v38 AS
    (kind TEXT,boot_uuid UUID,offset_seconds BIGINT,offset_nanoseconds INTEGER);

    -- ddl 02: budget_integer_fits
CREATE FUNCTION budget_integer_fits(n NUMERIC,lo NUMERIC,hi NUMERIC)
RETURNS BOOLEAN LANGUAGE plpgsql IMMUTABLE PARALLEL SAFE AS $fn$
BEGIN
    RETURN (n IS NOT NULL AND lo IS NOT NULL AND hi IS NOT NULL
            AND scale(n)=0 AND n>=lo AND n<=hi) IS TRUE;
END;
$fn$;

    -- ddl 03: clock_samples
CREATE TABLE clock_samples (
    sample_id UUID CONSTRAINT clock_samples_pkey PRIMARY KEY CONSTRAINT clock_samples_sample_id_check CHECK(sample_id<>'00000000-0000-0000-0000-000000000000'::uuid),
    tenant_id UUID NOT NULL CONSTRAINT clock_samples_tenant_id_check CHECK(tenant_id<>'00000000-0000-0000-0000-000000000000'::uuid),venue_id TEXT NOT NULL CONSTRAINT clock_samples_venue_id_check CHECK(venue_id<>''),
    process_epoch UUID NOT NULL CONSTRAINT clock_samples_process_epoch_check CHECK(process_epoch<>'00000000-0000-0000-0000-000000000000'::uuid),
    clock_domain budget_clock_domain_v38 NOT NULL CONSTRAINT clock_samples_clock_domain_check CHECK((num_nulls((clock_domain).kind,(clock_domain).boot_uuid,(clock_domain).offset_seconds,(clock_domain).offset_nanoseconds)=0
        AND (clock_domain).kind='boottime' 
        AND (clock_domain).boot_uuid<>'00000000-0000-0000-0000-000000000000'::uuid
        AND (clock_domain).offset_nanoseconds BETWEEN 0 AND 999999999) IS TRUE),
    send_wall_seconds BIGINT NOT NULL,send_wall_nanoseconds INTEGER NOT NULL CONSTRAINT clock_samples_send_wall_nanoseconds_check CHECK(send_wall_nanoseconds BETWEEN 0 AND 999999999),
    send_tick_before NUMERIC NOT NULL CONSTRAINT clock_samples_send_tick_before_check CHECK(budget_integer_fits(send_tick_before,0,9223372036854775807999999999)),send_tick_after NUMERIC NOT NULL CONSTRAINT clock_samples_send_tick_after_check CHECK(budget_integer_fits(send_tick_after,0,9223372036854775807999999999)),
    receive_wall_seconds BIGINT NOT NULL,receive_wall_nanoseconds INTEGER NOT NULL CONSTRAINT clock_samples_receive_wall_nanoseconds_check CHECK(receive_wall_nanoseconds BETWEEN 0 AND 999999999),
    receive_tick_before NUMERIC NOT NULL CONSTRAINT clock_samples_receive_tick_before_check CHECK(budget_integer_fits(receive_tick_before,0,9223372036854775807999999999)),receive_tick_after NUMERIC NOT NULL CONSTRAINT clock_samples_receive_tick_after_check CHECK(budget_integer_fits(receive_tick_after,0,9223372036854775807999999999)),
    server_ms BIGINT NOT NULL CONSTRAINT clock_samples_server_ms_check CHECK(server_ms>=0),
    theta2_ns NUMERIC NOT NULL CONSTRAINT clock_samples_theta2_ns_check CHECK(budget_integer_fits(theta2_ns,-18446744073709551615999999998,18465190817783261167614000000)),
    r_low_ns NUMERIC NOT NULL CONSTRAINT clock_samples_r_low_ns_check CHECK(budget_integer_fits(r_low_ns,0,9223372036854775807999999999)),r_high_ns NUMERIC NOT NULL CONSTRAINT clock_samples_r_high_ns_check CHECK(budget_integer_fits(r_high_ns,0,9223372036854775807999999999)),
    u2_ns NUMERIC NOT NULL CONSTRAINT clock_samples_u2_ns_check CHECK(budget_integer_fits(u2_ns,0,27688562854638036975813999999)),
    numeric_verdict TEXT NOT NULL CONSTRAINT clock_samples_numeric_verdict_check CHECK(numeric_verdict IN ('valid','invalid')),
    numeric_failures SMALLINT NOT NULL CONSTRAINT clock_samples_numeric_failures_check CHECK(numeric_failures BETWEEN 0 AND 3),algorithm_version SMALLINT NOT NULL CONSTRAINT clock_samples_algorithm_version_check CHECK(algorithm_version=1),
    recorded_at TIMESTAMPTZ NOT NULL,
    CONSTRAINT clock_samples_identity UNIQUE(sample_id,tenant_id,venue_id),
    CONSTRAINT clock_samples_brackets CHECK(send_tick_before<=send_tick_after AND send_tick_after<=receive_tick_before AND receive_tick_before<=receive_tick_after),
    CONSTRAINT clock_samples_wall_order CHECK((receive_wall_seconds::numeric*1000000000+receive_wall_nanoseconds)>=(send_wall_seconds::numeric*1000000000+send_wall_nanoseconds)),
    CONSTRAINT clock_samples_arithmetic CHECK(
        theta2_ns=server_ms::numeric*2000000-(send_wall_seconds::numeric*1000000000+send_wall_nanoseconds)-(receive_wall_seconds::numeric*1000000000+receive_wall_nanoseconds)
        AND r_low_ns=receive_tick_before-send_tick_after AND r_high_ns=receive_tick_after-send_tick_before
        AND u2_ns=abs(theta2_ns)+r_high_ns+200000000),
    CONSTRAINT clock_samples_verdict CHECK(numeric_failures=
        (CASE WHEN u2_ns>2000000000 THEN 1 ELSE 0 END)+
        (CASE WHEN abs((receive_wall_seconds::numeric-send_wall_seconds::numeric)*1000000000+receive_wall_nanoseconds-send_wall_nanoseconds-r_low_ns)>50000000
                    OR abs((receive_wall_seconds::numeric-send_wall_seconds::numeric)*1000000000+receive_wall_nanoseconds-send_wall_nanoseconds-r_high_ns)>50000000 THEN 2 ELSE 0 END)
        AND numeric_verdict=CASE WHEN numeric_failures=0 THEN 'valid' ELSE 'invalid' END)
);

    -- ddl 04: clock_samples_immutable
CREATE TRIGGER clock_samples_immutable BEFORE UPDATE ON clock_samples
    FOR EACH ROW EXECUTE FUNCTION ledger_row_is_immutable();

    -- ddl 05: clock_samples_epoch_fk
ALTER TABLE clock_samples ADD CONSTRAINT clock_samples_epoch_fk FOREIGN KEY(process_epoch) REFERENCES process_epochs(epoch_id) NOT VALID;

    -- ddl 06: venue_pauses
CREATE TABLE venue_pauses (
    tenant_id UUID NOT NULL,venue_id TEXT NOT NULL,CONSTRAINT venue_pauses_pkey PRIMARY KEY(tenant_id,venue_id),
    status SMALLINT NOT NULL CONSTRAINT venue_pauses_status_check CHECK(status IN (418,429)),code INTEGER,
    pause_state TEXT NOT NULL CONSTRAINT venue_pauses_pause_state_check CHECK(pause_state IN ('bounded','unresolved')),
    observation_id UUID NOT NULL CONSTRAINT venue_pauses_observation_id_check CHECK(observation_id<>'00000000-0000-0000-0000-000000000000'::uuid),
    original_domain budget_clock_domain_v38,observed_tick NUMERIC,duration_ns NUMERIC,
    restoration_domain budget_clock_domain_v38,restoration_anchor NUMERIC,deadline_tick NUMERIC,
    until_at TIMESTAMPTZ,recorded_at TIMESTAMPTZ NOT NULL,
    CONSTRAINT venue_pauses_original CHECK((num_nulls(original_domain,observed_tick,duration_ns)=3 OR
        (num_nulls(original_domain,observed_tick,duration_ns)=0 AND ((num_nulls((original_domain).kind,(original_domain).boot_uuid,(original_domain).offset_seconds,(original_domain).offset_nanoseconds)=0
        AND (original_domain).kind='boottime' 
        AND (original_domain).boot_uuid<>'00000000-0000-0000-0000-000000000000'::uuid
        AND (original_domain).offset_nanoseconds BETWEEN 0 AND 999999999) IS TRUE) AND budget_integer_fits(observed_tick,0,9223372036854775807999999999) AND budget_integer_fits(duration_ns,0,9223372036854775807999999999))) IS TRUE),
    CONSTRAINT venue_pauses_restoration CHECK((num_nulls(restoration_domain,restoration_anchor,deadline_tick)=3 OR
        (num_nulls(restoration_domain,restoration_anchor,deadline_tick)=0 AND ((num_nulls((restoration_domain).kind,(restoration_domain).boot_uuid,(restoration_domain).offset_seconds,(restoration_domain).offset_nanoseconds)=0
        AND (restoration_domain).kind='boottime' 
        AND (restoration_domain).boot_uuid<>'00000000-0000-0000-0000-000000000000'::uuid
        AND (restoration_domain).offset_nanoseconds BETWEEN 0 AND 999999999) IS TRUE)
         AND budget_integer_fits(restoration_anchor,0,9223372036854775807999999999) AND budget_integer_fits(deadline_tick,0,9223372036854775807999999999) AND deadline_tick=restoration_anchor+duration_ns)) IS TRUE),
    CONSTRAINT venue_pauses_bounded CHECK((pause_state='unresolved' OR
        (num_nulls(original_domain,observed_tick,duration_ns,restoration_domain,restoration_anchor,deadline_tick)=0)) IS TRUE)
);

    -- ddl 07: budget_pause_moves_forward
CREATE FUNCTION budget_pause_moves_forward() RETURNS TRIGGER LANGUAGE plpgsql AS $fn$
BEGIN
    IF TG_OP='DELETE' OR (NEW.tenant_id,NEW.venue_id) IS DISTINCT FROM (OLD.tenant_id,OLD.venue_id)
       OR NEW.recorded_at<OLD.recorded_at
       OR (NEW.observation_id=OLD.observation_id AND
           (NEW.status,NEW.code,NEW.original_domain,NEW.observed_tick,NEW.duration_ns) IS DISTINCT FROM
           (OLD.status,OLD.code,OLD.original_domain,OLD.observed_tick,OLD.duration_ns))
       OR (NEW.restoration_domain IS NOT DISTINCT FROM OLD.restoration_domain AND NEW.deadline_tick<OLD.deadline_tick)
       OR (NEW.restoration_domain IS DISTINCT FROM OLD.restoration_domain AND NEW.pause_state='bounded'
           AND NEW.deadline_tick-NEW.restoration_anchor<OLD.duration_ns) THEN
        RAISE EXCEPTION 'venue pause cannot lose identity or shorten comparable restriction' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$fn$;

    -- ddl 08: budget_pause_moves_forward
CREATE TRIGGER budget_pause_moves_forward BEFORE UPDATE OR DELETE ON venue_pauses
    FOR EACH ROW EXECUTE FUNCTION budget_pause_moves_forward();

    -- ddl 09: incident_budget_usage
CREATE TABLE incident_budget_usage (
    allocation_id UUID CONSTRAINT incident_budget_usage_pkey PRIMARY KEY CONSTRAINT incident_budget_usage_allocation_id_check CHECK(allocation_id<>'00000000-0000-0000-0000-000000000000'::uuid),incident_id UUID NOT NULL,
    tenant_id UUID NOT NULL,venue_id TEXT NOT NULL,generation BIGINT NOT NULL CONSTRAINT incident_budget_usage_generation_check CHECK(generation>=0),
    request_id UUID NOT NULL CONSTRAINT incident_budget_usage_request_id_check CHECK(request_id<>'00000000-0000-0000-0000-000000000000'::uuid),content_digest BYTEA NOT NULL CONSTRAINT incident_budget_usage_content_digest_check CHECK(octet_length(content_digest)=32),
    request_class TEXT NOT NULL CONSTRAINT incident_budget_usage_request_class_check CHECK(request_class IN ('steady','protective')),
    weight INTEGER NOT NULL CONSTRAINT incident_budget_usage_weight_check CHECK(weight BETWEEN 1 AND 1200),orders SMALLINT NOT NULL CONSTRAINT incident_budget_usage_orders_check CHECK(orders BETWEEN 0 AND 20),
    clock_domain budget_clock_domain_v38 NOT NULL CONSTRAINT incident_budget_usage_clock_domain_check CHECK((num_nulls((clock_domain).kind,(clock_domain).boot_uuid,(clock_domain).offset_seconds,(clock_domain).offset_nanoseconds)=0
        AND (clock_domain).kind='boottime' 
        AND (clock_domain).boot_uuid<>'00000000-0000-0000-0000-000000000000'::uuid
        AND (clock_domain).offset_nanoseconds BETWEEN 0 AND 999999999) IS TRUE),
    from_tick NUMERIC NOT NULL CONSTRAINT incident_budget_usage_from_tick_check CHECK(budget_integer_fits(from_tick,0,9223372036854775807999999999)),to_tick NUMERIC NOT NULL CONSTRAINT incident_budget_usage_to_tick_check CHECK(budget_integer_fits(to_tick,0,9223372036854775807999999999)),
    recorded_at TIMESTAMPTZ NOT NULL,CONSTRAINT budget_usage_interval CHECK(from_tick<=to_tick)
);

    -- ddl 10: incident_budget_usage_incident
CREATE INDEX incident_budget_usage_incident ON incident_budget_usage(incident_id,generation,allocation_id);

    -- ddl 11: budget_allocation_equal_replay
CREATE FUNCTION budget_allocation_equal_replay() RETURNS TRIGGER LANGUAGE plpgsql AS $fn$
BEGIN
    IF TG_OP='DELETE' OR NEW IS DISTINCT FROM OLD THEN
        RAISE EXCEPTION 'allocation replay must preserve complete content' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$fn$;

    -- ddl 12: budget_allocation_equal_replay
CREATE TRIGGER budget_allocation_equal_replay BEFORE UPDATE OR DELETE ON incident_budget_usage
    FOR EACH ROW EXECUTE FUNCTION budget_allocation_equal_replay();

    -- ddl 13: budget_usage_incident_fk
ALTER TABLE incident_budget_usage ADD CONSTRAINT budget_usage_incident_fk FOREIGN KEY(incident_id) REFERENCES recovery_incidents(incident_id) NOT VALID;

    -- ddl 14: recovery_incidents_reason_v38
ALTER TABLE recovery_incidents ADD CONSTRAINT recovery_incidents_reason_v38 CHECK(reason IN ('unclean_start','uncertain_outcome','unlanded_send','persistence_failure','unpermitted_mutation','stream_suspended','operation_aborted','generation_exhausted','venue_discrepancy','liability_unknown','request_budget_emergency')) NOT VALID;

    -- ddl 15: venue_orders_capture_columns
ALTER TABLE venue_orders
    ADD COLUMN live_capture_epoch UUID,
    ADD COLUMN live_wall_seconds BIGINT,
    ADD COLUMN live_wall_nanoseconds INTEGER,
    ADD COLUMN live_tick_before NUMERIC,
    ADD COLUMN live_tick_after NUMERIC,
    ADD COLUMN live_capture_domain budget_clock_domain_v38,
    ADD COLUMN terminal_capture_epoch UUID,
    ADD COLUMN terminal_wall_seconds BIGINT,
    ADD COLUMN terminal_wall_nanoseconds INTEGER,
    ADD COLUMN terminal_tick_before NUMERIC,
    ADD COLUMN terminal_tick_after NUMERIC,
    ADD COLUMN terminal_capture_domain budget_clock_domain_v38;

    -- ddl 16: venue_orders_capture_constraints
ALTER TABLE venue_orders
    ADD CONSTRAINT venue_orders_live_capture_whole CHECK((num_nulls(live_capture_epoch,live_wall_seconds,live_wall_nanoseconds,live_tick_before,live_tick_after,live_capture_domain)=6 OR
        (num_nulls(live_capture_epoch,live_wall_seconds,live_wall_nanoseconds,live_tick_before,live_tick_after,live_capture_domain)=0 AND live_capture_epoch<>'00000000-0000-0000-0000-000000000000'::uuid
         AND live_wall_nanoseconds BETWEEN 0 AND 999999999 AND budget_integer_fits(live_tick_before,0,9223372036854775807999999999) AND budget_integer_fits(live_tick_after,0,9223372036854775807999999999)
         AND live_tick_before<=live_tick_after AND ((num_nulls((live_capture_domain).kind,(live_capture_domain).boot_uuid,(live_capture_domain).offset_seconds,(live_capture_domain).offset_nanoseconds)=0
        AND (live_capture_domain).kind='boottime' 
        AND (live_capture_domain).boot_uuid<>'00000000-0000-0000-0000-000000000000'::uuid
        AND (live_capture_domain).offset_nanoseconds BETWEEN 0 AND 999999999) IS TRUE))) IS TRUE) NOT VALID,
    ADD CONSTRAINT venue_orders_terminal_capture_whole CHECK((num_nulls(terminal_capture_epoch,terminal_wall_seconds,terminal_wall_nanoseconds,terminal_tick_before,terminal_tick_after,terminal_capture_domain)=6 OR
        (num_nulls(terminal_capture_epoch,terminal_wall_seconds,terminal_wall_nanoseconds,terminal_tick_before,terminal_tick_after,terminal_capture_domain)=0 AND terminal_capture_epoch<>'00000000-0000-0000-0000-000000000000'::uuid
         AND terminal_wall_nanoseconds BETWEEN 0 AND 999999999 AND budget_integer_fits(terminal_tick_before,0,9223372036854775807999999999) AND budget_integer_fits(terminal_tick_after,0,9223372036854775807999999999)
         AND terminal_tick_before<=terminal_tick_after AND ((num_nulls((terminal_capture_domain).kind,(terminal_capture_domain).boot_uuid,(terminal_capture_domain).offset_seconds,(terminal_capture_domain).offset_nanoseconds)=0
        AND (terminal_capture_domain).kind='boottime' 
        AND (terminal_capture_domain).boot_uuid<>'00000000-0000-0000-0000-000000000000'::uuid
        AND (terminal_capture_domain).offset_nanoseconds BETWEEN 0 AND 999999999) IS TRUE))) IS TRUE) NOT VALID,
    ADD CONSTRAINT venue_orders_live_sample_fk FOREIGN KEY(live_clock_sample_id,tenant_id,venue_id) REFERENCES clock_samples(sample_id,tenant_id,venue_id) NOT VALID,
    ADD CONSTRAINT venue_orders_terminal_sample_fk FOREIGN KEY(terminal_clock_sample_id,tenant_id,venue_id) REFERENCES clock_samples(sample_id,tenant_id,venue_id) NOT VALID;

    -- ddl 17: venue_orders_move_forward
CREATE OR REPLACE FUNCTION venue_orders_move_forward() RETURNS TRIGGER LANGUAGE plpgsql AS $fn$
BEGIN
    IF TG_OP = 'DELETE'
       -- identity and first-seen evidence, primary key included
       OR (NEW.venue_id, NEW.account_scope, NEW.symbol, NEW.exchange_order_id, NEW.tenant_id, NEW.client_order_id,
           NEW.side, NEW.order_type, NEW.orig_qty, NEW.stop_price, NEW.live_seen_at, NEW.live_clock_sample_id,
           NEW.live_clock_wall_delta_ms, NEW.live_clock_mono_delta_ms, NEW.live_clock_verdict,
           NEW.live_capture_epoch, NEW.live_wall_seconds, NEW.live_wall_nanoseconds, NEW.live_tick_before, NEW.live_tick_after, NEW.live_capture_domain)
          IS DISTINCT FROM
          (OLD.venue_id, OLD.account_scope, OLD.symbol, OLD.exchange_order_id, OLD.tenant_id, OLD.client_order_id,
           OLD.side, OLD.order_type, OLD.orig_qty, OLD.stop_price, OLD.live_seen_at, OLD.live_clock_sample_id,
           OLD.live_clock_wall_delta_ms, OLD.live_clock_mono_delta_ms, OLD.live_clock_verdict,
           OLD.live_capture_epoch, OLD.live_wall_seconds, OLD.live_wall_nanoseconds, OLD.live_tick_before, OLD.live_tick_after, OLD.live_capture_domain)
       -- ownership: a leg link is permanent; a non-leg owner changes only by adoption into a leg
       OR (OLD.owner = 'leg' AND (NEW.owner, NEW.intent_id, NEW.position_id, NEW.kind)
                                 IS DISTINCT FROM (OLD.owner, OLD.intent_id, OLD.position_id, OLD.kind))
       OR (OLD.owner <> 'leg' AND NEW.owner <> 'leg' AND NEW.owner <> OLD.owner)
       OR NEW.executed_cum < OLD.executed_cum OR NEW.fills_qty < OLD.fills_qty
       OR NEW.trades_cursor < OLD.trades_cursor OR NEW.last_checked_at < OLD.last_checked_at
       -- a terminal order's state and its terminal evidence are final
       OR (OLD.phase = 'terminal' AND
           (NEW.phase, NEW.status, NEW.executed_cum, NEW.cumulative_quote, NEW.expiry_reason, NEW.terminal_seen_at,
            NEW.terminal_clock_sample_id, NEW.terminal_clock_wall_delta_ms, NEW.terminal_clock_mono_delta_ms,
            NEW.terminal_clock_verdict,
            NEW.terminal_capture_epoch, NEW.terminal_wall_seconds, NEW.terminal_wall_nanoseconds, NEW.terminal_tick_before, NEW.terminal_tick_after, NEW.terminal_capture_domain)
           IS DISTINCT FROM
           (OLD.phase, OLD.status, OLD.executed_cum, OLD.cumulative_quote, OLD.expiry_reason, OLD.terminal_seen_at,
            OLD.terminal_clock_sample_id, OLD.terminal_clock_wall_delta_ms, OLD.terminal_clock_mono_delta_ms,
            OLD.terminal_clock_verdict,
            OLD.terminal_capture_epoch, OLD.terminal_wall_seconds, OLD.terminal_wall_nanoseconds, OLD.terminal_tick_before, OLD.terminal_tick_after, OLD.terminal_capture_domain))
    THEN
        RAISE EXCEPTION 'ledger invariant: venue_orders % on % only moves forward', TG_OP, OLD.exchange_order_id
            USING ERRCODE = 'check_violation';
    END IF;
    RETURN NEW;
END;
$fn$;

    -- ddl 18: spot_intent_clock_capture_columns
ALTER TABLE spot_intent_clock
    ADD COLUMN budget_clock_domain budget_clock_domain_v38,ADD COLUMN budget_created_tick NUMERIC,
    ADD COLUMN budget_recorded_at TIMESTAMPTZ,ADD COLUMN capture_epoch UUID,
    ADD COLUMN capture_wall_seconds BIGINT,ADD COLUMN capture_wall_nanoseconds INTEGER,
    ADD COLUMN capture_tick_before NUMERIC,ADD COLUMN capture_tick_after NUMERIC,
    ADD COLUMN capture_domain budget_clock_domain_v38;

    -- ddl 19: spot_intent_clock_receipt_default
ALTER TABLE spot_intent_clock ALTER COLUMN budget_recorded_at SET DEFAULT CURRENT_TIMESTAMP;

    -- ddl 20: spot_intent_clock_constraints
ALTER TABLE spot_intent_clock
    ADD CONSTRAINT spot_intent_clock_budget_whole CHECK((num_nulls(budget_clock_domain,budget_created_tick)=2 OR
        (num_nulls(budget_clock_domain,budget_created_tick)=0 AND ((num_nulls((budget_clock_domain).kind,(budget_clock_domain).boot_uuid,(budget_clock_domain).offset_seconds,(budget_clock_domain).offset_nanoseconds)=0
        AND (budget_clock_domain).kind='boottime' 
        AND (budget_clock_domain).boot_uuid<>'00000000-0000-0000-0000-000000000000'::uuid
        AND (budget_clock_domain).offset_nanoseconds BETWEEN 0 AND 999999999) IS TRUE) AND budget_integer_fits(budget_created_tick,0,9223372036854775807999999999))) IS TRUE) NOT VALID,
    ADD CONSTRAINT spot_intent_clock_capture_whole CHECK((num_nulls(capture_epoch,capture_wall_seconds,capture_wall_nanoseconds,capture_tick_before,capture_tick_after,capture_domain)=6 OR
        (num_nulls(capture_epoch,capture_wall_seconds,capture_wall_nanoseconds,capture_tick_before,capture_tick_after,capture_domain)=0 AND capture_epoch<>'00000000-0000-0000-0000-000000000000'::uuid
         AND capture_wall_nanoseconds BETWEEN 0 AND 999999999 AND budget_integer_fits(capture_tick_before,0,9223372036854775807999999999) AND budget_integer_fits(capture_tick_after,0,9223372036854775807999999999)
         AND capture_tick_before<=capture_tick_after AND ((num_nulls((capture_domain).kind,(capture_domain).boot_uuid,(capture_domain).offset_seconds,(capture_domain).offset_nanoseconds)=0
        AND (capture_domain).kind='boottime' 
        AND (capture_domain).boot_uuid<>'00000000-0000-0000-0000-000000000000'::uuid
        AND (capture_domain).offset_nanoseconds BETWEEN 0 AND 999999999) IS TRUE))) IS TRUE) NOT VALID,
    ADD CONSTRAINT spot_intent_clock_sample_fk FOREIGN KEY(clock_sample_id,tenant_id,venue_id) REFERENCES clock_samples(sample_id,tenant_id,venue_id) NOT VALID,
    ADD CONSTRAINT spot_intent_clock_attempt_fk FOREIGN KEY(attempt_id) REFERENCES submit_attempts(attempt_id) NOT VALID,
    ADD CONSTRAINT spot_intent_clock_intent_fk FOREIGN KEY(intent_id) REFERENCES order_intents(intent_id) NOT VALID;

    -- ddl 21: budget_venue_state
CREATE TABLE budget_venue_state (
    venue_id TEXT CONSTRAINT budget_venue_state_pkey PRIMARY KEY,initialization_state TEXT NOT NULL DEFAULT 'incomplete' CONSTRAINT budget_venue_state_initialization_state_check CHECK(initialization_state IN ('incomplete','complete')),
    initialization_id UUID,audit_id UUID,quiescence_generation BIGINT NOT NULL DEFAULT 0 CONSTRAINT budget_venue_state_quiescence_generation_check CHECK(quiescence_generation>=0),
    schema_version SMALLINT NOT NULL DEFAULT 38 CONSTRAINT budget_venue_state_schema_version_check CHECK(schema_version=38),proof_version SMALLINT NOT NULL DEFAULT 1 CONSTRAINT budget_venue_state_proof_version_check CHECK(proof_version=1),
    recovery_generation BIGINT NOT NULL DEFAULT 0 CONSTRAINT budget_venue_state_recovery_generation_check CHECK(recovery_generation>=0),recovery_status TEXT NOT NULL DEFAULT 'unknown' CONSTRAINT budget_venue_state_recovery_status_check CHECK(recovery_status IN ('unknown','comparable','quarantined')),
    cohort_id UUID,source_high_water UUID,epoch_count_60 SMALLINT NOT NULL DEFAULT 9 CONSTRAINT budget_venue_state_epoch_count_60_check CHECK(epoch_count_60 BETWEEN 0 AND 9),
    epoch_count_10 SMALLINT NOT NULL DEFAULT 3 CONSTRAINT budget_venue_state_epoch_count_10_check CHECK(epoch_count_10 BETWEEN 0 AND 3),recorded_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT budget_venue_initialized CHECK((initialization_state<>'complete' OR
        (initialization_id IS NOT NULL AND initialization_id<>'00000000-0000-0000-0000-000000000000'::uuid AND audit_id IS NOT NULL AND audit_id<>'00000000-0000-0000-0000-000000000000'::uuid)) IS TRUE)
);

    -- ddl 22: budget_venue_moves_forward
CREATE FUNCTION budget_venue_moves_forward() RETURNS TRIGGER LANGUAGE plpgsql AS $fn$
BEGIN
    IF TG_OP='DELETE' OR NEW.venue_id IS DISTINCT FROM OLD.venue_id
       OR (NEW.schema_version,NEW.proof_version) IS DISTINCT FROM (OLD.schema_version,OLD.proof_version)
       OR NEW.quiescence_generation<OLD.quiescence_generation OR NEW.recovery_generation<OLD.recovery_generation
       OR (OLD.initialization_state='complete' AND (NEW.initialization_state<>'complete' OR NEW.initialization_id IS DISTINCT FROM OLD.initialization_id))
       OR (NEW.quiescence_generation=OLD.quiescence_generation AND OLD.audit_id IS NOT NULL AND
           (NEW.audit_id,NEW.source_high_water) IS DISTINCT FROM (OLD.audit_id,OLD.source_high_water)) THEN
        RAISE EXCEPTION 'venue initialization cannot reset or lose provenance' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$fn$;

    -- ddl 23: budget_venue_moves_forward
CREATE TRIGGER budget_venue_moves_forward BEFORE UPDATE OR DELETE ON budget_venue_state
    FOR EACH ROW EXECUTE FUNCTION budget_venue_moves_forward();

    -- ddl 24: budget_epochs
CREATE TABLE budget_epochs (
    process_epoch UUID CONSTRAINT budget_epochs_pkey PRIMARY KEY CONSTRAINT budget_epochs_process_epoch_check CHECK(process_epoch<>'00000000-0000-0000-0000-000000000000'::uuid),venue_id TEXT NOT NULL,
    clock_domain budget_clock_domain_v38 NOT NULL CONSTRAINT budget_epochs_clock_domain_check CHECK((num_nulls((clock_domain).kind,(clock_domain).boot_uuid,(clock_domain).offset_seconds,(clock_domain).offset_nanoseconds)=0
        AND (clock_domain).kind='boottime' 
        AND (clock_domain).boot_uuid<>'00000000-0000-0000-0000-000000000000'::uuid
        AND (clock_domain).offset_nanoseconds BETWEEN 0 AND 999999999) IS TRUE),
    start_tick NUMERIC NOT NULL CONSTRAINT budget_epochs_start_tick_check CHECK(budget_integer_fits(start_tick,0,9223372036854775807999999999)),predecessor_epoch UUID,predecessor_clean BOOLEAN,
    recorded_at TIMESTAMPTZ NOT NULL,CONSTRAINT budget_epoch_predecessor CHECK((predecessor_epoch IS NULL)=(predecessor_clean IS NULL))
);

    -- ddl 25: budget_epochs_venue_time
CREATE INDEX budget_epochs_venue_time ON budget_epochs(venue_id,((clock_domain).kind),((clock_domain).boot_uuid),((clock_domain).offset_seconds),((clock_domain).offset_nanoseconds),start_tick,process_epoch);

    -- ddl 26: budget_epochs_process_fk
ALTER TABLE budget_epochs ADD CONSTRAINT budget_epochs_process_fk FOREIGN KEY(process_epoch) REFERENCES process_epochs(epoch_id) NOT VALID;

    -- ddl 27: budget_epochs_immutable
CREATE TRIGGER budget_epochs_immutable BEFORE UPDATE OR DELETE ON budget_epochs
    FOR EACH ROW EXECUTE FUNCTION ledger_row_is_immutable();

    -- ddl 28: budget_source_audits
CREATE TABLE budget_source_audits (
    audit_id UUID CONSTRAINT budget_source_audits_pkey PRIMARY KEY CONSTRAINT budget_source_audits_audit_id_check CHECK(audit_id<>'00000000-0000-0000-0000-000000000000'::uuid),venue_id TEXT NOT NULL,quiescence_generation BIGINT NOT NULL CONSTRAINT budget_source_audits_quiescence_generation_check CHECK(quiescence_generation>=0),
    high_water UUID,cursor UUID,examined_count BIGINT NOT NULL DEFAULT 0 CONSTRAINT budget_source_audits_examined_count_check CHECK(examined_count BETWEEN 0 AND 1000000),
    status TEXT NOT NULL CONSTRAINT budget_source_audits_status_check CHECK(status IN ('running','complete','failed')),failure_reason TEXT CONSTRAINT budget_source_audits_failure_reason_check CHECK(octet_length(failure_reason) BETWEEN 1 AND 64),
    started_at TIMESTAMPTZ NOT NULL,completed_at TIMESTAMPTZ,schema_version SMALLINT NOT NULL CONSTRAINT budget_source_audits_schema_version_check CHECK(schema_version=38),proof_generation BIGINT NOT NULL CONSTRAINT budget_source_audits_proof_generation_check CHECK(proof_generation>=0),
    CONSTRAINT budget_audit_cursor CHECK((high_water IS NOT NULL OR (cursor IS NULL AND examined_count=0)) AND (cursor IS NULL OR cursor<=high_water)),
    CONSTRAINT budget_audit_terminal CHECK((status='running' AND completed_at IS NULL AND failure_reason IS NULL)
        OR (status='complete' AND completed_at IS NOT NULL AND failure_reason IS NULL AND cursor IS NOT DISTINCT FROM high_water)
        OR (status='failed' AND completed_at IS NOT NULL AND failure_reason IS NOT NULL))
);

    -- ddl 29: budget_audit_moves_forward
CREATE FUNCTION budget_audit_moves_forward() RETURNS TRIGGER LANGUAGE plpgsql AS $fn$
BEGIN
    IF TG_OP='DELETE' OR
       (NEW.audit_id,NEW.venue_id,NEW.quiescence_generation,NEW.high_water,NEW.started_at,NEW.schema_version,NEW.proof_generation)
           IS DISTINCT FROM (OLD.audit_id,OLD.venue_id,OLD.quiescence_generation,OLD.high_water,OLD.started_at,OLD.schema_version,OLD.proof_generation)
       OR NEW.examined_count<OLD.examined_count OR (OLD.cursor IS NOT NULL AND (NEW.cursor IS NULL OR NEW.cursor<OLD.cursor))
       OR (OLD.status<>'running' AND NEW IS DISTINCT FROM OLD) THEN
        RAISE EXCEPTION 'source audit identity cursor and terminal state are monotone' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$fn$;

    -- ddl 30: budget_audit_moves_forward
CREATE TRIGGER budget_audit_moves_forward BEFORE UPDATE OR DELETE ON budget_source_audits
    FOR EACH ROW EXECUTE FUNCTION budget_audit_moves_forward();

    -- ddl 31: budget_recovery_cohorts
CREATE TABLE budget_recovery_cohorts (
    cohort_id UUID CONSTRAINT budget_recovery_cohorts_pkey PRIMARY KEY CONSTRAINT budget_recovery_cohorts_cohort_id_check CHECK(cohort_id<>'00000000-0000-0000-0000-000000000000'::uuid),venue_id TEXT NOT NULL,audit_id UUID NOT NULL CONSTRAINT budget_recovery_cohorts_audit_id_fkey REFERENCES budget_source_audits(audit_id),
    high_water UUID,source_generation BIGINT NOT NULL CONSTRAINT budget_recovery_cohorts_source_generation_check CHECK(source_generation>=0),
    clock_domain budget_clock_domain_v38 NOT NULL CONSTRAINT budget_recovery_cohorts_clock_domain_check CHECK((num_nulls((clock_domain).kind,(clock_domain).boot_uuid,(clock_domain).offset_seconds,(clock_domain).offset_nanoseconds)=0
        AND (clock_domain).kind='boottime' 
        AND (clock_domain).boot_uuid<>'00000000-0000-0000-0000-000000000000'::uuid
        AND (clock_domain).offset_nanoseconds BETWEEN 0 AND 999999999) IS TRUE),anchor_tick NUMERIC NOT NULL CONSTRAINT budget_recovery_cohorts_anchor_tick_check CHECK(budget_integer_fits(anchor_tick,0,9223372036854775807999999999)),
    recovery_generation BIGINT NOT NULL CONSTRAINT budget_recovery_cohorts_recovery_generation_check CHECK(recovery_generation>=0),algorithm_version SMALLINT NOT NULL CONSTRAINT budget_recovery_cohorts_algorithm_version_check CHECK(algorithm_version=1),recorded_at TIMESTAMPTZ NOT NULL
);

    -- ddl 32: budget_cohorts_immutable
CREATE TRIGGER budget_cohorts_immutable BEFORE UPDATE OR DELETE ON budget_recovery_cohorts
    FOR EACH ROW EXECUTE FUNCTION ledger_row_is_immutable();

    -- ddl 33: budget_expiry_proofs
CREATE TABLE budget_expiry_proofs (
    proof_id UUID CONSTRAINT budget_expiry_proofs_pkey PRIMARY KEY CONSTRAINT budget_expiry_proofs_proof_id_check CHECK(proof_id<>'00000000-0000-0000-0000-000000000000'::uuid),venue_id TEXT NOT NULL,proof_kind TEXT NOT NULL CONSTRAINT budget_expiry_proofs_proof_kind_check CHECK(proof_kind IN ('same_domain','cohort')),
    source_attempt_id UUID,cohort_id UUID CONSTRAINT budget_expiry_proofs_cohort_id_fkey REFERENCES budget_recovery_cohorts(cohort_id),
    clock_domain budget_clock_domain_v38 NOT NULL CONSTRAINT budget_expiry_proofs_clock_domain_check CHECK((num_nulls((clock_domain).kind,(clock_domain).boot_uuid,(clock_domain).offset_seconds,(clock_domain).offset_nanoseconds)=0
        AND (clock_domain).kind='boottime' 
        AND (clock_domain).boot_uuid<>'00000000-0000-0000-0000-000000000000'::uuid
        AND (clock_domain).offset_nanoseconds BETWEEN 0 AND 999999999) IS TRUE),
    start_tick NUMERIC NOT NULL CONSTRAINT budget_expiry_proofs_start_tick_check CHECK(budget_integer_fits(start_tick,0,9223372036854775807999999999)),end_tick NUMERIC NOT NULL CONSTRAINT budget_expiry_proofs_end_tick_check CHECK(budget_integer_fits(end_tick,0,9223372036854775807999999999)),
    high_water UUID,source_generation BIGINT,recorded_at TIMESTAMPTZ NOT NULL,CONSTRAINT budget_proof_identity UNIQUE(proof_id,venue_id),
    CONSTRAINT budget_proof_elapsed CHECK(end_tick-start_tick>=86460000000000),
    CONSTRAINT budget_proof_kind_whole CHECK((proof_kind='same_domain' AND source_attempt_id IS NOT NULL AND source_attempt_id<>'00000000-0000-0000-0000-000000000000'::uuid AND cohort_id IS NULL AND high_water IS NULL AND source_generation IS NULL)
        OR (proof_kind='cohort' AND source_attempt_id IS NULL AND cohort_id IS NOT NULL AND source_generation IS NOT NULL AND source_generation>=0))
);

    -- ddl 34: budget_expiry_proofs_immutable
CREATE TRIGGER budget_expiry_proofs_immutable BEFORE UPDATE OR DELETE ON budget_expiry_proofs
    FOR EACH ROW EXECUTE FUNCTION ledger_row_is_immutable();

    -- ddl 35: budget_daily_charges
CREATE TABLE budget_daily_charges (
    attempt_id UUID CONSTRAINT budget_daily_charges_pkey PRIMARY KEY,venue_id TEXT NOT NULL,tenant_id UUID NOT NULL,intent_id UUID NOT NULL,
    account_scope TEXT NOT NULL,client_order_id TEXT NOT NULL,kind TEXT NOT NULL CONSTRAINT budget_daily_charges_kind_check CHECK(kind IN ('entry','exit','protection','cancel','sweep_close')),
    source_created_at TIMESTAMPTZ NOT NULL,budget_clock_domain budget_clock_domain_v38,budget_created_tick NUMERIC,budget_recorded_at TIMESTAMPTZ,
    daily_units SMALLINT NOT NULL CONSTRAINT budget_daily_charges_daily_units_check CHECK(daily_units=CASE WHEN kind='cancel' THEN 0 ELSE 1 END),
    expiry_proof_id UUID,retired_at TIMESTAMPTZ,
    CONSTRAINT budget_charge_proof_fk FOREIGN KEY(expiry_proof_id,venue_id) REFERENCES budget_expiry_proofs(proof_id,venue_id),
    CONSTRAINT budget_charge_timing CHECK((num_nulls(budget_clock_domain,budget_created_tick)=2 OR
        (num_nulls(budget_clock_domain,budget_created_tick)=0 AND ((num_nulls((budget_clock_domain).kind,(budget_clock_domain).boot_uuid,(budget_clock_domain).offset_seconds,(budget_clock_domain).offset_nanoseconds)=0
        AND (budget_clock_domain).kind='boottime' 
        AND (budget_clock_domain).boot_uuid<>'00000000-0000-0000-0000-000000000000'::uuid
        AND (budget_clock_domain).offset_nanoseconds BETWEEN 0 AND 999999999) IS TRUE) AND budget_integer_fits(budget_created_tick,0,9223372036854775807999999999))) IS TRUE),
    CONSTRAINT budget_charge_retirement_whole CHECK((expiry_proof_id IS NULL)=(retired_at IS NULL))
);

    -- ddl 36: budget_daily_charges_active
CREATE INDEX budget_daily_charges_active ON budget_daily_charges(venue_id,attempt_id) WHERE daily_units=1 AND expiry_proof_id IS NULL;

    -- ddl 37: budget_charge_clock_fk
ALTER TABLE budget_daily_charges ADD CONSTRAINT budget_charge_clock_fk FOREIGN KEY(attempt_id) REFERENCES spot_intent_clock(attempt_id) NOT VALID;

    -- ddl 38: budget_clock_charge_fk
ALTER TABLE spot_intent_clock ADD CONSTRAINT budget_clock_charge_fk FOREIGN KEY(attempt_id) REFERENCES budget_daily_charges(attempt_id) DEFERRABLE INITIALLY DEFERRED NOT VALID;

    -- ddl 39: budget_charge_moves_forward
CREATE FUNCTION budget_charge_moves_forward() RETURNS TRIGGER LANGUAGE plpgsql AS $fn$
BEGIN
    IF TG_OP='DELETE' OR
       (NEW.attempt_id,NEW.venue_id,NEW.tenant_id,NEW.intent_id,NEW.account_scope,NEW.client_order_id,NEW.kind,NEW.source_created_at,NEW.budget_clock_domain,NEW.budget_created_tick,NEW.budget_recorded_at,NEW.daily_units)
           IS DISTINCT FROM (OLD.attempt_id,OLD.venue_id,OLD.tenant_id,OLD.intent_id,OLD.account_scope,OLD.client_order_id,OLD.kind,OLD.source_created_at,OLD.budget_clock_domain,OLD.budget_created_tick,OLD.budget_recorded_at,OLD.daily_units)
       OR (OLD.expiry_proof_id IS NOT NULL AND (NEW.expiry_proof_id,NEW.retired_at) IS DISTINCT FROM (OLD.expiry_proof_id,OLD.retired_at)) THEN
        RAISE EXCEPTION 'daily charge identity and retirement are immutable' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$fn$;

    -- ddl 40: budget_charge_moves_forward
CREATE TRIGGER budget_charge_moves_forward BEFORE UPDATE OR DELETE ON budget_daily_charges
    FOR EACH ROW EXECUTE FUNCTION budget_charge_moves_forward();

    -- ddl 41: budget_project_clocks
CREATE FUNCTION budget_project_clocks() RETURNS TRIGGER LANGUAGE plpgsql VOLATILE AS $fn$
DECLARE
    v_venue TEXT;v_initialization TEXT;v_count BIGINT;v_venues BIGINT;v_bad BIGINT;v_union BIGINT;
BEGIN
    IF current_setting('transaction_isolation')<>'read committed' THEN
        RAISE EXCEPTION 'clock projection requires READ COMMITTED' USING ERRCODE='23514';
    END IF;
    -- SPI1: the common venue row serializes competing tenant writers.
    WITH scope AS (SELECT min(venue_id) AS venue,count(*) AS n,count(DISTINCT venue_id) AS venues FROM new_clocks)
    INSERT INTO budget_venue_state(venue_id) SELECT venue FROM scope WHERE n>0 AND n<=1000 AND venues=1
    ON CONFLICT(venue_id) DO UPDATE SET venue_id=EXCLUDED.venue_id
    RETURNING venue_id,initialization_state INTO v_venue,v_initialization;
    -- SPI2: VOLATILE gets a fresh post-lock snapshot; no per-row SQL.
    WITH incoming AS MATERIALIZED (
        SELECT c.*,i.account_scope,i.client_order_id,i.kind,a.attempt_id AS source_attempt,prior.attempt_id AS prior_charge,
            (a.attempt_id IS NULL OR i.intent_id IS NULL OR i.tenant_id IS DISTINCT FROM c.tenant_id
             OR i.venue_id IS DISTINCT FROM c.venue_id OR a.venue_id IS DISTINCT FROM c.venue_id
             OR a.account_scope IS DISTINCT FROM i.account_scope OR a.client_order_id IS DISTINCT FROM i.client_order_id
             OR a.created_at IS DISTINCT FROM c.attempt_created_at) AS bad
        FROM new_clocks c LEFT JOIN submit_attempts a ON a.attempt_id=c.attempt_id
        LEFT JOIN order_intents i ON i.intent_id=c.intent_id LEFT JOIN budget_daily_charges prior ON prior.attempt_id=c.attempt_id
    ), metrics AS MATERIALIZED (
        SELECT count(*) AS n,count(DISTINCT venue_id) AS venues,count(*) FILTER(WHERE bad) AS bad,
            (SELECT count(*) FROM (SELECT d.attempt_id FROM budget_daily_charges d
                WHERE d.venue_id=v_venue AND d.daily_units=1 AND d.expiry_proof_id IS NULL LIMIT 10001) active)
            +count(*) FILTER(WHERE kind<>'cancel' AND prior_charge IS NULL) AS total
        FROM incoming
    ), written AS (
        INSERT INTO budget_daily_charges(attempt_id,venue_id,tenant_id,intent_id,account_scope,client_order_id,kind,
            source_created_at,budget_clock_domain,budget_created_tick,budget_recorded_at,daily_units)
        SELECT c.attempt_id,c.venue_id,c.tenant_id,c.intent_id,c.account_scope,c.client_order_id,c.kind,
            c.attempt_created_at,c.budget_clock_domain,c.budget_created_tick,c.budget_recorded_at,
            CASE WHEN c.kind='cancel' THEN 0 ELSE 1 END
        FROM incoming c CROSS JOIN metrics m
        WHERE m.bad=0 AND m.n<=1000 AND m.venues<=1 AND v_initialization='complete' AND m.total<=10000
        ON CONFLICT(attempt_id) DO NOTHING RETURNING attempt_id
    )
    SELECT m.n,m.venues,m.bad,m.total FROM metrics m CROSS JOIN (SELECT count(*) FROM written) drained
    INTO v_count,v_venues,v_bad,v_union;
    IF v_count>0 AND (v_venues<>1 OR v_bad<>0 OR v_count>1000 OR v_union>10000 OR v_initialization IS DISTINCT FROM 'complete') THEN
        RAISE EXCEPTION 'clock source binding or global daily budget is incomplete' USING ERRCODE='23514';
    END IF;
    RETURN NULL;
END;
$fn$;

    -- ddl 42: budget_project_clocks
CREATE TRIGGER budget_project_clocks AFTER INSERT ON spot_intent_clock
    REFERENCING NEW TABLE AS new_clocks FOR EACH STATEMENT EXECUTE FUNCTION budget_project_clocks();

    -- ddl 43: budget_insert_clocks
CREATE FUNCTION budget_insert_clocks(
    p_ids UUID[],
    p_intent UUID,
    p_tenant UUID,
    p_venue TEXT,
    p_created TIMESTAMPTZ[],
    p_samples UUID[],
    p_wall BIGINT[],
    p_mono BIGINT[],
    p_verdict TEXT[],
    p_budget_kind TEXT[],
    p_budget_boot UUID[],
    p_budget_seconds BIGINT[],
    p_budget_nanos INTEGER[],
    p_budget_tick NUMERIC[],
    p_capture_epoch UUID[],
    p_capture_seconds BIGINT[],
    p_capture_nanos INTEGER[],
    p_capture_before NUMERIC[],
    p_capture_after NUMERIC[],
    p_capture_kind TEXT[],
    p_capture_boot UUID[],
    p_capture_offset_seconds BIGINT[],
    p_capture_offset_nanos INTEGER[])
RETURNS TABLE(
    attempt_id UUID,
    intent_id UUID,
    tenant_id UUID,
    venue_id TEXT,
    attempt_created_at TIMESTAMPTZ,
    clock_sample_id UUID,
    clock_wall_delta_ms BIGINT,
    clock_mono_delta_ms BIGINT,
    clock_verdict TEXT,
    budget_kind TEXT,
    budget_boot UUID,
    budget_offset_seconds BIGINT,
    budget_offset_nanoseconds INTEGER,
    budget_created_tick NUMERIC,
    budget_recorded_at TIMESTAMPTZ,
    capture_epoch UUID,
    capture_wall_seconds BIGINT,
    capture_wall_nanoseconds INTEGER,
    capture_tick_before NUMERIC,
    capture_tick_after NUMERIC,
    capture_kind TEXT,
    capture_boot UUID,
    capture_offset_seconds BIGINT,
    capture_offset_nanoseconds INTEGER,
    account_scope TEXT,
    client_order_id TEXT,
    kind TEXT,
    daily_units SMALLINT,
    expiry_proof_id UUID,
    retired_at TIMESTAMPTZ,
    budget_created_tick_scale INTEGER,
    budget_created_tick_fits BOOLEAN,
    capture_tick_before_scale INTEGER,
    capture_tick_before_fits BOOLEAN,
    capture_tick_after_scale INTEGER,
    capture_tick_after_fits BOOLEAN) LANGUAGE plpgsql VOLATILE AS $fn$
DECLARE
    v_n INTEGER:=cardinality(p_ids);v_i INTEGER;v_seen UUID[]:='{}';v_returned BIGINT;
BEGIN
    IF v_n IS NULL OR v_n>1000 OR cardinality(p_ids) IS DISTINCT FROM v_n OR (v_n>0 AND (array_lower(p_ids,1) IS DISTINCT FROM 1 OR array_ndims(p_ids) IS DISTINCT FROM 1)) OR
       cardinality(p_created) IS DISTINCT FROM v_n OR (v_n>0 AND (array_lower(p_created,1) IS DISTINCT FROM 1 OR array_ndims(p_created) IS DISTINCT FROM 1)) OR
       cardinality(p_samples) IS DISTINCT FROM v_n OR (v_n>0 AND (array_lower(p_samples,1) IS DISTINCT FROM 1 OR array_ndims(p_samples) IS DISTINCT FROM 1)) OR
       cardinality(p_wall) IS DISTINCT FROM v_n OR (v_n>0 AND (array_lower(p_wall,1) IS DISTINCT FROM 1 OR array_ndims(p_wall) IS DISTINCT FROM 1)) OR
       cardinality(p_mono) IS DISTINCT FROM v_n OR (v_n>0 AND (array_lower(p_mono,1) IS DISTINCT FROM 1 OR array_ndims(p_mono) IS DISTINCT FROM 1)) OR
       cardinality(p_verdict) IS DISTINCT FROM v_n OR (v_n>0 AND (array_lower(p_verdict,1) IS DISTINCT FROM 1 OR array_ndims(p_verdict) IS DISTINCT FROM 1)) OR
       cardinality(p_budget_kind) IS DISTINCT FROM v_n OR (v_n>0 AND (array_lower(p_budget_kind,1) IS DISTINCT FROM 1 OR array_ndims(p_budget_kind) IS DISTINCT FROM 1)) OR
       cardinality(p_budget_boot) IS DISTINCT FROM v_n OR (v_n>0 AND (array_lower(p_budget_boot,1) IS DISTINCT FROM 1 OR array_ndims(p_budget_boot) IS DISTINCT FROM 1)) OR
       cardinality(p_budget_seconds) IS DISTINCT FROM v_n OR (v_n>0 AND (array_lower(p_budget_seconds,1) IS DISTINCT FROM 1 OR array_ndims(p_budget_seconds) IS DISTINCT FROM 1)) OR
       cardinality(p_budget_nanos) IS DISTINCT FROM v_n OR (v_n>0 AND (array_lower(p_budget_nanos,1) IS DISTINCT FROM 1 OR array_ndims(p_budget_nanos) IS DISTINCT FROM 1)) OR
       cardinality(p_budget_tick) IS DISTINCT FROM v_n OR (v_n>0 AND (array_lower(p_budget_tick,1) IS DISTINCT FROM 1 OR array_ndims(p_budget_tick) IS DISTINCT FROM 1)) OR
       cardinality(p_capture_epoch) IS DISTINCT FROM v_n OR (v_n>0 AND (array_lower(p_capture_epoch,1) IS DISTINCT FROM 1 OR array_ndims(p_capture_epoch) IS DISTINCT FROM 1)) OR
       cardinality(p_capture_seconds) IS DISTINCT FROM v_n OR (v_n>0 AND (array_lower(p_capture_seconds,1) IS DISTINCT FROM 1 OR array_ndims(p_capture_seconds) IS DISTINCT FROM 1)) OR
       cardinality(p_capture_nanos) IS DISTINCT FROM v_n OR (v_n>0 AND (array_lower(p_capture_nanos,1) IS DISTINCT FROM 1 OR array_ndims(p_capture_nanos) IS DISTINCT FROM 1)) OR
       cardinality(p_capture_before) IS DISTINCT FROM v_n OR (v_n>0 AND (array_lower(p_capture_before,1) IS DISTINCT FROM 1 OR array_ndims(p_capture_before) IS DISTINCT FROM 1)) OR
       cardinality(p_capture_after) IS DISTINCT FROM v_n OR (v_n>0 AND (array_lower(p_capture_after,1) IS DISTINCT FROM 1 OR array_ndims(p_capture_after) IS DISTINCT FROM 1)) OR
       cardinality(p_capture_kind) IS DISTINCT FROM v_n OR (v_n>0 AND (array_lower(p_capture_kind,1) IS DISTINCT FROM 1 OR array_ndims(p_capture_kind) IS DISTINCT FROM 1)) OR
       cardinality(p_capture_boot) IS DISTINCT FROM v_n OR (v_n>0 AND (array_lower(p_capture_boot,1) IS DISTINCT FROM 1 OR array_ndims(p_capture_boot) IS DISTINCT FROM 1)) OR
       cardinality(p_capture_offset_seconds) IS DISTINCT FROM v_n OR (v_n>0 AND (array_lower(p_capture_offset_seconds,1) IS DISTINCT FROM 1 OR array_ndims(p_capture_offset_seconds) IS DISTINCT FROM 1)) OR
       cardinality(p_capture_offset_nanos) IS DISTINCT FROM v_n OR (v_n>0 AND (array_lower(p_capture_offset_nanos,1) IS DISTINCT FROM 1 OR array_ndims(p_capture_offset_nanos) IS DISTINCT FROM 1)) THEN
        RAISE EXCEPTION 'clock input arrays have incompatible bounded shapes' USING ERRCODE='23514';
    END IF;
    IF p_intent IS NULL OR p_intent='00000000-0000-0000-0000-000000000000'::uuid OR p_tenant IS NULL OR p_tenant='00000000-0000-0000-0000-000000000000'::uuid OR p_venue IS NULL OR p_venue='' THEN
        RAISE EXCEPTION 'clock input identity is missing' USING ERRCODE='23514';
    END IF;
    FOR v_i IN 1..v_n LOOP
        IF p_ids[v_i] IS NULL OR p_ids[v_i]='00000000-0000-0000-0000-000000000000'::uuid OR p_ids[v_i]=ANY(v_seen) THEN
            RAISE EXCEPTION 'clock input IDs must be nonnil and unique' USING ERRCODE='23514';
        END IF;
        v_seen:=array_append(v_seen,p_ids[v_i]);
        IF num_nulls(p_budget_kind[v_i],p_budget_boot[v_i],p_budget_seconds[v_i],p_budget_nanos[v_i],p_budget_tick[v_i]) NOT IN (0,5)
           OR (p_budget_kind[v_i] IS NOT NULL AND (p_budget_kind[v_i]<>'boottime' OR p_budget_boot[v_i]='00000000-0000-0000-0000-000000000000'::uuid
               OR p_budget_nanos[v_i] NOT BETWEEN 0 AND 999999999 OR NOT budget_integer_fits(p_budget_tick[v_i],0,9223372036854775807999999999))) THEN
            RAISE EXCEPTION 'clock budget coordinates are malformed' USING ERRCODE='23514';
        END IF;
        IF num_nulls(p_capture_epoch[v_i],p_capture_seconds[v_i],p_capture_nanos[v_i],p_capture_before[v_i],p_capture_after[v_i],p_capture_kind[v_i],p_capture_boot[v_i],p_capture_offset_seconds[v_i],p_capture_offset_nanos[v_i]) NOT IN (0,9)
           OR (p_capture_epoch[v_i] IS NOT NULL AND (p_capture_epoch[v_i]='00000000-0000-0000-0000-000000000000'::uuid OR p_capture_kind[v_i]<>'boottime'
               OR p_capture_boot[v_i]='00000000-0000-0000-0000-000000000000'::uuid OR p_capture_nanos[v_i] NOT BETWEEN 0 AND 999999999
               OR p_capture_offset_nanos[v_i] NOT BETWEEN 0 AND 999999999 OR NOT budget_integer_fits(p_capture_before[v_i],0,9223372036854775807999999999)
               OR NOT budget_integer_fits(p_capture_after[v_i],0,9223372036854775807999999999) OR p_capture_before[v_i]>p_capture_after[v_i])) THEN
            RAISE EXCEPTION 'clock capture coordinates are malformed' USING ERRCODE='23514';
        END IF;
    END LOOP;
    -- SPI1: one bulk insertion; no enrichment of existing immutable clocks.
    INSERT INTO spot_intent_clock(attempt_id,intent_id,tenant_id,venue_id,attempt_created_at,
        clock_sample_id,clock_wall_delta_ms,clock_mono_delta_ms,clock_verdict,budget_clock_domain,budget_created_tick,
        capture_epoch,capture_wall_seconds,capture_wall_nanoseconds,capture_tick_before,capture_tick_after,capture_domain)
    SELECT p_ids[g.i],p_intent,p_tenant,p_venue,p_created[g.i],p_samples[g.i],p_wall[g.i],p_mono[g.i],p_verdict[g.i],
        CASE WHEN p_budget_kind[g.i] IS NULL THEN NULL::budget_clock_domain_v38
             ELSE ROW(p_budget_kind[g.i],p_budget_boot[g.i],p_budget_seconds[g.i],p_budget_nanos[g.i])::budget_clock_domain_v38 END,p_budget_tick[g.i],
        p_capture_epoch[g.i],p_capture_seconds[g.i],p_capture_nanos[g.i],p_capture_before[g.i],p_capture_after[g.i],
        CASE WHEN p_capture_kind[g.i] IS NULL THEN NULL::budget_clock_domain_v38
             ELSE ROW(p_capture_kind[g.i],p_capture_boot[g.i],p_capture_offset_seconds[g.i],p_capture_offset_nanos[g.i])::budget_clock_domain_v38 END
    FROM generate_subscripts(p_ids,1) g(i) ON CONFLICT ON CONSTRAINT spot_intent_clock_pkey DO NOTHING;
    -- SPI2: new statement snapshot checks ALL input, including all-conflict replay.
    RETURN QUERY SELECT
        c.attempt_id,
        c.intent_id,
        c.tenant_id,
        c.venue_id,
        c.attempt_created_at,
        c.clock_sample_id,
        c.clock_wall_delta_ms,
        c.clock_mono_delta_ms,
        c.clock_verdict,
        (c.budget_clock_domain).kind,
        (c.budget_clock_domain).boot_uuid,
        (c.budget_clock_domain).offset_seconds,
        (c.budget_clock_domain).offset_nanoseconds,
        c.budget_created_tick,
        c.budget_recorded_at,
        c.capture_epoch,
        c.capture_wall_seconds,
        c.capture_wall_nanoseconds,
        c.capture_tick_before,
        c.capture_tick_after,
        (c.capture_domain).kind,
        (c.capture_domain).boot_uuid,
        (c.capture_domain).offset_seconds,
        (c.capture_domain).offset_nanoseconds,
        d.account_scope,
        d.client_order_id,
        d.kind,
        d.daily_units,
        d.expiry_proof_id,
        d.retired_at,
        scale(c.budget_created_tick),
        budget_integer_fits(c.budget_created_tick,0,9223372036854775807999999999),
        scale(c.capture_tick_before),
        budget_integer_fits(c.capture_tick_before,0,9223372036854775807999999999),
        scale(c.capture_tick_after),
        budget_integer_fits(c.capture_tick_after,0,9223372036854775807999999999)
    FROM generate_subscripts(p_ids,1) g(i) JOIN spot_intent_clock c ON c.attempt_id=p_ids[g.i]
    JOIN budget_daily_charges d ON d.attempt_id=c.attempt_id JOIN order_intents s ON s.intent_id=c.intent_id
    WHERE c.intent_id IS NOT DISTINCT FROM p_intent
        AND c.tenant_id IS NOT DISTINCT FROM p_tenant
        AND c.venue_id IS NOT DISTINCT FROM p_venue
        AND c.attempt_created_at IS NOT DISTINCT FROM p_created[g.i]
        AND c.clock_sample_id IS NOT DISTINCT FROM p_samples[g.i]
        AND c.clock_wall_delta_ms IS NOT DISTINCT FROM p_wall[g.i]
        AND c.clock_mono_delta_ms IS NOT DISTINCT FROM p_mono[g.i]
        AND c.clock_verdict IS NOT DISTINCT FROM p_verdict[g.i]
        AND (c.budget_clock_domain).kind IS NOT DISTINCT FROM p_budget_kind[g.i]
        AND (c.budget_clock_domain).boot_uuid IS NOT DISTINCT FROM p_budget_boot[g.i]
        AND (c.budget_clock_domain).offset_seconds IS NOT DISTINCT FROM p_budget_seconds[g.i]
        AND (c.budget_clock_domain).offset_nanoseconds IS NOT DISTINCT FROM p_budget_nanos[g.i]
        AND c.budget_created_tick IS NOT DISTINCT FROM p_budget_tick[g.i]
        AND c.capture_epoch IS NOT DISTINCT FROM p_capture_epoch[g.i]
        AND c.capture_wall_seconds IS NOT DISTINCT FROM p_capture_seconds[g.i]
        AND c.capture_wall_nanoseconds IS NOT DISTINCT FROM p_capture_nanos[g.i]
        AND c.capture_tick_before IS NOT DISTINCT FROM p_capture_before[g.i]
        AND c.capture_tick_after IS NOT DISTINCT FROM p_capture_after[g.i]
        AND (c.capture_domain).kind IS NOT DISTINCT FROM p_capture_kind[g.i]
        AND (c.capture_domain).boot_uuid IS NOT DISTINCT FROM p_capture_boot[g.i]
        AND (c.capture_domain).offset_seconds IS NOT DISTINCT FROM p_capture_offset_seconds[g.i]
        AND (c.capture_domain).offset_nanoseconds IS NOT DISTINCT FROM p_capture_offset_nanos[g.i]
        AND d.intent_id IS NOT DISTINCT FROM c.intent_id
        AND d.tenant_id IS NOT DISTINCT FROM c.tenant_id
        AND d.venue_id IS NOT DISTINCT FROM c.venue_id
        AND d.source_created_at IS NOT DISTINCT FROM c.attempt_created_at
        AND d.budget_clock_domain IS NOT DISTINCT FROM c.budget_clock_domain
        AND d.budget_created_tick IS NOT DISTINCT FROM c.budget_created_tick
        AND d.budget_recorded_at IS NOT DISTINCT FROM c.budget_recorded_at
        AND d.account_scope IS NOT DISTINCT FROM s.account_scope
        AND d.client_order_id IS NOT DISTINCT FROM s.client_order_id
        AND d.kind IS NOT DISTINCT FROM s.kind;
    GET DIAGNOSTICS v_returned=ROW_COUNT;
    IF v_returned<>v_n THEN
        RAISE EXCEPTION 'clock completion receipt has a missing or conflicting row' USING ERRCODE='23514';
    END IF;
END;
$fn$;

    -- ddl 44: n5_registry_root
CREATE TABLE n5_registry_root (
    registry_id SMALLINT CONSTRAINT n5_registry_root_pkey PRIMARY KEY CONSTRAINT n5_registry_root_registry_id_check CHECK(registry_id=1),schema_version SMALLINT NOT NULL CONSTRAINT n5_registry_root_schema_version_check CHECK(schema_version=38),
    manifest_version SMALLINT NOT NULL CONSTRAINT n5_registry_root_manifest_version_check CHECK(manifest_version=1),manifest_revision BIGINT NOT NULL CONSTRAINT n5_registry_root_manifest_revision_check CHECK(manifest_revision>=0),
    initialized BOOLEAN NOT NULL,registered_count INTEGER NOT NULL CONSTRAINT n5_registry_root_registered_count_check CHECK(registered_count BETWEEN 0 AND 4096),
    canonical_digest BYTEA NOT NULL CONSTRAINT n5_registry_root_canonical_digest_check CHECK(octet_length(canonical_digest)=32),initialization_id UUID NOT NULL CONSTRAINT n5_registry_root_initialization_id_check CHECK(initialization_id<>'00000000-0000-0000-0000-000000000000'::uuid),
    recovery_generation BIGINT NOT NULL CONSTRAINT n5_registry_root_recovery_generation_check CHECK(recovery_generation>=0),recovery_state TEXT NOT NULL CONSTRAINT n5_registry_root_recovery_state_check CHECK(recovery_state IN ('quarantined','normal')),
    recovery_audit_id UUID,registered_at TIMESTAMPTZ NOT NULL,
    CONSTRAINT n5_registry_recovery_whole CHECK((recovery_generation=0 OR (recovery_audit_id IS NOT NULL AND recovery_audit_id<>'00000000-0000-0000-0000-000000000000'::uuid)) IS TRUE),
    CONSTRAINT n5_registry_no_success_default CHECK(initialized OR recovery_state='quarantined')
);

    -- ddl 45: n5_registry_moves_forward
CREATE FUNCTION n5_registry_moves_forward() RETURNS TRIGGER LANGUAGE plpgsql AS $fn$
BEGIN
    IF TG_OP='DELETE' OR
       (NEW.registry_id,NEW.schema_version,NEW.manifest_version,NEW.initialization_id,NEW.registered_at)
           IS DISTINCT FROM (OLD.registry_id,OLD.schema_version,OLD.manifest_version,OLD.initialization_id,OLD.registered_at)
       OR (OLD.initialized AND NOT NEW.initialized) OR NEW.registered_count<OLD.registered_count
       OR NEW.recovery_generation<OLD.recovery_generation OR NEW.recovery_generation::numeric>OLD.recovery_generation::numeric+1
       OR NEW.manifest_revision<OLD.manifest_revision OR NEW.manifest_revision::numeric>OLD.manifest_revision::numeric+1
       OR (NEW.manifest_revision=OLD.manifest_revision AND (NEW.registered_count,NEW.canonical_digest) IS DISTINCT FROM (OLD.registered_count,OLD.canonical_digest))
       OR (NEW.manifest_revision>OLD.manifest_revision AND (NEW.registered_count<=OLD.registered_count OR NEW.registered_count-OLD.registered_count>64))
       OR (NEW.recovery_generation=OLD.recovery_generation AND NEW.recovery_audit_id IS DISTINCT FROM OLD.recovery_audit_id)
       OR (NEW.recovery_generation>OLD.recovery_generation AND (NEW.recovery_state<>'quarantined'
           OR (NEW.manifest_revision,NEW.registered_count,NEW.canonical_digest) IS DISTINCT FROM (OLD.manifest_revision,OLD.registered_count,OLD.canonical_digest))) THEN
        RAISE EXCEPTION 'N5 registry cannot reset history or change unaudited provenance' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$fn$;

    -- ddl 46: n5_registry_moves_forward
CREATE TRIGGER n5_registry_moves_forward BEFORE UPDATE OR DELETE ON n5_registry_root
    FOR EACH ROW EXECUTE FUNCTION n5_registry_moves_forward();

    -- ddl 47: n5_refresh_keys
CREATE TABLE n5_refresh_keys (
    venue_id TEXT NOT NULL CONSTRAINT n5_refresh_keys_venue_id_check CHECK(octet_length(venue_id) BETWEEN 1 AND 64),
    symbol TEXT NOT NULL CONSTRAINT n5_refresh_keys_symbol_check CHECK(octet_length(symbol) BETWEEN 1 AND 64),descriptor TEXT NOT NULL CONSTRAINT n5_refresh_keys_descriptor_check CHECK(descriptor='commission'),
    registry_id SMALLINT NOT NULL,registration_id UUID NOT NULL CONSTRAINT n5_refresh_keys_registration_id_check CHECK(registration_id<>'00000000-0000-0000-0000-000000000000'::uuid),
    registered_revision BIGINT NOT NULL CONSTRAINT n5_refresh_keys_registered_revision_check CHECK(registered_revision>0),registered_at TIMESTAMPTZ NOT NULL,
    provenance TEXT NOT NULL CONSTRAINT n5_refresh_keys_provenance_check CHECK(octet_length(provenance) BETWEEN 1 AND 64),
    CONSTRAINT n5_refresh_keys_pkey PRIMARY KEY(venue_id,symbol,descriptor),CONSTRAINT n5_key_registry_fk FOREIGN KEY(registry_id) REFERENCES n5_registry_root(registry_id)
);

    -- ddl 48: n5_key_is_append_only
CREATE FUNCTION n5_key_is_append_only() RETURNS TRIGGER LANGUAGE plpgsql AS $fn$
BEGIN
    IF TRUE THEN
        RAISE EXCEPTION 'N5 registered frequency keys are append-only' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$fn$;

    -- ddl 49: n5_key_is_append_only
CREATE TRIGGER n5_key_is_append_only BEFORE UPDATE OR DELETE ON n5_refresh_keys
    FOR EACH ROW EXECUTE FUNCTION n5_key_is_append_only();

    -- ddl 50: n5_refresh_state
CREATE TABLE n5_refresh_state (
    venue_id TEXT NOT NULL,symbol TEXT NOT NULL,descriptor TEXT NOT NULL,revision BIGINT NOT NULL CONSTRAINT n5_refresh_state_revision_check CHECK(revision>=0),
    claim_id UUID,owner_process UUID,content_digest BYTEA,claim_domain budget_clock_domain_v38,claim_tick NUMERIC,claim_audit_at TIMESTAMPTZ,
    eligibility_kind TEXT CONSTRAINT n5_refresh_state_eligibility_kind_check CHECK(eligibility_kind IN ('first_use','same_domain','continuous_observation')),
    handoff_domain budget_clock_domain_v38,handoff_tick NUMERIC,
    result_class TEXT CONSTRAINT n5_refresh_state_result_class_check CHECK(result_class IN ('success','no_send','transport_failure','venue_rejected','invalid_payload','policy_mismatch','unknown')),
    snapshot_claim_id UUID,snapshot_revision BIGINT,snapshot_owner UUID,snapshot_tenant UUID,snapshot_scope TEXT,snapshot_uid NUMERIC,
    snapshot_domain budget_clock_domain_v38,snapshot_tick NUMERIC,snapshot_as_of TIMESTAMPTZ,snapshot_validation_version SMALLINT,snapshot_hash BYTEA,snapshot_payload BYTEA,
    CONSTRAINT n5_refresh_state_pkey PRIMARY KEY(venue_id,symbol,descriptor),CONSTRAINT n5_state_key_fk FOREIGN KEY(venue_id,symbol,descriptor) REFERENCES n5_refresh_keys(venue_id,symbol,descriptor),
    CONSTRAINT n5_claim_whole CHECK(((revision=0 AND num_nulls(claim_id,owner_process,content_digest,claim_domain,claim_tick,claim_audit_at,eligibility_kind)=7 AND result_class IS NULL
            AND num_nulls(handoff_domain,handoff_tick)=2 AND num_nulls(snapshot_claim_id,snapshot_revision,snapshot_owner,snapshot_tenant,snapshot_scope,snapshot_uid,snapshot_domain,snapshot_tick,snapshot_as_of,snapshot_validation_version,snapshot_hash,snapshot_payload)=12)
        OR (revision>0 AND num_nulls(claim_id,owner_process,content_digest,claim_domain,claim_tick,claim_audit_at,eligibility_kind)=0 AND claim_id<>'00000000-0000-0000-0000-000000000000'::uuid AND owner_process<>'00000000-0000-0000-0000-000000000000'::uuid
            AND octet_length(content_digest)=32 AND ((num_nulls((claim_domain).kind,(claim_domain).boot_uuid,(claim_domain).offset_seconds,(claim_domain).offset_nanoseconds)=0
        AND (claim_domain).kind='boottime' 
        AND (claim_domain).boot_uuid<>'00000000-0000-0000-0000-000000000000'::uuid
        AND (claim_domain).offset_nanoseconds BETWEEN 0 AND 999999999) IS TRUE) AND budget_integer_fits(claim_tick,0,9223372036854775807999999999))) IS TRUE),
    CONSTRAINT n5_handoff_whole CHECK((num_nulls(handoff_domain,handoff_tick)=2 OR
        (num_nulls(handoff_domain,handoff_tick)=0 AND ((num_nulls((handoff_domain).kind,(handoff_domain).boot_uuid,(handoff_domain).offset_seconds,(handoff_domain).offset_nanoseconds)=0
        AND (handoff_domain).kind='boottime' 
        AND (handoff_domain).boot_uuid<>'00000000-0000-0000-0000-000000000000'::uuid
        AND (handoff_domain).offset_nanoseconds BETWEEN 0 AND 999999999) IS TRUE) AND budget_integer_fits(handoff_tick,0,9223372036854775807999999999)
         AND handoff_domain IS NOT DISTINCT FROM claim_domain AND handoff_tick>=claim_tick AND handoff_tick-claim_tick<=60000000000)) IS TRUE),
    CONSTRAINT n5_snapshot_whole CHECK((num_nulls(snapshot_claim_id,snapshot_revision,snapshot_owner,snapshot_tenant,snapshot_scope,snapshot_uid,snapshot_domain,snapshot_tick,snapshot_as_of,snapshot_validation_version,snapshot_hash,snapshot_payload)=12 OR
        (num_nulls(snapshot_claim_id,snapshot_revision,snapshot_owner,snapshot_tenant,snapshot_scope,snapshot_uid,snapshot_domain,snapshot_tick,snapshot_as_of,snapshot_validation_version,snapshot_hash,snapshot_payload)=0 AND snapshot_claim_id<>'00000000-0000-0000-0000-000000000000'::uuid AND snapshot_owner<>'00000000-0000-0000-0000-000000000000'::uuid AND snapshot_tenant<>'00000000-0000-0000-0000-000000000000'::uuid
         AND snapshot_revision>0 AND snapshot_revision<=revision AND octet_length(snapshot_scope) BETWEEN 1 AND 128
         AND budget_integer_fits(snapshot_uid,0,18446744073709551615) AND ((num_nulls((snapshot_domain).kind,(snapshot_domain).boot_uuid,(snapshot_domain).offset_seconds,(snapshot_domain).offset_nanoseconds)=0
        AND (snapshot_domain).kind='boottime' 
        AND (snapshot_domain).boot_uuid<>'00000000-0000-0000-0000-000000000000'::uuid
        AND (snapshot_domain).offset_nanoseconds BETWEEN 0 AND 999999999) IS TRUE) AND budget_integer_fits(snapshot_tick,0,9223372036854775807999999999)
         AND snapshot_validation_version=1 AND octet_length(snapshot_hash)=32 AND octet_length(snapshot_payload) BETWEEN 1 AND 16384)) IS TRUE)
);

    -- ddl 51: n5_state_moves_forward
CREATE FUNCTION n5_state_moves_forward() RETURNS TRIGGER LANGUAGE plpgsql AS $fn$
BEGIN
    IF TG_OP='DELETE' OR
       (NEW.venue_id,NEW.symbol,NEW.descriptor) IS DISTINCT FROM (OLD.venue_id,OLD.symbol,OLD.descriptor)
       OR NEW.revision<OLD.revision OR NEW.revision::numeric>OLD.revision::numeric+1
       OR (NEW.revision=OLD.revision AND ((NEW.claim_id,NEW.owner_process,NEW.content_digest,NEW.claim_domain,
        NEW.claim_tick,NEW.claim_audit_at,NEW.eligibility_kind) IS DISTINCT FROM (OLD.claim_id,OLD.owner_process,OLD.content_digest,OLD.claim_domain,
        OLD.claim_tick,OLD.claim_audit_at,OLD.eligibility_kind)))
       OR (NEW.revision>OLD.revision AND (NEW.claim_id IS NOT DISTINCT FROM OLD.claim_id OR NEW.result_class IS NOT NULL
           OR num_nulls(NEW.handoff_domain,NEW.handoff_tick)<>2 OR ((NEW.snapshot_claim_id,NEW.snapshot_revision,NEW.snapshot_owner,NEW.snapshot_tenant,
        NEW.snapshot_scope,NEW.snapshot_uid,NEW.snapshot_domain,NEW.snapshot_tick,
        NEW.snapshot_as_of,NEW.snapshot_validation_version,NEW.snapshot_hash,NEW.snapshot_payload) IS DISTINCT FROM (OLD.snapshot_claim_id,OLD.snapshot_revision,OLD.snapshot_owner,OLD.snapshot_tenant,
        OLD.snapshot_scope,OLD.snapshot_uid,OLD.snapshot_domain,OLD.snapshot_tick,
        OLD.snapshot_as_of,OLD.snapshot_validation_version,OLD.snapshot_hash,OLD.snapshot_payload))))
       OR (NEW.revision=OLD.revision AND OLD.result_class IS NOT NULL AND ((NEW.result_class,NEW.handoff_domain,NEW.handoff_tick,NEW.snapshot_claim_id,
        NEW.snapshot_revision,NEW.snapshot_owner,NEW.snapshot_tenant,NEW.snapshot_scope,
        NEW.snapshot_uid,NEW.snapshot_domain,NEW.snapshot_tick,NEW.snapshot_as_of,
        NEW.snapshot_validation_version,NEW.snapshot_hash,NEW.snapshot_payload) IS DISTINCT FROM (OLD.result_class,OLD.handoff_domain,OLD.handoff_tick,OLD.snapshot_claim_id,
        OLD.snapshot_revision,OLD.snapshot_owner,OLD.snapshot_tenant,OLD.snapshot_scope,
        OLD.snapshot_uid,OLD.snapshot_domain,OLD.snapshot_tick,OLD.snapshot_as_of,
        OLD.snapshot_validation_version,OLD.snapshot_hash,OLD.snapshot_payload)))
       OR (NEW.revision=OLD.revision AND OLD.handoff_domain IS NOT NULL
           AND (NEW.handoff_domain,NEW.handoff_tick) IS DISTINCT FROM (OLD.handoff_domain,OLD.handoff_tick))
       OR (NEW.revision=OLD.revision AND ((NEW.snapshot_claim_id,NEW.snapshot_revision,NEW.snapshot_owner,NEW.snapshot_tenant,
        NEW.snapshot_scope,NEW.snapshot_uid,NEW.snapshot_domain,NEW.snapshot_tick,
        NEW.snapshot_as_of,NEW.snapshot_validation_version,NEW.snapshot_hash,NEW.snapshot_payload) IS DISTINCT FROM (OLD.snapshot_claim_id,OLD.snapshot_revision,OLD.snapshot_owner,OLD.snapshot_tenant,
        OLD.snapshot_scope,OLD.snapshot_uid,OLD.snapshot_domain,OLD.snapshot_tick,
        OLD.snapshot_as_of,OLD.snapshot_validation_version,OLD.snapshot_hash,OLD.snapshot_payload)) AND
           (NEW.result_class IS DISTINCT FROM 'success' OR NEW.snapshot_claim_id IS DISTINCT FROM NEW.claim_id
            OR NEW.snapshot_revision IS DISTINCT FROM NEW.revision OR NEW.snapshot_owner IS DISTINCT FROM NEW.owner_process)) THEN
        RAISE EXCEPTION 'N5 claim result and snapshot transition conflict' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$fn$;

    -- ddl 52: n5_state_moves_forward
CREATE TRIGGER n5_state_moves_forward BEFORE UPDATE OR DELETE ON n5_refresh_state
    FOR EACH ROW EXECUTE FUNCTION n5_state_moves_forward();
END;
$migration38$;
