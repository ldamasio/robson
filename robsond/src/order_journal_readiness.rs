//! Bounded, flag-only catalog verification. The expected predicate is rendered
//! by this server from a temporary table with the real column types/collations.
use std::time::Duration;

use robson_exec::venue::VenueId;
use sqlx::{Connection, Row};
use uuid::Uuid;

use super::{PgJournalBackend, RetiringConnection};

const READY_BOUND: Duration = Duration::from_secs(2);
const INDEX_NAME: &str = "idx_submit_attempts_one_per_spot_client_id";

// Both catalog reads use this full structural check. Resolving the target via
// regclass respects this connection's search_path; the index must share its
// schema.
const CATALOG: &str = r#"
WITH needed(name) AS (VALUES ('accounting_state'), ('venue_accounts'), ('leg_routes'),
    ('balance_leg_admissions'), ('balance_legs'), ('venue_orders'), ('order_fills'), ('spot_intent_clock'))
SELECT t.oid::bigint AS target_oid, ix.oid::bigint AS index_oid, ns.nspname AS target_schema, t.relname AS target_name,
       current_setting('server_version_num')::integer >= 120000 AS supported,
       (SELECT bool_and(c.oid IS NOT NULL AND c.relkind IN ('r', 'p')
                        AND has_table_privilege(c.oid, 'SELECT'))
        FROM needed LEFT JOIN pg_class c ON c.oid = to_regclass(needed.name)) AS tables_ready,
       COALESCE(i.indrelid = t.oid AND ix.relnamespace = t.relnamespace AND am.amname = 'btree'
           AND i.indisunique AND i.indisvalid AND i.indisready AND i.indimmediate
           AND i.indnatts = 3 AND i.indnkeyatts = 3 AND i.indexprs IS NULL
           AND ARRAY(SELECT a.attname::text FROM unnest(i.indkey) WITH ORDINALITY AS k(attnum, n)
                     JOIN pg_attribute a ON a.attrelid = t.oid AND a.attnum = k.attnum ORDER BY k.n)
               = ARRAY['venue_id','account_scope','client_order_id']::text[], false) AS index_ready,
       pg_get_expr(i.indpred, i.indrelid, false) AS actual_predicate,
       pg_get_expr(e.indpred, e.indrelid, false) AS expected_predicate,
       COALESCE(i.indclass = e.indclass AND i.indcollation = e.indcollation
                AND i.indoption = e.indoption, false) AS same_key_semantics
FROM pg_class t JOIN pg_namespace ns ON ns.oid = t.relnamespace
LEFT JOIN pg_class ix ON ix.relnamespace = t.relnamespace AND ix.relname = $1
LEFT JOIN pg_index i ON i.indexrelid = ix.oid
LEFT JOIN pg_am am ON am.oid = ix.relam
LEFT JOIN pg_index e ON e.indexrelid = to_regclass($2)
WHERE t.oid = to_regclass('submit_attempts') AND t.relkind IN ('r', 'p')
"#;

fn identifier(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn validate(row: &sqlx::postgres::PgRow, one_attempt: bool) -> Result<(), String> {
    if !row.get::<bool, _>("supported") {
        return Err("balance schema requires PostgreSQL 12 or newer".to_string());
    }
    if !row.get::<bool, _>("tables_ready") {
        return Err("balance schema has a missing or unreadable ledger table".to_string());
    }
    if one_attempt && !row.get::<bool, _>("index_ready") {
        return Err(format!("{INDEX_NAME}: missing or invalid index definition"));
    }
    Ok(())
}

impl PgJournalBackend {
    pub(super) async fn probe_balance_schema(
        &self,
        venue: &str,
        one_attempt: bool,
    ) -> Result<(), String> {
        if one_attempt && venue != VenueId::BINANCE_SPOT {
            return Err(format!("no durable one-attempt index for venue {venue}"));
        }
        tokio::time::timeout(READY_BOUND, self.balance_schema_transaction(one_attempt))
            .await
            .map_err(|_| "balance schema readiness exceeded 2000 ms".to_string())?
    }

    async fn balance_schema_transaction(&self, one_attempt: bool) -> Result<(), String> {
        let error = |e: sqlx::Error| e.to_string();
        let connection = self.pool.acquire().await.map_err(error)?;
        let mut read = RetiringConnection { connection, finished: false };
        let mut tx = read.connection.begin().await.map_err(error)?;
        sqlx::query("SELECT set_config('lock_timeout', '300ms', true), set_config('statement_timeout', '1s', true), set_config('idle_in_transaction_session_timeout', '1s', true)")
            .execute(&mut *tx).await.map_err(error)?;
        let actual = sqlx::query(CATALOG)
            .bind(INDEX_NAME)
            .bind(Option::<&str>::None)
            .fetch_optional(&mut *tx)
            .await
            .map_err(error)?
            .ok_or_else(|| "balance schema has no submit_attempts table".to_string())?;
        validate(&actual, one_attempt)?;
        let mut matches = true;
        if one_attempt {
            let suffix = Uuid::now_v7().simple().to_string();
            let table = identifier(&format!("robson_expected_{suffix}"));
            let index = identifier(&format!("robson_expected_idx_{suffix}"));
            let target = format!(
                "{}.{}",
                identifier(actual.get("target_schema")),
                identifier(actual.get("target_name"))
            );
            sqlx::query(&format!("CREATE TEMP TABLE {table} (LIKE {target}) ON COMMIT DROP"))
                .execute(&mut *tx)
                .await
                .map_err(error)?;
            sqlx::query(&format!("CREATE UNIQUE INDEX {index} ON pg_temp.{table} (venue_id, account_scope, client_order_id) WHERE venue_id = 'binance:spot'"))
                .execute(&mut *tx).await.map_err(error)?;
            let checked = sqlx::query(CATALOG)
                .bind(INDEX_NAME)
                .bind(format!("pg_temp.{index}"))
                .fetch_optional(&mut *tx)
                .await
                .map_err(error)?;
            matches = checked.as_ref().is_some_and(|row| {
                validate(row, true).is_ok()
                    && row.get::<i64, _>("target_oid") == actual.get::<i64, _>("target_oid")
                    && row.get::<Option<i64>, _>("index_oid")
                        == actual.get::<Option<i64>, _>("index_oid")
                    && row.get::<bool, _>("same_key_semantics")
                    && row.get::<Option<String>, _>("expected_predicate").is_some()
                    && row.get::<Option<String>, _>("actual_predicate")
                        == row.get::<Option<String>, _>("expected_predicate")
            });
        }
        // Roll back even a predicate mismatch. Any error/cancellation/lost
        // answer retires the connection instead of returning a live temp object.
        tx.rollback().await.map_err(error)?;
        if !matches {
            return Err(format!(
                "{INDEX_NAME}: predicate or definition differs from expected index"
            ));
        }
        read.finished = true;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_identifiers_are_quoted_without_changing_their_names() {
        assert_eq!(identifier("a\"b"), "\"a\"\"b\"");
        assert_eq!(identifier("odd.schema"), "\"odd.schema\"");
    }
}
