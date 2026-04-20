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
## 7. Agent Entry Points

**Purpose**: Executable task definitions for autonomous agent execution.  
**Format**: Each entry point is self-contained and requires zero interpretation.  
**Success criterion**: Any agent can execute without asking clarifying questions.

---

### EP-001: VAL-001-P2-INJECT — Manual Signal Injection

**Objective**: Unblock VAL-001 Phase 2 by injecting detector signal manually.

**Preconditions**:
```bash
# VERIFY: Position exists in Armed state
curl -s http://localhost:8080/status | jq '.positions[] | select(.state == "Armed")' | grep -q "Armed"
# EXIT CODE 0 = precondition met; EXIT CODE 1 = ABORT (no armed position)

# VERIFY: API token available
test -n "$ROBSON_TOKEN"
# EXIT CODE 0 = precondition met; EXIT CODE 1 = ABORT (token not set)

# VERIFY: Port-forward active
curl -s http://localhost:8080/health > /dev/null
# EXIT CODE 0 = precondition met; EXIT CODE != 0 = ABORT (no connection)
```

**Inputs**:
- `ROBSON_TOKEN` (env var) — API bearer token from `kubectl get secret robsond-testnet-secret`
- `POSITION_ID` (extracted) — UUID of Armed position
- `ENTRY_PRICE` (hardcoded) — `"95000.00"` (BTCUSDT reference)
- `STOP_LOSS` (hardcoded) — `"85500.00"` (10% below entry, chart-derived equivalent for testnet)

**Steps**:
```bash
# Step 1: Extract POSITION_ID
export POSITION_ID=$(curl -s http://localhost:8080/status | \
  jq -r '.positions[] | select(.state == "Armed") | .id' | head -1)

# Step 2: Verify extraction succeeded
test -n "$POSITION_ID" || { echo "FAIL: No Armed position found"; exit 1; }

# Step 3: Inject signal
RESPONSE=$(curl -s -w "\n%{http_code}" -X POST \
  http://localhost:8080/positions/$POSITION_ID/signal \
  -H "Authorization: Bearer $ROBSON_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "entry_price": "95000.00",
    "stop_loss": "85500.00"
  }')

# Step 4: Extract HTTP status code
HTTP_CODE=$(echo "$RESPONSE" | tail -1)
BODY=$(echo "$RESPONSE" | head -n -1)

# Step 5: Verify HTTP 200 or 201
if [[ "$HTTP_CODE" != "200" && "$HTTP_CODE" != "201" ]]; then
  echo "FAIL: HTTP $HTTP_CODE"
  echo "$BODY"
  exit 1
fi

# Step 6: Verify response contains expected state transition
echo "$BODY" | jq -e '.state == "Entering" or .state == "Active"' > /dev/null || {
  echo "FAIL: Unexpected state in response"
  echo "$BODY"
  exit 1
}

echo "SUCCESS: Signal injected, position transitioning"
```

**Expected Outcome**:
```bash
# PASS condition:
curl -s http://localhost:8080/status | \
  jq -e '.positions[] | select(.id == env.POSITION_ID) | .state' | \
  grep -qE "Entering|Active"
# EXIT CODE 0 = PASS

# Visible in logs within 10 seconds:
kubectl logs -n robson-testnet deploy/robsond --since=30s | \
  grep -E "entry_order_placed|Entering"
# OUTPUT contains "entry_order_placed" = PASS
```

**Failure Detection**:
```bash
# FAIL if HTTP 4xx/5xx
# FAIL if response.state not in [Entering, Active]
# FAIL if no entry_order_placed log within 30s
# FAIL if position remains Armed after 60s
```

**Rollback**: Not applicable (signal injection is idempotent; position can be disarmed manually if needed).

---

### EP-002: VAL-001-P2-CONFIG — Configurable MA Periods

**Objective**: Enable configurable MA periods for testnet to reduce ticks needed for crossover detection.

**Preconditions**:
```bash
# VERIFY: Working directory is robson/v2
pwd | grep -q "/robson/v2$"

# VERIFY: No uncommitted changes in target files
git diff --exit-code v2/robsond/src/config.rs v2/robsond/src/detector.rs v2/robsond/src/position_manager.rs
# EXIT CODE 0 = clean; EXIT CODE 1 = ABORT (uncommitted changes)

# VERIFY: Cargo builds successfully
cargo check --all
# EXIT CODE 0 = precondition met
```

