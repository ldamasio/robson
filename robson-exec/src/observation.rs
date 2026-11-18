//! Observation only: never a permit, grant, deadline or financial decision.
use std::future::Future;

use tracing::{Instrument, Span};

tokio::task_local! { static RECONCILIATION: bool; }

/// Whether the current polled future belongs to the measured reconciliation.
pub fn reconciliation_active() -> bool {
    RECONCILIATION.try_with(|value| *value).unwrap_or(false)
}

/// Measure this scan, including refusal, under its existing journal permit.
pub async fn reconciliation<F: Future>(work: F) -> F::Output {
    RECONCILIATION
        .scope(
            true,
            work.instrument(tracing::info_span!(
                target: "robson_operation", "reconciliation_work",
                outcome = tracing::field::Empty
            )),
        )
        .await
}

/// Mark ambient causal attribution incomplete without changing any authority.
pub(crate) fn incomplete_current() {
    if reconciliation_active() {
        tracing::info_span!(target: "robson_operation", "operation_io", io_kind = 7_u64)
            .in_scope(|| {});
    }
}

/// Capture context before a caller hands an existing task to Tokio.
pub fn inherit_reconciliation<F: Future>(work: F) -> impl Future<Output = F::Output> {
    let origin = Origin::capture();
    async move { origin.run(work).await }
}

/// An origin follows existing bounded work; it never owns business authority.
#[derive(Clone, Debug)]
pub(crate) struct Origin {
    span: Span,
    measured: bool,
}
impl Origin {
    pub(crate) fn capture() -> Self {
        Self {
            span: Span::current(),
            measured: reconciliation_active(),
        }
    }
    pub(crate) fn measured() -> Option<Self> {
        reconciliation_active().then(Self::capture)
    }
    pub(crate) fn incomplete(&self) {
        tracing::info_span!(target: "robson_operation", parent: &self.span, "operation_io", io_kind = 7_u64).in_scope(|| {});
    }
    /// A coalesced row has no single causal owner. Keep at most its first
    /// reference, mark both observations incomplete, and never multiply IO.
    pub(crate) fn merge(slot: &mut Option<Self>, incoming: Option<Self>) {
        match (slot.as_ref(), incoming.as_ref()) {
            (Some(old), Some(new)) if old.span.id() == new.span.id() => {},
            (Some(old), other) => {
                old.incomplete();
                if let Some(new) = other {
                    new.incomplete();
                }
            },
            (None, Some(new)) => {
                new.incomplete();
                *slot = incoming;
            },
            (None, None) => {},
        }
    }
    pub(crate) async fn run<F: Future>(&self, work: F) -> F::Output {
        if !self.measured {
            incomplete_current();
        }
        let span = if self.measured {
            self.span.clone()
        } else {
            // Span::none would inherit the caller's entered span. An explicit
            // parentless barrier prevents both causal and transport laundering.
            tracing::info_span!(target: "robson_operation", parent: None, "unmeasured_work")
        };
        RECONCILIATION.scope(self.measured, work.instrument(span)).await
    }
}
