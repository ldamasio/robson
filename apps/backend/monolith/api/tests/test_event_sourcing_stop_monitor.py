# api/tests/test_event_sourcing_stop_monitor.py
"""
Tests for Event-Sourced Stop-Loss Monitor (ADR-0012).

Tests cover:
1. Backfill command (stop_price calculation)
2. Idempotency (execution_token prevents duplicates)
3. Event sourcing (event log + projection)
4. Deduplication (WS + CronJob simultaneous triggers)
"""

import pytest
from decimal import Decimal
from django.utils import timezone
from django.core.management import call_command
from io import StringIO

from api.models import Operation, Symbol, Strategy, Client, Order
from api.models.event_sourcing import (
    StopEvent, StopExecution, StopEventType, ExecutionSource, ExecutionStatus
)
from api.application.stop_monitor import PriceMonitor, StopExecutor, TriggerEvent, TriggerType
from django.db import IntegrityError


# =====================================================================
# FIXTURES
# =====================================================================

@pytest.fixture
def client_user(db):
    """Create test client."""
    from django.contrib.auth import get_user_model
    User = get_user_model()

    user = User.objects.create_user(
        username="testuser",
        email="test@example.com",
        password="testpass123"
    )

    client = Client.objects.create(
        user=user,
        name="Test Client",
        binance_api_key="test_key",
        binance_api_secret="test_secret",
    )

    return client


@pytest.fixture
def btc_symbol(client_user):
    """Create BTC/USDC symbol."""
    return Symbol.objects.create(
        client=client_user,
        name="BTCUSDC",
        base_asset="BTC",
        quote_asset="USDC",
        is_active=True,
    )


@pytest.fixture
def test_strategy(client_user):
    """Create test strategy."""
    return Strategy.objects.create(
        client=client_user,
        name="Test Strategy",
        description="Test strategy for stop monitor",
        is_active=True,
        config={},
    )


@pytest.fixture
def operation_long(client_user, btc_symbol, test_strategy):
    """Create LONG operation with stop_loss_percent (for backfill testing)."""
    operation = Operation.objects.create(
        client=client_user,
        symbol=btc_symbol,
        strategy=test_strategy,
        side="BUY",
        status="ACTIVE",
        stop_loss_percent=Decimal("2.0"),  # 2% stop
        # NO stop_price yet (will be backfilled)
    )

    # Create entry order with fill
    entry_order = Order.objects.create(
        client=client_user,
        symbol=btc_symbol,
        strategy=test_strategy,
        side="BUY",
        order_type="MARKET",
        quantity=Decimal("0.001"),
        filled_quantity=Decimal("0.001"),
        avg_fill_price=Decimal("90000.00"),  # Entry at $90,000
        status="FILLED",
        binance_order_id="12345",
    )

    operation.entry_orders.add(entry_order)
    return operation


@pytest.fixture
def operation_with_stop_price(client_user, btc_symbol, test_strategy):
    """Create operation with absolute stop_price."""
    operation = Operation.objects.create(
        client=client_user,
        symbol=btc_symbol,
        strategy=test_strategy,
        side="BUY",
        status="ACTIVE",
        stop_price=Decimal("88200.00"),  # Absolute stop level
        target_price=Decimal("93600.00"),  # Absolute target level
        stop_loss_percent=Decimal("2.0"),  # Reference only (deprecated)
    )

    # Create entry order
    entry_order = Order.objects.create(
        client=client_user,
        symbol=btc_symbol,
        strategy=test_strategy,
        side="BUY",
        order_type="MARKET",
        quantity=Decimal("0.001"),
        filled_quantity=Decimal("0.001"),
        avg_fill_price=Decimal("90000.00"),  # Entry at $90,000
        status="FILLED",
        binance_order_id="12345",
    )

    operation.entry_orders.add(entry_order)
    return operation


# =====================================================================
# TEST: BACKFILL COMMAND
# =====================================================================

