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

#[test]
fn embedded39_is_selected_and_is_one_transactional_statement() {
    let contract = contract39().expect("embedded39 contract");
    assert_eq!(
        contract.sql.as_ref(),
        include_str!("../../../migrations/20240101000039_venue_order_quote_floor.sql")
    );
    assert!(selects39(contract, Some(contract)));
    assert!(selects39(&contract.clone(), contract39()), "production inventory must select39");
    assert!(preflight39(contract).is_ok());
    assert!(!selects39(contract, None));
}

#[test]
fn only_the_exact_embedded39_identity_selects_the_bounded39_path() {
    let contract = contract39().expect("embedded39 contract");
    let witness = Migration::new(
        VERSION39,
        "ordinary witness".into(),
        MigrationType::Simple,
        "SELECT 1".into(),
        false,
    );
    assert!(!selects39(&witness, Some(contract)), "ordinary39 witnesses keep SQLx's path");
    let mut changed = contract.clone();
    changed.checksum.to_mut()[0] ^= 1;
    assert!(!selects39(&changed, Some(contract)));
    let mut no_tx = contract.clone();
    no_tx.no_tx = true;
    assert!(!selects39(&no_tx, Some(contract)));
    let mut other = contract.clone();
    other.version = VERSION;
    assert!(!selects39(&other, Some(contract)));
}

#[test]
fn production_refuses_every_non_embedded39_before_sql() {
    let contract = contract39().expect("embedded39 contract");
    assert!(matches!(route39(contract, contract39(), false), Ok(true)));
    assert!(matches!(route39(contract, contract39(), true), Ok(true)));
    let witness = Migration::new(
        VERSION39,
        "ordinary witness".into(),
        MigrationType::Simple,
        "SELECT 1".into(),
        false,
    );
    assert!(route39(&witness, contract39(), false).is_err(), "production ran ordinary39");
    assert!(route39(&witness, None, false).is_err(), "absent embedded39 ran ordinary39");
    assert!(matches!(route39(&witness, contract39(), true), Ok(false)));
    let mut changed = contract.clone();
    changed.checksum.to_mut()[0] ^= 1;
    assert!(route39(&changed, contract39(), false).is_err());
    let mut later = witness.clone();
    later.version += 1;
    assert!(matches!(route39(&later, contract39(), false), Ok(false)));
}

#[test]
fn preflight39_refuses_untimed_or_untransacted_shapes() {
    let selected = |sql: &'static str| {
        Migration::new(VERSION39, "shape".into(), MigrationType::Simple, sql.into(), false)
    };
    assert!(preflight39(&selected("ALTER TABLE t ADD COLUMN c INTEGER;\n")).is_ok());
    let header = "-- refused; kept\n\n  --\n-- a; b;\nALTER TABLE t ADD COLUMN c INTEGER;\n";
    assert!(preflight39(&selected(header)).is_ok(), "header semicolons are not commands");
    for sql in [
        "ALTER TABLE t ADD COLUMN c INTEGER",
        "ALTER TABLE t ADD COLUMN c INTEGER; SELECT 1;",
        "BEGIN; ALTER TABLE t ADD COLUMN c INTEGER; COMMIT;",
        "-- header;\nALTER TABLE t ADD COLUMN c INTEGER; SELECT 1;",
        "-- header;\nBEGIN; ALTER TABLE t ADD COLUMN c INTEGER; COMMIT;",
        "-- only a comment;",
        "-- header;\n",
        // After code starts, a `--` line may be quoted; it still counts.
        "SELECT $$\n--$$; SELECT 1;\nSELECT 2;",
        "ALTER TABLE t\n-- inner;\nADD COLUMN c INTEGER;",
        "ALTER TABLE t ADD COLUMN c INTEGER;\n-- trailing;",
    ] {
        assert!(preflight39(&selected(sql)).is_err(), "accepted {sql}");
    }
    let mut no_tx = selected("ALTER TABLE t ADD COLUMN c INTEGER;");
    no_tx.no_tx = true;
    assert!(preflight39(&no_tx).is_err());
    let mut down = selected("ALTER TABLE t ADD COLUMN c INTEGER;");
    down.migration_type = MigrationType::ReversibleDown;
    assert!(preflight39(&down).is_err());
}

