from decimal import ROUND_HALF_UP, Decimal

from clients.models import Client
from rest_framework.decorators import api_view, permission_classes
from rest_framework.permissions import IsAuthenticated
from rest_framework.response import Response

from api.models import Operation, Order
from api.models.margin import MarginPosition
from api.services.market_price_cache import get_cached_bid

USD_QUANT = Decimal("0.01")
PERCENT_QUANT = Decimal("0.01")
QTY_QUANT = Decimal("0.00000001")


def _quantize(value: Decimal, quant: Decimal) -> Decimal:
    return value.quantize(quant, rounding=ROUND_HALF_UP)


def _format_decimal(value: Decimal | None, quant: Decimal) -> str | None:
    if value is None:
        return None
    return str(_quantize(value, quant))


def _calculate_weighted_entry(order_list: list[Order]) -> tuple[Decimal | None, Decimal]:
    total_qty = Decimal("0")
    total_cost = Decimal("0")
    for order in order_list:
        qty = order.filled_quantity or order.quantity or Decimal("0")
        price = order.avg_fill_price if order.avg_fill_price is not None else order.price
        if price is None:
            continue
        total_qty += qty
        total_cost += price * qty
    if total_qty == 0:
        return None, Decimal("0")
    return total_cost / total_qty, total_qty


def _calculate_stop_price(entry_price: Decimal, percent: Decimal, side: str) -> Decimal:
    if side == "BUY":
        return entry_price * (Decimal("1") - (percent / Decimal("100")))
    return entry_price * (Decimal("1") + (percent / Decimal("100")))


def _calculate_target_price(entry_price: Decimal, percent: Decimal, side: str) -> Decimal:
    if side == "BUY":
        return entry_price * (Decimal("1") + (percent / Decimal("100")))
    return entry_price * (Decimal("1") - (percent / Decimal("100")))


def _calculate_distance_percent(target_price: Decimal, current_price: Decimal) -> Decimal:
    if current_price == 0:
        return Decimal("0")
    return ((target_price - current_price) / current_price) * Decimal("100")


