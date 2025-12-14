"""
Tests for validation framework.

Tests the operational and financial validation logic that acts as
the "paper trading" stage of the agentic workflow.
"""

from __future__ import annotations
import unittest
from decimal import Decimal

from api.application import (
    ValidationStatus,
    ValidationIssue,
    ValidationReport,
    TenantIsolationValidator,
    RiskConfigurationValidator,
    OperationValidator,
    ValidatePlanUseCase,
)


class ValidationReportTests(unittest.TestCase):
    """Tests for ValidationReport."""

    def test_empty_report_passes(self):
        """Empty report should have PASS status."""
        report = ValidationReport(status=ValidationStatus.PASS)
        self.assertEqual(report.status, ValidationStatus.PASS)
        self.assertFalse(report.has_failures())
        self.assertFalse(report.has_warnings())

    def test_adding_failure_changes_status(self):
        """Adding a failure should change report status to FAIL."""
        report = ValidationReport(status=ValidationStatus.PASS)

        issue = ValidationIssue(
            code="TEST_FAIL",
            severity=ValidationStatus.FAIL,
            message="Test failure",
        )
        report.add_issue(issue)

        self.assertTrue(report.has_failures())
        self.assertEqual(report.status, ValidationStatus.FAIL)

    def test_adding_warning_changes_status(self):
        """Adding a warning should change report status to WARNING."""
        report = ValidationReport(status=ValidationStatus.PASS)

        issue = ValidationIssue(
            code="TEST_WARN",
            severity=ValidationStatus.WARNING,
            message="Test warning",
        )
        report.add_issue(issue)

        self.assertTrue(report.has_warnings())
        self.assertEqual(report.status, ValidationStatus.WARNING)

    def test_failure_overrides_warning(self):
        """Failure status should override warning status."""
        report = ValidationReport(status=ValidationStatus.PASS)

        # Add warning first
        report.add_issue(
            ValidationIssue(
                code="TEST_WARN",
                severity=ValidationStatus.WARNING,
                message="Warning",
            )
        )
        self.assertEqual(report.status, ValidationStatus.WARNING)

        # Add failure - should override
        report.add_issue(
            ValidationIssue(
                code="TEST_FAIL",
                severity=ValidationStatus.FAIL,
                message="Failure",
            )
        )
        self.assertEqual(report.status, ValidationStatus.FAIL)

    def test_to_dict(self):
        """Report should serialize to dict."""
        report = ValidationReport(status=ValidationStatus.PASS)
        report.metadata["test"] = "value"

        report.add_issue(
            ValidationIssue(
                code="TEST",
                severity=ValidationStatus.WARNING,
                message="Test issue",
                details={"key": "value"},
            )
        )

        d = report.to_dict()

        self.assertEqual(d["status"], "WARNING")
        self.assertEqual(len(d["issues"]), 1)
        self.assertEqual(d["issues"][0]["code"], "TEST")
        self.assertEqual(d["metadata"]["test"], "value")
        self.assertEqual(d["summary"]["warnings"], 1)
        self.assertEqual(d["summary"]["failures"], 0)

    def test_to_human_readable(self):
        """Report should render as human-readable text."""
        report = ValidationReport(status=ValidationStatus.FAIL)
        report.metadata["plan_id"] = "test123"

        report.add_issue(
            ValidationIssue(
                code="TEST_FAIL",
                severity=ValidationStatus.FAIL,
                message="Something went wrong",
            )
        )

        text = report.to_human_readable()

        self.assertIn("VALIDATION REPORT", text)
        self.assertIn("STATUS: FAIL", text)
        self.assertIn("plan_id: test123", text)
        self.assertIn("TEST_FAIL", text)
        self.assertIn("Something went wrong", text)
        self.assertIn("EXECUTION BLOCKED", text)