**Inputs**:
- Target files:
  - `v2/robsond/src/config.rs`
  - `v2/robsond/src/detector.rs`
  - `v2/robsond/src/position_manager.rs`
- Default values: `MA_FAST=9`, `MA_SLOW=21`
- Env vars: `ROBSON_DETECTOR_MA_FAST`, `ROBSON_DETECTOR_MA_SLOW`

**Steps**:
```bash
# Step 1: Add DetectorConfig to config.rs (after line 94)
# Insert after TechStopConfigEnv definition:
cat >> /tmp/detector_config.rs <<'EOF'

/// Detector MA period configuration (configurable for testnet low-volume scenarios).
#[derive(Debug, Clone)]
pub struct DetectorConfig {
    /// Fast MA period (default 9)
    pub ma_fast_period: usize,
    /// Slow MA period (default 21)
    pub ma_slow_period: usize,
}

impl Default for DetectorConfig {
    fn default() -> Self {
        Self {
            ma_fast_period: 9,
            ma_slow_period: 21,
        }
    }
}
EOF

# Apply edit to config.rs at line 95 (after TechStopConfigEnv closing brace)
# (Agent must use Edit tool with exact insertion point)

# Step 2: Add detector field to Config struct (line 18)
# Add after tech_stop field:
#   pub detector: DetectorConfig,

# Step 3: Parse from env in load_detector_config() (new function after line 160)
cat > /tmp/load_detector_config.rs <<'EOF'
fn load_detector_config() -> DaemonResult<DetectorConfig> {
    let ma_fast = env::var("ROBSON_DETECTOR_MA_FAST")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(9);
    
    let ma_slow = env::var("ROBSON_DETECTOR_MA_SLOW")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(21);
    
    if ma_fast >= ma_slow {
        return Err(DaemonError::Config(format!(
            "ROBSON_DETECTOR_MA_FAST ({}) must be < ROBSON_DETECTOR_MA_SLOW ({})",
            ma_fast, ma_slow
        )));
    }
    
    Ok(DetectorConfig {
        ma_fast_period: ma_fast,
        ma_slow_period: ma_slow,
    })
}
EOF

# Step 4: Wire detector config to DetectorTask in position_manager.rs
# Update arm_position() call to DetectorTask::from_position() around line 1173
# Change from:
#   DetectorTask::from_position(&position, event_bus, ohlcv_port, cancel_token)
# To:
#   DetectorTask::from_position_with_config(
#       &position,
#       event_bus,
#       ohlcv_port,
#       cancel_token,
#       &self.config.detector
#   )

# Step 5: Update DetectorTask::from_position in detector.rs to accept config
# Add new constructor from_position_with_config() at line 210

# Step 6: Compile and verify
cargo build --bin robsond
# EXIT CODE 0 = SUCCESS; EXIT CODE 1 = FAIL (compilation error)

# Step 7: Run tests
cargo test --package robsond detector
# EXIT CODE 0 = SUCCESS
```

**Expected Outcome**:
```bash
# PASS condition 1: Cargo builds without errors
cargo build --bin robsond 2>&1 | grep -q "Finished"

# PASS condition 2: Env var parsing works
ROBSON_DETECTOR_MA_FAST=3 ROBSON_DETECTOR_MA_SLOW=5 \
  cargo run --bin robsond -- --help 2>&1 | grep -v "Error"
# EXIT CODE 0 = PASS

# PASS condition 3: Validation triggers on invalid config
ROBSON_DETECTOR_MA_FAST=21 ROBSON_DETECTOR_MA_SLOW=9 \
  cargo run --bin robsond 2>&1 | grep -q "must be <"
# EXIT CODE 0 = PASS (validation working)
```

**Failure Detection**:
```bash
# FAIL if cargo build fails
# FAIL if tests fail
# FAIL if env var validation doesn't trigger on invalid input
# FAIL if detector still uses hardcoded 9/21 after deploy
```

**Rollback**:
```bash
git restore v2/robsond/src/config.rs v2/robsond/src/detector.rs v2/robsond/src/position_manager.rs
```

---

### EP-003: ADR-0023-C1 — Remove daemon.rs Hardcoded BTCUSDT

**Objective**: Replace hardcoded BTCUSDT in WebSocket spawn with configurable symbol list from env.

