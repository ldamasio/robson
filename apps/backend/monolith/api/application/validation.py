"""
Validation framework for operational and financial validation.

This is the "paper trading" stage of the agentic workflow.
It validates correctness and risk WITHOUT production impact.

Principles:
- Reduce uncertainty
- Expose hidden risks
- Block unsafe execution
- Increase confidence

This is NOT developer CI. This is operational and financial validation.
"""

from __future__ import annotations
from dataclasses import dataclass, field
from typing import Protocol
from decimal import Decimal
from enum import Enum


class ValidationStatus(Enum):
    """Validation result status."""

    PASS = "PASS"
    FAIL = "FAIL"
    WARNING = "WARNING"


@dataclass
class ValidationIssue:
    """A single validation issue (error or warning)."""

    code: str
    severity: ValidationStatus
    message: str
    details: dict = field(default_factory=dict)

    def __str__(self) -> str:
        return f"[{self.severity.value}] {self.code}: {self.message}"


@dataclass
class ValidationReport:
    """
    Validation report containing all issues and final status.

    The report is the primary output of validation.
    It provides clear, actionable feedback to the user.
    """

    status: ValidationStatus
    issues: list[ValidationIssue] = field(default_factory=list)
    metadata: dict = field(default_factory=dict)

    def add_issue(self, issue: ValidationIssue) -> None:
        """Add an issue to the report."""
        self.issues.append(issue)

        # Update overall status based on severity
        if issue.severity == ValidationStatus.FAIL:
            self.status = ValidationStatus.FAIL
        elif issue.severity == ValidationStatus.WARNING and self.status == ValidationStatus.PASS:
            self.status = ValidationStatus.WARNING

    def has_failures(self) -> bool:
        """Check if report has any failures."""
        return self.status == ValidationStatus.FAIL

    def has_warnings(self) -> bool:
        """Check if report has any warnings."""
        return any(i.severity == ValidationStatus.WARNING for i in self.issues)

    def to_dict(self) -> dict:
        """Convert report to dictionary for JSON serialization."""
        return {
            "status": self.status.value,
            "issues": [
                {
                    "code": i.code,
                    "severity": i.severity.value,
                    "message": i.message,
                    "details": i.details,
                }
                for i in self.issues
            ],
            "metadata": self.metadata,
            "summary": {
                "total_issues": len(self.issues),
                "failures": sum(1 for i in self.issues if i.severity == ValidationStatus.FAIL),
                "warnings": sum(1 for i in self.issues if i.severity == ValidationStatus.WARNING),
            },
        }

    def to_human_readable(self) -> str:
        """Convert report to human-readable text."""
        lines = []
        lines.append("=" * 60)
        lines.append("VALIDATION REPORT")
        lines.append("=" * 60)
        lines.append("")

        # Status
        if self.status == ValidationStatus.PASS:
            lines.append("✓ STATUS: PASS")
        elif self.status == ValidationStatus.WARNING:
            lines.append("⚠ STATUS: WARNING (validation passed with concerns)")
        else:
            lines.append("✗ STATUS: FAIL")

        lines.append("")

        # Metadata
        if self.metadata:
            lines.append("Context:")
            for key, value in self.metadata.items():
                lines.append(f"  {key}: {value}")
            lines.append("")

        # Issues
        if self.issues:
            failures = [i for i in self.issues if i.severity == ValidationStatus.FAIL]
            warnings = [i for i in self.issues if i.severity == ValidationStatus.WARNING]

            if failures:
                lines.append("FAILURES:")
                for idx, issue in enumerate(failures, 1):
                    lines.append(f"  {idx}. [{issue.code}] {issue.message}")
                    if issue.details:
                        for k, v in issue.details.items():
                            lines.append(f"     {k}: {v}")
                lines.append("")

            if warnings:
                lines.append("WARNINGS:")
                for idx, issue in enumerate(warnings, 1):
                    lines.append(f"  {idx}. [{issue.code}] {issue.message}")
                    if issue.details:
                        for k, v in issue.details.items():
                            lines.append(f"     {k}: {v}")
                lines.append("")

        # Summary
        lines.append("Summary:")
        lines.append(f"  Total issues: {len(self.issues)}")
        lines.append(f"  Failures: {sum(1 for i in self.issues if i.severity == ValidationStatus.FAIL)}")
        lines.append(f"  Warnings: {sum(1 for i in self.issues if i.severity == ValidationStatus.WARNING)}")
        lines.append("")

        # Action
        if self.has_failures():
            lines.append("⚠️  EXECUTION BLOCKED")
            lines.append("   Fix the failures above before executing.")
        elif self.has_warnings():
            lines.append("⚠️  PROCEED WITH CAUTION")
            lines.append("   Review warnings before executing.")
        else:
            lines.append("✓ SAFE TO EXECUTE")

        lines.append("")
        lines.append("=" * 60)

        return "\n".join(lines)


