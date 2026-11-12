//! The debug stack budget (ADR-0056 step 8, stack headroom): the heaviest
//! lib-test scenarios, each re-run as a child of this test binary with a
//! 1 MiB thread stack, half the 2 MiB default. A scenario that needs more
//! fails here, by name, instead of aborting the whole binary on CI.
//!
//! Frame layouts are fixed for a binary, but the chain a scenario exercises
//! can vary, so this is a regression check over sampled scenarios, chosen as
//! the heaviest measured. The child must run exactly one test and pass it;
//! anything else fails, with the cause and the redacted tails of its output.
//!
//! The child's output is read by two threads. Once the child has ended they
//! get [`READ_DEADLINE`] to reach end of file; a reader still blocked then
//! (a descendant holding the pipe open) is abandoned and named as the cause.

use std::{
    collections::VecDeque,
    io::{BufRead, BufReader, Read},
    process::{Command, ExitStatus, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

/// The stack every scenario must fit, in bytes.
const BUDGET: usize = 1024 * 1024;

/// The heaviest scenarios, measured 2026-11-12 (stack-plan.md §3).
const SCENARIOS: &[&str] = &[
    "daemon::tests::a_main_loop_event_runs_under_the_main_loop_permit",
    "daemon::tests::a_stuck_main_loop_event_does_not_hold_the_daemon_past_the_stop_deadline",
    "position_manager::tests::test_different_capital_produces_different_position_size",
    "position_manager::tests::test_arm_immediate_automatic_processes_entry_without_market_tick",
    "api::tests::an_injected_signal_enters_and_protects_under_its_one_permit",
    "position_manager::tests::an_immediate_arm_under_a_sweep_claim_sends_nothing",
    "position_manager::tests::an_immediate_arm_enters_and_protects_under_its_own_permit",
    "position_manager::tests::test_e2e_detector_ma_crossover_signal",
    "position_manager::tests::test_restore_armed_immediate_automatic_processes_first_market_tick",
    "position_manager::tests::an_entry_refused_at_its_send_while_draining_ends_as_a_no_send",
];

/// How long one child may run before it is killed.
const DEADLINE: Duration = Duration::from_secs(60);

/// How long the readers may take to reach end of file once the child ended.
const READ_DEADLINE: Duration = Duration::from_secs(10);

/// The longest line kept; the rest of it is read and dropped.
const LINE_CAP: usize = 4 * 1024;

/// The most output kept per stream: whole redacted lines, each charged its
/// separator, so the rendered tail never exceeds it.
const TAIL_CAP: usize = 8 * 1024;

/// The most lines kept per stream, however short.
const TAIL_LINES: usize = 256;

const OVERFLOW: &str = "has overflowed its stack";

/// What draining one stream left: its evidence, gathered line by line, apart
/// from its redacted diagnostic text.
#[derive(Default)]
struct Drained {
    ran_one: bool,
    summary: Option<Summary>,
    /// The last `test result:` line, redacted: diagnostics only.
    summary_text: Option<String>,
    overflow: bool,
    tail: VecDeque<String>,
    tail_bytes: usize,
}

impl Drained {
    fn tail(&self) -> String {
        self.tail.iter().map(String::as_str).collect::<Vec<_>>().join("\n")
    }

    /// Keep `line`, already redacted, charging its separator.
    fn keep(&mut self, line: String) {
        self.tail_bytes += line.len() + 1;
        self.tail.push_back(line);
        while self.tail_bytes > TAIL_CAP || self.tail.len() > TAIL_LINES {
            let dropped = self.tail.pop_front().expect("a non-empty tail");
            self.tail_bytes -= dropped.len() + 1;
        }
    }
}

/// A libtest summary line's counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Summary {
    ok: bool,
    passed: u64,
    failed: u64,
    ignored: u64,
}

/// Parse `test result: ok. 1 passed; 0 failed; 0 ignored; …`.
fn parse_summary(line: &str) -> Option<Summary> {
    let (status, counts) = line.strip_prefix("test result: ")?.split_once(". ")?;
    let count = |label: &str| {
        counts
            .split(';')
            .find_map(|part| part.trim().strip_suffix(label)?.trim().parse().ok())
    };
    Some(Summary {
        ok: status == "ok",
        passed: count("passed")?,
        failed: count("failed")?,
        ignored: count("ignored")?,
    })
}

/// Read `stream` to its end with lines capped at [`LINE_CAP`]: record the
/// evidence of each line, then keep it, redacted, in a tail of at most
/// [`TAIL_CAP`] bytes of whole lines.
fn drain(stream: impl Read, secrets: &[String]) -> Drained {
    let mut reader = BufReader::new(stream);
    let mut drained = Drained::default();
    loop {
        let mut line = Vec::new();
        let mut truncated = false;
        let mut ended = false;
        loop {
            let buffer = match reader.fill_buf() {
                Ok(buffer) => buffer,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => &[],
            };
            if buffer.is_empty() {
                ended = true;
                break;
            }
            let (chunk, consumed, complete) = match buffer.iter().position(|&b| b == b'\n') {
                Some(end) => (&buffer[..end], end + 1, true),
                None => (buffer, buffer.len(), false),
            };
            let room = LINE_CAP - line.len();
            truncated |= chunk.len() > room;
            line.extend_from_slice(&chunk[..chunk.len().min(room)]);
            reader.consume(consumed);
            if complete {
                break;
            }
        }
        if ended && line.is_empty() {
            return drained;
        }
        let text = String::from_utf8_lossy(&line);
        drained.ran_one |= text == "running 1 test";
        drained.overflow |= text.contains(OVERFLOW);
        let mut kept = redact(&text, truncated, secrets);
        if text.starts_with("test result:") {
            drained.summary = parse_summary(&text);
            drained.summary_text = Some(kept.clone());
        }
        if truncated {
            kept.push_str(" [line cut]");
        }
        drained.keep(kept);
        if ended {
            return drained;
        }
    }
}

/// Redact one line before it is kept: each secret's exact value, a URL's
/// userinfo, and, in a cut line, a URL whose userinfo the cut may have
/// separated from its `@`, or a secret the cut left only the start of.
fn redact(line: &str, truncated: bool, secrets: &[String]) -> String {
    let mut out = line.to_string();
    for secret in secrets.iter().filter(|secret| !secret.is_empty()) {
        out = out.replace(secret.as_str(), "[redacted]");
        if truncated {
            let bytes = secret.as_bytes();
            if let Some(len) =
                (4..bytes.len()).rev().find(|&len| out.as_bytes().ends_with(&bytes[..len]))
            {
                out.truncate(out.len() - len);
                out.push_str("[redacted]");
            }
        }
    }
    // After `scheme://`, the authority runs to the first `/`, `?`, `#`,
    // whitespace, double quote or the end; userinfo is everything before its
    // last `@`. When in doubt, more goes: an `@` later in the same word (a raw
    // `/`, `?` or `#` in a password) redacts up to it, and a cut line's
    // authority with no `@` is redacted to the end.
    let mut from = 0;
    while let Some(found) = out[from..].find("://") {
        let start = from + found + 3;
        let word_end = out[start..]
            .find(|c: char| c.is_whitespace() || c == '"')
            .map_or(out.len(), |end| start + end);
        let authority_end =
            out[start..word_end].find(['/', '?', '#']).map_or(word_end, |end| start + end);
        let at = out[start..authority_end].rfind('@').or_else(|| out[start..word_end].rfind('@'));
        if let Some(at) = at {
            out.replace_range(start..start + at, "***");
            from = start + 3;
        } else if truncated && word_end == out.len() {
            out.replace_range(start.., "***[cut]");
            break;
        } else {
            from = start;
        }
    }
    out
}

/// Run `name` alone as a child of this binary under the budget; `Err` says why
/// it did not pass.
fn run(exe: &std::path::Path, name: &str, secrets: &[String]) -> Result<(), String> {
    let mut child = Command::new(exe)
        .args([name, "--exact", "--test-threads=1"])
        .env("RUST_MIN_STACK", BUDGET.to_string())
        .env_remove("DATABASE_URL")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("could not start: {error}"))?;
    let (out, err) = (child.stdout.take().expect("piped"), child.stderr.take().expect("piped"));
    let (out_secrets, err_secrets) = (secrets.to_vec(), secrets.to_vec());
    let (out_sender, out_receiver) = mpsc::channel();
    let (err_sender, err_receiver) = mpsc::channel();
    thread::spawn(move || out_sender.send(drain(out, &out_secrets)));
    thread::spawn(move || err_sender.send(drain(err, &err_secrets)));
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if started.elapsed() < DEADLINE => thread::sleep(Duration::from_millis(20)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(format!("did not finish within {DEADLINE:?}: killed and reaped"));
            },
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(format!("could not be waited for: {error}"));
            },
        }
    };
    let ended = Instant::now();
    let out = out_receiver.recv_timeout(READ_DEADLINE);
    let err = err_receiver.recv_timeout(READ_DEADLINE.saturating_sub(ended.elapsed()));
    let (Ok(out), Ok(err)) = (out, err) else {
        return Err(redact(
            &format!(
                "{}; its output was still open {READ_DEADLINE:?} after it ended (a descendant \
                 holding the pipe?): reader abandoned",
                status.err().unwrap_or_else(|| "finished".to_string())
            ),
            false,
            secrets,
        ));
    };
    status
        .and_then(|status| judge(status, &out, &err))
        .map_err(|cause| report(&cause, &out, &err, secrets))
}

