# Isolated Margin Trading - Technical Specification

**Document ID**: SPEC-MARGIN-001  
**Status**: Draft  
**Created**: 2024-12-23  
**Requirements**: REQ-FUT-MARGIN-001 through REQ-FUT-MARGIN-014  

---

## 1. Overview

This specification describes the technical implementation of Isolated Margin Trading in Robson Bot.

### 1.1 Scope

- Transfer operations between Spot and Isolated Margin
- Margin order execution (MARKET, LIMIT, STOP_LOSS)
- Position management and P&L tracking
- Risk management (margin monitoring, drawdown tracking)

### 1.2 Out of Scope

- Cross Margin trading
- Futures trading
- Multi-pair simultaneous positions (Phase 1)

---

## 2. Architecture

### 2.1 Hexagonal Architecture Integration

```
┌─────────────────────────────────────────────────────────────────────┐
│                           DRIVING ADAPTERS                          │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────────────────────┐ │
│  │  REST API   │  │   CLI       │  │   Django Management Cmd     │ │
│  └──────┬──────┘  └──────┬──────┘  └──────────────┬──────────────┘ │
└─────────┼────────────────┼────────────────────────┼─────────────────┘
          │                │                        │
          ▼                ▼                        ▼
┌─────────────────────────────────────────────────────────────────────┐
│                         APPLICATION CORE                            │
│  ┌─────────────────────────────────────────────────────────────────┐│
│  │                        USE CASES                                ││
│  │  ┌────────────────┐ ┌────────────────┐ ┌────────────────────┐  ││
│  │  │TransferToMargin│ │PlaceMarginOrder│ │CalculateMarginSize │  ││
│  │  └────────────────┘ └────────────────┘ └────────────────────┘  ││
│  │  ┌────────────────┐ ┌────────────────┐ ┌────────────────────┐  ││
│  │  │MonitorMarginLvl│ │ ClosePosition  │ │ TrackDrawdown      │  ││
│  │  └────────────────┘ └────────────────┘ └────────────────────┘  ││
│  └─────────────────────────────────────────────────────────────────┘│
│  ┌─────────────────────────────────────────────────────────────────┐│
│  │                        DOMAIN                                   ││
│  │  MarginPosition │ MarginAccount │ PolicyState │ TradingIntent  ││
│  └─────────────────────────────────────────────────────────────────┘│
│  ┌─────────────────────────────────────────────────────────────────┐│
│  │                        PORTS                                    ││
│  │  MarginExecutionPort │ MarginAccountPort │ MessageBusPort      ││
│  └─────────────────────────────────────────────────────────────────┘│
└─────────────────────────────────────────────────────────────────────┘
          │                │                        │
          ▼                ▼                        ▼
┌─────────────────────────────────────────────────────────────────────┐
│                          DRIVEN ADAPTERS                            │
│  ┌─────────────────────┐  ┌─────────────────┐  ┌─────────────────┐ │
│  │ BinanceMarginAdapter│  │ DjangoMarginRepo│  │ EventBusAdapter │ │
│  └─────────────────────┘  └─────────────────┘  └─────────────────┘ │
└─────────────────────────────────────────────────────────────────────┘
          │
          ▼
    ┌───────────┐
    │  BINANCE  │
    │    API    │
    └───────────┘
```

### 2.2 Component Dependencies

```
margin_use_cases.py
    ├── imports: domain/margin.py (entities)
    ├── imports: application/ports.py (interfaces)
    ├── depends: MarginExecutionPort
    ├── depends: MarginAccountPort
    ├── depends: MessageBusPort
    └── depends: PolicyStateRepository

margin_adapters.py (driven)
    ├── imports: ports.py (implements interfaces)
    ├── imports: binance.client (external)
    └── imports: django.conf.settings
```

### 2.3 Stop Execution Model (Robson Monitor)

For Iron Exit Protocol and other Robson-monitored strategies:

- The stop price is stored on the **Operation** (Level 2).
- No STOP_LOSS_LIMIT order is pre-placed on Binance.
- The stop monitor evaluates price and executes a **market** close when triggered.
- `STOP_LOSS_PLACED` is an internal movement (no exchange order id).
- Slippage is acceptable; missed exits are not.

---

## 3. Domain Model

### 3.1 MarginPosition Entity

**Location**: `apps/backend/core/domain/margin.py`

**Architecture note**: In the monolith database, each isolated margin
position is linked to a single **Operation (Level 2)** for full traceability.