@pytest.mark.django_db
def test_backfill_stop_price_calculates_correctly(operation_long):
    """Test backfill command calculates stop_price from percentage."""
    # GIVEN: Operation with stop_loss_percent but no stop_price
    assert operation_long.stop_price is None
    assert operation_long.stop_loss_percent == Decimal("2.0")
    assert operation_long.average_entry_price == Decimal("90000.00")

    # WHEN: Run backfill command
    out = StringIO()
    call_command('backfill_stop_price', '--dry-run', stdout=out)

    # Reload operation (dry-run doesn't save, so run again without dry-run)
    call_command('backfill_stop_price', stdout=out)
    operation_long.refresh_from_db()

    # THEN: stop_price calculated correctly (BUY stop is below entry)
    expected_stop = Decimal("90000.00") * (Decimal("1") - Decimal("2.0") / Decimal("100"))
    assert operation_long.stop_price == expected_stop
    assert operation_long.stop_price == Decimal("88200.00")


@pytest.mark.django_db
def test_backfill_validates_stop_direction(client_user, btc_symbol, test_strategy):
    """Test backfill validates stop_price direction (BUY stop must be < entry)."""
    # GIVEN: Invalid operation (BUY with stop > entry - should skip)
    operation = Operation.objects.create(
        client=client_user,
        symbol=btc_symbol,
        strategy=test_strategy,
        side="BUY",
        status="ACTIVE",
        stop_loss_percent=Decimal("-5.0"),  # NEGATIVE percent (invalid!)
    )

    entry_order = Order.objects.create(
        client=client_user,
        symbol=btc_symbol,
        strategy=test_strategy,
        side="BUY",
        order_type="MARKET",
        quantity=Decimal("0.001"),
        filled_quantity=Decimal("0.001"),
        avg_fill_price=Decimal("90000.00"),
        status="FILLED",
        binance_order_id="99999",
    )
    operation.entry_orders.add(entry_order)

    # WHEN: Run backfill
    out = StringIO()
    call_command('backfill_stop_price', stdout=out)
    output = out.getvalue()

    # THEN: Operation skipped with validation error
    assert "Skipped" in output or "Invalid stop" in output


# =====================================================================
# TEST: IDEMPOTENCY (Execution Token)
# =====================================================================

@pytest.mark.django_db
def test_execution_token_prevents_duplicate_events():
    """Test execution_token prevents duplicate events in stop_events table."""
    # GIVEN: First event with token
    from django.contrib.auth import get_user_model
    User = get_user_model()
    user = User.objects.create_user(username="test", email="test@test.com")
    client = Client.objects.create(user=user, name="Test")
    symbol = Symbol.objects.create(client=client, name="BTCUSDC", base_asset="BTC", quote_asset="USDC")
    strategy = Strategy.objects.create(client=client, name="Test", is_active=True)
    operation = Operation.objects.create(
        client=client,
        symbol=symbol,
        strategy=strategy,
        side="BUY",
        status="ACTIVE",
        stop_price=Decimal("88200.00"),
    )

    token = f"{operation.id}:88200.00:1234567890"

    event1 = StopEvent.objects.create(
        operation=operation,
        client=client,
        symbol="BTCUSDC",
        event_type=StopEventType.STOP_TRIGGERED,
        execution_token=token,
        source=ExecutionSource.CRONJOB,
        trigger_price=Decimal("88200.00"),
        stop_price=Decimal("88200.00"),
        quantity=Decimal("0.001"),
        side="SELL",
    )

    # WHEN: Try to create duplicate event with same token
    # THEN: Should raise IntegrityError
    with pytest.raises(IntegrityError):
        StopEvent.objects.create(
            operation=operation,
            client=client,
            symbol="BTCUSDC",
            event_type=StopEventType.EXECUTION_SUBMITTED,
            execution_token=token,  # SAME token
            source=ExecutionSource.WEBSOCKET,
            trigger_price=Decimal("88200.00"),
            stop_price=Decimal("88200.00"),
            quantity=Decimal("0.001"),
            side="SELL",
        )


