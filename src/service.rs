//! Shared, authorised conversation implementation used by tools and resources.
use crate::inbox::INBOX_URI;
use crate::{
    config::Config,
    domain::{validate_request_key, validate_text, Failure, RelayLine, Result},
    events::Hub,
    receipts::{receipt, Receipts, Reservation},
    relay::Relay,
};
use mcp_toolkit_resource_state::{CursorScope, CursorStore};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::Semaphore;

#[derive(Clone)]
pub struct Principal {
    pub subject: String,
    pub scopes: BTreeSet<String>,
    pub expires: u64,
}
impl Principal {
    pub fn require(&self, scope: &str) -> Result<()> {
        if self.expires <= epoch()? || !self.scopes.contains(scope) {
            return Err(Failure::denied());
        }
        Ok(())
    }
}
pub fn epoch() -> Result<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|v| v.as_secs())
        .map_err(|_| Failure::new("clock", "System clock is invalid."))
}

#[derive(Serialize, Deserialize)]
struct ReadCursor {
    buffer_id: u64,
    line_id: u64,
}
#[derive(Serialize, Deserialize)]
struct InboxCursor {
    position: u64,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InboxRead {
    pub after: Option<String>,
    pub limit: Option<usize>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Empty {}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub target: String,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Read {
    pub target: String,
    pub after: Option<String>,
    pub limit: Option<usize>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Send {
    pub target: String,
    pub text: String,
    pub request_key: String,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Membership {
    pub target: String,
    pub request_key: String,
}

pub struct Service {
    pub cfg: Arc<Config>,
    pub relay: Relay,
    pub hub: Hub,
    cursors: Mutex<CursorStore>,
    receipts: Receipts,
    writes: Semaphore,
    requests: Semaphore,
    last_write: Mutex<Option<Instant>>,
}
impl Service {
    pub fn new(cfg: Arc<Config>, relay: Relay) -> Result<Self> {
        cfg.validate()?;
        cfg.validate_state_dir()?;
        let receipts = Receipts::open(&cfg.state_dir.join("receipts.sqlite"))?;
        let cursors = CursorStore::new(Duration::from_secs(900), 2000, 1024 * 1024)
            .map_err(|_| Failure::new("configuration", "Invalid cursor limits."))?;
        let hub = Hub::with_attention(cfg.attention.clone());
        Ok(Self {
            cfg,
            relay,
            hub,
            cursors: Mutex::new(cursors),
            receipts,
            writes: Semaphore::new(1),
            requests: Semaphore::new(8),
            last_write: Mutex::new(None),
        })
    }
    pub async fn call(&self, name: &str, args: Value, p: &Principal) -> Result<Value> {
        self.authorize(p)?;
        let _request = self
            .requests
            .try_acquire()
            .map_err(|_| Failure::new("busy", "The service is busy; retry later."))?;
        if args
            .as_object()
            .is_none_or(|m| m.values().any(Value::is_null))
        {
            return Err(Failure::new(
                "invalid_arguments",
                "Arguments must be an object; omit optional fields rather than passing null.",
            ));
        }
        let output = match name {
            "irc_status" => {
                let _: Empty = parse(args)?;
                self.status().await
            }
            "irc_channels" => {
                let _: Empty = parse(args)?;
                Ok(self.channels(p))
            }
            "irc_channel_info" => {
                let a: Target = parse(args)?;
                self.info(&a.target).await
            }
            "irc_inbox" => {
                let a: InboxRead = parse(args)?;
                self.inbox(a, p)
            }
            "irc_read" => {
                let a: Read = parse(args)?;
                self.read(a, p).await
            }
            "irc_send" => {
                let a: Send = parse(args)?;
                validate_text(&a.text)?;
                self.write("send", &a.target, &a.text, &a.request_key, p)
                    .await
            }
            "irc_join" | "irc_part" => {
                let a: Membership = parse(args)?;
                self.write(
                    if name == "irc_join" { "join" } else { "part" },
                    &a.target,
                    "",
                    &a.request_key,
                    p,
                )
                .await
            }
            _ => Err(Failure::new("unknown_tool", "Unknown tool.")),
        }?;
        if !matches!(name, "irc_send" | "irc_join" | "irc_part") {
            p.require("irc.read")?;
        }
        bounded(output)
    }
    pub fn resource_uris(&self) -> Vec<String> {
        let mut v = vec!["irc://configured/status".into(), INBOX_URI.into()];
        v.extend(self.cfg.conversations.iter().map(|c| c.uri()));
        v
    }
    pub async fn resource(&self, uri: &str, p: &Principal) -> Result<Value> {
        self.authorize(p)?;
        let _request = self
            .requests
            .try_acquire()
            .map_err(|_| Failure::new("busy", "The service is busy; retry later."))?;
        let value = if uri == "irc://configured/status" {
            self.status().await?
        } else if uri == INBOX_URI {
            self.inbox(
                InboxRead {
                    after: None,
                    limit: Some(20),
                },
                p,
            )?
        } else {
            let c = self
                .cfg
                .conversations
                .iter()
                .find(|c| c.uri() == uri)
                .ok_or_else(Failure::denied)?;
            self.read(
                Read {
                    target: c.key.clone(),
                    after: None,
                    limit: Some(20),
                },
                p,
            )
            .await?
        };
        self.authorize(p)?;
        bounded(value)
    }
    fn authorize(&self, p: &Principal) -> Result<()> {
        p.require("irc.read")?;
        if p.subject != self.cfg.subject {
            return Err(Failure::denied());
        }
        Ok(())
    }
    fn inbox(&self, a: InboxRead, p: &Principal) -> Result<Value> {
        let before = self.hub.observation()?;
        if !before.relay_events_connected {
            return Err(Failure::new("event_stream_unavailable","The live Relay stream is unavailable; an empty inbox cannot establish no messages were missed."));
        }
        let scope = CursorScope {
            principal: p.subject.clone(),
            resource: INBOX_URI.into(),
            generation: before.generation.clone(),
        };
        let position = if let Some(token) = a.after.as_deref() {
            let data=self.cursors.lock().map_err(|_|Failure::backend())?
                .resolve(token,&scope,Instant::now()).map_err(|_|Failure::new("invalid_cursor","Inbox cursor is invalid, expired or from another reader or generation. Start a new bounded read."))?;
            Some(
                serde_json::from_slice::<InboxCursor>(&data)
                    .map_err(|_| Failure::shape())?
                    .position,
            )
        } else {
            None
        };
        let page = self
            .hub
            .inbox_page(&before.generation, position, a.limit.unwrap_or(20))?;
        let next = if position == Some(page.position) {
            a.after
        } else {
            let token = uuid::Uuid::new_v4().simple().to_string();
            let payload = serde_json::to_vec(&InboxCursor {
                position: page.position,
            })
            .map_err(|_| Failure::shape())?;
            self.cursors
                .lock()
                .map_err(|_| Failure::backend())?
                .insert(token.clone(), scope, payload, Instant::now())
                .map_err(|_| {
                    Failure::new("cursor_capacity", "Cursor storage is full. Retry later.")
                })?;
            Some(token)
        };
        let after = self.hub.observation()?;
        if after.generation != before.generation || !after.relay_events_connected {
            return Err(gap());
        }
        self.authorize(p)?;
        bounded(
            json!({"items":page.items,"next_cursor":next,"more_in_window":page.more,
            "retained_items":page.retained_items,"retention_trimmed":page.loss_floor>0,
            "source":"live_relay_events_since_generation","earlier_history_complete":false,
            "consumption":"not_acknowledged","reader_position":"caller_managed_cursor",
            "generation":before.generation,"resource_uri":INBOX_URI,
            "model_wake":"not_performed","receiver_policy":self.cfg.attention}),
        )
    }
    async fn status(&self) -> Result<Value> {
        let v = self.relay.version().await?;
        let state = self.hub.observation()?;
        Ok(
            json!({"identity_label":self.cfg.display_identity,"weechat_version":v["weechat_version"],
            "relay_api_version":v["relay_api_version"],"relay_api_reachable":true,
            "relay_events_connected":state.relay_events_connected,"generation":state.generation,
            "irc_connection":"not_attested","model_attention":"not_inferred",
            "history":"bounded_weechat_buffer_only","server_chathistory":false,
            "attention_inbox_uri":INBOX_URI,"attention_source":"live_relay_events_since_generation",
            "attention_policy":self.cfg.attention,"dot_webhooks":false,"identity_mode":"one_authorised_subject_one_service_identity"}),
        )
    }
    fn channels(&self, p: &Principal) -> Value {
        json!({"conversations":self.cfg.conversations.iter().map(|c|json!({"target":c.key,"name":c.target,"kind":if c.target.starts_with('#'){"channel"}else{"direct_message"},"description":c.description,"writable":c.writable && p.scopes.contains("irc.write"),"resource_uri":c.uri()})).collect::<Vec<_>>()})
    }
    async fn info(&self, key: &str) -> Result<Value> {
        let c = self.cfg.conversation(key)?;
        let b = self.relay.conversation(c, 0, true).await?;
        let mut members = Vec::new();
        if let Some(root) = b.get("nicklist_root") {
            collect_members(root, &mut members, 0);
        }
        Ok(
            json!({"target":key,"name":c.target,"topic":b.get("title"),"members":members,
            "members_complete":false,"resource_uri":c.uri(),"history_source":"weechat_buffer"}),
        )
    }
    async fn read(&self, a: Read, p: &Principal) -> Result<Value> {
        let limit = a.limit.unwrap_or(20);
        if !(1..=100).contains(&limit) {
            return Err(Failure::new(
                "invalid_limit",
                "Read limit must be between 1 and 100.",
            ));
        }
        let c = self.cfg.conversation(&a.target)?;
        let before = self.hub.observation()?;
        if !before.relay_events_connected {
            return Err(Failure::new("event_stream_unavailable","Relay event continuity is unavailable. Wait for reconnection before reading with cursors."));
        }
        let b = self.relay.conversation(c, 200, false).await?;
        let buffer_id = b
            .get("id")
            .and_then(Value::as_u64)
            .ok_or_else(Failure::shape)?;
        let raw = b
            .get("lines")
            .and_then(Value::as_array)
            .ok_or_else(Failure::shape)?;
        if raw.len() > 200 {
            return Err(Failure::shape());
        }
        let lines: Vec<RelayLine> = raw
            .iter()
            .map(|v| serde_json::from_value(v.clone()).map_err(|_| Failure::shape()))
            .collect::<Result<_>>()?;
        if lines.windows(2).any(|pair| pair[0].id >= pair[1].id) {
            return Err(Failure::shape());
        }
        let scope = CursorScope {
            principal: p.subject.clone(),
            resource: c.uri(),
            generation: before.generation.clone(),
        };
        let start = if let Some(token) = a.after.as_deref() {
            let data=self.cursors.lock().map_err(|_| Failure::backend())?.resolve(token,&scope,Instant::now())
                .map_err(|_| Failure::new("invalid_cursor","Cursor is invalid, expired or from a different reader or generation. Start a new bounded read."))?;
            let cursor: ReadCursor = serde_json::from_slice(&data).map_err(|_| Failure::shape())?;
            if cursor.buffer_id != buffer_id {
                return Err(gap());
            }
            lines
                .iter()
                .position(|l| l.id == cursor.line_id)
                .map(|i| i + 1)
                .ok_or_else(gap)?
        } else {
            lines.len().saturating_sub(limit)
        };
        let end = (start + limit).min(lines.len());
        let selected = &lines[start..end];
        let messages: Vec<_> = selected.iter().filter_map(RelayLine::message).collect();
        let after = self.hub.observation()?;
        if before.generation != after.generation || !after.relay_events_connected {
            return Err(gap());
        }
        p.require("irc.read")?;
        let next = if let Some(line) = selected.last() {
            let token = uuid::Uuid::new_v4().simple().to_string();
            let payload = serde_json::to_vec(&ReadCursor {
                buffer_id,
                line_id: line.id,
            })
            .map_err(|_| Failure::shape())?;
            self.cursors
                .lock()
                .map_err(|_| Failure::backend())?
                .insert(token.clone(), scope, payload, Instant::now())
                .map_err(|_| {
                    Failure::new("cursor_capacity", "Cursor storage is full. Retry later.")
                })?;
            Some(token)
        } else {
            a.after
        };
        let value = json!({"target":c.key,"messages":messages,"next_cursor":next,"more_in_window":end<lines.len(),
            "history_source":"weechat_buffer","continuity":"bounded_window_only","earlier_history_complete":false,
            "generation":after.generation,"scanned_lines":selected.len(),"resource_uri":c.uri()});
        if serde_json::to_vec(&value)
            .map_err(|_| Failure::shape())?
            .len()
            > 48 * 1024
        {
            return Err(Failure::new(
                "result_too_large",
                "Read result exceeds the byte budget; reduce the limit.",
            ));
        }
        Ok(value)
    }
    async fn write(
        &self,
        action: &str,
        key: &str,
        text: &str,
        request_key: &str,
        p: &Principal,
    ) -> Result<Value> {
        p.require("irc.write")?;
        validate_request_key(request_key)?;
        let c = self.cfg.conversation(key)?;
        if !c.writable {
            return Err(Failure::denied());
        }
        if action != "send" && !c.target.starts_with('#') {
            return Err(Failure::new(
                "channel_required",
                "Join and part apply only to configured channels.",
            ));
        }
        let fingerprint = format!(
            "{:x}",
            Sha256::digest(
                serde_json::to_vec(&json!([action, key, text])).map_err(|_| Failure::shape())?
            )
        );
        if let Some(r) = self
            .receipts
            .existing(&p.subject, request_key, &fingerprint, epoch()?)?
        {
            return Ok(json!(r));
        }
        let _write = self.writes.try_acquire().map_err(|_| {
            Failure::new(
                "write_busy",
                "Another write is in progress; retry with the same request key.",
            )
        })?;
        p.require("irc.write")?;
        {
            let mut last = self.last_write.lock().map_err(|_| Failure::backend())?;
            if last.is_some_and(|t| t.elapsed() < Duration::from_secs(1)) {
                return Err(Failure::new(
                    "rate_limited",
                    "Writes are limited to one per second. Retry with the same request key.",
                ));
            }
            *last = Some(Instant::now());
        }
        self.relay.version().await?;
        let (buffer, command) = if action == "join" {
            let b = format!("irc.server.{}", self.cfg.server);
            self.relay.buffer(&b, 0, false).await?;
            (b, format!("/join {}", c.target))
        } else {
            self.relay.conversation(c, 0, false).await?;
            (
                c.buffer(&self.cfg.server),
                if action == "part" {
                    "/part".into()
                } else {
                    text.into()
                },
            )
        };
        p.require("irc.write")?;
        match self
            .receipts
            .reserve(&p.subject, request_key, &fingerprint, epoch()?)?
        {
            Reservation::Existing(r) => return Ok(json!(r)),
            Reservation::New => {}
        }
        // If cancelled from here onward the durable reservation stays uncertain.
        let accepted = self.relay.input(&buffer, &command).await.is_ok();
        let outcome = if accepted && self.receipts.accepted(&p.subject, request_key).is_ok() {
            "accepted_by_client"
        } else {
            "outcome_unknown"
        };
        if accepted {
            let _ = self.hub.updated(&c.uri());
        }
        Ok(json!(receipt(request_key, outcome, false)))
    }
}
fn parse<T: serde::de::DeserializeOwned>(v: Value) -> Result<T> {
    serde_json::from_value(v).map_err(|_| {
        Failure::new(
            "invalid_arguments",
            "Arguments do not match this tool's schema.",
        )
    })
}
fn gap() -> Failure {
    Failure::new("history_gap","The buffer or relay generation changed, or the cursor anchor aged out. Start a new bounded read; continuity is not established.")
}
fn collect_members(v: &Value, out: &mut Vec<Value>, depth: usize) {
    if depth > 8 || out.len() >= 200 {
        return;
    }
    if let Some(nicks) = v.get("nicks").and_then(Value::as_array) {
        for n in nicks {
            if out.len() >= 200 {
                break;
            }
            if let Some(name) = n.get("name").and_then(Value::as_str) {
                out.push(json!({"nick":name,"prefix":n.get("prefix"),"attention":"unknown"}));
            }
        }
    }
    if let Some(groups) = v.get("groups").and_then(Value::as_array) {
        for g in groups {
            collect_members(g, out, depth + 1);
        }
    }
}

fn bounded(v: Value) -> Result<Value> {
    if serde_json::to_vec(&v).map_err(|_| Failure::shape())?.len() > 48 * 1024 {
        return Err(Failure::new(
            "result_too_large",
            "Result exceeds the byte budget. Request less data.",
        ));
    }
    Ok(v)
}
