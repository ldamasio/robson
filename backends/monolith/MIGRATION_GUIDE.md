🚀 Migration Guide - Robson Bot (Models)

Este guia descreve a migração dos models antigos para a nova estrutura organizada em `api/models`.

Status atual
- Concluído: `Symbol`, `Strategy`, `Order`, `Operation`, `Position`, `Trade` (refatorados com managers, propriedades calculadas e validações).
- Próximos: `TechnicalAnalysisInterpretation`, `TechnicalEvent`, `Argument`, `Reaseon → Reason` (rename), padrões de gráfico e indicadores estatísticos.

Antes de começar
- Caminho do manage.py: `backends/monolith/manage.py`.
- Banco: PostgreSQL via variáveis `RBS_PG_*` no `.env` de `backends/monolith/`.
- Rodar comandos a partir de `backends/monolith`.

1) Backup dos dados
```bash
cd backends/monolith
python manage.py dumpdata clients > backup_clients.json
python manage.py dumpdata api > backup_api.json
```

2) Estrutura de diretórios (já criada)
- `api/models/base.py`: mixins, bases, managers e choices.
- `api/models/trading.py`: models de trading refatorados.
- `api/models/__init__.py`: centraliza imports e mantém compatibilidade.
- `api/tests/test_models.py`: testes de regressão/contrato para os models.

3) Gerar e aplicar migrações
Você pode usar o wrapper `./bin/dj` (recomendado em dev) ou chamar `manage.py` diretamente.
```bash
cd backends/monolith
./bin/dj makemigrations api
./bin/dj migrate
# ou
python manage.py makemigrations api && python manage.py migrate
```
Notas importantes
- Evite `--fake-initial` a menos que você saiba que a migration inicial corresponde exatamente ao schema atual já existente no banco. Prefira corrigir o histórico ou criar migrations adicionais coerentes.

4) Rodar testes dos models
```bash
cd backends/monolith
./bin/dj test
```
Os testes cobrem: managers (`objects`/`active`), propriedades calculadas (ex.: `display_name`, `pair_display`, `win_rate`, `remaining_quantity`), validações (ex.: `stop_loss_price`), métodos (`mark_as_filled`, `update_performance`, `calculate_unrealized_pnl`, etc.).

5) Verificações pós-migração
- Imports funcionam: `from api.models import Symbol, Strategy, Order, Operation, Position, Trade`.
- Admin lista os models (arquivo `api/admin.py` já registra todos).
```bash
cd backends/monolith
python manage.py runserver
# Acesse /admin/
```
- Criar um registro básico no shell:
```bash
cd backends/monolith
python manage.py shell
```
```python
from clients.models import Client
from api.models import Symbol
client = Client.objects.first()
Symbol.objects.create(
    client=client,
    name="TESTUSDT",
    description="Test symbol",
    base_asset="TEST",
    quote_asset="USDT",
)
```

6) Migração de dados (quando necessário)
- Prefira uma migration de dados com `RunPython` para remapear/normalizar campos (ex.: uppercase de `name`, split em `base_asset`/`quote_asset`).
- Exemplo (esboço):
```python
from django.db import migrations

def split_symbol(apps, schema_editor):
    Symbol = apps.get_model('api', 'Symbol')
    for s in Symbol.objects.all():
        if not s.base_asset and not s.quote_asset and s.name and len(s.name) > 3:
            s.base_asset = s.name[:-4]
            s.quote_asset = s.name[-4:]
            s.save(update_fields=['base_asset', 'quote_asset'])

class Migration(migrations.Migration):
    dependencies = [('api', 'XXXX_previous')]
    operations = [migrations.RunPython(split_symbol)]
```

Novos recursos disponíveis
- Managers: `Symbol.active.all()`, `Symbol.objects.for_client(client_id)`, `Symbol.objects.active_for_client(client_id)`.
- Propriedades calculadas: `Symbol.display_name`, `Symbol.pair_display`, `Order.remaining_quantity`, `Order.fill_percentage`, `Position.is_long`, `Strategy.win_rate`, `Strategy.average_pnl_per_trade`.
- Validações: `Order.stop_loss_price` coerente com o `side` e `price`; normalização de uppercase em `Symbol`.
- Config JSON flexível: `Strategy.config` e `Strategy.risk_config` com helpers `get_config_value` e `get_risk_config_value`.

Pendências planejadas
- Corrigir o typo `Reaseon` → `Reason` via migration de rename.
- Migrar models de análise técnica, padrões de gráfico (Rectangle, Triangle, etc.) e indicadores (MA, RSI, MACD) com testes.
- Integrar metadados de símbolos/precisões a partir da API da Binance (`docs/vendor`) para popular/validar `min_qty`, `max_qty`, `tickSize`/`stepSize`.

Solução de problemas
- ImportError de models: verifique `api/models/__init__.py`.
- Conflitos de migração: revise histórico e dependências; evite `--fake-initial` sem necessidade.
- Models não aparecem no admin: confirme imports em `api/admin.py`.
- Testes falhando: alinhe campos/métodos aos contratos dos testes e gere novas migrations quando campos mudarem.

Observações
- Este guia reflete o estado atual do código e remove placeholders/duplicações. Ajuste conforme novas migrações sejam criadas.
