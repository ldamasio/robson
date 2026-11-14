//! Local capability refusal; neither venue I/O nor historical no-send
//! inference.

#[cfg(test)]
pub(crate) mod test_exchange;
#[cfg(test)]
pub(crate) mod worker_venue;

use robson_domain::Side;
use robson_exec::venue::{AccountKind, VenueCapabilities};

use crate::error::{DaemonError, DaemonResult};

pub(crate) fn require_side(capabilities: &VenueCapabilities, side: Side) -> DaemonResult<()> {
    if side == Side::Short && !capabilities.supports_short {
        return Err(DaemonError::VenueUnsupported { side });
    }
    Ok(())
}

/// Legacy position-account work cannot observe or mutate balance accounts.
pub(crate) fn require_position_account(
    capabilities: &VenueCapabilities,
    operation: &'static str,
) -> DaemonResult<()> {
    if capabilities.account_kind == AccountKind::BalanceAccount {
        return Err(DaemonError::Config(format!("{operation} requires a position account")));
    }
    Ok(())
}

pub(crate) fn record_refusal(site: &'static str) {
    crate::metrics::VENUE_CAPABILITY_REFUSALS.inc();
    tracing::warn!(site, "Venue capability refused entry work; leg retained without rearm");
}

#[cfg(test)]
pub(crate) async fn assert_account_forwarding(
    actual: &impl robson_exec::venue::AccountPort,
    delegate: &impl robson_exec::venue::AccountPort,
) {
    let symbol = robson_domain::Symbol::from_pair("ETHUSDT").unwrap();
    assert_eq!(actual.capabilities(), delegate.capabilities());
    let cash = actual.cash("USDT").await.unwrap();
    let expected = delegate.cash("USDT").await.unwrap();
    assert!(actual.cash("EUR").await.is_err());
    assert!(delegate.cash("EUR").await.is_err());
    assert_eq!((cash.free, cash.locked), (expected.free, expected.locked));
    assert_eq!(
        actual.valuation(&symbol).await.unwrap().price,
        delegate.valuation(&symbol).await.unwrap().price
    );
    let cost = actual.execution_costs(&symbol).await.unwrap();
    let expected = delegate.execution_costs(&symbol).await.unwrap();
    assert_eq!(cost.maker_bps, expected.maker_bps);
    assert_eq!(cost.taker_bps, expected.taker_bps);
    assert_eq!(cost.gap_bps, expected.gap_bps);
    assert_eq!(cost.fee_asset_policy, expected.fee_asset_policy);
    assert_eq!(cost.funding, expected.funding);
    assert_eq!(cost.source, expected.source);
}

#[cfg(test)]
mod tests {
    use robson_exec::venue::{AccountKind, MarketKind};

    use super::*;

    proptest::proptest! {
        #[test]
        fn legacy_account_work_depends_only_on_account_kind(
            balance in proptest::bool::ANY,
            spot in proptest::bool::ANY,
            supports_short in proptest::bool::ANY,
            has_funding in proptest::bool::ANY,
        ) {
            let mut capabilities = robson_testkit::balance_stub::spot_capabilities();
            capabilities.account_kind = if balance { AccountKind::BalanceAccount } else { AccountKind::PositionAccount };
            capabilities.market_kind = if spot { MarketKind::Spot } else { MarketKind::PerpUsdM };
            capabilities.supports_short = supports_short;
            capabilities.has_funding = has_funding;
            proptest::prop_assert_eq!(require_position_account(&capabilities, "fixture").is_ok(), !balance);
        }
    }

    proptest::proptest! {
        #[test]
        fn direction_depends_only_on_supports_short(
            supports_short in proptest::bool::ANY,
            short in proptest::bool::ANY,
            balance in proptest::bool::ANY,
            spot in proptest::bool::ANY,
        ) {
            let mut capabilities = robson_testkit::balance_stub::spot_capabilities();
            capabilities.supports_short = supports_short;
            capabilities.account_kind = if balance { AccountKind::BalanceAccount } else { AccountKind::PositionAccount };
            capabilities.market_kind = if spot { MarketKind::Spot } else { MarketKind::PerpUsdM };
            let side = if short { Side::Short } else { Side::Long };
            let result = require_side(&capabilities, side);
            proptest::prop_assert_eq!(result.is_ok(), !short || supports_short);
        }
    }
}