@api_view(["GET"])
@permission_classes([IsAuthenticated])
def active_positions(request):
    """
    Return active positions with current price and unrealized P&L.

    Includes BOTH:
    - Spot positions (from Operation model)
    - Margin positions (from MarginPosition model)
    """
    try:
        # Get client - try user.client_id first, then fallback to Client ID 1
        client_id = getattr(request.user, "client_id", None)
        client = None
        if client_id:
            try:
                client = Client.objects.get(id=client_id)
            except Client.DoesNotExist:
                pass
        if not client:
            try:
                client = Client.objects.get(id=1)
            except Client.DoesNotExist:
                pass

        positions = []
        price_cache: dict[str, Decimal] = {}

        # ============================================
        # PART 1: Spot Operations (legacy)
        # ============================================
        operations = (
            Operation.objects.filter(status="ACTIVE")
            .select_related("symbol")
            .prefetch_related("entry_orders")
        )
        if client_id:
            operations = operations.filter(client_id=client_id)

        for operation in operations:
            symbol = operation.symbol.name
            filled_orders = list(operation.entry_orders.filter(status="FILLED"))
            entry_price, quantity = _calculate_weighted_entry(filled_orders)

            if entry_price is None:
                entry_price = operation.average_entry_price
                quantity = operation.total_entry_quantity

            if symbol not in price_cache:
                price_cache[symbol] = get_cached_bid(symbol)
            current_price = price_cache[symbol]

            unrealized_pnl = Decimal("0")
            unrealized_pnl_percent = Decimal("0")
            if entry_price and quantity:
                if operation.side == "BUY":
                    unrealized_pnl = (current_price - entry_price) * quantity
                else:
                    unrealized_pnl = (entry_price - current_price) * quantity
                cost_basis = entry_price * quantity
                if cost_basis != 0:
                    unrealized_pnl_percent = (unrealized_pnl / cost_basis) * Decimal("100")

            stop_loss_price = None
            take_profit_price = None
            if entry_price and operation.stop_loss_percent:
                stop_loss_price = _calculate_stop_price(
                    entry_price, operation.stop_loss_percent, operation.side
                )
            if entry_price and operation.stop_gain_percent:
                take_profit_price = _calculate_target_price(
                    entry_price, operation.stop_gain_percent, operation.side
                )

            distance_to_stop = None
            distance_to_target = None
            if stop_loss_price is not None:
                distance_to_stop = _calculate_distance_percent(stop_loss_price, current_price)
            if take_profit_price is not None:
                distance_to_target = _calculate_distance_percent(take_profit_price, current_price)

            positions.append(
                {
                    "id": operation.id,
                    "operation_id": operation.id,
                    "symbol": symbol,
                    "side": operation.side,
                    "quantity": _format_decimal(quantity, QTY_QUANT),
                    "entry_price": _format_decimal(entry_price, USD_QUANT),
                    "current_price": _format_decimal(current_price, USD_QUANT),
                    "unrealized_pnl": _format_decimal(unrealized_pnl, USD_QUANT),
                    "unrealized_pnl_percent": _format_decimal(
                        unrealized_pnl_percent, PERCENT_QUANT
                    ),
                    "stop_loss": _format_decimal(stop_loss_price, USD_QUANT),
                    "take_profit": _format_decimal(take_profit_price, USD_QUANT),
                    "distance_to_stop_percent": _format_decimal(distance_to_stop, PERCENT_QUANT),
                    "distance_to_target_percent": _format_decimal(
                        distance_to_target, PERCENT_QUANT
                    ),
                    "status": "OPEN",
                    "type": "spot",
                }
            )

        # ============================================
        # PART 2: Margin Positions (new)
        # ============================================
        margin_positions_raw = MarginPosition.objects.filter(status=MarginPosition.Status.OPEN)
        if client:
            margin_positions_raw = margin_positions_raw.filter(client=client)

        # Group by symbol to avoid duplicate cards for the same Isolated Margin account
        grouped_margin = {}

        try:
            from .margin_views import _get_adapter
            adapter = _get_adapter()
        except ImportError:
            adapter = None

        for mp in margin_positions_raw:
            # Sanitize symbol: force uppercase and remove slashes for consistent lookup
            symbol = mp.symbol.replace("/", "").upper()
            
            if symbol not in grouped_margin:
                # Get current price
                if symbol not in price_cache:
                    price_cache[symbol] = get_cached_bid(symbol)
                current_price = price_cache[symbol]

                # Soft Sync: Determine REAL side from Binance
                current_margin_level = mp.margin_level
                # Use current side as fallback
                real_side = str(mp.side).strip().upper()
                
                if adapter:
                    try:
                        account_snapshot = adapter.get_margin_account(symbol)
                        if account_snapshot:
                            current_margin_level = account_snapshot.margin_level
                            
                            # Net base = (Free + Locked) - Borrowed
                            # LONG: Net Base > 0 (More owned than borrowed)
                            # SHORT: Net Base < 0 (More borrowed than owned)
                            net_base = (
                                account_snapshot.base_free 
                                + account_snapshot.base_locked 
                                - account_snapshot.base_borrowed
                            )
                            
                            threshold = Decimal("0.00000001")
                            if net_base > threshold:
                                real_side = "LONG"
                            elif net_base < -threshold:
                                real_side = "SHORT"
                            
                            # Update the side for the aggregator pass below
                            mp.side = real_side
                    except Exception as e:
                        import logging
                        logging.warning(f"Failed to sync margin side for {symbol}: {e}")
                
                import logging
                logging.warning(f"MARGIN_DEBUG: Symbol={symbol}, RealSide={real_side}, ML={current_margin_level}")

                grouped_margin[symbol] = {
                    "symbol": symbol,
                    "total_qty_long": Decimal("0"),
                    "total_qty_short": Decimal("0"),
                    "total_cost_long": Decimal("0"),
                    "total_cost_short": Decimal("0"),
                    "current_price": current_price,
                    "margin_level": current_margin_level,
                    "stop_loss": mp.stop_price,
                    "leverage": mp.leverage,
                    "risk_amount": Decimal("0"),
                    "risk_percent": Decimal("0"),
                    "force_side": real_side, # Binance truth
                }
            
            # Aggregate weights using the synced/normalized side
            side_norm = str(mp.side).strip().upper()
            is_long = side_norm in ["LONG", "BUY"]
            
            if is_long:
                grouped_margin[symbol]["total_qty_long"] += mp.quantity
                grouped_margin[symbol]["total_cost_long"] += mp.quantity * mp.entry_price
            else:
                grouped_margin[symbol]["total_qty_short"] += mp.quantity
                grouped_margin[symbol]["total_cost_short"] += mp.quantity * mp.entry_price
            
            grouped_margin[symbol]["risk_amount"] += mp.risk_amount
            grouped_margin[symbol]["risk_percent"] += mp.risk_percent
            grouped_margin[symbol]["leverage"] = max(grouped_margin[symbol]["leverage"], mp.leverage)
            
            # Most conservative stop
            if is_long:
                grouped_margin[symbol]["stop_loss"] = min(grouped_margin[symbol]["stop_loss"], mp.stop_price)
            else:
                grouped_margin[symbol]["stop_loss"] = max(grouped_margin[symbol]["stop_loss"], mp.stop_price)

        # Convert grouped results to final list
        for symbol, data in grouped_margin.items():
            net_qty = data["total_qty_long"] - data["total_qty_short"]
            abs_qty = abs(net_qty)
            
            # Skip if no net position remains
            if abs_qty < Decimal("0.00000001"):
                continue

            # FINAL SIDE: Use coordinated Binance truth
            final_side = data.get("force_side")
            
            # Weighted entry price based on the truth side
            if final_side == "LONG":
                avg_entry = data["total_cost_long"] / data["total_qty_long"] if data["total_qty_long"] else Decimal("0")
            else:
                avg_entry = data["total_cost_short"] / data["total_qty_short"] if data["total_qty_short"] else Decimal("0")

            # Calculate P&L using final_side
            unrealized_pnl = Decimal("0")
            unrealized_pnl_percent = Decimal("0")
            current_price = data["current_price"]
            
            if avg_entry and abs_qty and current_price:
                if final_side == "LONG":
                    unrealized_pnl = (current_price - avg_entry) * abs_qty
                else:
                    unrealized_pnl = (avg_entry - current_price) * abs_qty
                
                cost_basis = avg_entry * abs_qty
                if cost_basis != 0:
                    unrealized_pnl_percent = (unrealized_pnl / cost_basis) * Decimal("100")

            distance_to_stop = None
            if data["stop_loss"] and current_price:
                distance_to_stop = _calculate_distance_percent(data["stop_loss"], current_price)

            positions.append(
                {
                    "id": f"margin-{symbol}",
                    "operation_id": f"agg-{symbol}",
                    "symbol": symbol,
                    "side": final_side,
                    "quantity": _format_decimal(abs_qty, QTY_QUANT),
                    "entry_price": _format_decimal(avg_entry, USD_QUANT),
                    "current_price": _format_decimal(current_price, USD_QUANT),
                    "unrealized_pnl": _format_decimal(unrealized_pnl, USD_QUANT),
                    "unrealized_pnl_percent": _format_decimal(unrealized_pnl_percent, PERCENT_QUANT),
                    "stop_loss": _format_decimal(data["stop_loss"], USD_QUANT),
                    "take_profit": None,
                    "distance_to_stop_percent": _format_decimal(distance_to_stop, PERCENT_QUANT),
                    "distance_to_target_percent": None,
                    "status": "OPEN",
                    "type": "margin",
                    "leverage": data["leverage"],
                    "risk_amount": _format_decimal(data["risk_amount"], USD_QUANT),
                    "risk_percent": _format_decimal(data["risk_percent"], PERCENT_QUANT),
                    "margin_level": _format_decimal(data["margin_level"], PERCENT_QUANT) if data["margin_level"] else None,
                }
            )

        return Response({"positions": positions})
    except Exception as e:
        import logging

        logging.error(f"Failed to get positions: {e}", exc_info=True)
        return Response({"error": f"Failed to get positions: {e!s}"}, status=500)
