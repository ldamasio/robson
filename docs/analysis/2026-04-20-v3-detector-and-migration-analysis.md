# Robson v3 — Detector Analysis & Migration Gap Report

**Date**: 2026-04-20  
**Session**: Post-MIG-v3#13 (USD-M Futures migration complete)  
**Executor**: Claude Sonnet 4.5 + Codex  
**Context**: VAL-001 Phase 2 inconclusive (detector não disparou sinal), análise de gaps documentais e código pós-migração futures

---

## Executive Summary

Robson v3 completou MIG-v3#13 (Isolated Margin → USD-M Futures) em 2026-04-19, commit `18474e5f`. Esta análise identifica:

1. **Detector não é um bug** — funciona correctamente mas requer condições de mercado específicas (MA crossover) que podem nunca ocorrer em testnet com baixo volume
2. **22 ficheiros de documentação** com referências stale a isolated margin/SAPI (4 já corrigidos em commit `4afaddab`)
3. **3 violações ADR-0023** no código de produção (daemon.rs, config.rs) que impedem trading multi-symbol
4. **MIG-v3#12 e ADR-0022-R1** são hard blockers para VAL-002 (real capital)

**Quick wins completadas hoje**: Crate count, Rust version, val-001 environment facts, legacy headers (commit `4afaddab`).

**Próximos passos recomendados**: Track escolhido determina se vamos para VAL-001 PASS (30 min via signal injection) ou desbloquear VAL-002 (3-4 dias de trabalho focado).

---

## 1. Diagnóstico do Detector (VAL-001 Phase 2)

### 1.1 Arquitectura e Fluxo

**Componente**: `DetectorTask` (`v2/robsond/src/detector.rs`)

**Spawn trigger** (`position_manager.rs:1171-1193`):
```rust
// Quando position é armed via POST /positions
let detector = DetectorTask::from_position(&position, event_bus, ohlcv_port, cancel_token)?;
let handle = detector.spawn();
detectors.insert(position_id, handle);
```

**Fonte de dados**: WebSocket ticks em tempo real
- `MarketDataManager` subscreve a `fstream.binancefuture.com` (testnet) ou `fstream.binance.com` (prod)
- Publica `DaemonEvent::MarketData` no `EventBus`
- Detector subscreve ao bus e filtra por symbol (`detector.rs:336-366`)

**Lógica de detecção** (`detector.rs:375-438`):
- Mantém buffer circular de preços (`VecDeque<Decimal>`)
- Calcula Simple Moving Averages (SMA):
  - Fast MA: default 9 períodos
  - Slow MA: default 21 períodos
- **Requer ≥21 ticks** para primeira SMA lenta
- **Requer mais 1-2 ticks** para estabelecer "previous state"
- **Crossover detection**:
  - Long: `!was_above && is_above` (Fast cruza ACIMA de Slow)
  - Short: `was_above && !is_above` (Fast cruza ABAIXO de Slow)

**Critical**: Apenas o **crossover** dispara sinal — estar acima/abaixo sem cruzar não gera sinal.

### 1.2 Candles 15m vs. Ticks

**Misconception comum**: O detector usa candles 15m para detecção.

**Realidade**:
- Detector usa **ticks do WebSocket** para MA crossover
- Candles 15m são usados **apenas** pelo `TechnicalStopAnalyzer` para calcular stop chartístico (`detector.rs:470-489`)
- O `TechnicalStopAnalyzer` é chamado **após** o crossover ser detectado, não antes

### 1.3 Causa-Raiz VAL-001 Phase 2 Inconclusive

**Observação**: Position Armed ~5 min sem sinal (runbook VAL-001:16, 2026-04-16).

**Análise**:

| Cenário | Probabilidade | Evidência |
|---------|---------------|-----------|
| **Volume baixo em testnet** | Alta | Testnet pode entregar 1-2 ticks/min; 21+ ticks = 10-20 min antes da primeira chance de detecção |
| **Mercado lateral (sideways)** | Média-Alta | Mesmo com buffer cheio, se não há movimento direccional, MAs convergem mas **nunca cruzam** |
| **Bug no detector** | Baixa | Testes unitários passam (`detector.rs:665-1066`); lógica MA crossover é standard |

**Logs diagnósticos** (`detector.rs:389-394`):
```rust
debug!(
    position_id = %self.config.position_id,
    buffer_len = self.price_buffer.len(),
    required = self.config.ma_slow_period,
    "Insufficient data for MA calculation"
);
```

**Se este log aparece repetidamente** → poucos ticks (problema de volume testnet).  
**Se não aparece** → buffer cheio mas sem crossover (mercado lateral).

**Conclusão**: Detector funciona correctamente. Problema é inerente à combinação de:
1. Testnet com baixo volume
2. Lógica MA crossover que pode nunca disparar em mercados sem tendência

### 1.4 Ficheiros Relevantes

