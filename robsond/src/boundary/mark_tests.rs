//! 8p-4 proofs of the boundary mark and the owner's answer (8p PLAN-r3 §8 row
//! 8p-4): the kline's open and close times, the request's earliest time and
//! clock verdict, the limiter charge before the send, failed and refused
//! answers, the exact decimal close, and the (b1) answer's verdict. A fixture
//! venue records what it was asked, in order.
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use robson_exec::venue_ledger::ClockEvidence;
use rust_decimal_macros::dec;
use uuid::Uuid;

use super::*;

/// B of January 2027, the first boundary 8p owns, in milliseconds.
const B_MS: i64 = 1_798_761_600_000;
const OPEN_MS: i64 = B_MS - 60_000;
const CLOSE_MS: i64 = B_MS - 1;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Asked {
    Reserve(u32),
    Read { reserved: u32, start_ms: u64 },
}

/// A budget that grants or refuses, and one canned answer to any read.
struct Venue {
    busy: bool,
    answer: Result<(u16, String), String>,
    asked: Mutex<Vec<Asked>>,
}

impl Venue {
    fn answering(status: u16, body: &str) -> Self {
        Self {
            busy: false,
            answer: Ok((status, body.to_owned())),
            asked: Mutex::new(Vec::new()),
        }
    }

    fn asked(&self) -> Vec<Asked> {
        self.asked.lock().unwrap().clone()
    }
}

impl MarkVenue for Venue {
    /// The weight it charged.
    type Reserved = u32;

    fn try_reserve(&self, weight: u32) -> Result<u32, Failure> {
        self.asked.lock().unwrap().push(Asked::Reserve(weight));
        if self.busy {
            return Err("the budget is busy".into());
        }
        Ok(weight)
    }

    fn read_kline(
        &self,
        reserved: u32,
        start_ms: u64,
    ) -> impl Future<Output = Result<(u16, String), Failure>> + Send {
        self.asked.lock().unwrap().push(Asked::Read { reserved, start_ms });
        let answer = self.answer.clone().map_err(Failure::from);
        async move { answer }
    }
}

fn january() -> NaiveDate {
    NaiveDate::from_ymd_opt(2027, 1, 1).unwrap()
}

fn time(ms: i64) -> DateTime<Utc> {
    DateTime::from_timestamp_millis(ms).unwrap()
}

/// Robson's clock at `ms` with a valid verdict.
fn valid(ms: i64) -> RobsonTime {
    RobsonTime {
        at: time(ms),
        verdict: Some(ClockVerdict::Valid),
    }
}

/// One kline row as Binance gives it, with the given times and close.
fn row(open: i64, close_time: i64, close: &str) -> String {
    format!(
        r#"[{open},"97000.00000000","97100.00000000","96900.00000000",{close},"12.30000000",{close_time},"1193500.00000000",42,"6.10000000","592000.00000000","0"]"#
    )
}

fn one(open: i64, close_time: i64, close: &str) -> String {
    format!("[{}]", row(open, close_time, close))
}

const CLOSE: &str = r#""97050.12345678""#;

async fn mark(venue: &Venue, now: RobsonTime) -> Result<Decimal, Failure> {
    kline_at(venue, january(), now).await
}

#[tokio::test]
async fn kline_at_b_gives_its_exact_close() {
    let venue = Venue::answering(200, &one(OPEN_MS, CLOSE_MS, CLOSE));
    let close = mark(&venue, valid(B_MS + 60_000)).await.unwrap();
    assert_eq!((close, close.scale()), (dec!(97050.12345678), 8));
    let start_ms = u64::try_from(OPEN_MS).unwrap();
    assert_eq!(venue.asked(), [Asked::Reserve(2), Asked::Read { reserved: 2, start_ms }]);
}

#[tokio::test]
async fn kline_with_a_wrong_open_time_is_refused() {
    // The close time is B - 1 ms each time; only the open time is wrong.
    for open in [OPEN_MS - 60_000, OPEN_MS + 1, OPEN_MS - 1, B_MS] {
        let venue = Venue::answering(200, &one(open, CLOSE_MS, CLOSE));
        let refused = mark(&venue, valid(B_MS + 60_000)).await;
        assert!(refused.is_err(), "open {open}: {refused:?}");
        assert_eq!(venue.asked().len(), 2, "requested once, then refused");
    }
}

#[tokio::test]
async fn kline_with_a_wrong_close_time_is_refused() {
    // The open time is B - 60 s each time; only the close time is wrong.
    for close_time in [CLOSE_MS - 1, B_MS, B_MS + 59_999, CLOSE_MS + 60_000] {
        let venue = Venue::answering(200, &one(OPEN_MS, close_time, CLOSE));
        let refused = mark(&venue, valid(B_MS + 60_000)).await;
        assert!(refused.is_err(), "close time {close_time}: {refused:?}");
    }
}

#[tokio::test]
async fn kline_is_not_requested_before_b_plus_sixty_seconds() {
    let early = [
        valid(B_MS + 59_999),
        valid(B_MS),
        valid(B_MS - 1),
        RobsonTime {
            at: time(B_MS + 60_000),
            verdict: Some(ClockVerdict::Invalid),
        },
        RobsonTime {
            at: time(B_MS + 3_600_000),
            verdict: None,
        },
    ];
    for now in early {
        let venue = Venue::answering(200, &one(OPEN_MS, CLOSE_MS, CLOSE));
        assert!(mark(&venue, now).await.is_err(), "{now:?}");
        assert_eq!(venue.asked(), [], "nothing reserved or sent at {now:?}");
    }
    // A month named by another day than its first is no boundary.
    let venue = Venue::answering(200, &one(OPEN_MS, CLOSE_MS, CLOSE));
    let second = NaiveDate::from_ymd_opt(2027, 1, 2).unwrap();
    assert!(kline_at(&venue, second, valid(B_MS + 86_400_000)).await.is_err());
    assert_eq!(venue.asked(), []);
    // From B + 60 s on a valid clock it is requested.
    let venue = Venue::answering(200, &one(OPEN_MS, CLOSE_MS, CLOSE));
    assert_eq!(mark(&venue, valid(B_MS + 60_000)).await.unwrap(), dec!(97050.12345678));
}