class TenantIsolationValidatorTests(unittest.TestCase):
    """Tests for TenantIsolationValidator."""

    def test_missing_client_id_fails(self):
        """Missing client_id should fail validation."""
        validator = TenantIsolationValidator()
        report = validator.validate({})

        self.assertTrue(report.has_failures())
        self.assertEqual(report.issues[0].code, "TENANT_MISSING")

    def test_none_client_id_fails(self):
        """None client_id should fail validation."""
        validator = TenantIsolationValidator()
        report = validator.validate({"client_id": None})

        self.assertTrue(report.has_failures())
        self.assertEqual(report.issues[0].code, "TENANT_MISSING")

    def test_zero_client_id_fails(self):
        """Zero client_id should fail validation."""
        validator = TenantIsolationValidator()
        report = validator.validate({"client_id": 0})

        self.assertTrue(report.has_failures())
        self.assertEqual(report.issues[0].code, "TENANT_INVALID")

    def test_negative_client_id_fails(self):
        """Negative client_id should fail validation."""
        validator = TenantIsolationValidator()
        report = validator.validate({"client_id": -1})

        self.assertTrue(report.has_failures())
        self.assertEqual(report.issues[0].code, "TENANT_INVALID")

    def test_invalid_client_id_format_fails(self):
        """Non-integer client_id should fail validation."""
        validator = TenantIsolationValidator()
        report = validator.validate({"client_id": "abc"})

        self.assertTrue(report.has_failures())
        self.assertEqual(report.issues[0].code, "TENANT_INVALID_FORMAT")

    def test_valid_client_id_passes(self):
        """Valid client_id should pass validation."""
        validator = TenantIsolationValidator()
        report = validator.validate({"client_id": 1})

        self.assertFalse(report.has_failures())
        self.assertEqual(report.status, ValidationStatus.PASS)


class RiskConfigurationValidatorTests(unittest.TestCase):
    """Tests for RiskConfigurationValidator."""

    def test_missing_risk_config_fails(self):
        """Missing risk_config should fail validation."""
        validator = RiskConfigurationValidator()
        report = validator.validate({})

        self.assertTrue(report.has_failures())
        self.assertEqual(report.issues[0].code, "RISK_CONFIG_MISSING")

    def test_empty_risk_config_fails(self):
        """Empty risk_config should fail validation."""
        validator = RiskConfigurationValidator()
        report = validator.validate({"risk_config": {}})

        self.assertTrue(report.has_failures())
        # Should have multiple failures (max_drawdown, stop_loss missing)
        codes = [i.code for i in report.issues]
        self.assertIn("MAX_DRAWDOWN_MISSING", codes)
        self.assertIn("STOP_LOSS_MISSING", codes)

    def test_complete_risk_config_passes(self):
        """Complete risk_config should pass validation."""
        validator = RiskConfigurationValidator()
        report = validator.validate({
            "risk_config": {
                "max_drawdown_percent": 20,
                "stop_loss_percent": 2,
                "max_position_size_percent": 10,
            }
        })

        self.assertFalse(report.has_failures())
        self.assertEqual(report.status, ValidationStatus.PASS)

    def test_high_drawdown_warns(self):
        """High max_drawdown should generate warning."""
        validator = RiskConfigurationValidator()
        report = validator.validate({
            "risk_config": {
                "max_drawdown_percent": 60,
                "stop_loss_percent": 2,
            }
        })

        self.assertFalse(report.has_failures())
        self.assertTrue(report.has_warnings())
        codes = [i.code for i in report.issues]
        self.assertIn("MAX_DRAWDOWN_HIGH", codes)

    def test_high_stop_loss_warns(self):
        """High stop_loss should generate warning."""
        validator = RiskConfigurationValidator()
        report = validator.validate({
            "risk_config": {
                "max_drawdown_percent": 20,
                "stop_loss_percent": 15,
            }
        })

        self.assertFalse(report.has_failures())
        self.assertTrue(report.has_warnings())
        codes = [i.code for i in report.issues]
        self.assertIn("STOP_LOSS_HIGH", codes)

    def test_invalid_drawdown_format_fails(self):
        """Invalid max_drawdown format should fail."""
        validator = RiskConfigurationValidator()
        report = validator.validate({
            "risk_config": {
                "max_drawdown_percent": "abc",
                "stop_loss_percent": 2,
            }
        })

        self.assertTrue(report.has_failures())
        codes = [i.code for i in report.issues]
        self.assertIn("MAX_DRAWDOWN_INVALID_FORMAT", codes)


