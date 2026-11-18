use sqlx::migrate::MigrationType;

use super::*;

fn fixture(sql: &'static str) -> Migration {
    Migration::new(VERSION, "synthetic test only".into(), MigrationType::Simple, sql.into(), false)
}

const ONE_DO: &str = "DO $migration38$\nBEGIN NULL; END\n$migration38$;";

#[test]
fn embedded_contract_is_the_complete_inventory_not_a_synthetic_candidate() {
    let adapter = BoundedMigration {
        connection: None,
        status: Status { phase: Phase::Preflight, failure: None },
        contract: None,
        gate: Default::default(),
    };
    let contract = adapter.contract().expect("embedded38 contract");
    assert_eq!(contract.version, VERSION);
    assert_eq!(
        contract.sql.as_ref(),
        include_str!("../../../migrations/20240101000038_request_budget.sql")
    );
    assert!(preflight(contract, Some(contract)).is_ok());
    assert!(
        preflight(&fixture(ONE_DO), Some(contract)).is_err(),
        "synthetic SQL cannot replace embedded38"
    );
    let mut changed = contract.clone();
    changed.checksum.to_mut()[0] ^= 1;
    assert!(preflight(&changed, Some(contract)).is_err(), "checksum drift cannot select38");
}

#[test]
fn absent_compiled_contract_never_accepts_matching_metadata() {
    let candidate = fixture(ONE_DO);
    assert!(preflight(&candidate, None).is_err());
    assert!(preflight(&candidate, Some(&candidate)).is_ok());
}

#[test]
fn compiled_contract_requires_exact_sql_and_checksum() {
    let expected = fixture(ONE_DO);
    let mut changed_sql = expected.clone();
    changed_sql.sql = "DO $migration38$ BEGIN RAISE NOTICE 'changed'; END $migration38$;".into();
    assert_eq!(changed_sql.checksum, expected.checksum);
    assert!(preflight(&changed_sql, Some(&expected)).is_err());
    let mut changed_checksum = expected.clone();
    changed_checksum.checksum = vec![0; expected.checksum.len()].into();
    assert!(preflight(&changed_checksum, Some(&expected)).is_err());
    let independently_constructed = fixture(ONE_DO);
    assert!(preflight(&independently_constructed, Some(&expected)).is_ok());
}

#[test]
fn version_transaction_and_migration_kind_cannot_drift() {
    let expected = fixture(ONE_DO);
    let mut wrong_version = expected.clone();
    wrong_version.version -= 1;
    assert!(preflight(&wrong_version, Some(&expected)).is_err());
    assert!(preflight(&expected, Some(&wrong_version)).is_err());
    let mut no_transaction = expected.clone();
    no_transaction.no_tx = true;
    assert!(preflight(&no_transaction, Some(&expected)).is_err());
    assert!(preflight(&expected, Some(&no_transaction)).is_err());
    let mut down = expected.clone();
    down.migration_type = MigrationType::ReversibleDown;
    assert!(preflight(&down, Some(&expected)).is_err());
}

#[test]
fn canonical_do_rejects_multiple_top_level_statements() {
    for sql in [
        "SELECT 1",
        "DO $migration38$ BEGIN NULL; END $migration38$; SELECT 1;",
        "DO $migration38$ BEGIN NULL; END $migration38$; DO $migration38$ BEGIN NULL; END $migration38$;",
        "BEGIN; DO $migration38$ BEGIN NULL; END $migration38$; COMMIT;",
        "DO $wrong$ BEGIN NULL; END $wrong$;",
    ] {
        let candidate = fixture(sql);
        assert!(preflight(&candidate, Some(&candidate)).is_err(), "accepted {sql}");
    }
    let with_outer_whitespace = fixture("\n DO $migration38$ BEGIN NULL; END $migration38$; \n");
    assert!(preflight(&with_outer_whitespace, Some(&with_outer_whitespace)).is_ok());
}

#[test]
fn opaque_apply_and_postcommit_failures_never_mean_unapplied() {
    assert_eq!(interrupted(Phase::Preflight), Outcome::ApplyNotStarted);
    assert_eq!(interrupted(Phase::SetupStarted), Outcome::ApplyNotStarted);
    assert_eq!(interrupted(Phase::SetupConfirmed), Outcome::ApplyNotStarted);
    // This includes a server error from SQLx's postcommit elapsed UPDATE.
    assert_eq!(interrupted(Phase::DelegatedApply), Outcome::MayHaveCommitted);
    assert_eq!(interrupted(Phase::ApplyReturned), Outcome::AppliedButSessionUnrestored);
    assert_eq!(interrupted(Phase::RestoreStarted), Outcome::AppliedButSessionUnrestored);
}

#[tokio::test]
async fn retired_adapter_cannot_be_reused_by_any_migrate_entry() {
    let failure = Failure {
        at: Phase::DelegatedApply,
        outcome: Outcome::MayHaveCommitted,
    };
    let status = Status {
        phase: Phase::Retired,
        failure: Some(failure),
    };
    let mut adapter = BoundedMigration {
        connection: None,
        status,
        contract: None,
        gate: Default::default(),
    };
    let candidate = fixture(ONE_DO);
    let mut ordinary = candidate.clone();
    ordinary.version -= 1;
    assert!(adapter.ensure_migrations_table().await.is_err());
    assert!(adapter.dirty_version().await.is_err());
    assert!(adapter.list_applied_migrations().await.is_err());
    assert!(adapter.lock().await.is_err());
    assert!(adapter.unlock().await.is_err());
    assert!(adapter.apply(&candidate).await.is_err());
    assert!(adapter.apply(&ordinary).await.is_err());
    assert!(adapter.revert(&ordinary).await.is_err());
    assert_eq!(adapter.status(), status);
}
