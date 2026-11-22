use clap::Parser;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

use super::*;
use crate::Cli;

#[test]
fn parses_cash_reanchor_command() {
    let cli = Cli::try_parse_from([
        "robson-cli",
        "cash",
        "reanchor",
        "--reason",
        "withdrew 100 USDT",
        "--robsond-url",
        "http://robsond:8080",
    ])
    .unwrap();
    let Cli::Cash(CashArgs { command: CashCommand::Reanchor(args) }) = cli else {
        panic!("expected the cash command");
    };
    assert_eq!(args.reason, "withdrew 100 USDT");
    assert_eq!(args.robsond_url, "http://robsond:8080");
    assert!(
        Cli::try_parse_from(["robson-cli", "cash", "reanchor"]).is_err(),
        "reason required"
    );
}

/// A blank or oversized reason is refused before any token or request.
#[tokio::test]
async fn a_blank_or_oversized_reason_is_a_usage_error() {
    for reason in ["   ".to_string(), "x".repeat(2001)] {
        let args = CashArgs {
            command: CashCommand::Reanchor(ReanchorArgs {
                reason,
                robsond_url: "http://127.0.0.1:1".into(),
            }),
        };
        assert_eq!(run(args).await, EXIT_USAGE_ERROR);
    }
}

/// The wire: one `POST /venue/cash-reanchor` with the bearer token and the
/// reason as JSON; the daemon's status and JSON body come back as given.
#[tokio::test]
async fn cash_reanchor_posts_the_reason_with_the_token() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut chunk = [0_u8; 4096];
        loop {
            let read = socket.read(&mut chunk).await.unwrap();
            if read == 0 {
                break;
            }
            request.extend_from_slice(&chunk[..read]);
            let text = String::from_utf8_lossy(&request).to_string();
            if let Some(head_end) = text.find("\r\n\r\n") {
                let length = text[..head_end]
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length: ")
                            .map(str::to_string)
                    })
                    .and_then(|value| value.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                if request.len() >= head_end + 4 + length {
                    break;
                }
            }
        }
        let body = r#"{"error":"busy"}"#;
        let answer = format!(
            "HTTP/1.1 409 Conflict\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        socket.write_all(answer.as_bytes()).await.unwrap();
        String::from_utf8(request).unwrap()
    });
    let (status, body) = ApiClient::new(&format!("{url}/"), "token-1")
        .cash_reanchor("withdrew 100")
        .await
        .unwrap();
    assert_eq!((status, body), (409, serde_json::json!({ "error": "busy" })));
    let request = server.await.unwrap();
    let (head, body) = request.split_once("\r\n\r\n").unwrap();
    assert!(head.starts_with("POST /venue/cash-reanchor HTTP/1.1\r\n"), "{head}");
    let head = head.to_ascii_lowercase();
    assert!(head.contains("\r\nauthorization: bearer token-1\r\n"), "{head}");
    assert!(head.contains("\r\ncontent-type: application/json\r\n"), "{head}");
    let sent: serde_json::Value = serde_json::from_str(body).unwrap();
    assert_eq!(sent, serde_json::json!({ "reason": "withdrew 100" }));
}
