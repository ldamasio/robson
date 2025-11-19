# Multi-Timeframe Analysis - Technical Specification

## Module: `apps/backend/core/domain/analysis.py`

## Overview

Technical specification for multi-timeframe analysis feature. Implements simultaneous analysis of multiple timeframes with configurable weights and signal consolidation.

This implements requirement: `docs/requirements/example-multi-timeframe-analysis.md`

---

## Dependencies

### Internal
- `apps.backend.core.domain.signal` - Signal entity
- `apps.backend.core.domain.timeframe` - Timeframe value object
- `apps.backend.core.application.ports` - MarketDataRepository port
- `apps.backend.monolith.api.models.user` - User model (for tenant)

### External
- `python-binance` - Exchange data fetching
- `pandas` - Time-series analysis (optional)
- `numpy` - Mathematical operations

### Database
- Read market data via MarketDataRepository port
- Cache results in Redis (via CachePort)

---

## Domain Entities

### Value Object: `Timeframe`

**Responsibility**: Represent valid timeframe periods

```python
from enum import Enum

class Timeframe(str, Enum):
    """Valid timeframe periods."""
    ONE_MINUTE = "1m"
    FIVE_MINUTES = "5m"
    FIFTEEN_MINUTES = "15m"
    ONE_HOUR = "1h"
    FOUR_HOURS = "4h"
    ONE_DAY = "1d"

    @property
    def minutes(self) -> int:
        """Convert timeframe to minutes."""
        mapping = {
            "1m": 1,
            "5m": 5,
            "15m": 15,
            "1h": 60,
            "4h": 240,
            "1d": 1440
        }
        return mapping[self.value]
```

### Entity: `TimeframeSignal`

**Responsibility**: Signal for single timeframe

**Attributes**:
- `timeframe: Timeframe` - The timeframe period
- `signal_type: str` - "BUY", "SELL", or "HOLD"
- `strength: Decimal` - Signal strength (0.0 to 1.0)
- `confidence: Decimal` - Confidence level (0.0 to 1.0)
- `timestamp: datetime` - When signal was generated

```python
from dataclasses import dataclass
from decimal import Decimal
from datetime import datetime

@dataclass(frozen=True)
class TimeframeSignal:
    """Signal generated for specific timeframe."""
    timeframe: Timeframe
    signal_type: str  # "BUY", "SELL", "HOLD"
    strength: Decimal  # 0.0 to 1.0
    confidence: Decimal  # 0.0 to 1.0
    timestamp: datetime

    def __post_init__(self):
        """Validate signal values."""
        if self.signal_type not in ("BUY", "SELL", "HOLD"):
            raise ValueError(f"Invalid signal type: {self.signal_type}")
        if not (0 <= self.strength <= 1):
            raise ValueError(f"Strength must be 0-1: {self.strength}")
        if not (0 <= self.confidence <= 1):
            raise ValueError(f"Confidence must be 0-1: {self.confidence}")
```

### Entity: `MultiTimeframeAnalysis`

**Responsibility**: Consolidated analysis across timeframes

**Attributes**:
- `symbol: str` - Trading symbol (e.g., "BTCUSDT")
- `signals: List[TimeframeSignal]` - Individual timeframe signals
- `consolidated_score: int` - Final score (0-100)
- `recommendation: str` - "BUY", "SELL", or "HOLD"
- `analyzed_at: datetime` - Analysis timestamp

```python
@dataclass(frozen=True)
class MultiTimeframeAnalysis:
    """Multi-timeframe analysis result."""
    symbol: str
    signals: List[TimeframeSignal]
    consolidated_score: int  # 0-100
    recommendation: str  # "BUY", "SELL", "HOLD"
    analyzed_at: datetime

    def __post_init__(self):
        """Validate analysis."""
        if not (0 <= self.consolidated_score <= 100):
            raise ValueError(f"Score must be 0-100: {self.consolidated_score}")
        if self.recommendation not in ("BUY", "SELL", "HOLD"):
            raise ValueError(f"Invalid recommendation: {self.recommendation}")
```

---

## Use Case: `AnalyzeMultiTimeframeUseCase`

**Responsibility**: Execute multi-timeframe analysis

### Port Definitions

```python
from typing import Protocol, List
from decimal import Decimal

class MarketDataRepository(Protocol):
    """Port for market data access."""

    def fetch_ohlcv(
        self,
        symbol: str,
        timeframe: Timeframe,
        limit: int,
        user_id: str
    ) -> List[dict]:
        """Fetch OHLCV data for symbol and timeframe.

        CRITICAL: Must filter by user_id for multi-tenant isolation.
        """
        ...

class CachePort(Protocol):
    """Port for caching results."""

    async def get(self, key: str) -> Optional[str]: ...
    async def set(self, key: str, value: str, ttl: int): ...
```

### Use Case Implementation

