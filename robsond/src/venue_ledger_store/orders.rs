//! Order evidence writes. The accounting row is always the first write/lock.

use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{DateTime, NaiveDate, Utc};
use robson_domain::OrderSide;
use robson_exec::{
    venue::{OrderPhase, VenueId, VenueOrderRecord},
    venue_ledger::{
        merge_order, AdmissionTerms, ClockEvidence, ClockVerdict, LedgerError, LegOrderKind,
        LegPartition, OrderKey, OrderLeg, OrderLink, OrderOwner, OrderWriteResult, SeenAt,
        StoredOrder,
    },
};
use rust_decimal::Decimal;
use sqlx::{postgres::PgRow, Connection, PgConnection, Postgres, Row, Transaction};
use tracing::Instrument as _;
use uuid::Uuid;

use super::{backend, commit_error, LedgerConnection, PgVenueLedger};
use crate::spot_order_set_store::JournalGeneration;

/// One statement from four bounded sections; see order_source.sql.
pub(super) const ORDER_PROJECTION: &str = concat!(
    include_str!("../spot_order_set_store/order_source.sql"),
    include_str!("../spot_order_set_store/order_sell.sql"),
    include_str!("../spot_order_set_store/order_cancel.sql"),
    include_str!("../spot_order_set_store/order_write.sql"),
);

pub(super) struct OrderContext {
    pub(super) order: Option<StoredOrder>,
    pub(super) leg: Option<OrderLeg>,
}

/// Privately constructed coordinate of one opted, leg-owned, changed order
/// write whose COMMIT positively succeeded. It is not an observation ACK.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) struct OrderCommit {
    key: OrderKey,
    owner: OrderOwner,
    generation: JournalGeneration,
    executed_cum: Decimal,
    accounting_version: i64,
}

#[cfg(test)]
impl OrderCommit {
    pub(super) fn key(&self) -> &OrderKey {
        &self.key
    }

    pub(super) fn owner(&self) -> &OrderOwner {
        &self.owner
    }

    pub(super) fn generation(&self) -> &JournalGeneration {
        &self.generation
    }

    pub(super) fn executed_cum(&self) -> Decimal {
        self.executed_cum
    }

    pub(super) fn accounting_version(&self) -> i64 {
        self.accounting_version
    }
}

use crate::venue_clock_store::codec::CaptureBind;
pub(super) use crate::venue_clock_store::codec::ORDER_COLUMNS;

fn exact_capture_error(_: crate::venue_clock_store::codec::CodecError) -> LedgerError {
    LedgerError::InvalidEvidence("malformed exact clock capture".into())
}

impl PgVenueLedger {
    /// Persist an actual venue order record, never an absence/unknown response
    /// or a placement's un-timed FULL fills. At most nine statements,
    /// including the pre-read, which already runs under the server bounds and
    /// the certificate's key. A replay without new evidence is BEGIN, the
    /// bounds, the key, one read and ROLLBACK, with no bump. An opted changed
    /// write adds one exact membership statement.
    pub async fn record_order(
        &self,
        record: &VenueOrderRecord,
        link: &OrderLink,
        seen: SeenAt,
    ) -> Result<OrderWriteResult, LedgerError> {
        self.record_order_core::<false>(record, link, seen)
            .await
            .map(|(result, _)| result)
    }

    /// Test-only observer: the same write, plus the commit coordinate of an
    /// opted, leg-owned, changed write whose COMMIT positively succeeded.
    #[cfg(test)]
    pub(super) async fn record_order_observed(
        &self,
        record: &VenueOrderRecord,
        link: &OrderLink,
        seen: SeenAt,
    ) -> Result<(OrderWriteResult, Option<OrderCommit>), LedgerError> {
        self.record_order_core::<true>(record, link, seen).await
    }

