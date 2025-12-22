from decimal import Decimal
from typing import TypedDict

from django.core.cache import cache
from django.utils import timezone

from api.application.adapters import BinanceMarketData


class CachedQuotes(TypedDict):
    bid: Decimal
    ask: Decimal
    timestamp: int


CACHE_TTL_SECONDS = 1


def _cache_key(symbol: str, timestamp: int) -> str:
    return f"market_price:{symbol}:{timestamp}"


def get_cached_quotes(symbol: str) -> CachedQuotes:
    normalized_symbol = symbol.upper()
    timestamp = int(timezone.now().timestamp())
    key = _cache_key(normalized_symbol, timestamp)
    cached = cache.get(key)
    if cached:
        return cached

    market_data = BinanceMarketData()
    order_book = market_data.client.get_order_book(symbol=normalized_symbol, limit=5)
    bid = Decimal(str(order_book["bids"][0][0]))
    ask = Decimal(str(order_book["asks"][0][0]))

    payload: CachedQuotes = {
        "bid": bid,
        "ask": ask,
        "timestamp": timestamp,
    }
    cache.set(key, payload, timeout=CACHE_TTL_SECONDS)
    return payload


def get_cached_bid(symbol: str) -> Decimal:
    return get_cached_quotes(symbol)["bid"]


def get_cached_ask(symbol: str) -> Decimal:
    return get_cached_quotes(symbol)["ask"]
