//! Order evidence writes. The accounting row is always the first write/lock.

use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{DateTime, NaiveDate, Utc};
use robson_domain::OrderSide;
use robson_exec::{
    venue::{OrderPhase, VenueId, VenueOrderRecord},
    venue_ledger::{
        merge_order, ClockEvidence, ClockVerdict, LedgerError, LegOrderKind, OrderKey, OrderLeg,
        OrderLink, OrderOwner, OrderWriteResult, SeenAt, StoredOrder,
    },
};
use rust_decimal::Decimal;
use sqlx::{postgres::PgRow, Connection, PgConnection, Postgres, QueryBuilder, Row, Transaction};
use uuid::Uuid;

use super::{backend, commit_error, LedgerConnection, PgVenueLedger};

pub(super) struct OrderContext {
    pub(super) order: Option<StoredOrder>,
    pub(super) leg: Option<OrderLeg>,
}

impl PgVenueLedger {
    /// Persist an actual venue order record, never an absence/unknown response
    /// or a placement's un-timed FULL fills. At most eight statements,
    /// including the pre-read. A replay without new evidence is one read
    /// and no bump.
    pub async fn record_order(
        &self,
        record: &VenueOrderRecord,
        link: &OrderLink,
        seen: SeenAt,
    ) -> Result<OrderWriteResult, LedgerError> {
        let committing = AtomicBool::new(false);
        self.bounded(async {
            let key = OrderKey::from_record(record,link)?;
            let position = match link.owner { OrderOwner::Leg {position_id,..} => Some(position_id), _ => None };
            let connection = self.pool.acquire().await.map_err(backend)?;
            let mut held = LedgerConnection::new(connection);
            let before = self.read_order_context(held.connection(),&key,position,false).await?;
            let proposed = merge_order(before.order.as_ref(),record,link,&seen,before.leg.as_ref())?;
            if !proposed.changed {
                held.finished=true;
                return Ok(OrderWriteResult {changed:false,anomalies:proposed.anomalies});
            }
            self.statement();
            let mut tx = held.connection().begin().await.map_err(backend)?;
            self.timeouts(&mut tx).await?;
            self.statement();
            let bumped=sqlx::query("UPDATE accounting_state SET version=version+1 WHERE tenant_id=$1 AND venue_id=$2")
                .bind(self.tenant_id).bind(key.venue.as_str()).execute(&mut *tx).await.map_err(backend)?;
            if bumped.rows_affected()!=1 {return Err(LedgerError::NoAccountingRow);}
            let locked=self.read_order_context(&mut tx,&key,position,true).await?;
            let merged=merge_order(locked.order.as_ref(),record,link,&seen,locked.leg.as_ref())?;
            if !merged.changed {
                self.statement();
                tx.rollback().await.map_err(backend)?;
                held.finished=true;
                return Ok(OrderWriteResult {changed:false,anomalies:merged.anomalies});
            }
            self.write_order(&mut tx,&merged.order,locked.order.is_some()).await?;
            if merged.consumed_delta>Decimal::ZERO || merged.set_cohort {
                let OrderOwner::Leg {position_id,..}=merged.order.owner else {
                    return Err(LedgerError::Invariant("unowned order changes a leg".into()));
                };
                self.statement();
                let updated=sqlx::query("UPDATE balance_legs SET consumed_cum=consumed_cum+$1,cohort_month=CASE WHEN $2 THEN COALESCE(cohort_month,decided_month) ELSE cohort_month END WHERE position_id=$3 AND tenant_id=$4 AND venue_id=$5")
                    .bind(merged.consumed_delta).bind(merged.set_cohort).bind(position_id).bind(self.tenant_id).bind(key.venue.as_str())
                    .execute(&mut *tx).await.map_err(order_error)?;
                if updated.rows_affected()!=1 {return Err(LedgerError::Invariant("order leg disappeared".into()));}
            }
            self.statement();
            committing.store(true,Ordering::Relaxed);
            tx.commit().await.map_err(commit_error)?;
            held.finished=true;
            Ok(OrderWriteResult {changed:true,anomalies:merged.anomalies})
        },&committing).await
    }