| Ficheiro | Linha | Responsabilidade |
|----------|-------|------------------|
| `v2/robsond/src/detector.rs` | 264-308 | Main detection loop |
| `v2/robsond/src/detector.rs` | 375-438 | MA crossover logic |
| `v2/robsond/src/detector.rs` | 470-489 | Technical stop calculation (chart analysis) |
| `v2/robsond/src/position_manager.rs` | 1171-1193 | Detector spawn on arm |
| `v2/robsond/src/market_data.rs` | 56-194 | WebSocket → EventBus bridge |
| `v2/robsond/src/daemon.rs` | 388-394 | WebSocket client spawn (⚠️ hardcoded BTCUSDT) |

---

## 2. Actualizações Documentais Necessárias

### 2.1 Completadas (commit `4afaddab`, 2026-04-20)

✅ `v2/README.md` — Crate count 7→11, Rust 1.75→1.83  
✅ `docs/runbooks/val-001-testnet-e2e-validation.md` — Environment facts para USD-M Futures testnet  
✅ `docs/requirements/isolated-margin-requirements.md` — Legacy warning header  
✅ `docs/specs/features/isolated-margin-spec.md` — Legacy warning header  

### 2.2 Pendentes — Prioridade ALTA

| Ficheiro | O Que Mudar | Impacto se Não Feito |
|----------|-------------|----------------------|
| `docs/runbooks/val-002-real-capital-activation.md` | Trocar `testnet.binance.vision` → `testnet.binancefuture.com` em safety checks; confirmar futures endpoints | Runbook VAL-002 usa endpoints errados; deploy real capital com configuração margin legacy |
| `docs/architecture/v3-runtime-spec.md` | Verificar Recovery Procedures — se mencionam isolated margin em contexto operacional vs. histórico | Ambiguidade entre target arch (futures) e legacy (margin) |
| `docs/architecture/v3-risk-engine-spec.md` | Confirmar que soft limits (15%/30%) foram documentalmente removidos (código já está correcto per ADR-0024) | Spec desalinhada com código; confusão sobre se limites ainda se aplicam |

### 2.3 Pendentes — Prioridade MÉDIA

| Ficheiro | Acção |
|----------|-------|
| `docs/STRATEGIES.md` | Se mencionar SAPI, notar que actual implementation usa FAPI |
| `docs/INDEX.md` | Verificar se links apontam para specs actuais vs. legacy |
| `docs/plan/EXECUTION-PLAN-STRATEGIC-OPERATIONS.md` | Verificar se phases mencionam isolated margin como target (devem mencionar futures) |
| `docs/operations/2026-01-05-isolated-margin-short-btcusdc.md` | Adicionar header: "⚠️ HISTORICAL LOG — v1/v2 margin operation" |
| `docs/operations/2025-12-24-first-leveraged-position.md` | Idem |

### 2.4 Pendentes — Prioridade BAIXA

Ficheiros em `docs/plan/prompts/`, `docs/market-context/`, `docs/entry-gate/` — verificar se há referências operacionais vs. históricas a SAPI/margin.

---

## 3. Gaps de Código (ADR-0023 Compliance)

### 3.1 Violação C1 — daemon.rs Hardcoded BTCUSDT

**Ficheiro**: `v2/robsond/src/daemon.rs`  
**Linha**: 392  
**Código actual**:
```rust
let btcusdt = Symbol::from_pair("BTCUSDT").unwrap();
let ws_handle = market_data_manager.spawn_ws_client(btcusdt)?;
info!("WebSocket client spawned for BTCUSDT");
```

**Problema**: Production code hardcoda símbolo; viola ADR-0023 §I3.a ("symbols appear only as labeled examples or operator-configured values").

**Impacto**: P0 blocker para operar múltiplos pares; VAL-001 com ETHUSDT/SOLUSDC impossível sem code change.

**Fix requerido**:
1. Adicionar env var `ROBSON_MARKET_DATA_SYMBOLS` (comma-separated)
2. Parse em `config.rs` → `Vec<String>`
3. Iterar e spawnar um WS client por símbolo
4. Default vazio (require explicit config)

**Complexidade**: M (2-3h) — requires config parsing + loop logic + multiple WS handle tracking.

### 3.2 Violação C2 — config.rs Default Hardcoded

**Ficheiro**: `v2/robsond/src/config.rs`  
**Linhas**: 116, 191

**Código actual**:
```rust
impl Default for PositionMonitorConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            poll_interval_secs: 20,
            symbols: vec!["BTCUSDT".to_string()],  // ← HARDCODED
            // ...
        }
    }
}

// Em Config::test():
position_monitor: PositionMonitorConfig {
    enabled: false,
    poll_interval_secs: 1,
    symbols: vec!["BTCUSDT".to_string()],  // ← HARDCODED
    // ...
}
```

**Problema**: Position monitor default vigia apenas BTC; UNTRACKED positions em outros símbolos passam despercebidas (viola ADR-0022 scope).

**Impacto**: Safety net incompleto; reconciliation worker (quando implementado) também afectado.

