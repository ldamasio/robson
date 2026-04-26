//! Daemon configuration.
//!
//! Loads configuration from environment variables with sensible defaults.

use std::{env, str::FromStr};

use rust_decimal::Decimal;
use uuid::Uuid;

use crate::error::{DaemonError, DaemonResult};

// =============================================================================
// Configuration
// =============================================================================

/// Daemon configuration.
#[derive(Debug, Clone)]
pub struct Config {
    /// API server configuration
    pub api: ApiConfig,

    /// Engine configuration
    pub engine: EngineConfig,

    /// Technical stop policy configuration (ADR-0024)
    pub tech_stop: TechStopConfigEnv,

    /// Projection configuration
    pub projection: ProjectionConfig,

    /// Market data WebSocket configuration
    pub market_data: MarketDataConfig,

    /// Position monitor configuration (safety net)
    pub position_monitor: PositionMonitorConfig,

    /// Reconciliation worker configuration.
    pub reconciliation: ReconciliationConfig,

    /// Environment (test, development, production)
    pub environment: Environment,
}

/// API server configuration.
///
/// CORS allow-list is read directly from `ROBSON_CORS_ALLOWED_ORIGINS`
/// (comma-separated) inside `api::build_cors_layer`; not duplicated here.
#[derive(Debug, Clone)]
pub struct ApiConfig {
    /// Host to bind to
    pub host: String,
    /// Port to bind to
    pub port: u16,
    /// Bearer token for authenticating mutating API routes.
    /// Required when ROBSON_ENV=production; optional otherwise.
    pub api_token: Option<String>,
}

/// Projection configuration.
#[derive(Debug, Clone)]
pub struct ProjectionConfig {
    /// Database connection URL
    pub database_url: Option<String>,
    /// Tenant ID for event polling
    pub tenant_id: Option<Uuid>,
    /// Stream key to poll events from
    pub stream_key: String,
    /// Poll interval in milliseconds
    pub poll_interval_ms: u64,
}

/// Engine configuration.
///
/// Note: risk per trade is NOT configurable — it is fixed at 1% by v3 policy.
/// See `RiskConfig::RISK_PER_TRADE_PCT` in robson-domain.
#[derive(Debug, Clone)]
pub struct EngineConfig {
    /// Operator's declared starting capital in quote currency (e.g., USDT).
    ///
    /// Source: `ROBSON_CAPITAL_BASE` env var. Required for the first month of
    /// operation. After the first `MonthBoundaryReset`, the persisted
    /// `monthly_state.capital_base` takes precedence (see ADR-0024 §6).
    ///
    /// This value is the initial equity the operator brings to the account.
    /// Position sizing is derived: `position_size = (capital_base × 1%) / stop_distance`.
    pub capital_base: Decimal,
    /// Minimum tech stop distance (0.001 = 0.1%)
    pub min_tech_stop_percent: Decimal,
    /// Maximum tech stop distance (0.10 = 10%)
    pub max_tech_stop_percent: Decimal,
}

/// Technical stop policy configuration loaded from environment (ADR-0024).
///
/// Canonical env vars use percentage semantics (e.g., 1.0 = 1%).
/// Legacy vars (ROBSON_MIN_TECH_STOP_PERCENT, ROBSON_MAX_TECH_STOP_PERCENT)
/// use fraction semantics and are mapped by multiplying by 100.
#[derive(Debug, Clone)]
pub struct TechStopConfigEnv {
    /// Minimum tech stop as percentage (env: ROBSON_MIN_TECH_STOP_PCT, default
    /// 1.0%)
    pub min_stop_pct: Decimal,
    /// Maximum tech stop as percentage (env: ROBSON_MAX_TECH_STOP_PCT, default
    /// 10.0%)
    pub max_stop_pct: Decimal,
    /// Support/resistance level to use (env: ROBSON_TECH_STOP_SUPPORT_N,
    /// default 2)
    pub support_level_n: usize,
    /// Lookback candles for analysis (env: ROBSON_TECH_STOP_LOOKBACK, default
    /// 100)
    pub lookback_candles: usize,
}

/// Market data configuration.
#[derive(Debug, Clone)]
pub struct MarketDataConfig {
    /// Symbols to subscribe via WebSocket (e.g., ["BTCUSDT", "ETHUSDT"])
    pub symbols: Vec<String>,
}

impl Default for MarketDataConfig {
    fn default() -> Self {
        Self { symbols: vec![] }
    }
}