    /// Optional order and its incoming/already-linked leg in ONE statement.
    /// Numeric columns stay native NUMERIC/Decimal: no JSON/f64 intermediary.
    pub(super) async fn read_order_context(
        &self,
        connection: &mut PgConnection,
        key: &OrderKey,
        incoming_position: Option<Uuid>,
        locked: bool,
    ) -> Result<OrderContext, LedgerError> {
        let lock = if locked { " FOR UPDATE" } else { "" };
        let sql = format!(
            "SELECT o.*, l.position_id AS leg_position_id,l.tenant_id AS leg_tenant_id,
            l.venue_id AS leg_venue_id,l.account_scope AS leg_scope,l.symbol AS leg_symbol,
            l.admission_intent_id AS leg_admission,l.decided_month AS leg_month,
            l.cohort_month AS leg_cohort,l.consumed_cum AS leg_consumed,l.closed_at AS leg_closed
            FROM (VALUES(1)) AS seed(n)
            LEFT JOIN LATERAL (SELECT * FROM venue_orders WHERE venue_id=$1 AND account_scope=$2
                AND symbol=$3 AND exchange_order_id=$4{lock}) o ON true
            LEFT JOIN balance_legs l ON l.position_id=COALESCE($5,o.position_id)"
        );
        self.statement();
        let row = sqlx::query(&sql)
            .bind(key.venue.as_str())
            .bind(&key.account_scope)
            .bind(&key.symbol)
            .bind(key.exchange_order_id.to_string())
            .bind(incoming_position)
            .fetch_one(connection)
            .await
            .map_err(backend)?;
        Ok(OrderContext {
            order: decode_order(&row, self.tenant_id)?,
            leg: decode_leg(&row, self.tenant_id)?,
        })
    }

