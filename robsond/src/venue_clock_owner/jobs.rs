//! Only the concrete owner's two blocking-job slots. Completed outputs are
//! consumed exactly once; timeout/cancellation preserves pending ownership.
use super::*;

pub(super) async fn complete<T, E>(
    slot: &mut Option<JoinHandle<Result<T, E>>>,
    deadline: Instant,
    cancel: &CancellationToken,
    classify: fn(E) -> TimeError,
) -> Result<T, TimeError> {
    let job = slot.as_mut().ok_or(TimeError::Resource)?;
    let output = tokio::select! {
        biased;
        _=cancel.cancelled()=>return Err(TimeError::Closing),
        result=timeout_at(deadline,job)=>match result {
            Ok(output)=>output,
            Err(_)=>return Err(TimeError::Expired),
        },
    };
    // Even JoinErr and inner Err have consumed the output. Clear BEFORE
    // propagating either; retirement must never poll that handle again.
    *slot = None;
    let value = output.map_err(|_| TimeError::Resource)?.map_err(classify)?;
    if Instant::now() >= deadline {
        return Err(TimeError::Expired);
    }
    Ok(value)
}

pub(super) async fn retire<T, E>(slot: &mut Option<JoinHandle<Result<T, E>>>) -> bool {
    let Some(job) = slot.as_mut() else {
        return true;
    };
    let result = job.await;
    *slot = None;
    result.is_ok()
}

#[cfg(test)]
mod tests;