/// The failure message: the cause, redacted like everything else that reaches
/// it, then each stream's redacted tail.
fn report(cause: &str, out: &Drained, err: &Drained, secrets: &[String]) -> String {
    format!(
        "{}\n--- stdout, last {TAIL_CAP} B, redacted ---\n{}\n--- stderr, last {TAIL_CAP} B, redacted ---\n{}",
        redact(cause, false, secrets),
        out.tail(),
        err.tail()
    )
}

/// Whether a finished child ran exactly one test and passed it.
fn judge(status: ExitStatus, out: &Drained, err: &Drained) -> Result<(), String> {
    use std::os::unix::process::ExitStatusExt;
    if err.overflow {
        return Err(format!("needs more than the {BUDGET}-byte debug stack budget"));
    }
    match (status.code(), status.signal()) {
        (Some(0), _) => {},
        (Some(code), _) => return Err(format!("failed: exit code {code}")),
        (None, Some(6)) => return Err("aborted, cause unknown".to_string()),
        (None, signal) => return Err(format!("killed by signal {signal:?}")),
    }
    let counts = |passed, ignored| Some(Summary { ok: true, passed, failed: 0, ignored });
    match out.summary {
        summary if out.ran_one && summary == counts(1, 0) => Ok(()),
        summary if summary == counts(0, 1) => {
            Err("ignored: a budget scenario must run".to_string())
        },
        summary if summary == counts(0, 0) => {
            Err("not found: renamed, removed or gated out; update the list".to_string())
        },
        _ => Err(format!("unexpected summary: {}", out.summary_text.as_deref().unwrap_or("none"))),
    }
}

