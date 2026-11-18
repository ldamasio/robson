//! Public lease/gate handles; a gate exclusively borrows its lease.
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use tokio::{
    sync::{mpsc, watch},
    time::Instant,
};

use super::{
    super::{DiagnosticIdentity, DiagnosticRecord, ProxyMode, Shared},
    registry_matches, ClientLoss, ControlBoundary, ControlError, Plan, State,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ControlHit {
    pub boundary: ControlBoundary,
    pub record: DiagnosticRecord,
    pub complete: Option<DiagnosticRecord>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ControlSnapshot {
    pub proof_valid: bool,
    pub closed: bool,
    pub armed: bool,
    pub held: Option<ControlHit>,
    pub orphan: bool,
    pub client_loss: Option<ClientLoss>,
    pub forwarded: u64,
    pub released: bool,
}
pub struct ControlLease {
    shared: Arc<Shared>,
    state: Arc<State>,
}
pub struct ControlGate<'a> {
    lease: &'a mut ControlLease,
    decision: mpsc::Sender<()>,
    done: bool,
}

impl ControlLease {
    #[cfg(test)]
    pub(in crate::db_proxy) fn gate_empty(&self) -> bool {
        self.state.plan.lock().unwrap().is_none()
    }
    pub(in crate::db_proxy) fn new(
        shared: &Arc<Shared>,
        identity: DiagnosticIdentity,
        orphan: bool,
    ) -> Result<Self, ControlError> {
        if *shared.mode.borrow() != ProxyMode::Pass {
            return Err(ControlError::Incompatible);
        }
        if !registry_matches(shared, identity) {
            return Err(ControlError::InvalidIdentity);
        }
        Self::publish(shared, identity, orphan)
    }

    pub(in crate::db_proxy) fn publish(
        shared: &Arc<Shared>,
        identity: DiagnosticIdentity,
        orphan: bool,
    ) -> Result<Self, ControlError> {
        let mut slot = shared.control.lock().unwrap();
        if slot.is_some() {
            return Err(ControlError::Busy);
        }
        let state = Arc::new(State {
            identity,
            deadline: Instant::now() + Duration::from_secs(6),
            cancelled: AtomicBool::new(false),
            shared: Arc::downgrade(shared),
            orphan,
            do_used: AtomicBool::new(false),
            plan: Mutex::new(None),
            status: watch::Sender::new(ControlSnapshot {
                proof_valid: false,
                closed: false,
                armed: false,
                held: None,
                orphan: false,
                client_loss: None,
                forwarded: 0,
                released: false,
            }),
            #[cfg(test)]
            read_progress: std::sync::atomic::AtomicU64::new(0),
            #[cfg(test)]
            write_pause: Mutex::new(None),
        });
        *slot = Some(state.clone());
        drop(slot);
        if !registry_matches(shared, identity) {
            state.invalid();
            let mut slot = shared.control.lock().unwrap();
            if slot.as_ref().is_some_and(|current| Arc::ptr_eq(current, &state)) {
                slot.take();
            }
            return Err(ControlError::InvalidIdentity);
        }
        shared.mode.send_modify(|_| {});
        Ok(Self { shared: shared.clone(), state })
    }

    pub fn arm(
        &mut self,
        ordinal: u64,
        boundary: ControlBoundary,
    ) -> Result<ControlGate<'_>, ControlError> {
        if !registry_matches(&self.shared, self.state.identity) {
            self.state.invalid();
            return Err(ControlError::InvalidIdentity);
        }
        self.arm_after_check(ordinal, boundary)
    }
    pub(in crate::db_proxy) fn arm_after_check(
        &mut self,
        ordinal: u64,
        boundary: ControlBoundary,
    ) -> Result<ControlGate<'_>, ControlError> {
        let mut plan = self.state.plan.lock().unwrap();
        if self.state.cancelled.load(Ordering::Acquire) || ordinal == 0 {
            return Err(ControlError::Closed);
        }
        if plan.is_some() {
            return Err(ControlError::Busy);
        }
        let (decision, receiver) = mpsc::channel(1);
        *plan = Some(Plan {
            ordinal,
            boundary,
            decision: Some(receiver),
            complete: None,
        });
        self.state.status.send_modify(|s| {
            s.armed = true;
            s.held = None;
            s.released = false;
        });
        drop(plan);
        Ok(ControlGate { lease: self, decision, done: false })
    }

    pub fn snapshot(&self) -> ControlSnapshot {
        *self.state.status.borrow()
    }

    pub async fn close(self) -> Result<ControlSnapshot, ControlError> {
        let mut status = self.state.status.subscribe();
        self.state.cancel();
        while !status.borrow_and_update().closed {
            status.changed().await.map_err(|_| ControlError::Closed)?;
        }
        let result = *status.borrow();
        Ok(result)
    }
}
impl Drop for ControlLease {
    fn drop(&mut self) {
        self.state.cancel();
    }
}
impl ControlGate<'_> {
    pub fn snapshot(&self) -> ControlSnapshot {
        self.lease.snapshot()
    }
    pub async fn held(&mut self) -> Result<ControlHit, ControlError> {
        let mut status = self.lease.state.status.subscribe();
        loop {
            let current = *status.borrow_and_update();
            if current.closed
                || !current.armed
                || self.lease.state.cancelled.load(Ordering::Acquire)
            {
                return Err(ControlError::Closed);
            }
            if let Some(hit) = current.held {
                if !registry_matches(&self.lease.shared, self.lease.state.identity) {
                    self.lease.state.invalid();
                    return Err(ControlError::InvalidIdentity);
                }
                return Ok(hit);
            }
            status.changed().await.map_err(|_| ControlError::Closed)?;
        }
    }
    pub async fn release(mut self) -> Result<(), ControlError> {
        self.held().await?;
        let mut status = self.lease.state.status.subscribe();
        self.decision.try_send(()).map_err(|_| ControlError::Closed)?;
        loop {
            let current = *status.borrow_and_update();
            if current.closed
                || !current.proof_valid
                || self.lease.state.cancelled.load(Ordering::Acquire)
            {
                return Err(ControlError::InvalidProof);
            }
            if !current.armed {
                if !current.released {
                    return Err(ControlError::Closed);
                }
                self.done = true;
                return Ok(());
            }
            status.changed().await.map_err(|_| ControlError::Closed)?;
        }
    }
}
impl Drop for ControlGate<'_> {
    fn drop(&mut self) {
        if !self.done {
            self.lease.state.cancel();
        }
    }
}
