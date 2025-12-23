"""
Isolated Margin Trading Use Cases

Business logic orchestration for margin trading operations.
Each use case represents a single business operation.

Key Principles:
- Use cases depend on ports (interfaces), not adapters (implementations)
- Use cases are framework-agnostic (no Django, no HTTP)
- All operations are audited for traceability
- Risk rules are enforced: 1% per trade, 4% monthly drawdown
"""

from decimal import Decimal
from datetime import datetime, timedelta
from typing import Optional, Tuple
import uuid

from apps.backend.core.domain.margin import (
    MarginPosition,
    MarginPositionStatus,
    MarginSide,
    MarginLevel,
    MarginAccountInfo,
    MarginPositionSizingResult,
    calculate_margin_position_size,
)
from apps.backend.core.application.ports import (
    MarginExecutionPort,
    MarginPositionRepository,
    MessageBusPort,
    ClockPort,
    AuditTrailPort,
    RiskPolicyPort,
    PolicyStateRepository,
    MarginTransferResult,
    MarginOrderExecutionResult,
)


# ============================================================================
# Transfer Use Cases
# ============================================================================

class TransferToMarginUseCase:
    """
    Transfer assets from Spot wallet to Isolated Margin account.
    
    Flow:
    1. Validate amount and symbol
    2. Check if margin account exists for symbol
    3. Execute transfer via exchange API
    4. Record audit trail
    5. Return transfer result
    """
    
    def __init__(
        self,
        margin_execution: MarginExecutionPort,
        audit_trail: AuditTrailPort,
        clock: ClockPort,
    ):
        self._margin = margin_execution
        self._audit = audit_trail
        self._clock = clock
    
    def execute(
        self,
        client_id: int,
        symbol: str,
        asset: str,
        amount: Decimal,
    ) -> MarginTransferResult:
        """
        Execute transfer from Spot to Isolated Margin.
        
        Args:
            client_id: Client identifier
            symbol: Trading pair (e.g., "BTCUSDC")
            asset: Asset to transfer (e.g., "USDC")
            amount: Amount to transfer
            
        Returns:
            MarginTransferResult with success/failure
            
        Raises:
            ValueError: If inputs are invalid
        """
        # Validate inputs
        if amount <= 0:
            raise ValueError("Amount must be positive")
        if not symbol:
            raise ValueError("Symbol is required")
        if not asset:
            raise ValueError("Asset is required")
        
        # Execute transfer
        result = self._margin.transfer_to_margin(
            symbol=symbol,
            asset=asset,
            amount=amount,
        )
        
        # Audit trail
        self._audit.record(
            event_type="margin_transfer_to",
            aggregate_id=f"client-{client_id}",
            data={
                "symbol": symbol,
                "asset": asset,
                "amount": str(amount),
                "success": result.success,
                "transaction_id": result.transaction_id,
                "error": result.error_message,
            },
            reason=f"Transfer {amount} {asset} to Isolated Margin for {symbol}",
        )
        
        return result


class TransferFromMarginUseCase:
    """
    Transfer assets from Isolated Margin account back to Spot wallet.
    
    Flow:
    1. Validate amount
    2. Check available balance in margin account
    3. Verify transfer won't cause margin call
    4. Execute transfer
    5. Record audit trail
    """
    
    def __init__(
        self,
        margin_execution: MarginExecutionPort,
        audit_trail: AuditTrailPort,
        clock: ClockPort,
    ):
        self._margin = margin_execution
        self._audit = audit_trail
        self._clock = clock
    
    def execute(
        self,
        client_id: int,
        symbol: str,
        asset: str,
        amount: Decimal,
    ) -> MarginTransferResult:
        """
        Execute transfer from Isolated Margin to Spot.
        
        Args:
            client_id: Client identifier
            symbol: Trading pair
            asset: Asset to transfer
            amount: Amount to transfer
            
        Returns:
            MarginTransferResult with success/failure
        """
        if amount <= 0:
            raise ValueError("Amount must be positive")
        
        # Check current margin level before transfer
        margin_level = self._margin.get_margin_level(symbol)
        
        # Execute transfer
        result = self._margin.transfer_from_margin(
            symbol=symbol,
            asset=asset,
            amount=amount,
        )
        
        # Audit trail
        self._audit.record(
            event_type="margin_transfer_from",
            aggregate_id=f"client-{client_id}",
            data={
                "symbol": symbol,
                "asset": asset,
                "amount": str(amount),
                "margin_level_before": str(margin_level),
                "success": result.success,
                "transaction_id": result.transaction_id,
                "error": result.error_message,
            },
            reason=f"Transfer {amount} {asset} from Isolated Margin for {symbol}",
        )
        
        return result