# ==========================================
# VALIDATOR PROTOCOLS
# ==========================================


class Validator(Protocol):
    """Protocol for validation components."""

    def validate(self, context: dict) -> ValidationReport:
        """
        Perform validation and return a report.

        Args:
            context: Validation context (plan data, configuration, etc.)

        Returns:
            ValidationReport with issues and status
        """
        ...


# ==========================================
# CONCRETE VALIDATORS
# ==========================================


class TenantIsolationValidator:
    """
    Validates that tenant context is present and all operations are scoped.

    This is CRITICAL for multi-tenant security.
    Without explicit tenant scoping, operations could affect wrong accounts.
    """

    def validate(self, context: dict) -> ValidationReport:
        """Validate tenant isolation."""
        report = ValidationReport(status=ValidationStatus.PASS)
        report.metadata["validator"] = "TenantIsolationValidator"

        client_id = context.get("client_id")

        # Rule 1: client_id must be present
        if client_id is None:
            report.add_issue(
                ValidationIssue(
                    code="TENANT_MISSING",
                    severity=ValidationStatus.FAIL,
                    message="client_id is required but not provided",
                    details={
                        "requirement": "All operations must be explicitly scoped to a tenant",
                        "impact": "Without tenant context, operation could affect wrong account",
                    },
                )
            )
            return report

        # Rule 2: client_id must be valid (non-zero, positive)
        try:
            client_id_int = int(client_id)
            if client_id_int <= 0:
                report.add_issue(
                    ValidationIssue(
                        code="TENANT_INVALID",
                        severity=ValidationStatus.FAIL,
                        message=f"client_id must be positive, got: {client_id}",
                        details={"provided": client_id},
                    )
                )
        except (ValueError, TypeError):
            report.add_issue(
                ValidationIssue(
                    code="TENANT_INVALID_FORMAT",
                    severity=ValidationStatus.FAIL,
                    message=f"client_id must be an integer, got: {type(client_id).__name__}",
                    details={"provided": str(client_id)},
                )
            )

        return report