class OperationValidatorTests(unittest.TestCase):
    """Tests for OperationValidator."""

    def test_missing_operation_type_fails(self):
        """Missing operation type should fail."""
        validator = OperationValidator()
        report = validator.validate({"operation": {}})

        self.assertTrue(report.has_failures())
        self.assertEqual(report.issues[0].code, "OPERATION_TYPE_MISSING")

    def test_buy_without_symbol_fails(self):
        """Buy operation without symbol should fail."""
        validator = OperationValidator()
        report = validator.validate({
            "operation": {
                "type": "buy",
                "quantity": "0.1",
            }
        })

        self.assertTrue(report.has_failures())
        codes = [i.code for i in report.issues]
        self.assertIn("SYMBOL_MISSING", codes)

    def test_buy_without_quantity_fails(self):
        """Buy operation without quantity should fail."""
        validator = OperationValidator()
        report = validator.validate({
            "operation": {
                "type": "buy",
                "symbol": "BTCUSDT",
            }
        })

        self.assertTrue(report.has_failures())
        codes = [i.code for i in report.issues]
        self.assertIn("QUANTITY_MISSING", codes)

    def test_complete_buy_passes(self):
        """Complete buy operation should pass."""
        validator = OperationValidator()
        report = validator.validate({
            "operation": {
                "type": "buy",
                "symbol": "BTCUSDT",
                "quantity": "0.1",
                "price": "50000",
            }
        })

        self.assertFalse(report.has_failures())
        self.assertEqual(report.status, ValidationStatus.PASS)

    def test_negative_quantity_fails(self):
        """Negative quantity should fail."""
        validator = OperationValidator()
        report = validator.validate({
            "operation": {
                "type": "buy",
                "symbol": "BTCUSDT",
                "quantity": "-0.1",
            }
        })

        self.assertTrue(report.has_failures())
        codes = [i.code for i in report.issues]
        self.assertIn("QUANTITY_INVALID", codes)


class ValidatePlanUseCaseTests(unittest.TestCase):
    """Tests for ValidatePlanUseCase."""

    def test_complete_valid_plan_passes(self):
        """Complete valid plan should pass all validators."""
        use_case = ValidatePlanUseCase()

        context = {
            "plan_id": "test123",
            "client_id": 1,
            "risk_config": {
                "max_drawdown_percent": 20,
                "stop_loss_percent": 2,
                "max_position_size_percent": 10,
            },
            "operation": {
                "type": "buy",
                "symbol": "BTCUSDT",
                "quantity": "0.1",
                "price": "50000",
            },
        }

        report = use_case.execute(context)

        self.assertFalse(report.has_failures())
        self.assertEqual(report.status, ValidationStatus.PASS)

    def test_missing_client_id_fails(self):
        """Missing client_id should fail."""
        use_case = ValidatePlanUseCase()

        context = {
            "plan_id": "test123",
            "risk_config": {
                "max_drawdown_percent": 20,
                "stop_loss_percent": 2,
            },
            "operation": {
                "type": "buy",
                "symbol": "BTCUSDT",
                "quantity": "0.1",
            },
        }

        report = use_case.execute(context)

        self.assertTrue(report.has_failures())
        codes = [i.code for i in report.issues]
        self.assertIn("TENANT_MISSING", codes)

    def test_multiple_failures_aggregated(self):
        """Multiple failures should be aggregated."""
        use_case = ValidatePlanUseCase()

        context = {
            "plan_id": "test123",
            # Missing client_id
            # Missing risk_config
            "operation": {
                "type": "buy",
                # Missing symbol and quantity
            },
        }

        report = use_case.execute(context)

        self.assertTrue(report.has_failures())
        # Should have failures from multiple validators
        self.assertGreater(len(report.issues), 3)


if __name__ == "__main__":
    unittest.main()