#[tokio::test]
async fn kline_reserves_two_weight_before_send() {
    let venue = Venue::answering(200, &one(OPEN_MS, CLOSE_MS, CLOSE));
    mark(&venue, valid(B_MS + 61_000)).await.unwrap();
    let start_ms = u64::try_from(OPEN_MS).unwrap();
    assert_eq!(venue.asked(), [Asked::Reserve(2), Asked::Read { reserved: 2, start_ms }]);
    // A busy budget refuses at once: nothing is sent and K stays pending.
    let busy = Venue {
        busy: true,
        ..Venue::answering(200, &one(OPEN_MS, CLOSE_MS, CLOSE))
    };
    assert!(mark(&busy, valid(B_MS + 61_000)).await.is_err());
    assert_eq!(busy.asked(), [Asked::Reserve(2)]);
}

#[tokio::test]
async fn a_failed_or_refused_kline_read_keeps_k_pending() {
    let good = one(OPEN_MS, CLOSE_MS, CLOSE);
    let answers = [
        (429, good.clone()),
        (418, good.clone()),
        (500, good.clone()),
        (200, String::new()),
        (200, "[]".to_owned()),
        (
            200,
            format!("[{},{}]", row(OPEN_MS, CLOSE_MS, CLOSE), row(B_MS, B_MS + 59_999, CLOSE)),
        ),
        (200, r#"{"code":-1121,"msg":"Invalid symbol."}"#.to_owned()),
        (200, format!("[[{OPEN_MS}]]")),
        (200, good.replace(&format!("[{OPEN_MS},"), &format!("[\"{OPEN_MS}\","))),
        (200, good.replace(&format!(",{CLOSE_MS},"), &format!(",{CLOSE_MS}.0,"))),
    ];
    for (status, body) in answers {
        let venue = Venue::answering(status, &body);
        let refused = mark(&venue, valid(B_MS + 60_000)).await;
        assert!(refused.is_err(), "HTTP {status} {body}: {refused:?}");
    }
    let failed = Venue {
        answer: Err("connection reset".into()),
        ..Venue::answering(200, "")
    };
    assert!(mark(&failed, valid(B_MS + 60_000)).await.is_err());
    assert_eq!(failed.asked().len(), 2);
}

#[tokio::test(start_paused = true)]
async fn a_kline_read_without_an_answer_ends_at_its_deadline() {
    struct Silent;
    impl MarkVenue for Silent {
        type Reserved = ();

        fn try_reserve(&self, _: u32) -> Result<(), Failure> {
            Ok(())
        }

        fn read_kline(
            &self,
            _: (),
            _: u64,
        ) -> impl Future<Output = Result<(u16, String), Failure>> + Send {
            std::future::pending()
        }
    }
    let started = tokio::time::Instant::now();
    assert!(kline_at(&Silent, january(), valid(B_MS + 60_000)).await.is_err());
    assert_eq!(started.elapsed(), Duration::from_secs(10));
}

#[tokio::test]
async fn kline_close_is_an_exact_positive_decimal() {
    let refused = [
        r#""0""#,
        r#""0.00000000""#,
        r#""-97050.1""#,
        "97050.12345678",
        r#""9.705e4""#,
        r#""97050.1234567890123456789012345678""#,
        r#""1.00000000000000000000000000001""#,
        r#""""#,
        "null",
    ];
    for close in refused {
        let venue = Venue::answering(200, &one(OPEN_MS, CLOSE_MS, close));
        let answer = mark(&venue, valid(B_MS + 60_000)).await;
        assert!(answer.is_err(), "close {close}: {answer:?}");
    }
    let exact = [
        (r#""0.00000001""#, dec!(0.00000001)),
        (r#""123456.7890123456789012345678""#, dec!(123456.7890123456789012345678)),
    ];
    for (close, expected) in exact {
        let venue = Venue::answering(200, &one(OPEN_MS, CLOSE_MS, close));
        let mark = mark(&venue, valid(B_MS + 60_000)).await.unwrap();
        assert_eq!((mark, mark.scale()), (expected, expected.scale()), "close {close}");
    }
}

#[test]
fn the_owner_answer_hands_8p3_its_status_and_send_verdict() {
    let evidence = |verdict| ClockEvidence {
        sample_id: Uuid::from_u128(7),
        wall_delta_ms: 3,
        mono_delta_ms: 4,
        verdict,
    };
    for (clock, verdict) in [
        (Some(evidence(ClockVerdict::Valid)), Some(ClockVerdict::Valid)),
        (Some(evidence(ClockVerdict::Invalid)), Some(ClockVerdict::Invalid)),
        (None, None),
    ] {
        let sent = SeenAt {
            at: time(B_MS + 5_000),
            clock,
            capture: None,
        };
        let answer = LookupAnswer { status: "NEW".into(), sent };
        let lookup = answer.lookup();
        assert_eq!(lookup.status, "NEW");
        assert_eq!(lookup.sent, RobsonTime { at: time(B_MS + 5_000), verdict });
    }
}
