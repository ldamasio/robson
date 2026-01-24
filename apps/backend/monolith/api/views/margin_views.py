"""
REST API views for Isolated Margin Trading.

Endpoints:
- GET  /api/margin/account/{symbol}/     - Get margin account status
- POST /api/margin/transfer/to/          - Transfer Spot → Margin
- POST /api/margin/transfer/from/        - Transfer Margin → Spot
- POST /api/margin/position/calculate/   - Calculate position size
- POST /api/margin/position/open/        - Open margin position
- POST /api/margin/position/{id}/close/  - Close margin position
- GET  /api/margin/positions/            - List positions
- GET  /api/margin/positions/{id}/       - Get position details
- GET  /api/margin/monitor/              - Monitor margin levels
"""

from decimal import Decimal, InvalidOperation
from rest_framework.decorators import api_view, permission_classes
from rest_framework.permissions import IsAuthenticated
from rest_framework.response import Response
from rest_framework import status

from django.utils import timezone

from api.models import MarginPosition, MarginTransfer
from api.application.margin_adapters import BinanceMarginAdapter, MockMarginAdapter
from api.services.market_price_cache import get_cached_bid

# Import margin position sizing - use local implementation to avoid path issues in container
def calculate_margin_position_size(
    capital,
    entry_price,
    stop_price,
    side=None,
    leverage=3,
    max_risk_percent=1.0,
):
    """
    Calculate position size for margin trading using the Golden Rule.
    
    Position Size = (Risk Amount) / (Stop Distance)
    
    Where:
    - Risk Amount = Capital × (max_risk_percent / 100)
    - Stop Distance = |Entry Price - Stop Price|
    """
    from decimal import Decimal
    
    capital = Decimal(str(capital))
    entry_price = Decimal(str(entry_price))
    stop_price = Decimal(str(stop_price))
    max_risk_percent = Decimal(str(max_risk_percent))
    
    risk_amount = capital * (max_risk_percent / Decimal('100'))
    stop_distance = abs(entry_price - stop_price)
    
    if stop_distance == 0:
        return Decimal('0')
    
    quantity = risk_amount / stop_distance
    return quantity


def _get_adapter(use_testnet: bool = None):
    """Get the appropriate margin adapter based on settings."""
    from django.conf import settings
    
    if use_testnet is None:
        use_testnet = getattr(settings, 'BINANCE_USE_TESTNET', True)
    
    return BinanceMarginAdapter(use_testnet=use_testnet)


# ============================================================================
# Account Status
# ============================================================================

@api_view(['GET'])
@permission_classes([IsAuthenticated])
def margin_account(request, symbol):
    """
    Get Isolated Margin account status for a symbol.
    
    Path Parameters:
        symbol: Trading pair (e.g., BTCUSDC)
        
    Response:
        {
            "symbol": "BTCUSDC",
            "base_asset": "BTC",
            "base_free": "0.001",
            "quote_asset": "USDC",
            "quote_free": "100.00",
            "margin_level": "999.00",
            "margin_health": "SAFE",
            "liquidation_price": "0",
            "can_trade": true
        }
    """
    try:
        adapter = _get_adapter()
        account = adapter.get_margin_account(symbol.upper())
        
        # Classify margin health
        margin_level = account.margin_level
        if margin_level >= Decimal("2.0"):
            health = "SAFE"
        elif margin_level >= Decimal("1.5"):
            health = "CAUTION"
        elif margin_level >= Decimal("1.3"):
            health = "WARNING"
        elif margin_level >= Decimal("1.1"):
            health = "CRITICAL"
        else:
            health = "DANGER"
        response_data = {
            "symbol": account.symbol,
            "base_asset": account.base_asset,
            "base_free": str(account.base_free),
            "base_locked": str(account.base_locked),
            "base_borrowed": str(account.base_borrowed),
            "quote_asset": account.quote_asset,
            "quote_free": str(account.quote_free),
            "quote_locked": str(account.quote_locked),
            "quote_borrowed": str(account.quote_borrowed),
            "margin_level": str(account.margin_level),
            "marginLevel": str(account.margin_level), # Compatibility for Balance.jsx
            "totalUSDC": str(account.quote_free + account.quote_locked), # Compatibility
            "freeUSDC": str(account.quote_free), # Compatibility
            "margin_health": health,
            "liquidation_price": str(account.liquidation_price),
            "can_trade": account.is_margin_trade_enabled,
        }
        
        # Calculate Estimated Net BTC (Equity in BTC)
        price = get_cached_bid(symbol.upper())
        if price:
            net_base = (account.base_free + account.base_locked) - account.base_borrowed
            net_quote = (account.quote_free + account.quote_locked) - account.quote_borrowed
            net_equity_btc = net_base + (net_quote / price)
            response_data["net_equity_btc"] = str(net_equity_btc.quantize(Decimal("0.00000001")))
        else:
            response_data["net_equity_btc"] = "N/A"
            
        return Response(response_data)
        
    except ValueError as e:
        return Response(
            {"error": str(e)},
            status=status.HTTP_404_NOT_FOUND
        )
    except Exception as e:
        return Response(
            {"error": str(e)},
            status=status.HTTP_500_INTERNAL_SERVER_ERROR
        )