@pytest.mark.django_db
def test_stop_executor_idempotency_prevents_duplicate_execution(operation_with_stop_price, mocker):
    """Test StopExecutor prevents duplicate execution when same token is used."""
    # GIVEN: Mock market data and execution
    mock_market_data = mocker.MagicMock()
    mock_market_data.best_bid.return_value = Decimal("88000.00")  # Below stop

    mock_execution = mocker.MagicMock()
    mock_execution.place_market.return_value = {
        "orderId": "999",
        "executedQty": "0.001",
        "status": "FILLED",
        "fills": [{
            "price": "88000.00",
            "qty": "0.001",
            "commission": "0.00001",
        }],
    }

    # Create trigger event
    trigger = TriggerEvent(
        operation_id=operation_with_stop_price.id,
        trigger_type=TriggerType.STOP_LOSS,
        trigger_price=Decimal("88200.00"),
        current_price=Decimal("88000.00"),
        entry_price=Decimal("90000.00"),
        quantity=Decimal("0.001"),
        symbol="BTCUSDC",
        expected_pnl=Decimal("-2.00"),
    )

    # WHEN: Execute once (succeeds)
    executor = StopExecutor(execution_port=mock_execution)
    result1 = executor.execute(trigger, source="cron")

    assert result1.success is True
    assert StopEvent.objects.filter(operation=operation_with_stop_price).count() == 3  # TRIGGERED, SUBMITTED, EXECUTED

    # WHEN: Execute again with SAME trigger (simulates WS + CronJob race)
    # Create second executor to simulate independent process
    executor2 = StopExecutor(execution_port=mock_execution)

    # Manually set same timestamp to force collision
    import time
    original_now = timezone.now

    def fixed_now():
        return original_now()

    mocker.patch('django.utils.timezone.now', side_effect=fixed_now)

    result2 = executor2.execute(trigger, source="ws")

    # THEN: Second execution prevented (idempotency)
    assert result2.success is False
    assert "Duplicate execution prevented" in result2.error or "idempotency" in result2.error


# =====================================================================
# TEST: EVENT SOURCING (Event Log + Projection)
# =====================================================================

@pytest.mark.django_db
def test_stop_executor_emits_events_on_success(operation_with_stop_price, mocker):
    """Test StopExecutor emits events to stop_events on successful execution."""
    # GIVEN: Mock execution
    mock_execution = mocker.MagicMock()
    mock_execution.place_market.return_value = {
        "orderId": "12345",
        "executedQty": "0.001",
        "status": "FILLED",
        "fills": [{
            "price": "88000.00",
            "qty": "0.001",
            "commission": "0.00001",
        }],
    }

    trigger = TriggerEvent(
        operation_id=operation_with_stop_price.id,
        trigger_type=TriggerType.STOP_LOSS,
        trigger_price=Decimal("88200.00"),
        current_price=Decimal("88000.00"),
        entry_price=Decimal("90000.00"),
        quantity=Decimal("0.001"),
        symbol="BTCUSDC",
        expected_pnl=Decimal("-2.00"),
    )

    # WHEN: Execute stop
    executor = StopExecutor(execution_port=mock_execution)
    result = executor.execute(trigger, source="cron")

    # THEN: Success
    assert result.success is True

    # THEN: Events emitted (TRIGGERED, SUBMITTED, EXECUTED)
    events = StopEvent.objects.filter(operation=operation_with_stop_price).order_by('event_seq')
    assert events.count() == 3

    assert events[0].event_type == StopEventType.STOP_TRIGGERED
    assert events[1].event_type == StopEventType.EXECUTION_SUBMITTED
    assert events[2].event_type == StopEventType.EXECUTED

    # THEN: All events have same execution_token
    token = events[0].execution_token
    assert all(e.execution_token == token for e in events)

    # THEN: EXECUTED event has fill details
    executed_event = events[2]
    assert executed_event.exchange_order_id == "12345"
    assert executed_event.fill_price == Decimal("88000.00")
    assert executed_event.slippage_pct is not None


