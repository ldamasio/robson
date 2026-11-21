//! Supervised Spot preflight (`robsond spot-supervised-preflight`).
//!
//! A supervisor-launched child that talks to its parent over one connected
//! AF_UNIX stream inherited as stdin. The trust boundary is the local POSIX
//! UID: the socket peer must be the current euid and the unchanged non-init
//! parent, checked natively before every read is accepted. This
//! authenticates the initiating OS principal and a live parent; it does not
//! prove a remote signer is fenced. The parent owns the real supervisor
//! closure and its revocation on every outcome.
//!
//! The command verifies the isolated loopback PostgreSQL 16 target named by
//! typed launch fields, then rehearses epoch, install and descriptive source
//! audit under that fence. Final C2/2b and inventory gates remain closed.
//! Errors are stable categories only; no credential, DSN or foreign frame text
//! is ever printed.

use std::{
    net::IpAddr,
    os::fd::{AsFd, AsRawFd, RawFd},
    time::Duration,
};

use robson_exec::order_journal::DEFAULT_ACCOUNT_SCOPE;
use serde::{Deserialize, Serialize};
use sqlx::{
    postgres::{PgConnectOptions, PgPoolOptions, PgSslMode},
    ConnectOptions, PgPool,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::UnixStream,
    time::{timeout, timeout_at, Instant},
};
use uuid::Uuid;

use crate::{
    spot_order_set_store::{
        cold_install::{ClosedWriterFence, FencePhase, FenceReceipt, FixtureFenceError},
        cold_owner::RestoreIdentity,
        spot_startup::{record_epoch, Admission, RestoredTarget, SpotSupervisor},
    },
    stop::StopRequest,
};

/// Cap on each frame, both directions, newline included.
const FRAME_CAP: u64 = 16 * 1024;
/// Matching receive and reply deadline for one exchange or native step.
const STEP_BOUND: Duration = Duration::from_secs(10);
/// The whole preflight, intake to report.
const WHOLE_BOUND: Duration = Duration::from_secs(60);
const SCHEMA: u32 = 1;
const ISOLATED_DB_PREFIX: &str = "_sqlx_test_";
const ISOLATED_DB_SUFFIX_LEN: usize = 52;
const PG16_MIN: i32 = 160_000;
const PG16_END: i32 = 170_000;

/// Stable refusal categories; the only failure text this command emits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    UnsupportedTransport,
    Peer,
    Frame,
    Stale,
    Eof,
    Target,
    Database,
    ServerVersion,
    Epoch,
    Capture,
    Install,
    Audit,
    Timeout,
    Cancelled,
}

impl Refusal {
    pub fn category(self) -> &'static str {
        match self {
            Self::UnsupportedTransport => "unsupported_transport",
            Self::Peer => "peer_not_parent",
            Self::Frame => "invalid_frame",
            Self::Stale => "stale_or_foreign_reply",
            Self::Eof => "parent_eof",
            Self::Target => "untrusted_target",
            Self::Database => "database_unavailable",
            Self::ServerVersion => "server_not_pg16",
            Self::Epoch => "supervised_epoch_refused",
            Self::Capture => "supervised_capture_refused",
            Self::Install => "supervised_install_refused",
            Self::Audit => "descriptive_c2_audit_refused",
            Self::Timeout => "deadline_exceeded",
            Self::Cancelled => "cancelled",
        }
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.category())
    }
}

impl std::error::Error for Refusal {}

/// Bootstrap frame from the parent. No `Debug`: it carries a password.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Hello {
    schema: u32,
    operation: Uuid,
    owner: Uuid,
    target: Uuid,
    database: String,
    host: String,
    port: u16,
    installer_user: String,
    installer_password: String,
    successor_user: String,
    system_identifier: String,
    data_directory: String,
    postmaster_started: String,
    tenant_id: Uuid,
    account_uid: String,
    accounting_version: i64,
}

/// Stage the child asks the parent to approve.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Phase {
    Connect,
    BeforeEpoch,
    BeforeCapture,
    BeforeInstall,
    BeforePublish,
    AfterReadback,
    BeforeSourceAudit,
    AfterSourceAudit,
}

/// Child request; the parent echoes this exact object as its approval.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Challenge {
    schema: u32,
    seq: u64,
    challenge: Uuid,
    operation: Uuid,
    owner: Uuid,
    target: Uuid,
    phase: Phase,
}