# ============================================================================
# Transfers
# ============================================================================

@api_view(['POST'])
@permission_classes([IsAuthenticated])
def transfer_to_margin(request):
    """
    Transfer assets from Spot to Isolated Margin.
    
    Request Body:
        {
            "symbol": "BTCUSDC",
            "asset": "USDC",
            "amount": "100.00"
        }
        
    Response:
        {
            "success": true,
            "transaction_id": "12345",
            "asset": "USDC",
            "amount": "100.00",
            "from": "SPOT",
            "to": "ISOLATED_MARGIN:BTCUSDC"
        }
    """
    symbol = request.data.get("symbol", "").upper()
    asset = request.data.get("asset", "").upper()
    amount_str = request.data.get("amount", "")
    
    if not all([symbol, asset, amount_str]):
        return Response(
            {"error": "symbol, asset, and amount are required"},
            status=status.HTTP_400_BAD_REQUEST
        )
    
    try:
        amount = Decimal(amount_str)
        if amount <= 0:
            raise ValueError("Amount must be positive")
    except (InvalidOperation, ValueError) as e:
        return Response(
            {"error": f"Invalid amount: {e}"},
            status=status.HTTP_400_BAD_REQUEST
        )
    
    try:
        adapter = _get_adapter()
        result = adapter.transfer_to_margin(symbol, asset, amount)
        
        # Record transfer
        if result.success:
            MarginTransfer.objects.create(
                transaction_id=result.transaction_id or f"manual-{timezone.now().timestamp()}",
                client=request.user.client,
                symbol=symbol,
                asset=asset,
                amount=amount,
                direction=MarginTransfer.Direction.TO_MARGIN,
                success=True,
            )
        
        return Response({
            "success": result.success,
            "transaction_id": result.transaction_id,
            "asset": result.asset,
            "amount": str(result.amount),
            "from": result.from_account,
            "to": result.to_account,
            "error": result.error_message,
        })
        
    except Exception as e:
        return Response(
            {"error": str(e)},
            status=status.HTTP_500_INTERNAL_SERVER_ERROR
        )


@api_view(['POST'])
@permission_classes([IsAuthenticated])
def transfer_from_margin(request):
    """
    Transfer assets from Isolated Margin to Spot.
    
    Request Body:
        {
            "symbol": "BTCUSDC",
            "asset": "USDC",
            "amount": "100.00"
        }
    """
    symbol = request.data.get("symbol", "").upper()
    asset = request.data.get("asset", "").upper()
    amount_str = request.data.get("amount", "")
    
    if not all([symbol, asset, amount_str]):
        return Response(
            {"error": "symbol, asset, and amount are required"},
            status=status.HTTP_400_BAD_REQUEST
        )
    
    try:
        amount = Decimal(amount_str)
        if amount <= 0:
            raise ValueError("Amount must be positive")
    except (InvalidOperation, ValueError) as e:
        return Response(
            {"error": f"Invalid amount: {e}"},
            status=status.HTTP_400_BAD_REQUEST
        )
    
    try:
        adapter = _get_adapter()
        result = adapter.transfer_from_margin(symbol, asset, amount)
        
        # Record transfer
        if result.success:
            MarginTransfer.objects.create(
                transaction_id=result.transaction_id or f"manual-{timezone.now().timestamp()}",
                client=request.user.client,
                symbol=symbol,
                asset=asset,
                amount=amount,
                direction=MarginTransfer.Direction.FROM_MARGIN,
                success=True,
            )
        
        return Response({
            "success": result.success,
            "transaction_id": result.transaction_id,
            "asset": result.asset,
            "amount": str(result.amount),
            "from": result.from_account,
            "to": result.to_account,
            "error": result.error_message,
        })
        
    except Exception as e:
        return Response(
            {"error": str(e)},
            status=status.HTTP_500_INTERNAL_SERVER_ERROR
        )


