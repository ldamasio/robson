//! Named crash points for the crash harness (ADR-0056 step 7).
//!
//! `crash_point!(NAME)` at a call site SIGKILLs this process when the harness
//! armed `NAME`, so a real `robsond` dies at a named place with no unwinding
//! and no shutdown path. Armed with [`STALL_ENV`] as well, the thread that
//! reaches `NAME` blocks there for good instead: an operation that never ends,
//! whatever the async runtime does. The call site expands to nothing without
//! the `crash-points` feature, and this module does not exist then. The feature
//! is rejected at compile time in a release build; production builds use
//! `--features postgres` (`Dockerfile`).
//!
//! To add a crash point, add one line to the table below and put
//! `crash_point!(ITS_NAME)` at the place.

#[cfg(not(debug_assertions))]
compile_error!(
    "feature `crash-points` must never be combined with a release build: it can SIGKILL the \
     process and redirect the venue connection"
);

use std::{io::Write, sync::OnceLock, time::Duration};

macro_rules! crash_points {
    ($($constant:ident = $name:literal;)*) => {
        $(pub const $constant: &str = $name;)*
        /// Every crash point; arming any other name is refused.
        pub const ALL: &[&str] = &[$($name),*];
    };
}

crash_points! {
    // The entry order is about to be handed to the venue adapter.
    ENTRY_BEFORE_SEND = "entry_before_send";
    // The venue answered the entry order; nothing has recorded the answer.
    ENTRY_AFTER_ACK = "entry_after_ack";
    // A protective stop's intent is committed; the request has not left.
    PROTECTION_BEFORE_SEND = "protection_before_send";
    // A stop passed its check; the clean-stop marker is not written yet.
    STOP_BEFORE_MARKER = "stop_before_marker";
    // The clean-stop marker is written; the process has not exited.
    STOP_AFTER_MARKER = "stop_after_marker";
    // The startup phase holds its permit; the startup scan has not run.
    STARTUP_BEFORE_SCAN = "startup_before_scan";
}

/// Crash point to arm.
pub const POINT_ENV: &str = "ROBSON_CRASH_POINT";
/// File that receives the point's name just before the process kills itself.
pub const MARKER_FILE_ENV: &str = "ROBSON_CRASH_MARKER_FILE";
/// Seconds after which the process SIGKILLs itself, whatever it is doing, so a
/// stuck child outlives neither its test nor a killed test runner.
pub const WATCHDOG_ENV: &str = "ROBSON_CRASH_WATCHDOG_SECS";
/// Loopback base URL of the fake venue that replaces every Binance REST host.
pub const VENUE_URL_ENV: &str = "ROBSON_HARNESS_VENUE_URL";
/// Set to `1` to stall the thread at the armed point instead of killing.
pub const STALL_ENV: &str = "ROBSON_CRASH_STALL";

static ARMED: OnceLock<Option<Armed>> = OnceLock::new();

struct Armed {
    point: &'static str,
    marker_file: Option<String>,
    stall: bool,
}

/// Validate the harness environment and arm it. Call first in `main`.
///
/// # Panics
/// With `ROBSON_ENV=production`, an unknown crash point or an invalid watchdog.
pub fn arm_from_env() {
    let environment = std::env::var("ROBSON_ENV").unwrap_or_default();
    let requested = std::env::var(POINT_ENV).ok();
    let marker_file = std::env::var(MARKER_FILE_ENV).ok();
    let stall = std::env::var(STALL_ENV).is_ok_and(|value| value == "1");
    let armed = resolve(&environment, requested.as_deref(), marker_file, stall)
        .unwrap_or_else(|refused| panic!("crash harness refused: {refused}"));
    let _ = ARMED.set(armed);

    if let Ok(seconds) = std::env::var(WATCHDOG_ENV) {
        let seconds: u64 = seconds
            .parse()
            .unwrap_or_else(|_| panic!("crash harness refused: invalid {WATCHDOG_ENV}"));
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(seconds));
            kill_self();
        });
    }
}

fn resolve(
    environment: &str,
    requested: Option<&str>,
    marker_file: Option<String>,
    stall: bool,
) -> Result<Option<Armed>, String> {
    if matches!(environment.to_lowercase().as_str(), "production" | "prod") {
        return Err("a crash-points build must not run with ROBSON_ENV=production".to_string());
    }
    let Some(requested) = requested else {
        return Ok(None);
    };
    let point = ALL
        .iter()
        .find(|known| **known == requested)
        .ok_or_else(|| format!("unknown crash point {requested}; known: {ALL:?}"))?;
    Ok(Some(Armed { point, marker_file, stall }))
}

/// Die here if `point` is the armed crash point, or stall here when armed to.
/// Reached through [`crash_point!`](crate::crash_point).
pub fn hit(point: &'static str) {
    let Some(armed) = ARMED.get_or_init(|| None) else {
        return;
    };
    if armed.point != point {
        return;
    }
    if let Some(path) = &armed.marker_file {
        let mut file = std::fs::File::create(path).expect("crash marker file");
        file.write_all(point.as_bytes()).expect("write crash marker");
        file.sync_all().expect("sync crash marker");
    }
    if armed.stall {
        loop {
            std::thread::park();
        }
    }
    kill_self();
}

fn kill_self() -> ! {
    // SAFETY: `kill` takes no pointers; this only signals this process.
    unsafe {
        libc::kill(libc::getpid(), libc::SIGKILL);
    }
    // Delivery is not synchronous for every thread. Nothing runs past here.
    loop {
        std::thread::park();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_is_refused_with_or_without_a_point() {
        for environment in ["production", "PROD"] {
            assert!(resolve(environment, None, None, false).is_err());
            assert!(resolve(environment, Some(ENTRY_AFTER_ACK), None, true).is_err());
        }
    }

    #[test]
    fn only_a_known_point_arms() {
        assert!(resolve("test", Some("nonexistent"), None, false).is_err());
        let armed = resolve("test", Some(ENTRY_BEFORE_SEND), None, true).unwrap().unwrap();
        assert_eq!((armed.point, armed.stall), (ENTRY_BEFORE_SEND, true));
        assert!(resolve("test", None, None, true).unwrap().is_none());
    }

    #[test]
    fn an_unarmed_hit_is_inert() {
        hit(ENTRY_AFTER_ACK);
    }
}
