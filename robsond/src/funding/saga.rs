#[cfg(feature = "postgres")]
use std::sync::Arc;

#[cfg(feature = "postgres")]
use chrono::{Duration, Utc};
#[cfg(feature = "postgres")]
use robson_eventlog::{append_event, ActorType, Event, QueryOptions};
#[cfg(feature = "postgres")]
use robson_exec::{
    ports::{SpotOrderQuantity, SpotOrderRequest, SpotOrderSide, UniversalTransferType},
    ExchangePort,
};
#[cfg(feature = "postgres")]
use robson_store::Store;
#[cfg(feature = "postgres")]
use rust_decimal::Decimal;
#[cfg(feature = "postgres")]
use rust_decimal_macros::dec;
#[cfg(feature = "postgres")]
use serde_json::json;
#[cfg(feature = "postgres")]
use sqlx::{PgPool, Row};
#[cfg(feature = "postgres")]
use tokio::sync::RwLock;
#[cfg(feature = "postgres")]
use uuid::Uuid;

#[cfg(feature = "postgres")]
use super::types::{
    ExecuteFundingResponse, FundingEventView, FundingQuote, FundingQuoteItem, FundingSagaSummary,
    FundingSagaView, FundingState,
};
#[cfg(feature = "postgres")]
use crate::{
    config::FundingConfig,
    error::{DaemonError, DaemonResult},
    position_manager::PositionManager,
};

#[cfg(feature = "postgres")]
const SPOT_FEE_RATE: Decimal = dec!(0.001);

#[cfg(feature = "postgres")]
pub struct FundingService<E: ExchangePort + 'static, S: Store + 'static> {
    pool: Arc<PgPool>,
    tenant_id: Uuid,
    exchange: Arc<E>,
    position_manager: Arc<RwLock<PositionManager<E, S>>>,
    config: FundingConfig,
}

#[cfg(feature = "postgres")]
impl<E: ExchangePort + 'static, S: Store + 'static> FundingService<E, S> {
    pub fn new(
        pool: Arc<PgPool>,
        tenant_id: Uuid,
        exchange: Arc<E>,
        position_manager: Arc<RwLock<PositionManager<E, S>>>,
        config: FundingConfig,
    ) -> Self {
        Self {
            pool,
            tenant_id,
            exchange,
            position_manager,
            config,
        }
    }

    pub async fn quote(&self) -> DaemonResult<FundingQuote> {
        let quote_id = Uuid::new_v4();
        let balances = self.exchange.get_spot_account_balances().await?;
        let expires_at = Utc::now() + Duration::seconds(self.config.quote_ttl_secs as i64);
        let mut items = Vec::new();
        let mut estimated_usdt = Decimal::ZERO;
        let mut fees = Decimal::ZERO;
        let buffer = Decimal::from(self.config.slippage_bps) / Decimal::from(10_000u32);

        for balance in balances {
            if balance.asset == "USDT" || balance.free <= Decimal::ZERO {
                continue;
            }
            let Some(route) = self.resolve_usdt_route(&balance.asset).await? else {
                tracing::debug!(asset = %balance.asset, "Skipping spot asset without USDT route");
                continue;
            };
            let price = self.exchange.get_spot_price(&route.symbol).await?;
            let gross = match route.side {
                SpotOrderSide::Sell => balance.free * price.as_decimal(),
                SpotOrderSide::Buy => balance.free / price.as_decimal(),
            };
            if gross < self.config.dust_usdt {
                continue;
            }
            let fee = gross * SPOT_FEE_RATE;
            let est_usdt = gross * (Decimal::ONE - buffer) - fee;
            if est_usdt < self.config.dust_usdt {
                continue;
            }
            fees += fee;
            estimated_usdt += est_usdt;
            items.push(FundingQuoteItem {
                asset: balance.asset,
                qty: balance.free,
                est_usdt,
                symbol: route.symbol,
            });
        }

        let quote = FundingQuote {
            quote_id,
            items,
            estimated_usdt,
            fees,
            slippage_bps: self.config.slippage_bps,
            expires_at,
        };
        self.append_and_project(
            quote_id,
            FundingState::Quoted,
            "FundingQuoted",
            json!({ "quote": quote }),
        )
        .await?;
        Ok(quote)
    }

