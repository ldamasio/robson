//! Trade-page writes use one accounting transaction and one bulk fill insert.
//! Native NUMERIC fields never pass through JSON or floating point.
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{DateTime, Utc};
use robson_exec::{
    venue::{TradePage, VenueId},
    venue_ledger::{
        plan_trades, LedgerError, OrderKey, OrderOwner, StoredOrder, StoredTrade, TradeWriteResult,
        VenueIntId,
    },
};
use rust_decimal::Decimal;
use sqlx::{postgres::PgRow, Connection, PgConnection, Postgres, Row, Transaction};
use tracing::Instrument as _;

use super::{
    backend, commit_error,
    orders::{decode_order, order_error, ORDER_COLUMNS},
    LedgerConnection, PgVenueLedger,
};

const FILL_COLUMNS: &str = "f.trade_id AS fill_trade_id,f.exchange_order_id AS fill_order_id,
    f.tenant_id AS fill_tenant_id,f.price AS fill_price,f.qty AS fill_qty,
    f.quote_qty AS fill_quote_qty,f.commission AS fill_commission,
    f.commission_asset AS fill_asset,f.trade_time AS fill_time,f.recorded_at AS fill_recorded_at";
pub(super) const TRADE_GENERATION: &str =
    include_str!("../spot_order_set_store/trade_generation.sql");

