//! Validated Relay event routing, separate from the WebSocket's transport lifecycle.
//! Only configured IRC buffers are mapped; client buffer IDs are never identity proofs.
use crate::{
    attention::valid_nick,
    config::{Config, Conversation},
    domain::{Failure, RelayLine, Result},
    events::Hub,
};
use serde_json::Value;
use std::{collections::HashMap, sync::Arc};

#[derive(Clone)]
struct BufferContext {
    conversation: Conversation,
    own_nick: String,
}
pub struct RelayEvents {
    cfg: Arc<Config>,
    buffers: HashMap<u64, BufferContext>,
}
impl RelayEvents {
    pub fn new(cfg: Arc<Config>) -> Self {
        Self {
            cfg,
            buffers: HashMap::new(),
        }
    }
    pub fn snapshot(&mut self, rows: &Value) -> Result<()> {
        let rows = rows.as_array().ok_or_else(Failure::shape)?;
        if rows.len() > 2048 {
            return Err(Failure::shape());
        }
        self.buffers.clear();
        for row in rows {
            self.map_buffer(row)?;
        }
        Ok(())
    }
    fn map_buffer(&mut self, row: &Value) -> Result<()> {
        let name = row
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(Failure::shape)?;
        let Some(c) = self
            .cfg
            .conversations
            .iter()
            .find(|c| c.buffer(&self.cfg.server) == name)
        else {
            return Ok(());
        };
        let id = row
            .get("id")
            .and_then(Value::as_u64)
            .ok_or_else(Failure::shape)?;
        let vars = row.get("local_variables").ok_or_else(Failure::shape)?;
        let own_nick = vars
            .get("nick")
            .and_then(Value::as_str)
            .ok_or_else(Failure::shape)?;
        let expected_type = if c.target.starts_with('#') {
            "channel"
        } else {
            "private"
        };
        if vars.get("plugin").and_then(Value::as_str) != Some("irc")
            || vars.get("server").and_then(Value::as_str) != Some(self.cfg.server.as_str())
            || vars.get("channel").and_then(Value::as_str) != Some(c.target.as_str())
            || vars.get("type").and_then(Value::as_str) != Some(expected_type)
            || !valid_nick(own_nick)
        {
            return Err(Failure::shape());
        }
        if self
            .buffers
            .get(&id)
            .is_some_and(|old| old.conversation.key != c.key)
        {
            return Err(Failure::shape());
        }
        self.buffers.insert(
            id,
            BufferContext {
                conversation: c.clone(),
                own_nick: own_nick.to_owned(),
            },
        );
        Ok(())
    }
    pub fn apply(&mut self, event: &Value, hub: &Hub) -> Result<()> {
        if event.get("code").and_then(Value::as_i64) != Some(0) {
            return Err(Failure::shape());
        }
        let name = event
            .get("event_name")
            .and_then(Value::as_str)
            .ok_or_else(Failure::shape)?;
        if matches!(name, "upgrade" | "upgrade_ended" | "quit") {
            return Err(Failure::backend());
        }
        let Some(id) = event.get("buffer_id").and_then(Value::as_u64) else {
            return Ok(());
        };
        if name == "buffer_opened" {
            let row = event.get("body").ok_or_else(Failure::shape)?;
            if row.get("id").and_then(Value::as_u64) != Some(id) {
                return Err(Failure::shape());
            }
            self.map_buffer(row)?;
            if let Some(c) = self.buffers.get(&id) {
                hub.updated(&c.conversation.uri())?;
            }
            // Historical lines on buffer_opened are not new-message attention events.
            return Ok(());
        }
        let Some(context) = self.buffers.get(&id).cloned() else {
            return Ok(());
        };
        match name {
            "buffer_closed"
            | "buffer_closing"
            | "buffer_cleared"
            | "buffer_renamed"
            | "buffer_merged"
            | "buffer_unmerged"
            | "buffer_type_changed" => Err(Failure::backend()),
            "buffer_line_added" | "buffer_line_data_changed" => {
                let line: RelayLine =
                    serde_json::from_value(event.get("body").cloned().ok_or_else(Failure::shape)?)
                        .map_err(|_| Failure::shape())?;
                hub.updated(&context.conversation.uri())?;
                hub.record(
                    &context.conversation,
                    &context.own_nick,
                    id,
                    &line,
                    name == "buffer_line_data_changed",
                )
            }
            "buffer_localvar_added" | "buffer_localvar_changed" | "buffer_localvar_removed" => {
                let row = event.get("body").ok_or_else(Failure::shape)?;
                if row.get("id").and_then(Value::as_u64) != Some(id)
                    || row.get("name").and_then(Value::as_str)
                        != Some(context.conversation.buffer(&self.cfg.server).as_str())
                {
                    return Err(Failure::shape());
                }
                self.map_buffer(row)?;
                hub.updated(&context.conversation.uri())
            }
            "buffer_title_changed" => hub.updated(&context.conversation.uri()),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn cfg() -> Arc<Config> {
        Arc::new(toml::from_str(include_str!("../config.example.toml")).unwrap())
    }
    fn snapshot() -> Value {
        json!([{"id":10,"name":"irc.pilot.#agent-pilot","local_variables":{
        "plugin":"irc","server":"pilot","channel":"#agent-pilot","type":"channel","nick":"agent"}}])
    }
    fn event(buffer: u64, name: &str, text: &str) -> Value {
        json!({"code":0,"event_name":name,"buffer_id":buffer,"body":{
        "id":1,"date":"2026-01-01T00:00:00Z","message":text,"tags":["irc_privmsg","nick_peer"]}})
    }
    #[test]
    fn only_admitted_buffers_produce_attention() {
        let mut events = RelayEvents::new(cfg());
        events.snapshot(&snapshot()).unwrap();
        let hub = Hub::new();
        hub.boundary(true).unwrap();
        events
            .apply(&event(99, "buffer_line_added", "agent: foreign"), &hub)
            .unwrap();
        events
            .apply(&event(10, "buffer_line_added", "agent: hello"), &hub)
            .unwrap();
        let generation = hub.observation().unwrap().generation;
        let page = hub.inbox_page(&generation, None, 20).unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].message.text, "agent: hello");
    }
    #[test]
    fn documented_line_update_event_is_handled() {
        let mut events = RelayEvents::new(cfg());
        events.snapshot(&snapshot()).unwrap();
        let hub = Hub::new();
        hub.boundary(true).unwrap();
        events
            .apply(&event(10, "buffer_line_added", "agent: first"), &hub)
            .unwrap();
        events
            .apply(
                &event(10, "buffer_line_data_changed", "agent: revised"),
                &hub,
            )
            .unwrap();
        let p = hub
            .inbox_page(&hub.observation().unwrap().generation, None, 20)
            .unwrap();
        assert_eq!(p.items.len(), 2);
        assert_eq!(p.items[1].kind, "message_updated");
    }
    #[test]
    fn same_display_name_cannot_admit_a_non_irc_buffer() {
        let mut rows = snapshot();
        rows[0]["local_variables"]["plugin"] = json!("exec");
        assert!(RelayEvents::new(cfg()).snapshot(&rows).is_err());
    }
    #[test]
    fn unrelated_buffer_close_does_not_disconnect_the_service() {
        let mut events = RelayEvents::new(cfg());
        events.snapshot(&snapshot()).unwrap();
        let hub = Hub::new();
        hub.boundary(true).unwrap();
        assert!(events
            .apply(
                &json!({"code":0,"event_name":"buffer_closed","buffer_id":999}),
                &hub
            )
            .is_ok());
        assert!(events
            .apply(
                &json!({"code":0,"event_name":"buffer_closed","buffer_id":10}),
                &hub
            )
            .is_err());
    }
}