    pub async fn execute(
        &self,
        quote_id: Uuid,
        idempotency_key: &str,
    ) -> DaemonResult<ExecuteFundingResponse> {
        let mut view = self.load_saga(quote_id).await?;
        if view.state == FundingState::Refreshed.as_str() {
            return Ok(ExecuteFundingResponse { saga_id: quote_id, state: view.state });
        }
        if view.state == FundingState::Failed.as_str() {
            return Ok(ExecuteFundingResponse { saga_id: quote_id, state: view.state });
        }

        let quote = self.load_quote(quote_id).await?;
        if Utc::now() > quote.expires_at && view.state == FundingState::Quoted.as_str() {
            self.fail(quote_id, "quote_expired").await?;
            return Err(DaemonError::Config("quote_expired".to_string()));
        }

        if view.state == FundingState::Quoted.as_str() {
            self.append_and_project(
                quote_id,
                FundingState::Converting,
                "FundingStarted",
                json!({ "idempotency_key": idempotency_key }),
            )
            .await?;
            view.state = FundingState::Converting.as_str().to_string();
        }

        if view.state == FundingState::Converting.as_str() {
            for item in &quote.items {
                let client_order_id = spot_client_order_id(quote_id, &item.asset);
                let route = route_from_quote_item(&item.asset, &item.symbol)?;
                let order =
                    match self.exchange.get_spot_order(&item.symbol, &client_order_id).await? {
                        Some(order) if order.status == "FILLED" => order,
                        _ => {
                            self.exchange
                                .place_spot_market_order(SpotOrderRequest {
                                    symbol: item.symbol.clone(),
                                    side: route.side,
                                    quantity_kind: route.quantity_kind,
                                    quantity: item.qty,
                                    client_order_id: client_order_id.clone(),
                                })
                                .await?
                        },
                    };
                if order.status != "FILLED" {
                    self.fail(quote_id, &format!("spot_order_not_filled:{}", item.asset)).await?;
                    return Err(DaemonError::Config(format!(
                        "spot_order_not_filled:{}",
                        item.asset
                    )));
                }
                let usdt_out = match route.side {
                    SpotOrderSide::Sell => order.cummulative_quote_qty,
                    SpotOrderSide::Buy if order.fee_asset == "USDT" => {
                        (order.executed_qty - order.fee).max(Decimal::ZERO)
                    },
                    SpotOrderSide::Buy => order.executed_qty,
                };
                self.append_and_project(
                    quote_id,
                    FundingState::Converting,
                    "ConversionExecuted",
                    json!({
                        "asset": item.asset,
                        "qty": order.executed_qty,
                        "usdt_out": usdt_out,
                        "client_order_id": client_order_id,
                    }),
                )
                .await?;
            }

            self.append_and_project(
                quote_id,
                FundingState::Converted,
                "FundingConverted",
                json!({ "spot_usdt": self.current_spot_usdt().await? }),
            )
            .await?;
            view.state = FundingState::Converted.as_str().to_string();
        }

        if view.state == FundingState::Converted.as_str()
            || view.state == FundingState::Transferring.as_str()
        {
            let amount = self.current_spot_usdt().await?;
            if amount > Decimal::ZERO {
                let client_tran_key = transfer_client_key(quote_id);
                self.append_and_project(
                    quote_id,
                    FundingState::Transferring,
                    "TransferPrepared",
                    json!({ "client_tran_key": client_tran_key, "amount": amount }),
                )
                .await?;
                let existing = self
                    .exchange
                    .get_transfer_history(
                        UniversalTransferType::MainUmfuture,
                        Utc::now() - Duration::hours(24),
                    )
                    .await?
                    .into_iter()
                    .find(|t| t.client_tran_key.as_deref() == Some(&client_tran_key));
                let transfer_id = match existing {
                    Some(t) => t.transfer_id,
                    None => {
                        self.exchange
                            .universal_transfer(
                                "USDT",
                                amount,
                                UniversalTransferType::MainUmfuture,
                                &client_tran_key,
                            )
                            .await?
                    },
                };
                self.append_and_project(
                    quote_id,
                    FundingState::Transferring,
                    "TransferExecuted",
                    json!({ "transfer_id": transfer_id.0, "amount": amount }),
                )
                .await?;
            }
            self.append_and_project(quote_id, FundingState::Settled, "FundingSettled", json!({}))
                .await?;
            view.state = FundingState::Settled.as_str().to_string();
        }

        if view.state == FundingState::Settled.as_str() {
            let capital = self.refresh_capital().await?;
            self.append_and_project(
                quote_id,
                FundingState::Refreshed,
                "CapitalRefreshed",
                json!({ "capital": capital }),
            )
            .await?;
            view.state = FundingState::Refreshed.as_str().to_string();
        }

        Ok(ExecuteFundingResponse { saga_id: quote_id, state: view.state })
    }