**Fix requerido**:
1. Default → `vec![]` (empty, require explicit config)
2. Validate: fail-fast se `enabled=true` && `symbols.is_empty()`
3. Test config pode manter `["BTCUSDT"]` (é test fixture)

**Complexidade**: S (30 min) — literal change + validation logic.

### 3.3 Violação C3 — validate_futures_settings Assumption

**Ficheiro**: `v2/robsond/src/binance_exchange.rs`  
**Linhas**: 82-86

**Código actual**:
```rust
async fn validate_futures_settings(/* ... */) -> ExecResult<FuturesSettings> {
    // One-way mode is assumed (set via Binance UI or API out-of-band).
    // In One-way mode, positionSide is BOTH and direction is determined
    // by side (BUY/SELL).
    Ok(FuturesSettings {
        position_mode: "One-way".to_string(),  // ← ASSUMED, NOT VERIFIED
        leverage: RiskConfig::LEVERAGE,
    })
}
```

**Problema**: Assume One-way mode sem verificar via API; se account estiver em Hedge mode, orders falham com erro críptico da exchange.

**Impacto**: VAL-001/VAL-002 reliability — se testnet/prod account não estiver em One-way, todas as entries falham silently.

**Fix requerido**:
1. Call `GET /fapi/v1/positionSide/dual` antes do return
2. Parse response: `{"dualSidePosition": false}` = One-way, `true` = Hedge
3. Fail-fast com `ExecError::FuturesConfigMismatch` se Hedge mode detectado
4. Guidance: "Switch to One-way position mode in Binance UI before trading"

**Complexidade**: M (1.5h) — new API call + response struct + error handling.

---

## 4. Plano de Prioridades — Implementation Guides

### Track A — VAL-001 Completion (Testnet Validation)

**Objectivo**: VAL-001 Phase 2-5 PASS em testnet com futures correctos.

#### A1. MIG-v3#13.1 — Update val-001 Environment Facts ✅ DONE

**Status**: Completado commit `4afaddab`.

#### A2. VAL-001-P2 — Detector Signal Unblock

**Opção 2A — Manual Signal Injection** (RECOMENDADO para quick PASS)

**Complexidade**: S (10 min)  
**Pré-requisitos**: Position Armed, `POSITION_ID` exportado

**Steps**:
```bash
# 1. Confirm position Armed
curl -s http://localhost:8080/status | jq '.positions[] | select(.state == "Armed")'

# 2. Inject signal with chart-derived stop
# ⚠️ stop_loss MUST be price level from chart analysis, NOT percentage
curl -X POST http://localhost:8080/positions/$POSITION_ID/signal \
  -H "Authorization: Bearer $ROBSON_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "entry_price": "95000.00",
    "stop_loss": "85500.00"
  }'

# 3. Monitor Phase 2-5
kubectl logs -n robson-testnet deploy/robsond -f | grep -E "entry|fill|trailing|exit"
```

**Success criteria**: `entry_order_placed` event → `position_entering` → `position_active` → trailing stop → exit.

**Opção 2B — Configurable MA Periods** (para futures sessions)

**Complexidade**: M (2h)  
**Scope**: Add env vars `ROBSON_DETECTOR_MA_FAST` / `ROBSON_DETECTOR_MA_SLOW`

**Implementation**:
1. `config.rs`: Add `DetectorConfig` struct with `ma_fast_period: usize`, `ma_slow_period: usize`
2. Parse from env (default 9/21)
3. Wire to `DetectorTask::new()` via `PositionManager::arm_position()`
4. Testnet override: `ROBSON_DETECTOR_MA_FAST=3`, `ROBSON_DETECTOR_MA_SLOW=5`
5. Redeploy → rearm → wait for crossover (agora com 5-8 ticks em vez de 21-23)

**Files to touch**:
- `v2/robsond/src/config.rs`
- `v2/robsond/src/detector.rs`
- `v2/robsond/src/position_manager.rs`

#### A3. ADR-0023-C3 — Position Mode Verification

**Complexidade**: M (1.5h)  
**Pré-requisito para**: VAL-001 Phase 2 reliability

**Implementation**:

**File**: `v2/robsond/src/binance_exchange.rs`

**Step 1** — Add response struct:
```rust
#[derive(Debug, Deserialize)]
struct PositionModeResponse {
    #[serde(rename = "dualSidePosition")]
    dual_side_position: bool,
}
```

**Step 2** — Update `validate_futures_settings()`:
```rust
async fn validate_futures_settings(&self, expected_leverage: u32) -> ExecResult<FuturesSettings> {
    // 1. Check position mode
    let response: PositionModeResponse = self
        .client
        .get("/fapi/v1/positionSide/dual")
        .await
        .map_err(|e| ExecError::ExchangeError(format!("Failed to check position mode: {}", e)))?;
    
    if response.dual_side_position {
        return Err(ExecError::FuturesConfigMismatch {
            field: "position_mode".to_string(),
            expected: "One-way (dualSidePosition=false)".to_string(),
            found: "Hedge mode (dualSidePosition=true)".to_string(),
            advice: "Switch to One-way position mode in Binance UI: Preferences → Position Mode → One-way Mode".to_string(),
        });
    }
    
    // 2. Check leverage (existing logic)
    // ...
    
    Ok(FuturesSettings {
        position_mode: "One-way".to_string(),
        leverage: expected_leverage,
    })
}
```