# ============================================================================
# Position Sizing
# ============================================================================

@api_view(['POST'])
@permission_classes([IsAuthenticated])
def calculate_position_size(request):
    """
    Calculate optimal position size for a margin trade.
    
    Uses the 1% risk rule to determine position size based on stop distance.
    
    Request Body:
        {
            "symbol": "BTCUSDC",
            "side": "LONG",
            "entry_price": "100000",
            "stop_price": "98000",
            "capital": "1000",
            "leverage": 3,
            "risk_percent": "1.0"
        }
        
    Response:
        {
            "quantity": "0.00166667",
            "position_value": "166.67",
            "margin_required": "55.56",
            "risk_amount": "10.00",
            "risk_percent": "1.00",
            "stop_distance": "2000",
            "stop_distance_percent": "2.00",
            "leverage": 3,
            "is_capped": false
        }
    """
    try:
        symbol = request.data.get("symbol", "").upper()
        side = request.data.get("side", "").upper()
        entry_price = Decimal(request.data.get("entry_price", "0"))
        stop_price = Decimal(request.data.get("stop_price", "0"))
        capital = Decimal(request.data.get("capital", "0"))
        leverage = int(request.data.get("leverage", 3))
        risk_percent = Decimal(request.data.get("risk_percent", "1.0"))
        
    except (InvalidOperation, ValueError, TypeError) as e:
        return Response(
            {"error": f"Invalid parameter: {e}"},
            status=status.HTTP_400_BAD_REQUEST
        )
    
    # Validate inputs
    if not symbol:
        return Response({"error": "symbol is required"}, status=status.HTTP_400_BAD_REQUEST)
    if side not in ("LONG", "SHORT"):
        return Response({"error": "side must be LONG or SHORT"}, status=status.HTTP_400_BAD_REQUEST)
    if entry_price <= 0:
        return Response({"error": "entry_price must be positive"}, status=status.HTTP_400_BAD_REQUEST)
    if stop_price <= 0:
        return Response({"error": "stop_price must be positive"}, status=status.HTTP_400_BAD_REQUEST)
    if capital <= 0:
        return Response({"error": "capital must be positive"}, status=status.HTTP_400_BAD_REQUEST)
    
    try:
        result = calculate_margin_position_size(
            capital=capital,
            entry_price=entry_price,
            stop_price=stop_price,
            side=side,
            leverage=leverage,
            max_risk_percent=risk_percent,
        )
        
        return Response({
            "symbol": symbol,
            "side": side,
            "entry_price": str(entry_price),
            "stop_price": str(stop_price),
            "quantity": str(result.quantity),
            "position_value": str(result.position_value),
            "margin_required": str(result.margin_required),
            "risk_amount": str(result.risk_amount),
            "risk_percent": str(result.risk_percent),
            "stop_distance": str(result.stop_distance),
            "stop_distance_percent": str(result.stop_distance_percent),
            "leverage": result.leverage,
            "is_capped": result.is_capped,
            "cap_reason": result.cap_reason,
        })
        
    except ValueError as e:
        return Response(
            {"error": str(e)},
            status=status.HTTP_400_BAD_REQUEST
        )


# ============================================================================
# Position Management
# ============================================================================

