use std::{
    future::Future,
    io::ErrorKind::WouldBlock,
    pin::Pin,
    sync::atomic::{AtomicU8, Ordering},
    task::Poll,
    time::Duration,
};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::{mpsc, watch},
};

use super::{control_tests::*, ControlError::InvalidIdentity, *};

#[tokio::test]
async fn orphan_keeps_the_partial_backend_reader_and_both_upstream_halves() {
    bounded(async {
        let (proxy, client, mut server) = pair().await;
        let lease = proxy.control(identity(&proxy), true).unwrap();
        let state = control::active(&proxy.shared, identity(&proxy).connection).unwrap();
        let notification = packet(b'A', b"\0\0\0*c\0payload00\0");
        server.write_all(&notification[..6]).await.unwrap();
        while state.read_progress.load(Ordering::Acquire) != 1 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        drop(client);
        while !lease.snapshot().orphan {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        server.write_all(&notification[6..]).await.unwrap();
        server.write_all(&notification).await.unwrap();
        while state.read_progress.load(Ordering::Acquire) != 32 {
            assert!(!lease.snapshot().closed, "orphan_discarded_partial_backend_read");
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        assert_eq!(server.try_read(&mut [0]).unwrap_err().kind(), WouldBlock);
        assert!(!lease.snapshot().closed);
        assert!(lease.close().await.unwrap().closed);
        assert_eq!(server.read(&mut [0]).await.unwrap(), 0);
    })
    .await;
}

#[tokio::test]
async fn real_client_write_error_enters_orphan_without_forwarding_the_held_query() {
    bounded(async {
        let (proxy, mut client, mut server) = pair().await;
        let mut lease = proxy.control(identity(&proxy), true).unwrap();
        let mut gate = lease.arm(1, ControlBoundary::BeforeForward).unwrap();
        send(&mut client, &[(b'Q', b"UPDATE fixture\0")]).await;
        gate.held().await.unwrap();
        #[allow(deprecated)]
        client.set_linger(Some(Duration::ZERO)).unwrap();
        drop(client);
        for _ in 0..16 {
            match server.write_all(&packet(b'A', b"\0\0\0*c\0payload00\0")).await {
                Ok(()) => {},
                Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => break,
                Err(error) => panic!("backend notification injection failed: {error}"),
            }
            if gate.snapshot().client_loss == Some(ClientLoss::Write) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        assert_eq!(gate.snapshot().client_loss, Some(ClientLoss::Write));
        assert!(gate.snapshot().orphan);
        assert_eq!(server.try_read(&mut [0]).unwrap_err().kind(), WouldBlock);
        drop(gate);
        lease.close().await.unwrap();
        assert_eq!(server.read(&mut [0]).await.unwrap(), 0);
    })
    .await;
}

#[tokio::test]
async fn partial_write_keeps_complete_write_bytes_but_refuses_invalidated_ack() {
    bounded(async {
        for invalidation in 0..4 {
            let (proxy, mut client, mut server)=pair().await;
            send(&mut client,&[(b'Q',b"UPDATE fixture\0")]).await;recv(&mut server).await;
            send(&mut server,&[(b'C',b"UPDATE 1\0"),(b'Z',b"I")]).await;recv(&mut client).await;recv(&mut client).await;
            let record=proxy.diagnostics().unwrap().records.into_iter().find(|r|r.event==DiagnosticEvent::Sent).unwrap();
            let mut lease=proxy.control(identity(&proxy),false).unwrap();
            let state=control::active(&proxy.shared,identity(&proxy).connection).unwrap();
            let mut gate=lease.arm(1,ControlBoundary::BeforeForward).unwrap();
            let pending=AtomicU8::new(0);
            let frames=control::io::Frames { shared:&proxy.shared,pending:&pending,diagnostic:None,id:record.identity.connection };
            let (progress, mut changed)=watch::channel(0);
            let (proceed, receiver)=mpsc::channel(1);
            let mut writer=PartialWriter::new(Vec::<u8>::new(), Some(WritePause { progress, proceed:receiver }));
            {
                let forward=frames.forward(&mut writer,b'Q',b"UPDATE fixture\0",Some(record),true);
                tokio::pin!(forward);
                tokio::select! { hit=gate.held()=>{hit.unwrap();}, _=&mut forward=>panic!("write finished before gate") }
                let release=gate.release();tokio::pin!(release);
                tokio::select! { _=changed.changed()=>{}, _=&mut release=>panic!("ack before full write"), _=&mut forward=>panic!("write was not partial") }
                assert_eq!(*changed.borrow(),2);
                release_still_pending(release.as_mut()).await;
                match invalidation {
                    0=>state.invalid(),
                    1=>proxy.set_mode(ProxyMode::Sever),
                    2=>pending.store(1,Ordering::SeqCst),
                    _=>tokio::time::sleep_until(state.deadline+Duration::from_millis(5)).await,
                }
                proceed.send(()).await.unwrap();
                let (physical,ack)=tokio::join!(&mut forward,&mut release);
                assert!(physical.is_err());assert!(ack.is_err());
            }
            assert_eq!(writer.inner,packet(b'Q',b"UPDATE fixture\0"));
            assert!(!lease.close().await.unwrap().released);
        }
        active_partial_write_emitter().await;
    }).await;
}

#[tokio::test]
async fn activation_after_real_close_and_close_arm_interleavings_leave_no_slot() {
    bounded(async {
        let (proxy, client, mut server) = pair().await;
        let observed = identity(&proxy);
        drop(client);
        assert_eq!(server.read(&mut [0]).await.unwrap(), 0);
        closed(&proxy).await;
        // Enter the same publication stage after the optimistic first check.
        assert!(matches!(
            ControlLease::publish(&proxy.shared, observed, false),
            Err(InvalidIdentity)
        ));
        assert!(proxy.shared.control.lock().unwrap().is_none());
        super::control_test_support::closed_is_observed_after_socket_owners().await;
        for armed in [false, true] {
            let (proxy, client, mut server) = pair().await;
            let mut lease = proxy.control(identity(&proxy), false).unwrap();
            if armed {
                drop(lease.arm(1, ControlBoundary::BeforeForward).unwrap());
            }
            drop(client);
            assert_eq!(server.read(&mut [0]).await.unwrap(), 0);
            closed(&proxy).await;
            assert!(lease.arm_after_check(1, ControlBoundary::BeforeForward).is_err());
            assert!(lease.gate_empty());
            let final_state = lease.close().await.unwrap();
            assert!(final_state.closed);
            assert!(!final_state.armed);
            assert!(final_state.held.is_none());
        }
    })
    .await;
}

#[tokio::test]
async fn actual_ring_phase_and_connection_overflow_refuse_control_authority() {
    bounded(async {
        for phase in [false, true] {
            let (proxy, mut client, mut server) = pair().await;
            let (tag, sql, count) = if phase {
                (b'P', b"s\0UPDATE fixture\0\0\0".as_slice(), 33)
            } else {
                (b'Q', b"UPDATE fixture\0".as_slice(), 700)
            };
            for _ in 0..count {
                send(&mut client, &[(tag, sql)]).await;
                recv(&mut server).await;
                if !phase {
                    send(&mut server, &[(b'C', b"UPDATE 1\0"), (b'Z', b"I")]).await;
                    recv(&mut client).await;
                    recv(&mut client).await;
                }
            }
            let trace = proxy.diagnostics().unwrap();
            let overflow = if phase {
                trace.phase_overflow
            } else {
                trace.dropped
            };
            assert!(overflow > 0);
            assert!(matches!(proxy.control(identity(&proxy), false), Err(InvalidIdentity)));
        }
        super::control_test_support::global_phase_overflow_refuses_other_connection().await;
        let (proxy, listener) = listening().await;
        let mut sockets = Vec::with_capacity(66);
        for _ in 0..33 {
            let mut client = TcpStream::connect(proxy.addr()).await.unwrap();
            client.write_all(&[0, 0, 0, 8, 0, 3, 0, 0]).await.unwrap();
            let server = accept(&listener).await;
            for _ in 0..3 {
                recv(&mut client).await;
            }
            sockets.extend([client, server]);
        }
        assert!(proxy.diagnostics().unwrap().connection_overflow > 0);
        assert!(matches!(proxy.control(identity(&proxy), false), Err(InvalidIdentity)));
    })
    .await;
}

async fn release_still_pending<F: Future>(mut future: Pin<&mut F>) {
    std::future::poll_fn(|cx| {
        assert!(future.as_mut().poll(cx).is_pending(), "release_ack_before_complete_write");
        Poll::Ready(())
    })
    .await;
}

async fn active_partial_write_emitter() {
    let (proxy, mut client, mut server) = pair().await;
    let mut lease = proxy.control(identity(&proxy), false).unwrap();
    let state = control::active(&proxy.shared, identity(&proxy).connection).unwrap();
    let (mut changed, proceed) = install_write_pause(&state).unwrap();
    let mut gate = lease.arm(1, ControlBoundary::BeforeForward).unwrap();
    send(&mut client, &[(b'Q', b"UPDATE fixture\0")]).await;
    let hit = gate.held().await.unwrap();
    let expected = packet(b'Q', b"UPDATE fixture\0");
    let forwarded = |r: &DiagnosticRecord| {
        r.identity == hit.record.identity
            && r.ordinal == hit.record.ordinal
            && r.phase == hit.record.phase
            && r.event == hit.record.event
            && r.delivery == DiagnosticDelivery::Forwarded
    };
    {
        let release = gate.release();
        tokio::pin!(release);
        tokio::select! {
            progress = changed.changed() => progress.unwrap(),
            _ = &mut release => panic!("release_ack_before_complete_write"),
        }
        assert_eq!(*changed.borrow(), 2);
        let mut prefix = [0; 2];
        server.read_exact(&mut prefix).await.unwrap();
        assert_eq!(prefix.as_slice(), &expected[..2]);
        release_still_pending(release.as_mut()).await;
        assert!(
            !proxy.diagnostics().unwrap().records.iter().any(&forwarded),
            "active_partial_write_emitted_forwarded_early"
        );
        proceed.send(()).await.unwrap();
        release.await.unwrap();
    }
    let mut rest = vec![0; expected.len() - 2];
    server.read_exact(&mut rest).await.unwrap();
    assert_eq!(rest.as_slice(), &expected[2..]);
    assert!(
        proxy.diagnostics().unwrap().records.iter().any(&forwarded),
        "active_partial_write_missing_forwarded"
    );
    let (_unused_progress, _unused_proceed) = install_write_pause(&state).unwrap();
    lease.close().await.unwrap();
    let cleared = state.write_pause.lock().unwrap().is_none();
    assert!(cleared, "unused_write_pause_survived_close");
    assert_eq!(server.read(&mut [0]).await.unwrap(), 0);
}