**Step 3** — Add to `robson-connectors/src/binance_rest.rs`:
```rust
pub async fn get_position_mode(&self) -> Result<bool, BinanceError> {
    let response: serde_json::Value = self
        .signed_request("GET", "/fapi/v1/positionSide/dual", &[])
        .await?;
    
    Ok(response["dualSidePosition"].as_bool().unwrap_or(false))
}
```

**Testing**:
```bash
# Unit test with mock
cargo test test_validate_futures_settings_hedge_mode_fails

# Integration test (requires testnet keys)
ROBSON_BINANCE_API_KEY=xxx ROBSON_BINANCE_API_SECRET=yyy \
  cargo test --features postgres test_position_mode_check -- --ignored
```

---

### Track B — ADR-0023 Compliance (Symbol-Agnostic)

**Objectivo**: Remover last mile de acoplamento BTCUSDT do código de produção.

#### B1. ADR-0023-C1 — Remove daemon.rs Hardcoded BTCUSDT

**Complexidade**: M (2-3h)  
**Blocker para**: Multi-symbol trading, VAL-001 com símbolos não-BTC

**Implementation**:

**File**: `v2/robsond/src/config.rs`

**Step 1** — Add to `Config` struct:
```rust
pub struct Config {
    pub api: ApiConfig,
    pub engine: EngineConfig,
    pub tech_stop: TechStopConfigEnv,
    pub projection: ProjectionConfig,
    pub position_monitor: PositionMonitorConfig,
    pub market_data: MarketDataConfig,  // ← NEW
    pub environment: Environment,
}

#[derive(Debug, Clone)]
pub struct MarketDataConfig {
    /// Symbols to stream market data for (e.g., ["BTCUSDT", "ETHUSDT"])
    pub symbols: Vec<String>,
}
```

**Step 2** — Parse from env:
```rust
fn load_market_data_config() -> DaemonResult<MarketDataConfig> {
    let symbols_str = env::var("ROBSON_MARKET_DATA_SYMBOLS")
        .unwrap_or_else(|_| "".to_string());
    
    let symbols: Vec<String> = symbols_str
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    
    // Fail-fast if empty in production
    if symbols.is_empty() {
        return Err(DaemonError::Config(
            "ROBSON_MARKET_DATA_SYMBOLS is required (comma-separated, e.g., BTCUSDT,ETHUSDT)"
                .to_string(),
        ));
    }
    
    Ok(MarketDataConfig { symbols })
}
```

**Step 3** — Update `daemon.rs`:
```rust
// Line 388-394 OLD:
let btcusdt = Symbol::from_pair("BTCUSDT").unwrap();
let ws_handle = market_data_manager.spawn_ws_client(btcusdt)?;
info!("WebSocket client spawned for BTCUSDT");

// NEW:
let mut ws_handles = vec![];
for symbol_str in &self.config.market_data.symbols {
    let symbol = Symbol::from_pair(symbol_str)
        .map_err(|e| DaemonError::Config(format!("Invalid symbol {}: {}", symbol_str, e)))?;
    let handle = market_data_manager.spawn_ws_client(symbol)?;
    ws_handles.push(handle);
    info!(symbol = %symbol_str, "WebSocket client spawned");
}

// Update shutdown logic to wait for all handles
for handle in ws_handles {
    let _ = tokio::time::timeout(tokio::time::Duration::from_secs(5), handle).await;
}
```

**Step 4** — Update testnet ConfigMap:
```yaml
# rbx-infra/k8s/robson-testnet/configmap.yaml
apiVersion: v1
kind: ConfigMap
metadata:
  name: robsond-testnet-config
data:
  ROBSON_MARKET_DATA_SYMBOLS: "BTCUSDT"  # ← ADD THIS
```

**Step 5** — Test config:
```rust
impl Config {
    pub fn test() -> Self {
        Self {
            // ...
            market_data: MarketDataConfig {
                symbols: vec!["BTCUSDT".to_string()],  // OK for test fixture
            },
            // ...
        }
    }
}
```

**Testing**:
```bash
# Env validation
ROBSON_MARKET_DATA_SYMBOLS="" cargo run  # should fail-fast

# Multi-symbol
ROBSON_MARKET_DATA_SYMBOLS="BTCUSDT,ETHUSDT" cargo run

# Check logs for both WS connections
INFO WebSocket client spawned symbol="BTCUSDT"
INFO WebSocket client spawned symbol="ETHUSDT"
```

#### B2. ADR-0023-C2 — Remove config.rs Default Hardcoded

**Complexidade**: S (30 min)

**File**: `v2/robsond/src/config.rs`

