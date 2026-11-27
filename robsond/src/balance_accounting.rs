//! A balance-account daemon's accounting context on the shared paths
//! (ADR-0056 step 8l-4; PLAN-r4 §6.5, §6.7).
//!
//! On a balance-account daemon a stream suspension's registration and its
//! clearing, an incident, a release and every `monthly_state` write bump
//! `accounting_state.version` in their own transaction and, when an order-set
//! generation is installed, advance it by the exact non-membership CAS of
//! `trade_generation.sql`: none of them touches a SELL/CANCEL membership
//! input. A missing CAS row refuses, and the caller rolls its whole
//! transaction back. A position account (USD-M) builds no context, so its
//! statements are the ones it always sent.
//!
//! Every bump here is a certification input's write, so it runs under the
//! cutover certificate's shared key (ADR-0056 step 8m-2a, 8m PLAN-r4 §6.7):
//! [`BalanceAccounting::advance`] takes it as its first statement; a writer
//! that took it earlier, before a row lock of its own, passes its
//! [`Participating`] proof to [`BalanceAccounting::advance_participating`] or
//! [`BalanceAccounting::advance_from`], so the key is taken once.
use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, PoisonError, RwLock},
};

use robson_eventlog::certificate::{participate, Participating};
use robson_exec::{
    order_journal::DEFAULT_ACCOUNT_SCOPE,
    venue::{AccountKind, VenueId},
};
use robson_projector::AccountingStep;
use sqlx::PgConnection;
use tracing::Instrument as _;
use uuid::Uuid;

use crate::spot_order_set_store::JournalGeneration;

#[cfg(test)]
#[path = "balance_accounting_pg_tests.rs"]
mod pg_tests;

#[cfg(test)]
#[path = "certificate_participation_pg_tests.rs"]
pub(crate) mod certificate_tests;

#[cfg(test)]
#[path = "certificate_projector_pg_tests.rs"]
mod certificate_projector_tests;

/// The ledger's transaction-local server bounds, the first statement after
/// `BEGIN` of every transaction this context opens (PLAN-r4 §7.2).
pub(crate) const BOUNDS: &str = "SELECT set_config('lock_timeout', '300ms', true), set_config('statement_timeout', '1s', true), set_config('idle_in_transaction_session_timeout', '1s', true)";

/// Every accounting row of the tenant, locked in (tenant, venue) order, as
/// [`BUMP`] takes them: the cash writer's row lock and a balance daemon's
/// epoch insert (ADR-0056 step 8l-3b), so neither waits on the other in a
/// cycle and a conclusion holding them is never overtaken by a new epoch.
pub(crate) const ACCOUNTING_ROWS: &str = "SELECT venue_id, version FROM public.accounting_state WHERE tenant_id = $1 ORDER BY venue_id FOR UPDATE";

/// One bump of every accounting row of the tenant, locked in (tenant, venue)
/// order, so writers that take several rows never wait on each other in a
/// cycle. A tenant with no row yet (8m's cutover creates it) moves nothing.
const BUMP: &str = "WITH locked AS (
        SELECT tenant_id, venue_id FROM public.accounting_state
        WHERE tenant_id = $1 ORDER BY tenant_id, venue_id FOR UPDATE)
    UPDATE public.accounting_state a SET version = a.version + 1
    FROM locked WHERE (a.tenant_id, a.venue_id) = (locked.tenant_id, locked.venue_id)
    RETURNING a.venue_id, a.version";

/// The trade page's exact non-membership CAS, reused unchanged.
const GENERATION_CAS: &str = include_str!("spot_order_set_store/trade_generation.sql");

/// The order-set owner's installed generation, one cell per balance-account
/// daemon, shared by its journal and its accounting context: every write
/// reads it at its own transaction. The owner publishes each generation it
/// installs, its identity rotated by a rebuild, so a write refused on a stale
/// one lands on its next retry. Empty, every writer is unopted (no CAS), and
/// nothing on main publishes.
#[derive(Debug, Clone, Default)]
pub(crate) struct GenerationCell(Arc<RwLock<Option<JournalGeneration>>>);