@pytest.mark.django_db
def test_stop_executor_updates_projection(operation_with_stop_price, mocker):
    """Test StopExecutor updates stop_executions projection."""
    # GIVEN: Mock execution
    mock_execution = mocker.MagicMock()
    mock_execution.place_market.return_value = {
        "orderId": "67890",
        "executedQty": "0.001",
        "status": "FILLED",
        "fills": [{
            "price": "88000.00",
            "qty": "0.001",
            "commission": "0.00001",
        }],
    }

    trigger = TriggerEvent(
        operation_id=operation_with_stop_price.id,
        trigger_type=TriggerType.STOP_LOSS,
        trigger_price=Decimal("88200.00"),
        current_price=Decimal("88000.00"),
        entry_price=Decimal("90000.00"),
        quantity=Decimal("0.001"),
        symbol="BTCUSDC",
        expected_pnl=Decimal("-2.00"),
    )

    # WHEN: Execute stop
    executor = StopExecutor(execution_port=mock_execution)
    result = executor.execute(trigger, source="ws")

    # THEN: Success
    assert result.success is True

    # THEN: Projection created and updated
    execution = StopExecution.objects.get(operation=operation_with_stop_price)

    assert execution.status == ExecutionStatus.EXECUTED
    assert execution.stop_price == Decimal("88200.00")
    assert execution.trigger_price == Decimal("88000.00")
    assert execution.exchange_order_id == "67890"
    assert execution.fill_price == Decimal("88000.00")
    assert execution.source == ExecutionSource.WEBSOCKET
    assert execution.triggered_at is not None
    assert execution.submitted_at is not None
    assert execution.executed_at is not None


@pytest.mark.django_db
def test_stop_executor_emits_failed_event_on_error(operation_with_stop_price, mocker):
    """Test StopExecutor emits FAILED event when execution fails."""
    # GIVEN: Mock execution that raises error
    mock_execution = mocker.MagicMock()
    mock_execution.place_market.side_effect = Exception("Binance API error")

    trigger = TriggerEvent(
        operation_id=operation_with_stop_price.id,
        trigger_type=TriggerType.STOP_LOSS,
        trigger_price=Decimal("88200.00"),
        current_price=Decimal("88000.00"),
        entry_price=Decimal("90000.00"),
        quantity=Decimal("0.001"),
        symbol="BTCUSDC",
        expected_pnl=Decimal("-2.00"),
    )

    # WHEN: Execute stop (will fail)
    executor = StopExecutor(execution_port=mock_execution)
    result = executor.execute(trigger, source="cron")

    # THEN: Execution failed
    assert result.success is False
    assert "Binance API error" in result.error

    # THEN: FAILED event emitted
    events = StopEvent.objects.filter(operation=operation_with_stop_price).order_by('event_seq')
    assert events.count() >= 3  # TRIGGERED, SUBMITTED, FAILED

    failed_event = events.filter(event_type=StopEventType.FAILED).first()
    assert failed_event is not None
    assert "Binance API error" in failed_event.error_message

    # THEN: Projection updated to FAILED
    execution = StopExecution.objects.get(operation=operation_with_stop_price)
    assert execution.status == ExecutionStatus.FAILED
    assert execution.error_message == "Binance API error"


# =====================================================================
# TEST: DEDUPLICATION (Simultaneous WS + CronJob Triggers)
# =====================================================================