/// Position monitor configuration (safety net for rogue positions).
#[derive(Debug, Clone)]
pub struct PositionMonitorConfig {
    /// Whether the position monitor is enabled
    pub enabled: bool,
    /// Polling interval in seconds
    pub poll_interval_secs: u64,
    /// Symbols to monitor (e.g., ["BTCUSDT", "ETHUSDT"])
    pub symbols: Vec<String>,
    /// Binance API key (required to enable runtime monitor).
    pub binance_api_key: Option<String>,
    /// Binance API secret (required to enable runtime monitor).
    pub binance_api_secret: Option<String>,
}

impl Default for PositionMonitorConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            poll_interval_secs: 20,
            symbols: vec![],
            binance_api_key: None,
            binance_api_secret: None,
        }
    }
}

/// Reconciliation worker configuration.
#[derive(Debug, Clone)]
pub struct ReconciliationConfig {
    /// Interval between full account reconciliation scans.
    pub interval_secs: u64,
}

impl Default for ReconciliationConfig {
    fn default() -> Self {
        Self { interval_secs: 60 }
    }
}

/// Environment type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Environment {
    /// Test environment (uses stubs)
    Test,
    /// Development environment
    Development,
    /// Production environment
    Production,
}

impl Config {
    /// Load configuration from environment variables.
    pub fn from_env() -> DaemonResult<Self> {
        // Load .env file if present (ignore errors)
        let _ = dotenvy::dotenv();

        let environment = Self::load_environment()?;
        let api = Self::load_api_config()?;
        let engine = Self::load_engine_config()?;
        let tech_stop = Self::load_tech_stop_config()?;
        let projection = Self::load_projection_config()?;
        let market_data = Self::load_market_data_config()?;
        let position_monitor = Self::load_position_monitor_config()?;
        let reconciliation = Self::load_reconciliation_config()?;

        // Fail-fast: API token is mandatory in production
        if environment == Environment::Production && api.api_token.is_none() {
            return Err(DaemonError::Config(
                "ROBSON_API_TOKEN is required when ROBSON_ENV=production".to_string(),
            ));
        }

        Ok(Self {
            api,
            engine,
            tech_stop,
            projection,
            market_data,
            position_monitor,
            reconciliation,
            environment,
        })
    }

    /// Create test configuration.
    pub fn test() -> Self {
        Self {
            api: ApiConfig {
                host: "127.0.0.1".to_string(),
                port: 0, // Let OS assign port
                api_token: None,
            },
            engine: EngineConfig {
                capital_base: Decimal::from(10000),
                min_tech_stop_percent: Decimal::new(1, 3), // 0.1%
                max_tech_stop_percent: Decimal::new(10, 2), // 10%
            },
            tech_stop: TechStopConfigEnv {
                min_stop_pct: Decimal::new(1, 1), // 0.1%
                max_stop_pct: Decimal::from(20),
                support_level_n: 2,
                lookback_candles: 100,
            },
            projection: ProjectionConfig {
                database_url: None,
                tenant_id: None,
                stream_key: "test:stream".to_string(),
                poll_interval_ms: 100,
            },
            market_data: MarketDataConfig { symbols: vec!["BTCUSDT".to_string()] },
            position_monitor: PositionMonitorConfig {
                enabled: false, // Disabled in tests
                poll_interval_secs: 1,
                symbols: vec!["BTCUSDT".to_string()],
                binance_api_key: None,
                binance_api_secret: None,
            },
            reconciliation: ReconciliationConfig { interval_secs: 1 },
            environment: Environment::Test,
        }
    }

    fn load_environment() -> DaemonResult<Environment> {
        let env_str = env::var("ROBSON_ENV").unwrap_or_else(|_| "development".to_string());

        match env_str.to_lowercase().as_str() {
            "test" => Ok(Environment::Test),
            "development" | "dev" => Ok(Environment::Development),
            "production" | "prod" => Ok(Environment::Production),
            other => Err(DaemonError::Config(format!(
                "Invalid ROBSON_ENV: {}. Expected: test, development, production",
                other
            ))),
        }
    }

    fn load_api_config() -> DaemonResult<ApiConfig> {
        let host = env::var("ROBSON_API_HOST").unwrap_or_else(|_| "0.0.0.0".to_string());
        let port_str = env::var("ROBSON_API_PORT").unwrap_or_else(|_| "8080".to_string());

        let port = port_str
            .parse::<u16>()
            .map_err(|_| DaemonError::Config(format!("Invalid ROBSON_API_PORT: {}", port_str)))?;

        let api_token = env::var("ROBSON_API_TOKEN").ok().filter(|v| !v.trim().is_empty());

        Ok(ApiConfig { host, port, api_token })
    }

