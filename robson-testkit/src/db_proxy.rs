//! A TCP proxy in front of PostgreSQL that injects failures (ADR-0056 step 7).
//!
//! It follows the frontend and backend message framing, so it can tell a
//! `COMMIT` from any other statement. A client connects through it without
//! TLS; a TLS request is answered with "not supported". The proxy never starts
//! a database; it only sits in front of one the test environment provides.

use std::{
    collections::{HashMap, VecDeque},
    net::SocketAddr,
    sync::{
        atomic::{AtomicU64, AtomicU8, Ordering},
        Arc, Mutex,
    },
};

use anyhow::Result;
use tokio::{
    io::{AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{
        tcp::{OwnedReadHalf, OwnedWriteHalf},
        TcpListener, TcpStream,
    },
    sync::watch,
    task::JoinHandle,
};

mod control;
#[cfg(test)]
mod control_lifecycle_tests;
#[cfg(test)]
mod control_namespace_tests;
#[cfg(test)]
mod control_ready_tests;
#[cfg(test)]
mod control_tests;
#[cfg(test)]
use control::io::test_support as control_test_support;
mod diagnostic;
pub use control::{
    ClientLoss, ControlBoundary, ControlError, ControlGate, ControlHit, ControlLease,
    ControlSnapshot,
};
pub use diagnostic::{
    Delivery as DiagnosticDelivery, Event as DiagnosticEvent, Identity as DiagnosticIdentity,
    JournalLabel, Phase as DiagnosticPhase, Record as DiagnosticRecord,
    Snapshot as DiagnosticSnapshot, SqlCategory,
};

const SSL_REQUEST: i32 = 80_877_103;
const GSSENC_REQUEST: i32 = 80_877_104;

/// How the proxy treats traffic from now on, on existing and new connections.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProxyMode {
    /// Relay everything.
    Pass,
    /// Close every connection, and every new one at once.
    Sever,
    /// Keep connections open and swallow everything: nothing arrives anywhere,
    /// the client only ever sees its own timeout.
    Blackhole,
    /// The next commit sent as a simple query, which is how
    /// `Transaction::commit` sends it, reaches the server and is applied; its
    /// acknowledgement is dropped and that connection closes. The mode then
    /// returns to `Pass`. A `COMMIT` through the extended protocol is not
    /// covered: it passes through, acknowledged, and the mode stays armed.
    DropCommitAck,
    /// [`Self::DropCommitAck`], for the first commit of a transaction that
    /// executed a statement containing this text. Follows `Parse`, `Bind` and
    /// `Execute` on each connection, so a statement SQLx cached and sends with
    /// no `Parse` still counts, and a commit of any other work passes.
    DropCommitAckOf(&'static str),
    /// Apply the next simple-query rollback but close before its answer.
    DropRollbackAck,
    /// As above, only after a transaction executed matching SQL text.
    DropRollbackAckOf(&'static str),
    /// Drop completion of one matching single-statement autocommit, after the
    /// server reports CommandComplete and idle ReadyForQuery. Parse/Bind alone
    /// do not fire it; explicit transactions and multi-statement simple
    /// queries are excluded. Cached extended executions remain attributable.
    DropAutocommitAckOf(&'static str),
    /// Dev-only: drop the answer to one matching simple-query `DO` sent after
    /// an explicit `BEGIN` on the same connection, then close that relay. A
    /// query binds as a candidate when sent; the mode is claimed only when its
    /// own answer is exactly `CommandComplete` `DO` then `ReadyForQuery` `T`.
    /// Errors, other statuses or messages, nonmatching, autocommit, extended
    /// and earlier queued answers are forwarded untouched, mode still armed.
    DropSimpleInTransactionAckOf(&'static str),
}

/// Connection and commit counts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProxyStats {
    pub connections: u64,
    /// `COMMIT`s sent as a simple query.
    pub commits: u64,
    pub commit_acks_dropped: u64,
    /// Statements executed: simple queries plus extended-protocol `Execute`s.
    pub statements: u64,
}

#[derive(Default)]
struct Counters {
    connections: AtomicU64,
    closed_relays: AtomicU64,
    commits: AtomicU64,
    commit_acks_dropped: AtomicU64,
    statements: AtomicU64,
    rollback_acks_dropped: AtomicU64,
    prepared_statement_parses: AtomicU64,
    autocommit_acks_dropped: AtomicU64,
    simple_in_transaction_acks_dropped: AtomicU64,
}

/// Optional test SQL only: never startup/authentication or bound parameter
/// data. Both retained statements and each statement's text have explicit
/// bounds.
struct QueryTrace {
    capacity: usize,
    statements: VecDeque<String>,
    dropped: u64,
    /// The exact mode preserves simple Query text, including the terminal
    /// delimiter, up to `TRACE_SQL_CHARS`. `E` capture is unchanged.
    exact: bool,
}

const TRACE_CAP: usize = 1024;
const TRACE_SQL_CHARS: usize = 16_384;

struct Shared {
    upstream: SocketAddr,
    mode: watch::Sender<ProxyMode>,
    counters: Counters,
    trace: Mutex<Option<QueryTrace>>,
    diagnostics: Mutex<Option<diagnostic::Trace>>,
    control: Mutex<Option<Arc<control::State>>>,
    #[cfg(test)]
    close_observation: Mutex<Option<control_test_support::CloseObservation>>,
}

impl Shared {
    fn blackholed(&self) -> bool {
        *self.mode.borrow() == ProxyMode::Blackhole
    }

    /// Claim a pending commit-ack drop, so only one connection fires it.
    fn claim_end_ack_drop(&self, transaction: &TransactionWatch, rollback: bool) -> bool {
        self.mode.send_if_modified(|mode| {
            let fires = match *mode {
                ProxyMode::DropCommitAck if !rollback => true,
                ProxyMode::DropCommitAckOf(text) if !rollback => transaction.executed(text),
                ProxyMode::DropRollbackAck if rollback => true,
                ProxyMode::DropRollbackAckOf(text) if rollback => transaction.executed(text),
                _ => false,
            };
            if fires {
                *mode = ProxyMode::Pass;
            }
            fires
        })
    }

    fn claim_autocommit_ack_drop(
        &self,
        transaction: &TransactionWatch,
        tag: u8,
        body: &[u8],
    ) -> bool {
        if transaction.in_transaction || !matches!(tag, b'Q' | b'E') {
            return false;
        }
        let Some(sql) = transaction.statement_text(tag, body) else {
            return false;
        };
        if tag == b'Q' && sql.contains(';') {
            return false;
        }
        self.mode.send_if_modified(|mode| {
            let fires = matches!(*mode, ProxyMode::DropAutocommitAckOf(text) if sql.contains(text));
            if fires {
                *mode = ProxyMode::Pass;
            }
            fires
        })
    }

    /// A candidate only; nothing is claimed until the server answers it.
    fn simple_in_transaction_candidate(
        &self,
        transaction: &TransactionWatch,
        tag: u8,
        body: &[u8],
    ) -> Option<&'static str> {
        if tag != b'Q' || !transaction.in_transaction {
            return None;
        }
        let ProxyMode::DropSimpleInTransactionAckOf(needle) = *self.mode.borrow() else {
            return None;
        };
        let sql = simple_query_text(body);
        let is_do = sql.len() > 2
            && sql.as_bytes()[..2].eq_ignore_ascii_case(b"DO")
            && sql.as_bytes()[2].is_ascii_whitespace();
        (is_do && sql.contains(needle)).then_some(needle)
    }

    /// Claim on positive server evidence, so only one connection fires it.
    fn claim_simple_in_transaction_ack_drop(&self, needle: &'static str) -> bool {
        self.mode.send_if_modified(|mode| {
            let fires = *mode == ProxyMode::DropSimpleInTransactionAckOf(needle);
            if fires {
                *mode = ProxyMode::Pass;
            }
            fires
        })
    }

    fn trace_query(&self, sql: impl FnOnce(bool) -> Option<String>) {
        let mut trace = self.trace.lock().unwrap();
        let Some(trace) = trace.as_mut() else { return };
        let Some(sql) = sql(trace.exact) else { return };
        if trace.statements.len() == trace.capacity {
            trace.statements.pop_front();
            trace.dropped += 1;
        }
        trace.statements.push_back(sql.chars().take(TRACE_SQL_CHARS).collect());
    }
}

/// What one connection executed in its current explicit transaction. A
/// statement is known by name once parsed and is run through a portal, so
/// execution is only visible by following both. Outside `BEGIN`…`COMMIT` a
/// statement commits on its own and belongs to no later transaction.
#[derive(Default)]
struct TransactionWatch {
    statements: HashMap<Vec<u8>, String>,
    portal_statement: HashMap<Vec<u8>, String>,
    in_transaction: bool,
    executed: Vec<String>,
}

impl TransactionWatch {
    fn diagnostic_metadata(&self, tag: u8, body: &[u8]) -> (SqlCategory, bool) {
        if !matches!(tag, b'P' | b'B' | b'E' | b'Q') {
            return (SqlCategory::Other, false);
        }
        let mut fields = body.splitn(3, |byte| *byte == 0);
        let first = fields.next().unwrap_or_default();
        let second = fields.next().unwrap_or_default();
        let text = match tag {
            b'P' => std::str::from_utf8(second).ok(),
            b'B' => self.statements.get(second).map(String::as_str),
            b'E' => self.portal_statement.get(first).map(String::as_str),
            b'Q' => std::str::from_utf8(first).ok(),
            _ => None,
        };
        text.map_or((SqlCategory::Other, true), |sql| {
            // Diagnostic suppression only: this is not a general proof of SQL
            // effects. Literals/comments can conservatively cause Unknown.
            let namespace_opaque = sql.len() > 16_384
                || ["PREPARE", "DEALLOCATE", "DISCARD", "DECLARE"].iter().any(|word| {
                    sql.as_bytes()
                        .windows(word.len())
                        .any(|part| part.eq_ignore_ascii_case(word.as_bytes()))
                });
            if namespace_opaque {
                (SqlCategory::Other, true)
            } else {
                (classify_diagnostic_sql(sql), false)
            }
        })
    }

    /// Note one frontend message.
    fn observe(&mut self, tag: u8, body: &[u8]) {
        let mut fields = body.splitn(3, |byte| *byte == 0);
        match tag {
            b'P' => {
                if let (Some(name), Some(text)) = (fields.next(), fields.next()) {
                    self.statements
                        .insert(name.to_vec(), String::from_utf8_lossy(text).into_owned());
                }
            },
            b'B' => {
                if let (Some(portal), Some(statement)) = (fields.next(), fields.next()) {
                    let text = self.statements.get(statement).cloned().unwrap_or_default();
                    self.portal_statement.insert(portal.to_vec(), text);
                }
            },
            b'E' => {
                let text = fields.next().and_then(|p| self.portal_statement.get(p)).cloned();
                if let Some(text) = text {
                    self.executed_statement(text);
                }
            },
            b'Q' => self.executed_statement(simple_query_text(body)),
            _ => {},
        }
    }

    /// One statement ran. Transaction control moves the boundary; anything
    /// else counts only inside a transaction.
    fn executed_statement(&mut self, text: String) {
        let control = text.trim().trim_end_matches(';').trim().to_ascii_uppercase();
        if control == "BEGIN" || control.starts_with("BEGIN ") {
            self.in_transaction = true;
            self.executed.clear();
        } else if control == "COMMIT" || control == "ROLLBACK" {
            self.in_transaction = false;
            self.executed.clear();
        } else if self.in_transaction {
            self.executed.push(text);
        }
    }

    fn statement_text(&self, tag: u8, body: &[u8]) -> Option<String> {
        match tag {
            b'Q' => Some(simple_query_text(body)),
            b'E' => body
                .split(|byte| *byte == 0)
                .next()
                .and_then(|portal| self.portal_statement.get(portal))
                .cloned(),
            _ => None,
        }
    }

    fn executed(&self, text: &str) -> bool {
        self.executed.iter().any(|statement| statement.contains(text))
    }
}

/// Binds [`ProxyMode::DropSimpleInTransactionAckOf`] candidates to their own
/// answer on one connection. Every forwarded simple `Query`, `Sync` and
/// `FunctionCall` ends in exactly one `ReadyForQuery`, so answers are counted
/// in order and a reply queued for an earlier request is never attributed to
/// a later candidate. The startup `ReadyForQuery` answers no request.
#[derive(Default)]
struct SimpleAckWatch {
    requested: AtomicU64,
    candidate: Mutex<Option<(u64, &'static str)>>,
}

impl SimpleAckWatch {
    /// Before forwarding one frontend message.
    fn request(&self, tag: u8, candidate: Option<&'static str>) {
        if !matches!(tag, b'Q' | b'S' | b'F') {
            return;
        }
        let ordinal = self.requested.fetch_add(1, Ordering::SeqCst) + 1;
        if let Some(needle) = candidate {
            // Retain only the first eligible outstanding slot per relay.
            self.candidate.lock().unwrap().get_or_insert((ordinal, needle));
        }
    }

    /// The candidate one backend message answers, if any. `answered` counts
    /// the requests whose `ReadyForQuery` has arrived.
    fn answering(&self, answered: &mut u64, tag: u8) -> Option<&'static str> {
        let current = *answered + 1;
        let mut candidate = self.candidate.lock().unwrap();
        let needle = candidate
            .as_ref()
            .filter(|(ordinal, _)| *ordinal == current)
            .map(|(_, needle)| *needle);
        if tag == b'Z' && *answered < self.requested.load(Ordering::SeqCst) {
            *answered = current;
            if needle.is_some() {
                *candidate = None;
            }
        }
        needle
    }
}

/// A running proxy. Dropping it stops listening and closes its connections.
pub struct DbProxy {
    addr: SocketAddr,
    shared: Arc<Shared>,
    accept: JoinHandle<()>,
}

impl Drop for DbProxy {
    fn drop(&mut self) {
        #[cfg(test)]
        self.shared.close_observation.lock().unwrap().take();
        self.accept.abort();
        self.shared.mode.send_replace(ProxyMode::Sever);
    }
}

impl DbProxy {
    /// Listen on a loopback port and relay to `upstream`, in `Pass` mode.
    pub async fn start(upstream: SocketAddr) -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let shared = Arc::new(Shared {
            upstream,
            mode: watch::Sender::new(ProxyMode::Pass),
            counters: Counters::default(),
            trace: Mutex::default(),
            diagnostics: Mutex::default(),
            control: Mutex::default(),
            #[cfg(test)]
            close_observation: Mutex::default(),
        });
        let accept = tokio::spawn(accept_loop(listener, shared.clone()));
        Ok(Self { addr, shared, accept })
    }

    /// The address clients connect to.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Bind one six-second control to an actual, known diagnostic identity.
    /// Proof remains provisional until the relay confirms its local ACK state.
    pub fn control(
        &self,
        identity: DiagnosticIdentity,
        orphan: bool,
    ) -> Result<ControlLease, ControlError> {
        ControlLease::new(&self.shared, identity, orphan)
    }

    /// Enable finite protocol metadata before connecting any test client.
    /// No SQL text, binds, authentication, error messages or cancellation key
    /// enters this stream. Existing connections cannot acquire a missing PID.
    pub fn enable_diagnostics(&self) {
        *self.shared.diagnostics.lock().unwrap() = Some(diagnostic::Trace::new());
    }

    /// A bounded snapshot; this never locks a connection or performs I/O.
    pub fn diagnostics(&self) -> Option<DiagnosticSnapshot> {
        self.shared
            .diagnostics
            .lock()
            .unwrap()
            .as_ref()
            .map(diagnostic::Trace::snapshot)
    }

    /// Live observation copies at most 32 identities, never the trace ring.
    pub fn diagnostic_identities(&self) -> Option<Vec<DiagnosticIdentity>> {
        self.shared
            .diagnostics
            .lock()
            .unwrap()
            .as_ref()
            .map(diagnostic::Trace::identities)
    }

    /// Successful matching autocommits whose completion was swallowed.
    pub fn autocommit_acks_dropped(&self) -> u64 {
        self.shared.counters.autocommit_acks_dropped.load(Ordering::Relaxed)
    }

    /// Successful matching in-transaction `DO` answers that were swallowed.
    pub fn simple_in_transaction_acks_dropped(&self) -> u64 {
        self.shared.counters.simple_in_transaction_acks_dropped.load(Ordering::Relaxed)
    }

    /// Change the failure mode.
    pub fn set_mode(&self, mode: ProxyMode) {
        if mode != ProxyMode::Pass {
            if let Some(state) = self.shared.control.lock().unwrap().as_ref() {
                state.invalid();
            }
        }
        self.shared.mode.send_replace(mode);
    }

    /// Current mode; `DropCommitAck` reverts to `Pass` once it has fired.
    pub fn mode(&self) -> ProxyMode {
        *self.shared.mode.borrow()
    }

    /// Enable bounded SQL tracing for an individual test. Off by default;
    /// records SQL text only, excluding startup/authentication packets and
    /// bind parameters. Callers must use nonsecret SQL fixtures.
    pub fn enable_query_trace(&self, capacity: usize) {
        self.reset_query_trace(capacity, false);
    }

    /// As [`Self::enable_query_trace`], but a simple query is recorded as its
    /// exact UTF-8 text up to the single trailing NUL, whitespace and terminal
    /// `;` included. Malformed bodies (missing/interior NUL, invalid UTF-8)
    /// are not recorded. Extended `Execute` capture is unchanged. Each entry
    /// keeps the preexisting `TRACE_SQL_CHARS` per-entry character cap. This
    /// only affects recorded evidence windows, never forwarded bytes or modes.
    pub fn enable_exact_query_trace(&self, capacity: usize) {
        self.reset_query_trace(capacity, true);
    }

    fn reset_query_trace(&self, capacity: usize, exact: bool) {
        *self.shared.trace.lock().unwrap() = Some(QueryTrace {
            capacity: capacity.clamp(1, TRACE_CAP),
            statements: VecDeque::new(),
            dropped: 0,
            exact,
        });
    }

    pub fn disable_query_trace(&self) {
        *self.shared.trace.lock().unwrap() = None;
    }

    pub fn query_trace(&self) -> Vec<String> {
        self.shared
            .trace
            .lock()
            .unwrap()
            .as_ref()
            .map_or_else(Vec::new, |trace| trace.statements.iter().cloned().collect())
    }

    pub fn query_trace_dropped(&self) -> u64 {
        self.shared.trace.lock().unwrap().as_ref().map_or(0, |trace| trace.dropped)
    }

    pub fn rollback_acks_dropped(&self) -> u64 {
        self.shared.counters.rollback_acks_dropped.load(Ordering::Relaxed)
    }

    /// Complete frontend Parse messages observed before forwarding, including
    /// preparations rejected by PostgreSQL. Not successful preparations or
    /// execution/operation attempts: cached execution can send no Parse.
    pub fn prepared_statement_parses(&self) -> u64 {
        self.shared.counters.prepared_statement_parses.load(Ordering::Relaxed)
    }

    /// Counts so far.
    pub fn stats(&self) -> ProxyStats {
        let counters = &self.shared.counters;
        ProxyStats {
            connections: counters.connections.load(Ordering::Relaxed),
            commits: counters.commits.load(Ordering::Relaxed),
            commit_acks_dropped: counters.commit_acks_dropped.load(Ordering::Relaxed),
            statements: counters.statements.load(Ordering::Relaxed),
        }
    }

    /// Relay scopes that exited after dropping their forwarding future and
    /// sockets. This is a passive lifetime observation, not SQL durability or
    /// protocol authority. Callers must also resolve native/session state.
    pub fn closed_relays(&self) -> u64 {
        self.shared.counters.closed_relays.load(Ordering::Acquire)
    }
}

async fn accept_loop(listener: TcpListener, shared: Arc<Shared>) {
    while let Ok((client, _)) = listener.accept().await {
        tokio::spawn(relay_connection(client, shared.clone()));
    }
}

/// Relay one client connection until it ends or the mode severs it. Dropping
/// the relay closes its sockets, so both peers see a closed connection.
async fn relay_connection(client: TcpStream, shared: Arc<Shared>) {
    let connection_id = shared.counters.connections.fetch_add(1, Ordering::Relaxed) + 1;
    let mut mode = shared.mode.subscribe();
    let _closed = ControlClosed(&shared, connection_id);
    let relay = relay(client, &shared, connection_id);
    tokio::pin!(relay);
    loop {
        let state = control::active(&shared, connection_id);
        if *mode.borrow() == ProxyMode::Sever
            || state.as_ref().is_some_and(|s| s.cancelled.load(Ordering::Acquire))
        {
            break;
        }
        let expires = async {
            match state.as_ref() {
                Some(state) => tokio::time::sleep_until(state.deadline).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            result = &mut relay => { let _ = result; break; },
            changed = mode.changed() => { if changed.is_err() { break; } },
            () = expires => { if let Some(state) = state.as_ref() { state.invalid(); } break; },
        }
    }
}

struct ControlClosed<'a>(&'a Shared, u64);
impl Drop for ControlClosed<'_> {
    fn drop(&mut self) {
        let state = {
            let mut slot = self.0.control.lock().unwrap();
            if slot.as_ref().is_some_and(|s| s.identity.connection == self.1) {
                slot.take()
            } else {
                None
            }
        };
        if let Some(state) = state {
            state.closed();
        }
        // The relay future is declared after this guard, so its I/O futures
        // and socket halves have already dropped before this release witness.
        self.0.counters.closed_relays.fetch_add(1, Ordering::Release);
    }
}

