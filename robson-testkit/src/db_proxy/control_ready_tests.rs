//! Ready authority comes from the whole response group, not the arm instant.
use std::sync::Arc;

use tokio::{
    io::AsyncReadExt,
    net::TcpStream,
    sync::{oneshot, Barrier},
};

use super::{control_tests::*, *};

const QUERY: &[u8] = b"UPDATE fixture\0";
const COMPLETE: &[u8] = b"UPDATE 1\0";
const NOTICE: &[u8] = b"\0\0\0*c\0payload00\0";

async fn extended(client: &mut TcpStream, server: &mut TcpStream, count: usize) {
    let mut requests: Vec<(u8, &[u8])> = vec![(b'P', b"s\0UPDATE fixture\0\0\0")];
    for _ in 0..count {
        requests.extend([
            (b'B', b"\0s\0\0\0\0\0\0\0".as_slice()),
            (b'E', b"\0\0\0\0\0"),
        ]);
    }
    requests.push((b'S', b""));
    send(client, &requests).await;
    for _ in &requests {
        recv(server).await;
    }
    send(server, &[(b'1', b""), (b'2', b"")]).await;
    assert_eq!(recv(client).await.0, b'1');
    assert_eq!(recv(client).await.0, b'2');
}

async fn clean_boundary(
    lease: &mut ControlLease,
    client: &mut TcpStream,
    server: &mut TcpStream,
    ordinal: u64,
) {
    let mut gate = lease.arm(ordinal, ControlBoundary::BeforeReady).unwrap();
    send(client, &[(b'Q', QUERY)]).await;
    assert_eq!(recv(server).await.0, b'Q');
    send(server, &[(b'C', COMPLETE), (b'Z', b"I")]).await;
    let hit = gate.held().await.unwrap();
    assert_eq!(hit.record.ordinal, None);
    assert_eq!(hit.complete.unwrap().ordinal, Some(ordinal));
    gate.release().await.unwrap();
    assert_eq!(recv(client).await.0, b'C');
    assert_eq!(recv(client).await.0, b'Z');
}

#[tokio::test]
async fn ready_rejects_prearm_completion_and_rearm_inside_one_group() {
    bounded(async {
        for (prior, rearm, activate_late) in [
            (b'C', false, false),
            (b'C', false, true),
            (b'C', true, false),
            (b'I', false, false),
            (b'I', false, true),
            (b's', false, false),
            (b's', false, true),
            (b'E', false, false),
            (b'E', false, true),
        ] {
            let (proxy, mut client, mut server) = pair().await;
            let mut lease = if activate_late {
                None
            } else {
                Some(proxy.control(identity(&proxy), false).unwrap())
            };
            let ordinal = if prior == b'E' {
                send(&mut client, &[(b'Q', QUERY)]).await;
                recv(&mut server).await;
                1
            } else {
                extended(&mut client, &mut server, 2).await;
                2
            };
            let prior_body = match prior {
                b'C' => COMPLETE,
                b'E' => b"SERROR\0C23514\0Mfixture\0\0",
                _ => b"",
            };
            if rearm {
                let mut first =
                    lease.as_mut().unwrap().arm(1, ControlBoundary::BeforeCommandComplete).unwrap();
                send(&mut server, &[(prior, prior_body)]).await;
                first.held().await.unwrap();
                first.release().await.unwrap();
            } else {
                send(&mut server, &[(prior, prior_body)]).await;
            }
            assert_eq!(recv(&mut client).await.0, prior);
            let mut lease =
                lease.unwrap_or_else(|| proxy.control(identity(&proxy), false).unwrap());
            let mut gate = lease.arm(ordinal, ControlBoundary::BeforeReady).unwrap();
            let replies = if prior == b'E' {
                vec![(b'C', COMPLETE), (b'Z', b"I".as_slice())]
            } else {
                vec![(b'2', b"".as_slice()), (b'C', COMPLETE), (b'Z', b"I")]
            };
            send(&mut server, &replies).await;
            assert!(gate.held().await.is_err(), "ready_ignored_prearm_group_history");
            drop(gate);
            assert!(lease.close().await.unwrap().closed);
            assert_eq!(server.read(&mut [0]).await.unwrap(), 0);
        }
    })
    .await;
}

#[tokio::test]
async fn ready_rearm_after_real_boundary_uses_only_the_new_group() {
    bounded(async {
        let (proxy, mut client, mut server) = pair().await;
        let mut lease = proxy.control(identity(&proxy), false).unwrap();
        for ordinal in 1..=3 {
            clean_boundary(&mut lease, &mut client, &mut server, ordinal).await;
        }
        assert!(lease.close().await.unwrap().closed);
        assert_eq!(server.read(&mut [0]).await.unwrap(), 0);
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn generated_ready_arm_backend_histories_match_script_oracle() {
    bounded(async {
        for history in 0..128usize {
            // Independent oracle: this script fixes its C count before any
            // control/Trace helper runs. Modes force both orders and race them.
            let completions = 1 + history % 2;
            let order = (history / 2) % 3;
            let notices = (history / 6) % 4;
            let previous_groups = history / 24;
            let expected_success = completions == 1;
            let (proxy, mut client, mut server) = pair().await;
            let mut lease = proxy.control(identity(&proxy), false).unwrap();
            for ordinal in 1..=previous_groups {
                clean_boundary(&mut lease, &mut client, &mut server, ordinal as u64).await;
            }
            extended(&mut client, &mut server, completions).await;
            let start = Arc::new(Barrier::new(2));
            let backend_start = start.clone();
            let (armed, installed) = oneshot::channel();
            let backend = tokio::spawn(async move {
                backend_start.wait().await;
                let prefix = if completions == 2 {
                    (b'C', COMPLETE)
                } else {
                    (b'A', NOTICE)
                };
                send(&mut server, &[prefix]).await;
                installed.await.unwrap();
                let mut replies = vec![];
                for _ in 0..notices {
                    replies.push((b'A', NOTICE));
                }
                if completions == 2 {
                    replies.push((b'2', b"".as_slice()));
                }
                replies.extend([(b'C', COMPLETE), (b'Z', b"I".as_slice())]);
                send(&mut server, &replies).await;
                server
            });
            let ordinal = (previous_groups + completions) as u64;
            let mut gate = if order == 0 {
                let gate = lease.arm(ordinal, ControlBoundary::BeforeReady).unwrap();
                start.wait().await;
                gate
            } else {
                start.wait().await;
                if order == 1 {
                    recv(&mut client).await;
                }
                lease.arm(ordinal, ControlBoundary::BeforeReady).unwrap()
            };
            armed.send(()).unwrap();
            let outcome = gate.held().await;
            assert_eq!(
                outcome.is_ok(),
                expected_success,
                "ready_group_script_oracle history={history}"
            );
            if expected_success {
                let hit = outcome.unwrap();
                assert_eq!(hit.record.ordinal, None);
                assert_eq!(hit.complete.unwrap().ordinal, Some(ordinal));
                gate.release().await.unwrap();
            } else {
                drop(gate);
            }
            let mut server = backend.await.unwrap();
            let closed = lease.close().await.unwrap();
            assert!(closed.closed && !closed.armed && closed.held.is_none());
            assert_eq!(server.read(&mut [0]).await.unwrap(), 0);
        }
    })
    .await;
}