@api_view(['POST'])
@permission_classes([IsAuthenticated])
def open_position(request):
    """
    Open a new margin position.
    
    IMPORTANT: This will execute REAL trades if not in testnet mode!
    
    Request Body:
        {
            "symbol": "BTCUSDC",
            "side": "LONG",
            "entry_price": "100000",
            "stop_price": "98000",
            "target_price": "105000",
            "capital": "1000",
            "leverage": 3,
            "dry_run": true
        }
    """
    try:
        symbol = request.data.get("symbol", "").upper()
        raw_side = request.data.get("side", "").upper()
        # Normalize side to standard internal LONG/SHORT
        if raw_side in ["BUY", "LONG"]:
            side = "LONG"
        elif raw_side in ["SELL", "SHORT"]:
            side = "SHORT"
        else:
            side = raw_side # Let cleaner/validator catch it
            
        entry_price = Decimal(request.data.get("entry_price", "0"))
        stop_price = Decimal(request.data.get("stop_price", "0"))
        target_price_str = request.data.get("target_price")
        target_price = Decimal(target_price_str) if target_price_str else None
        capital = Decimal(request.data.get("capital", "0"))
        leverage = int(request.data.get("leverage", 3))
        dry_run = request.data.get("dry_run", True)
        
    except (InvalidOperation, ValueError, TypeError) as e:
        return Response(
            {"error": f"Invalid parameter: {e}"},
            status=status.HTTP_400_BAD_REQUEST
        )
    
    # Validate
    if not all([symbol, side, entry_price, stop_price, capital]):
        return Response(
            {"error": "symbol, side, entry_price, stop_price, and capital are required"},
            status=status.HTTP_400_BAD_REQUEST
        )
    
    try:
        # Calculate position size
        sizing = calculate_margin_position_size(
            capital=capital,
            entry_price=entry_price,
            stop_price=stop_price,
            side=side,
            leverage=leverage,
        )
        
        # Get adapter (mock for dry run)
        adapter = MockMarginAdapter() if dry_run else _get_adapter()
        
        # Place entry order
        order_side = "BUY" if side == "LONG" else "SELL"
        entry_result = adapter.place_margin_order(
            symbol=symbol,
            side=order_side,
            order_type="MARKET",
            quantity=sizing.quantity,
        )
        
        if not entry_result.success:
            return Response(
                {"error": f"Entry order failed: {entry_result.error_message}"},
                status=status.HTTP_400_BAD_REQUEST
            )
        
        # Place stop-loss
        stop_side = "SELL" if side == "LONG" else "BUY"
        stop_result = adapter.place_margin_order(
            symbol=symbol,
            side=stop_side,
            order_type="STOP_LOSS_LIMIT",
            quantity=sizing.quantity,
            price=stop_price * Decimal("0.999") if side == "LONG" else stop_price * Decimal("1.001"),
            stop_price=stop_price,
        )
        
        # Create position record
        import uuid
        position = MarginPosition.objects.create(
            position_id=f"margin-{uuid.uuid4()}",
            client=request.user.client,
            symbol=symbol,
            side=side,
            status=MarginPosition.Status.OPEN,
            leverage=leverage,
            entry_price=entry_result.avg_fill_price or entry_price,
            stop_price=stop_price,
            target_price=target_price,
            quantity=sizing.quantity,
            position_value=sizing.position_value,
            margin_allocated=sizing.margin_required,
            risk_amount=sizing.risk_amount,
            risk_percent=sizing.risk_percent,
            current_price=entry_result.avg_fill_price or entry_price,
            binance_entry_order_id=entry_result.binance_order_id,
            binance_stop_order_id=stop_result.binance_order_id if stop_result.success else None,
            opened_at=timezone.now(),
        )
        
        return Response({
            "success": True,
            "mode": "DRY_RUN" if dry_run else "LIVE",
            "position_id": position.position_id,
            "symbol": symbol,
            "side": side,
            "quantity": str(sizing.quantity),
            "entry_price": str(position.entry_price),
            "stop_price": str(stop_price),
            "target_price": str(target_price) if target_price else None,
            "margin_required": str(sizing.margin_required),
            "risk_amount": str(sizing.risk_amount),
            "risk_percent": str(sizing.risk_percent),
            "entry_order_id": entry_result.binance_order_id,
            "stop_order_id": stop_result.binance_order_id if stop_result.success else None,
        }, status=status.HTTP_201_CREATED)
        
    except ValueError as e:
        return Response(
            {"error": str(e)},
            status=status.HTTP_400_BAD_REQUEST
        )
    except Exception as e:
        return Response(
            {"error": str(e)},
            status=status.HTTP_500_INTERNAL_SERVER_ERROR
        )


