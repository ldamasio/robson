//! Actual public reqwest TIME response path. No private driver witness or
//! qualified response constructor; application cancellation owns this future.
use std::{
    future::{poll_fn, Future},
    pin::Pin,
    task::Poll,
};

use encoding_rs::{Encoding, UTF_8};
use robson_connectors::binance::spot::PreparedSpotTime;
use robson_exec::{clock_records::RawClockCapture, venue_clock::NativeClock};
use serde::Deserialize;
use tokio::time::{timeout_at, Instant};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::venue_budget::{
    state::{PendingTime, SharedBudget},
    BudgetError,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TimeError {
    Budget(BudgetError),
    Expired,
    Closing,
    Transport,
    Storage(crate::venue_clock_store::StoreError),
    Native,
    Resource,
    Payload,
    Venue,
}

/// A TIME answer is tens of bytes. A body over this size is refused before it
/// is accumulated, so a broken or hostile intermediary cannot grow memory, and
/// decoding then works on at most this many bytes.
pub(crate) const MAX_TIME_BODY_BYTES: usize = 64 * 1024;

fn append_bounded(bytes: &mut Vec<u8>, chunk: &[u8]) -> Result<(), TimeError> {
    let length = bytes.len().checked_add(chunk.len()).ok_or(TimeError::Resource)?;
    if length > MAX_TIME_BODY_BYTES {
        return Err(TimeError::Resource);
    }
    bytes.try_reserve(chunk.len()).map_err(|_| TimeError::Resource)?;
    bytes.extend_from_slice(chunk);
    Ok(())
}

pub(crate) struct CompleteBody {
    bytes: Vec<u8>,
    content_type: Option<String>,
    pub(crate) receive: RawClockCapture,
    success: bool,
}

pub(crate) async fn receive(
    prepared: PreparedSpotTime,
    budget: &SharedBudget,
    pending: PendingTime,
    clock: &NativeClock,
    epoch: Uuid,
    deadline: Instant,
    cancel: &CancellationToken,
) -> Result<CompleteBody, TimeError> {
    let work = async {
        let mut sending = Box::pin(prepared.send());
        let mut first = Some(pending);
        let response = poll_fn(|cx| {
            if cancel.is_cancelled() {
                return Poll::Ready(Err(TimeError::Closing));
            }
            if Instant::now() >= deadline {
                return Poll::Ready(Err(TimeError::Expired));
            }
            budget
                .poll_send(&mut first, clock, epoch, || Pin::as_mut(&mut sending).poll(cx))
                .map_err(TimeError::Budget)
                .map(|next| next.map(|answer| answer.map_err(|_| TimeError::Transport)))
                .unwrap_or_else(|error| Poll::Ready(Err(error)))
        })
        .await?;
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let success = response.status().is_success();
        let mut response = response;
        let mut bytes = Vec::new();
        loop {
            if cancel.is_cancelled() {
                return Err(TimeError::Closing);
            }
            if Instant::now() >= deadline {
                return Err(TimeError::Expired);
            }
            let Some(chunk) = response.chunk().await.map_err(|_| TimeError::Transport)? else {
                break;
            };
            append_bounded(&mut bytes, &chunk)?;
        }
        let receive = clock.wall_bracket(epoch).map_err(|_| TimeError::Native)?;
        if Instant::now() >= deadline {
            return Err(TimeError::Expired);
        }
        Ok(CompleteBody { bytes, content_type, receive, success })
    };
    tokio::select! {
        biased;
        _=cancel.cancelled()=>Err(TimeError::Closing),
        answer=timeout_at(deadline,work)=>answer.map_err(|_|TimeError::Expired)?,
    }
}

#[derive(Deserialize)]
struct TimePayload {
    #[serde(rename = "serverTime")]
    server_ms: i64,
}

impl CompleteBody {
    /// Called in the concrete owner's bounded, joined parser job. The native
    /// full-body mark already exists before any charset/JSON conversion.
    pub(crate) fn decode(self) -> Result<(RawClockCapture, i64), TimeError> {
        if !self.success {
            return Err(TimeError::Venue);
        }
        let mime = self.content_type.as_deref().and_then(|value| value.parse::<mime::Mime>().ok());
        let encoding = mime
            .as_ref()
            .and_then(|value| value.get_param("charset"))
            .and_then(|value| Encoding::for_label(value.as_str().as_bytes()))
            .unwrap_or(UTF_8);
        let (encoding, bytes) = match Encoding::for_bom(&self.bytes) {
            Some((actual, length)) => (actual, &self.bytes[length..]),
            None => (encoding, self.bytes.as_slice()),
        };
        let mut decoder = encoding.new_decoder_without_bom_handling();
        let capacity = decoder.max_utf8_buffer_length(bytes.len()).ok_or(TimeError::Resource)?;
        let mut text = String::new();
        text.try_reserve_exact(capacity).map_err(|_| TimeError::Resource)?;
        let (result, read, _) = decoder.decode_to_string(bytes, &mut text, true);
        if result != encoding_rs::CoderResult::InputEmpty || read != bytes.len() {
            return Err(TimeError::Payload);
        }
        let payload: TimePayload = serde_json::from_str(&text).map_err(|_| TimeError::Payload)?;
        if payload.server_ms < 0 {
            return Err(TimeError::Payload);
        }
        Ok((self.receive, payload.server_ms))
    }
}

#[cfg(test)]
#[path = "venue_clock_http_tests.rs"]
mod tests;
