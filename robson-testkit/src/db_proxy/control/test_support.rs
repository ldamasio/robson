//! Private cfg(test) synchronization and subcases; no production authority.
use std::{
    io,
    pin::Pin,
    sync::{atomic::AtomicU8, Weak},
    task::{Context, Poll},
};

use crate::db_proxy::{control_tests::*, *};

// Only bounded synchronization lives in State; the writer owns no frame.
pub(in crate::db_proxy) struct WritePause {
    pub(in crate::db_proxy) progress: watch::Sender<usize>,
    pub(in crate::db_proxy) proceed: tokio::sync::mpsc::Receiver<()>,
}
pub(in crate::db_proxy) fn install_write_pause(
    state: &control::State,
) -> Result<(watch::Receiver<usize>, tokio::sync::mpsc::Sender<()>), &'static str> {
    let (progress, changed) = watch::channel(0);
    let (proceed, receiver) = tokio::sync::mpsc::channel(1);
    let mut slot = state.write_pause.lock().unwrap();
    if state.cancelled.load(Ordering::Acquire) || slot.is_some() {
        return Err("closed or occupied write pause");
    }
    *slot = Some(WritePause { progress, proceed: receiver });
    Ok((changed, proceed))
}
pub(in crate::db_proxy) fn clear_write_pause(state: &control::State) {
    state.cancelled.store(true, Ordering::Release);
    state.write_pause.lock().unwrap().take();
}
pub(in crate::db_proxy) struct PartialWriter<W> {
    pub inner: W,
    pause: Option<WritePause>,
    accepted: usize,
}
impl<W> PartialWriter<W> {
    pub(in crate::db_proxy) fn new(inner: W, pause: Option<WritePause>) -> Self {
        Self { inner, pause, accepted: 0 }
    }
}
impl<W: AsyncWrite + Unpin> AsyncWrite for PartialWriter<W> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        if let Some(pause) = this.pause.as_mut() {
            if this.accepted < 2 {
                let bound = bytes.len().min(2 - this.accepted);
                let written =
                    std::task::ready!(Pin::new(&mut this.inner).poll_write(cx, &bytes[..bound]))?;
                this.accepted += written;
                pause.progress.send_replace(this.accepted);
                return Poll::Ready(Ok(written));
            }
            match pause.proceed.poll_recv(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Some(())) => this.pause = None,
                Poll::Ready(None) => return Poll::Ready(Err(io::ErrorKind::BrokenPipe.into())),
            }
        }
        Pin::new(&mut this.inner).poll_write(cx, bytes)
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

// Outcome: 0=not observed, 1=not Closed, 2=Closed early, 3=State disappeared.
pub(in crate::db_proxy) struct CloseObservation {
    connection: u64,
    state: Weak<control::State>,
    outcome: Arc<AtomicU8>,
}
pub(in crate::db_proxy) struct SocketDropProbe<'a>(
    pub(in crate::db_proxy) &'a Shared,
    pub(in crate::db_proxy) u64,
);
impl Drop for SocketDropProbe<'_> {
    fn drop(&mut self) {
        let observation = {
            let mut slot = self.0.close_observation.lock().unwrap();
            if slot.as_ref().is_some_and(|probe| probe.connection == self.1) {
                slot.take()
            } else {
                None
            }
        };
        if let Some(observation) = observation {
            // The slot lock has ended; neither trace nor control is locked here.
            let value = observation.state.upgrade().map_or(3, |state| {
                if state.status.borrow().closed {
                    2
                } else {
                    1
                }
            });
            observation.outcome.store(value, Ordering::Release);
        }
    }
}
pub(in crate::db_proxy) async fn closed_is_observed_after_socket_owners() {
    let (proxy, _client, mut server) = pair().await;
    let observed = identity(&proxy);
    let lease = proxy.control(observed, false).unwrap();
    let state = control::active(&proxy.shared, observed.connection).unwrap();
    let outcome = Arc::new(AtomicU8::new(0));
    let previous = proxy.shared.close_observation.lock().unwrap().replace(CloseObservation {
        connection: observed.connection,
        state: Arc::downgrade(&state),
        outcome: outcome.clone(),
    });
    assert!(previous.is_none());
    assert!(lease.close().await.unwrap().closed);
    let result = outcome.load(Ordering::Acquire);
    assert_ne!(result, 0, "socket_owner_drop_probe_was_not_observed");
    assert_eq!(result, 1, "closed_published_before_socket_owner_drop");
    assert!(proxy.shared.close_observation.lock().unwrap().is_none());
    assert_eq!(server.read(&mut [0]).await.unwrap(), 0);
    closed(&proxy).await;
}

async fn another_pair(proxy: &DbProxy, listener: &TcpListener) -> (TcpStream, TcpStream) {
    let mut client = TcpStream::connect(proxy.addr()).await.unwrap();
    client.set_nodelay(true).unwrap();
    client.write_all(&[0, 0, 0, 8, 0, 3, 0, 0]).await.unwrap();
    let server = accept(listener).await;
    for tag in [b'R', b'K', b'Z'] {
        assert_eq!(recv(&mut client).await.0, tag);
    }
    (client, server)
}
pub(in crate::db_proxy) async fn global_phase_overflow_refuses_other_connection() {
    let (proxy, listener) = listening().await;
    let (mut client_a, mut server_a) = another_pair(&proxy, &listener).await;
    let a = identity(&proxy);
    let (client_b, mut server_b) = another_pair(&proxy, &listener).await;
    let b = proxy
        .diagnostic_identities()
        .unwrap()
        .into_iter()
        .find(|identity| identity.connection != a.connection)
        .unwrap();
    assert!(b.live && !b.attribution_unknown);
    for _ in 0..33 {
        send(&mut client_a, &[(b'P', b"s\0UPDATE fixture\0\0\0")]).await;
        assert_eq!(recv(&mut server_a).await.0, b'P');
    }
    let trace = proxy.diagnostics().unwrap();
    assert!(trace.phase_overflow > 0);
    assert_eq!(trace.dropped, 0);
    assert_eq!(trace.connection_overflow, 0);
    let current_b = proxy
        .diagnostic_identities()
        .unwrap()
        .into_iter()
        .find(|identity| identity.connection == b.connection)
        .unwrap();
    assert_eq!(current_b, b);
    assert!(current_b.live && !current_b.attribution_unknown);
    assert!(
        matches!(proxy.control(current_b, false), Err(ControlError::InvalidIdentity)),
        "global_phase_overflow_allowed_unrelated_connection"
    );
    drop((client_a, client_b));
    assert_eq!(server_a.read(&mut [0]).await.unwrap(), 0);
    assert_eq!(server_b.read(&mut [0]).await.unwrap(), 0);
    closed(&proxy).await;
}
