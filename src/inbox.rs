//! Bounded, non-destructive attention event projection, not a durable message queue.
//!
//! Only live observations enter this buffer. A generation change discards its contents;
//! retention eviction advances an explicit loss floor. Reads never acknowledge delivery.
use crate::{
    attention::{AttentionConfig, Reason},
    config::Conversation,
    domain::{Failure, Message, RelayLine, Result},
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, VecDeque},
    time::{Duration, Instant},
};

pub const INBOX_URI: &str = "irc://configured/inbox";
const PAGE_BYTES: usize = 32 * 1024;

#[derive(Debug, Clone, Serialize)]
pub struct Item {
    pub event_id: String,
    pub target: String,
    pub resource_uri: String,
    pub kind: &'static str,
    pub reason: Reason,
    pub delivery_timing: &'static str,
    pub message: Message,
    pub text_truncated: bool,
    pub original_text_bytes: usize,
}
struct Stored {
    sequence: u64,
    item: Item,
    at: Instant,
    bytes: usize,
}
#[derive(Debug, Clone)]
pub struct Page {
    pub items: Vec<Item>,
    pub position: u64,
    pub tail: u64,
    pub loss_floor: u64,
    pub more: bool,
    pub retained_items: usize,
}
pub struct Inbox {
    policy: AttentionConfig,
    generation: String,
    entries: VecDeque<Stored>,
    bytes: usize,
    sequence: u64,
    loss_floor: u64,
    seen: BTreeMap<(u64, u64), [u8; 32]>,
    seen_order: VecDeque<(u64, u64)>,
}
impl Inbox {
    pub fn new(policy: AttentionConfig, generation: String) -> Self {
        Self {
            policy,
            generation,
            entries: VecDeque::new(),
            bytes: 0,
            sequence: 0,
            loss_floor: 0,
            seen: BTreeMap::new(),
            seen_order: VecDeque::new(),
        }
    }
    pub fn reset(&mut self, generation: String) {
        self.generation = generation;
        self.entries.clear();
        self.bytes = 0;
        self.sequence = 0;
        self.loss_floor = 0;
        self.seen.clear();
        self.seen_order.clear();
    }
    fn evict(&mut self) {
        if let Some(item) = self.entries.pop_front() {
            self.bytes = self.bytes.saturating_sub(item.bytes);
            self.loss_floor = item.sequence;
        }
    }
    pub fn prune(&mut self, now: Instant) -> bool {
        let before = self.loss_floor;
        let age = Duration::from_secs(self.policy.max_age_seconds);
        while self
            .entries
            .front()
            .is_some_and(|e| now.saturating_duration_since(e.at) >= age)
        {
            self.evict();
        }
        before != self.loss_floor
    }
    /// Records a distinct relevant line or revision. Same text in different messages survives.
    /// Errors rather than silently dropping a relevant event that cannot fit its contract.
    pub fn record(
        &mut self,
        c: &Conversation,
        own_nick: &str,
        buffer_id: u64,
        line: &RelayLine,
        edited: bool,
        now: Instant,
    ) -> Result<bool> {
        let expired = self.prune(now);
        let Some(reason) = self.policy.classify(c, own_nick, line) else {
            // An edit may remove a prior mention. Invalidate the generation instead of
            // leaving an obsolete attention entry readable as if it were current.
            if edited && self.seen.contains_key(&(buffer_id, line.id)) {
                return Err(Failure::new(
                    "inbox_revision_gap",
                    "An attention message changed classification; restart the live view.",
                ));
            }
            return Ok(expired);
        };
        let Some(mut message) = line.message() else {
            return Ok(expired);
        };
        let fingerprint: [u8; 32] =
            Sha256::digest(serde_json::to_vec(&(&message, reason)).map_err(|_| Failure::shape())?)
                .into();
        let key = (buffer_id, line.id);
        if self.seen.get(&key) == Some(&fingerprint) {
            return Ok(expired);
        }
        let was_seen = self.seen.contains_key(&key);
        let original_text_bytes = message.text.len();
        let mut end = original_text_bytes.min(self.policy.max_message_bytes);
        while !message.text.is_char_boundary(end) {
            end -= 1;
        }
        message.text.truncate(end);
        let sequence = self.sequence.checked_add(1).ok_or_else(Failure::shape)?;
        let item = Item {
            event_id: format!("{}:{sequence}", self.generation),
            target: c.key.clone(),
            resource_uri: c.uri(),
            kind: if edited || was_seen {
                "message_updated"
            } else {
                "message"
            },
            reason,
            delivery_timing: reason.timing(),
            message,
            text_truncated: end < original_text_bytes,
            original_text_bytes,
        };
        let bytes = serde_json::to_vec(&item)
            .map_err(|_| Failure::shape())?
            .len();
        if bytes > self.policy.max_bytes || bytes > PAGE_BYTES {
            return Err(Failure::new(
                "inbox_event_too_large",
                "Relevant event exceeds the inbox budget; continuity cannot be retained.",
            ));
        }
        while self.entries.len() >= self.policy.max_entries
            || self.bytes.saturating_add(bytes) > self.policy.max_bytes
        {
            if self.entries.is_empty() {
                return Err(Failure::shape());
            }
            self.evict();
        }
        self.sequence = sequence;
        self.bytes += bytes;
        self.entries.push_back(Stored {
            sequence,
            item,
            at: now,
            bytes,
        });
        self.seen.insert(key, fingerprint);
        self.seen_order.retain(|k| k != &key);
        self.seen_order.push_back(key);
        while self.seen_order.len() > self.policy.max_entries.saturating_mul(2) {
            if let Some(old) = self.seen_order.pop_front() {
                self.seen.remove(&old);
            }
        }
        Ok(true)
    }
    /// Snapshot a page while the caller holds the shared observation lock.
    /// Position is a consumer-supplied opaque cursor payload, not a shared read marker.
    pub fn page(&mut self, after: Option<u64>, limit: usize, now: Instant) -> Result<Page> {
        if !(1..=100).contains(&limit) {
            return Err(Failure::new(
                "invalid_limit",
                "Inbox limit must be between 1 and 100.",
            ));
        }
        self.prune(now);
        let mut position = after.unwrap_or(self.loss_floor);
        if position < self.loss_floor || position > self.sequence {
            return Err(Failure::new("inbox_gap", "The inbox cursor predates retained observations. Start a fresh inbox read and use irc_read for available conversation context."));
        }
        let mut items = Vec::new();
        let mut bytes = 0;
        for entry in self.entries.iter().filter(|e| e.sequence > position) {
            if items.len() >= limit || bytes + entry.bytes > PAGE_BYTES {
                break;
            }
            bytes += entry.bytes;
            items.push(entry.item.clone());
        }
        if let Some(last) = items.last() {
            // Extracting no positions from user text: find the exact server-built event ID.
            if let Some(entry) = self
                .entries
                .iter()
                .find(|e| e.item.event_id == last.event_id)
            {
                position = entry.sequence;
            }
        }
        Ok(Page {
            items,
            position,
            tail: self.sequence,
            loss_floor: self.loss_floor,
            more: position < self.sequence,
            retained_items: self.entries.len(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn c() -> Conversation {
        Conversation {
            key: "pilot".into(),
            target: "#pilot".into(),
            description: String::new(),
            writable: true,
        }
    }
    fn line(id: u64, text: &str) -> RelayLine {
        RelayLine {
            id,
            date: "2026-01-01T00:00:00Z".into(),
            prefix: String::new(),
            message: text.into(),
            tags: vec!["irc_privmsg".into(), "nick_peer".into()],
            highlight: false,
        }
    }
    #[test]
    fn reads_are_non_destructive_and_replayable() {
        let now = Instant::now();
        let mut b = Inbox::new(AttentionConfig::default(), "e1".into());
        b.record(&c(), "agent", 1, &line(1, "agent: hello"), false, now)
            .unwrap();
        let a = b.page(Some(0), 20, now).unwrap();
        let retry = b.page(Some(0), 20, now).unwrap();
        assert_eq!(
            serde_json::to_value(&a.items).unwrap(),
            serde_json::to_value(&retry.items).unwrap()
        );
        assert_eq!(b.page(Some(a.position), 20, now).unwrap().items.len(), 0);
    }
    #[test]
    fn new_event_during_page_consumption_is_not_lost() {
        let now = Instant::now();
        let mut b = Inbox::new(AttentionConfig::default(), "e1".into());
        for id in 1..=3 {
            b.record(&c(), "agent", 1, &line(id, "agent: hello"), false, now)
                .unwrap();
        }
        let page = b.page(Some(1), 1, now).unwrap();
        b.record(&c(), "agent", 1, &line(4, "agent: hello"), false, now)
            .unwrap();
        assert_eq!(page.items[0].message.local_id, "2");
        let next = b.page(Some(page.position), 20, now).unwrap();
        assert_eq!(
            next.items
                .iter()
                .map(|e| e.message.local_id.as_str())
                .collect::<Vec<_>>(),
            vec!["3", "4"]
        );
    }
    #[test]
    fn dedup_uses_message_identity_not_text_and_preserves_edits() {
        let now = Instant::now();
        let mut b = Inbox::new(AttentionConfig::default(), "e1".into());
        assert!(b
            .record(&c(), "agent", 1, &line(1, "agent: hello"), false, now)
            .unwrap());
        assert!(!b
            .record(&c(), "agent", 1, &line(1, "agent: hello"), false, now)
            .unwrap());
        b.record(&c(), "agent", 1, &line(2, "agent: hello"), false, now)
            .unwrap();
        b.record(&c(), "agent", 1, &line(1, "agent: corrected"), true, now)
            .unwrap();
        let p = b.page(None, 20, now).unwrap();
        assert_eq!(p.items.len(), 3);
        assert_eq!(p.items[2].kind, "message_updated");
    }
    #[test]
    fn eviction_and_expiry_are_explicit_gaps() {
        let now = Instant::now();
        let policy = AttentionConfig {
            max_entries: 2,
            max_age_seconds: 1,
            ..AttentionConfig::default()
        };
        let mut b = Inbox::new(policy, "e1".into());
        for id in 1..=3 {
            b.record(&c(), "agent", 1, &line(id, "agent: hello"), false, now)
                .unwrap();
        }
        assert_eq!(b.page(Some(0), 20, now).unwrap_err().code, "inbox_gap");
        assert_eq!(b.page(None, 20, now).unwrap().loss_floor, 1);
        let later = now + Duration::from_secs(1);
        assert!(b.page(Some(2), 20, later).is_err());
        assert_eq!(b.page(None, 20, later).unwrap().items.len(), 0);
    }
    #[test]
    fn byte_and_utf8_limits_are_enforced() {
        let now = Instant::now();
        let policy = AttentionConfig {
            max_message_bytes: 65,
            max_bytes: 1500,
            ..AttentionConfig::default()
        };
        let mut b = Inbox::new(policy, "e1".into());
        for id in 0..20 {
            b.record(
                &c(),
                "agent",
                1,
                &line(id, &format!("agent: {}", "é".repeat(100))),
                false,
                now,
            )
            .unwrap();
        }
        assert!(b.bytes <= 1500);
        let p = b.page(None, 100, now).unwrap();
        assert!(p.loss_floor > 0);
        assert!(p
            .items
            .iter()
            .all(|e| e.text_truncated && e.message.text.len() <= 65));
    }
    #[test]
    fn reset_clears_contents_and_identity_dedup() {
        let now = Instant::now();
        let mut b = Inbox::new(AttentionConfig::default(), "e1".into());
        b.record(&c(), "agent", 1, &line(1, "agent: hello"), false, now)
            .unwrap();
        b.reset("e2".into());
        assert!(b.page(None, 20, now).unwrap().items.is_empty());
        b.record(&c(), "agent", 1, &line(1, "agent: hello"), false, now)
            .unwrap();
        assert_eq!(b.page(None, 20, now).unwrap().items[0].event_id, "e2:1");
    }
}