struct Binding {
    target: RestoredTarget,
    owner: Uuid,
    successor_user: String,
}

struct Launch {
    binding: Binding,
    options: PgConnectOptions,
    tenant: Uuid,
    account_uid: String,
    accounting_version: i64,
}

#[derive(Serialize)]
struct Receipt {
    preflight: &'static str,
    schema: u32,
    approvals: u64,
    server_version_num: i32,
    epoch_id: Uuid,
    generation_id: Uuid,
    source_audit_id: Uuid,
    source_examined: i64,
    source_sql: u64,
    successor_opened: bool,
    venue_calls: u32,
    stopped_at: [&'static str; 2],
}

/// Run the isolated rehearsal under the operator's stop and parent lifetime.
pub async fn run(stop: &StopRequest) -> Result<(), Refusal> {
    if stop.is_asked() {
        return Err(Refusal::Cancelled);
    }
    let channel = Channel::from_stdin()?;
    let deadline = Instant::now() + WHOLE_BOUND;
    let receipt = tokio::select! {
        biased;
        _ = stop.asked() => return Err(Refusal::Cancelled),
        lost = wait_for_parent_loss(channel.fd, channel.parent) => return Err(lost),
        result = timeout_at(deadline, preflight(channel, deadline)) => {
            result.map_err(|_| Refusal::Timeout)??
        },
    };
    let line = serde_json::to_string(&receipt).map_err(|_| Refusal::Frame)?;
    eprintln!("{line}");
    Ok(())
}

async fn preflight(mut channel: Channel, deadline: Instant) -> Result<Receipt, Refusal> {
    let launch = channel.hello().await?;
    channel.approve(&launch.binding, Phase::Connect, deadline).await?;
    let pool = timeout(
        STEP_BOUND,
        PgPoolOptions::new()
            .max_connections(2)
            .test_before_acquire(false)
            .connect_with(launch.options),
    )
    .await
    .map_err(|_| Refusal::Timeout)?
    .map_err(|_| Refusal::Database)?;
    let version = timeout(
        STEP_BOUND,
        sqlx::query_scalar::<_, i32>("SELECT current_setting('server_version_num')::int4")
            .fetch_one(&pool),
    )
    .await
    .map_err(|_| Refusal::Timeout)?
    .map_err(|_| Refusal::Database)?;
    if !(PG16_MIN..PG16_END).contains(&version) {
        return Err(Refusal::ServerVersion);
    }

    let supervisor = Supervisor { channel, binding: launch.binding };
    let epoched = record_epoch(
        pool.clone(),
        supervisor,
        launch.tenant,
        DEFAULT_ACCOUNT_SCOPE,
        env!("CARGO_PKG_VERSION"),
        deadline,
    )
    .await
    .map_err(|_| Refusal::Epoch)?;
    let epoch_id = epoched.epoch().epoch.epoch_id;
    let captured = epoched
        .capture(&launch.account_uid, launch.accounting_version)
        .await
        .map_err(|_| Refusal::Capture)?;
    let mut installed = captured.install().await.map_err(|_| Refusal::Install)?;
    let generation_id = installed.generation().generation_id;
    let audit = installed.audit_source_for_preflight().await.map_err(|_| Refusal::Audit)?;
    let approvals = installed.supervisor().channel.seq;
    pool.close().await;
    drop(installed);

    Ok(Receipt {
        preflight: "spot_supervised",
        schema: SCHEMA,
        approvals,
        server_version_num: version,
        epoch_id,
        generation_id,
        source_audit_id: audit.id,
        source_examined: audit.examined,
        source_sql: audit.commands,
        successor_opened: false,
        venue_calls: 0,
        stopped_at: ["c2_2b_gate_closed", "inventory_gate_closed"],
    })
}

/// Bound parent loss during an in-flight SQL stage, without consuming an
/// approval frame or spawning a task. Normal request/reply checks also
/// verify the peer immediately before accepting any authority.
async fn wait_for_parent_loss(fd: RawFd, parent: libc::pid_t) -> Refusal {
    loop {
        let mut poll = libc::pollfd { fd, events: libc::POLLRDHUP, revents: 0 };
        // SAFETY: this points to one initialized pollfd and the call is nonblocking.
        let status = unsafe { libc::poll(&mut poll, 1, 0) };
        if status > 0 && poll.revents != 0 {
            return Refusal::Eof;
        }
        // SAFETY: getppid has no preconditions.
        if unsafe { libc::getppid() } != parent {
            return Refusal::Peer;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

struct Supervisor {
    channel: Channel,
    binding: Binding,
}

impl ClosedWriterFence for Supervisor {
    async fn check(
        &mut self,
        restore: RestoreIdentity,
        phase: FencePhase,
        deadline: Instant,
    ) -> Result<FenceReceipt, FixtureFenceError> {
        if restore != self.binding.target.restore {
            return Err(FixtureFenceError);
        }
        let phase = match phase {
            FencePhase::BeforeEpoch => Phase::BeforeEpoch,
            FencePhase::BeforeCapture => Phase::BeforeCapture,
            FencePhase::BeforeRead => Phase::BeforeInstall,
            FencePhase::BeforePublish => Phase::BeforePublish,
            FencePhase::AfterReadback => Phase::AfterReadback,
            FencePhase::BeforeSourceAudit => Phase::BeforeSourceAudit,
            FencePhase::AfterSourceAudit => Phase::AfterSourceAudit,
        };
        self.channel
            .approve(&self.binding, phase, deadline)
            .await
            .map_err(|_| FixtureFenceError)?;
        Ok(FenceReceipt { restore, owner: self.binding.owner })
    }
}

impl SpotSupervisor for Supervisor {
    fn target(&self) -> &RestoredTarget {
        &self.binding.target
    }
    fn successor_login(&self) -> &str {
        &self.binding.successor_user
    }

    async fn admit(&mut self, _: &Admission<'_>, _: Instant) -> Result<PgPool, FixtureFenceError> {
        Err(FixtureFenceError)
    }
}

/// The sole owner of the inherited connection. Dropping it shuts the socket
/// down, so cancellation, EOF and errors all close the parent's end.
struct Channel {
    reader: BufReader<UnixStream>,
    fd: RawFd,
    parent: libc::pid_t,
    seq: u64,
    last_challenge: Option<Uuid>,
}

impl Channel {
    fn from_stdin() -> Result<Self, Refusal> {
        let owned = std::io::stdin()
            .as_fd()
            .try_clone_to_owned()
            .map_err(|_| Refusal::UnsupportedTransport)?;
        let fd = owned.as_raw_fd();
        if sockopt_int(fd, libc::SO_DOMAIN)? != libc::AF_UNIX
            || sockopt_int(fd, libc::SO_TYPE)? != libc::SOCK_STREAM
        {
            return Err(Refusal::UnsupportedTransport);
        }
        // SAFETY: getppid has no preconditions.
        let parent = unsafe { libc::getppid() };
        if parent <= 1 {
            return Err(Refusal::Peer);
        }
        check_peer(fd, parent)?;
        let std_stream = std::os::unix::net::UnixStream::from(owned);
        std_stream.set_nonblocking(true).map_err(|_| Refusal::UnsupportedTransport)?;
        let stream = UnixStream::from_std(std_stream).map_err(|_| Refusal::UnsupportedTransport)?;
        Ok(Self {
            reader: BufReader::with_capacity(FRAME_CAP as usize, stream),
            fd,
            parent,
            seq: 0,
            last_challenge: None,
        })
    }

    async fn hello(&mut self) -> Result<Launch, Refusal> {
        let bytes = self.read_frame().await?;
        check_peer(self.fd, self.parent)?;
        let hello: Hello = serde_json::from_slice(&bytes).map_err(|_| Refusal::Frame)?;
        validate_hello(hello)
    }

    async fn approve(
        &mut self,
        binding: &Binding,
        phase: Phase,
        deadline: Instant,
    ) -> Result<(), Refusal> {
        check_peer(self.fd, self.parent)?;
        let challenge = Uuid::now_v7();
        if self.last_challenge == Some(challenge) {
            return Err(Refusal::Stale);
        }
        self.seq += 1;
        self.last_challenge = Some(challenge);
        let ask = Challenge {
            schema: SCHEMA,
            seq: self.seq,
            challenge,
            operation: binding.target.restore.operation,
            owner: binding.owner,
            target: binding.target.restore.target,
            phase,
        };
        let limit = deadline.min(Instant::now() + STEP_BOUND);
        let reply = timeout_at(limit, async {
            self.write_frame(&ask).await?;
            self.read_frame().await
        })
        .await
        .map_err(|_| Refusal::Timeout)??;
        check_peer(self.fd, self.parent)?;
        decode_echo(&reply, &ask)
    }

    async fn read_frame(&mut self) -> Result<Vec<u8>, Refusal> {
        let mut buf = Vec::new();
        let read =
            timeout(STEP_BOUND, (&mut self.reader).take(FRAME_CAP).read_until(b'\n', &mut buf))
                .await
                .map_err(|_| Refusal::Timeout)?
                .map_err(|_| Refusal::Eof)?;
        if read == 0 {
            return Err(Refusal::Eof);
        }
        if buf.pop() != Some(b'\n') {
            return Err(Refusal::Frame);
        }
        Ok(buf)
    }

    async fn write_frame(&mut self, value: &Challenge) -> Result<(), Refusal> {
        let mut bytes = serde_json::to_vec(value).map_err(|_| Refusal::Frame)?;
        bytes.push(b'\n');
        if bytes.len() as u64 > FRAME_CAP {
            return Err(Refusal::Frame);
        }
        let stream = self.reader.get_mut();
        timeout(STEP_BOUND, async {
            stream.write_all(&bytes).await?;
            stream.flush().await
        })
        .await
        .map_err(|_| Refusal::Timeout)?
        .map_err(|_| Refusal::Eof)
    }
}

impl Drop for Channel {
    fn drop(&mut self) {
        // SAFETY: `fd` belongs to the stream in `reader`, which is dropped
        // only after this runs.
        unsafe {
            libc::shutdown(self.fd, libc::SHUT_RDWR);
        }
    }
}

fn sockopt_int(fd: RawFd, name: libc::c_int) -> Result<libc::c_int, Refusal> {
    let mut value: libc::c_int = 0;
    let mut len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
    // SAFETY: value/len describe a valid c_int buffer.
    let rc = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            name,
            (&mut value as *mut libc::c_int).cast(),
            &mut len,
        )
    };
    if rc != 0 || len as usize != std::mem::size_of::<libc::c_int>() {
        return Err(Refusal::UnsupportedTransport);
    }
    Ok(value)
}

/// The peer must be the same euid and the unchanged, live, non-init parent.
fn check_peer(fd: RawFd, parent: libc::pid_t) -> Result<(), Refusal> {
    // SAFETY: getppid/geteuid have no preconditions.
    let (now_parent, euid) = unsafe { (libc::getppid(), libc::geteuid()) };
    if now_parent != parent || now_parent <= 1 {
        return Err(Refusal::Peer);
    }
    let mut cred = libc::ucred { pid: 0, uid: 0, gid: 0 };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: cred/len describe a valid ucred buffer.
    let rc = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut cred as *mut libc::ucred).cast(),
            &mut len,
        )
    };
    if rc != 0 || len as usize != std::mem::size_of::<libc::ucred>() {
        return Err(Refusal::Peer);
    }
    if cred.pid != parent || cred.uid != euid {
        return Err(Refusal::Peer);
    }
    Ok(())
}