async fn relay(mut client: TcpStream, shared: &Shared, id: u64) -> std::io::Result<()> {
    let startup = read_startup_packet(&mut client).await?;
    let diagnostic =
        diagnostic::Connection::new(&shared.diagnostics, id, diagnostic::journal_label(&startup));
    // A blackhole also swallows a new connection: it never reaches the server.
    while shared.blackholed() {
        let mut swallowed = [0; 512];
        if client.read(&mut swallowed).await? == 0 {
            return Ok(());
        }
    }
    let mut server = TcpStream::connect(shared.upstream).await?;
    server.write_all(&startup).await?;

    // Messages cross one at a time; Nagle would add a delay to each.
    client.set_nodelay(true)?;
    server.set_nodelay(true)?;

    let (mut client_read, mut client_write) = client.into_split();
    let (mut server_read, mut server_write) = server.into_split();
    #[cfg(test)]
    let _socket_drop_probe = control_test_support::SocketDropProbe(shared, id);
    let commit_ack_pending = AtomicU8::new(0);
    let simple_ack = SimpleAckWatch::default();
    let frontend = frontend_to_backend(
        &mut client_read,
        &mut server_write,
        shared,
        &commit_ack_pending,
        &simple_ack,
        diagnostic.as_ref(),
        id,
    );
    let backend = backend_to_frontend(
        &mut server_read,
        &mut client_write,
        shared,
        &commit_ack_pending,
        &simple_ack,
        diagnostic.as_ref(),
        id,
    );
    tokio::pin!(frontend, backend);
    tokio::select! {
        result = &mut frontend => {
            if control::active(shared, id).is_some_and(|s| s.orphaned()) { backend.await } else { result }
        },
        result = &mut backend => result,
    }
}

