//! A real `robsond` child process for crash tests (ADR-0056 step 7).
//!
//! Spawned with a cleared environment, its logs in a file. Two independent
//! watchdogs keep a stuck child from hanging the run:
//! - a thread in the test process SIGKILLs it at a hard deadline, even when the
//!   test's async runtime is stuck;
//! - the child, built with `crash-points`, SIGKILLs itself shortly after that
//!   deadline, so it also dies when the test process itself was killed.

use std::{
    net::{SocketAddr, TcpListener},
    os::unix::process::ExitStatusExt,
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

use anyhow::{anyhow, bail, Context, Result};
use uuid::Uuid;

const SIGKILL: i32 = 9;
/// How much longer than the test-side deadline the child's own watchdog waits.
const SELF_WATCHDOG_MARGIN: Duration = Duration::from_secs(5);

/// How to run the daemon.
#[derive(Debug, Clone)]
pub struct DaemonSpec {
    pub binary: PathBuf,
    /// Directory for the log and the crash marker; must exist.
    pub work_dir: PathBuf,
    /// Connection string the daemon uses; normally through the
    /// [`crate::db_proxy`].
    pub database_url: String,
    pub tenant_id: Uuid,
    /// Fake venue base URL.
    pub venue_url: String,
    /// Crash point to arm; `None` runs without one.
    pub crash_point: Option<String>,
    /// Test-side hard deadline, measured from spawn.
    pub deadline: Duration,
    /// Further environment for the daemon.
    pub extra_env: Vec<(String, String)>,
}

/// A running daemon. Dropping it SIGKILLs and reaps the process.
pub struct DaemonProcess {
    child: Arc<Mutex<Child>>,
    watchdog_fired: Arc<AtomicBool>,
    finished: Arc<AtomicBool>,
    api: SocketAddr,
    log_path: PathBuf,
    marker_path: PathBuf,
}

impl DaemonProcess {
    /// Spawn the daemon described by `spec`.
    pub fn spawn(spec: &DaemonSpec) -> Result<Self> {
        let api = free_loopback_addr()?;
        let run_id = Uuid::now_v7();
        let log_path = spec.work_dir.join(format!("robsond-{run_id}.log"));
        let marker_path = spec.work_dir.join(format!("crash-marker-{run_id}"));
        let log = std::fs::File::create(&log_path).context("daemon log file")?;

        let mut command = Command::new(&spec.binary);
        command
            .env_clear()
            // No .env from a checkout is picked up from here.
            .current_dir(&spec.work_dir)
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log)
            .env("RUST_LOG", "robsond=info,robson_exec=info")
            .env("ROBSON_ENV", "test")
            .env("ROBSON_API_HOST", "127.0.0.1")
            .env("ROBSON_API_PORT", api.port().to_string())
            .env("DATABASE_URL", &spec.database_url)
            .env("PROJECTION_TENANT_ID", spec.tenant_id.to_string())
            .env("ROBSON_VENUE", "binance_usdm")
            .env("ROBSON_BINANCE_USE_TESTNET", "true")
            .env("ROBSON_BINANCE_API_KEY", "fixture-key")
            .env("ROBSON_BINANCE_API_SECRET", "fixture-secret")
            .env("ROBSON_HARNESS_VENUE_URL", &spec.venue_url)
            .env("ROBSON_MARKET_DATA_SYMBOLS", crate::fake_venue::FAKE_SYMBOL)
            // Never dialled to anything real: nothing listens on port 1.
            .env("ROBSON_MARKET_DATA_WS_ENDPOINTS", "wss://127.0.0.1:1/harness")
            .env("ROBSON_CRASH_MARKER_FILE", &marker_path)
            .env(
                "ROBSON_CRASH_WATCHDOG_SECS",
                (spec.deadline + SELF_WATCHDOG_MARGIN).as_secs().to_string(),
            );
        if let Some(point) = &spec.crash_point {
            command.env("ROBSON_CRASH_POINT", point);
        }
        command.envs(spec.extra_env.iter().map(|(key, value)| (key, value)));

        let child = command.spawn().context("spawn robsond")?;
        let process = Self {
            child: Arc::new(Mutex::new(child)),
            watchdog_fired: Arc::default(),
            finished: Arc::default(),
            api,
            log_path,
            marker_path,
        };
        process.start_watchdog(spec.deadline);
        Ok(process)
    }

    /// Wrap an already spawned child in the test-side watchdog; used to test
    /// it.
    pub fn from_child(child: Child, deadline: Duration, work_dir: &Path) -> Result<Self> {
        let process = Self {
            child: Arc::new(Mutex::new(child)),
            watchdog_fired: Arc::default(),
            finished: Arc::default(),
            api: free_loopback_addr()?,
            log_path: work_dir.join("no-log"),
            marker_path: work_dir.join("no-marker"),
        };
        process.start_watchdog(deadline);
        Ok(process)
    }

    fn start_watchdog(&self, deadline: Duration) {
        let child = self.child.clone();
        let fired = self.watchdog_fired.clone();
        let finished = self.finished.clone();
        let started = Instant::now();
        std::thread::spawn(move || {
            while !finished.load(Ordering::SeqCst) {
                if started.elapsed() >= deadline {
                    fired.store(true, Ordering::SeqCst);
                    let mut child = child.lock().unwrap_or_else(|e| e.into_inner());
                    let _ = child.kill();
                    let _ = child.wait();
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        });
    }

    /// The daemon's API base URL.
    pub fn api_url(&self) -> String {
        format!("http://{}", self.api)
    }

    fn try_wait(&self) -> Result<Option<ExitStatus>> {
        let mut child = self.child.lock().unwrap_or_else(|e| e.into_inner());
        Ok(child.try_wait()?)
    }

    /// Whether the test-side watchdog had to kill the child.
    pub fn watchdog_fired(&self) -> bool {
        self.watchdog_fired.load(Ordering::SeqCst)
    }

    /// Wait until the API answers.
    pub async fn wait_ready(&self, within: Duration) -> Result<()> {
        let started = Instant::now();
        let client = reqwest::Client::new();
        let url = format!("{}/healthz", self.api_url());
        loop {
            if let Some(status) = self.try_wait()? {
                bail!("daemon exited before ready: {status}\n{}", self.log_tail());
            }
            let answered = client
                .get(&url)
                .timeout(Duration::from_secs(2))
                .send()
                .await
                .is_ok_and(|response| response.status().is_success());
            if answered {
                return Ok(());
            }
            if started.elapsed() >= within {
                bail!("daemon not ready within {within:?}\n{}", self.log_tail());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Wait until the process has exited, within `within`.
    pub async fn wait_exit(&self, within: Duration) -> Result<ExitStatus> {
        let started = Instant::now();
        loop {
            if let Some(status) = self.try_wait()? {
                return Ok(status);
            }
            if started.elapsed() >= within {
                bail!("daemon still running after {within:?}\n{}", self.log_tail());
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Wait until the process dies by SIGKILL at `point`, as named by its
    /// marker file. Any other death is an error.
    pub async fn wait_killed_at(&self, point: &str, within: Duration) -> Result<()> {
        let status = self.wait_exit(within).await?;
        if status.signal() != Some(SIGKILL) || self.watchdog_fired() {
            bail!(
                "expected death by SIGKILL at {point}, got {status} (test watchdog fired: {})\n{}",
                self.watchdog_fired(),
                self.log_tail()
            );
        }
        let marker = std::fs::read_to_string(&self.marker_path)
            .map_err(|e| anyhow!("no crash marker for {point}: {e}\n{}", self.log_tail()))?;
        if marker != point {
            bail!("died at {marker}, expected {point}\n{}", self.log_tail());
        }
        Ok(())
    }

    /// The end of the daemon's log, for failure messages.
    pub fn log_tail(&self) -> String {
        let log = std::fs::read_to_string(&self.log_path).unwrap_or_default();
        let lines: Vec<&str> = log.lines().rev().take(60).collect();
        lines.into_iter().rev().collect::<Vec<_>>().join("\n")
    }
}

impl Drop for DaemonProcess {
    fn drop(&mut self) {
        self.finished.store(true, Ordering::SeqCst);
        let mut child = self.child.lock().unwrap_or_else(|e| e.into_inner());
        let _ = child.kill();
        let _ = child.wait();
    }
}

fn free_loopback_addr() -> Result<SocketAddr> {
    Ok(TcpListener::bind("127.0.0.1:0")?.local_addr()?)
}

/// Connection string for `database` through `proxy`, built from `base`
/// (`postgres[ql]://user:password@host[:port]/db[?params]`) so the credentials
/// stay whatever the environment supplied.
pub fn database_url_through(base: &str, proxy: SocketAddr, database: &str) -> Result<String> {
    let (scheme, rest) = base.split_once("://").context("DATABASE_URL has no scheme")?;
    let (credentials, _) = rest.rsplit_once('@').context("DATABASE_URL has no credentials")?;
    Ok(format!("{scheme}://{credentials}@{proxy}/{database}?sslmode=disable"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_database_url_keeps_its_credentials_and_changes_the_rest() {
        let proxy: SocketAddr = "127.0.0.1:4000".parse().unwrap();
        assert_eq!(
            database_url_through("postgresql://user:p%40ss@localhost:5432/base?x=1", proxy, "t1")
                .unwrap(),
            "postgresql://user:p%40ss@127.0.0.1:4000/t1?sslmode=disable"
        );
    }

    /// A stuck child and a stuck test: the watchdog kills the child at the
    /// deadline with nobody waiting on it.
    #[test]
    fn the_test_side_watchdog_kills_a_stuck_child() {
        let child = Command::new("sleep").arg("600").spawn().unwrap();
        let dir = std::env::temp_dir();
        let process = DaemonProcess::from_child(child, Duration::from_millis(200), &dir).unwrap();

        let started = Instant::now();
        while !process.watchdog_fired() && started.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(process.watchdog_fired());
        let status = loop {
            if let Some(status) = process.try_wait().unwrap() {
                break status;
            }
            assert!(started.elapsed() < Duration::from_secs(10));
            std::thread::sleep(Duration::from_millis(20));
        };
        assert_eq!(status.signal(), Some(SIGKILL));
    }

    #[test]
    fn dropping_the_handle_kills_the_child() {
        let child = Command::new("sleep").arg("600").spawn().unwrap();
        let pid = child.id();
        let dir = std::env::temp_dir();
        let process = DaemonProcess::from_child(child, Duration::from_secs(600), &dir).unwrap();
        drop(process);
        assert!(
            !Path::new(&format!("/proc/{pid}")).exists(),
            "child {pid} must be dead and reaped after drop"
        );
    }
}