```python
"""
Isolated Margin Trading Domain Model

Pure business entities for margin trading.
No framework dependencies.
"""

from dataclasses import dataclass, field
from decimal import Decimal
from datetime import datetime
from typing import Optional
from enum import Enum


class MarginPositionStatus(str, Enum):
    """Status of a margin position."""
    PENDING = "PENDING"          # Order placed, not filled
    OPEN = "OPEN"                # Position is active
    CLOSING = "CLOSING"          # Close order placed
    CLOSED = "CLOSED"            # Position closed normally
    STOPPED_OUT = "STOPPED_OUT"  # Closed by stop-loss
    LIQUIDATED = "LIQUIDATED"    # Forced liquidation


class MarginSide(str, Enum):
    """Position side."""
    LONG = "LONG"    # Buy first, sell to close
    SHORT = "SHORT"  # Sell first, buy to close


@dataclass
class MarginPosition:
    """
    Represents an Isolated Margin trading position.
    
    This entity tracks a leveraged position from open to close,
    including all risk parameters and P&L calculations.
    
    Key Principle: Each position has isolated margin -
    risk is limited to the margin allocated for this position only.
    """
    position_id: str
    client_id: int
    symbol: str
    side: MarginSide
    status: MarginPositionStatus
    
    # Position details
    entry_price: Decimal
    quantity: Decimal
    leverage: int
    
    # Risk parameters (USER provides these)
    stop_price: Decimal
    target_price: Optional[Decimal] = None
    
    # Margin details
    margin_allocated: Decimal = Decimal("0")
    borrowed_amount: Decimal = Decimal("0")
    interest_accrued: Decimal = Decimal("0")
    
    # Calculated at entry
    position_value: Decimal = Decimal("0")
    risk_amount: Decimal = Decimal("0")
    risk_percent: Decimal = Decimal("0")
    
    # Current state
    current_price: Decimal = Decimal("0")
    margin_level: Decimal = Decimal("999")  # High = safe
    
    # P&L (updated real-time)
    unrealized_pnl: Decimal = Decimal("0")
    realized_pnl: Decimal = Decimal("0")
    fees_paid: Decimal = Decimal("0")
    
    # Order IDs (for tracking)
    entry_order_id: Optional[str] = None
    stop_order_id: Optional[str] = None
    target_order_id: Optional[str] = None
    close_order_id: Optional[str] = None
    
    # Binance references
    binance_entry_order_id: Optional[str] = None
    binance_stop_order_id: Optional[str] = None
    
    # Timestamps
    created_at: datetime = field(default_factory=datetime.utcnow)
    opened_at: Optional[datetime] = None
    closed_at: Optional[datetime] = None
    
    # Audit
    close_reason: Optional[str] = None
    
    def __post_init__(self):
        """Validate invariants."""
        if self.quantity <= 0:
            raise ValueError("Quantity must be positive")
        if self.entry_price <= 0:
            raise ValueError("Entry price must be positive")
        if self.leverage < 1:
            raise ValueError("Leverage must be at least 1")
        if self.leverage > 10:
            raise ValueError("Maximum leverage is 10x for safety")
        
        # Validate stop is on correct side
        if self.side == MarginSide.LONG and self.stop_price >= self.entry_price:
            raise ValueError("LONG stop must be below entry price")
        if self.side == MarginSide.SHORT and self.stop_price <= self.entry_price:
            raise ValueError("SHORT stop must be above entry price")
    
    @property
    def is_open(self) -> bool:
        """Check if position is currently open."""
        return self.status == MarginPositionStatus.OPEN
    
    @property
    def is_closed(self) -> bool:
        """Check if position is closed (any reason)."""
        return self.status in {
            MarginPositionStatus.CLOSED,
            MarginPositionStatus.STOPPED_OUT,
            MarginPositionStatus.LIQUIDATED,
        }
    
    @property
    def stop_distance(self) -> Decimal:
        """Distance from entry to stop (always positive)."""
        return abs(self.entry_price - self.stop_price)
    
    @property
    def stop_distance_percent(self) -> Decimal:
        """Stop distance as percentage of entry price."""
        if self.entry_price == 0:
            return Decimal("0")
        return (self.stop_distance / self.entry_price) * Decimal("100")
    
    @property
    def total_pnl(self) -> Decimal:
        """Total P&L including fees and interest."""
        return self.realized_pnl + self.unrealized_pnl - self.fees_paid - self.interest_accrued
    
    @property
    def is_profitable(self) -> bool:
        """Check if position is currently profitable."""
        return self.total_pnl > 0
    
    @property
    def is_at_risk(self) -> bool:
        """Check if margin level is in warning zone."""
        return self.margin_level < Decimal("1.3")
    
    @property
    def is_critical(self) -> bool:
        """Check if margin level is critical (near liquidation)."""
        return self.margin_level < Decimal("1.1")
    
    def update_price(self, current_price: Decimal) -> "MarginPosition":
        """
        Update current price and recalculate P&L.
        
        Returns new instance (immutable pattern).
        """
        if self.side == MarginSide.LONG:
            unrealized = (current_price - self.entry_price) * self.quantity
        else:  # SHORT
            unrealized = (self.entry_price - current_price) * self.quantity
        
        return MarginPosition(
            **{**self.__dict__, 
               "current_price": current_price,
               "unrealized_pnl": unrealized.quantize(Decimal("0.00000001"))}
        )
    
    def mark_as_open(
        self,
        timestamp: datetime,
        binance_order_id: str,
        fill_price: Decimal,
        fill_quantity: Decimal,
    ) -> "MarginPosition":
        """Mark position as opened with fill details."""
        return MarginPosition(
            **{**self.__dict__,
               "status": MarginPositionStatus.OPEN,
               "opened_at": timestamp,
               "binance_entry_order_id": binance_order_id,
               "entry_price": fill_price,
               "quantity": fill_quantity,
               "position_value": fill_price * fill_quantity}
        )
    
    def mark_as_stopped(self, timestamp: datetime, fill_price: Decimal) -> "MarginPosition":
        """Mark position as stopped out."""
        if self.side == MarginSide.LONG:
            realized = (fill_price - self.entry_price) * self.quantity
        else:
            realized = (self.entry_price - fill_price) * self.quantity
        
        return MarginPosition(
            **{**self.__dict__,
               "status": MarginPositionStatus.STOPPED_OUT,
               "closed_at": timestamp,
               "realized_pnl": realized.quantize(Decimal("0.00000001")),
               "unrealized_pnl": Decimal("0"),
               "close_reason": "Stop-loss triggered"}
        )
    
    def mark_as_closed(
        self,
        timestamp: datetime,
        fill_price: Decimal,
        reason: str = "Manual close",
    ) -> "MarginPosition":
        """Mark position as closed."""
        if self.side == MarginSide.LONG:
            realized = (fill_price - self.entry_price) * self.quantity
        else:
            realized = (self.entry_price - fill_price) * self.quantity
        
        return MarginPosition(
            **{**self.__dict__,
               "status": MarginPositionStatus.CLOSED,
               "closed_at": timestamp,
               "realized_pnl": realized.quantize(Decimal("0.00000001")),
               "unrealized_pnl": Decimal("0"),
               "close_reason": reason}
        )


@dataclass(frozen=True)
class MarginAccountInfo:
    """
    Snapshot of Isolated Margin account status for a symbol.
    
    Immutable value object retrieved from exchange.
    """
    symbol: str
    
    # Base asset (e.g., BTC)
    base_asset: str
    base_free: Decimal
    base_locked: Decimal
    base_borrowed: Decimal
    base_interest: Decimal
    
    # Quote asset (e.g., USDC)
    quote_asset: str
    quote_free: Decimal
    quote_locked: Decimal
    quote_borrowed: Decimal
    quote_interest: Decimal
    
    # Margin status
    margin_level: Decimal
    liquidation_price: Decimal
    
    # Trading enabled
    is_margin_trade_enabled: bool
    
    @property
    def total_base(self) -> Decimal:
        """Total base asset value."""
        return self.base_free + self.base_locked
    
    @property
    def total_quote(self) -> Decimal:
        """Total quote asset value."""
        return self.quote_free + self.quote_locked
    
    @property
    def available_quote(self) -> Decimal:
        """Available quote for new positions."""
        return self.quote_free
    
    @property
    def is_healthy(self) -> bool:
        """Check if margin level is healthy (> 1.5)."""
        return self.margin_level > Decimal("1.5")


@dataclass(frozen=True)
class TransferResult:
    """Result of a transfer operation."""
    success: bool
    transaction_id: Optional[str]
    asset: str
    amount: Decimal
    from_account: str
    to_account: str
    error_message: Optional[str] = None


@dataclass(frozen=True)
class MarginOrderResult:
    """Result of a margin order placement."""
    success: bool
    order_id: Optional[str]
    binance_order_id: Optional[str]
    symbol: str
    side: str
    order_type: str
    quantity: Decimal
    price: Optional[Decimal]
    filled_quantity: Decimal = Decimal("0")
    avg_fill_price: Optional[Decimal] = None
    status: str = "NEW"
    error_message: Optional[str] = None
```