#[test]
fn embedded40_identity_and_canonical_do_shape() {
    let contract = contract40().expect("embedded40 contract");
    assert_eq!(
        contract.sql.as_ref(),
        include_str!("../../../migrations/20240101000040_balance_transitions.sql")
    );
    assert_eq!(contract.version, VERSION40);
    assert!(matches_embedded(&contract.clone(), contract));
    assert!(preflight40(contract).is_ok());

    let mut changed = [
        contract.clone(),
        contract.clone(),
        contract.clone(),
        contract.clone(),
        contract.clone(),
        contract.clone(),
    ];
    changed[0].version += 1;
    changed[1].description = "other".into();
    changed[2].sql = "SELECT 1".into();
    changed[3].checksum.to_mut()[0] ^= 1;
    changed[4].migration_type = MigrationType::ReversibleDown;
    changed[5].no_tx = true;
    assert!(changed.iter().all(|candidate| !matches_embedded(candidate, contract)));
    assert!(preflight40(&changed[4]).is_err());
    assert!(preflight40(&changed[5]).is_err());

    for sql in [
        "DO $migration40$\nBEGIN NULL; END;\n$migration40$",
        "-- header\nDO $migration40$\nBEGIN NULL; END;\n$migration40$;",
        "DO $migration40$\nBEGIN NULL; END;\n$migration40$; SELECT 1;",
        "DO $migration40$\nBEGIN NULL; END;\n$migration40$; DO $migration40$\nBEGIN NULL; END;\n$migration40$;",
    ] {
        let mut invalid = contract.clone();
        invalid.sql = sql.into();
        assert!(preflight40(&invalid).is_err(), "accepted {sql}");
    }
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

type Preflight = fn(&Migration) -> Result<(), MigrateError>;

/// The S8-4 files (PLAN-r4 §4.2): version, exact bytes, DO delimiter,
/// preflight and the DDL commands of each, in order.
const SPOT_ACCOUNTING: [(i64, &str, &str, Preflight, &[&str]); 5] = [
    (
        VERSION46,
        include_str!("../../../migrations/20240101000046_spot_cash_anchors.sql"),
        "$migration46$",
        preflight46,
        &[
            "CREATE FUNCTION public.cash_anchor_moves_forward()",
            "CREATE TABLE public.cash_anchors",
            "CREATE TRIGGER cash_anchors_move_forward",
            "CREATE TABLE public.cash_checkpoints",
        ],
    ),
    (
        VERSION47,
        include_str!("../../../migrations/20240101000047_spot_cash_discrepancies.sql"),
        "$migration47$",
        preflight47,
        &[
            "CREATE FUNCTION public.cash_discrepancy_moves_forward()",
            "CREATE TABLE public.cash_discrepancies",
            "CREATE TRIGGER cash_discrepancies_move_forward",
            "CREATE UNIQUE INDEX idx_cash_discrepancies_open",
        ],
    ),
    (
        VERSION48,
        include_str!("../../../migrations/20240101000048_spot_cash_reanchors.sql"),
        "$migration48$",
        preflight48,
        &[
            "CREATE TABLE public.cash_sightings",
            "CREATE TRIGGER cash_sightings_are_immutable",
            "CREATE TABLE public.cash_reanchors",
            "CREATE TRIGGER cash_reanchors_are_immutable",
        ],
    ),
    (
        VERSION49,
        include_str!("../../../migrations/20240101000049_spot_cash_flows_dust.sql"),
        "$migration49$",
        preflight49,
        &[
            "CREATE TABLE public.cash_attested_flows",
            "CREATE TRIGGER cash_attested_flows_are_immutable",
            "CREATE TABLE public.dust_ledger",
            "CREATE TRIGGER dust_ledger_is_immutable",
        ],
    ),
    (
        VERSION50,
        include_str!("../../../migrations/20240101000050_spot_accounting_invalidations.sql"),
        "$migration50$",
        preflight50,
        &[
            "CREATE TABLE public.accounting_invalidations",
            "CREATE TRIGGER accounting_invalidations_are_immutable",
        ],
    ),
];

fn spot_accounting_contract(version: i64) -> &'static Migration {
    match version {
        VERSION46 => contract46(),
        VERSION47 => contract47(),
        VERSION48 => contract48(),
        VERSION49 => contract49(),
        VERSION50 => contract50(),
        _ => None,
    }
    .expect("embedded S8-4 contract")
}