class RiskConfigurationValidator:
    """
    Validates risk configuration and enforcement.

    Ensures that:
    - Risk limits are defined
    - Stop-loss and drawdown rules are sane
    - Position sizing constraints are reasonable
    """

    def validate(self, context: dict) -> ValidationReport:
        """Validate risk configuration."""
        report = ValidationReport(status=ValidationStatus.PASS)
        report.metadata["validator"] = "RiskConfigurationValidator"

        risk_config = context.get("risk_config", {})

        # Rule 1: risk_config must exist
        if not risk_config:
            report.add_issue(
                ValidationIssue(
                    code="RISK_CONFIG_MISSING",
                    severity=ValidationStatus.FAIL,
                    message="risk_config is required for live execution",
                    details={
                        "requirement": "Define max_drawdown, stop_loss_percent, max_position_size",
                    },
                )
            )
            return report

        # Rule 2: max_drawdown must be defined and reasonable
        max_drawdown = risk_config.get("max_drawdown_percent")
        if max_drawdown is None:
            report.add_issue(
                ValidationIssue(
                    code="MAX_DRAWDOWN_MISSING",
                    severity=ValidationStatus.FAIL,
                    message="max_drawdown_percent is required",
                    details={
                        "requirement": "Define maximum allowed drawdown percentage",
                        "example": "max_drawdown_percent: 10 (for 10%)",
                    },
                )
            )
        else:
            try:
                dd = Decimal(str(max_drawdown))
                if dd <= 0 or dd > 100:
                    report.add_issue(
                        ValidationIssue(
                            code="MAX_DRAWDOWN_INVALID",
                            severity=ValidationStatus.FAIL,
                            message=f"max_drawdown_percent must be between 0 and 100, got: {dd}",
                            details={"provided": str(dd)},
                        )
                    )
                elif dd > 50:
                    report.add_issue(
                        ValidationIssue(
                            code="MAX_DRAWDOWN_HIGH",
                            severity=ValidationStatus.WARNING,
                            message=f"max_drawdown_percent is very high: {dd}%",
                            details={
                                "provided": str(dd),
                                "recommendation": "Consider limiting drawdown to 20-30%",
                            },
                        )
                    )
            except (ValueError, TypeError):
                report.add_issue(
                    ValidationIssue(
                        code="MAX_DRAWDOWN_INVALID_FORMAT",
                        severity=ValidationStatus.FAIL,
                        message=f"max_drawdown_percent must be numeric",
                        details={"provided": str(max_drawdown)},
                    )
                )

        # Rule 3: stop_loss_percent must be defined and reasonable
        stop_loss = risk_config.get("stop_loss_percent")
        if stop_loss is None:
            report.add_issue(
                ValidationIssue(
                    code="STOP_LOSS_MISSING",
                    severity=ValidationStatus.FAIL,
                    message="stop_loss_percent is required",
                    details={
                        "requirement": "Define stop-loss percentage for each position",
                        "example": "stop_loss_percent: 2 (for 2% per trade)",
                    },
                )
            )
        else:
            try:
                sl = Decimal(str(stop_loss))
                if sl <= 0 or sl > 100:
                    report.add_issue(
                        ValidationIssue(
                            code="STOP_LOSS_INVALID",
                            severity=ValidationStatus.FAIL,
                            message=f"stop_loss_percent must be between 0 and 100, got: {sl}",
                            details={"provided": str(sl)},
                        )
                    )
                elif sl > 10:
                    report.add_issue(
                        ValidationIssue(
                            code="STOP_LOSS_HIGH",
                            severity=ValidationStatus.WARNING,
                            message=f"stop_loss_percent is very high: {sl}%",
                            details={
                                "provided": str(sl),
                                "recommendation": "Consider limiting stop-loss to 2-5% per trade",
                            },
                        )
                    )
            except (ValueError, TypeError):
                report.add_issue(
                    ValidationIssue(
                        code="STOP_LOSS_INVALID_FORMAT",
                        severity=ValidationStatus.FAIL,
                        message=f"stop_loss_percent must be numeric",
                        details={"provided": str(stop_loss)},
                    )
                )

        # Rule 4: max_position_size must be defined and reasonable
        max_position = risk_config.get("max_position_size_percent")
        if max_position is None:
            report.add_issue(
                ValidationIssue(
                    code="MAX_POSITION_MISSING",
                    severity=ValidationStatus.WARNING,
                    message="max_position_size_percent is recommended",
                    details={
                        "recommendation": "Define maximum position size as % of capital",
                        "example": "max_position_size_percent: 10 (for 10% max per position)",
                    },
                )
            )
        else:
            try:
                mp = Decimal(str(max_position))
                if mp <= 0 or mp > 100:
                    report.add_issue(
                        ValidationIssue(
                            code="MAX_POSITION_INVALID",
                            severity=ValidationStatus.FAIL,
                            message=f"max_position_size_percent must be between 0 and 100, got: {mp}",
                            details={"provided": str(mp)},
                        )
                    )
                elif mp > 50:
                    report.add_issue(
                        ValidationIssue(
                            code="MAX_POSITION_HIGH",
                            severity=ValidationStatus.WARNING,
                            message=f"max_position_size_percent is very high: {mp}%",
                            details={
                                "provided": str(mp),
                                "recommendation": "Consider limiting position size to 10-20%",
                            },
                        )
                    )
            except (ValueError, TypeError):
                report.add_issue(
                    ValidationIssue(
                        code="MAX_POSITION_INVALID_FORMAT",
                        severity=ValidationStatus.FAIL,
                        message=f"max_position_size_percent must be numeric",
                        details={"provided": str(max_position)},
                    )
                )

        return report