/// The startup packet, after refusing any TLS negotiation that precedes it.
async fn read_startup_packet(client: &mut TcpStream) -> std::io::Result<Vec<u8>> {
    loop {
        let length = client.read_i32().await?;
        let code = client.read_i32().await?;
        let mut rest = vec![0; (length - 8).max(0) as usize];
        client.read_exact(&mut rest).await?;
        if matches!(code, SSL_REQUEST | GSSENC_REQUEST) {
            client.write_all(b"N").await?;
            continue;
        }
        let mut packet = length.to_be_bytes().to_vec();
        packet.extend_from_slice(&code.to_be_bytes());
        packet.extend_from_slice(&rest);
        return Ok(packet);
    }
}

async fn frontend_to_backend(
    client: &mut OwnedReadHalf,
    server: &mut OwnedWriteHalf,
    shared: &Shared,
    commit_ack_pending: &AtomicU8,
    simple_ack: &SimpleAckWatch,
    diagnostic: Option<&diagnostic::Connection<'_>>,
    id: u64,
) -> std::io::Result<()> {
    let mut transaction = TransactionWatch::default();
    let frames = control::io::Frames {
        shared,
        pending: commit_ack_pending,
        diagnostic,
        id,
    };
    loop {
        let (tag, body) = frames.read(client, true).await?;
        if shared.blackholed() {
            continue;
        }
        if tag == b'P' {
            shared.counters.prepared_statement_parses.fetch_add(1, Ordering::Relaxed);
        }
        if is_simple_query_commit(tag, &body) {
            shared.counters.commits.fetch_add(1, Ordering::Relaxed);
            if shared.claim_end_ack_drop(&transaction, false) {
                // Set before the commit is forwarded, so the backend half has
                // always seen it by the time the reply arrives.
                commit_ack_pending.store(1, Ordering::SeqCst);
            }
        }
        if tag == b'Q'
            && simple_query_text(&body).eq_ignore_ascii_case("ROLLBACK")
            && shared.claim_end_ack_drop(&transaction, true)
        {
            commit_ack_pending.store(2, Ordering::SeqCst);
        }
        if shared.claim_autocommit_ack_drop(&transaction, tag, &body) {
            commit_ack_pending.store(3, Ordering::SeqCst);
        }
        if matches!(tag, b'Q' | b'E') {
            shared.counters.statements.fetch_add(1, Ordering::Relaxed);
            shared.trace_query(|exact| match tag {
                b'Q' if exact => exact_simple_query_text(&body),
                _ => transaction.statement_text(tag, &body),
            });
        }
        let record = diagnostic.and_then(|diagnostic| {
            let (category, namespace_opaque) = transaction.diagnostic_metadata(tag, &body);
            let supported = !namespace_opaque
                && (tag != b'Q'
                    || !body.contains(&b';')
                    || control::active(shared, id).is_some_and(|s| {
                        s.association_supported(
                            shared,
                            commit_ack_pending.load(Ordering::SeqCst),
                            &body,
                        )
                    }));
            diagnostic.frontend(tag, category, supported)
        });
        // Bound before forwarding, so the backend half always knows it first.
        let candidate = shared.simple_in_transaction_candidate(&transaction, tag, &body);
        simple_ack.request(tag, candidate);
        transaction.observe(tag, &body);
        frames.forward(server, tag, &body, record, true).await?;
    }
}