fn decode_echo(reply: &[u8], ask: &Challenge) -> Result<(), Refusal> {
    let echoed: Challenge = serde_json::from_slice(reply).map_err(|_| Refusal::Frame)?;
    if &echoed != ask {
        return Err(Refusal::Stale);
    }
    Ok(())
}

fn validate_hello(hello: Hello) -> Result<Launch, Refusal> {
    if hello.schema != SCHEMA
        || hello.operation.is_nil()
        || hello.owner.is_nil()
        || hello.target.is_nil()
        || hello.tenant_id.is_nil()
        || hello.account_uid.trim().is_empty()
        || hello.account_uid.len() > 256
        || hello.account_uid.contains('\0')
        || hello.accounting_version < 0
    {
        return Err(Refusal::Frame);
    }
    // A literal loopback address only: no names, no resolution.
    let host: IpAddr = hello.host.parse().map_err(|_| Refusal::Target)?;
    if !host.is_loopback() || hello.port == 0 {
        return Err(Refusal::Target);
    }
    if !is_isolated_database(&hello.database)
        || !is_role_name(&hello.installer_user)
        || !is_role_name(&hello.successor_user)
        || hello.installer_user == hello.successor_user
        || hello.installer_password.is_empty()
        || hello.installer_password.len() > 1024
        || hello.installer_password.contains('\0')
        || !hello.data_directory.starts_with('/')
        || hello.data_directory.len() > 4096
        || hello.data_directory.contains('\0')
        || hello.system_identifier.parse::<u64>().map_or(true, |id| id == 0)
        || hello.postmaster_started.is_empty()
        || hello.postmaster_started.len() > 64
        || hello.postmaster_started.contains('\0')
    {
        return Err(Refusal::Target);
    }
    // Official SQLx still reads PGOPTIONS even with pgpass disabled.
    // Reject that and inherited certificate configuration before building options.
    if ["PGOPTIONS", "PGSSLROOTCERT", "PGSSLCERT", "PGSSLKEY"]
        .iter()
        .any(|name| std::env::var_os(name).is_some())
    {
        return Err(Refusal::Target);
    }
    let options = PgConnectOptions::new_without_pgpass()
        .host(&host.to_string())
        .port(hello.port)
        .database(&hello.database)
        .username(&hello.installer_user)
        .password(&hello.installer_password)
        .ssl_mode(PgSslMode::Disable)
        .application_name("robsond-spot-supervised-preflight")
        .options([
            ("TimeZone", "UTC"),
            ("search_path", "public"),
            ("statement_timeout", "2s"),
        ])
        .disable_statement_logging();
    Ok(Launch {
        binding: Binding {
            target: RestoredTarget {
                restore: RestoreIdentity {
                    operation: hello.operation,
                    target: hello.target,
                },
                database: hello.database,
                system_identifier: hello.system_identifier,
                data_directory: hello.data_directory,
                postmaster_started: hello.postmaster_started,
            },
            owner: hello.owner,
            successor_user: hello.successor_user,
        },
        options,
        tenant: hello.tenant_id,
        account_uid: hello.account_uid,
        accounting_version: hello.accounting_version,
    })
}

