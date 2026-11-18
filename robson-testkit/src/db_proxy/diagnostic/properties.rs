//! Generated protocol grammars supply the reference associations independently:
//! expectations are attached to server messages when the script is constructed,
//! before any trace implementation is invoked. No parser internals are reused.
use super::*;

#[derive(Clone)]
struct Reply {
    tag: u8,
    body: &'static [u8],
    expected: Option<(Phase, Option<u64>, SqlCategory)>,
    invalidates_cache: bool,
}
struct Group {
    requests: Vec<(u8, SqlCategory)>,
    replies: Vec<Reply>,
}
fn reply(tag: u8, expected: Option<(Phase, Option<u64>, SqlCategory)>) -> Reply {
    Reply {
        tag,
        body: match tag {
            b'E' => b"SERROR\0C23514\0Mnever-retained-sentinel\0Dprivate\0\0",
            b'Z' => b"I",
            _ => b"",
        },
        expected,
        invalidates_cache: false,
    }
}
fn group(kind: u64, ordinal: &mut u64) -> Group {
    let cat = SqlCategory::LegWrite;
    let unknown = Some((Phase::Unknown, None, SqlCategory::Other));
    let mut requests = Vec::new();
    let mut replies = Vec::new();
    match kind % 9 {
        0 => {
            *ordinal += 1;
            requests.push((b'Q', cat));
            replies.push(reply(b'C', Some((Phase::Simple, Some(*ordinal), cat))));
            replies.push(reply(b'Z', unknown));
        },
        1 => {
            requests.extend([
                (b'P', cat),
                (b'D', SqlCategory::Other),
                (b'S', SqlCategory::Other),
            ]);
            replies.extend([reply(b'1', None), reply(b'n', None), reply(b'Z', unknown)]);
        },
        2 | 3 => {
            *ordinal += 1;
            requests.extend([
                (b'B', cat),
                (b'E', cat),
                (b'C', SqlCategory::Other),
                (b'S', SqlCategory::Other),
            ]);
            replies.extend([
                reply(b'2', None),
                reply(b'C', Some((Phase::Execute, Some(*ordinal), cat))),
                reply(b'3', None),
            ]);
            if kind % 9 == 3 {
                replies.push(reply(b'E', Some((Phase::Execute, Some(*ordinal), cat))));
            }
            replies.push(reply(b'Z', unknown));
        },
        4 | 5 => {
            *ordinal += 1;
            let phase = if kind % 9 == 4 {
                Phase::Parse
            } else {
                Phase::Bind
            };
            if phase == Phase::Parse {
                requests.push((b'P', cat));
            }
            requests.extend([(b'B', cat), (b'E', cat), (b'S', SqlCategory::Other)]);
            replies.extend([reply(b'E', Some((phase, None, cat))), reply(b'Z', unknown)]);
        },
        6 => {
            *ordinal += 1;
            requests.extend([(b'B', cat), (b'E', cat), (b'S', SqlCategory::Other)]);
            replies.extend([
                reply(b'2', None),
                reply(b'E', Some((Phase::Execute, Some(*ordinal), cat))),
                reply(b'Z', unknown),
            ]);
        },
        7 => {
            *ordinal += 2;
            requests.extend([
                (b'B', cat),
                (b'E', cat),
                (b'B', cat),
                (b'E', cat),
                (b'C', SqlCategory::Other),
                (b'S', SqlCategory::Other),
            ]);
            replies.extend([
                reply(b'2', None),
                reply(b'C', Some((Phase::Execute, Some(*ordinal - 1), cat))),
                reply(b'2', None),
                reply(b'C', Some((Phase::Execute, Some(*ordinal), cat))),
                reply(b'3', None),
                reply(b'E', unknown),
                reply(b'Z', unknown),
            ]);
        },
        _ => {
            *ordinal += 1;
            requests.extend([
                (b'B', cat),
                (b'E', cat),
                (b'P', cat),
                (b'B', cat),
                (b'S', SqlCategory::Other),
            ]);
            replies.extend([
                reply(b'2', None),
                reply(b'E', Some((Phase::Execute, Some(*ordinal), cat))),
                reply(b'Z', unknown),
            ]);
        },
    }
    // Failed or server-skipped preparation makes the existing frontend cache
    // unproven. This reference fact comes from the constructed server script.
    if matches!(kind % 9, 4 | 5 | 8) {
        for response in &mut replies {
            response.invalidates_cache = response.tag == b'E';
        }
    }
    Group { requests, replies }
}