    pub async fn refresh_capital(&self) -> DaemonResult<Decimal> {
        let balance = self.exchange.get_futures_balance().await?;
        let capital = balance.wallet_balance;
        if capital <= Decimal::ZERO {
            return Err(DaemonError::Config(format!(
                "Exchange reports zero or negative wallet balance: {capital}"
            )));
        }
        let manager = self.position_manager.read().await;
        manager.update_engine_capital(capital);
        Ok(capital)
    }

    pub async fn get(&self, saga_id: Uuid) -> DaemonResult<FundingSagaView> {
        self.load_saga(saga_id).await
    }

    pub async fn list(&self) -> DaemonResult<Vec<FundingSagaSummary>> {
        let rows = sqlx::query(
            r#"
            SELECT saga_id, state, estimated_usdt, created_at
            FROM funding_sagas
            WHERE tenant_id = $1
            ORDER BY created_at DESC
            LIMIT 100
            "#,
        )
        .bind(self.tenant_id)
        .fetch_all(&*self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| FundingSagaSummary {
                saga_id: row.get("saga_id"),
                state: row.get("state"),
                estimated_usdt: row.get("estimated_usdt"),
                created_at: row.get("created_at"),
            })
            .collect())
    }

    pub async fn resume_non_terminal(&self) -> DaemonResult<usize> {
        let rows = sqlx::query(
            r#"
            SELECT saga_id
            FROM funding_sagas
            WHERE tenant_id = $1
              AND state NOT IN ('REFRESHED', 'FAILED')
            ORDER BY updated_at ASC
            LIMIT 20
            "#,
        )
        .bind(self.tenant_id)
        .fetch_all(&*self.pool)
        .await?;

        let mut resumed = 0;
        for row in rows {
            let saga_id: Uuid = row.get("saga_id");
            let _ = self.execute(saga_id, "funding-worker-resume").await;
            resumed += 1;
        }
        Ok(resumed)
    }

    async fn current_spot_usdt(&self) -> DaemonResult<Decimal> {
        Ok(self
            .exchange
            .get_spot_account_balances()
            .await?
            .into_iter()
            .find(|b| b.asset == "USDT")
            .map(|b| b.free)
            .unwrap_or(Decimal::ZERO))
    }

    async fn resolve_usdt_route(&self, asset: &str) -> DaemonResult<Option<SpotConversionRoute>> {
        let direct = format!("{asset}USDT");
        if self.exchange.spot_symbol_is_trading(&direct).await? {
            return Ok(Some(SpotConversionRoute {
                symbol: direct,
                side: SpotOrderSide::Sell,
                quantity_kind: SpotOrderQuantity::Base,
            }));
        }

        let inverse = format!("USDT{asset}");
        if self.exchange.spot_symbol_is_trading(&inverse).await? {
            return Ok(Some(SpotConversionRoute {
                symbol: inverse,
                side: SpotOrderSide::Buy,
                quantity_kind: SpotOrderQuantity::Quote,
            }));
        }

        Ok(None)
    }

    async fn fail(&self, saga_id: Uuid, reason: &str) -> DaemonResult<()> {
        self.append_and_project(
            saga_id,
            FundingState::Failed,
            "FundingFailed",
            json!({ "reason": reason }),
        )
        .await
    }

    async fn load_quote(&self, saga_id: Uuid) -> DaemonResult<FundingQuote> {
        let snapshot: serde_json::Value = sqlx::query_scalar(
            "SELECT quote FROM funding_sagas WHERE tenant_id = $1 AND saga_id = $2",
        )
        .bind(self.tenant_id)
        .bind(saga_id)
        .fetch_optional(&*self.pool)
        .await?
        .ok_or_else(|| DaemonError::Config("funding_saga_not_found".to_string()))?;

        serde_json::from_value(snapshot)
            .map_err(|e| DaemonError::Config(format!("invalid funding quote snapshot: {e}")))
    }

    async fn load_saga(&self, saga_id: Uuid) -> DaemonResult<FundingSagaView> {
        let row = sqlx::query(
            r#"
            SELECT state, quote, updated_at
            FROM funding_sagas
            WHERE tenant_id = $1 AND saga_id = $2
            "#,
        )
        .bind(self.tenant_id)
        .bind(saga_id)
        .fetch_optional(&*self.pool)
        .await?
        .ok_or_else(|| DaemonError::Config("funding_saga_not_found".to_string()))?;

        let quote: FundingQuote = serde_json::from_value(row.get("quote"))
            .map_err(|e| DaemonError::Config(format!("invalid funding quote snapshot: {e}")))?;
        let events = robson_eventlog::query_events(
            &self.pool,
            QueryOptions::new(self.tenant_id).stream(stream_key(saga_id)),
        )
        .await
        .map_err(|e| DaemonError::EventLog(e.to_string()))?
        .into_iter()
        .map(|event| FundingEventView {
            event_type: event.event_type,
            at: event.occurred_at,
            payload: event.payload,
        })
        .collect();

        Ok(FundingSagaView {
            saga_id,
            state: row.get("state"),
            items: quote.items,
            events,
            updated_at: row.get("updated_at"),
        })
    }

    async fn append_and_project(
        &self,
        saga_id: Uuid,
        state: FundingState,
        event_type: &str,
        payload: serde_json::Value,
    ) -> DaemonResult<()> {
        let mut event =
            Event::new(self.tenant_id, stream_key(saga_id), event_type, payload.clone())
                .with_actor(ActorType::Daemon, Some("funding-saga".to_string()));
        event.workflow_id = Some(saga_id);
        event.command_id = Some(Uuid::new_v4());
        append_event(&self.pool, &stream_key(saga_id), None, event)
            .await
            .map_err(|e| DaemonError::EventLog(e.to_string()))?;

        let quote = if let Some(quote) = payload.get("quote").cloned() {
            quote
        } else {
            sqlx::query_scalar::<_, serde_json::Value>(
                "SELECT quote FROM funding_sagas WHERE tenant_id = $1 AND saga_id = $2",
            )
            .bind(self.tenant_id)
            .bind(saga_id)
            .fetch_optional(&*self.pool)
            .await?
            .unwrap_or_else(|| {
                json!({
                    "quote_id": saga_id,
                    "items": [],
                    "estimated_usdt": "0",
                    "fees": "0",
                    "slippage_bps": self.config.slippage_bps,
                    "expires_at": Utc::now()
                })
            })
        };
        let estimated_usdt = serde_json::from_value::<FundingQuote>(quote.clone())
            .map(|q| q.estimated_usdt)
            .unwrap_or(Decimal::ZERO);

        sqlx::query(
            r#"
            INSERT INTO funding_sagas (
                tenant_id, saga_id, state, quote, estimated_usdt, created_at, updated_at
            ) VALUES ($1, $2, $3, $4, $5, NOW(), NOW())
            ON CONFLICT (tenant_id, saga_id) DO UPDATE SET
                state = EXCLUDED.state,
                quote = COALESCE(funding_sagas.quote, EXCLUDED.quote),
                estimated_usdt = COALESCE(funding_sagas.estimated_usdt, EXCLUDED.estimated_usdt),
                updated_at = NOW()
            "#,
        )
        .bind(self.tenant_id)
        .bind(saga_id)
        .bind(state.as_str())
        .bind(quote)
        .bind(estimated_usdt)
        .execute(&*self.pool)
        .await?;

        Ok(())
    }
}

