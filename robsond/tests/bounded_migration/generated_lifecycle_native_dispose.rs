//! Native owner retirement and shared-pool disposal for generated runs.
use super::*;

pub(crate) async fn dispose(
    active: Option<Fixture>,
    parked: Option<Parked>,
    pool: PgPool,
    end: tokio::time::Instant,
) -> Result<()> {
    let retirement = dispose_resources(active, parked).await;
    let closed = tokio::time::timeout_at(end, pool.close())
        .await
        .context("generated shared cleanup5s pool close");
    match (retirement, closed) {
        (Ok(()), Ok(())) => Ok(()),
        (a, b) => Err(anyhow::anyhow!("generated cleanup native={} pool={}", cause(&a), cause(&b))),
    }
}

/// Native owner retirement and observer/proxy disposal without closing the
/// shared pool, so a generated run can start its next case in the same
/// database.
pub(crate) async fn dispose_resources(
    active: Option<Fixture>,
    parked: Option<Parked>,
) -> Result<()> {
    let retirement = if let Some(f) = active {
        match park(f).await {
            Ok(p) => {
                drop(p.observer);
                p.proxy.set_mode(ProxyMode::Sever);
                drop(p.proxy);
                Ok(())
            },
            Err(e) => Err(e),
        }
    } else {
        Ok(())
    };
    if let Some(p) = parked {
        drop(p.observer);
        p.proxy.set_mode(ProxyMode::Sever);
        drop(p.proxy);
    }
    retirement
}
