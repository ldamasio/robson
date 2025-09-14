from __future__ import annotations
from core.application.ports import EventBusPort


class NoopEventBus(EventBusPort):
    def publish(self, topic: str, payload: dict) -> None:
        # Intentionally do nothing; replace with RabbitMQ/Kafka adapter later.
        return None

