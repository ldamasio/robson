from decimal import Decimal, ROUND_HALF_UP

from rest_framework.decorators import api_view, permission_classes
from rest_framework.permissions import IsAuthenticated
from rest_framework.response import Response

from api.models import Operation, Order
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


@api_view(['GET'])
@permission_classes([IsAuthenticated])
def active_positions(request):
    """Return active positions with current price and unrealized P&L."""
    try:
        client_id = request.user.client_id
        # Allow None for admin/superuser to see all positions
        # if not client_id:
        #     return Response({"error": "Client not found"}, status=400)

        operations = (
            Operation.objects.filter(client_id=client_id, status="ACTIVE")
            .select_related("symbol")
            .prefetch_related("entry_orders")
        )

        positions = []
        price_cache: dict[str, Decimal] = {}
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
                stop_loss_price = _calculate_stop_price(entry_price, operation.stop_loss_percent, operation.side)
            if entry_price and operation.stop_gain_percent:
                take_profit_price = _calculate_target_price(entry_price, operation.stop_gain_percent, operation.side)

            distance_to_stop = None
            distance_to_target = None
            if stop_loss_price is not None:
                distance_to_stop = _calculate_distance_percent(stop_loss_price, current_price)
            if take_profit_price is not None:
                distance_to_target = _calculate_distance_percent(take_profit_price, current_price)

            status = "OPEN" if operation.status == "ACTIVE" else operation.status

            positions.append({
                "id": operation.id,
                "operation_id": operation.id,
                "symbol": symbol,
                "side": operation.side,
                "quantity": _format_decimal(quantity, QTY_QUANT),
                "entry_price": _format_decimal(entry_price, USD_QUANT),
                "current_price": _format_decimal(current_price, USD_QUANT),
                "unrealized_pnl": _format_decimal(unrealized_pnl, USD_QUANT),
                "unrealized_pnl_percent": _format_decimal(unrealized_pnl_percent, PERCENT_QUANT),
                "stop_loss": _format_decimal(stop_loss_price, USD_QUANT),
                "take_profit": _format_decimal(take_profit_price, USD_QUANT),
                "distance_to_stop_percent": _format_decimal(distance_to_stop, PERCENT_QUANT),
                "distance_to_target_percent": _format_decimal(distance_to_target, PERCENT_QUANT),
                "status": status,
            })

        return Response({"positions": positions})
    except Exception:
        return Response({"error": "Failed to get positions"}, status=500)
