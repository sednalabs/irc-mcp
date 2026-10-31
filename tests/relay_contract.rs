//! Isolated transport tests. Run with Cargo; these do not touch a live IRC server.
use irc_mcp::{config::Config,relay::Relay};
use serde_json::json;
use std::sync::Arc;
use tokio::{io::{AsyncReadExt,AsyncWriteExt},net::UnixListener};

async fn fixture(response:&str)->(Relay,std::path::PathBuf,tokio::task::JoinHandle<String>) {
    let dir=std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir(&dir).unwrap();let socket=dir.join("relay.sock");
    let listener=UnixListener::bind(&socket).unwrap();
    let mut cfg:Config=toml::from_str(include_str!("../config.example.toml")).unwrap();
    cfg.relay_socket=socket;
    let relay=Relay::new(Arc::new(cfg),"test-only-password".into()).unwrap();
    let response=response.to_owned();
    let handle=tokio::spawn(async move {
        let (mut stream,_)=listener.accept().await.unwrap();let mut bytes=Vec::new();
        loop {
            let mut byte=[0u8;1];stream.read_exact(&mut byte).await.unwrap();bytes.push(byte[0]);
            assert!(bytes.len()<32*1024);
            if bytes.ends_with(b"\r\n\r\n") {break;}
        }
        let headers=String::from_utf8(bytes).unwrap();
        let content_length=headers.lines().find_map(|line|line.to_ascii_lowercase().strip_prefix("content-length:").and_then(|v|v.trim().parse::<usize>().ok())).unwrap_or(0);
        let mut body=vec![0u8;content_length];stream.read_exact(&mut body).await.unwrap();
        stream.write_all(response.as_bytes()).await.unwrap();stream.shutdown().await.unwrap();
        format!("{headers}{}",String::from_utf8(body).unwrap())
    });
    (relay,dir,handle)
}
fn json_response(value:serde_json::Value)->String {
    let body=value.to_string();format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body)
}
#[tokio::test]
async fn valid_version_is_read_over_private_socket() {
    let response=json_response(json!({"weechat_version":"4.10.1","relay_api_version":"0.0.1"}));
    let (relay,dir,task)=fixture(&response).await;
    assert!(relay.version().await.is_ok());
    let request=task.await.unwrap();assert!(request.starts_with("GET /api/version HTTP/1.1"));
    assert!(request.to_ascii_lowercase().contains("authorization: basic "));
    std::fs::remove_dir_all(dir).unwrap();
}
#[tokio::test]
async fn old_version_is_rejected_after_transport_success() {
    let response=json_response(json!({"weechat_version":"4.6.3","relay_api_version":"0.0.1"}));
    let (relay,dir,task)=fixture(&response).await;
    assert_eq!(relay.version().await.unwrap_err().code,"relay_upgrade_required");
    task.await.unwrap();std::fs::remove_dir_all(dir).unwrap();
}
#[tokio::test]
async fn input_uses_explicit_buffer_and_reports_only_client_acceptance() {
    let (relay,dir,task)=fixture("HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
    relay.input("irc.pilot.#agent-pilot","literal message").await.unwrap();
    let request=task.await.unwrap();let body=request.split_once("\r\n\r\n").unwrap().1;
    let value:serde_json::Value=serde_json::from_str(body).unwrap();
    assert_eq!(value["buffer_name"],"irc.pilot.#agent-pilot");assert_eq!(value["command"],"literal message");
    std::fs::remove_dir_all(dir).unwrap();
}
#[tokio::test]
async fn oversized_body_is_rejected_without_reflecting_it() {
    let body="a".repeat(1024*1024+1);
    let response=format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body);
    let (relay,dir,task)=fixture(&response).await;
    assert!(relay.version().await.is_err());
    // A bounded client may close early; the mock writer can observe that close.
    let _=task.await;std::fs::remove_dir_all(dir).unwrap();
}