```python
class AnalyzeMultiTimeframeUseCase:
    """Analyze multiple timeframes and consolidate signals."""

    def __init__(
        self,
        market_data_repo: MarketDataRepository,
        cache: CachePort,
        default_weights: Optional[Dict[Timeframe, Decimal]] = None
    ):
        self._market_data = market_data_repo
        self._cache = cache
        self._default_weights = default_weights or self._get_default_weights()

    def execute(
        self,
        symbol: str,
        timeframes: List[Timeframe],
        user_id: str,
        weights: Optional[Dict[Timeframe, Decimal]] = None
    ) -> MultiTimeframeAnalysis:
        """Execute multi-timeframe analysis.

        Args:
            symbol: Trading symbol (e.g., "BTCUSDT")
            timeframes: List of timeframes to analyze
            user_id: User ID (for multi-tenant isolation)
            weights: Optional custom weights per timeframe

        Returns:
            Multi-timeframe analysis with consolidated score

        Raises:
            ValueError: If timeframes empty or invalid weights
            InsufficientDataError: If market data insufficient
        """
        # 1. Validate inputs
        if not timeframes:
            raise ValueError("At least one timeframe required")

        weights = weights or self._default_weights

        # 2. Check cache
        cache_key = f"mtf:{user_id}:{symbol}:{','.join(t.value for t in timeframes)}"
        cached = await self._cache.get(cache_key)
        if cached:
            return self._deserialize(cached)

        # 3. Analyze each timeframe (can be parallelized)
        signals = []
        for tf in timeframes:
            signal = self._analyze_single_timeframe(symbol, tf, user_id)
            signals.append(signal)

        # 4. Consolidate signals
        score = self._consolidate_signals(signals, weights)
        recommendation = self._score_to_recommendation(score)

        # 5. Create result
        analysis = MultiTimeframeAnalysis(
            symbol=symbol,
            signals=signals,
            consolidated_score=score,
            recommendation=recommendation,
            analyzed_at=datetime.now()
        )

        # 6. Cache result
        await self._cache.set(
            cache_key,
            self._serialize(analysis),
            ttl=60  # 1 minute TTL
        )

        return analysis

    def _analyze_single_timeframe(
        self,
        symbol: str,
        timeframe: Timeframe,
        user_id: str
    ) -> TimeframeSignal:
        """Analyze single timeframe.

        Logic:
        1. Fetch OHLCV data (last 100 periods)
        2. Calculate technical indicators (SMA, RSI, MACD)
        3. Generate signal based on indicators
        4. Calculate confidence based on indicator agreement
        """
        # Fetch data (CRITICAL: filtered by user_id)
        ohlcv = self._market_data.fetch_ohlcv(
            symbol, timeframe, limit=100, user_id=user_id
        )

        if len(ohlcv) < 50:
            raise InsufficientDataError(
                f"Need at least 50 periods, got {len(ohlcv)}"
            )

        # Calculate indicators (simplified)
        sma_signal = self._calculate_sma_signal(ohlcv)
        rsi_signal = self._calculate_rsi_signal(ohlcv)
        macd_signal = self._calculate_macd_signal(ohlcv)

        # Determine signal type
        signals = [sma_signal, rsi_signal, macd_signal]
        buy_votes = sum(1 for s in signals if s == "BUY")
        sell_votes = sum(1 for s in signals if s == "SELL")

        if buy_votes >= 2:
            signal_type = "BUY"
            strength = Decimal(buy_votes) / Decimal(3)
        elif sell_votes >= 2:
            signal_type = "SELL"
            strength = Decimal(sell_votes) / Decimal(3)
        else:
            signal_type = "HOLD"
            strength = Decimal("0.5")

        # Calculate confidence (all indicators agree = high confidence)
        confidence = Decimal("1.0") if len(set(signals)) == 1 else Decimal("0.66")

        return TimeframeSignal(
            timeframe=timeframe,
            signal_type=signal_type,
            strength=strength,
            confidence=confidence,
            timestamp=datetime.now()
        )

    def _consolidate_signals(
        self,
        signals: List[TimeframeSignal],
        weights: Dict[Timeframe, Decimal]
    ) -> int:
        """Consolidate signals into single score (0-100).

        Logic:
        1. For each signal, calculate weighted score
        2. BUY signals contribute positive (0-100)
        3. SELL signals contribute negative (0-100)
        4. HOLD signals are neutral (50)
        5. Final score = weighted average, clamped to 0-100
        """
        total_weight = Decimal(0)
        weighted_sum = Decimal(0)

        for signal in signals:
            weight = weights.get(signal.timeframe, Decimal("1.0"))

            if signal.signal_type == "BUY":
                # BUY: 50 + (strength * 50) = 50-100
                score = 50 + (signal.strength * 50)
            elif signal.signal_type == "SELL":
                # SELL: 50 - (strength * 50) = 0-50
                score = 50 - (signal.strength * 50)
            else:  # HOLD
                score = Decimal(50)

            # Apply confidence as multiplier to weight
            effective_weight = weight * signal.confidence

            weighted_sum += score * effective_weight
            total_weight += effective_weight

        # Calculate weighted average
        if total_weight == 0:
            return 50  # Neutral if no weighted signals

        final_score = int(weighted_sum / total_weight)

        # Clamp to 0-100
        return max(0, min(100, final_score))

    def _score_to_recommendation(self, score: int) -> str:
        """Convert score to recommendation."""
        if score >= 60:
            return "BUY"
        elif score <= 40:
            return "SELL"
        else:
            return "HOLD"

    @staticmethod
    def _get_default_weights() -> Dict[Timeframe, Decimal]:
        """Get default weights (larger timeframes = more weight)."""
        return {
            Timeframe.ONE_MINUTE: Decimal("0.5"),
            Timeframe.FIVE_MINUTES: Decimal("0.75"),
            Timeframe.FIFTEEN_MINUTES: Decimal("1.0"),
            Timeframe.ONE_HOUR: Decimal("1.5"),
            Timeframe.FOUR_HOURS: Decimal("2.0"),
            Timeframe.ONE_DAY: Decimal("2.5")
        }
```