impl PgVenueLedger {
    /// One page, at most 1,000 trades and eleven statements, the pre-read
    /// already under the server bounds and the certificate's key. A duplicate
    /// page without cursor progress is BEGIN, the bounds, the key, one read and
    /// ROLLBACK: no bump, no rotation update.
    /// An opted changed page adds one exact generation statement, and new
    /// fills of an order with no governed leg add one invalidation statement.
    pub async fn record_trades(
        &self,
        key: &OrderKey,
        page: &TradePage,
        recorded_at: DateTime<Utc>,
    ) -> Result<TradeWriteResult, LedgerError> {
        let committing = AtomicBool::new(false);
        self.bounded(async {
            if page.trades.len()>1_000 {
                return Err(LedgerError::InvalidEvidence("trade page exceeds 1000 rows".into()));
            }
            let ids: Vec<String>=page.trades.iter().map(|fill| fill.trade_id.parse::<VenueIntId>().map(|id| id.to_string())).collect::<Result<_,_>>()?;
            if let Some(generation)=&self.trade_projection {
                if !generation.is_valid() || generation.tenant_id!=self.tenant_id
                    || key.venue.as_str()!=VenueId::BINANCE_SPOT || generation.account_scope!=key.account_scope {
                    return Err(LedgerError::Invariant("trade projection scope mismatch".into()));
                }
            }
            let connection=self.pool.acquire().await.map_err(backend)?;
            let mut held=LedgerConnection::new(connection);
            self.statement();
            let mut tx=held.connection().begin().await.map_err(backend)?;
            self.timeouts(&mut tx).await?;
            // The certificate's shared key right after the bounds, before any read
            // or row lock (8m PLAN-r4 §6.7).
            self.statement();
            let _key = robson_eventlog::certificate::participate(&mut tx).await.map_err(backend)?;
            let (before,existing)=self.read_trade_context(&mut tx,key,&ids).await?;
            let proposed=plan_trades(&before,self.tenant_id,page,&existing,recorded_at)?;
            if !proposed.changed {
                self.statement();
                tx.rollback().await.map_err(backend)?;
                held.finished=true;
                return Ok(TradeWriteResult {changed:false});
            }
            self.statement();
            // The opted branch only adds RETURNING; legacy keeps its statement.
            let version=if self.trade_projection.is_some() {
                let version=sqlx::query_scalar::<_,i64>("UPDATE accounting_state SET version=version+1 WHERE tenant_id=$1 AND venue_id=$2 RETURNING version")
                    .bind(self.tenant_id).bind(key.venue.as_str()).fetch_optional(&mut *tx).await.map_err(backend)?;
                Some(version.ok_or(LedgerError::NoAccountingRow)?)
            } else {
                let bump=sqlx::query("UPDATE accounting_state SET version=version+1 WHERE tenant_id=$1 AND venue_id=$2")
                    .bind(self.tenant_id).bind(key.venue.as_str()).execute(&mut *tx).await.map_err(backend)?;
                if bump.rows_affected()!=1 {return Err(LedgerError::NoAccountingRow);}
                None
            };
            let locked=self.read_order_context(&mut tx,key,None,true).await?.order.ok_or(LedgerError::UnknownOrder)?;
            let existing=self.read_existing_fills(&mut tx,key,&ids).await?;
            let planned=plan_trades(&locked,self.tenant_id,page,&existing,recorded_at)?;
            if !planned.changed {
                self.statement();
                tx.rollback().await.map_err(backend)?;
                held.finished=true;
                return Ok(TradeWriteResult {changed:false});
            }
            if !planned.new_fills.is_empty() {
                self.insert_fills(&mut tx,key,&planned.new_fills).await?;
                if !matches!(planned.order.owner,OrderOwner::Leg {..}) {
                    self.invalidate_unattributed(&mut tx,key,&planned.new_fills).await?;
                }
            }
            self.statement();
            let updated=sqlx::query("UPDATE venue_orders SET fills_qty=$1,executed_cum=$2,trades_cursor=$3,
                last_checked_at=GREATEST(last_checked_at,$4),cumulative_quote=$5,quote_floor=$11 WHERE venue_id=$6 AND account_scope=$7
                AND symbol=$8 AND exchange_order_id=$9 AND tenant_id=$10")
                .bind(planned.order.fills_qty).bind(planned.order.executed_cum).bind(planned.order.trades_cursor)
                .bind(planned.order.last_checked_at).bind(planned.order.cumulative_quote).bind(key.venue.as_str()).bind(&key.account_scope)
                .bind(&key.symbol).bind(key.exchange_order_id.to_string()).bind(self.tenant_id).bind(planned.order.quote_floor)
                .execute(&mut *tx).await.map_err(backend)?;
            if updated.rows_affected()!=1 {return Err(LedgerError::Invariant("trade order disappeared".into()));}
            if planned.consumed_delta>Decimal::ZERO {
                let OrderOwner::Leg {position_id,..}=planned.order.owner else {
                    return Err(LedgerError::Invariant("unowned trades consume a leg".into()));
                };
                self.statement();
                let leg=sqlx::query("UPDATE balance_legs SET consumed_cum=consumed_cum+$1
                    WHERE position_id=$2 AND tenant_id=$3 AND venue_id=$4")
                    .bind(planned.consumed_delta).bind(position_id).bind(self.tenant_id).bind(key.venue.as_str())
                    .execute(&mut *tx).await.map_err(order_error)?;
                if leg.rows_affected()!=1 {return Err(LedgerError::Invariant("trade leg disappeared".into()));}
            }
            if let (Some(generation),Some(version))=(&self.trade_projection,version) {
                // Membership inputs are untouched by trade pages: one exact
                // version CAS. A refusal returns unfinished, so the guard
                // retires the connection and rolls back every native write.
                self.statement();
                let advanced=sqlx::query_scalar::<_,i64>(TRADE_GENERATION)
                    .bind(generation.tenant_id).bind(&generation.account_scope).bind(generation.generation_id)
                    .bind(generation.process_epoch).bind(generation.source_audit_id).bind(version)
                    .fetch_optional(&mut *tx)
                    .instrument(tracing::info_span!(target: "robson_operation", "operation_io", io_kind = 1_u64))
                    .await.map_err(backend)?;
                if advanced!=Some(version) {
                    return Err(LedgerError::Invariant("trade projection generation refused".into()));
                }
            }
            self.statement();
            committing.store(true,Ordering::Relaxed);
            tx.commit().await.map_err(commit_error)?;
            held.finished=true;
            Ok(TradeWriteResult {changed:true})
        },&committing).await
    }

    /// Rotation metadata only: one bounded transaction, no accounting bump.
    /// The server bounds come first, as for every ledger write, so a lock
    /// wait ends on the server instead of outliving the client's deadline. A
    /// missing COMMIT answer is uncertain because the update may have landed.
    pub async fn touch_checked(
        &self,
        key: &OrderKey,
        at: DateTime<Utc>,
    ) -> Result<(), LedgerError> {
        let committing = AtomicBool::new(false);
        self.bounded(async {
            let connection=self.pool.acquire().await.map_err(backend)?;
            let mut held=LedgerConnection::new(connection);
            self.statement();
            let mut tx=held.connection().begin().await.map_err(backend)?;
            self.timeouts(&mut tx).await?;
            self.statement();
            let updated=sqlx::query("UPDATE venue_orders SET last_checked_at=GREATEST(last_checked_at,$1)
                WHERE venue_id=$2 AND account_scope=$3 AND symbol=$4 AND exchange_order_id=$5 AND tenant_id=$6")
                .bind(at).bind(key.venue.as_str()).bind(&key.account_scope).bind(&key.symbol)
                .bind(key.exchange_order_id.to_string()).bind(self.tenant_id)
                .execute(&mut *tx).await.map_err(backend)?;
            if updated.rows_affected()!=1 {return Err(LedgerError::UnknownOrder);}
            self.statement();
            committing.store(true,Ordering::Relaxed);
            tx.commit().await.map_err(commit_error)?;
            held.finished=true;
            Ok(())
        },&committing).await
    }

    pub(super) async fn read_trade_context(
        &self,
        conn: &mut PgConnection,
        key: &OrderKey,
        ids: &[String],
    ) -> Result<(StoredOrder, Vec<StoredTrade>), LedgerError> {
        self.statement();
        let sql = format!(
            "SELECT {ORDER_COLUMNS},{FILL_COLUMNS} FROM venue_orders o LEFT JOIN order_fills f
            ON f.venue_id=$1 AND f.account_scope=$2 AND f.symbol=$3 AND f.trade_id=ANY($5)
            WHERE o.venue_id=$1 AND o.account_scope=$2 AND o.symbol=$3 AND o.exchange_order_id=$4"
        );
        let rows = sqlx::query(&sql)
            .bind(key.venue.as_str())
            .bind(&key.account_scope)
            .bind(&key.symbol)
            .bind(key.exchange_order_id.to_string())
            .bind(ids)
            .fetch_all(conn)
            .await
            .map_err(backend)?;
        let first = rows.first().ok_or(LedgerError::UnknownOrder)?;
        let order = decode_order(first, self.tenant_id)?.ok_or(LedgerError::UnknownOrder)?;
        let fills = rows
            .iter()
            .map(|row| decode_fill(row, key))
            .filter_map(Result::transpose)
            .collect::<Result<Vec<_>, _>>()?;
        Ok((order, fills))
    }

    async fn read_existing_fills(
        &self,
        conn: &mut PgConnection,
        key: &OrderKey,
        ids: &[String],
    ) -> Result<Vec<StoredTrade>, LedgerError> {
        self.statement();
        let sql = format!(
            "SELECT {FILL_COLUMNS} FROM order_fills f
            WHERE f.venue_id=$1 AND f.account_scope=$2 AND f.symbol=$3 AND f.trade_id=ANY($4)"
        );
        let rows = sqlx::query(&sql)
            .bind(key.venue.as_str())
            .bind(&key.account_scope)
            .bind(&key.symbol)
            .bind(ids)
            .fetch_all(conn)
            .await
            .map_err(backend)?;
        rows.iter()
            .map(|row| decode_fill(row, key))
            .filter_map(Result::transpose)
            .collect()
    }

    async fn insert_fills(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        key: &OrderKey,
        fills: &[StoredTrade],
    ) -> Result<(), LedgerError> {
        let ids: Vec<_> = fills.iter().map(|f| f.trade_id.to_string()).collect();
        let prices: Vec<_> = fills.iter().map(|f| f.price).collect();
        let quantities: Vec<_> = fills.iter().map(|f| f.qty).collect();
        let quotes: Vec<_> = fills.iter().map(|f| f.quote_qty).collect();
        let commissions: Vec<_> = fills.iter().map(|f| f.commission).collect();
        let assets: Vec<_> = fills.iter().map(|f| f.commission_asset.clone()).collect();
        let times: Vec<_> = fills.iter().map(|f| f.trade_time).collect();
        let recorded: Vec<_> = fills.iter().map(|f| f.recorded_at).collect();
        self.statement();
        sqlx::query("INSERT INTO order_fills (venue_id,account_scope,symbol,trade_id,exchange_order_id,
            tenant_id,price,qty,quote_qty,commission,commission_asset,trade_time,recorded_at)
            SELECT $1,$2,$3,u.id,$4,$5,u.price,u.qty,u.quote,u.commission,u.asset,u.time,u.recorded
            FROM UNNEST($6::text[],$7::numeric[],$8::numeric[],$9::numeric[],$10::numeric[],
                $11::text[],$12::timestamptz[],$13::timestamptz[]) AS u(id,price,qty,quote,commission,asset,time,recorded)")
            .bind(key.venue.as_str()).bind(&key.account_scope).bind(&key.symbol).bind(key.exchange_order_id.to_string())
            .bind(self.tenant_id).bind(ids).bind(prices).bind(quantities).bind(quotes).bind(commissions).bind(assets)
            .bind(times).bind(recorded).execute(&mut **tx).await.map_err(trade_error)?;
        Ok(())
    }

    /// ADR-0056 step 8l-3 (PLAN-r4 §6.10): new fills of an order with no
    /// governed leg invalidate the UTC month of each one's venue time, in one
    /// idempotent statement. Leg fills add none: their effect on every cash
    /// conclusion is derived at read time.
    async fn invalidate_unattributed(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        key: &OrderKey,
        fills: &[StoredTrade],
    ) -> Result<(), LedgerError> {
        let ids: Vec<_> = fills.iter().map(|_| uuid::Uuid::now_v7()).collect();
        let trades: Vec<_> = fills.iter().map(|f| f.trade_id.to_string()).collect();
        let times: Vec<_> = fills.iter().map(|f| f.trade_time).collect();
        self.statement();
        sqlx::query("INSERT INTO accounting_invalidations (invalidation_id,tenant_id,venue_id,month,reason,
            evidence_key,account_scope,symbol,exchange_order_id,trade_id,trade_time)
            SELECT u.id,$1,$2,date_trunc('month',u.time AT TIME ZONE 'UTC')::date,'unattributed_fill',
                $3||'/'||$4||'/'||u.trade,$3,$4,$5,u.trade,u.time
            FROM UNNEST($6::uuid[],$7::text[],$8::timestamptz[]) AS u(id,trade,time) ON CONFLICT DO NOTHING")
            .bind(self.tenant_id).bind(key.venue.as_str()).bind(&key.account_scope).bind(&key.symbol)
            .bind(key.exchange_order_id.to_string()).bind(ids).bind(trades).bind(times)
            .execute(&mut **tx).await.map_err(backend)?;
        Ok(())
    }
}

fn decode_fill(row: &PgRow, key: &OrderKey) -> Result<Option<StoredTrade>, LedgerError> {
    let id: Option<String> = row.try_get("fill_trade_id").map_err(backend)?;
    let Some(id) = id else { return Ok(None) };
    Ok(Some(StoredTrade {
        order: OrderKey {
            venue: VenueId::new(key.venue.as_str()),
            account_scope: key.account_scope.clone(),
            symbol: key.symbol.clone(),
            exchange_order_id: row
                .try_get::<String, _>("fill_order_id")
                .map_err(backend)?
                .parse()?,
        },
        tenant_id: row.try_get("fill_tenant_id").map_err(backend)?,
        trade_id: id.parse()?,
        price: row.try_get("fill_price").map_err(backend)?,
        qty: row.try_get("fill_qty").map_err(backend)?,
        quote_qty: row.try_get("fill_quote_qty").map_err(backend)?,
        commission: row.try_get("fill_commission").map_err(backend)?,
        commission_asset: row.try_get("fill_asset").map_err(backend)?,
        trade_time: row.try_get("fill_time").map_err(backend)?,
        recorded_at: row.try_get("fill_recorded_at").map_err(backend)?,
    }))
}

fn trade_error(error: sqlx::Error) -> LedgerError {
    if error.as_database_error().is_some_and(|e| e.code().as_deref() == Some("23505")) {
        LedgerError::TradeConflict
    } else {
        backend(error)
    }
}
