use std::sync::Mutex;

use super::{Connection, Delivery, Event, Phase, SqlCategory, Trace};

#[test]
fn preparation_error_has_no_execution_ordinal() {
    let trace = Mutex::new(Some(Trace::new()));
    let connection = Connection::new(&trace, 1, None).unwrap();
    connection.frontend(b'P', SqlCategory::JournalAccounting, true);
    connection.frontend(b'D', SqlCategory::JournalAccounting, true);
    connection.frontend(b'E', SqlCategory::JournalAccounting, true);
    connection.frontend(b'S', SqlCategory::Other, true);
    let error = connection
        .backend(b'E', b"SERROR\0C42601\0Msynthetic-private-message\0\0")
        .unwrap();
    assert_eq!(error.phase, Phase::Parse);
    assert_eq!(error.ordinal, None);
    assert_eq!(error.sqlstate, Some(*b"42601"));
    connection.backend(b'Z', b"I");
    let snapshot = trace.lock().unwrap().as_ref().unwrap().snapshot();
    assert!(!format!("{snapshot:?}").contains("synthetic-private-message"));
    assert!(
        snapshot.records.iter().any(|r| r.event == Event::Skipped
            && r.phase == Phase::Execute
            && r.ordinal == Some(1)),
        "parse_error_must_mark_skipped_execution"
    );
}

#[test]
fn deferred_error_keeps_completed_execution_until_sync() {
    let trace = Mutex::new(Some(Trace::new()));
    let connection = Connection::new(&trace, 1, None).unwrap();
    connection.frontend(b'B', SqlCategory::LegWrite, true);
    connection.frontend(b'E', SqlCategory::LegWrite, true);
    connection.frontend(b'C', SqlCategory::Other, true);
    connection.frontend(b'S', SqlCategory::Other, true);
    connection.backend(b'2', b"");
    let complete = connection.backend(b'C', b"UPDATE 1\0").unwrap();
    connection.delivery(complete, Delivery::Held);
    connection.backend(b'3', b"");
    let error = connection.backend(b'E', b"SERROR\0C23514\0Mprivate\0\0").unwrap();
    assert_eq!(
        (error.phase, error.ordinal, error.category),
        (Phase::Execute, Some(1), SqlCategory::LegWrite)
    );
    let ready = connection.backend(b'Z', b"I").unwrap();
    assert_eq!(ready.ordinal, None);
    assert_eq!(ready.transaction, Some(b'I'));
}

#[test]
fn overflow_never_invents_an_execution_ordinal() {
    let trace = Mutex::new(Some(Trace::new()));
    let connection = Connection::new(&trace, 1, None).unwrap();
    for _ in 0..40 {
        connection.frontend(b'E', SqlCategory::FillWrite, true);
    }
    let current = trace.lock().unwrap().as_ref().unwrap().identities();
    assert!(
        current[0].attribution_unknown,
        "phase_overflow_must_publish_unknown_before_response"
    );
    let result = connection.backend(b'E', b"SERROR\0C57014\0\0").unwrap();
    assert_eq!((result.phase, result.ordinal), (Phase::Unknown, None));
    connection.backend(b'Z', b"E");
    connection.frontend(b'E', SqlCategory::FillWrite, true);
    let result = connection.backend(b'C', b"UPDATE 1\0").unwrap();
    assert_eq!(result.ordinal, None);
    let snapshot = trace.lock().unwrap().as_ref().unwrap().snapshot();
    assert_eq!(snapshot.phase_overflow, 1);
    assert!(snapshot.connections[0].attribution_unknown);
}

#[test]
fn pid_only_and_finite_trace_survive_close() {
    let trace = Mutex::new(Some(Trace::new()));
    let connection = Connection::new(&trace, 1, None).unwrap();
    connection.frontend(b'E', SqlCategory::FillWrite, true);
    let record = connection.backend(b'K', &[0, 0, 0, 42, 0xDE, 0xAD, 0xBE, 0xEF]).unwrap();
    assert_eq!(record.identity.pid, Some(42));
    assert_eq!(record.event, Event::BackendPid);
    assert_eq!(record.ordinal, None, "backend_pid_retained_cancel_secret_or_execution_ordinal");
    assert_eq!(record.phase, Phase::Unknown);
    assert_eq!(record.category, SqlCategory::Other);
    assert_eq!(record.sqlstate, None);
    assert_eq!(record.transaction, None);
    assert!(std::mem::size_of::<super::Record>() <= 128);
    for _ in 0..5_000 {
        connection.frontend(b'Q', SqlCategory::Begin, true);
    }
    drop(connection);
    let snapshot = trace.lock().unwrap().as_ref().unwrap().snapshot();
    assert_eq!(snapshot.records.len(), super::RECORD_LIMIT);
    assert!(snapshot.dropped > 0);
    assert!(!snapshot.connections[0].live);
    assert_eq!(snapshot.connections[0].pid, Some(42));
}

