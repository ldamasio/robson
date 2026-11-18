//! One finite test control, backed exclusively by the existing C3 identity.
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, Weak,
};

use tokio::{
    sync::{mpsc, watch},
    time::Instant,
};

use super::{
    DiagnosticEvent, DiagnosticIdentity, DiagnosticPhase, DiagnosticRecord, ProxyMode, Shared,
};
mod api;
pub(super) mod io;
pub use api::{ControlGate, ControlHit, ControlLease, ControlSnapshot};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControlBoundary {
    BeforeForward,
    BeforeCommandComplete,
    BeforeReady,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClientLoss {
    Read,
    Write,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControlError {
    Busy,
    InvalidIdentity,
    Incompatible,
    Closed,
    InvalidProof,
}
struct Plan {
    ordinal: u64,
    boundary: ControlBoundary,
    decision: Option<mpsc::Receiver<()>>,
    complete: Option<DiagnosticRecord>,
}
pub(super) struct State {
    pub identity: DiagnosticIdentity,
    pub deadline: Instant,
    pub cancelled: AtomicBool,
    shared: Weak<Shared>,
    orphan: bool,
    do_used: AtomicBool,
    plan: Mutex<Option<Plan>>,
    status: watch::Sender<ControlSnapshot>,
    #[cfg(test)]
    pub read_progress: std::sync::atomic::AtomicU64,
    #[cfg(test)]
    pub write_pause: Mutex<Option<super::control_tests::WritePause>>,
}
fn registry_matches(shared: &Shared, identity: DiagnosticIdentity) -> bool {
    shared
        .diagnostics
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|trace| trace.control_identity(identity.connection))
        == Some(identity)
}
pub(super) fn active(shared: &Shared, id: u64) -> Option<Arc<State>> {
    shared
        .control
        .lock()
        .unwrap()
        .as_ref()
        .filter(|state| state.identity.connection == id)
        .cloned()
}
impl State {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        self.status.send_modify(|_| {});
        if let Some(shared) = self.shared.upgrade() {
            shared.mode.send_modify(|_| {});
        }
    }
    pub fn invalid(&self) {
        self.cancelled.store(true, Ordering::Release);
        self.status.send_modify(|s| s.proof_valid = false);
        self.cancel();
    }
    pub fn check(&self, shared: &Shared, pending: u8) -> std::io::Result<()> {
        let pass = *shared.mode.borrow() == ProxyMode::Pass;
        if self.cancelled.load(Ordering::Acquire)
            || Instant::now() >= self.deadline
            || pending != 0
            || !pass
            || !registry_matches(shared, self.identity)
        {
            self.invalid();
            return Err(std::io::Error::other("invalid proxy control proof"));
        }
        let mut confirmed = false;
        self.status.send_if_modified(|s| {
            if self.cancelled.load(Ordering::Acquire) {
                return false;
            }
            confirmed = true;
            let changed = !s.proof_valid;
            s.proof_valid = true;
            changed
        });
        if confirmed && !self.cancelled.load(Ordering::Acquire) {
            Ok(())
        } else {
            Err(std::io::Error::other("cancelled proxy control"))
        }
    }
    pub fn client_loss(&self, shared: &Shared, pending: u8, loss: ClientLoss) -> bool {
        if self.check(shared, pending).is_err() {
            return false;
        }
        self.status.send_modify(|s| {
            if self.cancelled.load(Ordering::Acquire) {
                return;
            }
            if s.client_loss.is_none() || loss == ClientLoss::Write {
                s.client_loss = Some(loss);
            }
            s.orphan = self.orphan;
        });
        self.orphan && !self.cancelled.load(Ordering::Acquire)
    }
    pub fn orphaned(&self) -> bool {
        self.status.borrow().orphan
    }
    pub fn association_supported(&self, shared: &Shared, pending: u8, body: &[u8]) -> bool {
        self.check(shared, pending).is_ok()
            && canonical_do(body)
            && !self.do_used.swap(true, Ordering::AcqRel)
    }
    pub fn select(
        &self,
        tag: u8,
        record: Option<DiagnosticRecord>,
        frontend: bool,
        ready_ordinal: Option<u64>,
    ) -> Option<mpsc::Receiver<()>> {
        let record = record?;
        let mut slot = self.plan.lock().unwrap();
        if self.cancelled.load(Ordering::Acquire) {
            return None;
        }
        let plan = slot.as_mut().filter(|p| p.decision.is_some())?;
        let target = record.ordinal == Some(plan.ordinal);
        let mut complete = None;
        let hit = match plan.boundary {
            ControlBoundary::BeforeForward => {
                frontend
                    && target
                    && record.event == DiagnosticEvent::Sent
                    && matches!(record.phase, DiagnosticPhase::Simple | DiagnosticPhase::Execute)
            },
            ControlBoundary::BeforeCommandComplete => {
                !frontend
                    && target
                    && matches!(record.event, DiagnosticEvent::Complete | DiagnosticEvent::Error)
                    && matches!(tag, b'C' | b'E')
            },
            ControlBoundary::BeforeReady if !frontend => {
                if tag == b'C' {
                    if record.ordinal.is_none() || plan.complete.is_some() {
                        drop(slot);
                        self.invalid();
                        return None;
                    }
                    plan.complete = Some(record);
                }
                if matches!(tag, b'I' | b's' | b'E') {
                    drop(slot);
                    self.invalid();
                    return None;
                }
                if tag != b'Z' {
                    return None;
                }
                if plan.complete.as_ref().is_some_and(|record| record.ordinal != ready_ordinal) {
                    drop(slot);
                    self.invalid();
                    return None;
                }
                complete = plan.complete.take();
                complete.is_some_and(|c| c.ordinal == Some(plan.ordinal))
            },
            _ => false,
        };
        if !hit {
            return None;
        }
        let receiver = plan.decision.take();
        let hit = ControlHit {
            boundary: plan.boundary,
            record,
            complete,
        };
        self.status.send_modify(|s| s.held = Some(hit));
        drop(slot);
        receiver
    }
    pub fn finished(&self, selected: bool, forwarded: bool) {
        if selected {
            self.plan.lock().unwrap().take();
        }
        self.status.send_modify(|s| {
            s.forwarded = s.forwarded.saturating_add(u64::from(forwarded));
            if selected {
                s.held = None;
                s.armed = false;
                s.released = forwarded;
            }
        });
    }
    pub fn closed(&self) {
        #[cfg(test)]
        super::control_tests::clear_write_pause(self);
        // Plan -> status is the only nested control order; never trace or await.
        let mut plan = self.plan.lock().unwrap();
        self.cancelled.store(true, Ordering::Release);
        plan.take();
        self.status.send_modify(|s| {
            s.closed = true;
            s.orphan = false;
            s.armed = false;
            s.held = None;
        });
    }
}
pub(super) fn canonical_do(body: &[u8]) -> bool {
    let Some(sql) = body.strip_suffix(&[0]) else {
        return false;
    };
    if sql.len() > 16_384 || sql.contains(&0) {
        return false;
    }
    let Ok(sql) = std::str::from_utf8(sql) else {
        return false;
    };
    let Some(inner) = sql
        .trim()
        .strip_prefix("DO $migration38$")
        .and_then(|s| s.strip_suffix("$migration38$;"))
    else {
        return false;
    };
    !inner.contains("$migration38$")
}
