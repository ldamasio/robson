//! Exact leaf encoding and SQL for the fenced cold Spot installer.
//! Source: 3c944bba9, migrations M41:29/57, M42:3/24/44,
//! M43:3/26/49, M44:3/22, M45:3 and venue_ledger_orders/oracle_seed.sql:65.
//! This module never reads or advances the generation metadata.
use std::collections::BTreeMap;

use serde_json::{json, Value};
use sqlx::{types::Json, PgConnection, Postgres, Transaction};
use uuid::Uuid;

use super::{
    cancel::{CancelCounts, CancelLeaves},
    sell::{SellContribution, SellCounts, SellProjection, SellScope},
};

const TABLE_COUNT: usize = 10;
const LEAF_LIMIT: usize = 640;
const PAYLOAD_LIMIT: usize = 512 * 1024;
const TEXT_LIMIT: usize = 256;
const PREFIX: &str = "tenant_id,venue_id,account_scope,generation_id";
const SCOPE_TEST: &str = "(tenant_id,venue_id,account_scope,generation_id)
    = ($1::uuid,'binance:spot',$2::text,$3::uuid)";

struct LeafTable {
    name: &'static str,
    key: &'static [&'static str],
    columns: &'static [(&'static str, &'static str)],
}

macro_rules! leaf {
    ($name:literal, [$($key:ident),*], [$($column:ident : $ty:ident),* $(,)?]) => {
        LeafTable {
            name: concat!("spot_order_set_", $name),
            key: &[$(stringify!($key)),*],
            columns: &[$((stringify!($column), stringify!($ty))),*],
        }
    };
}

// The order is also the stable position of each table in the JSON wire array.
const TABLES: [LeafTable; TABLE_COUNT] = [
    leaf!("sell_intents", [intent_id], [
        intent_id: uuid, scope_kind: text, potential: boolean, conflict: boolean,
        scope_symbol: text, scope_position: uuid, source_position: uuid,
    ]),
    leaf!("sell_orders", [symbol, exchange_order_id], [
        symbol: text, exchange_order_id: text, scope_kind: text, potential: boolean,
        conflict: boolean, scope_symbol: text, scope_position: uuid,
    ]),
    leaf!("sell_legs", [symbol, position_id], [
        symbol: text, position_id: uuid, potential_count: bigint, conflict_count: bigint,
    ]),
    leaf!("sell_symbols", [symbol], [symbol: text, potential_count: bigint, conflict_count: bigint]),
    leaf!("sell_unbound", [], [potential_count: bigint, conflict_count: bigint]),
    leaf!("cancel_members", [intent_id], [
        intent_id: uuid, symbol: text, position_id: uuid, source_position: uuid, target_id: text,
        pending: boolean, conflict: boolean,
    ]),
    leaf!("cancel_groups", [symbol, position_id, target_id], [
        symbol: text, position_id: uuid, target_id: text, pending_count: bigint,
        conflict_count: bigint, terminal_mask: boolean,
    ]),
    leaf!("cancel_legs", [symbol, position_id], [
        symbol: text, position_id: uuid, active_count: bigint, conflict_count: bigint,
    ]),
    leaf!("cancel_unbound", [], [active_count: bigint, conflict_count: bigint]),
    leaf!("cancel_ungroupable", [symbol, position_id], [
        symbol: text, position_id: uuid, active_count: bigint, conflict_count: bigint,
    ]),
];

#[derive(Clone, Copy)]
pub(super) struct LeafScope<'a> {
    pub(super) tenant_id: Uuid,
    pub(super) account_scope: &'a str,
    pub(super) generation_id: Uuid,
}