@api_view(['POST'])
@permission_classes([IsAuthenticated])
def close_position(request, position_id):
    """
    Close an existing margin position.
    
    Path Parameters:
        position_id: Position ID to close
        
    Request Body:
        {
            "reason": "Manual close",
            "dry_run": true
        }
    """
    reason = request.data.get("reason", "Manual close")
    dry_run = request.data.get("dry_run", True)
    
    try:
        position = MarginPosition.objects.get(
            position_id=position_id,
            client=request.user.client,
        )
    except MarginPosition.DoesNotExist:
        return Response(
            {"error": "Position not found"},
            status=status.HTTP_404_NOT_FOUND
        )
    
    if not position.is_open:
        return Response(
            {"error": f"Position is not open (status: {position.status})"},
            status=status.HTTP_400_BAD_REQUEST
        )
    
    try:
        adapter = MockMarginAdapter() if dry_run else _get_adapter()
        
        # Cancel open orders
        if position.binance_stop_order_id:
            adapter.cancel_margin_order(position.symbol, position.binance_stop_order_id)
        if position.binance_target_order_id:
            adapter.cancel_margin_order(position.symbol, position.binance_target_order_id)
        
        # Place close order
        close_side = "SELL" if position.side == MarginPosition.Side.LONG else "BUY"
        close_result = adapter.place_margin_order(
            symbol=position.symbol,
            side=close_side,
            order_type="MARKET",
            quantity=position.quantity,
        )
        
        if not close_result.success:
            return Response(
                {"error": f"Close order failed: {close_result.error_message}"},
                status=status.HTTP_400_BAD_REQUEST
            )
        
        # Update position
        fill_price = close_result.avg_fill_price or position.current_price
        position.close(fill_price, reason)
        position.binance_close_order_id = close_result.binance_order_id
        position.save()
        
        return Response({
            "success": True,
            "mode": "DRY_RUN" if dry_run else "LIVE",
            "position_id": position.position_id,
            "close_price": str(fill_price),
            "realized_pnl": str(position.realized_pnl),
            "total_pnl": str(position.total_pnl),
            "is_profitable": position.is_profitable,
            "close_reason": reason,
        })
        
    except Exception as e:
        return Response(
            {"error": str(e)},
            status=status.HTTP_500_INTERNAL_SERVER_ERROR
        )


@api_view(['GET'])
@permission_classes([IsAuthenticated])
def list_positions(request):
    """
    List margin positions.
    
    Query Parameters:
        status: Filter by status (OPEN, CLOSED, etc.)
        symbol: Filter by symbol
    """
    queryset = MarginPosition.objects.filter(client=request.user.client)
    
    # Apply filters
    status_filter = request.query_params.get("status")
    if status_filter:
        queryset = queryset.filter(status=status_filter.upper())
    
    symbol_filter = request.query_params.get("symbol")
    if symbol_filter:
        queryset = queryset.filter(symbol=symbol_filter.upper())
    
    positions = queryset.order_by("-created_at")[:100]
    
    return Response({
        "count": positions.count(),
        "positions": [
            {
                "position_id": p.position_id,
                "symbol": p.symbol,
                "side": p.side,
                "status": p.status,
                "leverage": p.leverage,
                "entry_price": str(p.entry_price),
                "stop_price": str(p.stop_price),
                "target_price": str(p.target_price) if p.target_price else None,
                "quantity": str(p.quantity),
                "margin_allocated": str(p.margin_allocated),
                "unrealized_pnl": str(p.unrealized_pnl),
                "realized_pnl": str(p.realized_pnl),
                "total_pnl": str(p.total_pnl),
                "margin_level": str(p.margin_level),
                "margin_health": p.margin_health,
                "binance_stop_order_id": p.binance_stop_order_id,
                "transfer_count": p.transfers.count(),
                "opened_at": p.opened_at.isoformat() if p.opened_at else None,
                "closed_at": p.closed_at.isoformat() if p.closed_at else None,
            }
            for p in positions
        ]
    })