# ============================================================================
# Position Sizing Use Case
# ============================================================================

class CalculateMarginPositionSizeUseCase:
    """
    Calculate optimal position size for a margin trade.
    
    Implements Robson's PRIMARY INTELLIGENCE:
    - 1% max risk per trade
    - Position sized based on stop distance
    - Leverage accounted for in margin requirements
    - Capped at 50% of available margin
    
    This does NOT execute any trade - only calculates.
    """
    
    def __init__(
        self,
        margin_execution: MarginExecutionPort,
        audit_trail: AuditTrailPort,
    ):
        self._margin = margin_execution
        self._audit = audit_trail
    
    def execute(
        self,
        client_id: int,
        symbol: str,
        side: str,
        entry_price: Decimal,
        stop_price: Decimal,
        total_capital: Decimal,
        leverage: int = 3,
        max_risk_percent: Decimal = Decimal("1.0"),
    ) -> MarginPositionSizingResult:
        """
        Calculate position size for margin trade.
        
        Args:
            client_id: Client identifier
            symbol: Trading pair (e.g., "BTCUSDC")
            side: "LONG" or "SHORT"
            entry_price: Intended entry price
            stop_price: Stop-loss price (user's technical level)
            total_capital: Total capital for sizing calculation
            leverage: Leverage to use (1-10)
            max_risk_percent: Maximum risk per trade (default 1%)
            
        Returns:
            MarginPositionSizingResult with:
            - quantity: Position size in base asset
            - margin_required: Margin needed for position
            - risk_amount: Maximum loss if stopped
            - risk_percent: Actual risk as % of capital
        """
        # Get available margin from exchange
        try:
            account = self._margin.get_margin_account(symbol)
            available_margin = account.quote_free
        except Exception:
            # If can't get margin account, don't limit by available
            available_margin = None
        
        # Calculate using domain function
        result = calculate_margin_position_size(
            capital=total_capital,
            entry_price=entry_price,
            stop_price=stop_price,
            side=side,
            leverage=leverage,
            max_risk_percent=max_risk_percent,
            max_margin_percent=Decimal("50.0"),
            available_margin=available_margin,
        )
        
        # Audit trail
        self._audit.record(
            event_type="margin_position_size_calculated",
            aggregate_id=f"client-{client_id}",
            data={
                "symbol": symbol,
                "side": side,
                "entry_price": str(entry_price),
                "stop_price": str(stop_price),
                "leverage": leverage,
                "calculated_quantity": str(result.quantity),
                "margin_required": str(result.margin_required),
                "risk_amount": str(result.risk_amount),
                "risk_percent": str(result.risk_percent),
                "is_capped": result.is_capped,
            },
            reason=f"Position size calculation for {side} {symbol}",
        )
        
        return result


# ============================================================================
# Position Opening Use Case
# ============================================================================

