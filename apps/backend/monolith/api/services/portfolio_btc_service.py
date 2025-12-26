"""
Portfolio BTC Service - Calculate portfolio value in BTC.

This service implements the core business logic for tracking portfolio
performance in BTC terms (crypto investor's preferred metric).

User's Profit Formula:
    Profit (BTC) = Current Balance (BTC) + Withdrawals (BTC) - Deposits (BTC)

This formula shows how much BTC value the Robson bot has generated
for the user since inception, considering both current holdings and
past withdrawals as "realized profits".
"""

from decimal import Decimal
from typing import Dict, Optional
from datetime import datetime
from django.utils import timezone
import logging

from api.models.audit import AuditTransaction, TransactionType, BalanceSnapshot
from clients.models import Client
from .btc_conversion_service import BTCConversionService
from api.application.adapters import BinanceExecution

logger = logging.getLogger(__name__)


class PortfolioBTCService:
    """
    Calculate and track portfolio value in BTC.

    This service provides:
    1. Total portfolio value denominated in BTC
    2. Profit calculation using user's formula
    3. Historical BTC value tracking
    4. Balance snapshot functionality with BTC valuation
    """

    def __init__(self, client: Client, execution: BinanceExecution = None):
        """
        Initialize portfolio BTC service.

        Args:
            client: Client to calculate portfolio for
            execution: Binance execution adapter. If None, creates default instance.
        """
        self.client = client
        self.execution = execution or BinanceExecution()
        self.converter = BTCConversionService()

    def calculate_total_portfolio_btc(self) -> Dict:
        """
        Calculate total portfolio value in BTC.

        Includes:
        - Spot balances (all assets converted to BTC)
        - Isolated margin positions (net of debt, converted to BTC)
        - Subtracts margin debts (borrowed amounts)

        Returns:
            Dict with breakdown:
            {
                "total_btc": Decimal,
                "spot_btc": Decimal,
                "margin_btc": Decimal,
                "margin_debt_btc": Decimal,
                "breakdown": {
                    "BTC": Decimal,
                    "ETH": Decimal,
                    "USDC": Decimal,
                    ...
                }
            }

        Example:
            >>> service.calculate_total_portfolio_btc()
            {
                "total_btc": Decimal("0.5234"),
                "spot_btc": Decimal("0.5000"),
                "margin_btc": Decimal("0.0234"),
                "margin_debt_btc": Decimal("0.0000"),
                "breakdown": {
                    "BTC": Decimal("0.50000000"),
                    "ETH": Decimal("0.02000000"),
                    "USDC": Decimal("0.00340000"),
                }
            }
        """
        # Get spot balances
        try:
            spot_balances = self.execution.get_account_balance()
            spot_btc_value, spot_breakdown = self._calculate_spot_btc(spot_balances)
        except Exception as e:
            logger.error(f"Failed to fetch spot balances: {e}")
            spot_btc_value = Decimal("0")
            spot_breakdown = {}

        # Get margin balances
        try:
            margin_btc_value, margin_debt_btc = self._calculate_margin_btc()
        except Exception as e:
            logger.error(f"Failed to calculate margin BTC: {e}")
            margin_btc_value = Decimal("0")
            margin_debt_btc = Decimal("0")

        # Total portfolio in BTC
        total_btc = spot_btc_value + margin_btc_value

        return {
            "total_btc": total_btc,
            "spot_btc": spot_btc_value,
            "margin_btc": margin_btc_value,
            "margin_debt_btc": margin_debt_btc,
            "breakdown": {**spot_breakdown},
        }

    def _calculate_spot_btc(self, balances: Dict) -> tuple[Decimal, Dict[str, Decimal]]:
        """
        Calculate spot portfolio value in BTC.

        Args:
            balances: Binance account balance response

        Returns:
            (total_btc, breakdown_dict)
        """
        if "balances" not in balances:
            return Decimal("0"), {}

        breakdown = self.converter.convert_balances_to_btc(balances["balances"])
        total_btc = sum(breakdown.values(), Decimal("0"))

        return total_btc, breakdown

    def _calculate_margin_btc(self) -> tuple[Decimal, Decimal]:
        """
        Calculate margin positions value in BTC.

        Returns:
            (net_btc_value, total_debt_btc)

        Note:
            - Net value = (assets - liabilities) converted to BTC
            - Debt is subtracted to show true portfolio value
        """
        try:
            # Get isolated margin account info
            # Note: This is a simplified implementation
            # In production, you'd iterate through all isolated margin symbols
            margin_info = self.execution.client.get_isolated_margin_account(
                symbols='BTCUSDC'
            )
            assets = margin_info.get('assets', [])

            if not assets:
                return Decimal("0"), Decimal("0")

            base_asset = assets[0].get('baseAsset', {})
            quote_asset = assets[0].get('quoteAsset', {})

            # Calculate BTC value of base asset (e.g., BTC in BTCUSDC)
            base_asset_name = base_asset.get('asset', 'BTC')
            base_free = Decimal(str(base_asset.get('free', '0')))
            base_borrowed = Decimal(str(base_asset.get('borrowed', '0')))
            base_net = base_free - base_borrowed

            base_btc_value = self.converter.convert_to_btc(base_asset_name, base_net)

            # Calculate BTC value of quote asset (e.g., USDC in BTCUSDC)
            quote_asset_name = quote_asset.get('asset', 'USDC')
            quote_free = Decimal(str(quote_asset.get('free', '0')))
            quote_borrowed = Decimal(str(quote_asset.get('borrowed', '0')))
            quote_net = quote_free - quote_borrowed

            quote_btc_value = self.converter.convert_to_btc(quote_asset_name, quote_net)

            # Total margin value (net of debt)
            total_btc_value = base_btc_value + quote_btc_value

            # Track total debt in BTC
            debt_btc_value = (
                self.converter.convert_to_btc(base_asset_name, base_borrowed) +
                self.converter.convert_to_btc(quote_asset_name, quote_borrowed)
            )

            return total_btc_value, debt_btc_value

        except Exception as e:
            logger.error(f"Failed to calculate margin BTC: {e}")
            return Decimal("0"), Decimal("0")

    def calculate_profit_btc(self, since: Optional[datetime] = None) -> Dict:
        """
        Calculate profit in BTC using user's formula.

        Formula:
        Profit (BTC) = Current Balance (BTC) + Withdrawals (BTC) - Deposits (BTC)

        This formula considers:
        - Current portfolio value in BTC
        - All withdrawals as "realized profits" (they increased user's BTC wealth)
        - All deposits as "investments" (they represent user's capital input)

        Args:
            since: Optional start date for calculation. If None, calculate from inception.

        Returns:
            {
                "profit_btc": Decimal,
                "profit_percent": Decimal,
                "current_balance_btc": Decimal,
                "total_deposits_btc": Decimal,
                "total_withdrawals_btc": Decimal,
                "net_inflows_btc": Decimal,
                "start_date": datetime,
                "calculated_at": datetime,
            }

        Example:
            >>> service.calculate_profit_btc()
            {
                "profit_btc": Decimal("0.0234"),
                "profit_percent": Decimal("4.67"),
                "current_balance_btc": Decimal("0.5234"),
                "total_deposits_btc": Decimal("0.5000"),
                "total_withdrawals_btc": Decimal("0.0000"),
                "net_inflows_btc": Decimal("0.5000"),
                "start_date": datetime(2024, 1, 1),
                "calculated_at": datetime(2025, 12, 26),
            }
        """
        # Current portfolio value
        portfolio = self.calculate_total_portfolio_btc()
        current_balance_btc = portfolio["total_btc"]

        # Query deposits and withdrawals
        queryset = AuditTransaction.objects.filter(
            client=self.client,
            transaction_type__in=[TransactionType.DEPOSIT, TransactionType.WITHDRAWAL],
        )

        if since:
            queryset = queryset.filter(executed_at__gte=since)

        # Calculate totals
        total_deposits_btc = Decimal("0")
        total_withdrawals_btc = Decimal("0")

        for tx in queryset:
            # Convert amount to BTC
            amount_btc = self.converter.convert_to_btc(tx.asset, tx.quantity)

            if tx.transaction_type == TransactionType.DEPOSIT:
                total_deposits_btc += amount_btc
            else:  # WITHDRAWAL
                total_withdrawals_btc += amount_btc

        # Apply user's formula: Profit = Current + Withdrawals - Deposits
        profit_btc = current_balance_btc + total_withdrawals_btc - total_deposits_btc

        # Calculate percentage (profit / initial investment)
        # Initial investment = deposits - withdrawals (net money added)
        net_inflows_btc = total_deposits_btc - total_withdrawals_btc

        if net_inflows_btc > 0:
            profit_percent = (profit_btc / net_inflows_btc) * Decimal("100")
        else:
            # If no deposits or more withdrawals than deposits, use 0 as baseline
            profit_percent = Decimal("0")

        # Get start date (first deposit or account creation)
        first_deposit = queryset.filter(
            transaction_type=TransactionType.DEPOSIT
        ).order_by('executed_at').first()

        start_date = first_deposit.executed_at if first_deposit else since

        return {
            "profit_btc": profit_btc,
            "profit_percent": profit_percent,
            "current_balance_btc": current_balance_btc,
            "total_deposits_btc": total_deposits_btc,
            "total_withdrawals_btc": total_withdrawals_btc,
            "net_inflows_btc": net_inflows_btc,
            "start_date": start_date,
            "calculated_at": timezone.now(),
        }

    def take_balance_snapshot_btc(self) -> BalanceSnapshot:
        """
        Take a balance snapshot with BTC valuation.

        Extends the existing AuditService.take_balance_snapshot() to include
        BTC valuation for historical tracking.

        Returns:
            BalanceSnapshot with BTC fields populated

        Note:
            This method relies on AuditService for the base snapshot logic.
            In production, integrate with existing snapshot infrastructure.
        """
        from .audit_service import AuditService

        # Get existing snapshot (which calculates USD value)
        audit_service = AuditService(self.client, self.execution)
        snapshot = audit_service.take_balance_snapshot()

        # Calculate BTC value
        portfolio = self.calculate_total_portfolio_btc()

        # Update snapshot with BTC fields
        snapshot.total_equity_btc = portfolio["total_btc"]
        snapshot.spot_btc_value = portfolio["spot_btc"]
        snapshot.margin_btc_value = portfolio["margin_btc"]

        snapshot.save()

        logger.info(
            f"Balance snapshot taken for client {self.client.id}: "
            f"{portfolio['total_btc']} BTC"
        )
        return snapshot

    def get_btc_history(
        self,
        start_date: Optional[datetime] = None,
        end_date: Optional[datetime] = None
    ) -> list:
        """
        Get historical portfolio value in BTC.

        Args:
            start_date: Start of period
            end_date: End of period

        Returns:
            List of BalanceSnapshot objects with BTC values

        Example:
            >>> from datetime import datetime, timedelta
            >>> start = datetime.now() - timedelta(days=30)
            >>> snapshots = service.get_btc_history(start_date=start)
            >>> len(snapshots)
            30
        """
        queryset = BalanceSnapshot.objects.filter(
            client=self.client,
            total_equity_btc__isnull=False,
        )

        if start_date:
            queryset = queryset.filter(snapshot_time__gte=start_date)
        if end_date:
            queryset = queryset.filter(snapshot_time__lte=end_date)

        return list(queryset.order_by('snapshot_time'))
