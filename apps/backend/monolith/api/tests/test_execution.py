"""
Tests for execution framework.

Tests the SAFE BY DEFAULT execution logic:
- DRY-RUN is default (no real orders)
- LIVE requires explicit acknowledgement
- Guards prevent unsafe execution
- Audit trail always recorded
"""

from __future__ import annotations
import unittest

from api.application import (
    ExecutionMode,
    ExecutionStatus,
    ExecutionGuard,
    ExecutionResult,
    ValidationRequiredGuard,
    TenantContextGuard,
    ExecutionLimitsGuard,
    AcknowledgementGuard,
    ExecutePlanUseCase,
)


class ExecutionResultTests(unittest.TestCase):
    """Tests for ExecutionResult."""

    def test_empty_result_success(self):
        """Empty result should be success."""
        result = ExecutionResult(
            status=ExecutionStatus.SUCCESS,
            mode=ExecutionMode.DRY_RUN,
        )
        self.assertEqual(result.status, ExecutionStatus.SUCCESS)
        self.assertTrue(result.is_success())
        self.assertFalse(result.is_blocked())

    def test_blocked_result(self):
        """Blocked result should not be success."""
        result = ExecutionResult(
            status=ExecutionStatus.BLOCKED,
            mode=ExecutionMode.DRY_RUN,
        )
        self.assertTrue(result.is_blocked())
        self.assertFalse(result.is_success())

    def test_add_guard(self):
        """Can add guards to result."""
        result = ExecutionResult(
            status=ExecutionStatus.SUCCESS,
            mode=ExecutionMode.DRY_RUN,
        )

        guard = ExecutionGuard(
            name="Test",
            passed=True,
            message="Test passed",
        )
        result.add_guard(guard)

        self.assertEqual(len(result.guards), 1)
        self.assertEqual(result.guards[0].name, "Test")

    def test_to_dict(self):
        """Result serializes to dict."""
        result = ExecutionResult(
            status=ExecutionStatus.SUCCESS,
            mode=ExecutionMode.DRY_RUN,
        )
        result.metadata["test"] = "value"
        result.add_action({"type": "TEST", "description": "Test action"})

        d = result.to_dict()

        self.assertEqual(d["status"], "SUCCESS")
        self.assertEqual(d["mode"], "DRY_RUN")
        self.assertEqual(d["metadata"]["test"], "value")
        self.assertEqual(len(d["actions"]), 1)
        self.assertEqual(d["summary"]["total_actions"], 1)


class ValidationRequiredGuardTests(unittest.TestCase):
    """Tests for ValidationRequiredGuard."""

    def test_dry_run_doesnt_require_validation(self):
        """DRY-RUN doesn't require validation."""
        guard = ValidationRequiredGuard()
        result = guard.check({"mode": ExecutionMode.DRY_RUN})

        self.assertTrue(result.passed)

    def test_live_requires_validation(self):
        """LIVE requires validation."""
        guard = ValidationRequiredGuard()
        result = guard.check({
            "mode": ExecutionMode.LIVE,
            "validated": False,
        })

        self.assertFalse(result.passed)
        self.assertEqual(result.name, "ValidationRequired")

    def test_live_with_failed_validation_blocked(self):
        """LIVE with failed validation is blocked."""
        guard = ValidationRequiredGuard()
        result = guard.check({
            "mode": ExecutionMode.LIVE,
            "validated": True,
            "validation_passed": False,
        })

        self.assertFalse(result.passed)

    def test_live_with_passed_validation_allowed(self):
        """LIVE with passed validation is allowed."""
        guard = ValidationRequiredGuard()
        result = guard.check({
            "mode": ExecutionMode.LIVE,
            "validated": True,
            "validation_passed": True,
        })

        self.assertTrue(result.passed)


class TenantContextGuardTests(unittest.TestCase):
    """Tests for TenantContextGuard."""

    def test_missing_client_id_blocked(self):
        """Missing client_id blocks execution."""
        guard = TenantContextGuard()
        result = guard.check({})

        self.assertFalse(result.passed)
        self.assertEqual(result.name, "TenantContext")

    def test_invalid_client_id_blocked(self):
        """Invalid client_id blocks execution."""
        guard = TenantContextGuard()
        result = guard.check({"client_id": 0})

        self.assertFalse(result.passed)

    def test_valid_client_id_allowed(self):
        """Valid client_id is allowed."""
        guard = TenantContextGuard()
        result = guard.check({"client_id": 1})

        self.assertTrue(result.passed)


class ExecutionLimitsGuardTests(unittest.TestCase):
    """Tests for ExecutionLimitsGuard."""

    def test_dry_run_no_limits(self):
        """DRY-RUN doesn't enforce limits."""
        guard = ExecutionLimitsGuard()
        result = guard.check({
            "mode": ExecutionMode.DRY_RUN,
            "limits": {"max_orders_per_day": 1},
            "stats": {"orders_today": 100},  # Way over limit
        })

        self.assertTrue(result.passed)

    def test_live_enforces_max_orders(self):
        """LIVE enforces max orders limit."""
        guard = ExecutionLimitsGuard()
        result = guard.check({
            "mode": ExecutionMode.LIVE,
            "limits": {"max_orders_per_day": 10},
            "stats": {"orders_today": 10},  # At limit
        })

        self.assertFalse(result.passed)
        self.assertIn("Max orders limit reached", result.message)

    def test_live_within_limits_allowed(self):
        """LIVE within limits is allowed."""
        guard = ExecutionLimitsGuard()
        result = guard.check({
            "mode": ExecutionMode.LIVE,
            "limits": {"max_orders_per_day": 10},
            "stats": {"orders_today": 5, "notional_today": 0, "loss_today": 0},
        })

        self.assertTrue(result.passed)