#[test]
fn embedded46_to_50_identity_and_canonical_do_shape() {
    for (version, file, delimiter, preflight, _) in SPOT_ACCOUNTING {
        let contract = spot_accounting_contract(version);
        assert_eq!(contract.version, version);
        assert_eq!(contract.sql.as_ref(), file);
        assert!(!contract.no_tx && contract.migration_type == MigrationType::Simple);
        assert!(matches_embedded(&contract.clone(), contract));
        assert!(preflight(contract).is_ok());

        let mut changed = vec![contract.clone(); 6];
        changed[0].version += 1;
        changed[1].description = "other".into();
        changed[2].sql = "SELECT 1".into();
        changed[3].checksum.to_mut()[0] ^= 1;
        changed[4].migration_type = MigrationType::ReversibleUp;
        changed[5].no_tx = true;
        assert!(changed.iter().all(|candidate| !matches_embedded(candidate, contract)));
        assert!(preflight(&changed[4]).is_err());
        assert!(preflight(&changed[5]).is_err());

        for sql in [
            format!("DO {delimiter}\nBEGIN NULL; END;\n{delimiter}"),
            format!("-- header\nDO {delimiter}\nBEGIN NULL; END;\n{delimiter};"),
            format!("DO {delimiter}\nBEGIN NULL; END;\n{delimiter}; SELECT 1;"),
            format!(
                "DO {delimiter}\nBEGIN NULL; END;\n{delimiter}; DO {delimiter}\nBEGIN NULL; END;\n{delimiter};"
            ),
            "DO $migration45$\nBEGIN NULL; END;\n$migration45$;".to_owned(),
        ] {
            let mut invalid = contract.clone();
            invalid.sql = sql.clone().into();
            assert!(preflight(&invalid).is_err(), "accepted {sql}");
        }
        for (other, ..) in SPOT_ACCOUNTING {
            if other != version {
                let foreign = spot_accounting_contract(other);
                let mut renumbered = foreign.clone();
                renumbered.version = version;
                assert!(!matches_embedded(&renumbered, contract), "{other} as {version}");
                assert!(preflight(foreign).is_err(), "{version} accepted {other}'s DO");
            }
        }
    }
}

#[tokio::test]
async fn non_identical_46_to_50_are_refused_before_any_connection_use() {
    // No connection at all: a refusal must come from the contract check, an
    // identical source reaches the owned apply and finds the connection gone.
    let mut adapter = BoundedMigration {
        connection: None,
        status: Status { phase: Phase::Preflight, failure: None },
        contract: None,
        gate: Default::default(),
    };
    for (version, ..) in SPOT_ACCOUNTING {
        let contract = spot_accounting_contract(version);
        let expected = format!("migration{} contract mismatch", version % 100);
        let mut checksum = contract.clone();
        checksum.checksum.to_mut()[0] ^= 1;
        let mut sql = contract.clone();
        sql.sql = "SELECT 1".into();
        let mut no_tx = contract.clone();
        no_tx.no_tx = true;
        let mut description = contract.clone();
        description.description = "other".into();
        for wrong in [checksum, sql, no_tx, description] {
            let error = adapter.apply(&wrong).await.expect_err("non-identical source applied");
            assert!(
                matches!(&error, MigrateError::Source(source) if source.to_string() == expected),
                "{version}: {error}"
            );
        }
        let identical = adapter.apply(contract).await.expect_err("no connection");
        assert!(matches!(
            &identical,
            MigrateError::Source(source) if source.to_string() == "migration connection is retired"
        ));
        assert_eq!(adapter.status(), Status { phase: Phase::Preflight, failure: None });
    }
}