---

## 4. Ports (Interfaces)

### 4.1 MarginExecutionPort

**Location**: `apps/backend/core/application/ports.py` (extend existing)

```python
class MarginExecutionPort(Protocol):
    """
    Port for Isolated Margin trading operations.
    
    Implementations:
    - BinanceMarginAdapter: Real execution on Binance
    - MockMarginAdapter: Paper trading / testing
    """
    
    def transfer_to_margin(
        self,
        symbol: str,
        asset: str,
        amount: Decimal,
    ) -> TransferResult:
        """
        Transfer asset from Spot to Isolated Margin.
        
        Args:
            symbol: Trading pair (e.g., "BTCUSDC")
            asset: Asset to transfer (e.g., "USDC")
            amount: Amount to transfer
            
        Returns:
            TransferResult with success/failure
        """
        ...
    
    def transfer_from_margin(
        self,
        symbol: str,
        asset: str,
        amount: Decimal,
    ) -> TransferResult:
        """
        Transfer asset from Isolated Margin back to Spot.
        
        Args:
            symbol: Trading pair (e.g., "BTCUSDC")
            asset: Asset to transfer (e.g., "USDC")
            amount: Amount to transfer
            
        Returns:
            TransferResult with success/failure
        """
        ...
    
    def get_margin_account(self, symbol: str) -> MarginAccountInfo:
        """
        Get Isolated Margin account info for a symbol.
        
        Args:
            symbol: Trading pair (e.g., "BTCUSDC")
            
        Returns:
            MarginAccountInfo with balances and margin level
        """
        ...
    
    def place_margin_order(
        self,
        symbol: str,
        side: str,
        order_type: str,
        quantity: Decimal,
        price: Optional[Decimal] = None,
        stop_price: Optional[Decimal] = None,
        side_effect_type: Optional[str] = None,
    ) -> MarginOrderResult:
        """
        Place an order on Isolated Margin account.
        
        Args:
            symbol: Trading pair
            side: "BUY" or "SELL"
            order_type: "MARKET", "LIMIT", "STOP_LOSS_LIMIT"
            quantity: Order quantity
            price: Limit price (for LIMIT and STOP_LOSS_LIMIT)
            stop_price: Trigger price (for STOP_LOSS_LIMIT)
            side_effect_type: "MARGIN_BUY", "AUTO_REPAY", or None
            
        Returns:
            MarginOrderResult with order details
        """
        ...
    
    def cancel_margin_order(
        self,
        symbol: str,
        order_id: str,
    ) -> bool:
        """
        Cancel an open Isolated Margin order.
        
        Args:
            symbol: Trading pair
            order_id: Order ID to cancel
            
        Returns:
            True if cancelled, False otherwise
        """
        ...
    
    def get_margin_level(self, symbol: str) -> Decimal:
        """
        Get current margin level for symbol.
        
        Returns:
            Margin level as Decimal (e.g., 1.5 = 150%)
        """
        ...
```