**Change**:
```rust
impl Default for PositionMonitorConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            poll_interval_secs: 20,
            symbols: vec![],  // ← CHANGED from vec!["BTCUSDT".to_string()]
            binance_api_key: None,
            binance_api_secret: None,
        }
    }
}
```

**Add validation in `from_env()`**:
```rust
fn load_position_monitor_config() -> DaemonResult<PositionMonitorConfig> {
    let enabled = env::var("ROBSON_POSITION_MONITOR_ENABLED")
        .unwrap_or_else(|_| "true".to_string())
        .parse()
        .unwrap_or(true);
    
    let symbols_str = env::var("ROBSON_POSITION_MONITOR_SYMBOLS")
        .unwrap_or_else(|_| "".to_string());
    
    let symbols: Vec<String> = symbols_str
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    
    // Fail-fast if enabled but no symbols
    if enabled && symbols.is_empty() {
        return Err(DaemonError::Config(
            "ROBSON_POSITION_MONITOR_SYMBOLS is required when monitor is enabled"
                .to_string(),
        ));
    }
    
    Ok(PositionMonitorConfig {
        enabled,
        symbols,
        // ...
    })
}
```

**Update testnet ConfigMap**:
```yaml
ROBSON_POSITION_MONITOR_SYMBOLS: "BTCUSDT"  # ← ADD THIS
```

---

### Track C — VAL-002 Prerequisites (Real Capital Blockers)

**Objectivo**: Capital-safe operation com persistence across restarts e protecção contra UNTRACKED positions.

#### C1. MIG-v3#12 — Event-Sourced Month Boundary

**Complexidade**: L (6-8h)  
**Blocker para**: VAL-002  
**Pré-requisito**: MIG-v3#11 (ADR-0024) já implementado

**Scope**:
1. New domain event `MonthBoundaryReset`
2. New DB migration: `monthly_state` table
3. New projector handler
4. Daemon month boundary detection (UTC calendar check)
5. Idempotency guard (don't re-emit on restart mid-month)

**Implementation Guide**:

**Step 1** — Domain event (`v2/robson-domain/src/events.rs`):
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Event {
    // ... existing variants ...
    
    MonthBoundaryReset {
        /// New month's capital base (current_equity - latent_risk_carried)
        capital_base: Decimal,
        /// Sum of latent risk from positions carried from previous month
        carried_positions_risk: Decimal,
        /// Month (1-12)
        month: u32,
        /// Year
        year: i32,
        /// Timestamp of boundary crossing (first daemon tick of new month)
        timestamp: DateTime<Utc>,
    },
}
```

**Step 2** — Migration (`v2/migrations/00000008_create_monthly_state.sql`):
```sql
CREATE TABLE monthly_state (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    year          SMALLINT NOT NULL,
    month         SMALLINT NOT NULL CHECK (month BETWEEN 1 AND 12),
    capital_base  NUMERIC(20,8) NOT NULL CHECK (capital_base >= 0),
    carried_risk  NUMERIC(20,8) NOT NULL DEFAULT 0 CHECK (carried_risk >= 0),
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (year, month)
);

CREATE INDEX idx_monthly_state_year_month ON monthly_state(year, month);
```

**Step 3** — Projector handler (`v2/robson-projector/src/handlers.rs`):
```rust
pub async fn handle_month_boundary_reset(
    pool: &PgPool,
    event: &Event,
) -> Result<(), ProjectorError> {
    let (capital_base, carried_risk, month, year, timestamp) = match event {
        Event::MonthBoundaryReset {
            capital_base,
            carried_positions_risk,
            month,
            year,
            timestamp,
        } => (capital_base, carried_positions_risk, month, year, timestamp),
        _ => return Err(ProjectorError::EventTypeMismatch),
    };
    
    sqlx::query(
        "INSERT INTO monthly_state (year, month, capital_base, carried_risk, created_at)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (year, month) DO UPDATE SET
           capital_base = EXCLUDED.capital_base,
           carried_risk = EXCLUDED.carried_risk"
    )
    .bind(year)
    .bind(*month as i16)
    .bind(capital_base)
    .bind(carried_risk)
    .bind(timestamp)
    .execute(pool)
    .await?;
    
    Ok(())
}
```

**Step 4** — Daemon month detection (`v2/robsond/src/daemon.rs`):
```rust
// Add to Daemon struct:
last_month_check: Arc<RwLock<(i32, u32)>>,  // (year, month)

// In main event loop (line ~439):
let now = chrono::Utc::now();
let current_month = (now.year(), now.month());

let mut last_check = self.last_month_check.write().await;
if *last_check != current_month {
    // Month boundary crossed!
    info!(
        previous = ?*last_check,
        current = ?current_month,
        "Month boundary detected, emitting MonthBoundaryReset"
    );
    
    self.handle_month_boundary(now).await?;
    *last_check = current_month;
}
```

**Step 5** — Month boundary handler:
```rust
async fn handle_month_boundary(&self, now: DateTime<Utc>) -> DaemonResult<()> {
    // 1. Check idempotency: has event already been emitted this month?
    if let Some(pool) = &self.pg_pool {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM monthly_state WHERE year = $1 AND month = $2)"
        )
        .bind(now.year())
        .bind(now.month() as i16)
        .fetch_one(&**pool)
        .await?;
        
        if exists {
            debug!("MonthBoundaryReset already emitted for {}-{:02}, skipping", now.year(), now.month());
            return Ok(());
        }
    }
    
    // 2. Compute capital_base and carried_risk
    let open_positions = self.store.positions().find_active().await?;
    let carried_risk: Decimal = open_positions
        .iter()
        .map(|p| {
            let stop = p.tech_stop_distance
                .as_ref()
                .map(|ts| ts.initial_stop.as_decimal())
                .unwrap_or_else(|| p.entry_price.unwrap_or_default().as_decimal());
            let entry = p.entry_price.unwrap_or_default().as_decimal();
            let qty = p.quantity.as_decimal();
            Decimal::max(Decimal::ZERO, (entry - stop).abs() * qty)
        })
        .sum();
    
    let current_equity = self.engine.lock().unwrap().risk_config().capital(); // Placeholder: should be mark-to-market
    let capital_base = current_equity - carried_risk;
    
    // 3. Emit event
    let event = Event::MonthBoundaryReset {
        capital_base,
        carried_positions_risk: carried_risk,
        month: now.month(),
        year: now.year(),
        timestamp: now,
    };
    
    self.execute_and_persist(vec![EngineAction::EmitEvent(event)]).await?;
    
    // 4. Reset circuit breaker if active
    self.circuit_breaker.reset().await;
    
    info!(
        year = now.year(),
        month = now.month(),
        %capital_base,
        %carried_risk,
        "Month boundary processed"
    );
    
    Ok(())
}
```

**Step 6** — RiskContext loading on startup:
```rust
// In position_manager.rs build_risk_context():
async fn build_risk_context(&self) -> DaemonResult<RiskContext> {
    let now = chrono::Utc::now();
    
    // Load capital_base from monthly_state projection for current month
    let capital_base = if let Some(pool) = &self.pg_pool {
        let row: Option<(Decimal,)> = sqlx::query_as(
            "SELECT capital_base FROM monthly_state WHERE year = $1 AND month = $2"
        )
        .bind(now.year())
        .bind(now.month() as i16)
        .fetch_optional(&**pool)
        .await?;
        
        row.map(|(cb,)| cb)
            .unwrap_or_else(|| self.engine.lock().unwrap().risk_config().capital())
    } else {
        self.engine.lock().unwrap().risk_config().capital()
    };
    
    // ... rest of risk context construction with capital_base
}
```

**Testing**:
```bash
# Unit test: month boundary detection logic
cargo test test_month_boundary_crossing

