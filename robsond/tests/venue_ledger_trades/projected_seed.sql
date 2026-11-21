-- Exact initialized membership for the four journal facts and two native SELLs.
-- Each projection table has at least one witness in this isolated database.
WITH journal AS (
    INSERT INTO public.order_intents (
        intent_id,tenant_id,venue_id,account_scope,client_order_id,position_id,kind,
        cancels_order_id,params,created_at
    )
    SELECT f.id,$1,'binance:spot',$2,f.id::text,f.position,f.kind,f.target,'{}'::jsonb,now()
    FROM (VALUES
        ($6::uuid,NULL::uuid,'exit',NULL::text),
        ($7::uuid,$4::uuid,'cancel','1'),
        ($8::uuid,$4::uuid,'cancel',NULL::text),
        ($9::uuid,NULL::uuid,'cancel','1')
    ) f(id,position,kind,target)
    RETURNING intent_id,position_id,kind,cancels_order_id
), sell_j AS (
    INSERT INTO public.spot_order_set_sell_intents (
        tenant_id,venue_id,account_scope,generation_id,intent_id,scope_kind,potential,conflict
    )
    SELECT $1,'binance:spot',$2,$3,intent_id,'unbound',true,false FROM journal WHERE kind='exit'
), sell_o AS (
    INSERT INTO public.spot_order_set_sell_orders (
        tenant_id,venue_id,account_scope,generation_id,symbol,exchange_order_id,scope_kind,
        potential,conflict,scope_symbol,scope_position
    )
    SELECT $1,'binance:spot',$2,$3,$5,o.exchange_order_id,
           CASE WHEN o.position_id IS NULL THEN 'symbol' ELSE 'leg' END,
           true,o.position_id IS NOT NULL,$5,o.position_id
    FROM public.venue_orders o
    WHERE (o.tenant_id,o.venue_id,o.account_scope,o.symbol) = ($1,'binance:spot',$2,$5)
      AND o.exchange_order_id IN ('1','2')
), sell_leg AS (
    INSERT INTO public.spot_order_set_sell_legs VALUES ($1,'binance:spot',$2,$3,$5,$4,1,1)
), sell_symbol AS (
    INSERT INTO public.spot_order_set_sell_symbols VALUES ($1,'binance:spot',$2,$3,$5,1,0)
), sell_unbound AS (
    INSERT INTO public.spot_order_set_sell_unbound VALUES ($1,'binance:spot',$2,$3,1,0)
), cancel_members AS (
    INSERT INTO public.spot_order_set_cancel_members (
        tenant_id,venue_id,account_scope,generation_id,intent_id,symbol,position_id,target_id,pending,conflict
    )
    SELECT $1,'binance:spot',$2,$3,intent_id,
           CASE WHEN position_id IS NOT NULL THEN $5 END,position_id,cancels_order_id,true,false
    FROM journal WHERE kind='cancel'
), cancel_group AS (
    INSERT INTO public.spot_order_set_cancel_groups VALUES ($1,'binance:spot',$2,$3,$5,$4,'1',1,0,false)
), cancel_leg AS (
    INSERT INTO public.spot_order_set_cancel_legs VALUES ($1,'binance:spot',$2,$3,$5,$4,1,0)
), cancel_unbound AS (
    INSERT INTO public.spot_order_set_cancel_unbound VALUES ($1,'binance:spot',$2,$3,1,0)
)
INSERT INTO public.spot_order_set_cancel_ungroupable VALUES ($1,'binance:spot',$2,$3,$5,$4,1,0)
