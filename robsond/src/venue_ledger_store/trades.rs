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

use super::{
    backend, commit_error,
    orders::{decode_order, order_error},
    PgVenueLedger,
};
use crate::order_journal_store::RetiringConnection;

const FILL_COLUMNS: &str = "f.trade_id AS fill_trade_id,f.exchange_order_id AS fill_order_id,
    f.tenant_id AS fill_tenant_id,f.price AS fill_price,f.qty AS fill_qty,
    f.quote_qty AS fill_quote_qty,f.commission AS fill_commission,
    f.commission_asset AS fill_asset,f.trade_time AS fill_time,f.recorded_at AS fill_recorded_at";

impl PgVenueLedger {
    /// One page, at most 1,000 trades and ten statements. A duplicate page
    /// without cursor progress is one read, no bump, and no rotation update.
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
            let connection=self.pool.acquire().await.map_err(backend)?;
            let mut held=RetiringConnection {connection,finished:false};
            let (before,existing)=self.read_trade_context(&mut held.connection,key,&ids).await?;
            let proposed=plan_trades(&before,self.tenant_id,page,&existing,recorded_at)?;
            if !proposed.changed {
                held.finished=true;
                return Ok(TradeWriteResult {changed:false});
            }
            self.statement();
            let mut tx=held.connection.begin().await.map_err(backend)?;
            self.timeouts(&mut tx).await?;
            self.statement();
            let bump=sqlx::query("UPDATE accounting_state SET version=version+1 WHERE tenant_id=$1 AND venue_id=$2")
                .bind(self.tenant_id).bind(key.venue.as_str()).execute(&mut *tx).await.map_err(backend)?;
            if bump.rows_affected()!=1 {return Err(LedgerError::NoAccountingRow);}
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
            }
            self.statement();
            let updated=sqlx::query("UPDATE venue_orders SET fills_qty=$1,executed_cum=$2,trades_cursor=$3,
                last_checked_at=GREATEST(last_checked_at,$4) WHERE venue_id=$5 AND account_scope=$6
                AND symbol=$7 AND exchange_order_id=$8 AND tenant_id=$9")
                .bind(planned.order.fills_qty).bind(planned.order.executed_cum).bind(planned.order.trades_cursor)
                .bind(planned.order.last_checked_at).bind(key.venue.as_str()).bind(&key.account_scope)
                .bind(&key.symbol).bind(key.exchange_order_id.to_string()).bind(self.tenant_id)
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
            self.statement();
            committing.store(true,Ordering::Relaxed);
            tx.commit().await.map_err(commit_error)?;
            held.finished=true;
            Ok(TradeWriteResult {changed:true})
        },&committing).await
    }

    /// Rotation metadata only: one autocommit statement, no accounting bump.
    /// A missing answer is uncertain because that autocommit may have landed.
    pub async fn touch_checked(
        &self,
        key: &OrderKey,
        at: DateTime<Utc>,
    ) -> Result<(), LedgerError> {
        let committing = AtomicBool::new(false);
        self.bounded(async {
            let connection=self.pool.acquire().await.map_err(backend)?;
            let mut held=RetiringConnection {connection,finished:false};
            self.statement();
            committing.store(true,Ordering::Relaxed);
            let updated=sqlx::query("UPDATE venue_orders SET last_checked_at=GREATEST(last_checked_at,$1)
                WHERE venue_id=$2 AND account_scope=$3 AND symbol=$4 AND exchange_order_id=$5 AND tenant_id=$6")
                .bind(at).bind(key.venue.as_str()).bind(&key.account_scope).bind(&key.symbol)
                .bind(key.exchange_order_id.to_string()).bind(self.tenant_id)
                .execute(&mut *held.connection).await.map_err(commit_error)?;
            held.finished=true;
            if updated.rows_affected()!=1 {return Err(LedgerError::UnknownOrder);}
            Ok(())
        },&committing).await
    }

    async fn read_trade_context(
        &self,
        conn: &mut PgConnection,
        key: &OrderKey,
        ids: &[String],
    ) -> Result<(StoredOrder, Vec<StoredTrade>), LedgerError> {
        self.statement();
        let sql = format!(
            "SELECT o.*,{FILL_COLUMNS} FROM venue_orders o LEFT JOIN order_fills f
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
