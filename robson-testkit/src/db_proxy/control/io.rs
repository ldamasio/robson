//! Frame ownership never moves into control state or the report channel.
use std::{
    io,
    sync::{
        atomic::{AtomicU8, Ordering},
        Arc,
    },
};

use tokio::{
    io::{AsyncReadExt, AsyncWrite},
    net::tcp::OwnedReadHalf,
};

use super::{
    super::{diagnostic, DiagnosticDelivery, DiagnosticRecord, Shared},
    active, ClientLoss, State,
};

#[cfg(test)]
#[path = "test_support.rs"]
pub(in crate::db_proxy) mod test_support;

pub(in crate::db_proxy) struct Frames<'a, 'trace> {
    pub shared: &'a Shared,
    pub pending: &'a AtomicU8,
    pub diagnostic: Option<&'a diagnostic::Connection<'trace>>,
    pub id: u64,
}
impl Frames<'_, '_> {
    pub async fn read(
        &self,
        stream: &mut OwnedReadHalf,
        frontend: bool,
    ) -> io::Result<(u8, Vec<u8>)> {
        let result = async {
            let tag = stream.read_u8().await?;
            let length = stream.read_i32().await?;
            let state = active(self.shared, self.id);
            if let Some(state) = state.as_ref() {
                if !(4..=65_535).contains(&length) {
                    state.invalid();
                    return Err(io::Error::other("controlled frame exceeds 64 KiB"));
                }
            }
            let mut body = vec![0; (length - 4).max(0) as usize];
            #[cfg(test)]
            super::super::control_tests::read_body(stream, &mut body, state, frontend).await?;
            #[cfg(not(test))]
            stream.read_exact(&mut body).await?;
            Ok((tag, body))
        }
        .await;
        if result.is_err() && frontend {
            if let Some(state) = active(self.shared, self.id) {
                state.client_loss(
                    self.shared,
                    self.pending.load(Ordering::SeqCst),
                    ClientLoss::Read,
                );
            }
        }
        result
    }

    pub async fn forward<W: AsyncWrite + Unpin>(
        &self,
        stream: &mut W,
        tag: u8,
        body: &[u8],
        record: Option<DiagnosticRecord>,
        frontend: bool,
    ) -> io::Result<()> {
        let Some(state) = active(self.shared, self.id) else {
            return super::super::forward_frontend(stream, tag, body, self.diagnostic, record)
                .await;
        };
        state.check(self.shared, self.pending.load(Ordering::SeqCst))?;
        if body.len() > 65_531 {
            state.invalid();
            return Err(io::Error::other("controlled frame exceeds 64 KiB"));
        }
        let decision = if !frontend && state.orphaned() {
            None
        } else {
            let ready_ordinal = if !frontend && tag == b'Z' {
                self.shared
                    .diagnostics
                    .lock()
                    .unwrap()
                    .as_ref()
                    .and_then(|trace| trace.control_ready_ordinal(self.id))
            } else {
                None
            };
            state.select(tag, record, frontend, ready_ordinal)
        };
        let mut frame = Frame {
            state: state.clone(),
            diagnostic: self.diagnostic,
            record,
            selected: decision.is_some(),
            done: false,
        };
        if let Some(mut decision) = decision {
            frame.mark(DiagnosticDelivery::Held);
            let mut status = state.status.subscribe();
            loop {
                if state.cancelled.load(Ordering::Acquire) {
                    return Err(io::Error::other("cancelled control gate"));
                }
                if !frontend && state.orphaned() {
                    return Ok(());
                }
                tokio::select! {
                    released = decision.recv() => { if released.is_none() { return Err(io::Error::other("closed control gate")); } break; },
                    changed = status.changed() => { changed.map_err(io::Error::other)?; },
                }
            }
        }
        state.check(self.shared, self.pending.load(Ordering::SeqCst))?;
        if !frontend && state.orphaned() {
            return Ok(());
        }
        #[cfg(test)]
        let pause = if frame.selected && frontend {
            state.write_pause.lock().unwrap().take()
        } else {
            None
        };
        #[cfg(test)]
        let mut writer = super::super::control_tests::PartialWriter::new(stream, pause);
        #[cfg(test)]
        let stream = &mut writer;
        if let Err(error) = super::super::write_message(stream, tag, body).await {
            if !frontend
                && state.client_loss(
                    self.shared,
                    self.pending.load(Ordering::SeqCst),
                    ClientLoss::Write,
                )
            {
                return Ok(());
            }
            return Err(error);
        }
        frame.mark(DiagnosticDelivery::Forwarded);
        frame.done = true;
        let valid = state.check(self.shared, self.pending.load(Ordering::SeqCst));
        state.finished(frame.selected && valid.is_ok(), true);
        valid?;
        // Cooperate with the absolute deadline even under continuously ready I/O.
        tokio::task::yield_now().await;
        Ok(())
    }
}
struct Frame<'a, 'trace> {
    state: Arc<State>,
    diagnostic: Option<&'a diagnostic::Connection<'trace>>,
    record: Option<DiagnosticRecord>,
    selected: bool,
    done: bool,
}
impl Frame<'_, '_> {
    fn mark(&self, delivery: DiagnosticDelivery) {
        if let (Some(trace), Some(record)) = (self.diagnostic, self.record) {
            trace.delivery(record, delivery);
        }
    }
}
impl Drop for Frame<'_, '_> {
    fn drop(&mut self) {
        if !self.done {
            self.mark(DiagnosticDelivery::Dropped);
            self.state.finished(self.selected, false);
        }
    }
}