#[test]
fn only_canonical_unique_journal_startup_label_is_kept() {
    let mut packet = vec![0u8; 8];
    packet.extend_from_slice(b"user\0private\0application_name\0rbx-diag-c11-j1\0\0");
    assert_eq!(super::journal_label(&packet), Some(super::JournalLabel { case: 11, leg: 1 }));
    packet.pop();
    packet.extend_from_slice(b"application_name\0rbx-diag-c12-j0\0\0");
    assert_eq!(super::journal_label(&packet), None);
    let mut packet = vec![0u8; 8];
    packet.extend_from_slice(b"application_name\0rbx-diag-c01-j0\0\0");
    assert_eq!(super::journal_label(&packet), None);
}

#[test]
fn late_delivery_cannot_restore_pre_overflow_identity() {
    let trace = Mutex::new(Some(Trace::new()));
    let connection = Connection::new(&trace, 1, None).unwrap();
    connection.frontend(b'E', SqlCategory::FillWrite, true);
    let received = connection.backend(b'C', b"INSERT 0 1\0").unwrap();
    assert!(!received.identity.attribution_unknown);
    for _ in 0..33 {
        connection.frontend(b'E', SqlCategory::FillWrite, true);
    }
    connection.delivery(received, Delivery::Forwarded);
    let held = trace.lock().unwrap();
    let current = held.as_ref().unwrap().identities();
    let snapshot = held.as_ref().unwrap().snapshot();
    drop(held);
    assert!(current[0].attribution_unknown, "late delivery restored stale attribution");
    assert!(!snapshot.records.last().unwrap().identity.attribution_unknown);
}

#[test]
fn unexpected_ack_updates_identity_before_another_frame_arrives() {
    let trace = Mutex::new(Some(Trace::new()));
    let connection = Connection::new(&trace, 1, None).unwrap();
    connection.frontend(b'E', SqlCategory::FillWrite, true);
    connection.backend(b'1', b"");
    assert!(trace.lock().unwrap().as_ref().unwrap().identities()[0].attribution_unknown);
}

#[test]
fn two_completed_executes_one_sync_deferred_error_is_unknown() {
    let trace = Mutex::new(Some(Trace::new()));
    let connection = Connection::new(&trace, 1, None).unwrap();
    for tag in [b'B', b'E', b'B', b'E', b'C', b'S'] {
        connection.frontend(tag, SqlCategory::LegWrite, true);
    }
    for tag in [b'2', b'C', b'2', b'C', b'3'] {
        connection.backend(tag, b"");
    }
    let error = connection.backend(b'E', b"C23514\0Mdeferred-private-sentinel\0\0").unwrap();
    assert_eq!(
        (error.phase, error.ordinal, error.category),
        (Phase::Unknown, None, SqlCategory::Other),
        "multi_completion_deferred_error_falsely_named_last_execute"
    );
}