    fn load_engine_config() -> DaemonResult<EngineConfig> {
        // Note: risk per trade is NOT loaded from env — fixed at 1% by v3 policy.
        let min_tech_stop = Self::load_decimal_env(
            "ROBSON_MIN_TECH_STOP_PERCENT",
            Decimal::new(1, 3), // 0.1%
        )?;

        let max_tech_stop = Self::load_decimal_env(
            "ROBSON_MAX_TECH_STOP_PERCENT",
            Decimal::new(10, 2), // 10%
        )?;

        let capital_base = Self::load_decimal_env(
            "ROBSON_CAPITAL_BASE",
            Decimal::new(10, 3), // 0.01; operator must configure
        )?;

        if capital_base <= Decimal::ZERO {
            return Err(DaemonError::Config("ROBSON_CAPITAL_BASE must be positive".to_string()));
        }

        Ok(EngineConfig {
            capital_base,
            min_tech_stop_percent: min_tech_stop,
            max_tech_stop_percent: max_tech_stop,
        })
    }

    fn load_tech_stop_config() -> DaemonResult<TechStopConfigEnv> {
        // Primary: ROBSON_MIN_TECH_STOP_PCT / ROBSON_MAX_TECH_STOP_PCT (percentage
        // semantics) Legacy fallback: ROBSON_MIN_TECH_STOP_PERCENT /
        // ROBSON_MAX_TECH_STOP_PERCENT (fraction semantics → × 100)
        let min_stop_pct = match env::var("ROBSON_MIN_TECH_STOP_PCT") {
            Ok(val) => Decimal::from_str(&val).map_err(|_| {
                DaemonError::Config(format!("Invalid ROBSON_MIN_TECH_STOP_PCT value: {}", val))
            })?,
            Err(_) => {
                let legacy =
                    Self::load_decimal_env("ROBSON_MIN_TECH_STOP_PERCENT", Decimal::new(1, 3))?;
                legacy * Decimal::from(100)
            },
        };

        let max_stop_pct = match env::var("ROBSON_MAX_TECH_STOP_PCT") {
            Ok(val) => Decimal::from_str(&val).map_err(|_| {
                DaemonError::Config(format!("Invalid ROBSON_MAX_TECH_STOP_PCT value: {}", val))
            })?,
            Err(_) => {
                let legacy =
                    Self::load_decimal_env("ROBSON_MAX_TECH_STOP_PERCENT", Decimal::new(10, 2))?;
                legacy * Decimal::from(100)
            },
        };

        let support_level_n = env::var("ROBSON_TECH_STOP_SUPPORT_N")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(2);

        let lookback_candles = env::var("ROBSON_TECH_STOP_LOOKBACK")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(100);

        Ok(TechStopConfigEnv {
            min_stop_pct,
            max_stop_pct,
            support_level_n,
            lookback_candles,
        })
    }

    fn load_decimal_env(key: &str, default: Decimal) -> DaemonResult<Decimal> {
        match env::var(key) {
            Ok(val) => Decimal::from_str(&val)
                .map_err(|_| DaemonError::Config(format!("Invalid {} value: {}", key, val))),
            Err(_) => Ok(default),
        }
    }

    fn load_projection_config() -> DaemonResult<ProjectionConfig> {
        let database_url = env::var("DATABASE_URL").ok();

        let tenant_id = if database_url.is_some() {
            let tenant_str = env::var("PROJECTION_TENANT_ID").map_err(|_| {
                DaemonError::Config(
                    "PROJECTION_TENANT_ID required when DATABASE_URL is set".to_string(),
                )
            })?;
            Some(Uuid::parse_str(&tenant_str).map_err(|_| {
                DaemonError::Config(format!("Invalid PROJECTION_TENANT_ID: {}", tenant_str))
            })?)
        } else {
            None
        };

        let stream_key =
            env::var("PROJECTION_STREAM_KEY").unwrap_or_else(|_| "robson:daemon".to_string());

        let poll_interval_str =
            env::var("PROJECTION_POLL_INTERVAL_MS").unwrap_or_else(|_| "100".to_string());
        let poll_interval_ms = poll_interval_str.parse::<u64>().map_err(|_| {
            DaemonError::Config(format!(
                "Invalid PROJECTION_POLL_INTERVAL_MS: {}",
                poll_interval_str
            ))
        })?;

        Ok(ProjectionConfig {
            database_url,
            tenant_id,
            stream_key,
            poll_interval_ms,
        })
    }