#[test]
fn generated_protocol_grammar_preserves_independent_associations() {
    for seed in 0..128u64 {
        let trace = Mutex::new(Some(Trace::new()));
        let connection = Connection::new(&trace, 1, None).unwrap();
        let mut rng = seed + 1;
        let mut ordinal = 0;
        let mut actions = 0;
        let mut cache_unproven = false;
        for batch in 0..8 {
            rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let mut groups = Vec::new();
            for n in 0..1 + (rng % 2) {
                let g = group((rng >> (n * 7)) + batch, &mut ordinal);
                for &(tag, category) in &g.requests {
                    let local = connection.frontend(tag, category, true).unwrap();
                    connection.delivery(local, Delivery::Forwarded);
                    actions += 1;
                }
                groups.push(g);
            }
            for g in groups {
                for expected in g.replies {
                    let actual = connection.backend(expected.tag, expected.body);
                    match expected.expected {
                        None => assert!(actual.is_none(), "grammar_metadata_ack"),
                        Some(mut want) => {
                            if cache_unproven {
                                want = (Phase::Unknown, None, SqlCategory::Other);
                            }
                            let actual = actual.unwrap();
                            assert_eq!(
                                (actual.phase, actual.ordinal, actual.category),
                                want,
                                "grammar_response_attribution seed={seed}"
                            );
                            if expected.tag == b'E' {
                                assert_eq!(actual.sqlstate, Some(*b"23514"));
                            }
                            connection.delivery(
                                actual,
                                if rng % 2 == 0 {
                                    Delivery::Dropped
                                } else {
                                    Delivery::Forwarded
                                },
                            );
                            cache_unproven |= expected.invalidates_cache;
                        },
                    }
                    actions += 1;
                }
            }
        }
        // Include the 37 overflow-tail frames and both connection closes.
        assert!(actions + 39 <= 256);
        assert_eq!(
            trace.lock().unwrap().as_ref().unwrap().identities()[0].attribution_unknown,
            cache_unproven
        );
        drop(connection);
        // Overflow needs a fresh frontier, independently of preparation loss.
        let connection = Connection::new(&trace, 2, None).unwrap();
        // Lost protocol boundaries cannot be repaired by a bare ReadyForQuery.
        for _ in 0..33 {
            connection.frontend(b'E', SqlCategory::FillWrite, true);
        }
        let e = connection.backend(b'E', b"C57014\0Mprivate\0\0").unwrap();
        assert_eq!((e.phase, e.ordinal), (Phase::Unknown, None), "grammar_overflow_attribution");
        connection.backend(b'Z', b"E");
        connection.frontend(b'Q', SqlCategory::Commit, true);
        let e = connection.backend(b'C', b"COMMIT\0").unwrap();
        assert_eq!(e.ordinal, None, "grammar_bare_ready_restored_lost_group");
        drop(connection);
        let snapshot = trace.lock().unwrap().as_ref().unwrap().snapshot();
        assert!(snapshot.records.len() <= RECORD_LIMIT);
        assert_eq!(snapshot.phase_overflow, 1);
        assert!(!format!("{snapshot:?}").contains("sentinel"));
        assert!(snapshot.connections.iter().all(|identity| !identity.live));
    }
}

#[test]
fn malformed_codes_multi_queries_and_connection_cap_stay_unknown() {
    for body in [
        b"C57014\0Msecret\0".as_slice(),
        b"C57014\0C23514\0\0",
        b"C57x14\0\0",
        b"C\0\0",
    ] {
        assert_eq!(sqlstate(body), None, "malformed_sqlstate_cannot_authorize_refusal");
    }
    let trace = Mutex::new(Some(Trace::new()));
    let mut connections = Vec::new();
    for n in 0..CONNECTION_LIMIT {
        connections.push(Connection::new(&trace, n as u64, None).unwrap());
    }
    assert!(Connection::new(&trace, 100, None).is_none());
    let c = &connections[0];
    c.frontend(b'Q', SqlCategory::Commit, false);
    let complete = c.backend(b'C', b"COMMIT\0").unwrap();
    assert_eq!((complete.phase, complete.ordinal), (Phase::Unknown, None));
    let snapshot = trace.lock().unwrap().as_ref().unwrap().snapshot();
    assert_eq!(snapshot.connection_overflow, 1);
    assert_eq!(snapshot.connections.len(), CONNECTION_LIMIT);
}

#[test]
fn generated_cycle_boundaries_do_not_borrow_unrelated_statement_authority() {
    for seed in 0..128u64 {
        let trace = Mutex::new(Some(Trace::new()));
        let connection = Connection::new(&trace, 1, None).unwrap();
        let category = if seed & 1 == 0 {
            SqlCategory::LegWrite
        } else {
            SqlCategory::FillWrite
        };
        let expected = match seed % 4 {
            0 => {
                connection.frontend(b'B', category, true);
                connection.frontend(b'E', category, true);
                if seed & 4 != 0 {
                    connection.backend(b'2', b"");
                    connection.backend(b'C', b"UPDATE 1\0");
                }
                connection.frontend(b'Q', SqlCategory::OrderWrite, true);
                connection.frontend(b'S', SqlCategory::Other, true);
                Some((Phase::Unknown, None, SqlCategory::Other))
            },
            1 => {
                for tag in [b'B', b'E', b'S'] {
                    connection.frontend(tag, category, true);
                }
                connection.frontend(b'Q', SqlCategory::OrderWrite, true);
                connection.backend(b'2', b"");
                connection.backend(b'C', b"UPDATE 1\0");
                Some((Phase::Execute, Some(1), category))
            },
            2 => {
                connection.frontend(b'F', SqlCategory::Other, true);
                for tag in [b'P', b'B', b'E', b'S'] {
                    connection.frontend(tag, category, true);
                }
                Some((Phase::Unknown, None, SqlCategory::Other))
            },
            _ => {
                for tag in [b'B', b'E', b'C', b'S'] {
                    connection.frontend(tag, category, true);
                }
                for tag in [b'2', b'C', b'3'] {
                    connection.backend(tag, b"");
                }
                Some((Phase::Unknown, None, SqlCategory::Other))
            },
        }
        .unwrap();
        let body = if seed % 4 == 3 {
            b"SFATAL\0VFATAL\0C57P01\0Mprivate\0\0".as_slice()
        } else {
            b"SERROR\0VERROR\0C23514\0Mprivate\0\0".as_slice()
        };
        let error = connection.backend(b'E', body).unwrap();
        assert_eq!(
            (error.phase, error.ordinal, error.category),
            expected,
            "generated_cycle_boundary_false_attribution seed={seed}"
        );
        connection.backend(b'Z', b"I");
        if seed % 4 == 1 {
            let next = connection.backend(b'C', b"UPDATE 1\0").unwrap();
            assert_eq!(
                (next.phase, next.ordinal, next.category),
                (Phase::Simple, Some(2), SqlCategory::OrderWrite)
            );
        }
    }
}