@pytest.mark.django_db
def test_simultaneous_ws_and_cron_triggers_deduplicated(operation_with_stop_price, mocker):
    """Test simultaneous WS and CronJob triggers are deduplicated via execution_token."""
    # GIVEN: Mock execution (fast enough to cause race condition)
    mock_execution = mocker.MagicMock()
    mock_execution.place_market.return_value = {
        "orderId": "RACE123",
        "executedQty": "0.001",
        "status": "FILLED",
        "fills": [{
            "price": "88000.00",
            "qty": "0.001",
            "commission": "0.00001",
        }],
    }

    trigger = TriggerEvent(
        operation_id=operation_with_stop_price.id,
        trigger_type=TriggerType.STOP_LOSS,
        trigger_price=Decimal("88200.00"),
        current_price=Decimal("88000.00"),
        entry_price=Decimal("90000.00"),
        quantity=Decimal("0.001"),
        symbol="BTCUSDC",
        expected_pnl=Decimal("-2.00"),
    )

    # WHEN: Simulate race condition (same timestamp)
    # Fix timezone.now() to return same value for both executors
    fixed_time = timezone.now()
    mocker.patch('django.utils.timezone.now', return_value=fixed_time)

    # Execute from CronJob
    executor_cron = StopExecutor(execution_port=mock_execution)
    result_cron = executor_cron.execute(trigger, source="cron")

    # Execute from WebSocket (same timestamp = same token)
    executor_ws = StopExecutor(execution_port=mock_execution)
    result_ws = executor_ws.execute(trigger, source="ws")

    # THEN: One succeeds, one is rejected
    assert result_cron.success is True or result_ws.success is True
    assert result_cron.success is False or result_ws.success is False

    # THEN: Only one execution in projection
    executions = StopExecution.objects.filter(operation=operation_with_stop_price)
    assert executions.count() == 1

    # THEN: Only one EXECUTED event (or one FAILED if second failed fast)
    executed_events = StopEvent.objects.filter(
        operation=operation_with_stop_price,
        event_type=StopEventType.EXECUTED
    )
    assert executed_events.count() == 1


# =====================================================================
# TEST: MONITOR WITH ABSOLUTE STOP_PRICE
# =====================================================================

@pytest.mark.django_db
def test_price_monitor_uses_absolute_stop_price(operation_with_stop_price, mocker):
    """Test PriceMonitor uses operation.stop_price instead of recalculating from percentage."""
    # GIVEN: Operation with stop_price
    assert operation_with_stop_price.stop_price == Decimal("88200.00")

    # Mock market data (price below stop)
    mock_market_data = mocker.MagicMock()
    mock_market_data.best_bid.return_value = Decimal("88000.00")  # Below stop

    # WHEN: Monitor checks operation
    monitor = PriceMonitor(market_data_port=mock_market_data)
    trigger = monitor.check_operation(operation_with_stop_price)

    # THEN: Trigger detected
    assert trigger is not None
    assert trigger.trigger_type == TriggerType.STOP_LOSS
    assert trigger.trigger_price == Decimal("88200.00")  # Uses absolute stop_price
    assert trigger.current_price == Decimal("88000.00")


@pytest.mark.django_db
def test_price_monitor_skips_operation_without_stop_price(client_user, btc_symbol, test_strategy, mocker):
    """Test PriceMonitor skips operations without stop_price."""
    # GIVEN: Operation WITHOUT stop_price
    operation = Operation.objects.create(
        client=client_user,
        symbol=btc_symbol,
        strategy=test_strategy,
        side="BUY",
        status="ACTIVE",
        # NO stop_price
        # NO target_price
    )

    entry_order = Order.objects.create(
        client=client_user,
        symbol=btc_symbol,
        strategy=test_strategy,
        side="BUY",
        order_type="MARKET",
        quantity=Decimal("0.001"),
        filled_quantity=Decimal("0.001"),
        avg_fill_price=Decimal("90000.00"),
        status="FILLED",
    )
    operation.entry_orders.add(entry_order)

    # Mock market data
    mock_market_data = mocker.MagicMock()
    mock_market_data.best_bid.return_value = Decimal("85000.00")  # Way below entry

    # WHEN: Monitor checks operation
    monitor = PriceMonitor(market_data_port=mock_market_data)
    trigger = monitor.check_operation(operation)

    # THEN: No trigger (operation skipped)
    assert trigger is None