    fn load_market_data_config() -> DaemonResult<MarketDataConfig> {
        let symbols_str = env::var("ROBSON_MARKET_DATA_SYMBOLS").unwrap_or_default();
        let symbols: Vec<String> = symbols_str
            .split(',')
            .map(|s| s.trim().to_uppercase())
            .filter(|s| !s.is_empty())
            .collect();

        if symbols.is_empty() {
            return Err(DaemonError::Config("ROBSON_MARKET_DATA_SYMBOLS is required".to_string()));
        }

        Ok(MarketDataConfig { symbols })
    }

    fn load_position_monitor_config() -> DaemonResult<PositionMonitorConfig> {
        // Check if enabled
        let enabled = env::var("ROBSON_POSITION_MONITOR_ENABLED")
            .ok()
            .and_then(|v| v.parse::<bool>().ok())
            .unwrap_or(true); // Default: enabled

        // Poll interval
        let poll_interval_str =
            env::var("ROBSON_POSITION_MONITOR_POLL_INTERVAL").unwrap_or_else(|_| "20".to_string());
        let poll_interval_secs = poll_interval_str.parse::<u64>().map_err(|_| {
            DaemonError::Config(format!(
                "Invalid ROBSON_POSITION_MONITOR_POLL_INTERVAL: {}",
                poll_interval_str
            ))
        })?;

        // Symbols to monitor
        let symbols_str = env::var("ROBSON_POSITION_MONITOR_SYMBOLS").unwrap_or_default();
        let symbols: Vec<String> = symbols_str
            .split(',')
            .map(|s| s.trim().to_uppercase())
            .filter(|s| !s.is_empty())
            .collect();

        if enabled && symbols.is_empty() {
            return Err(DaemonError::Config(
                "ROBSON_POSITION_MONITOR_SYMBOLS is required when monitor is enabled".to_string(),
            ));
        }

        let binance_api_key = env::var("ROBSON_BINANCE_API_KEY")
            .ok()
            .or_else(|| env::var("BINANCE_API_KEY").ok())
            .filter(|v| !v.trim().is_empty());
        let binance_api_secret = env::var("ROBSON_BINANCE_API_SECRET")
            .ok()
            .or_else(|| env::var("BINANCE_API_SECRET").ok())
            .filter(|v| !v.trim().is_empty());

        Ok(PositionMonitorConfig {
            enabled,
            poll_interval_secs,
            symbols,
            binance_api_key,
            binance_api_secret,
        })
    }

    fn load_reconciliation_config() -> DaemonResult<ReconciliationConfig> {
        let interval_str =
            env::var("ROBSON_RECONCILIATION_INTERVAL_SECS").unwrap_or_else(|_| "60".to_string());
        let interval_secs = interval_str.parse::<u64>().map_err(|_| {
            DaemonError::Config(format!(
                "Invalid ROBSON_RECONCILIATION_INTERVAL_SECS: {}",
                interval_str
            ))
        })?;

        if interval_secs == 0 {
            return Err(DaemonError::Config(
                "ROBSON_RECONCILIATION_INTERVAL_SECS must be greater than 0".to_string(),
            ));
        }

        Ok(ReconciliationConfig { interval_secs })
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            api: ApiConfig {
                host: "0.0.0.0".to_string(),
                port: 8080,
                api_token: None,
            },
            engine: EngineConfig {
                capital_base: Decimal::from(10000),
                min_tech_stop_percent: Decimal::new(1, 3), // 0.1%
                max_tech_stop_percent: Decimal::new(10, 2), // 10%
            },
            tech_stop: TechStopConfigEnv {
                min_stop_pct: Decimal::ONE,      // 1%
                max_stop_pct: Decimal::from(10), // 10%
                support_level_n: 2,
                lookback_candles: 100,
            },
            projection: ProjectionConfig {
                database_url: None,
                tenant_id: None,
                stream_key: "robson:daemon".to_string(),
                poll_interval_ms: 100,
            },
            market_data: MarketDataConfig::default(),
            position_monitor: PositionMonitorConfig::default(),
            reconciliation: ReconciliationConfig::default(),
            environment: Environment::Development,
        }
    }
}

