use std::time::Duration;

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::oneshot,
    time::timeout,
};

use super::*;

async fn headers(stream: &mut TcpStream) -> String {
    let mut bytes = Vec::new();
    while !bytes.ends_with(b"\r\n\r\n") {
        assert!(bytes.len() < 4096, "bounded request head");
        bytes.push(stream.read_u8().await.unwrap());
    }
    String::from_utf8(bytes).unwrap().to_ascii_lowercase()
}

#[tokio::test]
async fn prepared_time_is_unsigned_and_returns_before_body_collection() {
    timeout(Duration::from_secs(2), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let prepared = BinanceRestClient::with_local_base_url(url).prepare_spot_time().unwrap();
        // Preparation itself owns no network future or request attempt.
        assert!(timeout(Duration::from_millis(20), listener.accept()).await.is_err());
        let (proceed, wait) = oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let request = headers(&mut socket).await;
            assert!(request.starts_with("get /api/v3/time http/1.1\r\n"));
            for absent in ["x-mbx-apikey", "signature", "timestamp", "key", "secret"] {
                assert!(!request.contains(absent), "unexpected credential: {absent}");
            }
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n").await.unwrap();
            wait.await.unwrap();
            socket.write_all(b"{}").await.unwrap();
        });
        let response = prepared.send().await.unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        proceed.send(()).unwrap();
        assert_eq!(response.bytes().await.unwrap().as_ref(), b"{}");
        server.await.unwrap();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn prepared_time_keeps_each_redirect_as_the_original_answer() {
    timeout(Duration::from_secs(3), async {
        for status in [301, 302, 303, 307, 308] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let destination = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let target = destination.local_addr().unwrap();
            let prepared = BinanceRestClient::with_local_base_url(format!(
                "http://{}",
                listener.local_addr().unwrap()
            ))
            .prepare_spot_time()
            .unwrap();
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                headers(&mut socket).await;
                socket
                    .write_all(format!("HTTP/1.1 {status} Redirect\r\nLocation: http://{target}/elsewhere\r\nContent-Length: 0\r\n\r\n").as_bytes())
                    .await
                    .unwrap();
            });
            assert_eq!(prepared.send().await.unwrap().status().as_u16(), status);
            server.await.unwrap();
            assert!(timeout(Duration::from_millis(50), destination.accept()).await.is_err());
        }
    })
    .await
    .unwrap();
}