    // Ordinary traffic passes CAPTURE=false and never builds a coordinate.
    async fn record_order_core<const CAPTURE: bool>(
        &self,
        record: &VenueOrderRecord,
        link: &OrderLink,
        seen: SeenAt,
    ) -> Result<(OrderWriteResult, Option<OrderCommit>), LedgerError> {
        let committing = AtomicBool::new(false);
        self.bounded(async {
            let key = OrderKey::from_record(record,link)?;
            if let Some(generation)=&self.order_projection {
                if !generation.is_valid() || generation.tenant_id!=self.tenant_id
                    || key.venue.as_str()!=VenueId::BINANCE_SPOT || generation.account_scope!=key.account_scope {
                    return Err(LedgerError::Invariant("order projection scope mismatch".into()));
                }
            }
            let position = match link.owner { OrderOwner::Leg {position_id,..} => Some(position_id), _ => None };
            let connection = self.pool.acquire().await.map_err(backend)?;
            let mut held = LedgerConnection::new(connection);
            self.statement();
            let mut tx = held.connection().begin().await.map_err(backend)?;
            self.timeouts(&mut tx).await?;
            // The certificate's shared key right after the bounds, before any read
            // or row lock (8m PLAN-r4 §6.7).
            self.statement();
            let _key = robson_eventlog::certificate::participate(&mut tx).await.map_err(backend)?;
            let before = self.read_order_context(&mut tx,&key,position,false).await?;
            let proposed = merge_order(before.order.as_ref(),record,link,&seen,before.leg.as_ref())?;
            if !proposed.changed {
                self.statement();
                tx.rollback().await.map_err(backend)?;
                held.finished=true;
                return Ok((OrderWriteResult {changed:false,anomalies:proposed.anomalies},None));
            }
            self.statement();
            // The opted branch only adds RETURNING; legacy keeps its statement.
            let version=if self.order_projection.is_some() {
                let version=sqlx::query_scalar::<_,i64>("UPDATE accounting_state SET version=version+1 WHERE tenant_id=$1 AND venue_id=$2 RETURNING version")
                    .bind(self.tenant_id).bind(key.venue.as_str()).fetch_optional(&mut *tx).await.map_err(backend)?;
                Some(version.ok_or(LedgerError::NoAccountingRow)?)
            } else {
                let bumped=sqlx::query("UPDATE accounting_state SET version=version+1 WHERE tenant_id=$1 AND venue_id=$2")
                    .bind(self.tenant_id).bind(key.venue.as_str()).execute(&mut *tx).await.map_err(backend)?;
                if bumped.rows_affected()!=1 {return Err(LedgerError::NoAccountingRow);}
                None
            };
            let locked=self.read_order_context(&mut tx,&key,position,true).await?;
            let merged=merge_order(locked.order.as_ref(),record,link,&seen,locked.leg.as_ref())?;
            if !merged.changed {
                self.statement();
                tx.rollback().await.map_err(backend)?;
                held.finished=true;
                return Ok((OrderWriteResult {changed:false,anomalies:merged.anomalies},None));
            }
            self.write_order(&mut tx,&merged.order,locked.order.is_some()).await?;
            if merged.consumed_delta>Decimal::ZERO || merged.set_cohort || merged.set_partition.is_some() {
                let OrderOwner::Leg {position_id,..}=merged.order.owner else {
                    return Err(LedgerError::Invariant("unowned order changes a leg".into()));
                };
                self.update_leg(&mut tx,position_id,&key.venue,merged.consumed_delta,merged.set_cohort,merged.set_partition.as_ref()).await?;
            }
            if let (Some(generation),Some(version))=(&self.order_projection,version) {
                // The locked pre-state fingerprint lets the statement verify the
                // exact prior members. A refusal returns unfinished, so the guard
                // retires the connection and rolls back every native write.
                let prior=locked.order.as_ref();
                let (owner,intent,prior_position,kind)=prior.map_or((None,None,None,None),|order| {
                    let (owner,intent,position,kind)=owner_values(&order.owner);
                    (Some(owner),intent,position,kind)
                });
                self.statement();
                let advanced=sqlx::query_scalar::<_,i64>(ORDER_PROJECTION)
                    .bind(generation.tenant_id).bind(&generation.account_scope).bind(generation.generation_id)
                    .bind(generation.process_epoch).bind(generation.source_audit_id).bind(version)
                    .bind(&key.symbol).bind(key.exchange_order_id.to_string())
                    .bind(prior.is_some()).bind(owner).bind(intent).bind(prior_position).bind(kind)
                    .bind(prior.map(|order| phase_text(order.phase))).bind(prior.map(|order| order.status.as_str()))
                    .fetch_optional(&mut *tx)
                    .instrument(tracing::info_span!(target: "robson_operation", "operation_io", io_kind = 1_u64))
                    .await.map_err(backend)?;
                if advanced!=Some(version) {
                    return Err(LedgerError::Invariant("order projection membership refused".into()));
                }
            }
            self.statement();
            committing.store(true,Ordering::Relaxed);
            tx.commit().await.map_err(commit_error)?;
            held.finished=true;
            // Built only after a confirmed COMMIT from values already held:
            // no statement, and never for an unowned or unprojected write.
            let commit = match (CAPTURE, &self.order_projection, version, merged.order.owner) {
                (true, Some(generation), Some(accounting_version), owner @ OrderOwner::Leg { .. }) => {
                    Some(OrderCommit {
                        key,
                        owner,
                        generation: generation.clone(),
                        executed_cum: merged.order.executed_cum,
                        accounting_version,
                    })
                },
                _ => None,
            };
            Ok((OrderWriteResult { changed: true, anomalies: merged.anomalies }, commit))
        },&committing).await
    }