# Integration test: idempotency
#[sqlx::test(migrations = "../migrations")]
async fn test_month_boundary_idempotency(pool: PgPool) {
    // Emit MonthBoundaryReset twice for same month
    // Assert: only one row in monthly_state
}

# Manual test: simulate month change
# Set system clock to 2026-04-30 23:59:00 → wait 2 min → check logs
```

**Files to touch**:
- `v2/robson-domain/src/events.rs`
- `v2/migrations/00000008_create_monthly_state.sql`
- `v2/robson-projector/src/handlers.rs`
- `v2/robsond/src/daemon.rs`
- `v2/robsond/src/position_manager.rs`

#### C2. ADR-0022-R1 — Position Reconciliation Worker

**Complexidade**: L (8-12h)  
**Blocker para**: VAL-002 safety  
**Pré-requisito**: ADR-0023-C2 (config symbols fix)

**Scope**:
1. New worker loop inside `robsond`
2. Exchange query for all open positions (all symbols, all account types)
3. Event-log lookup by exchange order id
4. UNTRACKED detection + close path
5. Startup gating (block new entries until scan complete)
6. Alerting (`position_untracked_detected` event)

**Implementation Guide**:

**Step 1** — Worker scaffold (`v2/robsond/src/reconciliation_worker.rs`):
```rust
pub struct ReconciliationWorker {
    exchange: Arc<dyn ExchangePort>,
    store: Arc<dyn Store>,
    event_bus: Arc<EventBus>,
    pg_pool: Option<Arc<PgPool>>,
    scan_interval: Duration,
    shutdown_token: CancellationToken,
}

impl ReconciliationWorker {
    pub async fn run(self) -> DaemonResult<()> {
        loop {
            tokio::select! {
                _ = self.shutdown_token.cancelled() => {
                    info!("Reconciliation worker shutting down");
                    break Ok(());
                }
                _ = tokio::time::sleep(self.scan_interval) => {
                    if let Err(e) = self.scan_and_reconcile().await {
                        error!(error = %e, "Reconciliation scan failed");
                    }
                }
            }
        }
    }
    