---

## 5. Adapters

### 5.1 BinanceMarginAdapter

**Location**: `apps/backend/monolith/api/application/margin_adapters.py`

```python
"""
Binance Isolated Margin Adapter

Implements MarginExecutionPort using Binance API.
"""

from __future__ import annotations
from decimal import Decimal
from typing import Optional
import logging

from django.conf import settings
from binance.client import Client
from binance.exceptions import BinanceAPIException

from apps.backend.core.domain.margin import (
    MarginAccountInfo,
    TransferResult,
    MarginOrderResult,
)
from apps.backend.core.application.ports import MarginExecutionPort

logger = logging.getLogger(__name__)


class BinanceMarginAdapter(MarginExecutionPort):
    """
    Binance Isolated Margin implementation.
    
    WARNING: This interacts with REAL money when BINANCE_USE_TESTNET=False.
    """
    
    def __init__(self, client: Client | None = None, use_testnet: bool = None):
        """Initialize adapter with Binance client."""
        if use_testnet is None:
            use_testnet = getattr(settings, 'BINANCE_USE_TESTNET', True)
        
        self.use_testnet = use_testnet
        
        if client:
            self.client = client
        else:
            if use_testnet:
                api_key = settings.BINANCE_API_KEY_TEST
                secret_key = settings.BINANCE_SECRET_KEY_TEST
            else:
                api_key = settings.BINANCE_API_KEY
                secret_key = settings.BINANCE_SECRET_KEY
            
            self.client = Client(api_key, secret_key, testnet=use_testnet)
        
        mode = "TESTNET" if use_testnet else "PRODUCTION"
        logger.info(f"BinanceMarginAdapter initialized in {mode} mode")
        
        if not use_testnet:
            logger.warning("⚠️ PRODUCTION mode - Real money operations!")
    
    def transfer_to_margin(
        self,
        symbol: str,
        asset: str,
        amount: Decimal,
    ) -> TransferResult:
        """Transfer from Spot to Isolated Margin."""
        try:
            response = self.client.transfer_spot_to_isolated_margin(
                asset=asset,
                symbol=symbol,
                amount=str(amount),
            )
            
            logger.info(f"Transfer to margin: {amount} {asset} for {symbol}")
            
            return TransferResult(
                success=True,
                transaction_id=str(response.get("tranId")),
                asset=asset,
                amount=amount,
                from_account="SPOT",
                to_account=f"ISOLATED_MARGIN:{symbol}",
            )
            
        except BinanceAPIException as e:
            logger.error(f"Transfer to margin failed: {e}")
            return TransferResult(
                success=False,
                transaction_id=None,
                asset=asset,
                amount=amount,
                from_account="SPOT",
                to_account=f"ISOLATED_MARGIN:{symbol}",
                error_message=str(e),
            )
    
    def transfer_from_margin(
        self,
        symbol: str,
        asset: str,
        amount: Decimal,
    ) -> TransferResult:
        """Transfer from Isolated Margin to Spot."""
        try:
            response = self.client.transfer_isolated_margin_to_spot(
                asset=asset,
                symbol=symbol,
                amount=str(amount),
            )
            
            logger.info(f"Transfer from margin: {amount} {asset} for {symbol}")
            
            return TransferResult(
                success=True,
                transaction_id=str(response.get("tranId")),
                asset=asset,
                amount=amount,
                from_account=f"ISOLATED_MARGIN:{symbol}",
                to_account="SPOT",
            )
            
        except BinanceAPIException as e:
            logger.error(f"Transfer from margin failed: {e}")
            return TransferResult(
                success=False,
                transaction_id=None,
                asset=asset,
                amount=amount,
                from_account=f"ISOLATED_MARGIN:{symbol}",
                to_account="SPOT",
                error_message=str(e),
            )
    
    def get_margin_account(self, symbol: str) -> MarginAccountInfo:
        """Get Isolated Margin account info."""
        response = self.client.get_isolated_margin_account()
        
        # Find the specific symbol
        for asset_info in response.get("assets", []):
            if asset_info.get("symbol") == symbol:
                base_asset = asset_info.get("baseAsset", {})
                quote_asset = asset_info.get("quoteAsset", {})
                
                return MarginAccountInfo(
                    symbol=symbol,
                    base_asset=base_asset.get("asset", ""),
                    base_free=Decimal(base_asset.get("free", "0")),
                    base_locked=Decimal(base_asset.get("locked", "0")),
                    base_borrowed=Decimal(base_asset.get("borrowed", "0")),
                    base_interest=Decimal(base_asset.get("interest", "0")),
                    quote_asset=quote_asset.get("asset", ""),
                    quote_free=Decimal(quote_asset.get("free", "0")),
                    quote_locked=Decimal(quote_asset.get("locked", "0")),
                    quote_borrowed=Decimal(quote_asset.get("borrowed", "0")),
                    quote_interest=Decimal(quote_asset.get("interest", "0")),
                    margin_level=Decimal(asset_info.get("marginLevel", "999")),
                    liquidation_price=Decimal(asset_info.get("liquidatePrice", "0")),
                    is_margin_trade_enabled=asset_info.get("marginRatio", "") != "",
                )
        
        raise ValueError(f"Symbol {symbol} not found in Isolated Margin account")
    
    def place_margin_order(
        self,
        symbol: str,
        side: str,
        order_type: str,
        quantity: Decimal,
        price: Optional[Decimal] = None,
        stop_price: Optional[Decimal] = None,
        side_effect_type: Optional[str] = None,
    ) -> MarginOrderResult:
        """Place Isolated Margin order."""
        try:
            params = {
                "symbol": symbol,
                "side": side,
                "type": order_type,
                "quantity": str(quantity),
                "isIsolated": "TRUE",
            }
            
            if price:
                params["price"] = str(price)
            
            if stop_price:
                params["stopPrice"] = str(stop_price)
            
            if order_type in ("LIMIT", "STOP_LOSS_LIMIT"):
                params["timeInForce"] = "GTC"
            
            if side_effect_type:
                params["sideEffectType"] = side_effect_type
            
            mode = "TESTNET" if self.use_testnet else "PRODUCTION"
            logger.info(f"Placing margin order on {mode}: {params}")
            
            response = self.client.create_margin_order(**params)
            
            return MarginOrderResult(
                success=True,
                order_id=str(response.get("clientOrderId")),
                binance_order_id=str(response.get("orderId")),
                symbol=symbol,
                side=side,
                order_type=order_type,
                quantity=quantity,
                price=price,
                filled_quantity=Decimal(response.get("executedQty", "0")),
                avg_fill_price=Decimal(response.get("price", "0")) if response.get("price") else None,
                status=response.get("status", "NEW"),
            )
            
        except BinanceAPIException as e:
            logger.error(f"Margin order failed: {e}")
            return MarginOrderResult(
                success=False,
                order_id=None,
                binance_order_id=None,
                symbol=symbol,
                side=side,
                order_type=order_type,
                quantity=quantity,
                price=price,
                error_message=str(e),
            )
    
    def cancel_margin_order(
        self,
        symbol: str,
        order_id: str,
    ) -> bool:
        """Cancel Isolated Margin order."""
        try:
            self.client.cancel_margin_order(
                symbol=symbol,
                orderId=order_id,
                isIsolated="TRUE",
            )
            logger.info(f"Cancelled margin order: {order_id}")
            return True
        except BinanceAPIException as e:
            logger.error(f"Cancel margin order failed: {e}")
            return False
    
    def get_margin_level(self, symbol: str) -> Decimal:
        """Get current margin level."""
        account = self.get_margin_account(symbol)
        return account.margin_level
```

