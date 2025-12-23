"""
Tests for InMemoryMessageBus

Run with: pytest apps/backend/core/tests/test_in_memory_message_bus.py -v
"""

import pytest
from datetime import datetime
from decimal import Decimal

from apps.backend.core.application.ports import (
    DomainEvent,
    OrderPlacedEvent,
    OrderFilledEvent,
)
from apps.backend.core.adapters.driven.messaging.in_memory import InMemoryMessageBus


class TestInMemoryMessageBus:
    """Test InMemoryMessageBus adapter."""

    @pytest.fixture
    def bus(self):
        """Create fresh message bus for each test."""
        return InMemoryMessageBus()

    @pytest.fixture
    def sample_event(self):
        """Create a sample OrderPlacedEvent."""
        return OrderPlacedEvent(
            event_id="evt-001",
            event_type="OrderPlaced",
            timestamp=datetime.now(),
            aggregate_id="order-123",
            order_id="order-123",
            intent_id="intent-456",
            client_id=1,
            symbol="BTCUSDC",
            side="BUY",
            quantity=Decimal("0.001"),
            price=Decimal("90000.00"),
            exchange_order_id="binance-789",
        )

    def test_publish_without_subscribers(self, bus, sample_event):
        """Publishing without subscribers should not raise error."""
        # Should not raise
        bus.publish(sample_event)

    def test_single_subscriber_receives_event(self, bus, sample_event):
        """Single subscriber should receive published event."""
        received = []

        def handler(event):
            received.append(event)

        bus.subscribe("OrderPlaced", handler)
        bus.publish(sample_event)

        assert len(received) == 1
        assert received[0] == sample_event

    def test_multiple_subscribers_all_receive_event(self, bus, sample_event):
        """All subscribers should receive the event."""
        received_1 = []
        received_2 = []

        def handler_1(event):
            received_1.append(event)

        def handler_2(event):
            received_2.append(event)

        bus.subscribe("OrderPlaced", handler_1)
        bus.subscribe("OrderPlaced", handler_2)
        bus.publish(sample_event)

        assert len(received_1) == 1
        assert len(received_2) == 1
        assert received_1[0] == sample_event
        assert received_2[0] == sample_event

    def test_handlers_called_in_order(self, bus, sample_event):
        """Handlers should be called in registration order."""
        call_order = []

        def handler_1(event):
            call_order.append(1)

        def handler_2(event):
            call_order.append(2)

        def handler_3(event):
            call_order.append(3)

        bus.subscribe("OrderPlaced", handler_1)
        bus.subscribe("OrderPlaced", handler_2)
        bus.subscribe("OrderPlaced", handler_3)
        bus.publish(sample_event)

        assert call_order == [1, 2, 3]

    def test_handler_exception_does_not_stop_others(self, bus, sample_event):
        """If one handler fails, others should still run."""
        received = []

        def failing_handler(event):
            raise RuntimeError("Handler failed!")

        def success_handler(event):
            received.append(event)

        bus.subscribe("OrderPlaced", failing_handler)
        bus.subscribe("OrderPlaced", success_handler)

        # Should not raise (exception is logged but not propagated)
        bus.publish(sample_event)

        # Second handler should still have received the event
        assert len(received) == 1

    def test_different_event_types_routed_correctly(self, bus):
        """Events should be routed only to matching subscribers."""
        order_placed_received = []
        order_filled_received = []

        def order_placed_handler(event):
            order_placed_received.append(event)

        def order_filled_handler(event):
            order_filled_received.append(event)

        bus.subscribe("OrderPlaced", order_placed_handler)
        bus.subscribe("OrderFilled", order_filled_handler)

        # Publish OrderPlaced event
        placed_event = OrderPlacedEvent(
            event_id="evt-001",
            event_type="OrderPlaced",
            timestamp=datetime.now(),
            aggregate_id="order-123",
            order_id="order-123",
            intent_id=None,
            client_id=1,
            symbol="BTCUSDC",
            side="BUY",
            quantity=Decimal("0.001"),
            price=Decimal("90000.00"),
            exchange_order_id="binance-789",
        )
        bus.publish(placed_event)

        # Publish OrderFilled event
        filled_event = OrderFilledEvent(
            event_id="evt-002",
            event_type="OrderFilled",
            timestamp=datetime.now(),
            aggregate_id="order-123",
            order_id="order-123",
            exchange_order_id="binance-789",
            filled_quantity=Decimal("0.001"),
            avg_fill_price=Decimal("90050.00"),
        )
        bus.publish(filled_event)

        # Check routing
        assert len(order_placed_received) == 1
        assert len(order_filled_received) == 1
        assert order_placed_received[0] == placed_event
        assert order_filled_received[0] == filled_event

    def test_unsubscribe(self, bus, sample_event):
        """Unsubscribing should stop handler from receiving events."""
        received = []

        def handler(event):
            received.append(event)

        bus.subscribe("OrderPlaced", handler)
        bus.publish(sample_event)
        assert len(received) == 1

        bus.unsubscribe("OrderPlaced", handler)
        bus.publish(sample_event)
        assert len(received) == 1  # Still 1, not 2

    def test_clear_all(self, bus, sample_event):
        """clear_all() should remove all subscriptions."""
        received_1 = []
        received_2 = []

        def handler_1(event):
            received_1.append(event)

        def handler_2(event):
            received_2.append(event)

        bus.subscribe("OrderPlaced", handler_1)
        bus.subscribe("OrderFilled", handler_2)

        bus.clear_all()

        bus.publish(sample_event)

        assert len(received_1) == 0
        assert len(received_2) == 0

    def test_get_handler_count(self, bus):
        """get_handler_count() should return correct count."""
        assert bus.get_handler_count("OrderPlaced") == 0

        bus.subscribe("OrderPlaced", lambda e: None)
        assert bus.get_handler_count("OrderPlaced") == 1

        bus.subscribe("OrderPlaced", lambda e: None)
        assert bus.get_handler_count("OrderPlaced") == 2

    def test_thread_safety_multiple_publishes(self, bus, sample_event):
        """Bus should handle concurrent publishes safely."""
        import threading

        received = []
        lock = threading.Lock()

        def handler(event):
            with lock:
                received.append(event)

        bus.subscribe("OrderPlaced", handler)

        threads = []
        for _ in range(10):
            t = threading.Thread(target=lambda: bus.publish(sample_event))
            threads.append(t)
            t.start()

        for t in threads:
            t.join()

        assert len(received) == 10