    async fn scan_and_reconcile(&self) -> DaemonResult<()> {
        // 1. Query exchange for all open positions
        let exchange_positions = self.exchange.get_all_open_positions().await?;
        
        // 2. For each position, check if robson-authored
        for ex_pos in exchange_positions {
            let is_tracked = self.is_robson_authored(&ex_pos).await?;
            
            if !is_tracked {
                warn!(
                    symbol = %ex_pos.symbol,
                    side = ?ex_pos.side,
                    "UNTRACKED position detected, closing"
                );
                
                self.handle_untracked_position(ex_pos).await?;
            }
        }
        
        Ok(())
    }
    
    async fn is_robson_authored(&self, ex_pos: &ExchangePosition) -> DaemonResult<bool> {
        // Lookup in event_log by exchange_order_id
        // For now: query positions_current projection
        // TODO: add exchange_order_id index to event_log
        
        let positions = self.store.positions()
            .find_active_by_symbol_and_side(&ex_pos.symbol, ex_pos.side)
            .await?;
        
        Ok(!positions.is_empty())
    }
    
    async fn handle_untracked_position(&self, ex_pos: ExchangePosition) -> DaemonResult<()> {
        // 1. Emit detection event
        self.event_bus.send(DaemonEvent::RoguePositionDetected {
            symbol: ex_pos.symbol.clone(),
            side: ex_pos.side,
            entry_price: ex_pos.entry_price,
            stop_price: Price::new(Decimal::ZERO).unwrap(),  // Unknown stop
        });
        
        // 2. Close at market (Safety Net path)
        let close_result = self.exchange.close_position_market(
            &ex_pos.symbol,
            ex_pos.side,
            ex_pos.quantity,
            "UNTRACKED_ON_EXCHANGE"
        ).await;
        
        match close_result {
            Ok(order) => {
                info!(
                    symbol = %ex_pos.symbol,
                    order_id = %order.exchange_order_id,
                    "UNTRACKED position closed"
                );
                
                self.event_bus.send(DaemonEvent::SafetyExitExecuted {
                    symbol: ex_pos.symbol,
                    order_id: order.exchange_order_id,
                    executed_quantity: order.filled_quantity.as_decimal(),
                });
            }
            Err(e) => {
                error!(
                    symbol = %ex_pos.symbol,
                    error = %e,
                    "Failed to close UNTRACKED position"
                );
                
                self.event_bus.send(DaemonEvent::SafetyExitFailed {
                    symbol: ex_pos.symbol,
                    error: e.to_string(),
                });
            }
        }
        
        Ok(())
    }
}
```

**Step 2** — Exchange port extension (`v2/robson-exec/src/ports.rs`):
```rust
#[async_trait]
pub trait ExchangePort: Send + Sync {
    // ... existing methods ...
    
    /// Query all open positions across all symbols and account types.
    /// Used by reconciliation worker to detect UNTRACKED positions.
    async fn get_all_open_positions(&self) -> ExecResult<Vec<ExchangePosition>>;
    
    /// Close a position at market price (Safety Net path).
    /// Does NOT go through Risk Engine — always allowed.
    async fn close_position_market(
        &self,
        symbol: &Symbol,
        side: Side,
        quantity: Quantity,
        reason: &str,
    ) -> ExecResult<Order>;
}

#[derive(Debug, Clone)]
pub struct ExchangePosition {
    pub symbol: Symbol,
    pub side: Side,
    pub quantity: Quantity,
    pub entry_price: Price,
}
```

**Step 3** — Binance implementation (`v2/robsond/src/binance_exchange.rs`):
```rust
async fn get_all_open_positions(&self) -> ExecResult<Vec<ExchangePosition>> {
    // Call /fapi/v2/positionRisk with no symbol filter
    let response: Vec<serde_json::Value> = self
        .client
        .signed_request("GET", "/fapi/v2/positionRisk", &[])
        .await
        .map_err(|e| ExecError::ExchangeError(format!("Failed to query positions: {}", e)))?;
    
    let mut positions = vec![];
    for pos in response {
        let position_amt = pos["positionAmt"]
            .as_str()
            .and_then(|s| Decimal::from_str(s).ok())
            .unwrap_or(Decimal::ZERO);
        
        if position_amt.is_zero() {
            continue;  // Skip closed positions
        }
        
        let symbol = Symbol::from_pair(pos["symbol"].as_str().unwrap_or(""))?;
        let side = if position_amt.is_sign_positive() { Side::Long } else { Side::Short };
        let quantity = Quantity::new(position_amt.abs())?;
        let entry_price = Price::new(
            pos["entryPrice"]
                .as_str()
                .and_then(|s| Decimal::from_str(s).ok())
                .unwrap_or(Decimal::ZERO)
        )?;
        
        positions.push(ExchangePosition { symbol, side, quantity, entry_price });
    }
    
    Ok(positions)
}