---

## 6. Use Cases

### 6.1 OpenMarginPositionUseCase

**Location**: `apps/backend/core/application/margin_use_cases.py`

```python
"""
Isolated Margin Use Cases

Business logic for margin trading operations.
"""

from decimal import Decimal
from datetime import datetime
from typing import Optional
import uuid

from apps.backend.core.domain.margin import (
    MarginPosition,
    MarginPositionStatus,
    MarginSide,
    MarginAccountInfo,
)
from apps.backend.core.application.ports import (
    MarginExecutionPort,
    MessageBusPort,
    ClockPort,
    AuditTrailPort,
)


class OpenMarginPositionUseCase:
    """
    Open a new Isolated Margin position.
    
    Flow:
    1. Validate inputs (symbol, side, stop price)
    2. Get current margin account status
    3. Calculate position size using 1% risk rule
    4. Transfer required margin from Spot (if needed)
    5. Place market order
    6. Place stop-loss order
    7. Record position and audit trail
    """
    
    def __init__(
        self,
        margin_execution: MarginExecutionPort,
        message_bus: MessageBusPort,
        clock: ClockPort,
        audit_trail: AuditTrailPort,
    ):
        self._margin = margin_execution
        self._bus = message_bus
        self._clock = clock
        self._audit = audit_trail
    
    def execute(
        self,
        client_id: int,
        symbol: str,
        side: str,
        stop_price: Decimal,
        target_price: Optional[Decimal] = None,
        leverage: int = 3,
        total_capital: Decimal = None,
        max_risk_percent: Decimal = Decimal("1.0"),
    ) -> MarginPosition:
        """
        Open a margin position.
        
        Args:
            client_id: Client identifier
            symbol: Trading pair (e.g., "BTCUSDC")
            side: "LONG" or "SHORT"
            stop_price: Stop-loss price (user's technical level)
            target_price: Optional take-profit price
            leverage: Leverage to use (1-10, default 3)
            total_capital: Total capital for position sizing
            max_risk_percent: Max risk per trade (default 1%)
            
        Returns:
            Created MarginPosition
            
        Raises:
            ValueError: If inputs are invalid
            RuntimeError: If order placement fails
        """
        now = self._clock.now()
        position_id = f"pos-{uuid.uuid4()}"
        
        # Get current market price
        account = self._margin.get_margin_account(symbol)
        
        # TODO: Get current price from market data
        # For now, we'll require it to be passed or fetched
        
        # ... implementation continues ...
        
        self._audit.record(
            event_type="margin_position_opened",
            aggregate_id=position_id,
            data={
                "client_id": client_id,
                "symbol": symbol,
                "side": side,
                "leverage": leverage,
            },
            reason="User initiated margin position",
        )
        
        # Return position (implementation to be completed)
        pass
```

