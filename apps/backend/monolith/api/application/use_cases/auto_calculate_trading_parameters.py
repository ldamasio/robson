"""
Auto-calculation of trading parameters from strategy configuration.

This module contains the use case for calculating trading parameters
including side, capital, technical stop, and position size.
"""

import logging
from decimal import Decimal, InvalidOperation

from ..ports import AccountBalancePort

logger = logging.getLogger(__name__)

# Confidence string to float mapping for persistence
CONFIDENCE_MAP = {
    "HIGH": Decimal("0.8"),
    "MEDIUM": Decimal("0.6"),
    "MED": Decimal("0.6"),
    "LOW": Decimal("0.4"),
}


def _quantize_quantity(quantity: Decimal) -> Decimal:
    """
    Quantize quantity to 8 decimal places (Binance standard).

    This ensures consistent quantity formatting across preview and persisted PLAN.

    Args:
        quantity: The quantity to quantize

    Returns:
        Quantized quantity with 8 decimal places
    """
    return quantity.quantize(Decimal("0.00000001"))


class AutoCalculateTradingParametersUseCase:
    """
    Use case for auto-calculating trading parameters from strategy configuration.

    This orchestrates:
    1. Determining trade side from strategy's market_bias or config
    2. Determining capital allocation from strategy config (FIXED or BALANCE mode)
    3. Calculating technical stop-loss using market data
    4. Calculating position size based on 1% risk rule

    Capital Modes:
    - FIXED: Uses strategy.config.capital_fixed value
    - BALANCE: Fetches available SPOT quote balance from exchange, applies strategy percentage

    Note on BALANCE mode:
    - Currently SPOT-only (does NOT support margin/isolated margin accounts)
    - Uses Symbol.quote_asset as the canonical source for quote asset
    - Uses available (free) balance, not total balance
    - Safe fallback to FIXED capital if balance fetch fails
    - Does NOT increase capital above available balance

    Used by both create_trading_intent and auto_calculate_parameters endpoints
    to avoid logic duplication.
    """

    def __init__(
        self,
        tech_stop_service,
        balance_provider: AccountBalancePort | None = None,
    ):
        """
        Initialize the use case.

        Args:
            tech_stop_service: BinanceTechnicalStopService instance
            balance_provider: Optional AccountBalancePort for BALANCE mode (SPOT only)
        """
        self.tech_stop_service = tech_stop_service
        self.balance_provider = balance_provider

    def execute(self, symbol_obj, strategy_obj, client_id: int | None = None) -> dict:
        """
        Calculate trading parameters from symbol and strategy.

        Args:
            symbol_obj: Symbol model instance (has .name, .quote_asset attributes)
            strategy_obj: Strategy model instance (has .market_bias, .get_config_value())
            client_id: Optional client ID for balance fetching in BALANCE mode

        Returns:
            Dictionary with calculated parameters:
            {
                "side": "BUY" | "SELL",
                "entry_price": Decimal,
                "stop_price": Decimal,
                "capital": Decimal,
                "capital_used": Decimal,  # Same as capital, for explicit tracking
                "capital_source": str,    # "FIXED" | "BALANCE" | "FALLBACK"
                "quantity": Decimal,      # Quantized to 8 decimal places
                "risk_amount": Decimal,
                "position_value": Decimal,
                "timeframe": str,
                "method_used": str,
                "confidence": str,        # "HIGH" | "MEDIUM" | "LOW"
                "confidence_float": Decimal,  # Mapped to 0.8/0.6/0.4 for persistence
                "side_source": str,
                "warnings": list[str],    # Combined balance + stop warnings
                "stop_result": TechnicalStopResult
            }

        Raises:
            TimeoutError: If Binance API calls exceed timeout
            Exception: If calculation fails critically

        Note:
            Balance fetch failures do NOT raise exceptions. They trigger fallback
            to safe default capital with a warning in the warnings list.
        """
        warnings = []

        # Determine side from Strategy.market_bias or config.default_side
        if hasattr(strategy_obj, "market_bias") and strategy_obj.market_bias:
            if strategy_obj.market_bias == "BULLISH":
                side = "BUY"
            elif strategy_obj.market_bias == "BEARISH":
                side = "SELL"
            else:  # NEUTRAL
                side = strategy_obj.get_config_value("default_side", "BUY")
            side_source = "strategy.market_bias"
        else:
            side = strategy_obj.get_config_value("default_side", "BUY")
            side_source = "strategy.config.default_side"

        # Determine capital from Strategy.config
        capital_mode = strategy_obj.get_config_value("capital_mode", "fixed").lower()
        capital_source = "UNKNOWN"
        capital = Decimal("0")

        # Safety limit for maximum capital
        MAX_CAPITAL = Decimal("100000.00")

        if capital_mode == "fixed":
            # FIXED mode: Use configured capital
            capital = Decimal(strategy_obj.get_config_value("capital_fixed", "1000.00"))
            capital_source = "FIXED"
            logger.info(f"Using FIXED capital mode: {capital}")

        elif capital_mode == "balance":
            # BALANCE mode: Fetch available SPOT quote balance from exchange
            if self.balance_provider is None:
                # Balance provider not configured - fall back safely
                capital = Decimal(strategy_obj.get_config_value("capital_fixed", "1000.00"))
                capital_source = "FALLBACK"
                warnings.append(
                    "Balance mode requested but balance provider not configured. "
                    "Using fixed capital fallback. "
                    "Contact administrator to enable balance fetching."
                )
                logger.warning(
                    f"Balance mode requested but balance_provider is None. "
                    f"Using fallback capital: {capital}"
                )
            else:
                # Use canonical quote asset from Symbol model
                quote_asset = symbol_obj.quote_asset

                # Get client_id from strategy if not provided
                if client_id is None and hasattr(strategy_obj, "client_id"):
                    client_id = strategy_obj.client_id

                if client_id is None:
                    # No client_id available - fall back safely
                    capital = Decimal(strategy_obj.get_config_value("capital_fixed", "1000.00"))
                    capital_source = "FALLBACK"
                    warnings.append(
                        "Balance mode requested but client_id not available. "
                        "Using fixed capital fallback."
                    )
                    logger.warning("Balance mode requested but client_id is None. Using fallback.")
                else:
                    account_type = strategy_obj.get_config_value("account_type", "spot")
                    if account_type not in ("spot", "isolated_margin"):
                        warnings.append(
                            f"Unknown account_type '{account_type}'. Falling back to SPOT."
                        )
                        account_type = "spot"

                    # Fetch balance from exchange with safe fallback
                    try:
                        available_balance = self.balance_provider.get_available_quote_balance(
                            client_id=client_id,
                            quote_asset=quote_asset,
                            account_type=account_type,
                            symbol=symbol_obj.name if account_type == "isolated_margin" else None,
                        )

                        # P0-1: If available balance is zero or negative, fall back
                        if available_balance <= 0:
                            capital = Decimal(
                                strategy_obj.get_config_value("capital_fixed", "1000.00")
                            )
                            capital_source = "FALLBACK"
                            warnings.append(
                                f"Available {quote_asset} balance is {available_balance}. "
                                f"Using fixed capital fallback."
                            )
                            logger.warning(
                                f"Available balance {available_balance} {quote_asset} is <= 0. "
                                f"Using fallback capital: {capital}"
                            )
                        else:
                            # Validate and parse capital_balance_percent
                            balance_percent = self._parse_and_validate_balance_percent(
                                strategy_obj, warnings
                            )
                            capital = available_balance * balance_percent

                            # P0-1: Apply MAX_CAPITAL cap only (do NOT increase capital above available)
                            if capital > MAX_CAPITAL:
                                warnings.append(
                                    f"Available balance ({available_balance} {quote_asset}) "
                                    f"results in capital ({capital}) above maximum ({MAX_CAPITAL}). "
                                    f"Using maximum capital instead for safety."
                                )
                                capital = MAX_CAPITAL

                            # P0-1: If capital is below typical minimum, warn but don't increase it
                            # (below 10 USDT may fail Binance minNotional filter at execution time)
                            if capital < Decimal("10.00"):
                                warnings.append(
                                    f"Computed capital (${capital}) is below typical exchange "
                                    f"minimum (minNotional ~$5-10). Execution may fail with "
                                    f"'Filter failure: MIN_NOTIONAL' error."
                                )

                            capital_source = "BALANCE"
                            logger.info(
                                f"Using BALANCE mode (SPOT): {available_balance} {quote_asset} available, "
                                f"{balance_percent * 100}% = {capital} capital"
                            )

                    except (TimeoutError, ConnectionError) as e:
                        # Exchange API timeout/connection error - fall back safely
                        capital = Decimal(strategy_obj.get_config_value("capital_fixed", "1000.00"))
                        capital_source = "FALLBACK"
                        error_type = "timeout" if isinstance(e, TimeoutError) else "connection"
                        warnings.append(
                            f"Exchange API {error_type} while fetching {quote_asset} balance. "
                            f"Using fixed capital fallback. "
                            f"Your intent was created successfully; balance retrieval will retry."
                        )
                        logger.warning(
                            f"Balance fetch failed ({error_type}): {e}. Using fallback capital: {capital}"
                        )
                    except Exception as e:
                        # Other API errors - fall back safely
                        capital = Decimal(strategy_obj.get_config_value("capital_fixed", "1000.00"))
                        capital_source = "FALLBACK"
                        # Don't expose detailed error messages to users (may contain sensitive info)
                        warnings.append(
                            f"Unable to fetch exchange balance. Using fixed capital fallback. "
                            f"Your intent was created successfully."
                        )
                        logger.error(f"Unexpected error fetching balance: {e}", exc_info=True)

        else:
            # Unknown capital mode - fall back to fixed
            capital = Decimal(strategy_obj.get_config_value("capital_fixed", "1000.00"))
            capital_source = "FALLBACK"
            warnings.append(
                f"Unknown capital mode '{capital_mode}'. Using fixed capital fallback. "
                f"Valid modes: 'fixed', 'balance'."
            )
            logger.warning(f"Unknown capital mode '{capital_mode}'. Using fallback.")

        # Get timeframe from strategy
        timeframe = strategy_obj.get_config_value("timeframe", "15m")

        # Calculate technical stop and position size
        result = self.tech_stop_service.calculate_position_with_technical_stop(
            symbol=symbol_obj.name,
            side=side,
            capital=capital,
            entry_price=None,  # Will fetch current price
            timeframe=timeframe,
            max_risk_percent=Decimal("1.0"),
        )

        # Extract and enrich result
        stop_result = result["stop_result"]

        # P0-3: Quantize quantity to 8 decimal places (unified with PLAN persistence)
        quantity = _quantize_quantity(result["quantity"])

        # P0-4: Map confidence string to float for persistence
        confidence_str = result["confidence"]
        confidence_float = self._map_confidence_to_float(confidence_str)

        # P0-2: Merge stop warnings into response warnings
        if hasattr(stop_result, "warnings") and stop_result.warnings:
            warnings.extend(stop_result.warnings)

        return {
            "side": side,
            "entry_price": stop_result.entry_price,
            "stop_price": stop_result.stop_price,
            "capital": capital,
            "capital_used": capital,  # Explicit tracking for audit trail
            "capital_source": capital_source,
            "quantity": quantity,  # P0-3: Quantized to 8 decimals
            "risk_amount": result["risk_amount"],
            "position_value": result["position_value"],
            "timeframe": timeframe,
            "method_used": result["method_used"],
            "confidence": confidence_str,
            "confidence_float": confidence_float,  # P0-4: For persistence
            "side_source": side_source,
            "warnings": warnings,  # P0-2: Combined balance + stop warnings
            "stop_result": stop_result,
        }

    def _parse_and_validate_balance_percent(self, strategy_obj, warnings: list) -> Decimal:
        """
        Parse and validate capital_balance_percent from strategy config.

        P0-1: Clamp to [0, 100] with warning when clamped.
        Invalid values fall back to 100% with warning.

        Args:
            strategy_obj: Strategy instance with get_config_value method
            warnings: List to append validation warnings to

        Returns:
            Validated balance percent as Decimal (0.0 to 1.0)
        """
        default_percent = "100"
        raw_value = strategy_obj.get_config_value("capital_balance_percent", default_percent)

        try:
            # Parse as Decimal
            percent = Decimal(str(raw_value))
        except (InvalidOperation, ValueError):
            warnings.append(
                f"Invalid capital_balance_percent value '{raw_value}'. "
                f"Using 100% of available balance."
            )
            return Decimal("1.0")  # 100%

        # Clamp to [0, 100] with warning
        if percent < 0:
            warnings.append(
                f"capital_balance_percent cannot be negative (got {percent}%). "
                f"Using 0% (no capital allocated)."
            )
            return Decimal("0.0")
        elif percent > 100:
            warnings.append(
                f"capital_balance_percent cannot exceed 100% (got {percent}%). Using 100%."
            )
            return Decimal("1.0")

        # Valid range - convert to decimal (0-1)
        return percent / Decimal("100")

    def _map_confidence_to_float(self, confidence_str: str) -> Decimal:
        """
        Map confidence string to float for persistence in TradingIntent.confidence.

        P0-4: HIGH -> 0.8, MEDIUM/MED -> 0.6, LOW -> 0.4

        Args:
            confidence_str: Confidence string ("HIGH", "MEDIUM", "MED", "LOW")

        Returns:
            Confidence as Decimal for persistence
        """
        # Normalize to uppercase
        key = confidence_str.upper() if confidence_str else "LOW"
        return CONFIDENCE_MAP.get(key, Decimal("0.4"))  # Default to LOW
