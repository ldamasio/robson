"""
Trading Intent use cases for the agentic workflow.

This module implements the business logic for creating and managing trading intents
in the PLAN → VALIDATE → EXECUTE workflow.

Hexagonal Architecture:
- No Django imports (framework-agnostic)
- Dependencies injected via ports
- Pure business logic
"""

from __future__ import annotations
from dataclasses import dataclass
from decimal import Decimal
from typing import Protocol
import uuid


@dataclass
class CreateTradingIntentCommand:
    """
    Command for creating a new trading intent.

    This represents the user's input in the manual entry flow.
    For auto-calculated intents, quantity can be provided to avoid recalculation drift.
    """
    symbol_id: int
    strategy_id: int
    side: str  # BUY or SELL
    entry_price: Decimal
    stop_price: Decimal
    capital: Decimal
    client_id: int
    target_price: Decimal | None = None
    regime: str = "unknown"
    confidence: float = 0.5
    reason: str = "Manual entry via UI"
    quantity: Decimal | None = None  # P0-3: Optional pre-calculated quantity (auto mode)


class SymbolRepository(Protocol):
    """Port for symbol data access."""

    def get_by_id(self, symbol_id: int, client_id: int) -> object:
        """Get symbol by ID for a specific client."""
        ...


class StrategyRepository(Protocol):
    """Port for strategy data access."""

    def get_by_id(self, strategy_id: int, client_id: int) -> object:
        """Get strategy by ID for a specific client."""
        ...


class TradingIntentRepository(Protocol):
    """Port for trading intent persistence."""

    def save(self, intent: dict) -> object:
        """Save a trading intent and return the persisted object."""
        ...

    def get_by_intent_id(self, intent_id: str, client_id: int) -> object:
        """Get trading intent by intent_id for a specific client."""
        ...

    def list_by_client(
        self,
        client_id: int,
        status: str | None = None,
        strategy_id: int | None = None,
        symbol_id: int | None = None,
        limit: int = 100,
        offset: int = 0
    ) -> list[object]:
        """List trading intents for a client with optional filters."""
        ...


