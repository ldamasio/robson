"""
Operations Command - Display operations with their movements.

Shows a complete view of trading operations with all associated movements
(trades, transfers, borrows, stop-losses, fees, etc.)

Usage:
    python manage.py operations                    # Show all operations
    python manage.py operations --open             # Show only open operations
    python manage.py operations --id OP-2024-001  # Show specific operation
    python manage.py operations --limit 5         # Show last 5 operations

The output format clearly shows the hierarchy:
    Operation (trade cycle) → Movements (atomic actions)
"""

from datetime import datetime
from decimal import Decimal

from django.core.management.base import BaseCommand
from django.db.models import Q

from api.models import Operation, Order, Trade
from api.models.margin import MarginPosition, MarginTransfer
from clients.models import Client


class Command(BaseCommand):
    help = 'Display operations with their movements in a beautiful CLI format'

    def add_arguments(self, parser):
        parser.add_argument(
            '--client-id',
            type=int,
            default=1,
            help='Client ID (default: 1)',
        )
        parser.add_argument(
            '--open',
            action='store_true',
            help='Show only open operations/positions',
        )
        parser.add_argument(
            '--closed',
            action='store_true',
            help='Show only closed operations/positions',
        )
        parser.add_argument(
            '--id',
            type=str,
            help='Show specific operation by ID',
        )
        parser.add_argument(
            '--limit',
            type=int,
            default=10,
            help='Maximum number of operations to show (default: 10)',
        )
        parser.add_argument(
            '--json',
            action='store_true',
            help='Output as JSON instead of formatted text',
        )

    def handle(self, *args, **options):
        client_id = options['client_id']
        show_open = options['open']
        show_closed = options['closed']
        operation_id = options['id']
        limit = options['limit']
        output_json = options['json']

        try:
            client = Client.objects.get(id=client_id)
        except Client.DoesNotExist:
            self.stderr.write(self.style.ERROR(f'Client {client_id} not found'))
            return

        if output_json:
            self._output_json(client, show_open, show_closed, operation_id, limit)
        else:
            self._output_formatted(client, show_open, show_closed, operation_id, limit)

    def _output_formatted(self, client, show_open, show_closed, operation_id, limit):
        """Output operations in beautiful CLI format."""
        
        # Get margin positions (these are our "operations" for now)
        positions = MarginPosition.objects.filter(client=client)
        
        if show_open:
            positions = positions.filter(status=MarginPosition.Status.OPEN)
        elif show_closed:
            positions = positions.exclude(status=MarginPosition.Status.OPEN)
        
        if operation_id:
            positions = positions.filter(
                Q(position_id__icontains=operation_id) | 
                Q(id=operation_id if operation_id.isdigit() else 0)
            )
        
        positions = positions.order_by('-created_at')[:limit]

        if not positions.exists():
            # Check if there are spot trades instead
            trades = Trade.objects.all().order_by('-entry_time')[:limit]
            if trades.exists():
                self._show_spot_trades(trades)
            else:
                self.stdout.write(self.style.WARNING('No operations found.'))
            return

        for position in positions:
            self._render_operation(position, client)
            self.stdout.write('')  # Blank line between operations

    def _render_operation(self, position, client):
        """Render a single operation with its movements."""
        
        # Header
        status_emoji = self._get_status_emoji(position.status)
        leverage_str = f"{position.leverage}x" if position.leverage else ""
        
        header = f"{status_emoji} OPERACAO: {position.position_id[:20]}... ({leverage_str} {position.side} {position.symbol})"
        
        width = 70
        
        self.stdout.write(self._box_top(width))
        self.stdout.write(self._box_line(header, width))
        self.stdout.write(self._box_separator(width))
        
        # Get movements for this position
        movements = self._get_movements(position, client)
        
        for movement in movements:
            line = self._format_movement(movement)
            self.stdout.write(self._box_line(line, width))
        
        # Summary line
        if position.status == MarginPosition.Status.OPEN:
            status_line = "          AGUARDANDO FECHAMENTO..."
        else:
            pnl = position.realized_pnl or Decimal('0')
            pnl_str = f"+${pnl:.2f}" if pnl >= 0 else f"-${abs(pnl):.2f}"
            status_line = f"          FECHADO: {pnl_str}"
        
        self.stdout.write(self._box_line(status_line, width))
        self.stdout.write(self._box_bottom(width))

    def _get_movements(self, position, client):
        """Get all movements related to a position."""
        movements = []
        
        # Get transfers for this position
        transfers = MarginTransfer.objects.filter(
            client=client,
            symbol=position.symbol,
            created_at__gte=position.created_at,
        ).order_by('created_at')
        
        for transfer in transfers:
            if transfer.direction == MarginTransfer.Direction.TO_MARGIN:
                emoji = "   "  # Transfer arrow
                type_str = "TRANSFER"
                description = f"{transfer.amount} {transfer.asset}    Spot -> Isolated"
            else:
                emoji = "   "
                type_str = "TRANSFER"
                description = f"{transfer.amount} {transfer.asset}    Isolated -> Spot"
            
            movements.append({
                'time': transfer.created_at,
                'emoji': emoji,
                'type': type_str,
                'description': description,
            })
        
        # Add entry order
        movements.append({
            'time': position.opened_at or position.created_at,
            'emoji': "  " if position.side == 'LONG' else "  ",
            'type': f"MARGIN_{position.side[:3]}",
            'description': f"{position.quantity} BTC @ ${position.entry_price:,.2f}",
        })
        
        # Add stop-loss if present
        if position.stop_price:
            movements.append({
                'time': position.opened_at or position.created_at,
                'emoji': "  ",
                'type': "STOP_LOSS",
                'description': f"Colocado @ ${position.stop_price:,.2f}",
            })
        
        # Sort by time
        movements.sort(key=lambda x: x['time'] if x['time'] else datetime.now())
        
        return movements

    def _format_movement(self, movement):
        """Format a single movement line."""
        time = movement['time']
        time_str = time.strftime('%H:%M:%S') if time else '        '
        emoji = movement['emoji']
        type_str = movement['type'].ljust(12)
        description = movement['description']
        
        return f"  {time_str}  {emoji} {type_str} {description}"

    def _show_spot_trades(self, trades):
        """Show spot trades when no margin positions exist."""
        width = 70
        
        self.stdout.write(self._box_top(width))
        self.stdout.write(self._box_line("   SPOT TRADES (sem operacoes de margin)", width))
        self.stdout.write(self._box_separator(width))
        
        for trade in trades:
            emoji = "  " if trade.side == 'BUY' else "  "
            time_str = trade.entry_time.strftime('%Y-%m-%d %H:%M') if trade.entry_time else ''
            symbol = trade.symbol.name if trade.symbol else 'BTCUSDC'
            line = f"  {time_str}  {emoji} {trade.side.ljust(6)} {trade.quantity} {symbol[:3]} @ ${trade.entry_price:,.2f}"
            self.stdout.write(self._box_line(line, width))
        
        self.stdout.write(self._box_bottom(width))

    def _get_status_emoji(self, status):
        """Get emoji for position status."""
        emojis = {
            MarginPosition.Status.OPEN: "  ",
            MarginPosition.Status.CLOSED: "  ",
            MarginPosition.Status.STOPPED_OUT: "  ",
            MarginPosition.Status.LIQUIDATED: "  ",
        }
        return emojis.get(status, "  ")

    def _box_top(self, width):
        """Top border of box."""
        return "+" + "-" * (width - 2) + "+"

    def _box_bottom(self, width):
        """Bottom border of box."""
        return "+" + "-" * (width - 2) + "+"

    def _box_separator(self, width):
        """Separator line in box."""
        return "+" + "-" * (width - 2) + "+"

    def _box_line(self, content, width):
        """Content line in box, padded to width."""
        # Truncate if too long
        if len(content) > width - 4:
            content = content[:width - 7] + "..."
        
        padding = width - len(content) - 4
        return "| " + content + " " * padding + " |"

    def _output_json(self, client, show_open, show_closed, operation_id, limit):
        """Output operations as JSON."""
        import json
        
        positions = MarginPosition.objects.filter(client=client)
        
        if show_open:
            positions = positions.filter(status=MarginPosition.Status.OPEN)
        elif show_closed:
            positions = positions.exclude(status=MarginPosition.Status.OPEN)
        
        positions = positions.order_by('-created_at')[:limit]
        
        data = []
        for pos in positions:
            movements = self._get_movements(pos, client)
            
            data.append({
                'id': pos.position_id,
                'symbol': pos.symbol,
                'side': pos.side,
                'leverage': pos.leverage,
                'status': pos.status,
                'entry_price': str(pos.entry_price),
                'stop_price': str(pos.stop_price) if pos.stop_price else None,
                'quantity': str(pos.quantity),
                'risk_amount': str(pos.risk_amount),
                'risk_percent': str(pos.risk_percent),
                'movements': [
                    {
                        'time': m['time'].isoformat() if m['time'] else None,
                        'type': m['type'],
                        'description': m['description'],
                    }
                    for m in movements
                ],
            })
        
        self.stdout.write(json.dumps(data, indent=2))

