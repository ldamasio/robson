//! Ordinary app decoder controls; these are not native/transport provenance.
use robson_exec::venue_clock::ClockDomain;

use super::*;

fn body(bytes: Vec<u8>, kind: &str) -> CompleteBody {
    CompleteBody {
        bytes,
        content_type: Some(kind.into()),
        success: true,
        receive: RawClockCapture {
            epoch: Uuid::from_u128(1),
            wall_seconds: 1_794_614_400,
            wall_nanoseconds: 27,
            domain: ClockDomain::new(Uuid::from_u128(2), 0, 0).unwrap(),
            before_ns: 100,
            after_ns: 127,
        },
    }
}
#[test]
fn complete_large_unknown_fields_preserve_raw_capture() {
    for size in [2 * 1024 * 1024, 8 * 1024 * 1024] {
        let bytes =
            format!("{{\"unknown\":\"{}\",\"serverTime\":1794614400000}}", "x".repeat(size))
                .into_bytes();
        let input = body(bytes, "application/json; charset=utf-8");
        let original = input.receive;
        let (capture, server) = input.decode().unwrap();
        assert_eq!(capture, original);
        assert_eq!(capture.wall_nanoseconds, 27);
        assert_eq!(server, 1_794_614_400_000);
    }
}
#[test]
fn full_text_charset_bom_and_end_are_used() {
    let json = "{\"unknown\":\"caf\u{e9}\",\"serverTime\":1}";
    let (bytes, _, _) = encoding_rs::WINDOWS_1252.encode(json);
    assert_eq!(
        body(bytes.into_owned(), "application/json; charset=windows-1252")
            .decode()
            .unwrap()
            .1,
        1
    );
    let mut bytes = vec![0xff, 0xfe];
    for unit in json.encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    assert_eq!(body(bytes, "application/json; charset=utf-8").decode().unwrap().1, 1);
    for bytes in [
        b"{\"serverTime\":1}garbage".as_slice(),
        b"{\"serverTime\":-1}",
        b"{\"serverTime\":9223372036854775808}",
        b"{\"serverTime\":1,\"serverTime\":2}",
        b"{\"unknown\":1}",
    ] {
        assert_eq!(
            body(bytes.to_vec(), "application/json").decode().err(),
            Some(TimeError::Payload)
        );
    }
}
