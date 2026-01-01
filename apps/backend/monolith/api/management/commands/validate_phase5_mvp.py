"""
Phase 5 MVP Validation Script

Executes Test Suites A/B/C from docs/testing/e2e-agentic-workflow.md (Scenario 9).
Produces PASS/FAIL evidence for each test case.

Usage:
    python manage.py validate_phase5_mvp
"""
import json
import uuid
from decimal import Decimal
from django.core.management.base import BaseCommand
from django.utils import timezone
from django.test import RequestFactory
from rest_framework.test import force_authenticate
from clients.models import Client
from api.models import Symbol, Strategy, TradingIntent
from api.models.trading import PatternTrigger
from api.views.trading_intent_views import pattern_trigger


class Command(BaseCommand):
    help = "Validate Phase 5 MVP (Pattern Auto-Trigger)"

    def __init__(self):
        super().__init__()
        self.factory = RequestFactory()
        self.results = []
        self.client = None
        self.symbol = None
        self.strategy = None

    def add_arguments(self, parser):
        parser.add_argument(
            "--client-id",
            type=int,
            help="Client ID to use for testing (defaults to first available)",
        )

    def handle(self, *args, **options):
        self.stdout.write(self.style.HTTP_INFO("=" * 80))
        self.stdout.write(self.style.HTTP_INFO("PHASE 5 MVP VALIDATION — PATTERN AUTO-TRIGGER"))
        self.stdout.write(self.style.HTTP_INFO("=" * 80))
        self.stdout.write("")

        # Setup test data
        if not self._setup_test_data(options.get("client_id")):
            self.stdout.write(self.style.ERROR("❌ Test data setup failed. Exiting."))
            return

        # Run test suites
        self._run_suite_a_happy_path()
        self._run_suite_b_idempotency()
        self._run_suite_c_live_block()

        # Summary
        self._print_summary()

    def _setup_test_data(self, client_id):
        """Setup test fixtures."""
        self.stdout.write(self.style.HTTP_INFO("Setting up test data..."))

        # Get client
        if client_id:
            try:
                self.client = Client.objects.get(id=client_id)
            except Client.DoesNotExist:
                self.stdout.write(self.style.ERROR(f"Client {client_id} not found"))
                return False
        else:
            if not Client.objects.exists():
                self.stdout.write(self.style.ERROR("No clients found in database"))
                return False
            self.client = Client.objects.first()

        self.stdout.write(f"  Client: ID={self.client.id}")

        # Get symbol
        try:
            self.symbol = Symbol.objects.get(name="BTCUSDT")
            self.stdout.write(f"  Symbol: {self.symbol.name} (ID={self.symbol.id})")
        except Symbol.DoesNotExist:
            self.stdout.write(self.style.ERROR("BTCUSDT symbol not found"))
            return False

        # Get strategy (optional)
        if Strategy.objects.exists():
            self.strategy = Strategy.objects.first()
            self.stdout.write(f"  Strategy: {self.strategy.name} (ID={self.strategy.id})")
        else:
            self.stdout.write(self.style.WARNING("  No strategies found (will skip strategy field)"))

        self.stdout.write("")
        return True

    def _call_pattern_trigger(self, payload):
        """Make a POST request to /api/pattern-triggers/."""
        request = self.factory.post(
            "/api/pattern-triggers/",
            data=json.dumps(payload),
            content_type="application/json",
        )
        # Get the first user associated with this client
        user = self.client.users.first()
        force_authenticate(request, user=user)
        response = pattern_trigger(request)
        return response

    def _record_result(self, test_id, description, passed, evidence):
        """Record test result."""
        self.results.append({
            "test_id": test_id,
            "description": description,
            "passed": passed,
            "evidence": evidence,
        })

        status_icon = "✅ PASS" if passed else "❌ FAIL"
        status_style = self.style.SUCCESS if passed else self.style.ERROR
        self.stdout.write(f"{status_style(status_icon)} {test_id}: {description}")
        if evidence:
            self.stdout.write(f"    Evidence: {evidence}")

    # ========== TEST SUITE A: HAPPY PATH ==========

    def _run_suite_a_happy_path(self):
        """Test Suite A: Happy Path (MVP-01 to MVP-05)."""
        self.stdout.write(self.style.HTTP_INFO("\n" + "=" * 80))
        self.stdout.write(self.style.HTTP_INFO("TEST SUITE A: HAPPY PATH (MVP-01 to MVP-05)"))
        self.stdout.write(self.style.HTTP_INFO("=" * 80))

        # Generate unique event ID for this test
        pattern_event_id = f"test_evt_{uuid.uuid4().hex[:8]}"

        # Prepare payload
        payload = {
            "pattern_code": "HAMMER",
            "pattern_event_id": pattern_event_id,
            "symbol": self.symbol.id,
            "side": "BUY",
            "entry_price": "95000",
            "stop_price": "93500",
            "capital": "100",
            "auto_validate": True,
        }

        if self.strategy:
            payload["strategy"] = self.strategy.id

        # Call endpoint
        response = self._call_pattern_trigger(payload)
        response_data = response.data if hasattr(response, "data") else {}

        # MVP-01: Pattern trigger creates TradingIntent
        intent_created = (
            response.status_code == 201
            and response_data.get("status") == "PROCESSED"
            and "intent_id" in response_data
        )
        self._record_result(
            "MVP-01",
            "Pattern trigger creates TradingIntent",
            intent_created,
            f"status={response.status_code}, response={response_data.get('status')}, intent_id={response_data.get('intent_id')}",
        )

        if not intent_created:
            self.stdout.write(self.style.ERROR(f"Full response: {response_data}"))
            return  # Cannot continue suite A without intent

        intent_id = response_data.get("intent_id")

        # MVP-02: Auto-validation works
        validation_result_present = "validation_result" in response_data
        self._record_result(
            "MVP-02",
            "Auto-validation works",
            validation_result_present,
            f"validation_result present: {validation_result_present}",
        )

        # MVP-03: Validation results returned
        if validation_result_present:
            validation_result = response_data.get("validation_result", {})
            has_status = "status" in validation_result
            self._record_result(
                "MVP-03",
                "Validation results returned",
                has_status,
                f"validation_result.status={validation_result.get('status')}",
            )
        else:
            self._record_result(
                "MVP-03",
                "Validation results returned",
                False,
                "validation_result missing from response",
            )

        # MVP-04: Pattern metadata persisted
        try:
            intent = TradingIntent.objects.get(intent_id=intent_id)
            metadata_correct = (
                intent.pattern_code == "HAMMER"
                and intent.pattern_event_id == pattern_event_id
                and intent.pattern_source == "pattern"
                and intent.pattern_triggered_at is not None
            )
            self._record_result(
                "MVP-04",
                "Pattern metadata persisted",
                metadata_correct,
                f"pattern_code={intent.pattern_code}, pattern_event_id={intent.pattern_event_id}, pattern_source={intent.pattern_source}",
            )
        except TradingIntent.DoesNotExist:
            self._record_result(
                "MVP-04",
                "Pattern metadata persisted",
                False,
                f"TradingIntent {intent_id} not found in database",
            )

        # MVP-05: Frontend shows "Triggered by pattern" (SKIPPED - requires frontend)
        self._record_result(
            "MVP-05",
            "Frontend shows 'Triggered by pattern'",
            None,  # Unknown
            "SKIPPED (requires frontend testing)",
        )

    # ========== TEST SUITE B: IDEMPOTENCY ==========

    def _run_suite_b_idempotency(self):
        """Test Suite B: Idempotency (MVP-06 to MVP-10)."""
        self.stdout.write(self.style.HTTP_INFO("\n" + "=" * 80))
        self.stdout.write(self.style.HTTP_INFO("TEST SUITE B: IDEMPOTENCY (MVP-06 to MVP-10)"))
        self.stdout.write(self.style.HTTP_INFO("=" * 80))

        # Generate unique event ID for this test
        pattern_event_id = f"test_evt_{uuid.uuid4().hex[:8]}"

        # Prepare payload
        payload = {
            "pattern_code": "HAMMER",
            "pattern_event_id": pattern_event_id,
            "symbol": self.symbol.id,
            "side": "BUY",
            "entry_price": "95000",
            "stop_price": "93500",
            "capital": "100",
            "auto_validate": True,
        }

        if self.strategy:
            payload["strategy"] = self.strategy.id

        # First call
        response1 = self._call_pattern_trigger(payload)
        response1_data = response1.data if hasattr(response1, "data") else {}

        if response1.status_code != 201 or response1_data.get("status") != "PROCESSED":
            self.stdout.write(self.style.ERROR(f"First call failed: {response1_data}"))
            return

        intent_id_1 = response1_data.get("intent_id")

        # Second call (duplicate)
        response2 = self._call_pattern_trigger(payload)
        response2_data = response2.data if hasattr(response2, "data") else {}

        # MVP-06: Repeated calls don't create duplicates
        # MVP-07: Response returns ALREADY_PROCESSED
        already_processed = (
            response2.status_code == 200
            and response2_data.get("status") == "ALREADY_PROCESSED"
        )
        self._record_result(
            "MVP-06/07",
            "Repeated calls don't create duplicates + returns ALREADY_PROCESSED",
            already_processed,
            f"status={response2.status_code}, response.status={response2_data.get('status')}",
        )

        # MVP-08: Same intent_id returned
        intent_id_2 = response2_data.get("intent_id")
        same_intent = intent_id_1 == intent_id_2
        self._record_result(
            "MVP-08",
            "Same intent_id returned",
            same_intent,
            f"intent_id_1={intent_id_1}, intent_id_2={intent_id_2}",
        )

        # MVP-09: DB enforces idempotency (check PatternTrigger count)
        trigger_count = PatternTrigger.objects.filter(
            pattern_event_id=pattern_event_id
        ).count()
        db_enforces_idempotency = trigger_count == 1
        self._record_result(
            "MVP-09",
            "DB enforces idempotency (unique constraint)",
            db_enforces_idempotency,
            f"PatternTrigger count for event_id={pattern_event_id}: {trigger_count}",
        )

        # MVP-10: Concurrent safety (SKIPPED - requires load testing)
        self._record_result(
            "MVP-10",
            "Concurrent safety",
            None,
            "SKIPPED (requires load testing)",
        )

    # ========== TEST SUITE C: LIVE AUTO-EXECUTION BLOCK ==========

    def _run_suite_c_live_block(self):
        """Test Suite C: LIVE Auto-Execution Block (MVP-11 to MVP-14)."""
        self.stdout.write(self.style.HTTP_INFO("\n" + "=" * 80))
        self.stdout.write(self.style.HTTP_INFO("TEST SUITE C: LIVE AUTO-EXECUTION BLOCK (MVP-11 to MVP-14)"))
        self.stdout.write(self.style.HTTP_INFO("=" * 80))

        # Generate unique event ID for this test
        pattern_event_id = f"test_evt_{uuid.uuid4().hex[:8]}"

        # Prepare payload with LIVE auto-execution
        payload = {
            "pattern_code": "HAMMER",
            "pattern_event_id": pattern_event_id,
            "symbol": self.symbol.id,
            "side": "BUY",
            "entry_price": "95000",
            "stop_price": "93500",
            "capital": "100",
            "auto_validate": True,
            "auto_execute": True,  # ← Enable auto-execute
            "execution_mode": "live",  # ← LIVE mode (should be blocked)
        }

        if self.strategy:
            payload["strategy"] = self.strategy.id

        # Call endpoint
        response = self._call_pattern_trigger(payload)
        response_data = response.data if hasattr(response, "data") else {}

        # MVP-11: LIVE auto-execution hard-blocked
        blocked = response.status_code == 400
        self._record_result(
            "MVP-11",
            "LIVE auto-execution hard-blocked",
            blocked,
            f"status={response.status_code}, expected=400",
        )

        # MVP-12: Error message explicit
        if blocked:
            error_message = str(response_data)
            message_explicit = "LIVE" in error_message and "auto-execution" in error_message
            self._record_result(
                "MVP-12",
                "Error message explicit and user-visible",
                message_explicit,
                f"error={error_message[:100]}",
            )
        else:
            self._record_result(
                "MVP-12",
                "Error message explicit and user-visible",
                False,
                f"Expected 400 error, got {response.status_code}",
            )

        # MVP-13: Dry-run auto allowed (test auto_execute=true + execution_mode=dry-run)
        payload_dryrun = payload.copy()
        payload_dryrun["pattern_event_id"] = f"test_evt_{uuid.uuid4().hex[:8]}"  # New event ID
        payload_dryrun["execution_mode"] = "dry-run"

        response_dryrun = self._call_pattern_trigger(payload_dryrun)
        dryrun_allowed = response_dryrun.status_code == 201
        self._record_result(
            "MVP-13",
            "Dry-run auto-execute allowed",
            dryrun_allowed,
            f"status={response_dryrun.status_code}, expected=201",
        )

        # MVP-14: Manual LIVE execution possible (SKIPPED - not in Phase 5 scope)
        self._record_result(
            "MVP-14",
            "Manual LIVE execution possible later",
            None,
            "SKIPPED (Phase 4 functionality, not Phase 5 scope)",
        )

    # ========== SUMMARY ==========

    def _print_summary(self):
        """Print summary of all test results."""
        self.stdout.write(self.style.HTTP_INFO("\n" + "=" * 80))
        self.stdout.write(self.style.HTTP_INFO("VALIDATION SUMMARY"))
        self.stdout.write(self.style.HTTP_INFO("=" * 80))

        passed = sum(1 for r in self.results if r["passed"] is True)
        failed = sum(1 for r in self.results if r["passed"] is False)
        skipped = sum(1 for r in self.results if r["passed"] is None)
        total = len(self.results)

        self.stdout.write(f"\nTotal tests: {total}")
        self.stdout.write(self.style.SUCCESS(f"  ✅ PASSED:  {passed}"))
        self.stdout.write(self.style.ERROR(f"  ❌ FAILED:  {failed}"))
        self.stdout.write(self.style.WARNING(f"  ⏭️  SKIPPED: {skipped}"))

        if failed > 0:
            self.stdout.write(self.style.ERROR("\n❌ VALIDATION FAILED"))
            self.stdout.write(self.style.ERROR("\nFailed tests:"))
            for r in self.results:
                if r["passed"] is False:
                    self.stdout.write(f"  - {r['test_id']}: {r['description']}")
                    self.stdout.write(f"    Evidence: {r['evidence']}")
        else:
            self.stdout.write(self.style.SUCCESS("\n✅ VALIDATION PASSED"))
            self.stdout.write("\nAll critical tests passed. Phase 5 MVP is validated.")

        self.stdout.write("")