    pub(super) async fn write_order(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        order: &StoredOrder,
        exists: bool,
    ) -> Result<(), LedgerError> {
        let (owner, intent, position, kind) = owner_values(&order.owner);
        let (live_sample, live_wall, live_mono, live_verdict) =
            clock_values(order.live_seen.clock.as_ref());
        let terminal_at = order.terminal_seen.as_ref().map(|seen| seen.at);
        let (terminal_sample, terminal_wall, terminal_mono, terminal_verdict) =
            clock_values(order.terminal_seen.as_ref().and_then(|seen| seen.clock.as_ref()));
        self.statement();
        let result=if exists {
            // Immutable identity/live evidence, fills and cursor are not named.
            // The first terminal clock tuple is kept even if stored partial
            // invalid evidence decoded conservatively as None.
            sqlx::query("UPDATE venue_orders SET owner=$1,intent_id=$2,position_id=$3,kind=$4,
                last_client_order_id=$5,status=$6,phase=$7,executed_cum=$8,cumulative_quote=$9,
                last_checked_at=GREATEST(last_checked_at,$10),expiry_reason=$11,
                venue_time_ms=$12,venue_update_time_ms=$13,venue_transact_time_ms=$14,venue_working_time_ms=$15,
                terminal_seen_at=COALESCE(terminal_seen_at,$16),
                terminal_clock_sample_id=CASE WHEN terminal_seen_at IS NULL THEN $17 ELSE terminal_clock_sample_id END,
                terminal_clock_wall_delta_ms=CASE WHEN terminal_seen_at IS NULL THEN $18 ELSE terminal_clock_wall_delta_ms END,
                terminal_clock_mono_delta_ms=CASE WHEN terminal_seen_at IS NULL THEN $19 ELSE terminal_clock_mono_delta_ms END,
                terminal_clock_verdict=CASE WHEN terminal_seen_at IS NULL THEN $20 ELSE terminal_clock_verdict END
                WHERE venue_id=$21 AND account_scope=$22 AND symbol=$23 AND exchange_order_id=$24 AND tenant_id=$25")
                .bind(owner).bind(intent).bind(position).bind(kind).bind(&order.last_client_order_id)
                .bind(&order.status).bind(phase_text(order.phase)).bind(order.executed_cum).bind(order.cumulative_quote)
                .bind(order.last_checked_at).bind(&order.expiry_reason).bind(order.venue_time_ms).bind(order.venue_update_time_ms)
                .bind(order.venue_transact_time_ms).bind(order.venue_working_time_ms).bind(terminal_at)
                .bind(terminal_sample).bind(terminal_wall).bind(terminal_mono).bind(terminal_verdict)
                .bind(order.key.venue.as_str()).bind(&order.key.account_scope).bind(&order.key.symbol)
                .bind(order.key.exchange_order_id.to_string()).bind(self.tenant_id).execute(&mut **tx).await
        } else {
            let mut insert=QueryBuilder::<Postgres>::new("INSERT INTO venue_orders (
                venue_id,account_scope,symbol,exchange_order_id,tenant_id,client_order_id,last_client_order_id,
                owner,intent_id,position_id,kind,side,order_type,orig_qty,stop_price,status,phase,
                executed_cum,cumulative_quote,last_checked_at,live_seen_at,live_clock_sample_id,
                live_clock_wall_delta_ms,live_clock_mono_delta_ms,live_clock_verdict,terminal_seen_at,
                terminal_clock_sample_id,terminal_clock_wall_delta_ms,terminal_clock_mono_delta_ms,
                terminal_clock_verdict,expiry_reason,venue_time_ms,venue_update_time_ms,venue_transact_time_ms,venue_working_time_ms) ");
            insert.push_values(std::iter::once(order),|mut row,order| {
                row.push_bind(order.key.venue.as_str()).push_bind(&order.key.account_scope).push_bind(&order.key.symbol)
                    .push_bind(order.key.exchange_order_id.to_string()).push_bind(self.tenant_id)
                    .push_bind(&order.client_order_id).push_bind(&order.last_client_order_id)
                    .push_bind(owner).push_bind(intent).push_bind(position).push_bind(kind)
                    .push_bind(order.side.to_string()).push_bind(&order.order_type).push_bind(order.orig_qty).push_bind(order.stop_price)
                    .push_bind(&order.status).push_bind(phase_text(order.phase)).push_bind(order.executed_cum).push_bind(order.cumulative_quote)
                    .push_bind(order.last_checked_at).push_bind(order.live_seen.at).push_bind(live_sample).push_bind(live_wall)
                    .push_bind(live_mono).push_bind(live_verdict).push_bind(terminal_at).push_bind(terminal_sample)
                    .push_bind(terminal_wall).push_bind(terminal_mono).push_bind(terminal_verdict)
                    .push_bind(&order.expiry_reason).push_bind(order.venue_time_ms).push_bind(order.venue_update_time_ms)
                    .push_bind(order.venue_transact_time_ms).push_bind(order.venue_working_time_ms);
            });
            insert.build().execute(&mut **tx).await
        }.map_err(order_error)?;
        if result.rows_affected() != 1 {
            return Err(LedgerError::Invariant("order write did not affect one row".into()));
        }
        Ok(())
    }
}

fn owner_values(
    owner: &OrderOwner,
) -> (&'static str, Option<Uuid>, Option<Uuid>, Option<&'static str>) {
    match owner {
        OrderOwner::Leg { intent_id, position_id, kind } => {
            ("leg", Some(*intent_id), Some(*position_id), Some(kind.as_str()))
        },
        OrderOwner::Unauthored => ("unauthored", None, None, None),
        OrderOwner::Discovered => ("discovered", None, None, None),
    }
}

fn clock_values(
    clock: Option<&ClockEvidence>,
) -> (Option<Uuid>, Option<i64>, Option<i64>, Option<&'static str>) {
    clock.map_or((None, None, None, None), |clock| {
        (
            Some(clock.sample_id),
            Some(clock.wall_delta_ms),
            Some(clock.mono_delta_ms),
            Some(match clock.verdict {
                ClockVerdict::Valid => "valid",
                ClockVerdict::Invalid => "invalid",
            }),
        )
    })
}

fn phase_text(phase: OrderPhase) -> &'static str {
    match phase {
        OrderPhase::Live => "live",
        OrderPhase::Terminal => "terminal",
        OrderPhase::Unknown => "unknown",
    }
}