class CreateTradingIntentUseCase:
    """
    Use case for creating a trading intent (PLAN step).

    This orchestrates:
    1. Load symbol and strategy from repository
    2. Calculate position size using 1% risk formula
    3. Calculate risk amount and risk percent
    4. Generate unique intent_id
    5. Create TradingIntent entity with status=PENDING
    6. Save to repository
    7. Return TradingIntent

    Position sizing formula:
        quantity = (capital × 1%) / |entry_price - stop_price|

    Risk calculations:
        risk_amount = capital × 1%
        risk_percent = (|entry_price - stop_price| / entry_price) × 100
    """

    def __init__(
        self,
        symbol_repo: SymbolRepository,
        strategy_repo: StrategyRepository,
        intent_repo: TradingIntentRepository,
    ):
        self.symbol_repo = symbol_repo
        self.strategy_repo = strategy_repo
        self.intent_repo = intent_repo

    def execute(self, command: CreateTradingIntentCommand) -> object:
        """
        Execute the create trading intent use case.

        Args:
            command: CreateTradingIntentCommand with all required parameters

        Returns:
            The persisted TradingIntent object

        Raises:
            ValueError: If parameters are invalid or entities not found
        """
        # Validate inputs
        self._validate_command(command)

        # Load required entities (will raise if not found)
        symbol = self.symbol_repo.get_by_id(command.symbol_id, command.client_id)
        strategy = self.strategy_repo.get_by_id(command.strategy_id, command.client_id)

        # Calculate position size and risk
        # P0-3: Use provided quantity if available (auto mode), otherwise calculate
        if command.quantity is not None:
            calculations = self._extract_risk_from_quantity(
                quantity=command.quantity,
                capital=command.capital,
                entry_price=command.entry_price,
                stop_price=command.stop_price,
            )
        else:
            calculations = self._calculate_position_and_risk(
                capital=command.capital,
                entry_price=command.entry_price,
                stop_price=command.stop_price,
                side=command.side,
            )

        # Generate unique intent_id
        intent_id = self._generate_intent_id()

        # Create intent data structure
        intent_data = {
            "intent_id": intent_id,
            "client_id": command.client_id,
            "symbol_id": command.symbol_id,
            "strategy_id": command.strategy_id,
            "side": command.side,
            "status": "PENDING",
            "quantity": calculations["quantity"],
            "entry_price": command.entry_price,
            "stop_price": command.stop_price,
            "target_price": command.target_price,
            "regime": command.regime,
            "confidence": command.confidence,
            "reason": command.reason,
            "capital": self._quantize_decimal(command.capital, decimal_places=8),
            "risk_amount": calculations["risk_amount"],
            "risk_percent": calculations["risk_percent"],
        }

        # Save to repository
        persisted_intent = self.intent_repo.save(intent_data)

        return persisted_intent

    def _validate_command(self, command: CreateTradingIntentCommand) -> None:
        """Validate command parameters."""
        if command.side not in ("BUY", "SELL"):
            raise ValueError(f"Invalid side: {command.side}. Must be BUY or SELL.")

        if command.capital <= 0:
            raise ValueError(f"Capital must be positive, got {command.capital}")

        if command.entry_price <= 0:
            raise ValueError(f"Entry price must be positive, got {command.entry_price}")

        if command.stop_price <= 0:
            raise ValueError(f"Stop price must be positive, got {command.stop_price}")

        if command.entry_price == command.stop_price:
            raise ValueError("Entry price and stop price cannot be equal")

        # Validate stop price direction
        if command.side == "BUY" and command.stop_price >= command.entry_price:
            raise ValueError("For BUY orders, stop price must be below entry price")

        if command.side == "SELL" and command.stop_price <= command.entry_price:
            raise ValueError("For SELL orders, stop price must be above entry price")

        if command.confidence < 0 or command.confidence > 1:
            raise ValueError(f"Confidence must be between 0 and 1, got {command.confidence}")

    def _calculate_position_and_risk(
        self,
        capital: Decimal,
        entry_price: Decimal,
        stop_price: Decimal,
        side: str,
    ) -> dict:
        """
        Calculate position size and risk metrics using 1% risk rule.

        Position size formula:
            quantity = (capital × 1%) / |entry_price - stop_price|

        Returns:
            dict with keys: quantity, risk_amount, risk_percent, position_value
        """
        # Calculate stop distance
        stop_distance = abs(entry_price - stop_price)

        # Risk amount (1% of capital)
        risk_percent_decimal = Decimal("0.01")  # 1%
        risk_amount = capital * risk_percent_decimal

        # Position size calculation
        quantity = risk_amount / stop_distance

        # Calculate risk as percentage of entry price
        risk_percent = (stop_distance / entry_price) * Decimal("100")

        # Position value (quantity * entry_price)
        position_value = quantity * entry_price

        # Quantize to match TradingIntent model constraints
        return {
            "quantity": self._quantize_decimal(quantity, decimal_places=8),
            "risk_amount": self._quantize_decimal(risk_amount, decimal_places=8),
            "risk_percent": self._quantize_decimal(risk_percent, decimal_places=2),
            "position_value": self._quantize_decimal(position_value, decimal_places=8),
        }

    def _extract_risk_from_quantity(
        self,
        quantity: Decimal,
        capital: Decimal,
        entry_price: Decimal,
        stop_price: Decimal,
    ) -> dict:
        """
        Extract risk metrics from a pre-calculated quantity (auto mode).

        P0-3: Used when quantity is already calculated by auto-calculation use case.
        Derives risk_amount and risk_percent from the known quantity.

        Args:
            quantity: Pre-calculated and quantized quantity
            capital: Capital amount
            entry_price: Entry price
            stop_price: Stop price

        Returns:
            dict with keys: quantity, risk_amount, risk_percent, position_value
        """
        # Calculate stop distance
        stop_distance = abs(entry_price - stop_price)

        # Risk amount from quantity and stop distance
        risk_amount = quantity * stop_distance

        # Calculate risk as percentage of entry price
        risk_percent = (stop_distance / entry_price) * Decimal("100")

        # Position value (quantity * entry_price)
        position_value = quantity * entry_price

        # Quantize to match TradingIntent model constraints
        return {
            "quantity": self._quantize_decimal(quantity, decimal_places=8),
            "risk_amount": self._quantize_decimal(risk_amount, decimal_places=8),
            "risk_percent": self._quantize_decimal(risk_percent, decimal_places=2),
            "position_value": self._quantize_decimal(position_value, decimal_places=8),
        }

    def _quantize_decimal(self, value: Decimal, decimal_places: int) -> Decimal:
        """
        Quantize a Decimal to the specified number of decimal places.

        This prevents ValidationError when saving to DecimalField with
        max_digits/decimal_places constraints.

        Args:
            value: The Decimal value to quantize
            decimal_places: Number of decimal places to round to

        Returns:
            Quantized Decimal value
        """
        quantizer = Decimal(10) ** -decimal_places
        return value.quantize(quantizer)

    def _generate_intent_id(self) -> str:
        """Generate a unique intent ID."""
        return str(uuid.uuid4())