/// Top-level statements of a canonical DO body between its `BEGIN` and final
/// `END;`, read past comments, string literals and nested dollar quotes.
fn do_body_statements(sql: &str, delimiter: &str) -> Vec<String> {
    let body = sql
        .trim_end()
        .strip_prefix(&format!("DO {delimiter}\n"))
        .and_then(|sql| sql.strip_suffix(&format!("\n{delimiter};")))
        .expect("canonical DO wrapper");
    let body = body
        .trim()
        .strip_prefix("BEGIN")
        .and_then(|body| body.strip_suffix("END;"))
        .expect("one BEGIN ... END; block");
    let chars: Vec<char> = body.chars().collect();
    let (mut statements, mut current, mut i) = (Vec::new(), String::new(), 0);
    while i < chars.len() {
        match chars[i] {
            '-' if chars.get(i + 1) == Some(&'-') => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            },
            '/' if chars.get(i + 1) == Some(&'*') => {
                let rest: String = chars[i..].iter().collect();
                let end = rest.find("*/").expect("closed block comment");
                i += rest[..end + 2].chars().count();
            },
            '\'' => {
                let mut j = i + 1;
                loop {
                    assert!(j < chars.len(), "unclosed string literal");
                    if chars[j] == '\'' && chars.get(j + 1) == Some(&'\'') {
                        j += 2;
                    } else if chars[j] == '\'' {
                        break;
                    } else {
                        j += 1;
                    }
                }
                current.extend(&chars[i..=j]);
                i = j + 1;
            },
            '$' if !chars.get(i + 1).is_some_and(char::is_ascii_digit) => {
                let mut j = i + 1;
                while j < chars.len() && (chars[j].is_ascii_alphanumeric() || chars[j] == '_') {
                    j += 1;
                }
                assert_eq!(chars.get(j), Some(&'$'), "dollar sign outside a quote tag");
                let tag: String = chars[i..=j].iter().collect();
                let after: String = chars[j + 1..].iter().collect();
                let end = after.find(&tag).expect("closed dollar quote");
                let quoted = tag.chars().count() * 2 + after[..end].chars().count();
                current.extend(&chars[i..i + quoted]);
                i += quoted;
            },
            ';' => {
                statements.push(current.split_whitespace().collect::<Vec<_>>().join(" "));
                current.clear();
                i += 1;
            },
            c => {
                current.push(c);
                i += 1;
            },
        }
    }
    assert!(current.trim().is_empty(), "text after the last statement: {current:?}");
    statements
}

#[test]
fn spot_accounting_dos_have_at_most_four_ddl() {
    // Main's statement bound is 1 s for the whole DO and the measured
    // production p90 is 152 ms per DDL command, so each S8-4 DO stays at
    // four DDL commands at most, constraints inline (PLAN-r4 §4.2).
    for (version, file, delimiter, _, inventory) in SPOT_ACCOUNTING {
        let statements = do_body_statements(file, delimiter);
        assert!(statements.len() <= 4, "{version} has {} DDL commands", statements.len());
        let ddl: Vec<String> = statements
            .iter()
            .map(|statement| {
                let words: Vec<&str> = statement.split_whitespace().collect();
                let len = if words.get(1) == Some(&"UNIQUE") {
                    4
                } else {
                    3
                };
                words[..len.min(words.len())].join(" ")
            })
            .collect();
        assert_eq!(ddl, inventory.to_vec(), "{version} is not its PLAN-r4 §4.2 DDL inventory");
        assert!(statements.iter().all(|statement| statement.starts_with("CREATE ")));
    }
    // The reader itself: a nested body's semicolons, quoted ones and
    // comments are not commands.
    let witness = "DO $m$\nBEGIN\n    -- a; b;\n    CREATE TABLE t (c TEXT CHECK (c <> ';'));\n    \
                   CREATE FUNCTION f() RETURNS INT LANGUAGE plpgsql AS $f$ BEGIN RETURN 1; END; $f$;\n    \
                   /* x; */ CREATE INDEX i ON t (c);\nEND;\n$m$;";
    assert_eq!(do_body_statements(witness, "$m$").len(), 3);
}
