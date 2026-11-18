use std::{
    pin::Pin,
    sync::Mutex,
    task::{Context, Poll},
};

use tokio::io::AsyncWrite;

use super::*;

struct Writer {
    remaining: usize,
    bytes: Vec<u8>,
}
impl AsyncWrite for Writer {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        if self.remaining == 0 {
            return Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()));
        }
        let n = buf.len().min(self.remaining);
        self.bytes.extend_from_slice(&buf[..n]);
        self.remaining -= n;
        Poll::Ready(Ok(n))
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn frontend_forwarding_requires_complete_write() {
    for remaining in [0, 3, 1024] {
        let trace = Mutex::new(Some(diagnostic::Trace::new()));
        let connection = diagnostic::Connection::new(&trace, 1, None).unwrap();
        let record = connection.frontend(b'Q', SqlCategory::Begin, true);
        let mut writer = Writer { remaining, bytes: Vec::new() };
        let result =
            forward_frontend(&mut writer, b'Q', b"BEGIN\0", Some(&connection), record).await;
        let held = trace.lock().unwrap();
        let snapshot = held.as_ref().unwrap().snapshot();
        drop(held);
        let forwarded = snapshot
            .records
            .iter()
            .filter(|r| {
                r.event == DiagnosticEvent::Sent && r.delivery == DiagnosticDelivery::Forwarded
            })
            .count();
        assert_eq!(
            forwarded,
            usize::from(result.is_ok()),
            "frontend_forwarded_before_complete_write"
        );
        assert_eq!(result.is_ok(), remaining == 1024);
        assert_eq!(writer.bytes.len(), remaining.min(11));
        if result.is_ok() {
            assert_eq!(writer.bytes, b"Q\0\0\0\x0aBEGIN\0");
        }
    }
}

async fn run_script(
    enabled: bool,
    mode: ProxyMode,
) -> (Vec<Vec<u8>>, Vec<u8>, ProxyStats, u64, u64, ProxyMode) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy = DbProxy::start(listener.local_addr().unwrap()).await.unwrap();
    if enabled {
        proxy.enable_diagnostics();
    }
    let script: Vec<(u8, &[u8], Vec<u8>)> = vec![
        (b'P', b"cached\0UPDATE order_fills SET qty=1\0\0\0", frame(b'1', b"")),
        (b'B', b"portal\0cached\0\0\0", frame(b'2', b"")),
        (b'E', b"portal\0\0\0\0\0", frame(b'C', b"UPDATE 1\0")),
        (b'S', b"", frame(b'Z', b"I")),
        (b'Q', b"BEGIN\0", [frame(b'C', b"BEGIN\0"), frame(b'Z', b"T")].concat()),
        (b'B', b"portal\0cached\0\0\0", frame(b'2', b"")),
        (b'E', b"portal\0\0\0\0\0", frame(b'C', b"UPDATE 1\0")),
        (b'S', b"", frame(b'Z', b"T")),
        (
            b'Q',
            if matches!(mode, ProxyMode::DropRollbackAck | ProxyMode::DropRollbackAckOf(_)) {
                b"ROLLBACK\0"
            } else {
                b"COMMIT\0"
            },
            [frame(b'C', b"END\0"), frame(b'Z', b"I")].concat(),
        ),
    ];
    let server_script = script.clone();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let len = socket.read_i32().await.unwrap();
        let mut startup = vec![0u8; len as usize - 4];
        socket.read_exact(&mut startup).await.unwrap();
        let mut seen = Vec::new();
        for (tag, body, reply) in server_script {
            let mut actual = vec![0; body.len() + 5];
            if socket.read_exact(&mut actual).await.is_err() {
                break;
            }
            assert_eq!(actual, frame(tag, body));
            seen.push(actual);
            if socket.write_all(&reply).await.is_err() {
                break;
            }
        }
        seen
    });
    let mut client = TcpStream::connect(proxy.addr()).await.unwrap();
    let mut startup = 8i32.to_be_bytes().to_vec();
    startup.extend_from_slice(&196608i32.to_be_bytes());
    client.write_all(&startup).await.unwrap();
    // Autocommit drops the first cached execution, other faults the final end.
    proxy.set_mode(mode);
    let mut received = Vec::new();
    for (tag, body, expected) in script {
        if client.write_all(&frame(tag, body)).await.is_err() {
            break;
        }
        // Autocommit intentionally holds C until Sync, so pipeline both.
        if matches!(mode, ProxyMode::DropAutocommitAckOf(_)) && tag == b'E' {
            continue;
        }
        let mut reply = vec![0; expected.len()];
        match client.read_exact(&mut reply).await {
            Ok(_) => received.extend_from_slice(&reply),
            Err(_) => break,
        }
    }
    drop(client);
    let seen = server.await.unwrap();
    assert_eq!(proxy.diagnostics().is_some(), enabled);
    (
        seen,
        received,
        proxy.stats(),
        proxy.rollback_acks_dropped(),
        proxy.autocommit_acks_dropped(),
        proxy.mode(),
    )
}
fn frame(tag: u8, body: &[u8]) -> Vec<u8> {
    let mut frame = vec![tag];
    frame.extend_from_slice(&(body.len() as i32 + 4).to_be_bytes());
    frame.extend_from_slice(body);
    frame
}
#[tokio::test]
async fn enabled_diagnostics_preserve_bytes_and_all_targeted_fault_counters() {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        for mode in [
            ProxyMode::Pass,
            ProxyMode::DropCommitAck,
            ProxyMode::DropCommitAckOf("order_fills"),
            ProxyMode::DropRollbackAck,
            ProxyMode::DropRollbackAckOf("order_fills"),
            ProxyMode::DropAutocommitAckOf("order_fills"),
        ] {
            let disabled = run_script(false, mode).await;
            let enabled = run_script(true, mode).await;
            assert_eq!(disabled, enabled, "diagnostics_changed_forwarded_bytes_or_fault_counters");
        }
    })
    .await
    .expect("bounded_equivalence_script_hung");
}

