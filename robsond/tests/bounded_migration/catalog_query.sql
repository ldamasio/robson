WITH
wanted(name) AS (VALUES
 ('clock_samples'),('venue_pauses'),('incident_budget_usage'),('budget_venue_state'),
 ('budget_epochs'),('budget_source_audits'),('budget_recovery_cohorts'),
 ('budget_expiry_proofs'),('budget_daily_charges'),('n5_registry_root'),
 ('n5_refresh_keys'),('n5_refresh_state'),('venue_orders'),('spot_intent_clock'),
 ('recovery_incidents'),('process_epochs'),('submit_attempts'),('order_intents'),('accounting_state')),
tables0 AS MATERIALIZED (
 SELECT c.* FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace
 WHERE n.nspname='public' AND c.relname IN(SELECT name FROM wanted) LIMIT 4097),
indexes0 AS MATERIALIZED (
 SELECT i.* FROM pg_catalog.pg_index i WHERE i.indrelid IN(SELECT oid FROM tables0) LIMIT 4097),
relations0 AS MATERIALIZED (
 SELECT c.* FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace
 WHERE n.nspname='public' AND (c.oid IN(SELECT oid FROM tables0)
 OR c.oid IN(SELECT indexrelid FROM indexes0) OR c.relname='budget_clock_domain_v38') LIMIT 4097),
attributes0 AS MATERIALIZED (
 SELECT a.* FROM pg_catalog.pg_attribute a WHERE a.attrelid IN(SELECT oid FROM relations0)
 AND a.attnum>0 AND NOT a.attisdropped LIMIT 4097),
constraints0 AS MATERIALIZED (
 SELECT c.* FROM pg_catalog.pg_constraint c WHERE c.conrelid IN(SELECT oid FROM tables0) LIMIT 4097),
triggers0 AS MATERIALIZED (
 SELECT t.* FROM pg_catalog.pg_trigger t WHERE t.tgrelid IN(SELECT oid FROM tables0) LIMIT 4097),
functions0 AS MATERIALIZED (
 SELECT p.* FROM pg_catalog.pg_proc p JOIN pg_catalog.pg_namespace n ON n.oid=p.pronamespace
 WHERE n.nspname='public' AND (p.oid IN(SELECT tgfoid FROM triggers0)
 OR p.proname IN('budget_integer_fits','budget_insert_clocks','budget_project_clocks')) LIMIT 4097),
types0 AS MATERIALIZED (
 SELECT t.* FROM pg_catalog.pg_type t JOIN pg_catalog.pg_namespace n ON n.oid=t.typnamespace
 WHERE n.nspname='public' AND (t.typrelid IN(SELECT oid FROM relations0)
 OR t.typelem IN(SELECT reltype FROM relations0) OR t.typname='budget_clock_domain_v38') LIMIT 4097),
objects0 AS MATERIALIZED (
 SELECT 'pg_class'::regclass::oid AS classid,c.oid,0 AS subid,'relation:public.'||c.relname AS key FROM relations0 c
 UNION ALL SELECT 'pg_class'::regclass::oid,a.attrelid,a.attnum,
 'attribute:public.'||c.relname||'/'||a.attname FROM attributes0 a JOIN relations0 c ON c.oid=a.attrelid
 UNION ALL SELECT 'pg_constraint'::regclass::oid,c.oid,0,
 'constraint:public.'||r.relname||'/'||c.conname FROM constraints0 c JOIN tables0 r ON r.oid=c.conrelid
 UNION ALL SELECT 'pg_proc'::regclass::oid,p.oid,0,'function:public.'||p.proname||'('||pg_catalog.pg_get_function_identity_arguments(p.oid)||')' FROM functions0 p
 UNION ALL SELECT 'pg_type'::regclass::oid,t.oid,0,'type:public.'||t.typname FROM types0 t
 UNION ALL SELECT 'pg_trigger'::regclass::oid,t.oid,0,
 CASE WHEN t.tgisinternal AND c.contype='f' THEN
 'ri:public.'||cr.relname||'/'||c.conname||'/'||p.proname||'/'||r.relname
 ELSE 'trigger:public.'||r.relname||'/'||t.tgname END
 FROM triggers0 t JOIN tables0 r ON r.oid=t.tgrelid JOIN pg_catalog.pg_proc p ON p.oid=t.tgfoid
 LEFT JOIN pg_catalog.pg_constraint c ON c.oid=t.tgconstraint LEFT JOIN pg_catalog.pg_class cr ON cr.oid=c.conrelid),