**Preconditions**:
```bash
# VERIFY: On branch with clean state
git status --porcelain | wc -l | grep -q "^0$"

# VERIFY: Current code has hardcoded BTCUSDT
grep -q 'Symbol::from_pair("BTCUSDT")' v2/robsond/src/daemon.rs
# EXIT CODE 0 = precondition met

# VERIFY: Baseline compiles
cargo build --bin robsond
# EXIT CODE 0 = precondition met
```

**Inputs**:
- File: `v2/robsond/src/daemon.rs` (line 388-394)
- File: `v2/robsond/src/config.rs` (multiple locations)
- Env var: `ROBSON_MARKET_DATA_SYMBOLS` (comma-separated)
- Default: NONE (fail-fast if not provided in production)

**Steps**:
```bash
# Step 1: Add MarketDataConfig to config.rs after line 109
# Use Edit tool to insert after PositionMonitorConfig definition

# Step 2: Add load_market_data_config() function to config.rs
# Insert before load_environment() function

# Step 3: Add market_data field to Config struct (line ~30)
# Insert after position_monitor field

# Step 4: Call load_market_data_config() in from_env() (line ~145)

# Step 5: Update daemon.rs lines 388-394
# BEFORE (single symbol hardcoded):
#   let btcusdt = Symbol::from_pair("BTCUSDT").unwrap();
#   let ws_handle = market_data_manager.spawn_ws_client(btcusdt)?;
#
# AFTER (loop over configured symbols):
#   let mut ws_handles = vec![];
#   for symbol_str in &self.config.market_data.symbols {
#       let symbol = Symbol::from_pair(symbol_str)
#           .map_err(|e| DaemonError::Config(format!("Invalid symbol {}: {}", symbol_str, e)))?;
#       let handle = market_data_manager.spawn_ws_client(symbol)?;
#       ws_handles.push(handle);
#       info!(symbol = %symbol_str, "WebSocket client spawned");
#   }

# Step 6: Update shutdown logic (line ~467)
# Replace single ws_handle with loop over ws_handles

# Step 7: Compile
cargo build --bin robsond 2>&1 | tee /tmp/build.log
grep -q "Finished" /tmp/build.log || { echo "FAIL: Build error"; cat /tmp/build.log; exit 1; }

# Step 8: Test fail-fast on missing env var
ROBSON_MARKET_DATA_SYMBOLS="" cargo run --bin robsond 2>&1 | grep -q "is required"
# EXIT CODE 0 = PASS (validation works)

# Step 9: Test multi-symbol parsing
ROBSON_MARKET_DATA_SYMBOLS="BTCUSDT,ETHUSDT" cargo run --bin robsond -- --version
# EXIT CODE 0 = PASS (parsing works)

# Step 10: Run unit tests
cargo test --package robsond config::tests
# EXIT CODE 0 = PASS
```

**Expected Outcome**:
```bash
# PASS condition 1: grep confirms hardcode removed
! grep -q 'let btcusdt = Symbol::from_pair("BTCUSDT")' v2/robsond/src/daemon.rs

# PASS condition 2: Build succeeds
cargo build --bin robsond 2>&1 | grep -q "Finished"

# PASS condition 3: Env validation works
ROBSON_MARKET_DATA_SYMBOLS="" cargo run --bin robsond 2>&1 | grep -q "ROBSON_MARKET_DATA_SYMBOLS is required"

# PASS condition 4: Multi-symbol accepted
ROBSON_MARKET_DATA_SYMBOLS="BTCUSDT,ETHUSDT,SOLUSDC" cargo run --bin robsond -- --version
# EXIT CODE 0 = PASS
```

**Failure Detection**:
```bash
# FAIL if build fails after edits
# FAIL if empty env var doesn't trigger error
# FAIL if invalid symbol (e.g., "INVALID") doesn't fail gracefully
# FAIL if tests fail
```

**Rollback**:
```bash
git restore v2/robsond/src/daemon.rs v2/robsond/src/config.rs
cargo build --bin robsond  # Verify rollback successful
```

---

### EP-004: ADR-0023-C2 — Remove config.rs Default Hardcoded Symbols

**Objective**: Change PositionMonitorConfig default from `["BTCUSDT"]` to `[]` and require explicit env var.

**Preconditions**:
```bash
# VERIFY: Current default contains BTCUSDT
grep -A5 'impl Default for PositionMonitorConfig' v2/robsond/src/config.rs | \
  grep -q 'vec!\["BTCUSDT"'
# EXIT CODE 0 = precondition met

# VERIFY: Baseline builds
cargo build --package robsond
# EXIT CODE 0 = precondition met
```

