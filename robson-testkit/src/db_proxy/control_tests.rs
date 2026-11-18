//! Scripted loopback peers only: these tests never connect to PostgreSQL.
use std::{future::Future, io, sync::atomic::Ordering, time::Duration};

use sqlx::{
    postgres::{PgConnectOptions, PgSslMode},
    Connection, Executor, PgConnection,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

use super::*;

pub(super) async fn bounded<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(8), future)
        .await
        .expect("control self-test hung")
}
pub(super) fn packet(tag: u8, body: &[u8]) -> Vec<u8> {
    let mut bytes = vec![tag];
    bytes.extend_from_slice(&(body.len() as i32 + 4).to_be_bytes());
    bytes.extend_from_slice(body);
    bytes
}
pub(super) async fn recv(stream: &mut TcpStream) -> (u8, Vec<u8>) {
    let tag = stream.read_u8().await.unwrap();
    let length = stream.read_i32().await.unwrap();
    assert!((4..=65_535).contains(&length));
    let mut body = vec![0; length as usize - 4];
    stream.read_exact(&mut body).await.unwrap();
    (tag, body)
}
pub(super) async fn send(stream: &mut TcpStream, messages: &[(u8, &[u8])]) {
    let bytes: Vec<_> = messages.iter().flat_map(|(tag, body)| packet(*tag, body)).collect();
    stream.write_all(&bytes).await.unwrap();
}
pub(super) async fn listening() -> (DbProxy, TcpListener) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy = DbProxy::start(listener.local_addr().unwrap()).await.unwrap();
    proxy.enable_diagnostics();
    (proxy, listener)
}
pub(super) async fn accept(listener: &TcpListener) -> TcpStream {
    let (mut server, _) = listener.accept().await.unwrap();
    server.set_nodelay(true).unwrap();
    let length = server.read_i32().await.unwrap();
    let mut startup = vec![0; length as usize - 4];
    server.read_exact(&mut startup).await.unwrap();
    send(&mut server, &[
        (b'R', &[0; 4]),
        (b'K', &[0, 0, 0, 42, 0, 0, 0, 0]),
        (b'Z', b"I"),
    ])
    .await;
    server
}
pub(super) async fn pair() -> (DbProxy, TcpStream, TcpStream) {
    let (proxy, listener) = listening().await;
    let mut client = TcpStream::connect(proxy.addr()).await.unwrap();
    client.set_nodelay(true).unwrap();
    client.write_all(&[0, 0, 0, 8, 0, 3, 0, 0]).await.unwrap();
    let server = accept(&listener).await;
    for tag in [b'R', b'K', b'Z'] {
        assert_eq!(recv(&mut client).await.0, tag);
    }
    (proxy, client, server)
}
pub(super) fn identity(proxy: &DbProxy) -> DiagnosticIdentity {
    proxy.diagnostic_identities().unwrap()[0]
}
pub(super) async fn closed(proxy: &DbProxy) {
    tokio::time::timeout(Duration::from_secs(1), async {
        while proxy.shared.control.lock().unwrap().is_some()
            || proxy.diagnostic_identities().unwrap().iter().any(|i| i.live)
        {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("relay/socket cleanup did not finish");
}

pub(super) async fn read_body<R: AsyncRead + Unpin>(
    reader: &mut R,
    body: &mut [u8],
    state: Option<Arc<control::State>>,
    frontend: bool,
) -> io::Result<()> {
    let mut filled = 0;
    while filled < body.len() {
        let read = reader.read(&mut body[filled..]).await?;
        if read == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        filled += read;
        if !frontend {
            if let Some(state) = state.as_ref() {
                state.read_progress.fetch_add(read as u64, Ordering::Release);
            }
        }
    }
    Ok(())
}
pub(super) use super::control_test_support::{
    clear_write_pause, install_write_pause, PartialWriter, WritePause,
};

#[tokio::test]
async fn sqlx_simple_and_prepared_stay_pending_at_each_real_gate() {
    bounded(async {
        for prepared in [false, true] {
            for boundary in [
                ControlBoundary::BeforeForward,
                ControlBoundary::BeforeCommandComplete,
                ControlBoundary::BeforeReady,
            ] {
                driver_gate(prepared, boundary, false).await;
            }
        }
        for prepared in [false, true] {
            driver_gate(prepared, ControlBoundary::BeforeCommandComplete, true).await;
        }
    })
    .await;
}
async fn driver_gate(prepared: bool, boundary: ControlBoundary, failed: bool) {
    let (proxy, listener) = listening().await;
    let options = PgConnectOptions::new_without_pgpass()
        .host("127.0.0.1")
        .port(proxy.addr().port())
        .username("fixture")
        .password("")
        .database("fixture")
        .ssl_mode(PgSslMode::Disable);
    let connecting =
        tokio::spawn(async move { PgConnection::connect_with(&options).await.unwrap() });
    let mut server = accept(&listener).await;
    let mut connection = connecting.await.unwrap();
    let before = tokio::time::Instant::now();
    let mut lease = proxy.control(identity(&proxy), false).unwrap();
    let after = tokio::time::Instant::now();
    let deadline = control::active(&proxy.shared, identity(&proxy).connection).unwrap().deadline;
    let contract = Duration::from_secs(6);
    assert!(
        before + contract <= deadline && deadline <= after + contract,
        "activation_deadline_is_not_six_seconds"
    );
    assert!(!lease.snapshot().proof_valid);
    let mut gate = Some(lease.arm(1, boundary).unwrap());
    let mut query = tokio::spawn(async move {
        let result = if prepared {
            sqlx::query("UPDATE fixture SET value=1").execute(&mut connection).await
        } else {
            connection.execute("UPDATE fixture SET value=1").await
        };
        (result, connection)
    });
    if prepared {
        for tag in [b'P', b'D', b'S'] {
            assert_eq!(recv(&mut server).await.0, tag);
        }
        send(&mut server, &[(b'1', b""), (b't', &[0, 0]), (b'n', b""), (b'Z', b"I")]).await;
        assert_eq!(recv(&mut server).await.0, b'B');
    }
    if boundary == ControlBoundary::BeforeForward {
        let hit = tokio::select! {
            hit = gate.as_mut().unwrap().held() => hit.unwrap(),
            _ = &mut query => panic!("query completed before execution gate"),
        };
        assert_eq!(hit.record.ordinal, Some(1));
        assert_eq!(hit.record.event, DiagnosticEvent::Sent);
        assert!(matches!(server.try_read(&mut [0]), Err(e) if e.kind()==io::ErrorKind::WouldBlock));
        gate.take().unwrap().release().await.unwrap();
    }
    assert_eq!(recv(&mut server).await.0, if prepared { b'E' } else { b'Q' });
    if prepared {
        for tag in [b'C', b'S'] {
            assert_eq!(recv(&mut server).await.0, tag);
        }
    }
    if prepared {
        send(&mut server, &[(b'2', b"")]).await;
    }
    send(&mut server, &[(b'C', b"UPDATE 1\0")]).await;
    if prepared {
        send(&mut server, &[(b'3', b"")]).await;
    }
    if failed {
        send(&mut server, &[(b'E', b"SERROR\0C23514\0Mfixture\0\0")]).await;
    }
    send(&mut server, &[(b'Z', b"I")]).await;
    if boundary != ControlBoundary::BeforeForward {
        let hit = tokio::select! {
            hit = gate.as_mut().unwrap().held() => hit.unwrap(),
            _ = &mut query => panic!("query completed before completion gate"),
        };
        assert_eq!(hit.record.identity.pid, Some(42));
        if boundary == ControlBoundary::BeforeReady {
            assert_eq!(hit.record.ordinal, None);
            assert_eq!(hit.complete.unwrap().ordinal, Some(1));
        }
        gate.take().unwrap().release().await.unwrap();
    }
    drop(gate);
    let (result, _connection) = query.await.unwrap();
    assert_eq!(result.is_err(), failed);
    drop(lease.arm(2, ControlBoundary::BeforeForward).unwrap());
    assert_eq!(
        control::active(&proxy.shared, identity(&proxy).connection).unwrap().deadline,
        deadline
    );
    assert!(lease.close().await.unwrap().closed);
    closed(&proxy).await;
}

#[tokio::test]
async fn error_and_ambiguous_ready_never_become_completion_success() {
    bounded(async {
        for mut replies in [
            vec![(b'E', b"SERROR\0C23514\0Mfixture\0\0".as_slice())],
            vec![
                (b'C', b"UPDATE 1\0".as_slice()),
                (b'E', b"SERROR\0C23514\0Mfixture\0\0"),
            ],
            vec![(b'C', b"UPDATE 1\0".as_slice()), (b'C', b"UPDATE 1\0")],
            vec![(b's', b"".as_slice()), (b'C', b"UPDATE 1\0")],
            vec![(b'I', b"".as_slice()), (b'C', b"UPDATE 1\0")],
            vec![(b'C', b"UPDATE 1\0".as_slice())],
        ] {
            let (proxy, mut client, mut server) = pair().await;
            let mut lease = proxy.control(identity(&proxy), false).unwrap();
            let mut gate = lease.arm(1, ControlBoundary::BeforeReady).unwrap();
            if replies.len() != 1 || replies[0].0 != b'C' {
                send(&mut client, &[(b'Q', b"UPDATE fixture\0")]).await;
                recv(&mut server).await;
            }
            replies.push((b'Z', b"I"));
            send(&mut server, &replies).await;
            assert!(gate.held().await.is_err());
            drop(gate);
            let final_state = lease.close().await.unwrap();
            assert!(!final_state.proof_valid);
            assert!(!final_state.armed);
            assert!(final_state.held.is_none());
        }
        let (proxy, mut client, mut server) = pair().await;
        let mut lease = proxy.control(identity(&proxy), false).unwrap();
        let mut gate = lease.arm(1, ControlBoundary::BeforeCommandComplete).unwrap();
        send(&mut client, &[(b'Q', b"UPDATE fixture\0")]).await;
        recv(&mut server).await;
        send(&mut server, &[(b'E', b"SERROR\0C23514\0Mfixture\0\0")]).await;
        let hit = gate.held().await.unwrap();
        assert_eq!(hit.record.event, DiagnosticEvent::Error);
        assert_eq!(hit.record.sqlstate, Some(*b"23514"));
        gate.release().await.unwrap();
        assert_eq!(recv(&mut client).await.0, b'E');
        lease.close().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn pending_legacy_ack_is_rejected_even_after_mode_returned_to_pass() {
    bounded(async {
        let (proxy, mut client, mut server) = pair().await;
        proxy.set_mode(ProxyMode::DropCommitAck);
        send(&mut client, &[(b'Q', b"COMMIT\0")]).await;
        recv(&mut server).await;
        assert_eq!(proxy.mode(), ProxyMode::Pass);
        let mut lease = proxy.control(identity(&proxy), false).unwrap();
        assert!(!lease.snapshot().proof_valid);
        let mut gate = lease.arm(1, ControlBoundary::BeforeCommandComplete).unwrap();
        send(&mut server, &[(b'C', b"COMMIT\0"), (b'Z', b"I")]).await;
        assert!(gate.held().await.is_err());
        drop(gate);
        assert!(!lease.close().await.unwrap().proof_valid);
    })
    .await;
}

#[tokio::test]
async fn gate_drop_sever_expiry_and_lease_drop_close_the_original_sockets() {
    bounded(async {
        for cut in 0..4 {
            let (proxy, mut client, mut server) = pair().await;
            let mut lease = proxy.control(identity(&proxy), true).unwrap();
            let mut held = None;
            if cut == 3 {
                let until =
                    control::active(&proxy.shared, identity(&proxy).connection).unwrap().deadline;
                tokio::time::sleep_until(until + Duration::from_millis(5)).await;
                closed(&proxy).await;
            } else if cut != 2 {
                let mut gate = lease.arm(1, ControlBoundary::BeforeForward).unwrap();
                send(&mut client, &[(b'Q', b"UPDATE fixture\0")]).await;
                let hit = gate.held().await.unwrap();
                held = Some(hit.record.ordinal);
                if cut == 1 {
                    proxy.set_mode(ProxyMode::Sever);
                }
                drop(gate);
            }
            if cut == 2 {
                drop(lease);
            } else {
                assert!(lease.close().await.unwrap().closed);
            }
            closed(&proxy).await;
            assert!(!proxy.diagnostics().unwrap().records.iter().any(|r| Some(r.ordinal) == held
                && r.delivery == DiagnosticDelivery::Forwarded
                && r.event == DiagnosticEvent::Sent));
            assert_eq!(server.read(&mut [0]).await.unwrap(), 0);
        }
    })
    .await;
}