dependencies0 AS MATERIALIZED (
 SELECT d.* FROM pg_catalog.pg_depend d WHERE EXISTS(
 SELECT 1 FROM objects0 o WHERE o.classid=d.classid AND o.oid=d.objid
 AND (o.subid=d.objsubid OR o.subid=0)) LIMIT 4097),
public_names0 AS MATERIALIZED (
 SELECT 'public-relation:'||c.relname AS key FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='public'
 UNION ALL SELECT 'public-type:'||t.typname FROM pg_catalog.pg_type t JOIN pg_catalog.pg_namespace n ON n.oid=t.typnamespace WHERE n.nspname='public'
 UNION ALL SELECT 'public-function:'||p.proname||'('||pg_catalog.pg_get_function_identity_arguments(p.oid)||')'
 FROM pg_catalog.pg_proc p JOIN pg_catalog.pg_namespace n ON n.oid=p.pronamespace WHERE n.nspname='public'
 LIMIT 4097),
precheck AS MATERIALIZED (
 SELECT objects_n+indexes_n+dependencies_n+public_names_n AS n,c.* FROM (
 SELECT (SELECT count(*) FROM objects0) AS objects_n,(SELECT count(*) FROM indexes0) AS indexes_n,
 (SELECT count(*) FROM dependencies0) AS dependencies_n,(SELECT count(*) FROM public_names0) AS public_names_n) c),
dependency_edges AS MATERIALIZED (
 SELECT 'dependency:'||coalesce(o.key,di.identity)||'->'||coalesce(r.key,ri.identity)||'/'||d.deptype::text AS edge_key,
 d.classid::text||':'||d.objid||':'||d.objsubid||'/'||d.refclassid||':'||d.refobjid||':'||d.refobjsubid AS identity,
 jsonb_build_object('object',coalesce(o.key,di.identity),'reference',coalesce(r.key,ri.identity),'type',d.deptype,
 'object_catalog',d.classid::regclass::text,'reference_catalog',d.refclassid::regclass::text) AS sem
 FROM dependencies0 d LEFT JOIN objects0 o ON o.classid=d.classid AND o.oid=d.objid AND o.subid=d.objsubid
 LEFT JOIN objects0 r ON r.classid=d.refclassid AND r.oid=d.refobjid AND r.subid=d.refobjsubid
 CROSS JOIN LATERAL pg_catalog.pg_identify_object(d.classid,d.objid,d.objsubid) di
 CROSS JOIN LATERAL pg_catalog.pg_identify_object(d.refclassid,d.refobjid,d.refobjsubid) ri
 WHERE (SELECT n<=4096 FROM precheck)),
-- Preserve every native dependency occurrence. Only this family is ordinalized.
-- Semantic ordering makes R/C independent of OIDs; native identity breaks ties
-- solely among equal semantic descriptors (including exact duplicate edges).
dependency_records AS MATERIALIZED (
 SELECT edge_key||'/occurrence:'||row_number() OVER (
 PARTITION BY edge_key COLLATE "C" ORDER BY sem::text COLLATE "C",identity COLLATE "C")::text AS key,
 identity,sem FROM dependency_edges),