/// Every listed scenario fits the budget in this build (ADR-0056 step 8).
#[test]
fn the_heaviest_paths_fit_the_debug_stack_budget() {
    let exe = std::env::current_exe().expect("this test binary");
    let secrets: Vec<String> = std::env::var("DATABASE_URL").into_iter().collect();
    let failures: Vec<String> = SCENARIOS
        .iter()
        .filter_map(|name| run(&exe, name, &secrets).err().map(|cause| format!("{name}: {cause}")))
        .collect();
    assert!(
        failures.is_empty(),
        "debug stack budget, {BUDGET} B:\n{}",
        failures.join("\n\n")
    );
}

/// A synthetic credential; never a real one.
const CANARY: &str = "postgres://canary-user:canary-pass@127.0.0.1:5432/canary";

fn assert_clean(text: &str) {
    for part in [
        "canary-user",
        "canary-pass",
        "canary-u",
        "canary-p",
        "nary-pa",
        "-pass@",
        "user:",
    ] {
        assert!(!text.contains(part), "{part} survived in {text:?}");
    }
}

#[test]
fn a_url_userinfo_is_redacted_without_any_secret() {
    let line = format!("connecting to {CANARY} (attempt 2)");
    let redacted = redact(&line, false, &[]);
    assert_clean(&redacted);
    assert!(redacted.contains("postgres://***@127.0.0.1:5432/canary"), "{redacted}");
}