class OperationValidator:
    """
    Validates specific operation parameters.

    Checks that the operation being validated has sane parameters:
    - Symbol is valid
    - Quantity is positive and reasonable
    - Price is positive (if limit order)
    - Side is valid
    """

    def validate(self, context: dict) -> ValidationReport:
        """Validate operation parameters."""
        report = ValidationReport(status=ValidationStatus.PASS)
        report.metadata["validator"] = "OperationValidator"

        operation = context.get("operation", {})

        # Rule 1: operation type must be specified
        op_type = operation.get("type")
        if not op_type:
            report.add_issue(
                ValidationIssue(
                    code="OPERATION_TYPE_MISSING",
                    severity=ValidationStatus.FAIL,
                    message="operation type is required",
                    details={"valid_types": ["buy", "sell", "cancel"]},
                )
            )
            return report

        # Rule 2: For buy/sell, validate parameters
        if op_type in ["buy", "sell"]:
            # Symbol
            symbol = operation.get("symbol")
            if not symbol:
                report.add_issue(
                    ValidationIssue(
                        code="SYMBOL_MISSING",
                        severity=ValidationStatus.FAIL,
                        message="symbol is required for buy/sell operations",
                    )
                )

            # Quantity
            qty = operation.get("quantity")
            if qty is None:
                report.add_issue(
                    ValidationIssue(
                        code="QUANTITY_MISSING",
                        severity=ValidationStatus.FAIL,
                        message="quantity is required for buy/sell operations",
                    )
                )
            else:
                try:
                    qty_decimal = Decimal(str(qty))
                    if qty_decimal <= 0:
                        report.add_issue(
                            ValidationIssue(
                                code="QUANTITY_INVALID",
                                severity=ValidationStatus.FAIL,
                                message=f"quantity must be positive, got: {qty_decimal}",
                                details={"provided": str(qty)},
                            )
                        )
                except (ValueError, TypeError):
                    report.add_issue(
                        ValidationIssue(
                            code="QUANTITY_INVALID_FORMAT",
                            severity=ValidationStatus.FAIL,
                            message=f"quantity must be numeric",
                            details={"provided": str(qty)},
                        )
                    )

            # Price (if limit order)
            price = operation.get("price")
            if price is not None:
                try:
                    price_decimal = Decimal(str(price))
                    if price_decimal <= 0:
                        report.add_issue(
                            ValidationIssue(
                                code="PRICE_INVALID",
                                severity=ValidationStatus.FAIL,
                                message=f"price must be positive, got: {price_decimal}",
                                details={"provided": str(price)},
                            )
                        )
                except (ValueError, TypeError):
                    report.add_issue(
                        ValidationIssue(
                            code="PRICE_INVALID_FORMAT",
                            severity=ValidationStatus.FAIL,
                            message=f"price must be numeric",
                            details={"provided": str(price)},
                        )
                    )

        return report


# ==========================================
# VALIDATION USE CASE
# ==========================================


class ValidatePlanUseCase:
    """
    Use case for validating an execution plan.

    This orchestrates all validators and produces a comprehensive report.
    """

    def __init__(self, validators: list[Validator] | None = None):
        self.validators = validators or [
            TenantIsolationValidator(),
            RiskConfigurationValidator(),
            OperationValidator(),
        ]

    def execute(self, plan_context: dict) -> ValidationReport:
        """
        Execute validation on a plan.

        Args:
            plan_context: Dictionary containing:
                - client_id: Tenant ID
                - risk_config: Risk configuration dict
                - operation: Operation parameters

        Returns:
            Consolidated ValidationReport
        """
        # Create master report
        master_report = ValidationReport(status=ValidationStatus.PASS)
        master_report.metadata["plan_id"] = plan_context.get("plan_id", "unknown")
        master_report.metadata["client_id"] = plan_context.get("client_id", "NOT_SET")

        # Run all validators
        for validator in self.validators:
            report = validator.validate(plan_context)

            # Merge issues into master report
            for issue in report.issues:
                master_report.add_issue(issue)

        return master_report
