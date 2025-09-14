from __future__ import annotations
from dataclasses import dataclass
from decimal import Decimal
from datetime import datetime


@dataclass(frozen=True)
class Symbol:
    base: str
    quote: str

    def as_pair(self) -> str:
        return f"{self.base}{self.quote}".upper()


@dataclass
class Order:
    id: str
    symbol: Symbol
    side: str           # "BUY" | "SELL"
    qty: Decimal
    price: Decimal
    created_at: datetime