#[cfg(feature = "postgres")]
fn stream_key(saga_id: Uuid) -> String {
    format!("funding:{saga_id}")
}

/// Binance `origClientOrderId` / `clientTranKey` must match `^[a-zA-Z0-9-_]{1,36}$`.
/// We use a 16-char truncated UUID (8 bytes hex) to stay well within the 36-char limit
/// while retaining enough entropy for idempotency (64 bits = 2^64 unique values).
#[cfg(feature = "postgres")]
fn spot_client_order_id(saga_id: Uuid, asset: &str) -> String {
    let short = &saga_id.simple().to_string()[..16];
    format!("rbx-{short}-{asset}")
}

#[cfg(feature = "postgres")]
fn transfer_client_key(saga_id: Uuid) -> String {
    let short = &saga_id.simple().to_string()[..16];
    format!("rbx-tr-{short}")
}

#[cfg(feature = "postgres")]
struct SpotConversionRoute {
    symbol: String,
    side: SpotOrderSide,
    quantity_kind: SpotOrderQuantity,
}

#[cfg(feature = "postgres")]
fn route_from_quote_item(asset: &str, symbol: &str) -> DaemonResult<SpotConversionRoute> {
    if symbol == format!("{asset}USDT") {
        return Ok(SpotConversionRoute {
            symbol: symbol.to_string(),
            side: SpotOrderSide::Sell,
            quantity_kind: SpotOrderQuantity::Base,
        });
    }

    if symbol == format!("USDT{asset}") {
        return Ok(SpotConversionRoute {
            symbol: symbol.to_string(),
            side: SpotOrderSide::Buy,
            quantity_kind: SpotOrderQuantity::Quote,
        });
    }

    Err(DaemonError::Config(format!("invalid_funding_route:{asset}:{symbol}")))
}
