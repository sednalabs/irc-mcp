//! A bounded, deterministic dirty-resource set, not an event log.
use std::collections::{BTreeSet, VecDeque};

/// Invalid resource inventory. No authority is inferred from these strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvalidationError { InvalidInventory }

/// Each accepted key can be pending once; insertion order prevents starvation.
#[derive(Debug)]
pub struct InvalidationSet {
    accepted: BTreeSet<String>,
    pending: BTreeSet<String>,
    queue: VecDeque<String>,
}
impl InvalidationSet {
    /// Construct a finite inventory. Caller must authorise every resource first.
    /// Errors when limits, blank keys, duplicates or oversized keys are supplied.
    pub fn new(keys: Vec<String>, max_keys: usize, max_key_bytes: usize)
        -> Result<Self, InvalidationError>
    {
        if keys.is_empty() || max_keys == 0 || max_key_bytes == 0 || keys.len() > max_keys
            || keys.iter().any(|k| k.is_empty() || k.trim() != k || k.len() > max_key_bytes)
        { return Err(InvalidationError::InvalidInventory); }
        let accepted: BTreeSet<_> = keys.iter().cloned().collect();
        if accepted.len() != keys.len() { return Err(InvalidationError::InvalidInventory); }
        Ok(Self { accepted, pending: BTreeSet::new(), queue: VecDeque::new() })
    }
    /// Returns true only when a new accepted key was queued; never grows on foreign input.
    pub fn mark(&mut self, key: &str) -> bool {
        if !self.accepted.contains(key) || !self.pending.insert(key.to_owned()) { return false; }
        self.queue.push_back(key.to_owned());
        true
    }
    /// Invalidate all accepted resources, for example after an event-stream gap.
    pub fn mark_all(&mut self) {
        let keys: Vec<_> = self.accepted.iter().cloned().collect();
        for key in keys { self.mark(&key); }
    }
    /// Consume a dirty marker, not a message or acknowledgement of work.
    pub fn pop(&mut self) -> Option<String> {
        let key = self.queue.pop_front()?;
        self.pending.remove(&key);
        Some(key)
    }
    /// Number of queued markers.
    pub fn pending_len(&self) -> usize { self.queue.len() }
}
