//! Coherent read of a persisted order-set generation identity.
//! Exact source and writer qualification belongs to the order-set owner.
use robson_exec::{order_journal::IntentRow, venue::VenueId};
use sqlx::PgConnection;
use tracing::Instrument as _;
use uuid::Uuid;

mod cold_source;

/// Private opt-in owner and structural binding for journal maintenance. Only
/// an isolated integration supplies one; it confers no readiness.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct JournalGeneration {
    pub(crate) tenant_id: Uuid,
    pub(crate) account_scope: String,
    pub(crate) generation_id: Uuid,
    pub(crate) process_epoch: Uuid,
    pub(crate) source_audit_id: Uuid,
}

impl JournalGeneration {
    pub(crate) fn is_valid(&self) -> bool {
        !(self.account_scope.trim().is_empty()
            || self.tenant_id.is_nil()
            || self.generation_id.is_nil()
            || self.process_epoch.is_nil()
            || self.source_audit_id.is_nil())
    }
}

const JOURNAL_UNBOUND: &str = include_str!("journal_unbound.sql");

/// One statement on the caller's transaction, after its original intent and
/// attempt effects and under its accounting lock. `version` is the one that
/// transaction's accounting write returned. No row is a refusal.
pub(crate) async fn maintain_unbound_journal(
    connection: &mut PgConnection,
    generation: &JournalGeneration,
    version: i64,
    intent: &IntentRow,
    prior_member_expected: bool,
) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query_scalar::<_, i64>(JOURNAL_UNBOUND)
        .bind(generation.tenant_id)
        .bind(&generation.account_scope)
        .bind(generation.generation_id)
        .bind(generation.process_epoch)
        .bind(generation.source_audit_id)
        .bind(version)
        .bind(intent.intent_id)
        .bind(&intent.client_order_id)
        .bind(intent.position_id)
        .bind(intent.kind.as_str())
        .bind(prior_member_expected)
        .fetch_optional(connection)
        .instrument(
            tracing::info_span!(target: "robson_operation", "operation_io", io_kind = 1_u64),
        )
        .await
}

#[derive(Clone, Copy)]
pub(crate) struct ExpectedGeneration<'a> {
    pub(crate) tenant_id: Uuid,
    pub(crate) venue: &'a VenueId,
    pub(crate) account_scope: &'a str,
    pub(crate) generation_id: Uuid,
    pub(crate) process_epoch: Uuid,
    pub(crate) source_audit_id: Uuid,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct ObservedGeneration {
    pub(crate) generation_id: Uuid,
    pub(crate) process_epoch: Uuid,
    pub(crate) source_audit_id: Uuid,
    pub(crate) accounting_version: i64,
}

const READ: &str = "SELECT g.generation_id,g.process_epoch,g.source_audit_id,g.accounting_version
    FROM public.spot_order_set_generation g
    JOIN public.accounting_state a
      ON (a.tenant_id,a.venue_id)=(g.tenant_id,g.venue_id)
    WHERE (g.tenant_id,g.venue_id,g.account_scope)=($1,$2,$3)
      AND (g.generation_id,g.process_epoch,g.source_audit_id)=($4,$5,$6)
      AND g.accounting_version=a.version";

pub(crate) async fn observe_generation(
    connection: &mut PgConnection,
    expected: ExpectedGeneration<'_>,
) -> Result<Option<ObservedGeneration>, sqlx::Error> {
    if expected.venue.as_str() != VenueId::BINANCE_SPOT
        || expected.account_scope.trim().is_empty()
        || expected.tenant_id.is_nil()
        || expected.generation_id.is_nil()
        || expected.process_epoch.is_nil()
        || expected.source_audit_id.is_nil()
    {
        return Ok(None);
    }
    let row = sqlx::query_as::<_, (Uuid, Uuid, Uuid, i64)>(READ)
        .bind(expected.tenant_id)
        .bind(expected.venue.as_str())
        .bind(expected.account_scope)
        .bind(expected.generation_id)
        .bind(expected.process_epoch)
        .bind(expected.source_audit_id)
        .fetch_optional(connection)
        .await?;
    Ok(row.map(|(generation_id, process_epoch, source_audit_id, accounting_version)| {
        ObservedGeneration {
            generation_id,
            process_epoch,
            source_audit_id,
            accounting_version,
        }
    }))
}