    /// The order write's one leg statement. Only a partition first set is
    /// guarded by `gross_qty IS NULL`; consumed and cohort updates are not.
    pub(super) async fn update_leg(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        position_id: Uuid,
        venue: &VenueId,
        consumed_delta: Decimal,
        set_cohort: bool,
        partition: Option<&LegPartition>,
    ) -> Result<(), LedgerError> {
        self.statement();
        let updated = sqlx::query(
            "UPDATE balance_legs SET consumed_cum=consumed_cum+$1,
            cohort_month=CASE WHEN $2 THEN COALESCE(cohort_month,decided_month) ELSE cohort_month END,
            gross_qty=CASE WHEN $6 THEN $7 ELSE gross_qty END,
            acquisition_cost=CASE WHEN $6 THEN $8 ELSE acquisition_cost END,
            base_commission=CASE WHEN $6 THEN $9 ELSE base_commission END,
            executable_qty=CASE WHEN $6 THEN $10 ELSE executable_qty END
            WHERE position_id=$3 AND tenant_id=$4 AND venue_id=$5 AND (NOT $6 OR gross_qty IS NULL)",
        )
        .bind(consumed_delta)
        .bind(set_cohort)
        .bind(position_id)
        .bind(self.tenant_id)
        .bind(venue.as_str())
        .bind(partition.is_some())
        .bind(partition.map(|p| p.gross))
        .bind(partition.map(|p| p.cost))
        .bind(partition.map(|p| p.base_fee))
        .bind(partition.map(|p| p.executable))
        .execute(&mut **tx)
        .await
        .map_err(order_error)?;
        if updated.rows_affected() != 1 {
            return Err(LedgerError::Invariant(
                if partition.is_some() {
                    "order leg disappeared or its partition is already set"
                } else {
                    "order leg disappeared"
                }
                .into(),
            ));
        }
        Ok(())
    }

