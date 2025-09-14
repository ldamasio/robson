🚀 Migration Guide – Robson Bot (Models)

This guide explains migrating legacy models to the organized structure under `api/models`.

Current Status
- Completed:
  - Trading: `Symbol`, `Strategy`, `Order`, `Operation`, `Position`, `Trade`
  - Technical Analysis: `TechnicalAnalysisInterpretation`, `TechnicalEvent`, `Argument`, `Reason`
  - Patterns: `Rectangle`, `Triangle`
  - Indicators: `MovingAverage`, `RSIIndicator`, `MACDIndicator`
- Facts: `Resistance`, `Support`, `Line`, `TrendLine`, `Channel`, `Accumulation`, `Sideways`, `Breakout`, `Uptrend`, `Downtrend`
- Next (remaining):
  - Legacy rules/config/reports (evaluate migration or deprecation)

Before You Start
- `manage.py` path: `backends/monolith/manage.py`.
- Database: PostgreSQL via `RBS_PG_*` variables in `backends/monolith/.env`.
- Run commands from `backends/monolith`.

1) Back up data
```bash
cd backends/monolith
python manage.py dumpdata clients > backup_clients.json
python manage.py dumpdata api > backup_api.json
```

2) Directory structure (already created)
- `api/models/base.py`: mixins, bases, managers, choices.
- `api/models/trading.py`: refactored trading models.
- `api/models/__init__.py`: centralized imports for compatibility.
- `api/tests/test_models.py`: regression/contract tests for the models.

3) Generate and apply migrations
Use the `./bin/dj` wrapper (recommended for dev) or call `manage.py` directly.
```bash
cd backends/monolith
./bin/dj makemigrations api
./bin/dj migrate
# or
python manage.py makemigrations api && python manage.py migrate
```
Important notes
- Avoid `--fake-initial` unless the initial migration exactly matches the current DB schema. Prefer fixing history or adding consistent follow‑ups.

4) Run model tests
```bash
cd backends/monolith
./bin/dj test
```
Tests cover: managers (`objects`/`active`), computed properties (e.g., `display_name`, `pair_display`, `win_rate`, `remaining_quantity`), validations (e.g., `stop_loss_price`), methods (`mark_as_filled`, `update_performance`, `calculate_unrealized_pnl`, etc.).

5) Post‑migration checks
- Imports work: `from api.models import Symbol, Strategy, Order, Operation, Position, Trade`.
- Admin lists the models (file `api/admin.py` registers them).
```bash
cd backends/monolith
python manage.py runserver
# Visit /admin/
```
- Create a basic record in the shell:
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

6) Data migration (when needed)
- Prefer a `RunPython` data migration to remap/normalize fields (e.g., uppercase `name`, split into `base_asset`/`quote_asset`).
- Example (sketch):
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

7) Clean‑slate reset (development)
- If you do not need to preserve data and want to avoid interactive rename prompts:
```bash
# from repo root
make dev-reset-api
```
This drops the dev Postgres volume, removes `api` migrations (except `__init__.py`), then recreates and applies migrations from the current models.

8) Rollback & recovery
- Roll back a specific app (example):
```bash
./bin/dj migrate api 0001
```
- Roll back all apps to zero (dangerous):
```bash
./bin/dj migrate zero
```
- Restore from backup (if you exported fixtures):
```bash
./bin/dj loaddata backup_clients.json
./bin/dj loaddata backup_api.json
```

9) Post‑migration cleanup
- Once all legacy models are migrated, remove temporary imports from `api/models/__init__.py` that point to legacy `api/models.py`.
- Remove any unused legacy classes from `backends/monolith/api/models.py`.
- Search and replace old import paths in code/tests to the new locations (`api/models/trading.py`, etc.).

10) Environment prerequisites
- Ensure these variables exist in `backends/monolith/.env`:
  - `RBS_SECRET_KEY`, `DEBUG=True`
  - `RBS_PG_DATABASE`, `RBS_PG_USER`, `RBS_PG_PASSWORD`, `RBS_PG_HOST=localhost`, `RBS_PG_PORT=5432`
  - Optional for development: `BINANCE_USE_TESTNET=True`, `TRADING_ENABLED=False`, `RBS_BINANCE_API_KEY_TEST`, `RBS_BINANCE_SECRET_KEY_TEST`

11) CI notes (optional)
- A GitHub Actions workflow (`.github/workflows/backend-tests.yml`) runs migrations and tests on every push/PR. Keep the suite green before merging.

New features available
- Managers: `Symbol.active.all()`, `Symbol.objects.for_client(client_id)`, `Symbol.objects.active_for_client(client_id)`.
- Computed properties: `Symbol.display_name`, `Symbol.pair_display`, `Order.remaining_quantity`, `Order.fill_percentage`, `Position.is_long`, `Strategy.win_rate`, `Strategy.average_pnl_per_trade`.
- Validations: `Order.stop_loss_price` coherent with `side`/`price`; uppercase normalization in `Symbol`.
- Flexible JSON config: `Strategy.config` and `Strategy.risk_config` with helpers `get_config_value` and `get_risk_config_value`.

Planned
- Integrate symbol metadata from Binance (`docs/vendor`) to populate/validate `min_qty`, `max_qty`, `tickSize`/`stepSize`.

Archival Note
- Once legacy rules/config/reports are migrated or deprecated and all legacy imports removed,
  this guide can be archived to `docs/history/` and replaced by a short "Model Architecture Overview".

Troubleshooting
- Model ImportError: check `api/models/__init__.py`.
- Migration conflicts: review history and dependencies; avoid `--fake-initial` unless required.
- Models not visible in admin: confirm imports in `api/admin.py`.
- Failing tests: align fields/methods to test contracts and generate migrations when fields change.

Notes
- This guide reflects the current code state and removes placeholders/duplication. Adjust as new migrations are introduced.
