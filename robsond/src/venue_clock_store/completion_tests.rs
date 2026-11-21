//! Counts/error classification only; these do not prove SQLx/PG completion.
use std::{error::Error, io};

use proptest::prelude::*;

use super::*;

#[test]
fn positive_counts_require_one_row_and_one_complete_result() {
    assert_eq!(Counts::default().finish(), Err(StoreError::Unpublished));
    let mut row_only = Counts::default();
    row_only.row().unwrap();
    assert_eq!(row_only.finish(), Err(StoreError::Unpublished));
    let mut command_only = Counts::default();
    command_only.command(1).unwrap();
    assert_eq!(command_only.finish(), Err(StoreError::Unpublished));
    let mut complete = Counts::default();
    complete.row().unwrap();
    complete.command(1).unwrap();
    assert_eq!(complete.finish(), Ok(()));
}

#[test]
fn extra_rows_results_or_wrong_affected_count_refuse() {
    let mut rows = Counts::default();
    rows.row().unwrap();
    assert_eq!(rows.row(), Err(StoreError::Unpublished));
    let mut results = Counts::default();
    results.command(1).unwrap();
    assert_eq!(results.command(1), Err(StoreError::Unpublished));
    for n in [0, 2, u64::MAX] {
        assert_eq!(Counts::default().command(n), Err(StoreError::Unpublished));
    }
}

#[derive(Debug, thiserror::Error)]
#[error("known test database refusal")]
struct KnownDatabase;
impl sqlx::error::DatabaseError for KnownDatabase {
    fn message(&self) -> &str {
        "known test database refusal"
    }
    fn as_error(&self) -> &(dyn Error + Send + Sync + 'static) {
        self
    }
    fn as_error_mut(&mut self) -> &mut (dyn Error + Send + Sync + 'static) {
        self
    }
    fn into_error(self: Box<Self>) -> Box<dyn Error + Send + Sync + 'static> {
        self
    }
    fn kind(&self) -> sqlx::error::ErrorKind {
        sqlx::error::ErrorKind::Other
    }
}

#[test]
fn explicit_server_protocol_and_decode_errors_are_terminal() {
    let known = [
        sqlx::Error::Database(Box::new(KnownDatabase)),
        sqlx::Error::Protocol("invalid response".into()),
        sqlx::Error::Decode(Box::new(io::Error::other("invalid value"))),
        sqlx::Error::ColumnDecode {
            index: "sample_id".into(),
            source: Box::new(io::Error::other("invalid value")),
        },
        sqlx::Error::TypeNotFound { type_name: "unexpected".into() },
        sqlx::Error::ColumnNotFound("missing".into()),
        sqlx::Error::InvalidArgument("invalid argument".into()),
        sqlx::Error::Configuration(Box::new(io::Error::other("invalid configuration"))),
        sqlx::Error::PoolTimedOut,
        sqlx::Error::WorkerCrashed,
    ];
    for error in known {
        assert_eq!(classify(error), StoreError::Invalid);
    }
}

#[test]
fn only_io_or_tls_errors_are_transport_ambiguity() {
    for error in [
        sqlx::Error::Io(io::Error::from(io::ErrorKind::ConnectionReset)),
        sqlx::Error::Tls(Box::new(io::Error::from(io::ErrorKind::UnexpectedEof))),
    ] {
        assert_eq!(classify(error), StoreError::Backend);
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 128, failure_persistence: None, ..ProptestConfig::default() })]

    #[test]
    fn count_acceptance_matches_complete_independent_inventory(events in prop::collection::vec(0u8..4, 0..8)) {
        let expected = events.len() == 2 && events.contains(&0) && events.contains(&1);
        let mut counts = Counts::default();
        let result = events.iter().try_for_each(|event| match event {
            0 => counts.row(),
            1 => counts.command(1),
            2 => counts.command(0),
            _ => counts.command(2),
        }).and_then(|()| counts.finish());
        prop_assert_eq!(result.is_ok(), expected);
    }
}
