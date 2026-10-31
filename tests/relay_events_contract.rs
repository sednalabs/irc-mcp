//! Real Unix-socket HTTP and WebSocket fixture for the Relay observer.
//! This is not a live WeeChat test; it verifies our adapter against documented frames.
use futures_util::{SinkExt, StreamExt};
use irc_mcp::{config::Config, events::Hub, relay::Relay};
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};
use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::UnixListener};
use tokio_tungstenite::{tungstenite::Message, WebSocketStream};
use tokio_util::sync::CancellationToken;

async fn next_json(ws:&mut WebSocketStream<tokio::net::UnixStream>)->Value {
    loop {
        match tokio::time::timeout(Duration::from_secs(3),ws.next()).await.unwrap().unwrap().unwrap() {
            Message::Text(t)=>return serde_json::from_str(&t).unwrap(),
            Message::Ping(p)=>ws.send(Message::Pong(p)).await.unwrap(),
            other=>panic!("unexpected frame: {other:?}"),
        }
    }
}
#[tokio::test]
async fn observer_uses_objects_syncs_before_snapshot_and_routes_only_relevant_events() {
    let dir=std::env::temp_dir().join(format!("relay-events-{}",uuid::Uuid::new_v4()));
    std::fs::create_dir(&dir).unwrap();let socket=dir.join("relay.sock");
    let listener=UnixListener::bind(&socket).unwrap();
    let server=tokio::spawn(async move {
        let (mut http,_)=listener.accept().await.unwrap();let mut headers=Vec::new();
        while !headers.ends_with(b"\r\n\r\n") {
            let byte=http.read_u8().await.unwrap();headers.push(byte);assert!(headers.len()<8192);
        }
        let body=json!({"weechat_version":"4.10.1","relay_api_version":"0.0.1"}).to_string();
        http.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).as_bytes()).await.unwrap();
        drop(http);
        let (stream,_)=listener.accept().await.unwrap();
        let mut ws=tokio_tungstenite::accept_async(stream).await.unwrap();
        let sync=next_json(&mut ws).await;assert_eq!(sync["request"],"POST /api/sync");
        assert!(sync.is_object());
        ws.send(Message::Text(json!({"request_id":"sync","code":204}).to_string().into())).await.unwrap();
        let snapshot=next_json(&mut ws).await;
        assert_eq!(snapshot["request_id"],"snapshot");assert!(snapshot.is_object());
        ws.send(Message::Text(json!({"request_id":"snapshot","code":200,"body":[
            {"id":10,"name":"irc.pilot.#agent-pilot","local_variables":{
                "plugin":"irc","server":"pilot","channel":"#agent-pilot","type":"channel","nick":"agent"}}
        ]}).to_string().into())).await.unwrap();
        for (id,text,nick) in [(1,"routine status","peer"),(2,"agent: question","peer"),(3,"agent: question","agent")] {
            ws.send(Message::Text(json!({"code":0,"event_name":"buffer_line_added","buffer_id":10,"body":{
                "id":id,"date":"2026-01-01T00:00:00Z","message":text,"tags":["irc_privmsg",format!("nick_{nick}")]}
            }).to_string().into())).await.unwrap();
        }
        // Keep the fixture connection alive until the observer is cancelled.
        while let Some(Ok(frame))=ws.next().await {
            if let Message::Ping(p)=frame {let _=ws.send(Message::Pong(p)).await;}
        }
    });
    let mut cfg:Config=toml::from_str(include_str!("../config.example.toml")).unwrap();cfg.relay_socket=socket;
    let relay=Relay::new(Arc::new(cfg),"fixture-only".into()).unwrap();
    let hub=Hub::new();let observed=hub.clone();let stop=CancellationToken::new();let stop_task=stop.clone();
    let observer=tokio::spawn(async move {relay.observe(observed,stop_task).await;});
    tokio::time::timeout(Duration::from_secs(3),async {
        loop {
            let state=hub.observation().unwrap();
            if state.relay_events_connected {
                let page=hub.inbox_page(&state.generation,None,20).unwrap();
                if !page.items.is_empty() {
                    assert_eq!(page.items.len(),1);assert_eq!(page.items[0].message.text,"agent: question");break;
                }
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }).await.unwrap();
    stop.cancel();tokio::time::timeout(Duration::from_secs(2),observer).await.unwrap().unwrap();
    tokio::time::timeout(Duration::from_secs(2),server).await.unwrap().unwrap();
    assert!(!hub.observation().unwrap().relay_events_connected);
    std::fs::remove_dir_all(dir).unwrap();
}
