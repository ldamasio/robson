🚀 Migration Guide - Robson Bot Models
This guide provides step-by-step migration from old models to the new organized structure.

📋 Migration Status
✅ Completed
Symbol - Refactored with significant improvements
Order - Completely rewritten (fixed typo symbol_orderd)
Strategy - Enhanced with flexible JSON configurations
Operation - Expanded functionality
Position - NEW - Position management
Trade - NEW - Trade history
🔄 Next in Queue
TechnicalAnalysisInterpretation
TechnicalEvent
Argument
Reaseon → Reason (fix typo)
📋 Planned
Chart Patterns (Rectangle, Triangle, etc.)
Statistical Indicators (MA, RSI, MACD, etc.)
Risk Management Rules
Configuration Models
🛠️ Implementation Steps
1. Backup Existing Data
￼
bash
# Backup before any migration
python manage.py dumpdata api > backup_api_models.json
python manage.py dumpdata clients > backup_clients.json
2. Create Directory Structure
￼
bash
mkdir -p api/models
mkdir -p api/tests
3. Implement Files
a) Create api/models/base.py
￼
python
# Copy content from "models_base" artifact
b) Create api/models/trading.py
￼
python
# Copy content from "models_trading" artifact
c) Create api/models/init.py
￼
python
# Copy content from "models_init" artifact
d) Create api/tests/test_models.py
￼
python
# Copy content from "models_tests" artifact
4. Run Tests
￼
bash
# Test if everything is working
python manage.py test api.tests.test_models -v 2
5. Generate and Apply Migrations
￼
bash
# Generate migrations for new models
python manage.py makemigrations api

# Check generated migrations
python manage.py showmigrations

# Apply migrations
python manage.py migrate
6. Migrate Existing Data (if any)
￼
python
# Data migration script - create if needed
# python manage.py shell

from api.models import Symbol as OldSymbol
from api.models.trading import Symbol as NewSymbol

# Migrate existing data if necessary
for old_symbol in OldSymbol.objects.all():
    # Migration logic
    pass
🔍 Post-Migration Checks
1. Verify Imports
￼
python
# These imports should work:
from api.models import Symbol, Strategy, Order, Operation, Position, Trade

# Check if admin still works
python manage.py runserver
# Access /admin/ and verify models appear
2. Verify Existing Functionality
￼
python
# Test record creation
python manage.py shell

from clients.models import Client
from api.models import Symbol, Strategy

client = Client.objects.first()
symbol = Symbol.objects.create(
    client=client,
    name="TESTUSDT",
    description="Test symbol",
    base_asset="TEST",
    quote_asset="USDT"
)

print("✅ Symbol creation working!")
3. Verify Views
￼
bash
# Test existing endpoints
curl http://localhost:8000/api/strategies/
# Should return JSON without errors
🆕 New Features Available
1. Custom Managers
￼
python
# Only active records
Symbol.active.all()

# Records by client
Symbol.objects.for_client(client_id)
Symbol.objects.active_for_client(client_id)
2. Calculated Properties
￼
python
# Symbol
symbol.display_name  # Uppercase name
symbol.pair_display  # "BTC/USDT"

# Strategy  
strategy.win_rate  # Win rate
strategy.average_pnl_per_trade  # Average P&L

# Order
order.remaining_quantity  # Remaining quantity
order.fill_percentage  # % filled
order.is_active  # If can be executed

# Position
position.is_long  # If it's a buy position
position.unrealized_pnl  # Unrealized P&L
3. Automatic Validations
￼
python
# Validations are executed automatically
order = Order(
    symbol=symbol,
    side='BUY',
    quantity=Decimal('0.1'),
    price=Decimal('50000'),
    stop_loss_price=Decimal('55000')  # Invalid!
)

order.save()  # Will generate ValidationError
4. Flexible JSON Configurations
￼
python
# Strategy with flexible configurations
strategy = Strategy.objects.create(
    name="My Strategy",
    config={
        "indicators": {
            "sma_fast": 10,
            "sma_slow": 30,
            "rsi_period": 14
        },
        "entry_conditions": [
            {"type": "crossover", "indicators": ["sma_fast", "sma_slow"]},
            {"type": "threshold", "indicator": "rsi", "value": 30}
        ]
    },
    risk_config={
        "max_position_size": 0.02,
        "stop_loss_pct": 0.03,
        "take_profit_pct": 0.06
    }
)

# Access configurations
sma_period = strategy.get_config_value("indicators.sma_fast")
max_position = strategy.get_risk_config_value("max_position_size")
⚠️ Potential Issues and Solutions
1. Import Error
￼
python
# Problem: ImportError: cannot import name 'Symbol'
# Solution: Check if __init__.py is correct
2. Migration Error
￼
bash
# Problem: Migration conflicts
# Solution: 
python manage.py migrate --fake-initial
3. Admin Doesn't Appear
￼
python
# Problem: Models don't appear in admin
# Solution: Check if admin.py is importing correctly
from api.models import Symbol, Strategy, Order
4. Tests Fail
￼
bash
# Problem: Old tests may fail
# Solution: Update imports in existing tests
🎯 Next Steps
Week 1: Foundation
✅ Implement base.py + trading.py
✅ Run tests
✅ Apply migrations
✅ Verify existing functionality
Week 2: Expansion
Migrate TechnicalAnalysis models
Fix typos (Reaseon → Reason)
Implement Chart Patterns
Add tests for new models
Week 3: Indicators
Implement Statistical Indicators
Create models for MA, RSI, MACD, etc.
Add automatic calculations
Integrate with market data
Week 4: Finalization
Migrate rules and configurations
Optimize performance
Document APIs
Prepare for production
📚 Additional Documentation
Django Models Best Practices
Django Migrations
Testing Django Models
🆘 Support
If you encounter problems during migration:

Check error logs
Run specific tests
Consult Django documentation
Revert to backup if necessary
Remember: This migration is incremental and safe. Each step is designed to maintain compatibility with existing code. Migração**

￼
bash
# Problema: Conflito de migrações
# Solução: 
python manage.py migrate --fake-initial
3. Admin Não Aparece
￼
python
# Problema: Models não aparecem no admin
# Solução: Verificar se admin.py está importando corretamente
from api.models import Symbol, Strategy, Order
4. Testes Falham
￼
bash
# Problema: Testes antigos podem falhar
# Solução: Atualizar imports nos testes existentes
🎯 Próximos Passos
Semana 1: Fundação
✅ Implementar base.py + trading.py
✅ Executar testes
✅ Aplicar migrações
✅ Verificar funcionalidades existentes
Semana 2: Expansão
Migrar TechnicalAnalysis models
Corrigir typos (Reaseon → Reason)
Implementar Chart Patterns
Adicionar testes para novos models
Semana 3: Indicadores
Implementar Statistical Indicators
Criar models para MA, RSI, MACD, etc.
Adicionar cálculos automáticos
Integrar com market data
Semana 4: Finalização
Migrar rules e configurations
Otimizar performance
Documentar APIs
Preparar para produção
📚 Documentação Adicional
Django Models Best Practices
Django Migrations
Testing Django Models
🆘 Suporte
Se encontrar problemas durante a migração:

Verificar logs de erro
Executar testes específicos
Consultar documentação Django
Reverter para backup se necessário
Lembre-se: Esta migração é incremental e segura. Cada passo foi projetado para manter compatibilidade com código existente.
