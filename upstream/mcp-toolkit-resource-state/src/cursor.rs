//! Opaque, scope-bound, expiring cursor payloads. Not an approval-token store.
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// All bindings are required; an HTTP connection identifier is not a principal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorScope {
    pub principal: String,
    pub resource: String,
    pub generation: String,
}
impl CursorScope {
    fn valid(&self) -> bool {
        [&self.principal, &self.resource, &self.generation]
            .into_iter()
            .all(|v| !v.is_empty() && v.trim() == v && v.len() <= 2048)
    }
}
/// Resolve failures deliberately collapse missing, expired and foreign tokens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorError {
    InvalidConfiguration,
    InvalidCursor,
    InvalidInput,
    Capacity,
}
#[derive(Debug)]
struct Entry {
    scope: CursorScope,
    payload: Vec<u8>,
    expires: Instant,
    cost: usize,
}
/// Bounded in-memory bytes. Full capacity rejects insertion rather than evicting live cursors.
#[derive(Debug)]
pub struct CursorStore {
    ttl: Duration,
    max_entries: usize,
    max_bytes: usize,
    bytes: usize,
    entries: BTreeMap<String, Entry>,
}
impl CursorStore {
    /// Errors on zero limits. Payload lifetime is never extended by a read.
    pub fn new(ttl: Duration, max_entries: usize, max_bytes: usize) -> Result<Self, CursorError> {
        if ttl.is_zero() || max_entries == 0 || max_bytes == 0 {
            return Err(CursorError::InvalidConfiguration);
        }
        Ok(Self {
            ttl,
            max_entries,
            max_bytes,
            bytes: 0,
            entries: BTreeMap::new(),
        })
    }
    fn sweep(&mut self, now: Instant) {
        self.entries.retain(|_, v| v.expires > now);
        self.bytes = self.entries.values().map(|e| e.cost).sum();
    }
    /// Insert a caller-generated unpredictable token. No silent overwrites.
    /// Errors on malformed scopes, duplicate tokens, expiry overflow or exhausted budgets.
    pub fn insert(
        &mut self,
        token: String,
        scope: CursorScope,
        payload: Vec<u8>,
        now: Instant,
    ) -> Result<(), CursorError> {
        self.sweep(now);
        if !scope.valid()
            || token.len() < 32
            || token.len() > 128
            || !token
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            || self.entries.contains_key(&token)
        {
            return Err(CursorError::InvalidInput);
        }
        let expires = now.checked_add(self.ttl).ok_or(CursorError::InvalidInput)?;
        let cost = token
            .len()
            .saturating_add(scope.principal.len())
            .saturating_add(scope.resource.len())
            .saturating_add(scope.generation.len())
            .saturating_add(payload.len());
        if self.entries.len() >= self.max_entries
            || cost > self.max_bytes.saturating_sub(self.bytes)
        {
            return Err(CursorError::Capacity);
        }
        self.entries.insert(
            token,
            Entry {
                scope,
                payload,
                expires,
                cost,
            },
        );
        self.bytes += cost;
        Ok(())
    }
    /// Returns a copy only for an exact live scope match. No token-existence oracle.
    pub fn resolve(
        &mut self,
        token: &str,
        scope: &CursorScope,
        now: Instant,
    ) -> Result<Vec<u8>, CursorError> {
        self.sweep(now);
        if token.len() > 128 {
            return Err(CursorError::InvalidCursor);
        }
        self.entries
            .get(token)
            .filter(|e| &e.scope == scope)
            .map(|e| e.payload.clone())
            .ok_or(CursorError::InvalidCursor)
    }
    /// Discard every cursor when the backend epoch changes.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
    }
}