async fn backend_to_frontend(
    server: &mut OwnedReadHalf,
    client: &mut OwnedWriteHalf,
    shared: &Shared,
    commit_ack_pending: &AtomicU8,
    simple_ack: &SimpleAckWatch,
    diagnostic: Option<&diagnostic::Connection<'_>>,
    id: u64,
) -> std::io::Result<()> {
    let mut autocommit_complete: Option<Vec<u8>> = None;
    let mut held_record = None;
    let mut startup = true;
    let mut answered = 0;
    let mut held_do: Option<(Vec<u8>, Option<DiagnosticRecord>)> = None;
    let mut do_spent = false;
    let frames = control::io::Frames {
        shared,
        pending: commit_ack_pending,
        diagnostic,
        id,
    };
    loop {
        let (tag, body) = frames.read(server, false).await?;
        let record = diagnostic.and_then(|trace| trace.backend(tag, &body));
        let mark = |record, delivery| {
            if let (Some(trace), Some(record)) = (diagnostic, record) {
                trace.delivery(record, delivery);
            }
        };
        let pending = commit_ack_pending.load(Ordering::SeqCst);
        let candidate = if startup {
            startup = tag != b'Z';
            None
        } else {
            simple_ack.answering(&mut answered, tag)
        };
        if let Some(state) = control::active(shared, id) {
            state.check(shared, pending)?;
        }
        if pending == 3 {
            // CommandComplete can precede a deferred-constraint ErrorResponse
            // at implicit commit. Preserve that failed exchange byte-for-byte;
            // only an error-free idle completion proves the write committed.
            if tag == b'E' {
                if let Some(complete) = autocommit_complete.take() {
                    write_message(client, b'C', &complete).await?;
                    mark(held_record.take(), DiagnosticDelivery::Forwarded);
                }
                commit_ack_pending.store(0, Ordering::SeqCst);
                write_message(client, tag, &body).await?;
                mark(record, DiagnosticDelivery::Forwarded);
                continue;
            }
            if tag == b'C' {
                autocommit_complete = Some(body);
                held_record = record;
                mark(record, DiagnosticDelivery::Held);
                continue;
            }
            if tag == b'Z' {
                if body.first() == Some(&b'I') && autocommit_complete.is_some() {
                    shared.counters.autocommit_acks_dropped.fetch_add(1, Ordering::Relaxed);
                    mark(held_record.take(), DiagnosticDelivery::Dropped);
                    mark(record, DiagnosticDelivery::Dropped);
                    return Ok(());
                }
                if let Some(complete) = autocommit_complete.take() {
                    write_message(client, b'C', &complete).await?;
                    mark(held_record.take(), DiagnosticDelivery::Forwarded);
                }
                commit_ack_pending.store(0, Ordering::SeqCst);
            }
            write_message(client, tag, &body).await?;
            mark(record, DiagnosticDelivery::Forwarded);
            continue;
        }
        if pending != 0 {
            mark(record, DiagnosticDelivery::Dropped);
            // The commit's reply, up to ReadyForQuery: the server has applied
            // it. Dropped, then the connection closes without an answer.
            if tag == b'Z' {
                if pending == 1 {
                    shared.counters.commit_acks_dropped.fetch_add(1, Ordering::Relaxed);
                } else if pending == 2 {
                    shared.counters.rollback_acks_dropped.fetch_add(1, Ordering::Relaxed);
                }
                return Ok(());
            }
            continue;
        }
        if let Some(needle) = candidate {
            match tag {
                b'C' if held_do.is_none() && !do_spent && body == b"DO\0" => {
                    mark(record, DiagnosticDelivery::Held);
                    held_do = Some((body, record));
                    continue;
                },
                // Positive evidence: the DO completed inside the transaction.
                b'Z' if body == b"T"
                    && held_do.is_some()
                    && shared.claim_simple_in_transaction_ack_drop(needle) =>
                {
                    shared
                        .counters
                        .simple_in_transaction_acks_dropped
                        .fetch_add(1, Ordering::Relaxed);
                    mark(held_do.take().and_then(|(_, held)| held), DiagnosticDelivery::Dropped);
                    mark(record, DiagnosticDelivery::Dropped);
                    return Ok(());
                },
                _ => {
                    // Notices may precede the completion; anything else, or
                    // anything after it, means this is not one successful DO.
                    do_spent |= tag != b'N' || held_do.is_some();
                    if let Some((complete, held)) = held_do.take() {
                        if shared.blackholed() {
                            mark(held, DiagnosticDelivery::Dropped);
                        } else {
                            frames.forward(client, b'C', &complete, held, false).await?;
                        }
                    }
                },
            }
            if tag == b'Z' {
                do_spent = false;
            }
        }
        if shared.blackholed() {
            mark(record, DiagnosticDelivery::Dropped);
            continue;
        }
        frames.forward(client, tag, &body, record, false).await?;
    }
}