raw AS MATERIALIZED (
 SELECT o.key,c.oid::text AS identity,jsonb_build_object(
 'kind',c.relkind,'persistence',c.relpersistence,'owner',pg_catalog.pg_get_userbyid(c.relowner),
 'access_method',am.amname,'options',c.reloptions,'acl',c.relacl::text,
 'rls',c.relrowsecurity,'force_rls',c.relforcerowsecurity,
 'row_type',pg_catalog.format_type(NULLIF(c.reltype,0),NULL),'partition',c.relispartition) AS sem
 FROM relations0 c JOIN objects0 o ON o.classid='pg_class'::regclass AND o.oid=c.oid AND o.subid=0
 LEFT JOIN pg_catalog.pg_am am ON am.oid=c.relam WHERE (SELECT n<=4096 FROM precheck)
 UNION ALL
 -- pg_attver is server-owned and derived from WAL insertion positions, which
 -- advance after rollback. It stays in native identity, never in semantics;
 -- every other option keeps its order, and NULL options remain NULL.
 SELECT o.key,a.attrelid::text||':'||a.attnum||coalesce('/'||(SELECT string_agg(u.opt,',' ORDER BY u.n)
 FROM unnest(a.attoptions) WITH ORDINALITY u(opt,n) WHERE starts_with(u.opt,'pg_attver=')),''),jsonb_build_object(
 'number',a.attnum,'type',pg_catalog.format_type(a.atttypid,a.atttypmod),'typmod',a.atttypmod,
 'not_null',a.attnotnull,'identity',a.attidentity,'generated',a.attgenerated,
 'collation',(SELECT n.nspname||'.'||c.collname FROM pg_catalog.pg_collation c JOIN pg_catalog.pg_namespace n ON n.oid=c.collnamespace WHERE c.oid=a.attcollation),
 'default',pg_catalog.pg_get_expr(d.adbin,d.adrelid,false),
 'options',CASE WHEN a.attoptions IS NOT NULL THEN ARRAY(SELECT u.opt
 FROM unnest(a.attoptions) WITH ORDINALITY u(opt,n)
 WHERE u.opt IS NULL OR NOT starts_with(u.opt,'pg_attver=') ORDER BY u.n) END)
 FROM attributes0 a JOIN objects0 o ON o.classid='pg_class'::regclass AND o.oid=a.attrelid AND o.subid=a.attnum
 LEFT JOIN pg_catalog.pg_attrdef d ON d.adrelid=a.attrelid AND d.adnum=a.attnum
 WHERE (SELECT n<=4096 FROM precheck)
 UNION ALL
 SELECT o.key,c.oid::text,jsonb_build_object(
 'type',c.contype,'validated',c.convalidated,'deferrable',c.condeferrable,'deferred',c.condeferred,
 'local',c.conislocal,'inheritance_count',c.coninhcount,'no_inherit',c.connoinherit,
 'table','public.'||r.relname,'parent_table',pn.nspname||'.'||pr.relname,
 'match',c.confmatchtype,'update',c.confupdtype,'delete',c.confdeltype,
 'index',ixn.nspname||'.'||ix.relname,
 'key_names',(SELECT jsonb_agg(a.attname ORDER BY k.n) FROM unnest(c.conkey) WITH ORDINALITY k(att,n) JOIN pg_catalog.pg_attribute a ON a.attrelid=c.conrelid AND a.attnum=k.att),
 'parent_keys',(SELECT jsonb_agg(a.attname ORDER BY k.n) FROM unnest(c.confkey) WITH ORDINALITY k(att,n) JOIN pg_catalog.pg_attribute a ON a.attrelid=c.confrelid AND a.attnum=k.att),
 'key_types',(SELECT jsonb_agg(pg_catalog.format_type(a.atttypid,a.atttypmod) ORDER BY k.n) FROM unnest(c.conkey) WITH ORDINALITY k(att,n) JOIN pg_catalog.pg_attribute a ON a.attrelid=c.conrelid AND a.attnum=k.att),
 'parent_types',(SELECT jsonb_agg(pg_catalog.format_type(a.atttypid,a.atttypmod) ORDER BY k.n) FROM unnest(c.confkey) WITH ORDINALITY k(att,n) JOIN pg_catalog.pg_attribute a ON a.attrelid=c.confrelid AND a.attnum=k.att),
 'definition',pg_catalog.pg_get_constraintdef(c.oid,false),
 'expression',pg_catalog.pg_get_expr(c.conbin,c.conrelid,false))
 FROM constraints0 c JOIN objects0 o ON o.classid='pg_constraint'::regclass AND o.oid=c.oid
 JOIN tables0 r ON r.oid=c.conrelid LEFT JOIN pg_catalog.pg_class pr ON pr.oid=c.confrelid
 LEFT JOIN pg_catalog.pg_namespace pn ON pn.oid=pr.relnamespace
 LEFT JOIN pg_catalog.pg_class ix ON ix.oid=c.conindid LEFT JOIN pg_catalog.pg_namespace ixn ON ixn.oid=ix.relnamespace
 WHERE (SELECT n<=4096 FROM precheck)
 UNION ALL
 SELECT 'index:public.'||r.relname,i.indexrelid::text,jsonb_build_object(
 'table','public.'||t.relname,'definition',pg_catalog.pg_get_indexdef(i.indexrelid,0,false),
 'unique',i.indisunique,'primary',i.indisprimary,'valid',i.indisvalid,'ready',i.indisready,
 'immediate',i.indimmediate,'exclusion',i.indisexclusion,'live',i.indislive,
 'key_count',i.indnkeyatts,'attribute_count',i.indnatts,'key_positions',i.indkey::text,
 'key_options',i.indoption::text,'predicate',pg_catalog.pg_get_expr(i.indpred,i.indrelid,false),
 'expressions',pg_catalog.pg_get_expr(i.indexprs,i.indrelid,false),
 'nulls_not_distinct_present',to_jsonb(i)?'indnullsnotdistinct',
 'nulls_not_distinct',to_jsonb(i)->'indnullsnotdistinct',
 'opclasses',(SELECT jsonb_agg(jsonb_build_array(n.nspname,c.opcname,fn.nspname,f.opfname,pg_catalog.format_type(c.opcintype,NULL)) ORDER BY k.n)
 FROM unnest(i.indclass::oid[]) WITH ORDINALITY k(id,n) JOIN pg_catalog.pg_opclass c ON c.oid=k.id
 JOIN pg_catalog.pg_namespace n ON n.oid=c.opcnamespace JOIN pg_catalog.pg_opfamily f ON f.oid=c.opcfamily JOIN pg_catalog.pg_namespace fn ON fn.oid=f.opfnamespace),
 'collations',(SELECT jsonb_agg(n.nspname||'.'||c.collname ORDER BY k.n) FROM unnest(i.indcollation::oid[]) WITH ORDINALITY k(id,n)
 LEFT JOIN pg_catalog.pg_collation c ON c.oid=k.id LEFT JOIN pg_catalog.pg_namespace n ON n.oid=c.collnamespace))
 FROM indexes0 i JOIN relations0 r ON r.oid=i.indexrelid JOIN tables0 t ON t.oid=i.indrelid
 WHERE (SELECT n<=4096 FROM precheck)
 UNION ALL
 SELECT o.key,p.oid::text,jsonb_build_object(
 'identity_arguments',pg_catalog.pg_get_function_identity_arguments(p.oid),
 'arguments',pg_catalog.pg_get_function_arguments(p.oid),'result',pg_catalog.pg_get_function_result(p.oid),
 'kind',p.prokind,'returns_set',p.proretset,'volatility',p.provolatile,'strict',p.proisstrict,
 'security_definer',p.prosecdef,'leakproof',p.proleakproof,'parallel',p.proparallel,
 'config',p.proconfig,'language',l.lanname,'source',p.prosrc,'binary',p.probin,
 'cost',p.procost::text,'rows',p.prorows::text,'definition',pg_catalog.pg_get_functiondef(p.oid),
 'owner',pg_catalog.pg_get_userbyid(p.proowner),'acl',p.proacl::text)
 FROM functions0 p JOIN objects0 o ON o.classid='pg_proc'::regclass AND o.oid=p.oid
 JOIN pg_catalog.pg_language l ON l.oid=p.prolang WHERE (SELECT n<=4096 FROM precheck)
 UNION ALL
 SELECT o.key,t.oid::text,jsonb_build_object(
 'kind',t.typtype,'category',t.typcategory,'not_null',t.typnotnull,
 'base',pg_catalog.format_type(NULLIF(t.typbasetype,0),t.typtypmod),
 'element',pg_catalog.format_type(NULLIF(t.typelem,0),NULL),'array',pg_catalog.format_type(NULLIF(t.typarray,0),NULL),
 'relation',(SELECT n.nspname||'.'||r.relname FROM pg_catalog.pg_class r JOIN pg_catalog.pg_namespace n ON n.oid=r.relnamespace WHERE r.oid=t.typrelid),
 'default',t.typdefault,'default_expression',pg_catalog.pg_get_expr(t.typdefaultbin,0,false),
 'owner',pg_catalog.pg_get_userbyid(t.typowner),'acl',t.typacl::text)
 FROM types0 t JOIN objects0 o ON o.classid='pg_type'::regclass AND o.oid=t.oid
 WHERE (SELECT n<=4096 FROM precheck)
 UNION ALL
 SELECT o.key,t.oid::text||':'||t.tgname,jsonb_build_object(
 'table','public.'||r.relname,'function',pn.nspname||'.'||p.proname||'('||pg_catalog.pg_get_function_identity_arguments(p.oid)||')',
 'type',t.tgtype,'enabled',t.tgenabled,'internal',t.tgisinternal,
 'deferrable',t.tgdeferrable,'deferred',t.tginitdeferred,'arguments',encode(t.tgargs,'hex'),
 'update_columns',t.tgattr::text,'when',pg_catalog.pg_get_expr(t.tgqual,t.tgrelid,false),
 'old_table',t.tgoldtable,'new_table',t.tgnewtable,
 'constraint',co.key,'other_table',onsp.nspname||'.'||oth.relname,
 'index',ixn.nspname||'.'||ix.relname,
 'binding_ok',CASE WHEN t.tgisinternal AND c.contype='f' THEN
 ((t.tgrelid=c.conrelid AND t.tgconstrrelid=c.confrelid)
 OR (t.tgrelid=c.confrelid AND t.tgconstrrelid=c.conrelid)) ELSE true END,
 'definition',CASE WHEN NOT t.tgisinternal THEN pg_catalog.pg_get_triggerdef(t.oid,false) END)
 FROM triggers0 t JOIN objects0 o ON o.classid='pg_trigger'::regclass AND o.oid=t.oid
 JOIN tables0 r ON r.oid=t.tgrelid JOIN pg_catalog.pg_proc p ON p.oid=t.tgfoid
 JOIN pg_catalog.pg_namespace pn ON pn.oid=p.pronamespace
 LEFT JOIN pg_catalog.pg_constraint c ON c.oid=t.tgconstraint
 LEFT JOIN objects0 co ON co.classid='pg_constraint'::regclass AND co.oid=t.tgconstraint
 LEFT JOIN pg_catalog.pg_class oth ON oth.oid=t.tgconstrrelid LEFT JOIN pg_catalog.pg_namespace onsp ON onsp.oid=oth.relnamespace
 LEFT JOIN pg_catalog.pg_class ix ON ix.oid=t.tgconstrindid LEFT JOIN pg_catalog.pg_namespace ixn ON ixn.oid=ix.relnamespace
 WHERE (SELECT n<=4096 FROM precheck)
 UNION ALL
 SELECT key,identity,sem FROM dependency_records
 UNION ALL
 SELECT key,NULL,jsonb_build_object('present',true) FROM public_names0 WHERE (SELECT n<=4096 FROM precheck)),