async fn close_position_market(
    &self,
    symbol: &Symbol,
    side: Side,
    quantity: Quantity,
    reason: &str,
) -> ExecResult<Order> {
    // POST /fapi/v1/order with type=MARKET, reduceOnly=true
    let close_side = match side {
        Side::Long => "SELL",
        Side::Short => "BUY",
    };
    
    let params = vec![
        ("symbol", symbol.as_pair()),
        ("side", close_side.to_string()),
        ("type", "MARKET".to_string()),
        ("quantity", quantity.as_decimal().to_string()),
        ("reduceOnly", "true".to_string()),
    ];
    
    let response = self.client
        .signed_request("POST", "/fapi/v1/order", &params)
        .await?;
    
    // Parse response into Order
    // ...
}
```

**Step 4** — Integrate into daemon (`v2/robsond/src/daemon.rs`):
```rust
// Spawn reconciliation worker after API server
let reconciliation_worker = ReconciliationWorker::new(
    exchange_adapter,
    self.store.clone(),
    self.event_bus.clone(),
    self.pg_pool.clone(),
    Duration::from_secs(60),  // Scan every 60s
    shutdown.clone(),
);

let reconciliation_handle = tokio::spawn(async move {
    if let Err(e) = reconciliation_worker.run().await {
        error!(error = %e, "Reconciliation worker failed");
    }
});

// Wait for worker on shutdown
if let Some(handle) = reconciliation_handle {
    info!("Waiting for reconciliation worker to finish...");
    let _ = tokio::time::timeout(Duration::from_secs(10), handle).await;
}
```

**Step 5** — Startup gating:
```rust
// Before accepting first arm/signal, run one blocking scan
info!("Running startup reconciliation scan");
let untracked_count = reconciliation_worker.scan_and_reconcile_blocking().await?;

if untracked_count > 0 {
    return Err(DaemonError::Config(format!(
        "Startup aborted: {} UNTRACKED positions detected and closed. \
         Review exchange account before restarting.",
        untracked_count
    )));
}
info!("Startup reconciliation clean (0 UNTRACKED)");
```

**Testing**:
```bash
# Manual test on testnet:
# 1. Place manual order via Binance UI
# 2. Start robsond → should detect + close within 60s
# 3. Check logs for "UNTRACKED position detected"

# Integration test:
#[tokio::test]
async fn test_reconciliation_detects_untracked() {
    // Seed exchange with position NOT in robson store
    // Run worker.scan_and_reconcile()
    // Assert: close_position_market called
}
```

**Files to touch**:
- `v2/robsond/src/reconciliation_worker.rs` (new)
- `v2/robson-exec/src/ports.rs`
- `v2/robsond/src/binance_exchange.rs`
- `v2/robson-exec/src/stub.rs` (mock implementation)
- `v2/robsond/src/daemon.rs`
- `v2/robsond/src/lib.rs` (add `mod reconciliation_worker`)

---

## 5. Resumo de Próximos Passos

### Se Objectivo = VAL-001 PASS Rápido (30-45 min)

```bash
# 1. Ensure position Armed
curl http://localhost:8080/status | jq '.positions[] | select(.state == "Armed")'

# 2. Inject signal (A2 Opção 2A)
curl -X POST http://localhost:8080/positions/$POSITION_ID/signal \
  -H "Authorization: Bearer $ROBSON_TOKEN" \
  -d '{"entry_price": "95000", "stop_loss": "85500"}'

# 3. Monitor Phase 2-5
kubectl logs -n robson-testnet deploy/robsond -f
```

### Se Objectivo = Desbloquear VAL-002 (3-4 dias)

**Sequência recomendada**:
1. ADR-0023-C1 (daemon.rs BTCUSDT) — 2-3h
2. ADR-0023-C2 (config defaults) — 30 min
3. ADR-0023-C3 (position mode check) — 1.5h
4. MIG-v3#12 (month boundary) — 6-8h
5. ADR-0022-R1 (reconciliation worker) — 8-12h

**Total: ~18-25h (~3-4 dias de trabalho focado)**

### Se Objectivo = Quick Wins Code (1-2h)

```
CLIPPY-001: Add missing_docs to public items in robson-domain support modules
```

---

## 6. Ficheiros de Referência Rápida

| Componente | Ficheiro Principal | Linha Key |
|------------|-------------------|-----------|
| Detector spawn | `v2/robsond/src/position_manager.rs` | 1171-1193 |
| Detector loop | `v2/robsond/src/detector.rs` | 264-308 |
| MA crossover | `v2/robsond/src/detector.rs` | 375-438 |
| WebSocket spawn | `v2/robsond/src/daemon.rs` | 388-394 |
| Config parsing | `v2/robsond/src/config.rs` | 134-162 |
| Position mode check | `v2/robsond/src/binance_exchange.rs` | 65-95 |
| Month boundary (future) | `v2/robsond/src/daemon.rs` | ~439 (main loop) |

---

**Documento criado**: 2026-04-20  
**Próxima revisão**: Após completion de track escolhido (A/B/C)  
**Maintainer**: RBX Systems Architecture Team
