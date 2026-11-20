//! Real blocking jobs exercise the same two-slot completion/retirement helpers.
//! No DB, HTTP, fake readiness or substituted native clock.
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc,
};

use super::*;

fn identity(error: TimeError) -> TimeError {
    error
}

#[tokio::test]
async fn blocking_success_is_consumed_once_and_retirement_is_empty() {
    let mut slot = Some(tokio::task::spawn_blocking(|| Ok::<u8, TimeError>(7)));
    assert_eq!(
        complete(
            &mut slot,
            Instant::now() + Duration::from_secs(2),
            &CancellationToken::new(),
            identity
        )
        .await,
        Ok(7)
    );
    assert!(slot.is_none());
    assert!(retire(&mut slot).await, "completed output is never polled twice");
}
#[tokio::test]
async fn blocking_inner_error_clears_slot_before_error_propagation() {
    let mut slot = Some(tokio::task::spawn_blocking(|| Err::<u8, _>(TimeError::Payload)));
    assert_eq!(
        complete(
            &mut slot,
            Instant::now() + Duration::from_secs(2),
            &CancellationToken::new(),
            identity
        )
        .await,
        Err(TimeError::Payload)
    );
    assert!(slot.is_none());
    assert!(retire(&mut slot).await, "inner error cannot leave an already-consumed handle");
}
#[tokio::test]
async fn blocking_join_error_clears_slot_and_retirement_cannot_repoll() {
    let mut slot = Some(tokio::task::spawn_blocking(|| -> Result<u8, TimeError> {
        panic!("blocking_job_join_error_control")
    }));
    assert_eq!(
        complete(
            &mut slot,
            Instant::now() + Duration::from_secs(2),
            &CancellationToken::new(),
            identity
        )
        .await,
        Err(TimeError::Resource)
    );
    assert!(slot.is_none());
    assert!(retire(&mut slot).await, "JoinError is terminal consumption too");
}

fn held_job(
    workers: Arc<Semaphore>,
    finished: Arc<AtomicBool>,
) -> (
    JoinHandle<Result<u8, TimeError>>,
    tokio::sync::oneshot::Receiver<()>,
    mpsc::SyncSender<()>,
) {
    let permit = workers.try_acquire_owned().unwrap();
    let (start, started) = tokio::sync::oneshot::channel();
    let (release, wait) = mpsc::sync_channel(1);
    let job = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        start.send(()).unwrap();
        wait.recv().unwrap();
        finished.store(true, Ordering::Release);
        Ok(7)
    });
    (job, started, release)
}

#[tokio::test]
async fn blocking_timeout_retains_handle_and_permit_until_actual_retirement() {
    let workers = Arc::new(Semaphore::new(1));
    let finished = Arc::new(AtomicBool::new(false));
    let (job, started, release) = held_job(Arc::clone(&workers), Arc::clone(&finished));
    let mut slot = Some(job);
    started.await.unwrap();
    assert_eq!(
        complete(&mut slot, Instant::now(), &CancellationToken::new(), identity).await,
        Err(TimeError::Expired)
    );
    assert!(slot.is_some());
    assert!(!finished.load(Ordering::Acquire));
    assert_eq!(workers.available_permits(), 0);
    release.send(()).unwrap();
    assert!(retire(&mut slot).await);
    assert!(slot.is_none());
    assert!(finished.load(Ordering::Acquire));
    assert_eq!(workers.available_permits(), 1);
    assert!(retire(&mut slot).await, "retirement itself consumes only once");
}
#[tokio::test]
async fn blocking_cancellation_retains_pending_job_for_real_join() {
    let workers = Arc::new(Semaphore::new(1));
    let finished = Arc::new(AtomicBool::new(false));
    let (job, started, release) = held_job(Arc::clone(&workers), Arc::clone(&finished));
    let mut slot = Some(job);
    started.await.unwrap();
    let cancel = CancellationToken::new();
    let (result, ()) = tokio::join!(
        complete(&mut slot, Instant::now() + Duration::from_secs(2), &cancel, identity),
        async {
            tokio::task::yield_now().await;
            cancel.cancel();
        },
    );
    assert_eq!(result, Err(TimeError::Closing));
    assert!(slot.is_some());
    assert!(!finished.load(Ordering::Acquire));
    assert_eq!(workers.available_permits(), 0);
    release.send(()).unwrap();
    assert!(retire(&mut slot).await);
    assert!(slot.is_none());
    assert!(finished.load(Ordering::Acquire));
    assert_eq!(workers.available_permits(), 1);
}
