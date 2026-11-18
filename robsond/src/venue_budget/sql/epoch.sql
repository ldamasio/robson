-- B05: 9 binds: process epoch UUID,venue TEXT,boot UUID,offset BIGINT,
-- offset ns INTEGER,start-tick TEXT,predecessor UUID?,predecessor clean BOOL?,
-- fixed PG-microsecond audit TIMESTAMPTZ. Exactly one row/result, equality.
-- No ON CONFLICT UPDATE: budget_epochs_immutable forbids every UPDATE.
INSERT INTO budget_epochs(process_epoch,venue_id,clock_domain,start_tick,
                          predecessor_epoch,predecessor_clean,recorded_at)
VALUES ($1::uuid,$2::text,
        ROW('boottime'::text,$3::uuid,$4::bigint,$5::integer)::budget_clock_domain_v38,
        $6::text::numeric,$7::uuid,$8::boolean,$9::timestamptz)
RETURNING process_epoch,venue_id,(clock_domain).kind AS domain_kind,
          (clock_domain).boot_uuid AS domain_boot,
          (clock_domain).offset_seconds AS domain_offset_seconds,
          (clock_domain).offset_nanoseconds AS domain_offset_ns,
          start_tick::text AS start_tick_text,
          budget_integer_fits(start_tick,0,9223372036854775807999999999) AS start_tick_fits,
          predecessor_epoch,predecessor_clean,
          (extract(epoch FROM recorded_at)*1000000)::bigint AS recorded_at_us;