---

## REST API Endpoint

### Endpoint: `POST /api/v1/analysis/multi-timeframe`

**Django View** (`apps/backend/monolith/api/views/analysis_views.py`):

```python
from rest_framework.decorators import api_view, permission_classes
from rest_framework.permissions import IsAuthenticated
from rest_framework.response import Response
from rest_framework import status

@api_view(['POST'])
@permission_classes([IsAuthenticated])
def multi_timeframe_analysis(request):
    """Multi-timeframe analysis endpoint.

    Request body:
        symbol (str): Trading symbol (e.g., "BTCUSDT")
        timeframes (list): List of timeframes (e.g., ["1h", "4h", "1d"])
        weights (dict, optional): Custom weights per timeframe

    Returns:
        200: Analysis result
        400: Invalid input
        429: Rate limit exceeded
    """
    # 1. Validate input
    serializer = MultiTimeframeAnalysisSerializer(data=request.data)
    serializer.is_valid(raise_exception=True)

    # 2. Check rate limit (via middleware or decorator)
    # ...

    # 3. Execute use case
    use_case = AnalyzeMultiTimeframeUseCase(
        market_data_repo=DjangoMarketDataRepository(),
        cache=RedisCacheAdapter()
    )

    result = use_case.execute(
        symbol=serializer.validated_data['symbol'],
        timeframes=serializer.validated_data['timeframes'],
        user_id=request.user.id,
        weights=serializer.validated_data.get('weights')
    )

    # 4. Serialize response
    response_serializer = MultiTimeframeAnalysisResponseSerializer(result)
    return Response(response_serializer.data)
```

---

## Test Cases

### Unit Test: Valid Analysis
```python
@pytest.mark.django_db
def test_multi_timeframe_analysis_valid():
    """Test multi-timeframe analysis with valid inputs."""
    # Arrange
    user = User.objects.create(username="trader1")
    use_case = AnalyzeMultiTimeframeUseCase(
        market_data_repo=MockMarketDataRepository(),
        cache=MockCache()
    )

    # Act
    result = use_case.execute(
        symbol="BTCUSDT",
        timeframes=[Timeframe.ONE_HOUR, Timeframe.FOUR_HOURS],
        user_id=user.id
    )

    # Assert
    assert result.symbol == "BTCUSDT"
    assert len(result.signals) == 2
    assert 0 <= result.consolidated_score <= 100
    assert result.recommendation in ("BUY", "SELL", "HOLD")
```

### Integration Test: Multi-Tenant Isolation
```python
@pytest.mark.django_db
def test_multi_timeframe_tenant_isolation():
    """Test that user B cannot access user A's analysis."""
    # Arrange
    user_a = User.objects.create(username="trader_a")
    user_b = User.objects.create(username="trader_b")

    # User A creates analysis (cached)
    use_case = AnalyzeMultiTimeframeUseCase(...)
    analysis_a = use_case.execute(..., user_id=user_a.id)

    # Act: User B tries to access
    analysis_b = use_case.execute(..., user_id=user_b.id)

    # Assert: Should get fresh analysis, not cached from user A
    assert analysis_a != analysis_b
```

---

## Performance Considerations

- **Caching**: 1-minute TTL for analysis results
- **Parallelization**: Analyze timeframes concurrently (future optimization)
- **Database**: Index on (symbol, timeframe, timestamp)
- **Rate Limiting**: 10 req/min (free), 100 req/min (premium)

## Security Considerations

- **Multi-Tenant**: Always filter by user_id
- **Input Validation**: Validate timeframes and weights
- **Rate Limiting**: Prevent abuse

---

**Status**: Ready for implementation
**Estimated Complexity**: Moderate
**Implementation Mode**: Autonomous (spec is complete)
