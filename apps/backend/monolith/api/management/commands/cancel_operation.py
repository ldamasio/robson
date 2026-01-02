# api/management/commands/cancel_operation.py
"""
Django management command to cancel an operation (Gate 6).

Internal command for testing and validation. Calls CancelOperationUseCase
to perform cancellation with proper business logic validation.

Usage:
    python manage.py cancel_operation --operation-id 123 --client-id 1 --confirm
"""

from django.core.management.base import BaseCommand

from api.application.use_cases import (
    CancelOperationUseCase,
    CancelOperationCommand,
)


class Command(BaseCommand):
    """Cancel an operation (internal command for testing/validation)."""

    help = 'Cancel an operation using the CancelOperationUseCase (Gate 6)'

    def add_arguments(self, parser):
        """Add command arguments."""
        parser.add_argument(
            '--operation-id',
            type=int,
            required=True,
            help='ID of the operation to cancel'
        )
        parser.add_argument(
            '--client-id',
            type=int,
            required=True,
            help='ID of the client requesting cancellation (for tenant isolation)'
        )
        parser.add_argument(
            '--confirm',
            action='store_true',
            help='Required for safety - must confirm to execute cancellation'
        )

    def handle(self, *args, **options):
        """Execute the cancellation command."""
        operation_id = options['operation_id']
        client_id = options['client_id']
        confirm = options['confirm']

        # Safety check: require --confirm flag
        if not confirm:
            self.stdout.write(self.style.ERROR('--confirm flag is required'))
            return

        # Execute use case
        use_case = CancelOperationUseCase()
        command = CancelOperationCommand(
            operation_id=operation_id,
            client_id=client_id
        )

        self.stdout.write(f"\n{'=' * 60}")
        self.stdout.write(f"Canceling Operation #{operation_id} for Client #{client_id}")
        self.stdout.write(f"{'=' * 60}\n")

        result = use_case.execute(command)

        if result.success:
            self.stdout.write(self.style.SUCCESS(
                f"✅ Operation cancelled successfully"
            ))
            self.stdout.write(f"   Operation ID: {result.operation_id}")
            self.stdout.write(f"   Status: {result.previous_status} → {result.new_status}")
        else:
            self.stdout.write(self.style.ERROR(
                f"❌ Cancellation failed"
            ))
            self.stdout.write(f"   Operation ID: {result.operation_id}")
            if result.previous_status:
                self.stdout.write(f"   Current Status: {result.previous_status}")
            self.stdout.write(f"   Error: {result.error_message}")

        self.stdout.write("")