class AcknowledgementGuardTests(unittest.TestCase):
    """Tests for AcknowledgementGuard."""

    def test_dry_run_no_acknowledgement(self):
        """DRY-RUN doesn't need acknowledgement."""
        guard = AcknowledgementGuard()
        result = guard.check({"mode": ExecutionMode.DRY_RUN})

        self.assertTrue(result.passed)

    def test_live_requires_acknowledgement(self):
        """LIVE requires acknowledgement."""
        guard = AcknowledgementGuard()
        result = guard.check({
            "mode": ExecutionMode.LIVE,
            "acknowledge_risk": False,
        })

        self.assertFalse(result.passed)
        self.assertEqual(result.name, "Acknowledgement")

    def test_live_with_acknowledgement_allowed(self):
        """LIVE with acknowledgement is allowed."""
        guard = AcknowledgementGuard()
        result = guard.check({
            "mode": ExecutionMode.LIVE,
            "acknowledge_risk": True,
        })

        self.assertTrue(result.passed)


class ExecutePlanUseCaseTests(unittest.TestCase):
    """Tests for ExecutePlanUseCase."""

    def test_dry_run_always_allowed(self):
        """DRY-RUN is always allowed (safe by default)."""
        use_case = ExecutePlanUseCase()

        context = {
            "plan_id": "test123",
            "client_id": 1,
            "mode": ExecutionMode.DRY_RUN,
            "operation": {"type": "buy", "symbol": "BTCUSDT", "quantity": "0.1"},
        }

        result = use_case.execute(context)

        self.assertTrue(result.is_success())
        self.assertEqual(result.mode, ExecutionMode.DRY_RUN)
        self.assertGreater(len(result.actions), 0)
        self.assertEqual(result.actions[0]["type"], "SIMULATED_ORDER")

    def test_live_without_validation_blocked(self):
        """LIVE without validation is blocked."""
        use_case = ExecutePlanUseCase()

        context = {
            "plan_id": "test123",
            "client_id": 1,
            "mode": ExecutionMode.LIVE,
            "validated": False,
        }

        result = use_case.execute(context)

        self.assertTrue(result.is_blocked())
        self.assertEqual(result.status, ExecutionStatus.BLOCKED)

    def test_live_without_acknowledgement_blocked(self):
        """LIVE without acknowledgement is blocked."""
        use_case = ExecutePlanUseCase()

        context = {
            "plan_id": "test123",
            "client_id": 1,
            "mode": ExecutionMode.LIVE,
            "validated": True,
            "validation_passed": True,
            "acknowledge_risk": False,  # Missing
        }

        result = use_case.execute(context)

        self.assertTrue(result.is_blocked())

    def test_live_with_all_requirements_allowed(self):
        """LIVE with all requirements is allowed."""
        use_case = ExecutePlanUseCase()

        context = {
            "plan_id": "test123",
            "client_id": 1,
            "mode": ExecutionMode.LIVE,
            "validated": True,
            "validation_passed": True,
            "acknowledge_risk": True,
            "limits": {},
            "stats": {"orders_today": 0, "notional_today": 0, "loss_today": 0},
            "operation": {"type": "buy", "symbol": "BTCUSDT", "quantity": "0.1"},
        }

        result = use_case.execute(context)

        self.assertTrue(result.is_success())
        self.assertEqual(result.mode, ExecutionMode.LIVE)
        self.assertGreater(len(result.actions), 0)
        self.assertEqual(result.actions[0]["type"], "REAL_ORDER")

    def test_missing_client_id_blocked(self):
        """Missing client_id blocks execution."""
        use_case = ExecutePlanUseCase()

        context = {
            "plan_id": "test123",
            # Missing client_id
            "mode": ExecutionMode.DRY_RUN,
        }

        result = use_case.execute(context)

        self.assertTrue(result.is_blocked())

    def test_all_guards_executed(self):
        """All guards are executed and recorded."""
        use_case = ExecutePlanUseCase()

        context = {
            "plan_id": "test123",
            "client_id": 1,
            "mode": ExecutionMode.DRY_RUN,
        }

        result = use_case.execute(context)

        # Should have guards for: Tenant, Validation, Acknowledgement, Limits
        self.assertGreaterEqual(len(result.guards), 4)
        guard_names = [g.name for g in result.guards]
        self.assertIn("TenantContext", guard_names)
        self.assertIn("ValidationRequired", guard_names)
        self.assertIn("Acknowledgement", guard_names)
        self.assertIn("ExecutionLimits", guard_names)


if __name__ == "__main__":
    unittest.main()
