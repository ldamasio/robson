# Robson Bot - Production Dashboard Roadmap

**Status**: Bot em PRODUÇÃO com primeira operação ativa
**Urgency**: HIGH - Usuário precisa monitorar posição real em mercado
**Date**: 2025-12-22

---

## Current Production Status

### ✅ What's Live
- **Operation #1**: BTC Long (0.00033 BTC @ $88,837.92)
- **Stop Monitor**: K8s CronJob rodando a cada 1 minuto (dry-run)
- **Backend API**: 15+ endpoints funcionais
- **Authentication**: JWT com refresh automático
- **Risk Management**: Position sizing (1% rule), Stop monitor

### ❌ Critical Gaps
- **No visual monitoring** - Usuário não pode ver sua posição
- **No real-time updates** - Preços desatualizados
- **No operations history** - Sem histórico de trades
- **No charts** - Análise técnica impossível via UI
- **CLI incomplete** - Falta comandos de monitoramento

---

## User Story: Trader's Daily Workflow

**Usuário:** Trader operando BTC spot com Robson Bot
**Necessidades diárias:**

### Morning (Abertura do mercado)
1. ✅ Login no sistema
2. ❌ Ver dashboard com posições abertas
3. ❌ Verificar P&L overnight (não implementado)
4. ❌ Checar alertas/notificações (não implementado)
5. ❌ Analisar gráfico 15min para decisões (chart vazio)

### During Market Hours
1. ❌ Monitorar preço em tempo real
2. ❌ Acompanhar distância do stop-loss
3. ❌ Ver operações executadas automaticamente
4. ❌ Analisar suportes/resistências no chart
5. ❌ Criar nova operação com stop técnico (não integrado)

### End of Day
1. ❌ Revisar operações do dia
2. ❌ Calcular P&L realizado/não-realizado
3. ❌ Exportar relatório (não implementado)
4. ❌ Ajustar stops se necessário (não implementado)

**Conclusion**: 90% do workflow diário NÃO está implementado!

---

## Technical Debt Analysis

### CLI (50% Complete)
**Working:**
- `plan`, `validate`, `execute` - Core agentic workflow
- Backend integration via Django management commands
- JSON output for automation

**Missing (Priority Order):**
1. **P0** (Critical): `status`, `positions`, `account`
2. **P1** (High): `price`, `orders`, `history`
3. **P2** (Medium): `signals`, `analysis`, `exposure`

### Frontend (30% Complete)
**Working:**
- Authentication + JWT refresh
- Balance/Patrimony fetch
- Strategies list
- OHLCV dataframe (weekly)

**Missing (Priority Order):**
1. **P0** (Critical): Position display, Real-time price, Chart
2. **P1** (High): Operations history, Risk metrics
3. **P2** (Medium): Volume indicators, Trend analysis

### Backend (80% Complete)
**Working:**
- REST API endpoints
- WebSocket support (partially)
- Hexagonal architecture
- Stop monitor (CronJob)

**Missing:**
1. Real-time price streaming endpoint
2. Operations history endpoint
3. P&L calculation endpoint
4. Technical analysis endpoint (for charts)

---

## Phase-Based Implementation Plan

### Phase 0: Emergency Dashboard (Week 1)
**Goal**: Give user ability to monitor live position

**CLI:**
- [x] `robson positions` - List open positions with P&L
- [x] `robson price <symbol>` - Get current market price
- [x] `robson account` - View balance and exposure

**Frontend:**
- [x] Complete Position component (entry, current, P&L)
- [x] Real-time price display with formatting
- [x] Basic chart (candlestick 15min, last 100 candles)

**Backend:**
- [x] `GET /api/portfolio/positions/` - Active operations
- [x] `GET /api/market/price/<symbol>/` - Current price
- [ ] `GET /api/portfolio/summary/` - Portfolio overview

**Outcome**: Trader can see what they own and current status

---

### Phase 1: Complete Monitoring (Week 2)
**Goal**: Full visibility of trading operations

**CLI:**
- [ ] `robson status <operation-id>` - Operation details
- [ ] `robson history --days 7` - Recent operations
- [ ] `robson orders --status open` - Open orders

**Frontend:**
- [ ] Operations tab with trade history table
- [ ] Risk component with live metrics
- [ ] WebSocket integration for price updates
- [ ] Order status tracking

**Backend:**
- [ ] `GET /api/operations/history/` - Historical operations
- [ ] `GET /api/orders/` - Order history with filters
- [ ] `GET /api/risk/metrics/` - Real-time risk metrics
- [ ] WebSocket endpoint for price streaming

**Outcome**: Complete audit trail and real-time awareness

---