---

## 7. Database Models

### 7.1 MarginPosition Model

**Location**: `apps/backend/monolith/api/models/margin.py`

```python
"""
Django models for Isolated Margin Trading.
"""

from django.db import models
from decimal import Decimal


class MarginPosition(models.Model):
    """
    Database model for margin positions.
    
    Maps domain MarginPosition to Django ORM.
    """
    
    class Status(models.TextChoices):
        PENDING = "PENDING", "Pending"
        OPEN = "OPEN", "Open"
        CLOSING = "CLOSING", "Closing"
        CLOSED = "CLOSED", "Closed"
        STOPPED_OUT = "STOPPED_OUT", "Stopped Out"
        LIQUIDATED = "LIQUIDATED", "Liquidated"
    
    class Side(models.TextChoices):
        LONG = "LONG", "Long"
        SHORT = "SHORT", "Short"
    
    # Identifiers
    position_id = models.CharField(max_length=64, unique=True, db_index=True)
    client = models.ForeignKey(
        'clients.Client',
        on_delete=models.PROTECT,
        related_name='margin_positions',
    )
    
    # Symbol
    symbol = models.CharField(max_length=20, db_index=True)
    
    # Position details
    side = models.CharField(max_length=10, choices=Side.choices)
    status = models.CharField(max_length=20, choices=Status.choices, default=Status.PENDING)
    leverage = models.PositiveSmallIntegerField(default=3)
    
    # Prices
    entry_price = models.DecimalField(max_digits=20, decimal_places=8)
    stop_price = models.DecimalField(max_digits=20, decimal_places=8)
    target_price = models.DecimalField(max_digits=20, decimal_places=8, null=True, blank=True)
    current_price = models.DecimalField(max_digits=20, decimal_places=8, default=Decimal("0"))
    
    # Quantities
    quantity = models.DecimalField(max_digits=20, decimal_places=8)
    position_value = models.DecimalField(max_digits=20, decimal_places=8, default=Decimal("0"))
    
    # Margin
    margin_allocated = models.DecimalField(max_digits=20, decimal_places=8, default=Decimal("0"))
    borrowed_amount = models.DecimalField(max_digits=20, decimal_places=8, default=Decimal("0"))
    interest_accrued = models.DecimalField(max_digits=20, decimal_places=8, default=Decimal("0"))
    margin_level = models.DecimalField(max_digits=10, decimal_places=4, default=Decimal("999"))
    
    # Risk
    risk_amount = models.DecimalField(max_digits=20, decimal_places=8, default=Decimal("0"))
    risk_percent = models.DecimalField(max_digits=5, decimal_places=2, default=Decimal("0"))
    
    # P&L
    unrealized_pnl = models.DecimalField(max_digits=20, decimal_places=8, default=Decimal("0"))
    realized_pnl = models.DecimalField(max_digits=20, decimal_places=8, default=Decimal("0"))
    fees_paid = models.DecimalField(max_digits=20, decimal_places=8, default=Decimal("0"))
    
    # Binance references
    binance_entry_order_id = models.CharField(max_length=64, null=True, blank=True)
    binance_stop_order_id = models.CharField(max_length=64, null=True, blank=True)
    
    # Timestamps
    created_at = models.DateTimeField(auto_now_add=True)
    opened_at = models.DateTimeField(null=True, blank=True)
    closed_at = models.DateTimeField(null=True, blank=True)
    updated_at = models.DateTimeField(auto_now=True)
    
    # Audit
    close_reason = models.CharField(max_length=255, null=True, blank=True)
    
    class Meta:
        db_table = 'api_margin_position'
        ordering = ['-created_at']
        indexes = [
            models.Index(fields=['client', 'status']),
            models.Index(fields=['symbol', 'status']),
        ]
    
    def __str__(self):
        return f"{self.side} {self.symbol} @ {self.entry_price} ({self.status})"
    
    @property
    def total_pnl(self) -> Decimal:
        """Total P&L including fees."""
        return self.realized_pnl + self.unrealized_pnl - self.fees_paid - self.interest_accrued
    
    @property
    def is_profitable(self) -> bool:
        """Check if position is profitable."""
        return self.total_pnl > 0
```

