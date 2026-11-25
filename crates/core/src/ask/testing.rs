//! Stand-ins the tests of `ask` share: passages, and providers that answer
//! one request with a fixed reply.

use std::sync::{Arc, RwLock};

use genai::ModelIden;
use genai::adapter::AdapterKind;
use genai::chat::{ChatMessage, ChatRequest};
use genai::resolver::AuthData;

use super::client;
use crate::cite;
use crate::retrieve::Passage;
use crate::settings::Settings;

pub(super) fn passage(label: &str, source: &str, text: &str) -> Passage {
    Passage {
        label: label.into(),
        source_id: source.into(),
        ord: cite::split_label(label).unwrap().1,
        page_start: Some(1),
        page_end: Some(1),
        section: String::new(),
        text: text.into(),
    }
}

/// A provider that gives `reply`, as a status and body, to one request.
/// Without a reply it closes the connection unanswered.
pub(super) async fn provider(
    kind: AdapterKind,
    reply: Option<(&str, &str)>,
) -> genai::ServiceTarget {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/", listener.local_addr().unwrap());
    let reply = reply.map(|(status, body)| {
        format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    });
    tokio::spawn(async move {
        let (mut conn, _) = listener.accept().await.unwrap();
        let mut request = [0; 8192];
        let _ = conn.read(&mut request).await;
        if let Some(reply) = reply {
            let _ = conn.write_all(reply.as_bytes()).await;
        }
    });
    genai::ServiceTarget {
        endpoint: genai::resolver::Endpoint::from_owned(endpoint),
        auth: AuthData::from_single("key"),
        model: ModelIden::new(kind, "some-model"),
    }
}

/// A provider that takes a request and never answers or closes it.
pub(super) async fn silent_provider() -> genai::ServiceTarget {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let _held = listener.accept().await;
        std::future::pending::<()>().await;
    });
    genai::ServiceTarget {
        endpoint: genai::resolver::Endpoint::from_owned(endpoint),
        auth: AuthData::from_single("key"),
        model: ModelIden::new(AdapterKind::OpenRouter, "some-model"),
    }
}

/// A server that answers one request with a streamed `reply` and hands
/// back the request's head.
pub(super) async fn recording_server(
    reply: &'static str,
) -> (String, tokio::sync::oneshot::Receiver<String>) {
    recording_server_as("text/event-stream", reply).await
}

pub(super) async fn recording_server_as(
    content_type: &'static str,
    reply: &'static str,
) -> (String, tokio::sync::oneshot::Receiver<String>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let (tx, rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let (mut conn, _) = listener.accept().await.unwrap();
        let mut request = vec![0; 16384];
        let n = conn.read(&mut request).await.unwrap();
        let _ = tx.send(String::from_utf8_lossy(&request[..n]).into_owned());
        let reply = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
            reply.len()
        );
        let _ = conn.write_all(reply.as_bytes()).await;
    });
    (address, rx)
}

pub(super) fn client_with_server(address: &str) -> genai::Client {
    let settings = Settings {
        model_server: Some(address.to_string()),
        keys: [("DEEPSEEK_API_KEY".to_string(), "key".to_string())].into(),
        ..Default::default()
    };
    client(Arc::new(RwLock::new(settings))).unwrap()
}

pub(super) fn question() -> ChatRequest {
    ChatRequest::new(vec![ChatMessage::user("hi")])
}