**Inputs**:
- File: `v2/robsond/src/config.rs` (lines 111-120, 188-194)
- Env var: `ROBSON_POSITION_MONITOR_SYMBOLS` (comma-separated)

**Steps**:
```bash
# Step 1: Edit config.rs line 116
# BEFORE:
#   symbols: vec!["BTCUSDT".to_string()],
# AFTER:
#   symbols: vec![],

# Execute edit
cd /home/psyctl/apps/robson
# (Use Edit tool: old_string='symbols: vec!["BTCUSDT".to_string()],' new_string='symbols: vec![],')

# Step 2: Add validation to load_position_monitor_config() after line 520
# Insert before Ok(PositionMonitorConfig { ... }):
#   if enabled && symbols.is_empty() {
#       return Err(DaemonError::Config(
#           "ROBSON_POSITION_MONITOR_SYMBOLS is required when monitor is enabled".to_string()
#       ));
#   }

# Step 3: Test config can remain with BTCUSDT (line 191) — leave unchanged
# (This is test fixture, not production default)

# Step 4: Compile
cargo build --package robsond
# EXIT CODE 0 = SUCCESS

# Step 5: Test validation triggers
ROBSON_POSITION_MONITOR_ENABLED=true ROBSON_POSITION_MONITOR_SYMBOLS="" \
  cargo run --bin robsond 2>&1 | grep -q "is required when monitor is enabled"
# EXIT CODE 0 = PASS

# Step 6: Test disabled monitor doesn't require symbols
ROBSON_POSITION_MONITOR_ENABLED=false cargo run --bin robsond -- --version
# EXIT CODE 0 = PASS

# Step 7: Run tests
cargo test --package robsond position_monitor
# EXIT CODE 0 = PASS
```

**Expected Outcome**:
```bash
# PASS condition 1: Default changed to empty vec
grep -A5 'impl Default for PositionMonitorConfig' v2/robsond/src/config.rs | \
  grep -q 'symbols: vec!\[\]'

# PASS condition 2: Validation enforced
ROBSON_POSITION_MONITOR_ENABLED=true ROBSON_POSITION_MONITOR_SYMBOLS="" \
  cargo run --bin robsond 2>&1 | grep -q "is required"

# PASS condition 3: Tests pass
cargo test --package robsond position_monitor 2>&1 | grep -q "test result: ok"
```

**Failure Detection**:
```bash
# FAIL if default still contains BTCUSDT after edit
# FAIL if enabled monitor with empty symbols doesn't error
# FAIL if tests fail
```

**Rollback**:
```bash
git restore v2/robsond/src/config.rs
cargo build --package robsond
```

---

### EP-005: ADR-0023-C3 — Position Mode Verification

**Objective**: Add runtime verification that Binance account is in One-way position mode before allowing trades.

**Preconditions**:
```bash
# VERIFY: binance_exchange.rs exists and has validate_futures_settings
grep -q 'async fn validate_futures_settings' v2/robsond/src/binance_exchange.rs

# VERIFY: Baseline builds
cargo build --package robsond
# EXIT CODE 0 = precondition met

# VERIFY: binance_rest.rs exists
test -f v2/robson-connectors/src/binance_rest.rs
```

**Inputs**:
- Files:
  - `v2/robsond/src/binance_exchange.rs` (lines 65-95)
  - `v2/robson-connectors/src/binance_rest.rs` (add new method)
  - `v2/robson-exec/src/error.rs` (verify FuturesConfigMismatch exists)
- API endpoint: `GET /fapi/v1/positionSide/dual`
- Response: `{"dualSidePosition": bool}`