    /// Optional order, its incoming/already-linked leg and that leg's current
    /// admission terms in ONE statement. Numeric columns stay native
    /// NUMERIC/Decimal: no JSON/f64 intermediary.
    pub(super) async fn read_order_context(
        &self,
        connection: &mut PgConnection,
        key: &OrderKey,
        incoming_position: Option<Uuid>,
        locked: bool,
    ) -> Result<OrderContext, LedgerError> {
        let lock = if locked { " FOR UPDATE" } else { "" };
        let sql = format!(
            "SELECT {ORDER_COLUMNS}, l.position_id AS leg_position_id,l.tenant_id AS leg_tenant_id,
            l.venue_id AS leg_venue_id,l.account_scope AS leg_scope,l.symbol AS leg_symbol,
            l.admission_intent_id AS leg_admission,l.decided_month AS leg_month,
            l.cohort_month AS leg_cohort,l.consumed_cum AS leg_consumed,l.closed_at AS leg_closed,
            l.gross_qty AS leg_gross,l.acquisition_cost AS leg_cost,l.base_commission AS leg_fee,
            l.base_commission_final AS leg_fee_final,l.executable_qty AS leg_executable,
            a.taker_bps AS leg_taker_bps,a.lot_step AS leg_lot_step
            FROM (VALUES(1)) AS seed(n)
            LEFT JOIN LATERAL (SELECT * FROM venue_orders WHERE venue_id=$1 AND account_scope=$2
                AND symbol=$3 AND exchange_order_id=$4{lock}) o ON true
            LEFT JOIN balance_legs l ON l.position_id=COALESCE($5,o.position_id)
            LEFT JOIN balance_leg_admissions a ON a.entry_intent_id=l.admission_intent_id
                AND a.position_id=l.position_id AND a.superseded_at IS NULL"
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
        let live_capture =
            CaptureBind::new(order.live_seen.capture.as_ref()).map_err(exact_capture_error)?;
        let terminal_capture =
            CaptureBind::new(order.terminal_seen.as_ref().and_then(|seen| seen.capture.as_ref()))
                .map_err(exact_capture_error)?;
        self.statement();
        let result=if exists {
            // Immutable identity/live evidence, fills and cursor are not named.
            // The first terminal clock tuple is kept even if stored partial
            // invalid evidence decoded conservatively as None.
            let query = sqlx::query("UPDATE venue_orders SET owner=$1,intent_id=$2,position_id=$3,kind=$4,
                last_client_order_id=$5,status=$6,phase=$7,executed_cum=$8,cumulative_quote=$9,
                last_checked_at=GREATEST(last_checked_at,$10),expiry_reason=$11,
                venue_time_ms=$12,venue_update_time_ms=$13,venue_transact_time_ms=$14,venue_working_time_ms=$15,
                terminal_seen_at=COALESCE(terminal_seen_at,$16),
                terminal_clock_sample_id=CASE WHEN terminal_seen_at IS NULL THEN $17 ELSE terminal_clock_sample_id END,
                terminal_clock_wall_delta_ms=CASE WHEN terminal_seen_at IS NULL THEN $18 ELSE terminal_clock_wall_delta_ms END,
                terminal_clock_mono_delta_ms=CASE WHEN terminal_seen_at IS NULL THEN $19 ELSE terminal_clock_mono_delta_ms END,
                terminal_clock_verdict=CASE WHEN terminal_seen_at IS NULL THEN $20 ELSE terminal_clock_verdict END,
                terminal_capture_epoch=CASE WHEN terminal_seen_at IS NULL THEN $26 ELSE terminal_capture_epoch END,
                terminal_wall_seconds=CASE WHEN terminal_seen_at IS NULL THEN $27 ELSE terminal_wall_seconds END,
                terminal_wall_nanoseconds=CASE WHEN terminal_seen_at IS NULL THEN $28 ELSE terminal_wall_nanoseconds END,
                terminal_tick_before=CASE WHEN terminal_seen_at IS NULL THEN $29 ELSE terminal_tick_before END,
                terminal_tick_after=CASE WHEN terminal_seen_at IS NULL THEN $30 ELSE terminal_tick_after END,
                terminal_capture_domain=CASE WHEN terminal_seen_at IS NULL THEN
                    CASE WHEN $31::text IS NULL THEN NULL::budget_clock_domain_v38
                    ELSE ROW($31,$32,$33,$34)::budget_clock_domain_v38 END ELSE terminal_capture_domain END,
                quote_floor=$35
                WHERE venue_id=$21 AND account_scope=$22 AND symbol=$23 AND exchange_order_id=$24 AND tenant_id=$25")
                .bind(owner).bind(intent).bind(position).bind(kind).bind(&order.last_client_order_id)
                .bind(&order.status).bind(phase_text(order.phase)).bind(order.executed_cum).bind(order.cumulative_quote)
                .bind(order.last_checked_at).bind(&order.expiry_reason).bind(order.venue_time_ms).bind(order.venue_update_time_ms)
                .bind(order.venue_transact_time_ms).bind(order.venue_working_time_ms).bind(terminal_at)
                .bind(terminal_sample).bind(terminal_wall).bind(terminal_mono).bind(terminal_verdict)
                .bind(order.key.venue.as_str()).bind(&order.key.account_scope).bind(&order.key.symbol)
                .bind(order.key.exchange_order_id.to_string()).bind(self.tenant_id);
            terminal_capture.bind(query).bind(order.quote_floor).execute(&mut **tx).await
        } else {
            let query=sqlx::query("INSERT INTO venue_orders (
                venue_id,account_scope,symbol,exchange_order_id,tenant_id,client_order_id,
                last_client_order_id,owner,intent_id,position_id,kind,side,
                order_type,orig_qty,stop_price,status,phase,executed_cum,
                cumulative_quote,last_checked_at,live_seen_at,live_clock_sample_id,live_clock_wall_delta_ms,live_clock_mono_delta_ms,
                live_clock_verdict,terminal_seen_at,terminal_clock_sample_id,terminal_clock_wall_delta_ms,terminal_clock_mono_delta_ms,terminal_clock_verdict,
                expiry_reason,venue_time_ms,venue_update_time_ms,venue_transact_time_ms,venue_working_time_ms,live_capture_epoch,
                live_wall_seconds,live_wall_nanoseconds,live_tick_before,live_tick_after,live_capture_domain,terminal_capture_epoch,
                terminal_wall_seconds,terminal_wall_nanoseconds,terminal_tick_before,terminal_tick_after,terminal_capture_domain,quote_floor)
                VALUES ($1,$2,$3,$4,$5,$6,
                $7,$8,$9,$10,$11,$12,
                $13,$14,$15,$16,$17,$18,
                $19,$20,$21,$22,$23,$24,
                $25,$26,$27,$28,$29,$30,
                $31,$32,$33,$34,$35,$36,
                $37,$38,$39,$40,CASE WHEN $41::text IS NULL THEN NULL::budget_clock_domain_v38 ELSE ROW($41,$42,$43,$44)::budget_clock_domain_v38 END,$45,
                $46,$47,$48,$49,CASE WHEN $50::text IS NULL THEN NULL::budget_clock_domain_v38 ELSE ROW($50,$51,$52,$53)::budget_clock_domain_v38 END,$54)")
                .bind(order.key.venue.as_str()).bind(&order.key.account_scope).bind(&order.key.symbol)
                    .bind(order.key.exchange_order_id.to_string()).bind(self.tenant_id)
                    .bind(&order.client_order_id).bind(&order.last_client_order_id)
                    .bind(owner).bind(intent).bind(position).bind(kind)
                    .bind(order.side.to_string()).bind(&order.order_type).bind(order.orig_qty).bind(order.stop_price)
                    .bind(&order.status).bind(phase_text(order.phase)).bind(order.executed_cum).bind(order.cumulative_quote)
                    .bind(order.last_checked_at).bind(order.live_seen.at).bind(live_sample).bind(live_wall)
                    .bind(live_mono).bind(live_verdict).bind(terminal_at).bind(terminal_sample)
                    .bind(terminal_wall).bind(terminal_mono).bind(terminal_verdict)
                    .bind(&order.expiry_reason).bind(order.venue_time_ms).bind(order.venue_update_time_ms)
                    .bind(order.venue_transact_time_ms).bind(order.venue_working_time_ms);
            terminal_capture.bind(live_capture.bind(query)).bind(order.quote_floor).execute(&mut **tx).await
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
    // Decode even without the legacy instant: partial/untimed raw evidence
    // must not collapse to absence. The schema's whole check is independent.
    let terminal_capture =
        crate::venue_clock_store::codec::capture(row, "terminal").map_err(exact_capture_error)?;
    if terminal_at.is_none() && terminal_capture.is_some() {
        return Err(LedgerError::InvalidEvidence(
            "terminal capture without terminal instant".into(),
        ));
    }
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
        quote_floor: row.try_get("quote_floor").map_err(backend)?,
        fills_qty: row.try_get("fills_qty").map_err(backend)?,
        trades_cursor: row.try_get("trades_cursor").map_err(backend)?,
        last_checked_at: row.try_get("last_checked_at").map_err(backend)?,
        live_seen: SeenAt {
            at: row.try_get("live_seen_at").map_err(backend)?,
            clock: read_clock(row, "live")?,
            capture: crate::venue_clock_store::codec::capture(row, "live")
                .map_err(exact_capture_error)?,
        },
        terminal_seen: terminal_at
            .map(|at| -> Result<SeenAt, LedgerError> {
                Ok(SeenAt {
                    at,
                    clock: read_clock(row, "terminal")?,
                    capture: terminal_capture,
                })
            })
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
    let amount = |column: &str| row.try_get::<Option<Decimal>, _>(column).map_err(backend);
    let partition = match (
        amount("leg_gross")?,
        amount("leg_cost")?,
        amount("leg_fee")?,
        amount("leg_executable")?,
    ) {
        (Some(gross), Some(cost), Some(base_fee), Some(executable)) => Some(LegPartition {
            gross,
            cost,
            base_fee,
            base_fee_final: row.try_get("leg_fee_final").map_err(backend)?,
            executable,
        }),
        (None, None, None, None) => None,
        _ => return Err(LedgerError::Invariant("partial leg partition".into())),
    };
    let terms = match (amount("leg_taker_bps")?, amount("leg_lot_step")?) {
        (Some(taker_bps), Some(lot_step)) => Some(AdmissionTerms { taker_bps, lot_step }),
        _ => None,
    };
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
        partition,
        terms,
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
            && database.constraint() == Some("venue_orders_quote_floor_bound")
        {
            return LedgerError::TradeConflict;
        }
        if database.code().as_deref() == Some("23514")
            && database.constraint() == Some("balance_legs_consumed_cum_check")
        {
            return LedgerError::Unrepresentable;
        }
    }
    backend(error)
}

#[cfg(test)]
mod pg_tests;
