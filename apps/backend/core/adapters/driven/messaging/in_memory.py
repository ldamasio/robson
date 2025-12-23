"""
In-Memory Message Bus Implementation

This adapter is for development and testing only.
It provides synchronous, in-process message delivery.

For production, use RabbitMQMessageBus instead.

Characteristics:
- Synchronous (blocks until all handlers complete)
- No persistence (messages lost if process crashes)
- No durability (subscribers must be registered before publish)
- Thread-safe (uses locks)
- Simple and fast for tests

Usage:
    bus = InMemoryMessageBus()
    bus.subscribe("OrderPlaced", handle_order)
    bus.publish(OrderPlacedEvent(...))
"""

from typing import Callable, Dict, List, Optional
from threading import Lock
import logging

from apps.backend.core.application.ports import DomainEvent


logger = logging.getLogger(__name__)


class InMemoryMessageBus:
    """
    In-memory message bus for development and testing.

    Implements MessageBusPort.
    """

    def __init__(self):
        self._handlers: Dict[str, List[Callable[[DomainEvent], None]]] = {}
        self._lock = Lock()
        logger.info("InMemoryMessageBus initialized")

    def publish(
        self,
        event: DomainEvent,
        routing_key: Optional[str] = None,
    ) -> None:
        """
        Publish event to all registered handlers.

        Args:
            event: Domain event to publish
            routing_key: Ignored in this implementation (no routing logic)

        Note:
            Handlers are called synchronously in registration order.
            If a handler raises an exception, it is logged but other handlers still run.
        """
        event_type = event.event_type

        with self._lock:
            handlers = self._handlers.get(event_type, [])

        if not handlers:
            logger.debug(f"No handlers registered for event type: {event_type}")
            return

        logger.info(f"Publishing event {event_type} (ID: {event.event_id}) to {len(handlers)} handler(s)")

        for handler in handlers:
            try:
                handler(event)
            except Exception as e:
                logger.error(
                    f"Handler {handler.__name__} failed for event {event_type} (ID: {event.event_id}): {e}",
                    exc_info=True,
                )
                # Continue to next handler (don't let one failure stop others)

        logger.debug(f"Event {event_type} (ID: {event.event_id}) published successfully")

    def subscribe(
        self,
        event_type: str,
        handler: Callable[[DomainEvent], None],
        routing_pattern: Optional[str] = None,
    ) -> None:
        """
        Subscribe to events of a specific type.

        Args:
            event_type: Type of event to subscribe to
            handler: Callback function
            routing_pattern: Ignored in this implementation

        Note:
            Handlers must be idempotent (may be called multiple times for same event).
        """
        with self._lock:
            if event_type not in self._handlers:
                self._handlers[event_type] = []

            self._handlers[event_type].append(handler)

        logger.info(f"Subscribed {handler.__name__} to event type: {event_type}")

    def unsubscribe(
        self,
        event_type: str,
        handler: Callable[[DomainEvent], None],
    ) -> None:
        """
        Unsubscribe handler from event type.

        Useful for testing cleanup.
        """
        with self._lock:
            if event_type in self._handlers:
                try:
                    self._handlers[event_type].remove(handler)
                    logger.info(f"Unsubscribed {handler.__name__} from event type: {event_type}")
                except ValueError:
                    logger.warning(f"Handler {handler.__name__} was not subscribed to {event_type}")

    def clear_all(self) -> None:
        """
        Clear all subscriptions.

        Useful for test cleanup.
        """
        with self._lock:
            self._handlers.clear()
        logger.info("All subscriptions cleared")

    def get_handler_count(self, event_type: str) -> int:
        """Get number of handlers for an event type (for testing)."""
        with self._lock:
            return len(self._handlers.get(event_type, []))