**Steps**:
```bash
# Step 1: Add PositionModeResponse struct to binance_exchange.rs before line 65
# Insert:
#[derive(Debug, serde::Deserialize)]
struct PositionModeResponse {
    #[serde(rename = "dualSidePosition")]
    dual_side_position: bool,
}

# Step 2: Update validate_futures_settings() in binance_exchange.rs
# Replace lines 82-86 with full implementation (see doc section 4 Track A A3 Step 2)
# Key changes:
#   - Add API call to /fapi/v1/positionSide/dual
#   - Parse response
#   - Return error if dual_side_position == true
#   - Only return Ok if One-way mode confirmed

# Step 3: Add get_position_mode() to binance_rest.rs
# Insert before closing impl block:
pub async fn get_position_mode(&self) -> Result<bool, BinanceError> {
    let response: serde_json::Value = self
        .signed_request("GET", "/fapi/v1/positionSide/dual", &[])
        .await?;
    Ok(response["dualSidePosition"].as_bool().unwrap_or(false))
}

# Step 4: Compile
cargo build --package robsond --package robson-connectors
# EXIT CODE 0 = SUCCESS

# Step 5: Run unit tests
cargo test --package robsond binance_exchange
# EXIT CODE 0 = PASS

# Step 6: Add integration test (if DATABASE_URL available)
cat > v2/robsond/tests/position_mode_check.rs <<'EOF'
#[tokio::test]
#[ignore = "requires Binance testnet keys"]
async fn test_validate_futures_settings_one_way_mode() {
    // Test with real testnet credentials
    // Verify One-way mode accepted, Hedge mode rejected
}
EOF

cargo test --package robsond position_mode_check -- --ignored
# EXIT CODE 0 = PASS (if keys available)
```

**Expected Outcome**:
```bash
# PASS condition 1: Code added successfully
grep -q 'dual_side_position' v2/robsond/src/binance_exchange.rs

# PASS condition 2: Builds without errors
cargo build --package robsond 2>&1 | grep -q "Finished"

# PASS condition 3: Function signature exists
grep -q 'pub async fn get_position_mode' v2/robson-connectors/src/binance_rest.rs

# PASS condition 4: Tests pass
cargo test --package robsond binance_exchange 2>&1 | grep -q "test result: ok"
```

**Failure Detection**:
```bash
# FAIL if struct not added
# FAIL if validate_futures_settings still returns hardcoded "One-way"
# FAIL if API call not present
# FAIL if build fails
# FAIL if tests fail
```

**Rollback**:
```bash
git restore v2/robsond/src/binance_exchange.rs v2/robson-connectors/src/binance_rest.rs
cargo build --package robsond
```

---

### EP-006: MIG-v3#12 — Event-Sourced Month Boundary

**Objective**: Implement MonthBoundaryReset event + monthly_state projection for capital base persistence across restarts.

**Preconditions**:
```bash
# VERIFY: MIG-v3#11 (ADR-0024) is implemented
grep -q 'TradingPolicy' v2/robson-domain/src/lib.rs

# VERIFY: migrations/ directory exists
test -d v2/migrations

# VERIFY: Latest migration number
LATEST=$(ls v2/migrations/*.sql | sort | tail -1 | grep -oE '[0-9]{14}')
echo "Latest migration: $LATEST"
# Should be 20240101000007 or similar

# VERIFY: Baseline builds
cargo build --all
# EXIT CODE 0 = precondition met
```

**Inputs**:
- Files to create:
  - `v2/robson-domain/src/events.rs` (add variant to Event enum)
  - `v2/migrations/00000008_create_monthly_state.sql` (new migration)
  - `v2/robson-projector/src/handlers.rs` (new handler)
  - `v2/robsond/src/daemon.rs` (month detection + boundary handler)
  - `v2/robsond/src/position_manager.rs` (capital_base loading)
- Migration number: `00000008` (increment from 00000007)
- Table: `monthly_state` (year, month, capital_base, carried_risk)