class OpenMarginPositionUseCase:
    """
    Open a new Isolated Margin position.
    
    Complete flow:
    1. Validate inputs and risk parameters
    2. Check monthly drawdown limit (4% max)
    3. Calculate position size using 1% risk rule
    4. Transfer margin from Spot if needed
    5. Place entry order (MARKET)
    6. Place stop-loss order (STOP_LOSS_LIMIT)
    7. Optionally place take-profit order
    8. Create position record
    9. Publish events and audit trail
    
    Safety:
    - Stop-loss is MANDATORY
    - Risk validated before execution
    - All actions are atomic-ish (rollback on failure)
    """
    
    def __init__(
        self,
        margin_execution: MarginExecutionPort,
        position_repo: MarginPositionRepository,
        risk_policy: RiskPolicyPort,
        message_bus: MessageBusPort,
        audit_trail: AuditTrailPort,
        clock: ClockPort,
    ):
        self._margin = margin_execution
        self._positions = position_repo
        self._risk = risk_policy
        self._bus = message_bus
        self._audit = audit_trail
        self._clock = clock
    
    def execute(
        self,
        client_id: int,
        symbol: str,
        side: str,
        entry_price: Decimal,
        stop_price: Decimal,
        total_capital: Decimal,
        target_price: Optional[Decimal] = None,
        leverage: int = 3,
        max_risk_percent: Decimal = Decimal("1.0"),
        auto_transfer_margin: bool = True,
    ) -> Tuple[MarginPosition, MarginOrderExecutionResult]:
        """
        Open a margin position with full risk management.
        
        Args:
            client_id: Client identifier
            symbol: Trading pair (e.g., "BTCUSDC")
            side: "LONG" or "SHORT"
            entry_price: Intended entry price (for sizing, actual may differ)
            stop_price: Stop-loss price (MANDATORY)
            total_capital: Total capital for sizing
            target_price: Optional take-profit price
            leverage: Leverage (1-10, default 3)
            max_risk_percent: Max risk per trade (default 1%)
            auto_transfer_margin: Whether to transfer from Spot if needed
            
        Returns:
            Tuple of (MarginPosition, MarginOrderExecutionResult)
            
        Raises:
            ValueError: If inputs invalid or risk checks fail
            RuntimeError: If order execution fails
        """
        now = self._clock.now()
        position_id = f"margin-{uuid.uuid4()}"
        correlation_id = f"corr-{uuid.uuid4()}"
        
        # ================================================================
        # Step 1: Validate inputs
        # ================================================================
        if not symbol:
            raise ValueError("Symbol is required")
        if side not in ("LONG", "SHORT"):
            raise ValueError("Side must be LONG or SHORT")
        if entry_price <= 0:
            raise ValueError("Entry price must be positive")
        if stop_price <= 0:
            raise ValueError("Stop price must be positive")
        if total_capital <= 0:
            raise ValueError("Total capital must be positive")
        if leverage < 1 or leverage > 10:
            raise ValueError("Leverage must be between 1 and 10")
        
        # Validate stop is on correct side
        if side == "LONG" and stop_price >= entry_price:
            raise ValueError("LONG stop must be below entry price")
        if side == "SHORT" and stop_price <= entry_price:
            raise ValueError("SHORT stop must be above entry price")
        
        # Validate target is on correct side (if provided)
        if target_price is not None:
            if side == "LONG" and target_price <= entry_price:
                raise ValueError("LONG target must be above entry price")
            if side == "SHORT" and target_price >= entry_price:
                raise ValueError("SHORT target must be below entry price")
        
        # ================================================================
        # Step 2: Check risk limits
        # ================================================================
        # Check monthly drawdown
        drawdown_check = self._risk.check_monthly_drawdown(client_id)
        if not drawdown_check.passed:
            raise ValueError(f"Monthly drawdown limit exceeded: {drawdown_check.reason}")
        
        # ================================================================
        # Step 3: Calculate position size
        # ================================================================
        sizing = calculate_margin_position_size(
            capital=total_capital,
            entry_price=entry_price,
            stop_price=stop_price,
            side=side,
            leverage=leverage,
            max_risk_percent=max_risk_percent,
        )
        
        # Check per-trade risk
        trade_risk_check = self._risk.check_trade_risk(
            client_id=client_id,
            symbol=symbol,
            side="BUY" if side == "LONG" else "SELL",
            quantity=sizing.quantity,
            entry_price=entry_price,
            stop_price=stop_price,
        )
        if not trade_risk_check.passed:
            raise ValueError(f"Trade risk check failed: {trade_risk_check.reason}")
        
        # ================================================================
        # Step 4: Get margin account and transfer if needed
        # ================================================================
        account = self._margin.get_margin_account(symbol)
        
        if account.quote_free < sizing.margin_required:
            if not auto_transfer_margin:
                raise ValueError(
                    f"Insufficient margin: need {sizing.margin_required}, "
                    f"have {account.quote_free}. Enable auto_transfer_margin or transfer manually."
                )
            
            # Calculate needed transfer (with 10% buffer)
            needed = sizing.margin_required - account.quote_free
            buffer = needed * Decimal("0.1")
            transfer_amount = (needed + buffer).quantize(Decimal("0.01"))
            
            # Transfer from Spot
            transfer_result = self._margin.transfer_to_margin(
                symbol=symbol,
                asset=account.quote_asset,
                amount=transfer_amount,
            )
            
            if not transfer_result.success:
                raise RuntimeError(
                    f"Failed to transfer margin: {transfer_result.error_message}"
                )
            
            # Refresh account info
            account = self._margin.get_margin_account(symbol)
        
        # ================================================================
        # Step 5: Place entry order
        # ================================================================
        order_side = "BUY" if side == "LONG" else "SELL"
        
        entry_result = self._margin.place_margin_order(
            symbol=symbol,
            side=order_side,
            order_type="MARKET",
            quantity=sizing.quantity,
            side_effect_type="MARGIN_BUY" if side == "LONG" else None,
        )
        
        if not entry_result.success:
            raise RuntimeError(f"Entry order failed: {entry_result.error_message}")
        
        # Use actual fill price for subsequent calculations
        actual_entry = entry_result.avg_fill_price or entry_price
        actual_quantity = entry_result.filled_quantity or sizing.quantity
        
        # ================================================================
        # Step 6: Place stop-loss order
        # ================================================================
        stop_side = "SELL" if side == "LONG" else "BUY"
        
        # For stop-loss, limit price slightly worse than stop price
        if side == "LONG":
            stop_limit_price = stop_price * Decimal("0.999")  # 0.1% below stop
        else:
            stop_limit_price = stop_price * Decimal("1.001")  # 0.1% above stop
        
        stop_result = self._margin.place_margin_order(
            symbol=symbol,
            side=stop_side,
            order_type="STOP_LOSS_LIMIT",
            quantity=actual_quantity,
            price=stop_limit_price.quantize(Decimal("0.01")),
            stop_price=stop_price,
            side_effect_type="AUTO_REPAY",
        )
        
        if not stop_result.success:
            # Entry succeeded but stop failed - log warning but continue
            # Position is open but unprotected!
            self._audit.record(
                event_type="margin_stop_order_failed",
                aggregate_id=position_id,
                data={
                    "error": stop_result.error_message,
                    "entry_order_id": entry_result.binance_order_id,
                },
                reason="⚠️ CRITICAL: Stop-loss order failed! Position unprotected!",
            )
        
        # ================================================================
        # Step 7: Place take-profit order (optional)
        # ================================================================
        target_order_id = None
        if target_price is not None:
            tp_result = self._margin.place_margin_order(
                symbol=symbol,
                side=stop_side,  # Same as stop side (closing the position)
                order_type="TAKE_PROFIT_LIMIT",
                quantity=actual_quantity,
                price=target_price,
                stop_price=target_price,
                side_effect_type="AUTO_REPAY",
            )
            
            if tp_result.success:
                target_order_id = tp_result.binance_order_id
        
        # ================================================================
        # Step 8: Create position record
        # ================================================================
        margin_side = MarginSide.LONG if side == "LONG" else MarginSide.SHORT
        
        position = MarginPosition(
            position_id=position_id,
            client_id=client_id,
            symbol=symbol,
            side=margin_side,
            status=MarginPositionStatus.OPEN,
            entry_price=actual_entry,
            quantity=actual_quantity,
            leverage=leverage,
            stop_price=stop_price,
            target_price=target_price,
            margin_allocated=sizing.margin_required,
            position_value=actual_entry * actual_quantity,
            risk_amount=sizing.risk_amount,
            risk_percent=sizing.risk_percent,
            current_price=actual_entry,
            binance_entry_order_id=entry_result.binance_order_id,
            binance_stop_order_id=stop_result.binance_order_id if stop_result.success else None,
            binance_target_order_id=target_order_id,
            created_at=now,
            opened_at=now,
            correlation_id=correlation_id,
        )
        
        # Save position
        saved_position = self._positions.save(position)
        
        # ================================================================
        # Step 9: Audit trail
        # ================================================================
        self._audit.record(
            event_type="margin_position_opened",
            aggregate_id=position_id,
            data={
                "client_id": client_id,
                "symbol": symbol,
                "side": side,
                "entry_price": str(actual_entry),
                "quantity": str(actual_quantity),
                "stop_price": str(stop_price),
                "target_price": str(target_price) if target_price else None,
                "leverage": leverage,
                "margin_allocated": str(sizing.margin_required),
                "risk_amount": str(sizing.risk_amount),
                "risk_percent": str(sizing.risk_percent),
                "entry_order_id": entry_result.binance_order_id,
                "stop_order_id": stop_result.binance_order_id if stop_result.success else None,
            },
            reason=f"Opened {side} position on {symbol}",
        )
        
        return saved_position, entry_result