#[derive(Debug, thiserror::Error)]
pub(in crate::spot_order_set_store) enum LeafError {
    #[error("cold leaf serialization exceeded its bound")]
    Budget,
    #[error("cold leaf exact comparison failed on {0}")]
    Mismatch(&'static str),
    #[error("cold leaf JSON could not be encoded")]
    Encoding(#[from] serde_json::Error),
    #[error("cold leaf SQL failed")]
    Database(#[from] sqlx::Error),
}

pub(super) struct PreparedLeaves {
    rows: [Vec<Value>; TABLE_COUNT],
    counts: [i64; TABLE_COUNT],
}

fn sell_scope<'a>(row: SellContribution<'a>) -> (&'static str, Option<&'a str>, Option<Uuid>) {
    match row.scope {
        SellScope::Leg { symbol, position } => ("leg", Some(symbol), Some(position)),
        SellScope::Symbol(symbol) => ("symbol", Some(symbol), None),
        SellScope::Unbound => ("unbound", None, None),
    }
}

fn sell_counts(counts: &SellCounts) -> Value {
    json!({"potential_count": counts.potential, "conflict_count": counts.conflict})
}

fn cancel_counts(counts: &CancelCounts) -> Value {
    json!({"active_count": counts.active_count, "conflict_count": counts.conflict_count})
}

impl PreparedLeaves {
    pub(super) fn new(
        sell: &SellProjection<'_>,
        cancel: &CancelLeaves<'_>,
    ) -> Result<Self, LeafError> {
        // Validate before allocating the wire representation. Reconstruction owns its
        // separate typed-data bound, and this method owns the serialized bound.
        let total = [
            sell.leaf_rows(),
            cancel.members.len(),
            cancel.groups.len(),
            cancel.legs.len(),
            usize::from(cancel.unbound.is_some()),
            cancel.ungroupable.len(),
        ]
        .into_iter()
        .try_fold(0usize, usize::checked_add)
        .ok_or(LeafError::Budget)?;
        if total > LEAF_LIMIT {
            return Err(LeafError::Budget);
        }
        let rows: [Vec<Value>; TABLE_COUNT] = [
            sell.intents.iter().map(|(id, row)| {
                let (kind, symbol, position) = sell_scope(row.contribution);
                json!({"intent_id": id, "scope_kind": kind, "potential": true,
                    "conflict": row.contribution.conflict, "scope_symbol": symbol,
                    "scope_position": position, "source_position": row.source_position})
            }).collect(),
            sell.orders.iter().map(|((symbol, order_id), row)| {
                let (kind, scope_symbol, scope_position) = sell_scope(*row);
                json!({"symbol": symbol, "exchange_order_id": order_id, "scope_kind": kind,
                    "potential": true, "conflict": row.conflict, "scope_symbol": scope_symbol,
                    "scope_position": scope_position})
            }).collect(),
            sell.legs.iter().map(|((symbol, position), counts)| {
                json!({"symbol": symbol, "position_id": position,
                    "potential_count": counts.potential, "conflict_count": counts.conflict})
            }).collect(),
            sell.symbols.iter().map(|(symbol, counts)| {
                json!({"symbol": symbol, "potential_count": counts.potential,
                    "conflict_count": counts.conflict})
            }).collect(),
            sell.unbound.iter().map(sell_counts).collect(),
            cancel.members.iter().map(|(id, row)| {
                json!({"intent_id": id, "symbol": row.leg.map(|k| k.symbol),
                    "position_id": row.leg.map(|k| k.position_id),
                    "source_position": row.source_position, "target_id": row.target_id,
                    "pending": row.pending, "conflict": row.conflict})
            }).collect(),
            cancel.groups.iter().map(|(key, row)| {
                json!({"symbol": key.leg.symbol, "position_id": key.leg.position_id, "target_id": key.target_id,
                    "pending_count": row.pending_count, "conflict_count": row.conflict_count,
                    "terminal_mask": row.terminal_mask})
            }).collect(),
            cancel.legs.iter().map(|(key, counts)| {
                json!({"symbol": key.symbol, "position_id": key.position_id,
                    "active_count": counts.active_count, "conflict_count": counts.conflict_count})
            }).collect(),
            cancel.unbound.iter().map(cancel_counts).collect(),
            cancel.ungroupable.iter().map(|(key, counts)| {
                json!({"symbol": key.symbol, "position_id": key.position_id,
                    "active_count": counts.active_count, "conflict_count": counts.conflict_count})
            }).collect(),
        ];
        let oversized_text = TABLES.iter().zip(&rows).any(|(table, values)| {
            values.iter().any(|row| {
                table.columns.iter().any(|(name, ty)| {
                    *ty == "text" && row[*name].as_str().is_some_and(|text| text.len() > TEXT_LIMIT)
                })
            })
        });
        if oversized_text || serde_json::to_vec(&rows)?.len() > PAYLOAD_LIMIT {
            return Err(LeafError::Budget);
        }
        let counts = rows.each_ref().map(|table| table.len() as i64);
        Ok(Self { rows, counts })
    }

    pub(super) fn counts(&self) -> &[i64] {
        &self.counts
    }

    pub(super) fn payload(&self) -> Json<&[Vec<Value>; TABLE_COUNT]> {
        Json(&self.rows)
    }

    /// Fixed ten wire commands, including empty batches. The caller's original
    /// transaction keeps partial inserts invisible on any refusal.
    pub(super) async fn insert(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        scope: &LeafScope<'_>,
    ) -> Result<(), LeafError> {
        for (table, expected) in TABLES.iter().zip(&self.rows) {
            let sql = insert_sql(table);
            let returned = sqlx::query_scalar::<_, Json<Value>>(&sql)
                .bind(scope.tenant_id)
                .bind(scope.account_scope)
                .bind(scope.generation_id)
                .bind(Json(expected))
                .fetch_all(&mut **tx)
                .await?;
            compare_rows(table, expected, returned.iter().map(|row| &row.0), returned.len())?;
        }
        Ok(())
    }

    pub(super) fn compare_namespace(&self, observed: &Value) -> Result<(), LeafError> {
        let observed = observed
            .as_array()
            .filter(|families| families.len() == TABLE_COUNT)
            .ok_or(LeafError::Mismatch("namespace"))?;
        for ((table, expected), returned) in TABLES.iter().zip(&self.rows).zip(observed) {
            let returned = returned.as_array().ok_or(LeafError::Mismatch(table.name))?;
            compare_rows(table, expected, returned.iter(), returned.len())?;
        }
        Ok(())
    }

    /// One independent, bounded postcommit readback using the same exact
    /// namespace prefix. A returned change or surplus row is a refusal.
    pub(super) async fn verify(
        &self,
        connection: &mut PgConnection,
        scope: &LeafScope<'_>,
    ) -> Result<(), LeafError> {
        let sql = format!("SELECT {}", namespace_expression());
        let Json(observed) = sqlx::query_scalar::<_, Json<Value>>(&sql)
            .bind(scope.tenant_id)
            .bind(scope.account_scope)
            .bind(scope.generation_id)
            .bind(self.counts())
            .fetch_one(connection)
            .await?;
        self.compare_namespace(&observed)
    }
}

fn compare_rows<'a>(
    table: &LeafTable,
    expected: &[Value],
    returned: impl Iterator<Item = &'a Value>,
    returned_len: usize,
) -> Result<(), LeafError> {
    let mismatch = || LeafError::Mismatch(table.name);
    if returned_len != expected.len() {
        return Err(mismatch());
    }
    let key = |row: &Value| {
        let object = row.as_object().ok_or_else(mismatch)?;
        if object.len() != table.columns.len() {
            return Err(mismatch());
        }
        table
            .key
            .iter()
            .map(|column| {
                object
                    .get(*column)
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .ok_or_else(mismatch)
            })
            .collect::<Result<Vec<_>, _>>()
    };
    let mut unmatched = BTreeMap::new();
    for row in expected {
        if unmatched.insert(key(row)?, row).is_some() {
            return Err(mismatch());
        }
    }
    for row in returned {
        if unmatched.remove(&key(row)?) != Some(row) {
            return Err(mismatch());
        }
    }
    if !unmatched.is_empty() {
        return Err(mismatch());
    }
    Ok(())
}

fn bound_text_fields(table: &LeafTable, alias: &str) -> String {
    let checks = table
        .columns
        .iter()
        .filter(|(_, ty)| *ty == "text")
        .map(|(name, _)| format!("coalesce(octet_length({alias}.{name}),0)<={TEXT_LIMIT}"))
        .collect::<Vec<_>>();
    if checks.is_empty() {
        "true".to_owned()
    } else {
        checks.join(" AND ")
    }
}

fn insert_sql(table: &LeafTable) -> String {
    let names = table.columns.iter().map(|(name, _)| *name).collect::<Vec<_>>().join(",");
    let input = table
        .columns
        .iter()
        .map(|(name, _)| format!("x.{name}"))
        .collect::<Vec<_>>()
        .join(",");
    let output = table
        .columns
        .iter()
        .map(|(name, _)| format!("'{name}',written.{name}"))
        .collect::<Vec<_>>()
        .join(",");
    let types = table
        .columns
        .iter()
        .map(|(name, ty)| format!("{name} {ty}"))
        .collect::<Vec<_>>()
        .join(",");
    let bounded = bound_text_fields(table, "written");
    format!(
        "INSERT INTO public.{table} AS written ({PREFIX},{names})
         SELECT $1::uuid,'binance:spot',$2::text,$3::uuid,{input}
         FROM jsonb_to_recordset($4::jsonb) AS x({types})
         RETURNING CASE WHEN
             (written.tenant_id,written.venue_id,written.account_scope,written.generation_id)
             = ($1::uuid,'binance:spot',$2::text,$3::uuid) AND {bounded}
             THEN jsonb_build_object({output}) ELSE 'null'::jsonb END",
        table = table.name
    )
}

/// Noise outside the exact four-column index prefix is not an input. For
/// each table the first surplus row is enough for the exact comparison to
/// fail. Over-width text produces a bounded JSON null diagnostic.
pub(super) fn namespace_expression() -> String {
    let tables = TABLES
        .iter()
        .enumerate()
        .map(|(index, table)| {
            let columns = table.columns.iter().map(|(name, _)| *name).collect::<Vec<_>>().join(",");
            let order = if table.key.is_empty() {
                String::new()
            } else {
                format!("ORDER BY {}", table.key.join(","))
            };
            let bounded = bound_text_fields(table, "leaf");
            format!(
                "(SELECT coalesce(jsonb_agg(CASE WHEN {bounded}
                                      THEN to_jsonb(leaf) ELSE 'null'::jsonb END),'[]'::jsonb)
                FROM (SELECT {columns} FROM public.{name}
                       WHERE {SCOPE_TEST} {order}
                       LIMIT (CASE WHEN ($4::bigint[])[{number}] BETWEEN 0 AND {LEAF_LIMIT}
                               THEN ($4::bigint[])[{number}]+1 ELSE 0 END)) AS leaf)",
                name = table.name,
                number = index + 1,
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    format!("jsonb_build_array({tables})")
}

/// Trusted scalar predicate to embed in the later CAS command. Bind scope to
/// $1/$2/$3, counts to $4, and the exact typed JSON matrix to $5. Use only in
/// a fresh SQL command after the ten INSERT statements. Both directions are
/// checked with bag semantics, including explicit JSON null fields.
pub(super) fn namespace_equality_sql() -> String {
    let read = namespace_expression();
    format!(
        "(WITH observed AS MATERIALIZED (SELECT {read} AS payload)
          SELECT cardinality($4::bigint[]) = {TABLE_COUNT}
              AND jsonb_array_length($5::jsonb) = {TABLE_COUNT}
              AND NOT EXISTS (
                  SELECT 1 FROM generate_series(0,{last}) AS leaf(slot)
                  WHERE ($4::bigint[])[leaf.slot+1] IS NULL
                     OR ($4::bigint[])[leaf.slot+1] NOT BETWEEN 0 AND {LEAF_LIMIT}
                     OR jsonb_array_length($5::jsonb -> leaf.slot)
                            IS DISTINCT FROM ($4::bigint[])[leaf.slot+1]
              )
              AND NOT EXISTS (
              SELECT 1 FROM observed o CROSS JOIN generate_series(0,{last}) AS leaf(slot)
              CROSS JOIN LATERAL (
                  (SELECT x.element FROM jsonb_array_elements(o.payload -> leaf.slot) AS x(element)
                   EXCEPT ALL
                   SELECT x.element FROM jsonb_array_elements($5::jsonb -> leaf.slot) AS x(element))
                  UNION ALL
                  (SELECT x.element FROM jsonb_array_elements($5::jsonb -> leaf.slot) AS x(element)
                   EXCEPT ALL
                   SELECT x.element FROM jsonb_array_elements(o.payload -> leaf.slot) AS x(element))
              ) AS differences
          ))",
        last = TABLE_COUNT - 1,
    )
}