fn read_clock(row: &PgRow, prefix: &str) -> Result<Option<ClockEvidence>, LedgerError> {
    let sample: Option<Uuid> =
        row.try_get(format!("{prefix}_clock_sample_id").as_str()).map_err(backend)?;
    let wall: Option<i64> =
        row.try_get(format!("{prefix}_clock_wall_delta_ms").as_str()).map_err(backend)?;
    let mono: Option<i64> =
        row.try_get(format!("{prefix}_clock_mono_delta_ms").as_str()).map_err(backend)?;
    let verdict: Option<&str> =
        row.try_get(format!("{prefix}_clock_verdict").as_str()).map_err(backend)?;
    // Incomplete evidence is invalid, never upgraded to a valid clock sample.
    Ok(match (sample, wall, mono, verdict) {
        (
            Some(sample_id),
            Some(wall_delta_ms),
            Some(mono_delta_ms),
            Some(verdict @ ("valid" | "invalid")),
        ) => Some(ClockEvidence {
            sample_id,
            wall_delta_ms,
            mono_delta_ms,
            verdict: if verdict == "valid" {
                ClockVerdict::Valid
            } else {
                ClockVerdict::Invalid
            },
        }),
        _ => None,
    })
}

pub(super) fn decode_order(row: &PgRow, tenant: Uuid) -> Result<Option<StoredOrder>, LedgerError> {
    let id: Option<String> = row.try_get("exchange_order_id").map_err(backend)?;
    let Some(id) = id else {
        return Ok(None);
    };
    if row.try_get::<Uuid, _>("tenant_id").map_err(backend)? != tenant {
        return Err(LedgerError::IdentityConflict);
    }
    let kind = || -> Result<LegOrderKind, LedgerError> {
        match row.try_get::<&str, _>("kind").map_err(backend)? {
            "entry" => Ok(LegOrderKind::Entry),
            "stop" => Ok(LegOrderKind::Stop),
            "sell" => Ok(LegOrderKind::Sell),
            _ => Err(LedgerError::Invariant("invalid stored leg kind".into())),
        }
    };
    let owner = match row.try_get::<&str, _>("owner").map_err(backend)? {
        "leg" => OrderOwner::Leg {
            intent_id: row.try_get("intent_id").map_err(backend)?,
            position_id: row.try_get("position_id").map_err(backend)?,
            kind: kind()?,
        },
        "unauthored" => OrderOwner::Unauthored,
        "discovered" => OrderOwner::Discovered,
        _ => return Err(LedgerError::Invariant("invalid stored order owner".into())),
    };
    let side = match row.try_get::<&str, _>("side").map_err(backend)? {
        "BUY" => OrderSide::Buy,
        "SELL" => OrderSide::Sell,
        _ => return Err(LedgerError::Invariant("invalid stored order side".into())),
    };
    let phase = match row.try_get::<&str, _>("phase").map_err(backend)? {
        "live" => OrderPhase::Live,
        "terminal" => OrderPhase::Terminal,
        "unknown" => OrderPhase::Unknown,
        _ => return Err(LedgerError::Invariant("invalid stored order phase".into())),
    };
    let terminal_at: Option<DateTime<Utc>> = row.try_get("terminal_seen_at").map_err(backend)?;
    Ok(Some(StoredOrder {
        key: OrderKey {
            venue: VenueId::new(row.try_get::<String, _>("venue_id").map_err(backend)?),
            account_scope: row.try_get("account_scope").map_err(backend)?,
            symbol: row.try_get("symbol").map_err(backend)?,
            exchange_order_id: id.parse()?,
        },
        owner,
        client_order_id: row.try_get("client_order_id").map_err(backend)?,
        last_client_order_id: row.try_get("last_client_order_id").map_err(backend)?,
        side,
        order_type: row.try_get("order_type").map_err(backend)?,
        orig_qty: row.try_get("orig_qty").map_err(backend)?,
        stop_price: row.try_get("stop_price").map_err(backend)?,
        status: row.try_get("status").map_err(backend)?,
        phase,
        executed_cum: row.try_get("executed_cum").map_err(backend)?,
        cumulative_quote: row.try_get("cumulative_quote").map_err(backend)?,
        fills_qty: row.try_get("fills_qty").map_err(backend)?,
        trades_cursor: row.try_get("trades_cursor").map_err(backend)?,
        last_checked_at: row.try_get("last_checked_at").map_err(backend)?,
        live_seen: SeenAt {
            at: row.try_get("live_seen_at").map_err(backend)?,
            clock: read_clock(row, "live")?,
        },
        terminal_seen: terminal_at
            .map(|at| read_clock(row, "terminal").map(|clock| SeenAt { at, clock }))
            .transpose()?,
        expiry_reason: row.try_get("expiry_reason").map_err(backend)?,
        venue_time_ms: row.try_get("venue_time_ms").map_err(backend)?,
        venue_update_time_ms: row.try_get("venue_update_time_ms").map_err(backend)?,
        venue_transact_time_ms: row.try_get("venue_transact_time_ms").map_err(backend)?,
        venue_working_time_ms: row.try_get("venue_working_time_ms").map_err(backend)?,
    }))
}

