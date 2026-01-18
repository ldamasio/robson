//! Daemon configuration.
//!
//! Loads configuration from environment variables with sensible defaults.

use crate::error::{DaemonError, DaemonResult};
use rust_decimal::Decimal;
use std::env;
use std::str::FromStr;
use uuid::Uuid;

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

    /// Projection configuration
    pub projection: ProjectionConfig,

    /// Environment (test, development, production)
    pub environment: Environment,
}

/// API server configuration.
#[derive(Debug, Clone)]
pub struct ApiConfig {
    /// Host to bind to
    pub host: String,
    /// Port to bind to
    pub port: u16,
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
#[derive(Debug, Clone)]
pub struct EngineConfig {
    /// Default risk percent for position sizing (0.01 = 1%)
    pub default_risk_percent: Decimal,
    /// Minimum tech stop distance (0.001 = 0.1%)
    pub min_tech_stop_percent: Decimal,
    /// Maximum tech stop distance (0.10 = 10%)
    pub max_tech_stop_percent: Decimal,
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
        let projection = Self::load_projection_config()?;

        Ok(Self { api, engine, projection, environment })
    }

    /// Create test configuration.
    pub fn test() -> Self {
        Self {
            api: ApiConfig {
                host: "127.0.0.1".to_string(),
                port: 0, // Let OS assign port
            },
            engine: EngineConfig {
                default_risk_percent: Decimal::new(1, 2),   // 1%
                min_tech_stop_percent: Decimal::new(1, 3),  // 0.1%
                max_tech_stop_percent: Decimal::new(10, 2), // 10%
            },
            projection: ProjectionConfig {
                database_url: None,
                tenant_id: None,
                stream_key: "test:stream".to_string(),
                poll_interval_ms: 100,
            },
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

        Ok(ApiConfig { host, port })
    }

    fn load_engine_config() -> DaemonResult<EngineConfig> {
        let default_risk = Self::load_decimal_env(
            "ROBSON_DEFAULT_RISK_PERCENT",
            Decimal::new(1, 2), // 1%
        )?;

        let min_tech_stop = Self::load_decimal_env(
            "ROBSON_MIN_TECH_STOP_PERCENT",
            Decimal::new(1, 3), // 0.1%
        )?;

        let max_tech_stop = Self::load_decimal_env(
            "ROBSON_MAX_TECH_STOP_PERCENT",
            Decimal::new(10, 2), // 10%
        )?;

        Ok(EngineConfig {
            default_risk_percent: default_risk,
            min_tech_stop_percent: min_tech_stop,
            max_tech_stop_percent: max_tech_stop,
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
}

impl Default for Config {
    fn default() -> Self {
        Self {
            api: ApiConfig { host: "0.0.0.0".to_string(), port: 8080 },
            engine: EngineConfig {
                default_risk_percent: Decimal::new(1, 2),   // 1%
                min_tech_stop_percent: Decimal::new(1, 3),  // 0.1%
                max_tech_stop_percent: Decimal::new(10, 2), // 10%
            },
            projection: ProjectionConfig {
                database_url: None,
                tenant_id: None,
                stream_key: "robson:daemon".to_string(),
                poll_interval_ms: 100,
            },
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
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();

        assert_eq!(config.api.port, 8080);
        assert_eq!(config.environment, Environment::Development);
    }

    #[test]
    fn test_test_config() {
        let config = Config::test();

        assert_eq!(config.api.port, 0);
        assert_eq!(config.environment, Environment::Test);
    }

    #[test]
    fn test_engine_config_defaults() {
        let config = Config::default();

        assert_eq!(config.engine.default_risk_percent, Decimal::new(1, 2));
        assert_eq!(config.engine.min_tech_stop_percent, Decimal::new(1, 3));
        assert_eq!(config.engine.max_tech_stop_percent, Decimal::new(10, 2));
    }

    #[test]
    fn test_environment_display() {
        assert_eq!(Environment::Test.to_string(), "test");
        assert_eq!(Environment::Development.to_string(), "development");
        assert_eq!(Environment::Production.to_string(), "production");
    }
}