async fn forward_frontend<W: AsyncWrite + Unpin>(
    stream: &mut W,
    tag: u8,
    body: &[u8],
    diagnostic: Option<&diagnostic::Connection<'_>>,
    record: Option<DiagnosticRecord>,
) -> std::io::Result<()> {
    write_message(stream, tag, body).await?;
    if let (Some(trace), Some(record)) = (diagnostic, record) {
        trace.delivery(record, DiagnosticDelivery::Forwarded);
    }
    Ok(())
}

async fn write_message<W: AsyncWrite + Unpin>(
    stream: &mut W,
    tag: u8,
    body: &[u8],
) -> std::io::Result<()> {
    let mut message = Vec::with_capacity(body.len() + 5);
    message.push(tag);
    message.extend_from_slice(&(body.len() as i32 + 4).to_be_bytes());
    message.extend_from_slice(body);
    stream.write_all(&message).await
}

/// A simple query (`Q`) that is a `COMMIT`. In the extended protocol `Parse`
/// only prepares a statement and execution comes later, so this does not look
/// there; see [`ProxyMode::DropCommitAck`].
fn is_simple_query_commit(tag: u8, body: &[u8]) -> bool {
    if tag != b'Q' {
        return false;
    }
    simple_query_text(body).eq_ignore_ascii_case("COMMIT")
}

fn simple_query_text(body: &[u8]) -> String {
    let end = body.iter().position(|byte| *byte == 0).unwrap_or(body.len());
    let statement = String::from_utf8_lossy(&body[..end]);
    statement.trim().trim_end_matches(';').trim().to_string()
}

/// Trace-only: the query string must end at its one canonical NUL.
fn exact_simple_query_text(body: &[u8]) -> Option<String> {
    let (&0, text) = body.split_last()? else {
        return None;
    };
    if text.contains(&0) {
        return None;
    }
    std::str::from_utf8(text).ok().map(str::to_owned)
}