#[cfg(test)]
mod tests {
    use anyhow::{ensure, Result};
    use sqlx::{ConnectOptions, PgPool};

    use super::*;

    #[sqlx::test(migrations = "../migrations")]
    #[ignore = "requires isolated PostgreSQL DATABASE_URL"]
    async fn persisted_order_set_generation_requires_the_exact_current_binding(
        pool: PgPool,
    ) -> Result<()> {
        let venue = VenueId::new(VenueId::BINANCE_SPOT);
        let wrong_venue = VenueId::new("binance:usdm");
        let binding = ExpectedGeneration {
            tenant_id: Uuid::now_v7(),
            venue: &venue,
            account_scope: "fixture-generation",
            generation_id: Uuid::now_v7(),
            process_epoch: Uuid::now_v7(),
            source_audit_id: Uuid::now_v7(),
        };
        // Structural native fixture: no cold source or writer authority is issued.
        let version: i64 = sqlx::query_scalar(
            "INSERT INTO accounting_state (tenant_id,venue_id,budget_model)
             VALUES ($1,$2,'net_from_start_non_expanding_v1') RETURNING version",
        )
        .bind(binding.tenant_id)
        .bind(binding.venue.as_str())
        .fetch_one(&pool)
        .await?;

        let mut connection = pool.connect_options().connect().await?;
        ensure!(observe_generation(&mut connection, binding).await?.is_none(), "absent row");
        sqlx::query(
            "INSERT INTO spot_order_set_generation (tenant_id,venue_id,account_scope,generation_id)
             VALUES ($1,$2,$3,$4)",
        )
        .bind(binding.tenant_id)
        .bind(binding.venue.as_str())
        .bind(binding.account_scope)
        .bind(binding.generation_id)
        .execute(&pool)
        .await?;
        ensure!(observe_generation(&mut connection, binding).await?.is_none(), "unbound row");
        drop(connection);

        sqlx::query(
            "UPDATE spot_order_set_generation
             SET process_epoch=$4,source_audit_id=$5,accounting_version=$6
             WHERE (tenant_id,venue_id,account_scope)=($1,$2,$3)",
        )
        .bind(binding.tenant_id)
        .bind(binding.venue.as_str())
        .bind(binding.account_scope)
        .bind(binding.process_epoch)
        .bind(binding.source_audit_id)
        .bind(version)
        .execute(&pool)
        .await?;
        // Fresh native backend supplies affirmative structural observation.
        let mut connection = pool.connect_options().connect().await?;
        ensure!(
            observe_generation(&mut connection, binding).await?
                == Some(ObservedGeneration {
                    generation_id: binding.generation_id,
                    process_epoch: binding.process_epoch,
                    source_audit_id: binding.source_audit_id,
                    accounting_version: version,
                }),
            "persisted exact same-snapshot identity"
        );
        for different in [
            ExpectedGeneration { tenant_id: Uuid::now_v7(), ..binding },
            ExpectedGeneration { venue: &wrong_venue, ..binding },
            ExpectedGeneration {
                account_scope: "other-account",
                ..binding
            },
            ExpectedGeneration { account_scope: "", ..binding },
            ExpectedGeneration { generation_id: Uuid::now_v7(), ..binding },
            ExpectedGeneration { process_epoch: Uuid::now_v7(), ..binding },
            ExpectedGeneration {
                source_audit_id: Uuid::now_v7(),
                ..binding
            },
            ExpectedGeneration { source_audit_id: Uuid::nil(), ..binding },
        ] {
            ensure!(
                observe_generation(&mut connection, different).await?.is_none(),
                "another tenant, route, generation or source remains unavailable"
            );
        }
        sqlx::query(
            "UPDATE accounting_state SET version=version+1 WHERE (tenant_id,venue_id)=($1,$2)",
        )
        .bind(binding.tenant_id)
        .bind(binding.venue.as_str())
        .execute(&pool)
        .await?;
        ensure!(
            observe_generation(&mut connection, binding).await?.is_none(),
            "original identity is stale at the actual new accounting version"
        );
        Ok(())
    }
}