### Phase 2: Technical Analysis Integration (Week 3)
**Goal**: Integrate technical stop rule into user workflow

**CLI:**
- [ ] `robson analyze <symbol>` - Show supports/resistances
- [ ] `robson plan buy <symbol>` - Auto-calculate technical stop
- [ ] `robson signals` - View generated signals

**Frontend:**
- [ ] Chart with support/resistance levels overlay
- [ ] Entry form with calculated stop + position size
- [ ] Visual representation of 1% risk rule
- [ ] Strategy selection with risk config display

**Backend:**
- [ ] `POST /api/trade/calculate-entry/` - Calculate stop + size
- [ ] `GET /api/analysis/levels/<symbol>/` - Support/resistance
- [ ] `GET /api/signals/` - Trading signals
- [ ] Integrate TechnicalStopCalculator into plan workflow

**Outcome**: User creates operations with proper risk management

---

### Phase 3: Advanced Features (Week 4)
**Goal**: Professional-grade trading platform

**CLI:**
- [ ] `robson backtest <strategy>` - Strategy backtesting
- [ ] `robson exposure` - Portfolio exposure analysis
- [ ] `robson alerts` - Configure price alerts

**Frontend:**
- [ ] Multiple timeframe charts (1m, 5m, 15m, 1h, 4h, 1d)
- [ ] Technical indicators (MA, RSI, MACD)
- [ ] Performance analytics dashboard
- [ ] Alert management interface

**Backend:**
- [ ] `GET /api/analytics/performance/` - Performance metrics
- [ ] `POST /api/alerts/` - Create price alerts
- [ ] `GET /api/backtest/` - Backtesting engine
- [ ] `GET /api/analytics/exposure/` - Risk concentration

**Outcome**: Professional trading platform ready for scaling

---

## Technology Stack Decisions

### Charts
**Decision**: Use **TradingView Lightweight Charts**
**Rationale**:
- Industry standard for trading UIs
- High performance (handles 10k+ candles)
- Built-in technical indicators
- Customizable overlays (support/resistance)
- MIT license (free)

**Alternative**: Recharts (simpler but less powerful)

### Real-Time Data
**Decision**: **WebSocket** with Server-Sent Events (SSE) fallback
**Implementation**:
- Backend: Django Channels for WebSocket
- Frontend: `react-use-websocket` (already installed)
- Protocol: JSON messages with event types

**Message Format**:
```json
{
  "type": "price_update",
  "symbol": "BTCUSDC",
  "price": 89245.50,
  "timestamp": 1703251200
}
```

### State Management (Frontend)
**Decision**: **Context API + useReducer** (no Redux yet)
**Rationale**:
- Already using AuthContext
- Simpler than Redux for current scale
- Can migrate to Redux later if needed

**State Slices**:
- `PositionsContext` - Active positions
- `PricesContext` - Real-time prices
- `OperationsContext` - Operations history

### CLI Enhancement
**Decision**: Keep **Cobra framework**, add subcommands
**New command structure**:
```
robson
├── plan/validate/execute  (existing)
├── positions             (new)
│   ├── list
│   ├── show <id>
│   └── close <id>
├── market                (new)
│   ├── price <symbol>
│   ├── ticker <symbol>
│   └── depth <symbol>
├── account               (new)
│   ├── balance
│   ├── portfolio
│   └── exposure
└── operations            (new)
    ├── list
    ├── show <id>
    └── history
```

---

## Backend API Design

### New Endpoints Required

**Portfolio/Positions:**
```
GET  /api/portfolio/summary/           # Balance + total value
GET  /api/portfolio/positions/         # Active positions with P&L
GET  /api/portfolio/positions/{id}/    # Single position details
POST /api/portfolio/positions/{id}/close/  # Close position
```

**Operations:**
```
GET  /api/operations/active/           # Active operations only
GET  /api/operations/history/          # Historical operations
GET  /api/operations/{id}/             # Operation details
GET  /api/operations/{id}/trades/      # Trades for operation
```

**Market Data:**
```
GET  /api/market/price/{symbol}/       # Current price (bid/ask/last)
GET  /api/market/ticker/{symbol}/      # 24h ticker stats
GET  /api/market/depth/{symbol}/       # Order book depth
WS   /ws/prices/                       # Real-time price stream
```

**Risk & Analysis:**
```
GET  /api/risk/metrics/                # Portfolio risk metrics
POST /api/trade/calculate-entry/       # Calculate stop + position size
GET  /api/analysis/levels/{symbol}/    # Support/resistance levels
GET  /api/analysis/indicators/{symbol}/ # Technical indicators
```

**Orders:**
```
GET  /api/orders/                      # Order history (filterable)
GET  /api/orders/{id}/                 # Order details
POST /api/orders/{id}/cancel/          # Cancel order
```