fn classify_diagnostic_sql(sql: &str) -> SqlCategory {
    if sql.len() > 16_384 {
        return SqlCategory::Other;
    }
    let sql = sql.trim().trim_end_matches(';').trim();
    let starts = |needle: &str| {
        sql.as_bytes()
            .get(..needle.len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(needle.as_bytes()))
    };
    let contains = |needle: &str| {
        sql.as_bytes()
            .windows(needle.len())
            .any(|part| part.eq_ignore_ascii_case(needle.as_bytes()))
    };
    if sql.eq_ignore_ascii_case("BEGIN") || starts("BEGIN ") {
        return SqlCategory::Begin;
    }
    if sql.eq_ignore_ascii_case("COMMIT") {
        return SqlCategory::Commit;
    }
    if sql.eq_ignore_ascii_case("ROLLBACK") {
        return SqlCategory::Rollback;
    }
    if starts("UPDATE accounting_state SET version = version + 1") {
        return SqlCategory::JournalAccounting;
    }
    if starts("UPDATE accounting_state SET version=version+1") {
        return SqlCategory::LedgerAccounting;
    }
    if starts("SELECT") {
        if contains("set_config(") {
            return SqlCategory::Timeouts;
        }
        if contains("balance_leg_admit(") {
            return SqlCategory::LegAdmission;
        }
        if contains("FROM venue_orders") {
            return SqlCategory::OrderRead;
        }
        if contains("FROM order_fills") {
            return SqlCategory::FillRead;
        }
    }
    for (table, category) in [
        ("venue_orders", SqlCategory::OrderWrite),
        ("order_fills", SqlCategory::FillWrite),
        ("balance_legs", SqlCategory::LegWrite),
        ("order_intents", SqlCategory::JournalIntent),
        ("submit_attempts", SqlCategory::JournalAttempt),
        ("spot_intent_clock", SqlCategory::JournalClock),
    ] {
        if (starts("INSERT INTO ") || starts("UPDATE ")) && contains(table) {
            return category;
        }
    }
    SqlCategory::Other
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A named statement is run through a portal, and on later runs SQLx sends
    /// no `Parse`: the transaction still counts as having run it.
    #[test]
    fn a_cached_statement_counts_as_executed() {
        let mut watch = TransactionWatch::default();
        watch.observe(b'Q', b"BEGIN\0");
        watch.observe(b'P', b"s_1\0INSERT INTO order_intents VALUES (1)\0\0\0");
        watch.observe(b'B', b"\0s_1\0\0\0");
        assert!(!watch.executed("order_intents"), "bound is not executed");
        watch.observe(b'E', b"\0\0\0\0\0");
        assert!(watch.executed("order_intents"));

        watch.observe(b'Q', b"COMMIT\0");
        assert!(!watch.executed("order_intents"));
        watch.observe(b'Q', b"BEGIN\0");
        watch.observe(b'B', b"\0s_1\0\0\0");
        watch.observe(b'E', b"\0\0\0\0\0");
        assert!(watch.executed("order_intents"), "no Parse this time");
    }

    /// An autocommit statement commits by itself: it does not count towards
    /// the transaction that happens to come next.
    #[test]
    fn an_autocommit_statement_is_not_in_the_next_transaction() {
        let mut watch = TransactionWatch::default();
        watch.observe(b'P', b"s_1\0SELECT * FROM order_intents\0\0\0");
        watch.observe(b'B', b"\0s_1\0\0\0");
        watch.observe(b'E', b"\0\0\0\0\0");
        assert!(!watch.executed("order_intents"));

        watch.observe(b'Q', b"BEGIN\0");
        watch.observe(b'P', b"s_2\0INSERT INTO other VALUES (1)\0\0\0");
        watch.observe(b'B', b"\0s_2\0\0\0");
        watch.observe(b'E', b"\0\0\0\0\0");
        assert!(!watch.executed("order_intents"));
        assert!(watch.executed("other"));
    }

    #[test]
    fn only_a_simple_query_commit_is_a_commit() {
        assert!(is_simple_query_commit(b'Q', b"COMMIT\0"));
        assert!(is_simple_query_commit(b'Q', b" commit ;\0"));
        assert!(!is_simple_query_commit(b'Q', b"SELECT 'COMMIT'\0"));
        assert!(!is_simple_query_commit(b'Q', b"COMMIT AND CHAIN\0"));
        // Parse prepares; it executes nothing.
        assert!(!is_simple_query_commit(b'P', b"\0COMMIT\0\0\0"));
        assert!(!is_simple_query_commit(b'B', b"COMMIT\0"));
    }

    fn message(tag: u8, body: &[u8]) -> Vec<u8> {
        let mut bytes = vec![tag];
        bytes.extend_from_slice(&(body.len() as i32 + 4).to_be_bytes());
        bytes.extend_from_slice(body);
        bytes
    }

    fn startup_packet() -> Vec<u8> {
        let mut packet = 8i32.to_be_bytes().to_vec();
        packet.extend_from_slice(&196_608i32.to_be_bytes());
        packet
    }

    /// Every exchange of a test runs inside this, so a proxy that stops
    /// forwarding or closing fails the test instead of hanging it.
    async fn bounded<T>(exchange: impl std::future::Future<Output = T>) -> T {
        tokio::time::timeout(std::time::Duration::from_secs(10), exchange)
            .await
            .expect("the proxy self-test hung")
    }

    /// A scripted server: after the startup packet, answers each message with
    /// `CommandComplete` and `ReadyForQuery`, reporting what it received.
    async fn scripted_server() -> (SocketAddr, tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (seen, received) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut startup = [0; 8];
            socket.read_exact(&mut startup).await.unwrap();
            while let Ok(tag) = socket.read_u8().await {
                let length = socket.read_i32().await.unwrap();
                let mut body = vec![0; length as usize - 4];
                socket.read_exact(&mut body).await.unwrap();
                seen.send(body.clone()).unwrap();
                let mut reply = message(b'C', b"COMMIT\0");
                reply.extend(message(b'Z', b"I"));
                let _ = tag;
                socket.write_all(&reply).await.unwrap();
            }
        });
        (addr, received)
    }

    async fn connected(proxy: &DbProxy) -> TcpStream {
        let mut client = TcpStream::connect(proxy.addr()).await.unwrap();
        client.write_all(&startup_packet()).await.unwrap();
        client
    }

    /// One reply of the scripted server: `C` then `Z`. Returns the two tags.
    async fn read_reply(client: &mut TcpStream) -> std::io::Result<(u8, u8)> {
        let mut bytes = [0u8; 18];
        client.read_exact(&mut bytes).await?;
        Ok((bytes[0], bytes[12]))
    }

    #[tokio::test]
    async fn a_dropped_commit_ack_reaches_the_server_and_closes_the_client() {
        bounded(async {
            let (upstream, mut received) = scripted_server().await;
            let proxy = DbProxy::start(upstream).await.unwrap();
            let mut client = connected(&proxy).await;

            client.write_all(&message(b'Q', b"INSERT\0")).await.unwrap();
            assert_eq!(read_reply(&mut client).await.unwrap(), (b'C', b'Z'));

            proxy.set_mode(ProxyMode::DropCommitAck);
            client.write_all(&message(b'Q', b"COMMIT\0")).await.unwrap();

            assert_eq!(received.recv().await.unwrap(), b"INSERT\0".to_vec());
            assert_eq!(received.recv().await.unwrap(), b"COMMIT\0".to_vec());
            assert!(read_reply(&mut client).await.is_err());
            assert_eq!(proxy.stats(), ProxyStats {
                connections: 1,
                commits: 1,
                commit_acks_dropped: 1,
                statements: 2,
            });
            assert_eq!(proxy.mode(), ProxyMode::Pass);
        })
        .await;
    }

    #[tokio::test]
    async fn sever_closes_the_connection() {
        bounded(async {
            let (upstream, _received) = scripted_server().await;
            let proxy = DbProxy::start(upstream).await.unwrap();
            let mut client = connected(&proxy).await;
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;

            proxy.set_mode(ProxyMode::Sever);
            assert!(read_reply(&mut client).await.is_err());
        })
        .await;
    }

    #[tokio::test]
    async fn relay_closure_is_observed_only_after_live_io_finishes() {
        bounded(async {
            for sever in [false, true] {
                let (upstream, _received) = scripted_server().await;
                let proxy = DbProxy::start(upstream).await.unwrap();
                let mut client = connected(&proxy).await;
                client.write_all(&message(b'Q', b"SELECT 1\0")).await.unwrap();
                assert_eq!(read_reply(&mut client).await.unwrap(), (b'C', b'Z'));
                assert_eq!(proxy.stats().connections, 1);
                assert_eq!(proxy.closed_relays(), 0, "live relay reported closed");
                if sever {
                    proxy.set_mode(ProxyMode::Sever);
                    assert!(read_reply(&mut client).await.is_err());
                }
                drop(client);
                while proxy.closed_relays() == 0 {
                    tokio::task::yield_now().await;
                }
                assert_eq!(proxy.closed_relays(), 1);
                assert_eq!(proxy.stats().connections, 1);
            }
        })
        .await;
    }

    #[tokio::test]
    async fn a_blackhole_delivers_nothing_in_either_direction() {
        bounded(async {
            let (upstream, mut received) = scripted_server().await;
            let proxy = DbProxy::start(upstream).await.unwrap();
            let mut client = connected(&proxy).await;
            client.write_all(&message(b'Q', b"SELECT 1\0")).await.unwrap();
            assert_eq!(read_reply(&mut client).await.unwrap(), (b'C', b'Z'));

            proxy.set_mode(ProxyMode::Blackhole);
            client.write_all(&message(b'Q', b"SELECT 2\0")).await.unwrap();
            let reply = tokio::time::timeout(
                std::time::Duration::from_millis(200),
                read_reply(&mut client),
            );
            assert!(reply.await.is_err(), "the client must see only its own timeout");
            assert_eq!(received.recv().await.unwrap(), b"SELECT 1\0".to_vec());
            assert!(received.try_recv().is_err());
        })
        .await;
    }

    #[tokio::test]
    async fn pass_preserves_forwarding_and_trace_is_opt_in_and_bounded() {
        bounded(async {
            let (upstream, mut received) = scripted_server().await;
            let proxy = DbProxy::start(upstream).await.unwrap();
            let mut client = connected(&proxy).await;
            client.write_all(&message(b'Q', b"SELECT 0\0")).await.unwrap();
            read_reply(&mut client).await.unwrap();
            assert!(proxy.query_trace().is_empty());
            proxy.enable_query_trace(2);
            for n in 1..=3 {
                client
                    .write_all(&message(b'Q', format!("SELECT {n}\0").as_bytes()))
                    .await
                    .unwrap();
                assert_eq!(read_reply(&mut client).await.unwrap(), (b'C', b'Z'));
            }
            assert_eq!(proxy.query_trace(), vec!["SELECT 2", "SELECT 3"]);
            assert_eq!(proxy.query_trace_dropped(), 1);
            assert_eq!(proxy.stats().statements, 4);
            assert_eq!(proxy.mode(), ProxyMode::Pass);
            for n in 0..=3 {
                assert_eq!(received.recv().await.unwrap(), format!("SELECT {n}\0").into_bytes());
            }
            proxy.disable_query_trace();
            assert!(proxy.query_trace().is_empty());

            // Exact mode keeps the terminal `;` and skips a malformed body.
            // Default mode still trims, and forwarded bytes never change.
            for (exact, bodies, trace) in [
                (false, vec![b" SELECT 4; \0".as_slice()], vec!["SELECT 4"]),
                (true, vec![b" SELECT 5; \0".as_slice(), b"SELECT 6\0x\0".as_slice()], vec![
                    " SELECT 5; ",
                ]),
            ] {
                if exact {
                    proxy.enable_exact_query_trace(2);
                } else {
                    proxy.enable_query_trace(2);
                }
                for body in bodies {
                    client.write_all(&message(b'Q', body)).await.unwrap();
                    assert_eq!(read_reply(&mut client).await.unwrap(), (b'C', b'Z'));
                    assert_eq!(received.recv().await.unwrap(), body.to_vec());
                }
                assert_eq!(proxy.query_trace(), trace);
                assert_eq!(proxy.query_trace_dropped(), 0);
            }
            assert_eq!(proxy.stats().statements, 7);
        })
        .await;
    }

    #[tokio::test]
    async fn a_targeted_lost_rollback_answer_applies_rollback_then_retires() {
        bounded(async {
            let (upstream, mut received) = scripted_server().await;
            let proxy = DbProxy::start(upstream).await.unwrap();
            let mut client = connected(&proxy).await;
            proxy.set_mode(ProxyMode::DropRollbackAckOf("expected"));
            for sql in [
                "BEGIN",
                "SELECT 1",
                "ROLLBACK",
                "BEGIN",
                "CREATE TEMP TABLE expected (n int)",
            ] {
                client.write_all(&message(b'Q', format!("{sql}\0").as_bytes())).await.unwrap();
                assert_eq!(read_reply(&mut client).await.unwrap(), (b'C', b'Z'));
                assert_eq!(received.recv().await.unwrap(), format!("{sql}\0").into_bytes());
            }
            client.write_all(&message(b'Q', b"ROLLBACK\0")).await.unwrap();
            assert_eq!(received.recv().await.unwrap(), b"ROLLBACK\0".to_vec());
            assert!(read_reply(&mut client).await.is_err());
            assert_eq!(proxy.rollback_acks_dropped(), 1);
            assert_eq!(proxy.stats().commit_acks_dropped, 0);
            assert_eq!(proxy.mode(), ProxyMode::Pass);
        })
        .await;
    }
    #[tokio::test]
    async fn targeted_autocommit_waits_for_execution_and_excludes_transactions() {
        bounded(async {
            let (upstream, mut received) = scripted_server().await;
            let proxy = DbProxy::start(upstream).await.unwrap();
            let mut client = connected(&proxy).await;
            proxy.set_mode(ProxyMode::DropAutocommitAckOf("target_touch"));
            for (tag, body) in [
                (b'Q', b"BEGIN\0".as_slice()),
                (b'Q', b"UPDATE target_touch SET n=1\0".as_slice()),
                (b'Q', b"COMMIT\0".as_slice()),
                (b'P', b"cached\0UPDATE target_touch SET n=2\0\0\0".as_slice()),
                (b'B', b"portal\0cached\0\0\0".as_slice()),
            ] {
                client.write_all(&message(tag, body)).await.unwrap();
                assert_eq!(read_reply(&mut client).await.unwrap(), (b'C', b'Z'));
                assert_eq!(received.recv().await.unwrap(), body);
                assert_eq!(proxy.autocommit_acks_dropped(), 0);
                assert_eq!(proxy.mode(), ProxyMode::DropAutocommitAckOf("target_touch"));
            }
            client.write_all(&message(b'E', b"portal\0\0\0\0\0")).await.unwrap();
            assert_eq!(received.recv().await.unwrap(), b"portal\0\0\0\0\0");
            assert!(read_reply(&mut client).await.is_err());
            assert_eq!(proxy.autocommit_acks_dropped(), 1);
            assert_eq!(proxy.mode(), ProxyMode::Pass);
        })
        .await;
    }

    #[test]
    fn autocommit_attribution_survives_cached_bind_and_ignores_multi_statement_query() {
        let mut watch = TransactionWatch::default();
        watch.observe(b'P', b"cached\0UPDATE target_touch SET n=2\0\0\0");
        watch.observe(b'B', b"portal\0cached\0\0\0");
        watch.observe(b'E', b"portal\0\0\0\0\0");
        watch.observe(b'B', b"other\0cached\0\0\0");
        assert_eq!(
            watch.statement_text(b'E', b"other\0\0\0\0\0"),
            Some("UPDATE target_touch SET n=2".into())
        );
        let shared = Shared {
            upstream: "127.0.0.1:1".parse().unwrap(),
            mode: watch::Sender::new(ProxyMode::DropAutocommitAckOf("target_touch")),
            counters: Counters::default(),
            trace: Mutex::default(),
            diagnostics: Mutex::default(),
            control: Mutex::default(),
            close_observation: Mutex::default(),
        };
        assert!(!shared.claim_autocommit_ack_drop(
            &watch,
            b'Q',
            b"BEGIN; UPDATE target_touch SET n=1\0"
        ));
        assert!(!shared.claim_autocommit_ack_drop(&watch, b'P', b"target_touch\0"));
        assert!(shared.claim_autocommit_ack_drop(&watch, b'E', b"other\0\0\0\0\0"));
        assert_eq!(*shared.mode.borrow(), ProxyMode::Pass);
    }
    #[tokio::test]
    async fn failed_autocommit_is_forwarded_without_claiming_a_landed_ack() {
        assert_failed_autocommit_is_forwarded(false).await;
    }

    #[tokio::test]
    async fn deferred_autocommit_error_after_command_complete_is_forwarded() {
        assert_failed_autocommit_is_forwarded(true).await;
    }

    async fn assert_failed_autocommit_is_forwarded(command_completed: bool) {
        bounded(async {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let error = message(b'E', b"SERROR\0C23514\0Mfixture failure\0\0");
            let mut expected = if command_completed {
                message(b'C', b"UPDATE 1\0")
            } else {
                vec![]
            };
            expected.extend(error);
            expected.extend(message(b'Z', b"I"));
            let reply = expected.clone();
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut startup = [0u8; 8];
                socket.read_exact(&mut startup).await.unwrap();
                assert_eq!(socket.read_u8().await.unwrap(), b'Q');
                let length = socket.read_i32().await.unwrap();
                let mut body = vec![0u8; length as usize - 4];
                socket.read_exact(&mut body).await.unwrap();
                assert_eq!(body, b"UPDATE target_touch SET n=2\0");
                socket.write_all(&reply).await.unwrap();
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            });
            let proxy = DbProxy::start(addr).await.unwrap();
            let mut client = connected(&proxy).await;
            proxy.set_mode(ProxyMode::DropAutocommitAckOf("target_touch"));
            client
                .write_all(&message(b'Q', b"UPDATE target_touch SET n=2\0"))
                .await
                .unwrap();
            let mut received = vec![0u8; expected.len()];
            let result = client.read_exact(&mut received).await;
            assert!(result.is_ok(), "failed autocommit completion was suppressed: {result:?}");
            assert_eq!(received, expected);
            assert_eq!(proxy.autocommit_acks_dropped(), 0);
            server.await.unwrap();
        })
        .await;
    }
}