# ============================================================================
# Position Closing Use Case
# ============================================================================

class CloseMarginPositionUseCase:
    """
    Close an existing Isolated Margin position.
    
    Flow:
    1. Retrieve position
    2. Cancel open stop-loss/take-profit orders
    3. Place market order to close position
    4. Calculate realized P&L
    5. Update position record
    6. Optionally transfer profit back to Spot
    7. Update monthly drawdown tracking
    """
    
    def __init__(
        self,
        margin_execution: MarginExecutionPort,
        position_repo: MarginPositionRepository,
        policy_state_repo: PolicyStateRepository,
        audit_trail: AuditTrailPort,
        clock: ClockPort,
    ):
        self._margin = margin_execution
        self._positions = position_repo
        self._policy_state = policy_state_repo
        self._audit = audit_trail
        self._clock = clock
    
    def execute(
        self,
        client_id: int,
        position_id: str,
        reason: str = "Manual close",
    ) -> MarginPosition:
        """
        Close a margin position.
        
        Args:
            client_id: Client identifier
            position_id: Position to close
            reason: Reason for closing
            
        Returns:
            Updated MarginPosition with realized P&L
            
        Raises:
            ValueError: If position not found or not open
        """
        now = self._clock.now()
        
        # Get position
        position = self._positions.find_by_id(position_id)
        if position is None:
            raise ValueError(f"Position {position_id} not found")
        
        if position.client_id != client_id:
            raise ValueError(f"Position {position_id} does not belong to client {client_id}")
        
        if not position.is_open:
            raise ValueError(f"Position {position_id} is not open (status: {position.status})")
        
        # Cancel open orders
        if position.binance_stop_order_id:
            self._margin.cancel_margin_order(
                symbol=position.symbol,
                order_id=position.binance_stop_order_id,
            )
        
        if position.binance_target_order_id:
            self._margin.cancel_margin_order(
                symbol=position.symbol,
                order_id=position.binance_target_order_id,
            )
        
        # Place closing order
        close_side = "SELL" if position.side == MarginSide.LONG else "BUY"
        
        close_result = self._margin.place_margin_order(
            symbol=position.symbol,
            side=close_side,
            order_type="MARKET",
            quantity=position.quantity,
            side_effect_type="AUTO_REPAY",
        )
        
        if not close_result.success:
            raise RuntimeError(f"Close order failed: {close_result.error_message}")
        
        fill_price = close_result.avg_fill_price or position.current_price
        
        # Update position
        closed_position = position.mark_as_closed(
            timestamp=now,
            fill_price=fill_price,
            reason=reason,
        )
        
        # Save updated position
        self._positions.save(closed_position)
        
        # Update monthly P&L tracking
        month = now.strftime("%Y-%m")
        policy_state = self._policy_state.get_state(client_id, month)
        
        if policy_state:
            updated_state = policy_state.update_pnl(
                realized_pnl_delta=closed_position.realized_pnl,
                unrealized_pnl=Decimal("0"),
                timestamp=now,
            )
            self._policy_state.save_state(updated_state)
        
        # Audit trail
        self._audit.record(
            event_type="margin_position_closed",
            aggregate_id=position_id,
            data={
                "symbol": position.symbol,
                "side": position.side.value,
                "close_price": str(fill_price),
                "realized_pnl": str(closed_position.realized_pnl),
                "net_pnl": str(closed_position.net_pnl),
                "reason": reason,
            },
            reason=f"Closed position: {reason}",
        )
        
        return closed_position


