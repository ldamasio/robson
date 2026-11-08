//! A TCP proxy in front of PostgreSQL that injects failures (ADR-0056 step 7).
//!
//! It follows the frontend and backend message framing, so it can tell a
//! `COMMIT` from any other statement. A client connects through it without
//! TLS; a TLS request is answered with "not supported". The proxy never starts
//! a database; it only sits in front of one the test environment provides.

use std::{
    net::SocketAddr,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
};

use anyhow::Result;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{
        tcp::{OwnedReadHalf, OwnedWriteHalf},
        TcpListener, TcpStream,
    },
    sync::watch,
    task::JoinHandle,
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
}

/// Connection and commit counts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProxyStats {
    pub connections: u64,
    /// `COMMIT`s sent as a simple query.
    pub commits: u64,
    pub commit_acks_dropped: u64,
}

#[derive(Default)]
struct Counters {
    connections: AtomicU64,
    commits: AtomicU64,
    commit_acks_dropped: AtomicU64,
}

struct Shared {
    upstream: SocketAddr,
    mode: watch::Sender<ProxyMode>,
    counters: Counters,
}

impl Shared {
    fn blackholed(&self) -> bool {
        *self.mode.borrow() == ProxyMode::Blackhole
    }

    /// Claim a pending `DropCommitAck`, so only one connection fires it.
    fn claim_commit_ack_drop(&self) -> bool {
        self.mode.send_if_modified(|mode| {
            let fires = *mode == ProxyMode::DropCommitAck;
            if fires {
                *mode = ProxyMode::Pass;
            }
            fires
        })
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
        });
        let accept = tokio::spawn(accept_loop(listener, shared.clone()));
        Ok(Self { addr, shared, accept })
    }

    /// The address clients connect to.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Change the failure mode.
    pub fn set_mode(&self, mode: ProxyMode) {
        self.shared.mode.send_replace(mode);
    }

    /// Current mode; `DropCommitAck` reverts to `Pass` once it has fired.
    pub fn mode(&self) -> ProxyMode {
        *self.shared.mode.borrow()
    }

    /// Counts so far.
    pub fn stats(&self) -> ProxyStats {
        let counters = &self.shared.counters;
        ProxyStats {
            connections: counters.connections.load(Ordering::Relaxed),
            commits: counters.commits.load(Ordering::Relaxed),
            commit_acks_dropped: counters.commit_acks_dropped.load(Ordering::Relaxed),
        }
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
    shared.counters.connections.fetch_add(1, Ordering::Relaxed);
    let mut mode = shared.mode.subscribe();
    let severed = async move {
        let _ = mode.wait_for(|mode| *mode == ProxyMode::Sever).await;
    };
    tokio::select! {
        result = relay(client, &shared) => { let _ = result; },
        () = severed => {},
    }
}

async fn relay(mut client: TcpStream, shared: &Shared) -> std::io::Result<()> {
    let startup = read_startup_packet(&mut client).await?;
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

    let (client_read, client_write) = client.into_split();
    let (server_read, server_write) = server.into_split();
    let commit_ack_pending = AtomicBool::new(false);
    // Whichever direction ends first ends the connection.
    tokio::select! {
        result = frontend_to_backend(client_read, server_write, shared, &commit_ack_pending) => result,
        result = backend_to_frontend(server_read, client_write, shared, &commit_ack_pending) => result,
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
    mut client: OwnedReadHalf,
    mut server: OwnedWriteHalf,
    shared: &Shared,
    commit_ack_pending: &AtomicBool,
) -> std::io::Result<()> {
    loop {
        let (tag, body) = read_message(&mut client).await?;
        if shared.blackholed() {
            continue;
        }
        if is_simple_query_commit(tag, &body) {
            shared.counters.commits.fetch_add(1, Ordering::Relaxed);
            if shared.claim_commit_ack_drop() {
                // Set before the commit is forwarded, so the backend half has
                // always seen it by the time the reply arrives.
                commit_ack_pending.store(true, Ordering::SeqCst);
            }
        }
        write_message(&mut server, tag, &body).await?;
    }
}

async fn backend_to_frontend(
    mut server: OwnedReadHalf,
    mut client: OwnedWriteHalf,
    shared: &Shared,
    commit_ack_pending: &AtomicBool,
) -> std::io::Result<()> {
    loop {
        let (tag, body) = read_message(&mut server).await?;
        if commit_ack_pending.load(Ordering::SeqCst) {
            // The commit's reply, up to ReadyForQuery: the server has applied
            // it. Dropped, then the connection closes without an answer.
            if tag == b'Z' {
                shared.counters.commit_acks_dropped.fetch_add(1, Ordering::Relaxed);
                return Ok(());
            }
            continue;
        }
        if shared.blackholed() {
            continue;
        }
        write_message(&mut client, tag, &body).await?;
    }
}

async fn read_message(stream: &mut OwnedReadHalf) -> std::io::Result<(u8, Vec<u8>)> {
    let tag = stream.read_u8().await?;
    let length = stream.read_i32().await?;
    let mut body = vec![0; (length - 4).max(0) as usize];
    stream.read_exact(&mut body).await?;
    Ok((tag, body))
}

async fn write_message(stream: &mut OwnedWriteHalf, tag: u8, body: &[u8]) -> std::io::Result<()> {
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
    let end = body.iter().position(|byte| *byte == 0).unwrap_or(body.len());
    let statement = String::from_utf8_lossy(&body[..end]);
    statement.trim().trim_end_matches(';').trim().eq_ignore_ascii_case("COMMIT")
}

#[cfg(test)]
mod tests {
    use super::*;

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
                commit_acks_dropped: 1
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
}