#[test]
fn simple_inside_unfinished_extended_cycle_stays_unknown_before_or_after_completion() {
    for completed in [false, true] {
        let trace = Mutex::new(Some(Trace::new()));
        let connection = Connection::new(&trace, 1, None).unwrap();
        connection.frontend(b'B', SqlCategory::LegWrite, true);
        connection.frontend(b'E', SqlCategory::LegWrite, true);
        if completed {
            connection.backend(b'2', b"");
            connection.backend(b'C', b"UPDATE 1\0");
        }
        let simple = connection.frontend(b'Q', SqlCategory::FillWrite, true).unwrap();
        assert_eq!(
            (simple.phase, simple.ordinal, simple.category),
            (Phase::Unknown, None, SqlCategory::Other),
            "unfinished_extended_simple_mix_claimed_known_query"
        );
        connection.frontend(b'S', SqlCategory::Other, true);
        let error = connection
            .backend(b'E', b"SERROR\0VERROR\0C23514\0Mprivate-mixed-error\0\0")
            .unwrap();
        assert_eq!((error.phase, error.ordinal), (Phase::Unknown, None));
    }
}
#[test]
fn simple_after_queued_sync_keeps_its_own_proven_boundary() {
    let trace = Mutex::new(Some(Trace::new()));
    let connection = Connection::new(&trace, 1, None).unwrap();
    for tag in [b'B', b'E', b'S'] {
        connection.frontend(tag, SqlCategory::LegWrite, true);
    }
    connection.frontend(b'Q', SqlCategory::FillWrite, true);
    connection.backend(b'2', b"");
    connection.backend(b'C', b"UPDATE 1\0");
    let deferred = connection.backend(b'E', b"SERROR\0VERROR\0C23514\0Mdeferred\0\0").unwrap();
    assert_eq!(
        (deferred.phase, deferred.ordinal, deferred.category),
        (Phase::Execute, Some(1), SqlCategory::LegWrite)
    );
    connection.backend(b'Z', b"I");
    let next = connection.backend(b'C', b"UPDATE 1\0").unwrap();
    assert_eq!(
        (next.phase, next.ordinal, next.category),
        (Phase::Simple, Some(2), SqlCategory::FillWrite),
        "queued_sync_boundary_was_lost"
    );
}
#[test]
fn extended_error_without_visible_sync_loses_future_attribution() {
    let trace = Mutex::new(Some(Trace::new()));
    let connection = Connection::new(&trace, 1, None).unwrap();
    for tag in [b'P', b'B', b'E'] {
        connection.frontend(tag, SqlCategory::LegWrite, true);
    }
    connection.backend(b'1', b"");
    connection.backend(b'2', b"");
    let error = connection.backend(b'E', b"SERROR\0VERROR\0C23514\0\0").unwrap();
    assert_eq!(
        (error.phase, error.ordinal, error.category),
        (Phase::Unknown, None, SqlCategory::Other),
        "missing_sync_error_retained_statement_association"
    );
    let current = trace.lock().unwrap().as_ref().unwrap().identities();
    assert!(
        current[0].attribution_unknown,
        "extended_error_without_sync_retained_known_boundary"
    );
    connection.frontend(b'Q', SqlCategory::FillWrite, true);
    connection.frontend(b'S', SqlCategory::Other, true);
    connection.backend(b'Z', b"I");
    connection.frontend(b'Q', SqlCategory::FillWrite, true);
    assert_eq!(connection.backend(b'C', b"UPDATE 1\0").unwrap().ordinal, None);
}
#[test]
fn unsupported_function_cycle_never_borrows_a_pipelined_parse() {
    let trace = Mutex::new(Some(Trace::new()));
    let connection = Connection::new(&trace, 1, None).unwrap();
    connection.frontend(b'F', SqlCategory::Other, true);
    for tag in [b'P', b'B', b'E', b'S'] {
        connection.frontend(tag, SqlCategory::LegWrite, true);
    }
    let error = connection
        .backend(b'E', b"SERROR\0VERROR\0C42883\0Mprivate-function-error\0\0")
        .unwrap();
    assert_eq!(
        (error.phase, error.ordinal, error.category),
        (Phase::Unknown, None, SqlCategory::Other),
        "unsupported_function_error_borrowed_parse_phase"
    );
}
#[test]
fn fatal_after_completion_is_connection_evidence_not_statement_attribution() {
    let trace = Mutex::new(Some(Trace::new()));
    let connection = Connection::new(&trace, 1, None).unwrap();
    for tag in [b'B', b'E', b'C', b'S'] {
        connection.frontend(tag, SqlCategory::LegWrite, true);
    }
    for tag in [b'2', b'C', b'3'] {
        connection.backend(tag, b"");
    }
    let error = connection
        .backend(b'E', b"SFATAL\0VFATAL\0C57P01\0Mprivate-shutdown\0\0")
        .unwrap();
    assert_eq!(
        (error.phase, error.ordinal, error.category),
        (Phase::Unknown, None, SqlCategory::Other),
        "fatal_response_claimed_prior_statement"
    );
    assert_eq!(error.sqlstate, Some(*b"57P01"));
}
#[test]
fn fatal_word_in_error_message_does_not_hide_valid_deferred_error() {
    let trace = Mutex::new(Some(Trace::new()));
    let connection = Connection::new(&trace, 1, None).unwrap();
    for tag in [b'B', b'E', b'C', b'S'] {
        connection.frontend(tag, SqlCategory::LegWrite, true);
    }
    for tag in [b'2', b'C', b'3'] {
        connection.backend(tag, b"");
    }
    let error = connection
        .backend(b'E', b"SERROR\0VERROR\0C23514\0MFATAL is only quoted message text\0\0")
        .unwrap();
    assert_eq!(
        (error.phase, error.ordinal, error.category),
        (Phase::Execute, Some(1), SqlCategory::LegWrite),
        "error_message_substring_changed_protocol_severity"
    );
}

#[test]
fn simple_error_ready_boundary_preserves_next_simple_query() {
    let trace = Mutex::new(Some(Trace::new()));
    let connection = Connection::new(&trace, 1, None).unwrap();
    connection.frontend(b'Q', SqlCategory::OrderWrite, true);
    let error = connection
        .backend(b'E', b"SERROR\0VERROR\0C23514\0Msimple-private\0\0")
        .unwrap();
    assert_eq!(
        (error.phase, error.ordinal, error.category),
        (Phase::Simple, Some(1), SqlCategory::OrderWrite),
        "simple_error_lost_own_query_association"
    );
    assert!(!error.identity.attribution_unknown);
    connection.backend(b'Z', b"I");
    connection.frontend(b'Q', SqlCategory::FillWrite, true);
    let next = connection.backend(b'C', b"UPDATE 1\0").unwrap();
    assert_eq!(
        (next.phase, next.ordinal, next.category),
        (Phase::Simple, Some(2), SqlCategory::FillWrite),
        "simple_error_lost_next_query_boundary"
    );
}