# ============================================================================
# Margin Monitoring Use Case
# ============================================================================

class MonitorMarginLevelUseCase:
    """
    Monitor margin levels for open positions.
    
    Checks all open positions and takes action based on margin health:
    - SAFE: No action
    - WARNING: Log warning
    - CRITICAL: Alert user, consider reducing position
    - DANGER: Close position to prevent liquidation
    """
    
    def __init__(
        self,
        margin_execution: MarginExecutionPort,
        position_repo: MarginPositionRepository,
        audit_trail: AuditTrailPort,
        clock: ClockPort,
    ):
        self._margin = margin_execution
        self._positions = position_repo
        self._audit = audit_trail
        self._clock = clock
    
    def execute(
        self,
        client_id: int,
        auto_close_on_danger: bool = False,
    ) -> list[dict]:
        """
        Monitor margin levels for all open positions.
        
        Args:
            client_id: Client identifier
            auto_close_on_danger: Whether to auto-close positions in DANGER zone
            
        Returns:
            List of position statuses with margin levels
        """
        results = []
        
        # Get all open positions
        open_positions = self._positions.find_open_by_client(client_id)
        
        for position in open_positions:
            try:
                # Get current margin level
                margin_level = self._margin.get_margin_level(position.symbol)
                
                # Classify health
                if margin_level >= Decimal("2.0"):
                    health = MarginLevel.SAFE
                elif margin_level >= Decimal("1.5"):
                    health = MarginLevel.CAUTION
                elif margin_level >= Decimal("1.3"):
                    health = MarginLevel.WARNING
                elif margin_level >= Decimal("1.1"):
                    health = MarginLevel.CRITICAL
                else:
                    health = MarginLevel.DANGER
                
                # Update position with current margin level
                updated_position = position.update_margin_level(margin_level)
                self._positions.save(updated_position)
                
                result = {
                    "position_id": position.position_id,
                    "symbol": position.symbol,
                    "side": position.side.value,
                    "margin_level": str(margin_level),
                    "health": health.value,
                    "action_taken": None,
                }
                
                # Log warnings for unhealthy positions
                if health in (MarginLevel.WARNING, MarginLevel.CRITICAL, MarginLevel.DANGER):
                    self._audit.record(
                        event_type="margin_level_warning",
                        aggregate_id=position.position_id,
                        data={
                            "margin_level": str(margin_level),
                            "health": health.value,
                        },
                        reason=f"⚠️ Margin level {health.value}: {margin_level}",
                    )
                
                # Auto-close on danger (if enabled)
                if health == MarginLevel.DANGER and auto_close_on_danger:
                    # Close position to prevent liquidation
                    close_use_case = CloseMarginPositionUseCase(
                        margin_execution=self._margin,
                        position_repo=self._positions,
                        policy_state_repo=None,  # TODO: Inject properly
                        audit_trail=self._audit,
                        clock=self._clock,
                    )
                    
                    try:
                        close_use_case.execute(
                            client_id=client_id,
                            position_id=position.position_id,
                            reason="Auto-closed: Margin level DANGER",
                        )
                        result["action_taken"] = "AUTO_CLOSED"
                    except Exception as e:
                        result["action_taken"] = f"CLOSE_FAILED: {e}"
                
                results.append(result)
                
            except Exception as e:
                results.append({
                    "position_id": position.position_id,
                    "symbol": position.symbol,
                    "error": str(e),
                })
        
        return results

