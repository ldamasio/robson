use super::*;

fn frame(tag: u8, body: &[u8]) -> Vec<u8> {
    let mut bytes = vec![tag];
    bytes.extend_from_slice(&(body.len() as i32 + 4).to_be_bytes());
    bytes.extend_from_slice(body);
    bytes
}

async fn exchange(script: Vec<(u8, &'static [u8], Vec<u8>)>) -> DiagnosticSnapshot {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy = DbProxy::start(listener.local_addr().unwrap()).await.unwrap();
        proxy.enable_diagnostics();
        let server_script = script.clone();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let length = socket.read_i32().await.unwrap();
            let mut startup = vec![0; length as usize - 4];
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
        proxy.diagnostics().unwrap()
    })
    .await
    .expect("bounded_namespace_script_hung")
}

#[tokio::test]
async fn sql_prepare_after_wire_close_cannot_relabel_cached_execution() {
    let snapshot = exchange(vec![
        (b'Q', b"BEGIN\0", [frame(b'C', b"BEGIN\0"), frame(b'Z', b"T")].concat()),
        (b'P', b"s\0SELECT id FROM venue_orders\0\0\0", frame(b'1', b"")),
        (b'S', b"", frame(b'Z', b"T")),
        (b'B', b"p\0s\0\0\0", frame(b'2', b"")),
        (b'E', b"p\0\0\0\0\0", frame(b'C', b"SELECT 0\0")),
        (b'S', b"", frame(b'Z', b"T")),
        (b'C', b"Ss\0", frame(b'3', b"")),
        (b'S', b"", frame(b'Z', b"T")),
        (
            b'Q',
            b"pRePaRe s AS SELECT id FROM order_fills\0",
            [frame(b'C', b"PREPARE\0"), frame(b'Z', b"T")].concat(),
        ),
        (b'B', b"later\0s\0\0\0", frame(b'2', b"")),
        (b'E', b"later\0\0\0\0\0", frame(b'C', b"SELECT 0\0")),
        (b'S', b"", frame(b'Z', b"T")),
    ])
    .await;
    let executions: Vec<_> = snapshot
        .records
        .iter()
        .filter(|r| {
            r.event == DiagnosticEvent::Complete && r.delivery == DiagnosticDelivery::Received
        })
        .collect();
    let accepted = executions[1];
    assert_eq!(
        (accepted.phase, accepted.ordinal, accepted.category),
        (DiagnosticPhase::Execute, Some(2), SqlCategory::OrderRead),
        "accepted_namespace_prefix_lost_known_category"
    );
    let last = executions.last().unwrap();
    assert_eq!(
        (last.phase, last.ordinal, last.category),
        (DiagnosticPhase::Unknown, None, SqlCategory::Other),
        "sql_prepare_reused_stale_execution_category"
    );
    assert!(last.identity.attribution_unknown);
}

#[tokio::test]
async fn namespace_sql_preparation_is_opaque_but_unrelated_columns_remain_known() {
    for body in [
        b"change\0pRePaRe s AS SELECT id FROM order_fills\0\0\0".as_slice(),
        b"change\0dEaLlOcAtE s\0\0\0",
        b"change\0dIsCaRd PLANS\0\0\0",
        b"change\0dEcLaRe p CURSOR FOR SELECT id FROM order_fills\0\0\0",
        // False Unknown from a literal is an explicit conservative trade-off.
        b"change\0SELECT 'PREPARE'\0\0\0",
    ] {
        let snapshot = exchange(vec![
            (b'P', body, frame(b'1', b"")),
            (b'S', b"", frame(b'Z', b"I")),
        ])
        .await;
        let parsed = snapshot.records.iter().find(|r| r.event == DiagnosticEvent::Sent).unwrap();
        assert_eq!(
            (parsed.phase, parsed.ordinal, parsed.category),
            (DiagnosticPhase::Unknown, None, SqlCategory::Other),
            "namespace_preparation_claimed_supported_phase"
        );
        assert!(parsed.identity.attribution_unknown);
    }
    let snapshot = exchange(vec![
        (
            b'P',
            b"change\0UPDATE venue_orders SET closed_at=NULL, executed_cum=0\0\0\0",
            frame(b'1', b""),
        ),
        (b'S', b"", frame(b'Z', b"I")),
    ])
    .await;
    let parsed = snapshot.records.iter().find(|r| r.event == DiagnosticEvent::Sent).unwrap();
    assert_eq!(
        (parsed.phase, parsed.ordinal, parsed.category),
        (DiagnosticPhase::Parse, None, SqlCategory::OrderWrite),
        "namespace_guard_matched_unrelated_column_names"
    );
    assert!(!parsed.identity.attribution_unknown);
}