fn decode_leg(row: &PgRow, tenant: Uuid) -> Result<Option<OrderLeg>, LedgerError> {
    let id: Option<Uuid> = row.try_get("leg_position_id").map_err(backend)?;
    let Some(position_id) = id else {
        return Ok(None);
    };
    if row.try_get::<Uuid, _>("leg_tenant_id").map_err(backend)? != tenant {
        return Err(LedgerError::IdentityConflict);
    }
    Ok(Some(OrderLeg {
        position_id,
        venue: VenueId::new(row.try_get::<String, _>("leg_venue_id").map_err(backend)?),
        account_scope: row.try_get("leg_scope").map_err(backend)?,
        symbol: row.try_get("leg_symbol").map_err(backend)?,
        admission_intent_id: row.try_get("leg_admission").map_err(backend)?,
        decided_month: row.try_get::<NaiveDate, _>("leg_month").map_err(backend)?,
        cohort_month: row.try_get("leg_cohort").map_err(backend)?,
        consumed_cum: row.try_get("leg_consumed").map_err(backend)?,
        closed_at: row.try_get("leg_closed").map_err(backend)?,
    }))
}

pub(super) fn order_error(error: sqlx::Error) -> LedgerError {
    if let sqlx::Error::Database(database) = &error {
        if (database.code().as_deref() == Some("23505")
            && matches!(
                database.constraint(),
                Some("idx_venue_orders_intent" | "venue_orders_pkey")
            ))
            || (database.code().as_deref() == Some("23503")
                && database.constraint() == Some("venue_orders_leg"))
        {
            return LedgerError::IdentityConflict;
        }
        if database.code().as_deref() == Some("23514")
            && database.constraint() == Some("balance_legs_consumed_cum_check")
        {
            return LedgerError::Unrepresentable;
        }
    }
    backend(error)
}