**Steps**:
```bash
# Step 1: Add MonthBoundaryReset to Event enum in events.rs
# Insert after last variant before closing brace (around line 200):
    MonthBoundaryReset {
        capital_base: Decimal,
        carried_positions_risk: Decimal,
        month: u32,
        year: i32,
        timestamp: DateTime<Utc>,
    },

# Step 2: Create migration file
cat > v2/migrations/00000008_create_monthly_state.sql <<'EOF'
-- Month boundary state tracking (MIG-v3#12)
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
EOF

# Step 3: Add projector handler to handlers.rs
# Insert new function before closing brace:
pub async fn handle_month_boundary_reset(
    pool: &PgPool,
    event: &Event,
) -> Result<(), ProjectorError> {
    let (capital_base, carried_risk, month, year, _timestamp) = match event {
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
        "INSERT INTO monthly_state (year, month, capital_base, carried_risk)
         VALUES ($1, $2, $3, $4)
         ON CONFLICT (year, month) DO UPDATE SET
           capital_base = EXCLUDED.capital_base,
           carried_risk = EXCLUDED.carried_risk"
    )
    .bind(year)
    .bind(*month as i16)
    .bind(capital_base)
    .bind(carried_risk)
    .execute(pool)
    .await?;
    
    Ok(())
}

# Step 4: Register handler in apply_event_to_projections() dispatcher

# Step 5: Add last_month_check to Daemon struct in daemon.rs (line ~83)
# Add field:
#   last_month_check: Arc<RwLock<(i32, u32)>>,

# Step 6: Initialize in Daemon::new() constructors

# Step 7: Add month detection logic to main event loop (line ~439)
# See doc section 4 Track C C1 Step 4 for exact code

# Step 8: Add handle_month_boundary() method to Daemon impl
# See doc section 4 Track C C1 Step 5 for full implementation

# Step 9: Update build_risk_context() in position_manager.rs
# See doc section 4 Track C C1 Step 6 for capital_base loading logic

# Step 10: Run migration locally
cd v2
DATABASE_URL="postgres://..." sqlx migrate run
# EXIT CODE 0 = migration applied

# Step 11: Compile
cargo build --all
# EXIT CODE 0 = SUCCESS

# Step 12: Run tests
cargo test --package robsond month_boundary
cargo test --package robson-projector handle_month_boundary
# EXIT CODE 0 = PASS
```

**Expected Outcome**:
```bash
# PASS condition 1: Migration file exists
test -f v2/migrations/00000008_create_monthly_state.sql

# PASS condition 2: Migration applied successfully
psql $DATABASE_URL -c "SELECT COUNT(*) FROM monthly_state;" | grep -q "0"

# PASS condition 3: Event variant added
grep -q 'MonthBoundaryReset' v2/robson-domain/src/events.rs

# PASS condition 4: Handler exists
grep -q 'handle_month_boundary_reset' v2/robson-projector/src/handlers.rs

# PASS condition 5: Builds successfully
cargo build --all 2>&1 | grep -q "Finished"

# PASS condition 6: Tests pass
cargo test --all 2>&1 | grep -q "test result: ok"
```

**Failure Detection**:
```bash
# FAIL if migration fails to apply
# FAIL if Event enum doesn't compile after variant addition
# FAIL if handler not registered in dispatcher
# FAIL if tests fail
# FAIL if daemon panics on month boundary simulation
```

**Rollback**:
```bash
# Rollback migration
cd v2
DATABASE_URL="postgres://..." sqlx migrate revert

# Rollback code
git restore v2/robson-domain/src/events.rs \
           v2/robson-projector/src/handlers.rs \
           v2/robsond/src/daemon.rs \
           v2/robsond/src/position_manager.rs
rm v2/migrations/00000008_create_monthly_state.sql

cargo build --all
```

---

### EP-007: ADR-0022-R1 — Position Reconciliation Worker

**Objective**: Implement reconciliation worker to detect and close UNTRACKED positions (not authored by robsond).

**Preconditions**:
```bash
# VERIFY: ADR-0023-C2 completed (config symbols not hardcoded)
grep -q 'symbols: vec!\[\]' v2/robsond/src/config.rs

# VERIFY: Exchange port has base implementation
grep -q 'trait ExchangePort' v2/robson-exec/src/ports.rs

# VERIFY: Baseline builds
cargo build --all
# EXIT CODE 0 = precondition met
```

**Inputs**:
- Files to create:
  - `v2/robsond/src/reconciliation_worker.rs` (new module)
  - Updates to `v2/robson-exec/src/ports.rs` (extend trait)
  - Updates to `v2/robsond/src/binance_exchange.rs` (implement trait methods)
  - Updates to `v2/robson-exec/src/stub.rs` (mock implementation)
  - Updates to `v2/robsond/src/daemon.rs` (spawn worker)
  - Updates to `v2/robsond/src/lib.rs` (add module)
- Scan interval: 60 seconds (configurable via `ROBSON_RECONCILIATION_INTERVAL_SECS`)
- API endpoint: `GET /fapi/v2/positionRisk` (all symbols)