/// Scripted loopback peers only: these tests never connect to PostgreSQL.
#[cfg(test)]
mod simple_in_transaction_ack_tests {
    use super::{
        control_tests::{accept, bounded, listening, pair, recv, send},
        *,
    };

    const NEEDLE: &str = "spot_transition_fixture";
    const ARMED: ProxyMode = ProxyMode::DropSimpleInTransactionAckOf(NEEDLE);
    const DO: &[u8] = b"DO $fixture$ BEGIN PERFORM 'spot_transition_fixture'; END $fixture$;\0";
    const OTHER_DO: &[u8] = b"DO $fixture$ BEGIN PERFORM 'other_fixture'; END $fixture$;\0";
    const DONE: &[(u8, &[u8])] = &[(b'C', b"DO\0"), (b'Z', b"T")];

    async fn connect(proxy: &DbProxy, listener: &TcpListener) -> (TcpStream, TcpStream) {
        let mut client = TcpStream::connect(proxy.addr()).await.unwrap();
        client.set_nodelay(true).unwrap();
        client.write_all(&[0, 0, 0, 8, 0, 3, 0, 0]).await.unwrap();
        let server = accept(listener).await;
        for tag in [b'R', b'K', b'Z'] {
            assert_eq!(recv(&mut client).await.0, tag);
        }
        (client, server)
    }

    async fn exchange(
        client: &mut TcpStream,
        server: &mut TcpStream,
        request: &[(u8, &[u8])],
        reply: &[(u8, &[u8])],
    ) {
        send(client, request).await;
        for (tag, body) in request {
            assert_eq!(recv(server).await, (*tag, body.to_vec()));
        }
        send(server, reply).await;
        for (tag, body) in reply {
            assert_eq!(recv(client).await, (*tag, body.to_vec()));
        }
    }

    async fn begin(client: &mut TcpStream, server: &mut TcpStream) {
        let reply: &[(u8, &[u8])] = &[(b'C', b"BEGIN\0"), (b'Z', b"T")];
        exchange(client, server, &[(b'Q', b"BEGIN\0")], reply).await;
    }

    fn extended_do() -> Vec<(u8, Vec<u8>)> {
        vec![
            (b'P', [b"\0".as_slice(), DO, &[0, 0]].concat()),
            (b'B', vec![0; 8]),
            (b'E', vec![0; 5]),
            (b'S', vec![]),
        ]
    }

    #[test]
    fn answers_bind_in_request_order_and_ignore_the_startup_ready() {
        let watch = SimpleAckWatch::default();
        let mut answered = 0;
        assert_eq!(watch.answering(&mut answered, b'Z'), None, "startup ReadyForQuery");
        assert_eq!(answered, 0);
        watch.request(b'Q', None);
        watch.request(b'P', None);
        watch.request(b'S', None);
        watch.request(b'Q', Some("x"));
        for tag in [b'C', b'Z', b'1', b'Z'] {
            assert_eq!(watch.answering(&mut answered, tag), None);
        }
        assert_eq!(watch.answering(&mut answered, b'C'), Some("x"));
        assert_eq!(watch.answering(&mut answered, b'Z'), Some("x"));
        assert_eq!(watch.answering(&mut answered, b'Z'), None, "unrequested ReadyForQuery");
        assert_eq!(answered, 3);
    }