---

## 8. Testing Strategy

### 8.1 Unit Tests (Domain)

```python
# apps/backend/core/tests/test_margin_domain.py

import pytest
from decimal import Decimal
from apps.backend.core.domain.margin import MarginPosition, MarginSide, MarginPositionStatus


def test_margin_position_long_stop_below_entry():
    """LONG stop must be below entry."""
    position = MarginPosition(
        position_id="test-1",
        client_id=1,
        symbol="BTCUSDC",
        side=MarginSide.LONG,
        status=MarginPositionStatus.PENDING,
        entry_price=Decimal("100000"),
        quantity=Decimal("0.001"),
        leverage=3,
        stop_price=Decimal("98000"),  # Below entry ✓
    )
    assert position.stop_distance == Decimal("2000")


def test_margin_position_long_stop_above_entry_fails():
    """LONG stop above entry should raise error."""
    with pytest.raises(ValueError, match="LONG stop must be below"):
        MarginPosition(
            position_id="test-2",
            client_id=1,
            symbol="BTCUSDC",
            side=MarginSide.LONG,
            status=MarginPositionStatus.PENDING,
            entry_price=Decimal("100000"),
            quantity=Decimal("0.001"),
            leverage=3,
            stop_price=Decimal("102000"),  # Above entry ✗
        )


def test_margin_position_pnl_calculation():
    """Test P&L calculation for LONG position."""
    position = MarginPosition(
        position_id="test-3",
        client_id=1,
        symbol="BTCUSDC",
        side=MarginSide.LONG,
        status=MarginPositionStatus.OPEN,
        entry_price=Decimal("100000"),
        quantity=Decimal("0.01"),
        leverage=3,
        stop_price=Decimal("98000"),
    )
    
    # Price goes up
    updated = position.update_price(Decimal("101000"))
    assert updated.unrealized_pnl == Decimal("10")  # 0.01 * 1000
    
    # Price goes down
    updated = position.update_price(Decimal("99000"))
    assert updated.unrealized_pnl == Decimal("-10")  # 0.01 * -1000
```