/// Exactly `_sqlx_test_` followed by 52 ASCII alphanumerics.
fn is_isolated_database(name: &str) -> bool {
    name.strip_prefix(ISOLATED_DB_PREFIX).is_some_and(|rest| {
        rest.len() == ISOLATED_DB_SUFFIX_LEN && rest.bytes().all(|b| b.is_ascii_alphanumeric())
    })
}

fn is_role_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 63
        && (bytes[0].is_ascii_lowercase() || bytes[0] == b'_')
        && bytes.iter().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ask(seq: u64) -> Challenge {
        Challenge {
            schema: SCHEMA,
            seq,
            challenge: Uuid::now_v7(),
            operation: Uuid::now_v7(),
            owner: Uuid::now_v7(),
            target: Uuid::now_v7(),
            phase: Phase::Connect,
        }
    }

    #[test]
    fn generated_echo_frames_accept_only_the_exact_challenge() {
        for seq in 1..64 {
            let expected = ask(seq);
            let exact = serde_json::to_vec(&expected).unwrap();
            assert_eq!(decode_echo(&exact, &expected), Ok(()));

            let mut replayed = expected.clone();
            replayed.seq = seq - 1;
            let bytes = serde_json::to_vec(&replayed).unwrap();
            assert_eq!(decode_echo(&bytes, &expected), Err(Refusal::Stale));

            let mut foreign = expected.clone();
            foreign.challenge = Uuid::now_v7();
            let bytes = serde_json::to_vec(&foreign).unwrap();
            assert_eq!(decode_echo(&bytes, &expected), Err(Refusal::Stale));

            let mut value = serde_json::to_value(&expected).unwrap();
            value["extra"] = serde_json::json!(seq);
            let bytes = serde_json::to_vec(&value).unwrap();
            assert_eq!(decode_echo(&bytes, &expected), Err(Refusal::Frame));

            let truncated = &exact[..exact.len() / 2];
            assert_eq!(decode_echo(truncated, &expected), Err(Refusal::Frame));
        }
    }

    #[test]
    fn isolated_database_name_is_exact() {
        let suffix = "a".repeat(ISOLATED_DB_SUFFIX_LEN);
        assert!(is_isolated_database(&format!("_sqlx_test_{suffix}")));
        assert!(!is_isolated_database(&format!("_sqlx_test_{suffix}a")));
        assert!(!is_isolated_database(&format!("_sqlx_test_{}", &suffix[1..])));
        assert!(!is_isolated_database(&format!("robson_{suffix}")));
        assert!(!is_isolated_database(&format!("_sqlx_test_{}-", &suffix[1..])));
    }
}