impl GenerationCell {
    /// The owner's handoff, after an install or a rebuild.
    #[cfg_attr(
        not(test),
        allow(dead_code, reason = "the order-set owner publishes (5a)")
    )]
    pub(crate) fn publish(&self, generation: JournalGeneration) {
        *self.0.write().unwrap_or_else(PoisonError::into_inner) = Some(generation);
    }

    pub(crate) fn current(&self) -> Option<JournalGeneration> {
        self.0.read().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

/// A balance-account daemon's accounting identity, built by the daemon and
/// consumed by its projector, its suspension clear and the accounting writers
/// after it.
#[derive(Debug, Clone)]
pub(crate) struct BalanceAccounting {
    pub(crate) tenant_id: Uuid,
    pub(crate) venue: VenueId,
    pub(crate) account_scope: String,
    /// The owner's cell, the one the daemon's journal reads.
    pub(crate) generation: GenerationCell,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum AccountingRefusal {
    #[error("balance accounting: {0}")]
    Database(#[from] sqlx::Error),
    #[error("balance accounting refused: the installed generation is not this context's")]
    Binding,
    #[error("balance accounting refused: no accounting row for the installed generation")]
    NoAccountingRow,
    #[error("balance accounting refused: stale order-set generation")]
    StaleGeneration,
    #[error("balance accounting refused: the venue's version is not the expected one")]
    #[cfg_attr(
        not(test),
        allow(
            dead_code,
            reason = "advance_from's refusal; 8m-4's tx2b and 8l-5 call it"
        )
    )]
    VersionMoved,
}

impl BalanceAccounting {
    /// The daemon's context on the cell its journal reads: `None` unless its
    /// venue is a balance account.
    pub(crate) fn for_daemon(
        account_kind: AccountKind,
        tenant_id: Uuid,
        venue: &VenueId,
        generation: &GenerationCell,
    ) -> Option<Self> {
        (account_kind == AccountKind::BalanceAccount)
            .then(|| Self::new(tenant_id, venue.clone(), generation))
    }

    /// A balance journal's context: Binance Spot, its tenant, its cell.
    pub(crate) fn for_journal(tenant_id: Uuid, generation: &GenerationCell) -> Self {
        Self::new(tenant_id, VenueId::new(VenueId::BINANCE_SPOT), generation)
    }

    fn new(tenant_id: Uuid, venue: VenueId, generation: &GenerationCell) -> Self {
        Self {
            tenant_id,
            venue,
            account_scope: DEFAULT_ACCOUNT_SCOPE.to_string(),
            generation: generation.clone(),
        }
    }

    /// The certificate's shared key, the bump and, when a generation is
    /// installed, its CAS: three statements on the caller's transaction, right
    /// after its bounds, on the cell's generation as read once here. The
    /// context venue's new version; `None` when the tenant has no row of it
    /// and nothing is installed. On any error the caller must not commit.
    pub(crate) async fn advance(
        &self,
        connection: &mut PgConnection,
    ) -> Result<Option<i64>, AccountingRefusal> {
        let generation = self.installed()?;
        let _key = participate(&mut *connection).await?;
        self.bump_and_advance(connection, generation.as_ref()).await
    }

    /// [`Self::advance`] on a transaction that already holds the key: the bump
    /// and the CAS.
    pub(crate) async fn advance_participating(
        &self,
        connection: &mut PgConnection,
        _key: &Participating,
    ) -> Result<Option<i64>, AccountingRefusal> {
        let generation = self.installed()?;
        self.bump_and_advance(connection, generation.as_ref()).await
    }

    /// The conditional variant on a transaction that holds the key (8m-4's
    /// tx2b bumps from v₁ + 1; offered to 8l-5's conditional bump): every row
    /// of the tenant bumped in order, the context venue's required at
    /// `expected` before it, then the CAS. Its new version, `expected + 1`;
    /// `VersionMoved` otherwise, and the caller must not commit.
    #[cfg_attr(
        not(test),
        allow(dead_code, reason = "8m-4's tx2b and 8l-5's conditional bump call it")
    )]
    pub(crate) async fn advance_from(
        &self,
        connection: &mut PgConnection,
        _key: &Participating,
        expected: i64,
    ) -> Result<i64, AccountingRefusal> {
        let generation = self.installed()?;
        let version = self.bump(connection).await?.ok_or(AccountingRefusal::NoAccountingRow)?;
        if Some(version) != expected.checked_add(1) {
            return Err(AccountingRefusal::VersionMoved);
        }
        self.advance_generation(connection, generation.as_ref(), Some(version)).await?;
        Ok(version)
    }

    /// The cell's generation, refused before any statement unless it is this
    /// context's.
    fn installed(&self) -> Result<Option<JournalGeneration>, AccountingRefusal> {
        let generation = self.generation.current();
        if generation.as_ref().is_some_and(|installed| {
            !installed.is_valid()
                || installed.tenant_id != self.tenant_id
                || installed.account_scope != self.account_scope
                || self.venue.as_str() != VenueId::BINANCE_SPOT
        }) {
            return Err(AccountingRefusal::Binding);
        }
        Ok(generation)
    }

    async fn bump_and_advance(
        &self,
        connection: &mut PgConnection,
        generation: Option<&JournalGeneration>,
    ) -> Result<Option<i64>, AccountingRefusal> {
        let version = self.bump(connection).await?;
        self.advance_generation(connection, generation, version).await?;
        Ok(version)
    }

    async fn bump(&self, connection: &mut PgConnection) -> Result<Option<i64>, AccountingRefusal> {
        let bumped = sqlx::query_as::<_, (String, i64)>(BUMP)
            .bind(self.tenant_id)
            .fetch_all(&mut *connection)
            .instrument(
                tracing::info_span!(target: "robson_operation", "operation_io", io_kind = 1_u64),
            )
            .await?;
        Ok(bumped
            .into_iter()
            .find(|(venue, _)| venue == self.venue.as_str())
            .map(|(_, version)| version))
    }

    async fn advance_generation(
        &self,
        connection: &mut PgConnection,
        generation: Option<&JournalGeneration>,
        version: Option<i64>,
    ) -> Result<(), AccountingRefusal> {
        let Some(generation) = generation else {
            return Ok(());
        };
        let version = version.ok_or(AccountingRefusal::NoAccountingRow)?;
        let advanced = sqlx::query_scalar::<_, i64>(GENERATION_CAS)
            .bind(generation.tenant_id)
            .bind(&generation.account_scope)
            .bind(generation.generation_id)
            .bind(generation.process_epoch)
            .bind(generation.source_audit_id)
            .bind(version)
            .fetch_optional(connection)
            .instrument(
                tracing::info_span!(target: "robson_operation", "operation_io", io_kind = 1_u64),
            )
            .await?;
        if advanced != Some(version) {
            return Err(AccountingRefusal::StaleGeneration);
        }
        Ok(())
    }
}

/// The projector's step: the bounds, then [`BalanceAccounting::advance`] (the
/// certificate's key, the bump, the CAS).
impl AccountingStep for BalanceAccounting {
    fn run<'c>(
        &'c self,
        connection: &'c mut PgConnection,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'c>> {
        Box::pin(async move {
            sqlx::query(BOUNDS)
                .execute(&mut *connection)
                .instrument(
                    tracing::info_span!(target: "robson_operation", "operation_io", io_kind = 1_u64),
                )
                .await
                .map_err(|error| error.to_string())?;
            self.advance(connection).await.map(drop).map_err(|error| error.to_string())
        })
    }
}

/// What every projector caller on a balance-account daemon passes.
pub(crate) fn projector_step(
    accounting: Option<&BalanceAccounting>,
) -> Option<&dyn AccountingStep> {
    accounting.map(|accounting| accounting as &dyn AccountingStep)
}
