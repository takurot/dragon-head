use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;

use hitl_bridge::notifier::{ApprovalNotification, ChatNotifier, SlackNotifier};
use uuid::Uuid;

#[test]
fn local_endpoint_rejects_non_loopback_or_ambiguous_urls() {
    for url in [
        "https://127.0.0.1:8000/api",
        "http://localhost:8000/api",
        "http://example.com:8000/api",
        "http://192.168.1.1:8000/api",
        "http://127.0.0.1/api",
        "http://127.0.0.1:0/api",
        "http://secret@127.0.0.1:8000/api",
        "http://127.0.0.1:8000/api?secret",
        "http://127.0.0.1:8000/api#secret",
        "http://127.0.0.1:8000/other",
        "http://127.0.0.1:8000/api/",
        "http://2130706433:8000/api",
    ] {
        let result = SlackNotifier::with_local_api_base_url("dummy", "C-demo", url);
        assert!(result.is_err(), "must reject {url}");
        assert!(!result.err().unwrap().to_string().contains(url));
    }
    for url in ["http://127.0.0.1:8000/api", "http://[::1]:8000/api"] {
        assert!(SlackNotifier::with_local_api_base_url("dummy", "C-demo", url).is_ok());
    }
}

fn notification() -> ApprovalNotification {
    ApprovalNotification {
        id: Uuid::new_v4(),
        rule_id: "demo".into(),
        action: "click".into(),
        outcome: None,
        som_image_png: None,
    }
}

fn notify_response(status: &str, body: &str, extra_headers: &str) -> anyhow::Result<String> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?;
    listener.set_nonblocking(true)?;
    let response = format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n{extra_headers}\r\n{body}", body.len());
    let server = thread::spawn(move || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let (mut stream, _) = loop {
            match listener.accept() {
                Ok(connection) => break connection,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "local API request timed out"
                    );
                    thread::sleep(std::time::Duration::from_millis(5));
                }
                Err(error) => panic!("local API accept failed: {error}"),
            }
        };
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        stream
            .set_write_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let mut received = Vec::new();
        let mut buffer = [0; 4096];
        loop {
            let n = stream.read(&mut buffer).unwrap();
            assert!(n > 0);
            received.extend_from_slice(&buffer[..n]);
            if let Some(end) = received.windows(4).position(|window| window == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&received[..end]);
                let length = headers
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length: ")
                            .and_then(|value| value.parse::<usize>().ok())
                    })
                    .unwrap();
                if received.len() >= end + 4 + length {
                    break;
                }
            }
        }
        assert!(String::from_utf8_lossy(&received).starts_with("POST /api/chat.postMessage "));
        stream.write_all(response.as_bytes()).unwrap();
    });
    let notifier = SlackNotifier::with_local_api_base_url(
        "dummy",
        "C-demo",
        &format!("http://{address}/api"),
    )?;
    let result = notifier.notify(&notification());
    server.join().unwrap();
    result
}

#[test]
fn http_and_slack_payload_must_both_indicate_success() {
    assert_eq!(
        notify_response("200 OK", r#"{"ok":true,"ts":"123.45"}"#, "").unwrap(),
        "C-demo:123.45"
    );
    for (status, body) in [
        ("302 Found", r#"{"ok":true,"ts":"123"}"#),
        ("400 Bad Request", r#"{"ok":true,"ts":"123"}"#),
        ("500 Internal Server Error", r#"{"ok":true,"ts":"123"}"#),
        ("200 OK", "not-json"),
        ("200 OK", r#"{"ok":"true","ts":"123"}"#),
        ("200 OK", r#"{"ok":false,"ts":"123"}"#),
        ("200 OK", r#"{"ok":true}"#),
        ("200 OK", r#"{"ok":true,"ts":5}"#),
        ("200 OK", r#"{"ok":true,"ts":""}"#),
        ("200 OK", r#"{"ok":true,"ts":"   "}"#),
    ] {
        assert!(
            notify_response(status, body, "").is_err(),
            "must reject {status}: {body}"
        );
    }
}

#[test]
fn local_redirect_is_not_followed_or_reported_successful() {
    let destination = TcpListener::bind("127.0.0.1:0").unwrap();
    destination.set_nonblocking(true).unwrap();
    let location = format!(
        "Location: http://{}/leaked\r\n",
        destination.local_addr().unwrap()
    );
    assert!(notify_response("302 Found", r#"{"ok":true,"ts":"123"}"#, &location).is_err());
    assert_eq!(
        destination.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}
