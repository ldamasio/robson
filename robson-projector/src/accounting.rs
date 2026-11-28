//! A balance-account daemon's accounting step in the projector's
//! `monthly_state` writes (ADR-0056 step 8l-4).

use std::{future::Future, pin::Pin};

use sqlx::PgConnection;

/// What a balance-account daemon runs first in every `monthly_state` write's
/// transaction, right after `BEGIN`: its server bounds, one bump of every
/// accounting row of the tenant in (tenant, venue) order and, when one is
/// installed, its order-set generation's exact CAS. The daemon supplies it;
/// an error rolls the whole write back. A position account supplies none, and
/// each of its writes takes only the cutover certificate's shared key in its
/// own transaction (ADR-0056 step 8m-2b): `BEGIN`, the key, the statement,
/// `COMMIT`.
pub trait AccountingStep: Send + Sync {
    fn run<'c>(
        &'c self,
        connection: &'c mut PgConnection,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'c>>;
}
