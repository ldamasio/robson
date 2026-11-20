//! One bounded loopback listener, kept alive through all admission refusals.
//! Connection counts are physical observations, not exchange request authority.
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{anyhow, ensure, Context};
use chrono::Utc;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

use super::*;

pub(super) struct Fixture {
    pub(super) base_url: String,
    connections: Arc<AtomicUsize>,
    reply: Arc<Mutex<Option<i64>>>,
    cancel: CancellationToken,
    task: JoinHandle<anyhow::Result<()>>,
}

impl Fixture {
    pub(super) async fn start(deadline: Instant) -> anyhow::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let base_url = format!("http://{}", listener.local_addr()?);
        let connections = Arc::new(AtomicUsize::new(0));
        let reply = Arc::new(Mutex::new(None));
        let cancel = CancellationToken::new();
        let (count, sent, stopped) = (Arc::clone(&connections), Arc::clone(&reply), cancel.clone());
        let task = tokio::spawn(async move {
            let work = async {
                loop {
                    let (mut socket, _) = tokio::select! {
                        biased;
                        _ = stopped.cancelled() => return Ok(()),
                        next = listener.accept() => next?,
                    };
                    ensure!(
                        count.fetch_add(1, Ordering::SeqCst) == 0,
                        "unexpected second physical TIME connection"
                    );
                    let exchange = async {
                        let mut request = Vec::new();
                        while !request.ends_with(b"\r\n\r\n") {
                            ensure!(request.len() < 16384, "bounded loopback head");
                            request.push(socket.read_u8().await?);
                        }
                        let request = std::str::from_utf8(&request)?;
                        ensure!(request.lines().next() == Some("GET /api/v3/time HTTP/1.1"));
                        ensure!(!request.to_ascii_lowercase().contains("x-mbx-apikey"));
                        ensure!(!request.contains("signature=") && !request.contains("timestamp="));
                        let server_ms = Utc::now().timestamp_millis();
                        *sent.lock().map_err(|_| anyhow!("reply lock poisoned"))? = Some(server_ms);
                        let body =
                            format!("{{\"serverTime\":{server_ms},\"unknown\":{{\"kept\":true}}}}");
                        let response = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                        socket.write_all(response.as_bytes()).await?;
                        socket.shutdown().await?;
                        Ok::<_, anyhow::Error>(())
                    };
                    // This bounds only the test server, not the original
                    // owner's HTTP deadline or first-poll budget.
                    tokio::select! {
                        biased;
                        _ = stopped.cancelled() => return Ok(()),
                        result = timeout_at((Instant::now() + Duration::from_secs(3)).min(deadline), exchange) => {
                            result.context("loopback exchange deadline")??;
                        },
                    }
                }
            };
            timeout_at(deadline, work).await.context("loopback fixture ceiling")?
        });
        Ok(Self {
            base_url,
            connections,
            reply,
            cancel,
            task,
        })
    }

    pub(super) fn connections(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }

    pub(super) fn server_ms(&self) -> anyhow::Result<i64> {
        self.reply
            .lock()
            .map_err(|_| anyhow!("reply lock poisoned"))?
            .context("no actual server reply")
    }

    pub(super) async fn finish(mut self, deadline: Instant) -> anyhow::Result<()> {
        self.cancel.cancel();
        match timeout_at((Instant::now() + Duration::from_secs(2)).min(deadline), &mut self.task)
            .await
        {
            Ok(result) => result.context("HTTP fixture task")?,
            Err(_) => {
                self.task.abort();
                let retired = timeout_at(deadline, &mut self.task).await;
                return Err(anyhow!("HTTP fixture needed abort; join={retired:?}"));
            },
        }?;
        ensure!(self.connections() <= 1, "second physical connection observed");
        Ok(())
    }
}
