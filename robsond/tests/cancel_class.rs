//! ADR-0056 step 7, PR C: a cancel's definite classes are typed, so no message
//! text, however it reads, can produce one.

use proptest::prelude::*;
use robson_exec::{cancel::classify_cancel, ExecError};

/// Any text, weighted toward the vocabulary a text match would key on.
fn message() -> impl Strategy<Value = String> {
    prop_oneof![
        ".*",
        "(?i)(unknown|already|gone|order|does not exist|cancell?ed|filled|success|-201[13]| ){0,8}",
    ]
}

proptest! {
    #[test]
    fn no_message_makes_a_cancel_definite(text in message()) {
        for error in [
            ExecError::Exchange(text.clone()),
            ExecError::OrderRejected(text.clone()),
            ExecError::RulesRejected(text.clone()),
            ExecError::ReduceOnlyRejected(text.clone()),
            ExecError::SendRefused(text.clone()),
            ExecError::IntentJournal(text.clone()),
            ExecError::InvalidState(text.clone()),
            ExecError::Config(text.clone()),
            ExecError::Timeout(text.clone()),
        ] {
            let class = classify_cancel(Err(error));
            prop_assert!(!class.is_definite());
            prop_assert_eq!(class.label(), "error");
        }
    }
}