impl std::fmt::Display for Environment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Environment::Test => write!(f, "test"),
            Environment::Development => write!(f, "development"),
            Environment::Production => write!(f, "production"),
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use std::sync::{Mutex, OnceLock};

    use super::*;

    struct EnvGuard {
        saved: Vec<(String, Option<String>)>,
    }

    impl EnvGuard {
        fn new(updates: &[(&str, Option<&str>)]) -> Self {
            let mut saved = Vec::with_capacity(updates.len());

            for (key, value) in updates {
                saved.push(((*key).to_string(), std::env::var(key).ok()));
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }

            Self { saved }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (key, value) in self.saved.iter().rev() {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
    }

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn test_default_config() {
        let config = Config::default();

        assert_eq!(config.api.port, 8080);
        assert_eq!(config.environment, Environment::Development);
        assert_eq!(config.tech_stop.min_stop_pct, Decimal::ONE);
        assert_eq!(config.tech_stop.max_stop_pct, Decimal::from(10));
        assert_eq!(config.tech_stop.support_level_n, 2);
        assert_eq!(config.tech_stop.lookback_candles, 100);
        assert!(config.market_data.symbols.is_empty());
        assert!(config.position_monitor.symbols.is_empty());
        assert_eq!(config.reconciliation.interval_secs, 60);
    }

    #[test]
    fn test_test_config() {
        let config = Config::test();

        assert_eq!(config.api.port, 0);
        assert_eq!(config.environment, Environment::Test);
        assert_eq!(config.tech_stop.min_stop_pct, Decimal::new(1, 1)); // 0.1%
        assert_eq!(config.tech_stop.max_stop_pct, Decimal::from(20));
        assert_eq!(config.market_data.symbols, vec!["BTCUSDT"]);
        assert_eq!(config.reconciliation.interval_secs, 1);
    }

    #[test]
    fn test_engine_config_defaults() {
        let config = Config::default();

        // Risk per trade is NOT in engine config — fixed at 1% in domain
        assert_eq!(config.engine.min_tech_stop_percent, Decimal::new(1, 3));
        assert_eq!(config.engine.max_tech_stop_percent, Decimal::new(10, 2));
    }

    #[test]
    fn test_environment_display() {
        assert_eq!(Environment::Test.to_string(), "test");
        assert_eq!(Environment::Development.to_string(), "development");
        assert_eq!(Environment::Production.to_string(), "production");
    }

    #[test]
    fn test_load_market_data_config_requires_symbols() {
        let _lock = env_lock().lock().unwrap();
        let _env = EnvGuard::new(&[("ROBSON_MARKET_DATA_SYMBOLS", Some(""))]);

        let err = Config::load_market_data_config().unwrap_err();
        assert!(matches!(
            err,
            DaemonError::Config(message) if message == "ROBSON_MARKET_DATA_SYMBOLS is required"
        ));
    }

    #[test]
    fn test_load_market_data_config_parses_multiple_symbols() {
        let _lock = env_lock().lock().unwrap();
        let _env =
            EnvGuard::new(&[("ROBSON_MARKET_DATA_SYMBOLS", Some("BTCUSDT, ethusdt,SOLUSDC"))]);

        let config = Config::load_market_data_config().unwrap();
        assert_eq!(config.symbols, vec!["BTCUSDT", "ETHUSDT", "SOLUSDC"]);
    }

    #[test]
    fn test_load_position_monitor_config_requires_symbols_when_enabled() {
        let _lock = env_lock().lock().unwrap();
        let _env = EnvGuard::new(&[
            ("ROBSON_POSITION_MONITOR_ENABLED", Some("true")),
            ("ROBSON_POSITION_MONITOR_SYMBOLS", Some("")),
        ]);

        let err = Config::load_position_monitor_config().unwrap_err();
        assert!(matches!(
            err,
            DaemonError::Config(message)
                if message
                    == "ROBSON_POSITION_MONITOR_SYMBOLS is required when monitor is enabled"
        ));
    }

    #[test]
    fn test_load_position_monitor_config_allows_empty_symbols_when_disabled() {
        let _lock = env_lock().lock().unwrap();
        let _env = EnvGuard::new(&[
            ("ROBSON_POSITION_MONITOR_ENABLED", Some("false")),
            ("ROBSON_POSITION_MONITOR_SYMBOLS", Some("")),
        ]);

        let config = Config::load_position_monitor_config().unwrap();
        assert!(!config.enabled);
        assert!(config.symbols.is_empty());
    }

    #[test]
    fn test_load_reconciliation_config_defaults_to_sixty_seconds() {
        let _lock = env_lock().lock().unwrap();
        let _env = EnvGuard::new(&[("ROBSON_RECONCILIATION_INTERVAL_SECS", None)]);

        let config = Config::load_reconciliation_config().unwrap();
        assert_eq!(config.interval_secs, 60);
    }

    #[test]
    fn test_load_reconciliation_config_rejects_zero() {
        let _lock = env_lock().lock().unwrap();
        let _env = EnvGuard::new(&[("ROBSON_RECONCILIATION_INTERVAL_SECS", Some("0"))]);

        let err = Config::load_reconciliation_config().unwrap_err();
        assert!(matches!(
            err,
            DaemonError::Config(message)
                if message == "ROBSON_RECONCILIATION_INTERVAL_SECS must be greater than 0"
        ));
    }
}
