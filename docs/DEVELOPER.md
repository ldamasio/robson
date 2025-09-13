Robson Bot – Developer Guide

Overview
- Robson é open source. Este guia padroniza o ambiente de desenvolvimento local, ciclo de migrações/testes e boas práticas de contribuição, mantendo produção isolada (GitOps/CI/CD).

Project Layout (essencial)
- Backend (Django): `backends/monolith/`
  - `manage.py`, `backend/settings.py`
  - `api/models/` (modelos refatorados: `base.py`, `trading.py`)
  - `api/tests/test_models.py`
  - `docker-compose.dev.yml` (Postgres local para dev)
  - `bin/dj` (helper script para dev)
- Frontend (Vite/React): `frontends/web/`
- Docs: `docs/`
  - `DEVELOPER.md` (este arquivo)
  - `AUTH_FLOW.md`
  - `vendor/` (submódulos de referência, ex.: Binance)

Prerequisitos
- Python 3.12+
- Node.js (para o front; ver versões no projeto)
- Docker + Docker Compose
- Postgres client (opcional, para inspeção via psql)

Setup Rápido (backend)
1) Clone e prepare venv
```
cd backends/monolith
cp .env.development.example .env
python -m venv .venv
source .venv/bin/activate
python -m pip install -r requirements.txt
```
Importante: garanta que as chaves de testnet da Binance existam no `.env` (mesmo que dummy), pois o settings as lê sem default:
```
RBS_BINANCE_API_KEY_TEST=dev-test-api-key
RBS_BINANCE_SECRET_KEY_TEST=dev-test-secret-key
```
2) Suba Postgres local (Docker)
Execute este comando a partir da raiz do repositório:
```
# na raiz do repositório
make dev-db-up
```
Alternativa direta sem Makefile:
```
docker compose -f backends/monolith/docker-compose.dev.yml up -d
```

Subir Postgres a partir da raiz do projeto
```
# na raiz do repositório
make dev-db-up       # sobe o Postgres de desenvolvimento
make dev-db-logs     # acompanha logs do container
make dev-db-down     # para o container
make dev-db-destroy  # para e remove o volume de dados
```
Alternativa direta sem Makefile:
```
docker compose -f backends/monolith/docker-compose.dev.yml up -d
docker compose -f backends/monolith/docker-compose.dev.yml down
```

Reset rápido de ambiente (clean slate)
- Se você não precisa preservar dados e deseja evitar prompts interativos do `makemigrations`, use o reset completo de dev (apaga volume do Postgres e migrações do app `api`):
```
# na raiz do repositório
make dev-reset-api
```
O alvo executa:
- `docker compose down -v` e `up -d` do Postgres de dev
- remove todos os arquivos de migração em `api/migrations` (exceto `__init__.py`)
- recria e aplica as migrações com o estado atual dos models
Depois disso, rode os testes normalmente:
```
cd backends/monolith
./bin/dj test
```
3) Migre e rode testes com o helper script
```
chmod +x bin/dj
./bin/dj makemigrations api
./bin/dj migrate
./bin/dj test
```
4) Runserver
```
./bin/dj runserver
```

Helper Script `bin/dj`
- Objetivo: encurtar comandos e aplicar “guard rails” para evitar uso de DB de produção.
- Requer `.env` apontando para Postgres local (localhost).
- Comandos úteis:
  - `./bin/dj db:up | db:down | db:destroy` – controla o Postgres local via Makefile
  - `./bin/dj makemigrations [app]` – cria migrations
  - `./bin/dj migrate` – aplica migrations
  - `./bin/dj test` – roda testes de models do app `api`
  - `./bin/dj runserver` – sobe o servidor local

Banco de Dados (Dev vs Prod)
- Nunca use o banco de produção em desenvolvimento/testes.
- Em dev: use o Postgres local via `docker-compose.dev.yml` (porta 5432, bind em localhost). Variáveis em `.env`:
  - `RBS_PG_HOST=localhost`, `RBS_PG_PORT=5432`, `RBS_PG_DATABASE=robson_dev`, `RBS_PG_USER=robson`, `RBS_PG_PASSWORD=robson`
- Para resetar: `make dev-db-destroy` e reaplicar migrations.

Política de Migrações
- Prefira migrations explícitas a auto renames ambíguos.
- Quando renomear campos, use `migrations.RenameField` e, se necessário, complemente com `RunPython` para migração de dados.
- Evite `--fake-initial` salvo em cenários específicos e compreendidos.
- Em dev, se não houver dados valiosos, dropar o DB e recriar pode simplificar.

Testes
- Rodar testes do app `api`:
```
./bin/dj test
```
- O Django criará automaticamente um banco de testes temporário no Postgres local.
- Escreva testes focados por domínio (ex.: `tests/test_models.py`).

Frontend (rápido)
```
cd frontends/web
nvm use 14
npm i
npm start
```

Integrações e Docs de Terceiros
- Submódulos e materiais de referência vivem em `docs/vendor`.
- Para sincronizar docs Binance: `make sync-binance-docs` (ver Makefile).

Guia de Contribuição
- Workflow sugerido:
  - Fork → branch de feature → PR com escopo pequeno e testes.
  - Descreva o impacto (schema/migrações, endpoints, breaking changes) no corpo do PR.
- Código
  - Mantenha a organização por domínio (`api/models/trading.py`, etc.).
  - Reuso via mixins e managers comuns (`api/models/base.py`).
  - Evite acessar serviços externos em testes; use flags (ex.: `TRADING_ENABLED=False`).
- Migrations
  - Inclua migrations relevantes e, se houver dados, considere `RunPython` para manter compatibilidade.
- Segurança e Dados
  - Jamais inclua segredos em commits. Use `.env` local e mantenha `.env.example` atualizado.
- Produção
  - Deploys para produção são feitos via GitOps/CI (GitHub Actions + ArgoCD + k3s). Não use o `bin/dj` para prod.

Contato e Suporte
- Abra issues com reprodução clara, logs e versão do ambiente.
- PRs são bem-vindos! Consulte este guia antes de submeter.

Coding Style
- Filosofia
  - Código simples, legível e com responsabilidade única por módulo/objeto.
  - Use type hints quando ajudar clareza/manutenção.
  - Nomeie com consistência (`snake_case` em Python; `PascalCase` para classes; `UPPER_SNAKE_CASE` para constantes).
- Estrutura
  - Imports: padrão → terceiros → locais. Evite imports circulares.
  - Models: prefira mixins e bases comuns (`api/models/base.py`).
  - Views/APIs: endpoints consistentes (snake_case), separação por domínio.
- Docstrings & Comentários
  - Docstrings em funções/métodos públicos, breves e úteis.
  - Comentários para “por quê”, não “o quê” (o código deve explicar o quê).
- Ferramentas (opcionais, recomendadas)
  - Black (formatador), isort (organiza imports), Flake8 (lint), Mypy (tipos).
  - Instalação no venv de dev: `python -m pip install black isort flake8 mypy`
  - Comandos úteis (executar em backends/monolith):
    - `black .`
    - `isort .`
    - `flake8 api/ backends/monolith/backend/`
    - `mypy api/` (se tipos forem adotados nos módulos)
- Pre-commit (opcional)
  - `python -m pip install pre-commit && pre-commit install`
  - Exemplo de hooks: black, isort, flake8. Configure conforme seu fluxo.