    #[tokio::test]
    async fn a_completed_in_transaction_do_loses_only_its_answer_and_closes() {
        bounded(async {
            for notice in [false, true] {
                let (proxy, mut client, mut server) = pair().await;
                proxy.set_mode(ARMED);
                begin(&mut client, &mut server).await;
                send(&mut client, &[(b'Q', DO)]).await;
                assert_eq!(recv(&mut server).await, (b'Q', DO.to_vec()));
                assert_eq!(proxy.mode(), ARMED, "claimed before server evidence");
                if notice {
                    send(&mut server, &[(b'N', b"SNOTICE\0Mfixture\0\0")]).await;
                    assert_eq!(recv(&mut client).await, (b'N', b"SNOTICE\0Mfixture\0\0".to_vec()));
                }
                send(&mut server, DONE).await;
                assert!(client.read_u8().await.is_err(), "the DO answer reached the client");
                assert!(server.read_u8().await.is_err(), "the relay stayed open");
                assert_eq!(proxy.simple_in_transaction_acks_dropped(), 1);
                assert_eq!(proxy.mode(), ProxyMode::Pass);
                assert_eq!(proxy.stats().commit_acks_dropped, 0);
                assert_eq!(proxy.rollback_acks_dropped(), 0);
                assert_eq!(proxy.autocommit_acks_dropped(), 0);
            }
        })
        .await;
    }

    #[tokio::test]
    async fn anything_but_one_completed_matching_in_transaction_do_is_untouched() {
        bounded(async {
            let multi = [&DO[..DO.len() - 1], b" ", DO].concat();
            let extended = extended_do();
            let extended: Vec<_> = extended.iter().map(|(t, b)| (*t, b.as_slice())).collect();
            let error: &[(u8, &[u8])] = &[
                (b'E', b"SERROR\0C23514\0Mfixture failure\0\0"),
                (b'Z', b"E"),
            ];
            let cases: [(bool, &[(u8, &[u8])], &[(u8, &[u8])]); 8] = [
                // ErrorResponse, with and without a completion before it.
                (true, &[(b'Q', DO)], error),
                (true, &[(b'Q', DO)], &[(b'C', b"DO\0"), error[0], error[1]]),
                // Completed, but not left inside the transaction.
                (true, &[(b'Q', DO)], &[(b'C', b"DO\0"), (b'Z', b"I")]),
                // Something after the completion.
                (true, &[(b'Q', DO)], &[(b'C', b"DO\0"), (b'N', b"SNOTICE\0\0"), (b'Z', b"T")]),
                (true, &[(b'Q', OTHER_DO)], DONE),
                // Implicit/autocommit: never a candidate, whatever the status.
                (false, &[(b'Q', DO)], DONE),
                (true, &extended, &[(b'1', b""), (b'2', b""), DONE[0], DONE[1]]),
                (true, &[(b'Q', &multi)], &[DONE[0], DONE[0], DONE[1]]),
            ];
            for (in_transaction, request, reply) in cases {
                let (proxy, mut client, mut server) = pair().await;
                proxy.set_mode(ARMED);
                if in_transaction {
                    begin(&mut client, &mut server).await;
                }
                exchange(&mut client, &mut server, request, reply).await;
                assert_eq!(proxy.simple_in_transaction_acks_dropped(), 0, "{reply:?}");
                assert_eq!(proxy.mode(), ARMED, "{reply:?}");
                // The relay stays open and keeps passing.
                exchange(&mut client, &mut server, &[(b'Q', b"SELECT 1\0")], &[
                    (b'C', b"SELECT 1\0"),
                    (b'Z', b"T"),
                ])
                .await;
            }
        })
        .await;
    }

    #[tokio::test]
    async fn earlier_queued_answers_are_untouched_and_binding_is_per_query() {
        bounded(async {
            // Pipelined: an extended Sync and a nonmatching DO answer first.
            let (proxy, mut client, mut server) = pair().await;
            proxy.set_mode(ARMED);
            begin(&mut client, &mut server).await;
            let extended = extended_do();
            let mut request: Vec<_> = extended.iter().map(|(t, b)| (*t, b.as_slice())).collect();
            request.extend([(b'Q', OTHER_DO), (b'Q', DO)]);
            send(&mut client, &request).await;
            for (tag, body) in &request {
                assert_eq!(recv(&mut server).await, (*tag, body.to_vec()));
            }
            let earlier: &[(u8, &[u8])] =
                &[(b'1', b""), (b'2', b""), DONE[0], DONE[1], DONE[0], DONE[1]];
            send(&mut server, &[earlier, DONE].concat()).await;
            for (tag, body) in earlier {
                assert_eq!(recv(&mut client).await, (*tag, body.to_vec()));
            }
            assert!(client.read_u8().await.is_err());
            assert_eq!(proxy.simple_in_transaction_acks_dropped(), 1);
            assert_eq!(proxy.mode(), ProxyMode::Pass);

            // A matching DO sent before arming answers untouched; the next fires.
            let (proxy, mut client, mut server) = pair().await;
            begin(&mut client, &mut server).await;
            send(&mut client, &[(b'Q', DO)]).await;
            assert_eq!(recv(&mut server).await, (b'Q', DO.to_vec()));
            proxy.set_mode(ARMED);
            send(&mut server, DONE).await;
            for (tag, body) in DONE {
                assert_eq!(recv(&mut client).await, (*tag, body.to_vec()));
            }
            assert_eq!(proxy.mode(), ARMED);
            send(&mut client, &[(b'Q', DO)]).await;
            assert_eq!(recv(&mut server).await, (b'Q', DO.to_vec()));
            send(&mut server, DONE).await;
            assert!(client.read_u8().await.is_err());
            assert_eq!(proxy.simple_in_transaction_acks_dropped(), 1);
            assert_eq!(proxy.mode(), ProxyMode::Pass);
        })
        .await;
    }

    #[tokio::test]
    async fn only_the_first_connection_with_positive_evidence_claims_the_mode() {
        bounded(async {
            let (proxy, listener) = listening().await;
            let mut autocommit = connect(&proxy, &listener).await;
            let mut failing = connect(&proxy, &listener).await;
            let mut winner = connect(&proxy, &listener).await;
            let mut late = connect(&proxy, &listener).await;
            for (client, server) in [&mut failing, &mut winner, &mut late] {
                begin(client, server).await;
            }
            proxy.set_mode(ARMED);
            for (client, server) in [&mut autocommit, &mut failing, &mut winner, &mut late] {
                send(client, &[(b'Q', DO)]).await;
                assert_eq!(recv(server).await, (b'Q', DO.to_vec()));
            }
            let error: &[(u8, &[u8])] = &[
                (b'E', b"SERROR\0C23514\0Mfixture failure\0\0"),
                (b'Z', b"E"),
            ];
            for ((client, server), reply) in [(&mut autocommit, DONE), (&mut failing, error)] {
                send(server, reply).await;
                for (tag, body) in reply {
                    assert_eq!(recv(client).await, (*tag, body.to_vec()));
                }
                assert_eq!(proxy.mode(), ARMED);
            }
            send(&mut winner.1, DONE).await;
            assert!(winner.0.read_u8().await.is_err());
            assert_eq!(proxy.mode(), ProxyMode::Pass);
            // Still a candidate, but the mode was already claimed elsewhere.
            send(&mut late.1, DONE).await;
            for (tag, body) in DONE {
                assert_eq!(recv(&mut late.0).await, (*tag, body.to_vec()));
            }
            assert_eq!(proxy.simple_in_transaction_acks_dropped(), 1);
            for (client, server) in [&mut autocommit, &mut failing, &mut late] {
                exchange(client, server, &[(b'Q', b"SELECT 1\0")], &[
                    (b'C', b"SELECT 1\0"),
                    (b'Z', b"I"),
                ])
                .await;
            }
        })
        .await;
    }
}

#[cfg(test)]
#[path = "db_proxy/forwarding_tests.rs"]
mod forwarding_tests;

#[cfg(test)]
#[path = "db_proxy/namespace_tests.rs"]
mod namespace_tests;
