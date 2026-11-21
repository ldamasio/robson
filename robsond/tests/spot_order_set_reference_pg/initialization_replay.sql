-- The caller owns the complete disposable source fence and installed empty
-- shadow tables. Replay only derived facts using each source's current row.
DO $replay$
DECLARE
    cancel_order text := current_setting('robson_test.replay_cancel_order', true);
    j order_intents%ROWTYPE;
    o venue_orders%ROWTYPE;
    has_j boolean;
    natives bigint := 0;
    paired bigint := 0;
    journals bigint := 0;
    members bigint := 0;
    targets bigint := 0;
    writes_before bigint;
BEGIN
    IF cancel_order IS NULL OR cancel_order NOT IN ('members_first', 'targets_first') THEN
        RAISE EXCEPTION 'replay requires an explicit cancel ordering';
    END IF;
    IF EXISTS (SELECT 1 FROM venue_orders v JOIN order_intents i ON i.intent_id = v.intent_id
               WHERE 'binance:spot' IN (v.venue_id, i.venue_id)
                 AND (v.tenant_id, v.venue_id) IS DISTINCT FROM (i.tenant_id, i.venue_id)) THEN
        RAISE EXCEPTION 'replay cannot reconcile across accounting owners';
    END IF;

    -- The canonical unique intent index makes the native side a one-to-one
    -- decision for the J replay. A conflicting pair stays in both scopes.
    FOR o IN SELECT v.* FROM venue_orders v WHERE v.venue_id = 'binance:spot'
             ORDER BY v.account_scope, v.symbol, v.exchange_order_id LOOP
        j := NULL;
        has_j := false;
        IF o.intent_id IS NOT NULL THEN
            SELECT i.* INTO j FROM order_intents i WHERE i.intent_id = o.intent_id;
            has_j := FOUND;
        END IF;
        PERFORM sell_shadow_sync(j, has_j, o, true, 'replay', (o.intent_id IS NOT NULL)::integer);
        natives := natives + 1;
        IF has_j AND j.kind IN ('protection', 'exit') THEN
            paired := paired + 1;
        END IF;
    END LOOP;
    FOR j IN SELECT i.* FROM order_intents i
             WHERE i.venue_id = 'binance:spot' AND i.kind IN ('protection', 'exit')
               AND NOT EXISTS (SELECT 1 FROM venue_orders v WHERE v.intent_id = i.intent_id)
             ORDER BY i.intent_id LOOP
        PERFORM sell_shadow_sync(j, true, NULL::venue_orders, false,  'replay', 1);
        journals := journals + 1;
    END LOOP;
    IF paired + journals <> (SELECT count(*) FROM order_intents i
                             WHERE i.venue_id = 'binance:spot' AND i.kind IN ('protection', 'exit')) THEN
        RAISE EXCEPTION 'sell replay coverage mismatch';
    END IF;

    -- The result must be independent of which side of a cancellation was
    -- observed first. A terminal proof can exist before any member arrives.
    writes_before := cancel_shadow_member_writes();
    FOR step IN 1..2 LOOP
        IF (step = 1) = (cancel_order = 'members_first') THEN
            FOR j IN SELECT i.* FROM order_intents i
                     WHERE i.venue_id = 'binance:spot' AND i.kind = 'cancel'
                     ORDER BY i.intent_id LOOP
                PERFORM cancel_shadow_sync_member(j);
                members := members + 1;
            END LOOP;
        ELSE
            FOR o IN SELECT v.* FROM venue_orders v
                     WHERE v.venue_id = 'binance:spot' AND v.owner = 'leg'
                     ORDER BY v.account_scope, v.symbol, v.exchange_order_id LOOP
                PERFORM cancel_shadow_sync_target(o, false);
                targets := targets + 1;
            END LOOP;
        END IF;
    END LOOP;
    IF cancel_shadow_member_writes() - writes_before <> members
       OR members <> (SELECT count(*) FROM cancel_shadow_members) THEN
        RAISE EXCEPTION 'cancel replay requires one physical insert per member';
    END IF;
    PERFORM set_config('robson_test.replay_coverage', json_build_object(
        'sell_natives', natives, 'sell_paired', paired, 'sell_journals', journals,
        'cancel_members', members, 'cancel_targets', targets, 'order', cancel_order)::text, true);
END;
$replay$;