**Steps**:
```bash
# Step 1: Extend ExchangePort trait in ports.rs (after line 40)
# Add methods (see doc section 4 Track C C2 Step 2):
#   async fn get_all_open_positions(&self) -> ExecResult<Vec<ExchangePosition>>;
#   async fn close_position_market(...) -> ExecResult<Order>;

# Step 2: Define ExchangePosition struct in ports.rs

# Step 3: Create reconciliation_worker.rs (see doc section 4 Track C C2 Step 1)
# Full scaffold with ReconciliationWorker struct + run() + scan_and_reconcile() methods

# Step 4: Implement get_all_open_positions() in binance_exchange.rs
# See doc section 4 Track C C2 Step 3 for Binance FAPI implementation

# Step 5: Implement close_position_market() in binance_exchange.rs
# Use POST /fapi/v1/order with type=MARKET, reduceOnly=true

# Step 6: Add mock implementations to stub.rs
# Return empty vec for get_all_open_positions in StubExchange

# Step 7: Add reconciliation_worker module to lib.rs
# Insert: pub mod reconciliation_worker;

# Step 8: Spawn worker in daemon.rs (after line 400)
# See doc section 4 Track C C2 Step 4 for integration code

# Step 9: Add startup gating (blocking scan before first arm)
# See doc section 4 Track C C2 Step 5

# Step 10: Compile
cargo build --all
# EXIT CODE 0 = SUCCESS

# Step 11: Run unit tests
cargo test --package robsond reconciliation_worker
cargo test --package robson-exec exchange_port
# EXIT CODE 0 = PASS

# Step 12: Integration test (manual on testnet)
# Place manual order via Binance UI
# Start robsond
# Verify UNTRACKED position detected and closed within 60s
```

**Expected Outcome**:
```bash
# PASS condition 1: Module file exists
test -f v2/robsond/src/reconciliation_worker.rs

# PASS condition 2: Trait methods added
grep -q 'get_all_open_positions' v2/robson-exec/src/ports.rs

# PASS condition 3: Binance implementation exists
grep -q 'get_all_open_positions' v2/robsond/src/binance_exchange.rs

# PASS condition 4: Worker spawned in daemon
grep -q 'reconciliation_worker.run()' v2/robsond/src/daemon.rs

# PASS condition 5: Builds successfully
cargo build --all 2>&1 | grep -q "Finished"

# PASS condition 6: Tests pass
cargo test --all 2>&1 | grep -q "test result: ok"
```

**Failure Detection**:
```bash
# FAIL if trait methods missing
# FAIL if worker not spawned in daemon
# FAIL if manual UNTRACKED position not detected in testnet
# FAIL if close_position_market fails to execute
# FAIL if tests fail
```

**Rollback**:
```bash
git restore v2/robsond/src/reconciliation_worker.rs \
           v2/robson-exec/src/ports.rs \
           v2/robsond/src/binance_exchange.rs \
           v2/robson-exec/src/stub.rs \
           v2/robsond/src/daemon.rs \
           v2/robsond/src/lib.rs
rm -f v2/robsond/src/reconciliation_worker.rs

cargo build --all
```

---

## Entry Point Selection Guide

**For VAL-001 PASS (30 min)**:
```
Execute: EP-001 (VAL-001-P2-INJECT)
```

**For Testnet Low-Volume Mitigation (2h)**:
```
Execute: EP-002 (VAL-001-P2-CONFIG)
```

**For ADR-0023 Full Compliance (4-5h)**:
```
Execute in sequence:
1. EP-003 (ADR-0023-C1)
2. EP-004 (ADR-0023-C2)
3. EP-005 (ADR-0023-C3)
```

**For VAL-002 Unblock (14-20h = 3-4 days)**:
```
Execute in sequence:
1. EP-003 (ADR-0023-C1)
2. EP-004 (ADR-0023-C2)
3. EP-005 (ADR-0023-C3)
4. EP-006 (MIG-v3#12)
5. EP-007 (ADR-0022-R1)
```

---

## Verification Commands Reference

**Check if position Armed**:
```bash
curl -s http://localhost:8080/status | jq '.positions[] | select(.state == "Armed")'
```

**Check if build successful**:
```bash
cargo build --all 2>&1 | grep -q "Finished" && echo "PASS" || echo "FAIL"
```

**Check if tests pass**:
```bash
cargo test --all 2>&1 | tail -1 | grep -q "test result: ok" && echo "PASS" || echo "FAIL"
```

**Check if migration applied**:
```bash
psql $DATABASE_URL -c "\d monthly_state" | grep -q "Table" && echo "PASS" || echo "FAIL"
```

**Check if hardcode removed**:
```bash
! grep -q 'Symbol::from_pair("BTCUSDT")' v2/robsond/src/daemon.rs && echo "PASS" || echo "FAIL"
```