async fn completion_after_cache_preparation_failure(skipped_parse: bool) -> DiagnosticRecord {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy = DbProxy::start(listener.local_addr().unwrap()).await.unwrap();
        proxy.enable_diagnostics();
        // The server accepted s as an order query. Its later rejected/skipped
        // Parse never replaces that statement, while the inherited cache does.
        let mut script: Vec<(u8, &[u8], Vec<u8>)> = vec![
            (b'P', b"s\0SELECT id FROM venue_orders\0\0\0", frame(b'1', b"")),
            (b'S', b"", frame(b'Z', b"I")),
            (b'B', b"p\0s\0\0\0", frame(b'2', b"")),
            (b'E', b"p\0\0\0\0\0", frame(b'C', b"SELECT 1\0")),
            (b'S', b"", frame(b'Z', b"I")),
        ];
        if skipped_parse {
            script.extend([
                (b'B', b"p\0s\0\0\0".as_slice(), frame(b'2', b"")),
                (b'E', b"p\0\0\0\0\0", Vec::new()),
            ]);
        }
        script.extend([
            (
                b'P',
                b"s\0UPDATE accounting_state SET version=version+1\0\0\0".as_slice(),
                Vec::new(),
            ),
            // Hold ErrorResponse until Sync is actually received. Otherwise
            // the existing missing-Sync safeguard could mask stale-cache use.
            (
                b'S',
                b"",
                [
                    frame(
                        b'E',
                        if skipped_parse {
                            b"SERROR\0C23514\0\0"
                        } else {
                            b"SERROR\0C42P05\0\0"
                        },
                    ),
                    frame(b'Z', b"I"),
                ]
                .concat(),
            ),
            (b'B', b"later\0s\0\0\0", frame(b'2', b"")),
            (b'E', b"later\0\0\0\0\0", frame(b'C', b"SELECT 1\0")),
            (b'S', b"", frame(b'Z', b"I")),
        ]);
        let server_script = script.clone();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let len = socket.read_i32().await.unwrap();
            let mut startup = vec![0; len as usize - 4];
            socket.read_exact(&mut startup).await.unwrap();
            for (tag, body, reply) in server_script {
                let mut actual = vec![0; body.len() + 5];
                socket.read_exact(&mut actual).await.unwrap();
                assert_eq!(actual, frame(tag, body));
                socket.write_all(&reply).await.unwrap();
            }
        });
        let mut client = TcpStream::connect(proxy.addr()).await.unwrap();
        let mut startup = 8i32.to_be_bytes().to_vec();
        startup.extend_from_slice(&196608i32.to_be_bytes());
        client.write_all(&startup).await.unwrap();
        for (tag, body, expected) in script {
            client.write_all(&frame(tag, body)).await.unwrap();
            let mut reply = vec![0; expected.len()];
            client.read_exact(&mut reply).await.unwrap();
            assert_eq!(reply, expected);
        }
        server.await.unwrap();
        let snapshot = proxy.diagnostics().unwrap();
        let completions: Vec<_> = snapshot
            .records
            .iter()
            .filter(|r| {
                r.event == DiagnosticEvent::Complete && r.delivery == DiagnosticDelivery::Received
            })
            .collect();
        let first = completions[0];
        assert_eq!(
            (first.phase, first.ordinal, first.category),
            (DiagnosticPhase::Execute, Some(1), SqlCategory::OrderRead),
            "accepted_cached_statement_lost_proven_category"
        );
        **completions.last().unwrap()
    })
    .await
    .expect("bounded_cache_error_script_hung")
}

#[tokio::test]
async fn rejected_named_parse_cannot_relabel_cached_execution() {
    let completion = completion_after_cache_preparation_failure(false).await;
    assert_eq!(
        (completion.phase, completion.ordinal, completion.category),
        (DiagnosticPhase::Unknown, None, SqlCategory::Other),
        "rejected_parse_cache_falsely_named_later_execution"
    );
    assert!(completion.identity.attribution_unknown);
}

#[tokio::test]
async fn skipped_parse_after_execution_error_cannot_relabel_cached_execution() {
    let completion = completion_after_cache_preparation_failure(true).await;
    assert_eq!(
        (completion.phase, completion.ordinal, completion.category),
        (DiagnosticPhase::Unknown, None, SqlCategory::Other),
        "skipped_parse_cache_falsely_named_later_execution"
    );
    assert!(completion.identity.attribution_unknown);
}
