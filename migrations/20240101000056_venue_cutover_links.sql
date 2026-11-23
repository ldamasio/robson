DO $migration56$
BEGIN
    -- ADR-0056 step 8m-1, S8-6 (2 of 2): one pending run per venue (single flight: a new run needs the previous
    -- one resumed to an end or abandoned), and the cutover_id of a venue account, a cash anchor and an activation
    -- basis names a run of the same tenant and venue (MATCH SIMPLE: a re-anchor anchor's NULL cutover_id is
    -- unconstrained). Completion inserts the venue account and certifies the activation basis only for its own
    -- run, in its one transaction (8m-5). venue_accounts is the only one of these tables already in production,
    -- where it is empty and unwritten while Spot is refused.
    CREATE UNIQUE INDEX idx_venue_cutovers_pending ON public.venue_cutovers (tenant_id, venue_id)
        WHERE state = 'pending';

    ALTER TABLE public.venue_accounts ADD CONSTRAINT venue_accounts_venue_cutover
        FOREIGN KEY (tenant_id, venue_id, cutover_id)
        REFERENCES public.venue_cutovers (tenant_id, venue_id, cutover_id);

    ALTER TABLE public.cash_anchors ADD CONSTRAINT cash_anchors_venue_cutover
        FOREIGN KEY (tenant_id, venue_id, cutover_id)
        REFERENCES public.venue_cutovers (tenant_id, venue_id, cutover_id);

    ALTER TABLE public.activation_bases ADD CONSTRAINT activation_bases_venue_cutover
        FOREIGN KEY (tenant_id, venue_id, cutover_id)
        REFERENCES public.venue_cutovers (tenant_id, venue_id, cutover_id);
END;
$migration56$;