#[test]
fn an_exact_database_url_value_is_redacted() {
    let secret = "host=db user=canary-user password=canary-pass".to_string();
    let redacted = redact(&format!("dsn {secret} rejected"), false, &[secret]);
    assert_clean(&redacted);
    assert_eq!(redacted, "dsn [redacted] rejected");
}

#[test]
fn a_credential_at_the_tail_boundary_never_survives() {
    // A credential line of 71 bytes, then `filler` bytes of output: with 8,098
    // the whole line is the oldest one kept; with 8,155 a byte-level tail of
    // TAIL_CAP would start inside its password.
    let credential = format!("password {CANARY} here\n");
    assert_eq!(credential.len(), 71);
    for filler in [
        TAIL_CAP - credential.len() - 23,
        TAIL_CAP - credential.len() + 34,
    ] {
        let mut input = credential.clone();
        let mut left = filler;
        while left > 0 {
            let line = left.min(101);
            input.push_str(&"x".repeat(line - 1));
            input.push('\n');
            left -= line;
        }
        for secrets in [vec![], vec![CANARY.to_string()]] {
            let drained = drain(input.as_bytes(), &secrets);
            assert_clean(&drained.tail());
            assert!(drained.tail_bytes <= TAIL_CAP, "{}", drained.tail_bytes);
        }
    }
}

#[test]
fn an_overlong_line_is_cut_and_its_cut_credential_redacted() {
    // The credential crosses the line cap, and the line runs on far past it.
    let line =
        format!("{}{CANARY}{}\nrunning 1 test\n", "a".repeat(LINE_CAP - 20), "b".repeat(100_000));
    let drained = drain(line.as_bytes(), &[]);
    assert_clean(&drained.tail());
    assert!(drained.ran_one, "a line after the overlong one is still read");
    let first = drained.tail.front().expect("the cut line");
    assert!(first.len() <= LINE_CAP + "***[cut] [line cut]".len(), "{}", first.len());
    assert!(first.ends_with("[line cut]"), "{first}");
}

#[test]
fn a_million_empty_lines_keep_the_tail_bounded() {
    let empty = drain("\n".repeat(1_000_000).as_bytes(), &[]);
    assert!(empty.tail.len() <= TAIL_LINES, "{} lines", empty.tail.len());
    let full = drain(format!("{}\n", "x".repeat(100)).repeat(1_000).as_bytes(), &[]);
    for drained in [empty, full] {
        assert!(drained.tail_bytes <= TAIL_CAP, "{} bytes", drained.tail_bytes);
        assert!(drained.tail().len() <= drained.tail_bytes, "rendered within the charge");
    }
}

#[test]
fn userinfo_punctuation_never_ends_the_redaction() {
    for (user, pass) in [
        ("canary-user", "canary-pass(alt)"),
        ("canary(u)", "canary(p)"),
        ("canary'user", "canary'pass"),
        ("canary!user", "canary!pass"),
        ("canary*user", "canary*pass"),
        ("canary;user", "canary;pass"),
        ("canary=user", "canary=pass"),
        ("canary-user", "canary/pass"),
        ("canary-user", "canary#pass?x"),
    ] {
        let line = format!("refused: postgres://{user}:{pass}@db/test now");
        let redacted = redact(&line, false, &[]);
        assert_eq!(redacted, "refused: postgres://***@db/test now", "{line}");
    }
}

#[test]
fn a_credential_in_a_summary_line_never_reaches_the_message() {
    use std::os::unix::process::ExitStatusExt;
    let child = format!("running 1 test\ntest result: {CANARY} 1 passed\n");
    let (out, err) = (drain(child.as_bytes(), &[]), drain(&b""[..], &[]));
    let cause = judge(ExitStatus::from_raw(0), &out, &err).expect_err("not a pass");
    assert!(cause.starts_with("unexpected summary"), "{cause}");
    let message = report(&cause, &out, &err, &[]);
    assert_clean(&message);
    assert!(message.contains("postgres://***@"), "{message}");
}
