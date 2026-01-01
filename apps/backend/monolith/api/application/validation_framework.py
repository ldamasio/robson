"""
Validation Framework wrapper for TradingIntent validation.

This provides a simple API for validating trading intents against
operational and financial constraints.
"""

from decimal import Decimal
from api.application.validation import (
    ValidationReport,
    ValidationStatus,
    ValidationIssue,
)
from api.models import TradingIntent


class ValidationFramework:
    """
    Framework for validating trading intents.

    This is a thin wrapper that provides a simple validate() method
    for TradingIntent objects.
    """

    def __init__(self):
        pass

    def validate(self, intent: TradingIntent) -> ValidationReport:
        """
        Validate a trading intent against operational and financial constraints.

        Args:
            intent: TradingIntent to validate

        Returns:
            ValidationReport with status and issues
        """
        report = ValidationReport(status=ValidationStatus.PASS)

        # Guard 1: Check if capital is positive
        if intent.capital <= 0:
            report.add_issue(ValidationIssue(
                code="INVALID_CAPITAL",
                severity=ValidationStatus.FAIL,
                message=f"Capital must be positive, got {intent.capital}",
                details={"capital": str(intent.capital)}
            ))

        # Guard 2: Check if quantity is positive
        if intent.quantity <= 0:
            report.add_issue(ValidationIssue(
                code="INVALID_QUANTITY",
                severity=ValidationStatus.FAIL,
                message=f"Quantity must be positive, got {intent.quantity}",
                details={"quantity": str(intent.quantity)}
            ))

        # Guard 3: Check if entry price != stop price
        if intent.entry_price == intent.stop_price:
            report.add_issue(ValidationIssue(
                code="INVALID_STOP_DISTANCE",
                severity=ValidationStatus.FAIL,
                message="Entry price must be different from stop price",
                details={
                    "entry_price": str(intent.entry_price),
                    "stop_price": str(intent.stop_price)
                }
            ))

        # Guard 4: Basic balance check (simplified for now)
        # TODO: Integrate with actual balance checking from Binance
        required_balance = intent.entry_price * intent.quantity
        report.add_issue(ValidationIssue(
            code="BALANCE_CHECK",
            severity=ValidationStatus.WARNING,
            message=f"Ensure sufficient balance: {required_balance} {intent.symbol.quote_asset}",
            details={"required_balance": str(required_balance)}
        ))

        return report