encoded AS MATERIALIZED (
 SELECT key,identity,sem,octet_length(jsonb_build_object('key',key,'identity',identity,'sem',sem)::text) AS bytes FROM raw),
summary AS MATERIALIZED (
 SELECT count(*) AS n,count(DISTINCT key) AS unique_n,count(*) FILTER (WHERE key IS NULL) AS null_key_n,
 coalesce(max(bytes),0) AS max_bytes,
 coalesce(sum(bytes+2),0) AS bytes FROM encoded),
duplicates AS MATERIALIZED (
 SELECT left(key,96) AS key_prefix,count(*) AS multiplicity,octet_length(key) AS key_bytes
 FROM encoded WHERE key IS NOT NULL GROUP BY key HAVING count(*)>1
 ORDER BY key COLLATE "C" LIMIT 4),
payload AS MATERIALIZED (
 SELECT jsonb_build_object('overflow',false,'records',coalesce(jsonb_agg(
 jsonb_build_object('key',key,'identity',identity,'sem',sem) ORDER BY key COLLATE "C"),'[]'::jsonb)) AS body
 FROM encoded WHERE (SELECT n<=4096 AND n=unique_n AND max_bytes<=65536 AND bytes<=4190000 FROM summary)),
context AS MATERIALIZED (
 SELECT jsonb_build_object('major',current_setting('server_version_num')::int/10000,
 'role',current_user,'schema',current_schema(),'search_path',current_setting('search_path'),
 'encoding',current_setting('server_encoding'),'pid',pg_backend_pid(),
 'lock',current_setting('lock_timeout'),'statement',current_setting('statement_timeout'),
 'idle',current_setting('idle_in_transaction_session_timeout'),
 'metadata',(SELECT jsonb_build_object('description',description,'success',success,
 'checksum',encode(checksum,'hex'),'elapsed',execution_time::text)
 FROM public._sqlx_migrations WHERE version=20240101000038)) AS header),
