//! Private WeeChat API adapter. No arbitrary endpoints or command passthrough.
use crate::{
    config::{Config, Conversation},
    domain::{Failure, Result},
    events::Hub,
    relay_events::RelayEvents,
};
use base64::Engine;
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use http_body_util::{BodyExt, Full};
use hyper_util::rt::TokioIo;
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};
use tokio::net::UnixStream;
use tokio_tungstenite::tungstenite::{
    client::IntoClientRequest,
    protocol::{Message, WebSocketConfig},
};
use tokio_util::sync::CancellationToken;

const MAX_RESPONSE: usize = 1024 * 1024;
const DEADLINE: Duration = Duration::from_secs(10);
#[derive(Clone)]
pub struct Relay {
    cfg: Arc<Config>,
    authorization: http::HeaderValue,
}
struct AbortTask(tokio::task::JoinHandle<()>);
impl Drop for AbortTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}
impl Relay {
    pub fn new(cfg: Arc<Config>, password: String) -> Result<Self> {
        let encoded = base64::engine::general_purpose::STANDARD.encode(format!("plain:{password}"));
        let mut authorization = http::HeaderValue::from_str(&format!("Basic {encoded}"))
            .map_err(|_| Failure::new("secret_file", "Invalid Relay credential."))?;
        authorization.set_sensitive(true);
        Ok(Self { cfg, authorization })
    }
    async fn exchange(
        &self,
        method: http::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<(u16, Value)> {
        let future = async {
            let stream = UnixStream::connect(&self.cfg.relay_socket)
                .await
                .map_err(|_| Failure::backend())?;
            let (mut sender, connection) =
                hyper::client::conn::http1::handshake(TokioIo::new(stream))
                    .await
                    .map_err(|_| Failure::backend())?;
            let _driver = AbortTask(tokio::spawn(async move {
                let _ = connection.await;
            }));
            let bytes = body
                .map(|v| serde_json::to_vec(&v))
                .transpose()
                .map_err(|_| Failure::shape())?
                .unwrap_or_default();
            let req = http::Request::builder()
                .method(method)
                .uri(path)
                .header(http::header::HOST, "localhost")
                .header(http::header::AUTHORIZATION, self.authorization.clone())
                .header(http::header::CONTENT_TYPE, "application/json")
                .header(http::header::ACCEPT, "application/json")
                .header(http::header::CONNECTION, "close")
                .body(Full::new(Bytes::from(bytes)))
                .map_err(|_| Failure::shape())?;
            let response = sender
                .send_request(req)
                .await
                .map_err(|_| Failure::backend())?;
            let status = response.status().as_u16();
            if response
                .headers()
                .contains_key(http::header::CONTENT_ENCODING)
            {
                return Err(Failure::shape());
            }
            let mut body = response.into_body();
            let mut data = Vec::new();
            while let Some(frame) = body.frame().await {
                let frame = frame.map_err(|_| Failure::backend())?;
                if let Some(chunk) = frame.data_ref() {
                    if chunk.len() > MAX_RESPONSE.saturating_sub(data.len()) {
                        return Err(Failure::new(
                            "relay_response_limit",
                            "Relay response exceeded the byte budget.",
                        ));
                    }
                    data.extend_from_slice(chunk);
                }
            }
            let value = if data.is_empty() {
                Value::Null
            } else {
                serde_json::from_slice(&data).map_err(|_| Failure::shape())?
            };
            Ok((status, value))
        };
        tokio::time::timeout(DEADLINE, future)
            .await
            .map_err(|_| Failure::backend())?
    }
    pub async fn version(&self) -> Result<Value> {
        let (status, v) = self
            .exchange(http::Method::GET, "/api/version", None)
            .await?;
        if status != 200 {
            return Err(Failure::backend());
        }
        validate_version(&v, &self.cfg.relay_api_versions)?;
        Ok(v)
    }
    pub async fn buffer(&self, name: &str, lines: usize, nicks: bool) -> Result<Value> {
        let path = format!(
            "/api/buffers/{}?lines=-{}&nicks={}&colors=strip",
            percent_encoding::utf8_percent_encode(name, percent_encoding::NON_ALPHANUMERIC),
            lines,
            nicks
        );
        let (status, v) = self.exchange(http::Method::GET, &path, None).await?;
        if status == 404 {
            return Err(Failure::new(
                "not_joined",
                "Configured conversation is not open in WeeChat.",
            ));
        }
        if status != 200 || v.get("name").and_then(Value::as_str) != Some(name) {
            return Err(Failure::shape());
        }
        Ok(v)
    }
    pub async fn conversation(&self, c: &Conversation, lines: usize, nicks: bool) -> Result<Value> {
        let v = self
            .buffer(&c.buffer(&self.cfg.server), lines, nicks)
            .await?;
        let vars = v.get("local_variables").ok_or_else(Failure::shape)?;
        if vars.get("plugin").and_then(Value::as_str) != Some("irc")
            || vars.get("server").and_then(Value::as_str) != Some(self.cfg.server.as_str())
            || vars.get("channel").and_then(Value::as_str) != Some(c.target.as_str())
        {
            return Err(Failure::shape());
        }
        Ok(v)
    }
    /// An OK means only that WeeChat accepted input, never server delivery.
    pub async fn input(&self, buffer_name: &str, command: &str) -> Result<()> {
        let (status, _) = self
            .exchange(
                http::Method::POST,
                "/api/input",
                Some(json!({"buffer_name":buffer_name,"command":command})),
            )
            .await?;
        if status != 204 {
            return Err(Failure::backend());
        }
        Ok(())
    }
    pub async fn observe(&self, hub: Hub, stop: CancellationToken) {
        let mut delay = 1u64;
        loop {
            let _ = hub.boundary(false);
            let result = tokio::select! { _=stop.cancelled()=>break, r=self.observe_once(&hub)=>r };
            let _ = hub.boundary(false);
            if stop.is_cancelled() {
                break;
            }
            // No raw backend errors, credentials, names or message text in logs.
            eprintln!(
                "relay event stream disconnected; reconnect scheduled (successful_close={})",
                result.is_ok()
            );
            let jitter = u64::from(uuid::Uuid::new_v4().as_bytes()[0]);
            tokio::select! { _=stop.cancelled()=>break, _=tokio::time::sleep(Duration::from_millis(delay*1000+jitter))=>{} }
            delay = (delay * 2).min(30);
        }
        let _ = hub.boundary(false);
    }
    async fn observe_once(&self, hub: &Hub) -> Result<()> {
        self.version().await?;
        let stream = tokio::time::timeout(DEADLINE, UnixStream::connect(&self.cfg.relay_socket))
            .await
            .map_err(|_| Failure::backend())?
            .map_err(|_| Failure::backend())?;
        let mut request = "ws://localhost/api"
            .into_client_request()
            .map_err(|_| Failure::shape())?;
        request
            .headers_mut()
            .insert(http::header::AUTHORIZATION, self.authorization.clone());
        let mut ws_config = WebSocketConfig::default();
        ws_config.max_message_size = Some(MAX_RESPONSE);
        ws_config.max_frame_size = Some(MAX_RESPONSE);
        let (mut ws, _) = tokio::time::timeout(
            DEADLINE,
            tokio_tungstenite::client_async_with_config(request, stream, Some(ws_config)),
        )
        .await
        .map_err(|_| Failure::backend())?
        .map_err(|_| Failure::backend())?;
        // Send individual documented request objects, not an unqualified JSON batch.
        send_frame(
            &mut ws,
            Message::Text(
                json!({
                    "request":"POST /api/sync", "request_id":"sync",
                    "body":{"nicks":false,"input":false,"colors":"strip"}
                })
                .to_string()
                .into(),
            ),
        )
        .await?;
        let mut events = RelayEvents::new(self.cfg.clone());
        let mut sync_seen = false;
        let mut ready = false;
        let startup_deadline = tokio::time::sleep(Duration::from_secs(20));
        tokio::pin!(startup_deadline);
        let mut tick = tokio::time::interval(Duration::from_secs(30));
        let mut last_frame = tokio::time::Instant::now();
        loop {
            tokio::select! {
                _=&mut startup_deadline, if !ready => return Err(Failure::backend()),
                _=tick.tick()=> {
                    hub.prune_inbox()?;
                    if last_frame.elapsed()>Duration::from_secs(90) {return Err(Failure::backend());}
                    tokio::time::timeout(DEADLINE,ws.send(Message::Ping(Vec::new().into()))).await.map_err(|_| Failure::backend())?.map_err(|_| Failure::backend())?;
                }
                frame=ws.next()=> {
                    last_frame=tokio::time::Instant::now();
                    match frame {
                        Some(Ok(Message::Text(text)))=> {
                            let v:Value=serde_json::from_str(&text).map_err(|_| Failure::shape())?;
                            if v.get("request_id").and_then(Value::as_str)==Some("sync") {
                                if sync_seen || v.get("code").and_then(Value::as_u64)!=Some(204) {return Err(Failure::shape());}
                                sync_seen=true;
                                // Subscribe before taking the metadata baseline. Coverage begins
                                // at the snapshot response on this same ordered WebSocket.
                                send_frame(&mut ws, Message::Text(json!({
                                    "request":"GET /api/buffers?lines=0&nicks=false&colors=strip",
                                    "request_id":"snapshot"
                                }).to_string().into())).await?;
                            } else if v.get("request_id").and_then(Value::as_str)==Some("snapshot") {
                                if !sync_seen || ready || v.get("code").and_then(Value::as_u64)!=Some(200) {return Err(Failure::shape());}
                                events.snapshot(v.get("body").ok_or_else(Failure::shape)?)?;
                                hub.boundary(true)?;
                                ready=true;
                            } else if v.get("code").and_then(Value::as_u64)==Some(0) {
                                // Pre-baseline traffic is deliberately not claimed as captured.
                                if ready { events.apply(&v, hub)?; }
                            } else {return Err(Failure::shape());}
                        }
                        Some(Ok(Message::Ping(payload)))=> { send_frame(&mut ws,Message::Pong(payload)).await?; }
                        Some(Ok(Message::Pong(_)))=>{}
                        Some(Ok(Message::Close(_)))|None=>return Ok(()),
                        Some(Ok(_))=>return Err(Failure::shape()),
                        Some(Err(_))=>return Err(Failure::backend()),
                    }
                }
            }
        }
    }
}
async fn send_frame(
    ws: &mut tokio_tungstenite::WebSocketStream<UnixStream>,
    frame: Message,
) -> Result<()> {
    tokio::time::timeout(DEADLINE, ws.send(frame))
        .await
        .map_err(|_| Failure::backend())?
        .map_err(|_| Failure::backend())
}

pub fn validate_version(value: &Value, apis: &[String]) -> Result<()> {
    let raw = value
        .get("weechat_version")
        .and_then(Value::as_str)
        .ok_or_else(Failure::shape)?;
    let v = semver::Version::parse(raw).map_err(|_| Failure::shape())?;
    if v < semver::Version::new(4, 10, 1) || !v.pre.is_empty() {
        return Err(Failure::new("relay_upgrade_required","A stable WeeChat 4.10.1 or later is required. Confirm current vendor advisories before deployment."));
    }
    let api = value
        .get("relay_api_version")
        .and_then(Value::as_str)
        .ok_or_else(Failure::shape)?;
    if !apis.iter().any(|a| a == api) {
        return Err(Failure::new(
            "relay_api_version",
            "Relay API version has not been explicitly accepted in configuration.",
        ));
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn refuses_vulnerable_or_prerelease_versions() {
        for v in ["4.6.3", "4.9.4", "4.10.0", "4.10.1-rc.1"] {
            assert!(validate_version(
                &json!({"weechat_version":v,"relay_api_version":"0.0.1"}),
                &["0.0.1".into()]
            )
            .is_err());
        }
    }
    #[test]
    fn accepts_explicit_api_only() {
        let v = json!({"weechat_version":"4.10.1","relay_api_version":"0.0.1"});
        assert!(validate_version(&v, &["0.0.1".into()]).is_ok());
        assert!(validate_version(&v, &["other".into()]).is_err());
    }
}