### 8.2 Integration Tests (Adapter)

```python
# apps/backend/monolith/api/tests/test_margin_adapter.py

import pytest
from decimal import Decimal
from unittest.mock import Mock, patch
from api.application.margin_adapters import BinanceMarginAdapter


@pytest.fixture
def mock_binance_client():
    """Create mock Binance client."""
    client = Mock()
    client.get_isolated_margin_account.return_value = {
        "assets": [{
            "symbol": "BTCUSDC",
            "baseAsset": {"asset": "BTC", "free": "0.01", "locked": "0", "borrowed": "0", "interest": "0"},
            "quoteAsset": {"asset": "USDC", "free": "1000", "locked": "0", "borrowed": "0", "interest": "0"},
            "marginLevel": "999",
            "liquidatePrice": "0",
        }]
    }
    return client


def test_get_margin_account(mock_binance_client):
    """Test getting margin account info."""
    adapter = BinanceMarginAdapter(client=mock_binance_client, use_testnet=True)
    
    account = adapter.get_margin_account("BTCUSDC")
    
    assert account.symbol == "BTCUSDC"
    assert account.quote_free == Decimal("1000")
    assert account.margin_level == Decimal("999")
```

---

## 9. Migration Plan

### Phase 1: Foundation
1. Create domain entities (`margin.py`)
2. Create ports (extend `ports.py`)
3. Create adapter (`margin_adapters.py`)
4. Create Django model and migration
5. Unit tests for domain

### Phase 2: Core Trading
6. Implement `OpenMarginPositionUseCase`
7. Implement `CloseMarginPositionUseCase`
8. Implement stop-loss placement
9. Integration tests

### Phase 3: Risk Management
10. Margin level monitoring
11. Drawdown tracking integration
12. Auto-pause on limits

### Phase 4: API
13. REST endpoints
14. CLI commands
15. End-to-end tests

---

## 10. Security Considerations

1. **API Credentials**: Require margin trading permissions on Binance API key
2. **Leverage Limits**: Cap at 10x maximum (configurable)
3. **Transfer Limits**: Validate transfers against available balance
4. **Audit Trail**: Log all margin operations
5. **Multi-tenant**: Strict client isolation

---

**Last Updated**: 2024-12-23  
**Version**: 1.0 (Draft)