result AS MATERIALIZED (
 SELECT body||jsonb_build_object('context',header) AS body FROM payload CROSS JOIN context)
SELECT CASE WHEN (SELECT n<=4096 FROM precheck)
 AND (SELECT n<=4096 AND n=unique_n AND max_bytes<=65536 AND bytes<=4190000 FROM summary)
 AND octet_length((body->'context')::text)<=65536
 AND octet_length(body::text)<=4194304 THEN body::text
 ELSE jsonb_build_object('overflow',true,'reason','catalog count/duplicate/byte bound',
 'diagnostic',jsonb_build_object(
 'precheck_n',(SELECT n FROM precheck),'objects_n',(SELECT objects_n FROM precheck),
 'indexes_n',(SELECT indexes_n FROM precheck),'dependencies_n',(SELECT dependencies_n FROM precheck),
 'public_names_n',(SELECT public_names_n FROM precheck),'encoded_n',(SELECT n FROM summary),
 'unique_n',(SELECT unique_n FROM summary),'null_key_n',(SELECT null_key_n FROM summary),
 'max_bytes',(SELECT max_bytes FROM summary),'bytes',(SELECT bytes FROM summary),
 'context_bytes',octet_length((body->'context')::text),'body_bytes',octet_length(body::text),
 'duplicate_keys',(SELECT coalesce(jsonb_agg(jsonb_build_object('key_prefix',key_prefix,
 'multiplicity',multiplicity,'key_bytes',key_bytes) ORDER BY key_prefix COLLATE "C"),'[]'::jsonb) FROM duplicates)))::text END
FROM result
