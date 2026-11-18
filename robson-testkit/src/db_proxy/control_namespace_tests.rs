//! The exception describes one canonical synthetic Q, never its SQL effects.
use super::{control_tests::*, *};
const DO: &[u8] = b"DO $migration38$ BEGIN NULL; END $migration38$;\0";

#[test]
fn canonical_do_recognizer_rejects_extra_statements_delimiters_and_invalid_bytes() {
    assert!(control::canonical_do(DO));
    for body in [
        b"DO $migration38$ BEGIN NULL; END $migration38$; SELECT 1\0".as_slice(),
        b"DO $migration38$ $migration38$ END $migration38$;\0",
        b"DO $$ BEGIN NULL; END $$;\0",
        b"DO $migration38$ BEGIN NULL; END $migration38$;",
        b"DO $migration38$ \xff $migration38$;\0",
        b"DO $migration38$ \0 $migration38$;\0",
    ] {
        assert!(!control::canonical_do(body));
    }
    let oversized = format!("DO $migration38${}$migration38$;\0", " ".repeat(16_384));
    assert!(!control::canonical_do(oversized.as_bytes()));
}

#[tokio::test]
async fn canonical_do_is_known_for_its_gate_and_for_a_later_gate() {
    bounded(async {
        for later in [false, true] {
            let (proxy, mut client, mut server) = pair().await;
            let mut lease = proxy.control(identity(&proxy), false).unwrap();
            assert!(matches!(proxy.control(identity(&proxy), false), Err(ControlError::Busy)));
            let mut gate =
                lease.arm(if later { 2 } else { 1 }, ControlBoundary::BeforeForward).unwrap();
            send(&mut client, &[(b'Q', DO)]).await;
            if later {
                assert_eq!(recv(&mut server).await.1, DO);
                send(&mut server, &[(b'C', b"DO\0"), (b'Z', b"I")]).await;
                recv(&mut client).await;
                recv(&mut client).await;
                send(&mut client, &[(b'Q', b"UPDATE fixture\0")]).await;
            }
            let hit = gate.held().await.unwrap();
            assert!(!hit.record.identity.attribution_unknown);
            assert_eq!(hit.record.ordinal, Some(if later { 2 } else { 1 }));
            gate.release().await.unwrap();
            recv(&mut server).await;
            lease.close().await.unwrap();
        }
    })
    .await;
}

#[tokio::test]
async fn second_canonical_do_and_namespace_opaque_do_cannot_reuse_the_exception() {
    bounded(async {
        for sql in [
            DO,
            b"DO $migration38$ DECLARE n int; BEGIN NULL; END $migration38$;\0",
            b"DO $migration38$ BEGIN NULL; END $migration38$; SELECT 1\0",
            b"DO $migration38$ $migration38$ END $migration38$;\0",
        ] {
            let (proxy, mut client, mut server) = pair().await;
            let lease = proxy.control(identity(&proxy), false).unwrap();
            if sql == DO {
                send(&mut client, &[(b'Q', DO)]).await;
                recv(&mut server).await;
                send(&mut server, &[(b'C', b"DO\0"), (b'Z', b"I")]).await;
                recv(&mut client).await;
                recv(&mut client).await;
            }
            send(&mut client, &[(b'Q', sql)]).await;
            closed(&proxy).await;
            assert!(!lease.close().await.unwrap().proof_valid);
        }
    })
    .await;
}

#[tokio::test]
async fn oversize_stale_unknown_and_default_pass_have_explicit_limits() {
    bounded(async {
        let (proxy, mut client, mut server) = pair().await;
        let mut stale = identity(&proxy);
        stale.pid = Some(43);
        assert!(matches!(proxy.control(stale, false), Err(ControlError::InvalidIdentity)));
        send(&mut client, &[(b'Q', b"DO $migration38$ BEGIN NULL; END $migration38$;\0")]).await;
        assert_eq!(recv(&mut server).await.1, b"DO $migration38$ BEGIN NULL; END $migration38$;\0");
        assert!(matches!(
            proxy.control(identity(&proxy), false),
            Err(ControlError::InvalidIdentity)
        ));
        drop(client);
        assert_eq!(server.read(&mut [0]).await.unwrap(), 0);
        let (proxy, mut client, mut server) = pair().await;
        let lease = proxy.control(identity(&proxy), false).unwrap();
        client.write_all(&[b'Q', 0, 1, 0, 0]).await.unwrap();
        closed(&proxy).await;
        assert!(!lease.close().await.unwrap().proof_valid);
        assert_eq!(server.read(&mut [0]).await.unwrap(), 0);
    })
    .await;
}