@api_view(['GET'])
@permission_classes([IsAuthenticated])
def get_position(request, position_id):
    """
    Get details of a specific margin position.
    """
    try:
        position = MarginPosition.objects.get(
            position_id=position_id,
            client=request.user.client,
        )
    except MarginPosition.DoesNotExist:
        return Response(
            {"error": "Position not found"},
            status=status.HTTP_404_NOT_FOUND
        )
    
    return Response({
        "position_id": position.position_id,
        "symbol": position.symbol,
        "side": position.side,
        "status": position.status,
        "leverage": position.leverage,
        "entry_price": str(position.entry_price),
        "stop_price": str(position.stop_price),
        "target_price": str(position.target_price) if position.target_price else None,
        "current_price": str(position.current_price),
        "close_price": str(position.close_price) if position.close_price else None,
        "quantity": str(position.quantity),
        "position_value": str(position.position_value),
        "margin_allocated": str(position.margin_allocated),
        "borrowed_amount": str(position.borrowed_amount),
        "interest_accrued": str(position.interest_accrued),
        "risk_amount": str(position.risk_amount),
        "risk_percent": str(position.risk_percent),
        "unrealized_pnl": str(position.unrealized_pnl),
        "realized_pnl": str(position.realized_pnl),
        "fees_paid": str(position.fees_paid),
        "total_pnl": str(position.total_pnl),
        "is_profitable": position.is_profitable,
        "margin_level": str(position.margin_level),
        "margin_health": position.margin_health,
        "is_at_risk": position.is_at_risk,
        "binance_entry_order_id": position.binance_entry_order_id,
        "binance_stop_order_id": position.binance_stop_order_id,
        "transfer_count": position.transfers.count(),
        "close_reason": position.close_reason,
        "created_at": position.created_at.isoformat(),
        "opened_at": position.opened_at.isoformat() if position.opened_at else None,
        "closed_at": position.closed_at.isoformat() if position.closed_at else None,
    })


# ============================================================================
# Monitoring
# ============================================================================

@api_view(['GET'])
@permission_classes([IsAuthenticated])
def monitor_margins(request):
    """
    Monitor margin levels for all open positions.
    
    Response:
        {
            "timestamp": "2024-12-23T12:00:00Z",
            "positions": [...],
            "alerts": [...]
        }
    """
    positions = MarginPosition.objects.filter(
        client=request.user.client,
        status=MarginPosition.Status.OPEN,
    )
    
    results = []
    alerts = []
    
    for position in positions:
        try:
            adapter = _get_adapter()
            margin_level = adapter.get_margin_level(position.symbol)
            
            # Update position
            position.margin_level = margin_level
            position.save(update_fields=['margin_level', 'updated_at'])
            
            health = position.margin_health
            
            result = {
                "position_id": position.position_id,
                "symbol": position.symbol,
                "side": position.side,
                "margin_level": str(margin_level),
                "margin_health": health,
                "quantity": str(position.quantity),
                "unrealized_pnl": str(position.unrealized_pnl),
            }
            results.append(result)
            
            # Generate alerts for unhealthy positions
            if health in ("WARNING", "CRITICAL", "DANGER"):
                alerts.append({
                    "position_id": position.position_id,
                    "symbol": position.symbol,
                    "health": health,
                    "margin_level": str(margin_level),
                    "message": f"Margin level {health}: {margin_level}",
                })
                
        except Exception as e:
            results.append({
                "position_id": position.position_id,
                "symbol": position.symbol,
                "error": str(e),
            })
    
    return Response({
        "timestamp": timezone.now().isoformat(),
        "positions": results,
        "alerts": alerts,
        "total_open": len(positions),
        "at_risk": len([r for r in results if r.get("margin_health") in ("WARNING", "CRITICAL", "DANGER")]),
    })