---

## Critical Business Rules (Must Enforce)

### 1. Technical Stop Rule (CORE)
**Rule**: ALL operations MUST calculate technical stop BEFORE entry
**Enforcement**:
- CLI `plan` command auto-calculates stop from 15min chart
- Frontend entry form REQUIRES stop confirmation
- Backend validates stop exists in validation step
- Reject plan if stop not set or invalid

**Implementation**:
```python
# In validate_plan.py
if not plan.stop_loss_price:
    raise ValidationError("Technical stop is mandatory")

if plan.stop_loss_price >= plan.entry_price (for LONG):
    raise ValidationError("Stop must be below entry for LONG")
```

### 2. Position Size Rule (CORE)
**Rule**: Position size MUST be calculated from stop distance (1% risk)
**Enforcement**:
- CLI shows calculated size, user can only accept/reject
- Frontend auto-calculates and displays
- Backend recalculates and validates in execution step

**Implementation**:
```python
# In execute_plan.py
calculated_size = calculate_position_size(
    capital=account.balance,
    entry_price=plan.entry_price,
    stop_price=plan.stop_loss_price,
    max_risk_percent=0.01
)

if abs(plan.quantity - calculated_size) > tolerance:
    raise ValidationError("Position size must match risk calculation")
```

### 3. Pre-Entry Validation (CORE)
**Rule**: NO execution without validation passing
**Enforcement**:
- CLI `execute` requires `--validated` flag (set by validate command)
- Backend checks validation_passed flag
- Frontend shows validation report before execute button enables

---

## Implementation Priorities (Next Sprint)

### Week 1 Tasks (Emergency Dashboard)

**Backend** (3 days):
1. Create `/api/portfolio/positions/` endpoint
2. Create `/api/operations/active/` endpoint
3. Create `/api/market/price/{symbol}/` endpoint
4. Add P&L calculation to Position model

**CLI** (2 days):
1. Implement `robson positions` command
2. Implement `robson price <symbol>` command
3. Implement `robson account` command

**Frontend** (5 days):
1. Complete Position component (fetch + display + P&L)
2. Implement real-time price display
3. Integrate TradingView Lightweight Charts
4. Connect WebSocket for price updates

**Testing** (2 days):
1. API endpoint tests
2. CLI command tests
3. Frontend component tests

**Total**: 12 developer-days (~2 weeks with 1 dev)

---

## Success Metrics

### Phase 0 (Week 1)
- [ ] User can view active positions from CLI
- [ ] User can view active positions from UI
- [ ] User can see real-time price updates
- [ ] User can see basic 15min chart

### Phase 1 (Week 2)
- [ ] User can view complete operations history
- [ ] User can track order status
- [ ] User sees live risk metrics
- [ ] WebSocket price updates working

### Phase 2 (Week 3)
- [ ] User creates operations with technical stop
- [ ] Chart shows support/resistance levels
- [ ] Position size auto-calculated
- [ ] Entry form validates 1% rule

### Phase 3 (Week 4)
- [ ] Multiple timeframe charts working
- [ ] Technical indicators displayed
- [ ] Performance analytics available
- [ ] Alert system functional

---

## Risk Mitigation

### Technical Risks
- **WebSocket scalability**: Use Redis pub/sub for multi-instance
- **Chart performance**: Lazy load historical candles
- **Real-time accuracy**: Cache with TTL, fallback to HTTP polling

### Business Risks
- **Missing stop-loss**: Enforce at all layers (CLI, API, DB)
- **Incorrect position sizing**: Recalculate in backend always
- **Execution without validation**: Hard checks, no bypasses

### Operational Risks
- **Production downtime**: Deploy in maintenance windows
- **Data loss**: Backup before schema changes
- **User confusion**: Progressive rollout, documentation

---

## Documentation Updates Required

1. **User Guide**: Add "Monitoring Your Positions" section
2. **CLI Reference**: Document all new commands
3. **API Docs**: OpenAPI spec for new endpoints
4. **Developer Guide**: WebSocket integration guide
5. **Architecture**: Update diagrams with dashboard components

---

## Next Immediate Actions (Today)

1. ✅ Create this roadmap document
2. [ ] Update CLAUDE.md with dashboard context
3. [ ] Create GitHub issues for Phase 0 tasks
4. [ ] Set up project board (TODO/IN PROGRESS/DONE)
5. [ ] Spike: TradingView chart integration (2h)
6. [ ] Spike: WebSocket price streaming (2h)

---

**Status**: Ready for implementation
**Owner**: Development team
**Timeline**: 4 weeks to professional-grade dashboard
**Blocker**: None - all dependencies satisfied
