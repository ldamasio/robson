from __future__ import annotations
from datetime import datetime, timezone
from core.application.ports import ClockPort


class RealClock(ClockPort):
    def now(self) -> datetime:
        return datetime.now(tz=timezone.utc)

