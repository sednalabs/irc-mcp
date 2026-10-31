//! Bounded invalidations and one atomic live observation/inbox generation.
//!
//! No transport await occurs under the state lock or during fan-out. Slow listeners
//! hold only bounded dirty URI markers, not independent copies of message history.
use crate::{
    attention::AttentionConfig,
    config::Conversation,
    domain::{Failure, RelayLine, Result},
    inbox::{Inbox, Page, INBOX_URI},
};
use mcp_toolkit_resource_state::InvalidationSet;
use std::{
    sync::{Arc, Mutex, Weak},
    time::Instant,
};
use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore};

pub struct Pending {
    queue: Mutex<InvalidationSet>,
    changed: Notify,
    _permit: OwnedSemaphorePermit,
}
impl Pending {
    fn mark(&self, uri: &str) -> Result<()> {
        if self.queue.lock().map_err(|_| Failure::backend())?.mark(uri) {
            self.changed.notify_one();
        }
        Ok(())
    }
    fn all(&self) -> Result<()> {
        self.queue
            .lock()
            .map_err(|_| Failure::backend())?
            .mark_all();
        self.changed.notify_one();
        Ok(())
    }
    /// One consumer per subscription. Dropping this future does not acknowledge chat.
    pub async fn next(&self) -> Result<String> {
        loop {
            let changed = self.changed.notified();
            let item = self.queue.lock().map_err(|_| Failure::backend())?.pop();
            if let Some(uri) = item {
                return Ok(uri);
            }
            changed.await;
        }
    }
}
#[derive(Clone)]
pub struct Hub {
    inner: Arc<Inner>,
}
struct Inner {
    listeners: Mutex<Vec<Weak<Pending>>>,
    permits: Arc<Semaphore>,
    state: Mutex<State>,
}
struct State {
    observation: Observation,
    inbox: Inbox,
}
#[derive(Clone)]
pub struct Observation {
    pub generation: String,
    pub relay_events_connected: bool,
}
impl Default for Hub {
    fn default() -> Self {
        Self::new()
    }
}
impl Hub {
    pub fn new() -> Self {
        Self::with_attention(AttentionConfig::default())
    }
    pub fn with_attention(policy: AttentionConfig) -> Self {
        let generation = uuid::Uuid::new_v4().to_string();
        let state = State {
            inbox: Inbox::new(policy, generation.clone()),
            observation: Observation {
                generation,
                relay_events_connected: false,
            },
        };
        Self {
            inner: Arc::new(Inner {
                listeners: Mutex::new(Vec::new()),
                permits: Arc::new(Semaphore::new(32)),
                state: Mutex::new(state),
            }),
        }
    }
    pub fn observation(&self) -> Result<Observation> {
        Ok(self
            .inner
            .state
            .lock()
            .map_err(|_| Failure::backend())?
            .observation
            .clone())
    }
    /// Changes observation and inbox epochs atomically, before notifying listeners.
    pub fn boundary(&self, connected: bool) -> Result<()> {
        {
            let mut state = self.inner.state.lock().map_err(|_| Failure::backend())?;
            let generation = uuid::Uuid::new_v4().to_string();
            state.inbox.reset(generation.clone());
            state.observation = Observation {
                generation,
                relay_events_connected: connected,
            };
        }
        for sink in self.sinks()? {
            sink.all()?;
        }
        Ok(())
    }
    pub fn record(
        &self,
        c: &Conversation,
        own_nick: &str,
        buffer_id: u64,
        line: &RelayLine,
        edited: bool,
    ) -> Result<()> {
        let changed = {
            let mut state = self.inner.state.lock().map_err(|_| Failure::backend())?;
            if !state.observation.relay_events_connected {
                return Ok(());
            }
            state
                .inbox
                .record(c, own_nick, buffer_id, line, edited, Instant::now())?
        };
        if changed {
            self.updated(INBOX_URI)?;
        }
        Ok(())
    }
    pub fn prune_inbox(&self) -> Result<()> {
        let changed = self
            .inner
            .state
            .lock()
            .map_err(|_| Failure::backend())?
            .inbox
            .prune(Instant::now());
        if changed {
            self.updated(INBOX_URI)?;
        }
        Ok(())
    }
    pub fn inbox_page(&self, generation: &str, after: Option<u64>, limit: usize) -> Result<Page> {
        let mut state = self.inner.state.lock().map_err(|_| Failure::backend())?;
        if !state.observation.relay_events_connected || state.observation.generation != generation {
            return Err(Failure::new(
                "inbox_gap",
                "The live observation generation changed. Start a new inbox read.",
            ));
        }
        state.inbox.page(after, limit, Instant::now())
    }
    pub fn register(&self, uris: Vec<String>) -> Result<Arc<Pending>> {
        let permit = self
            .inner
            .permits
            .clone()
            .try_acquire_owned()
            .map_err(|_| Failure::new("subscription_limit", "Too many active subscriptions."))?;
        let queue = InvalidationSet::new(uris, 34, 256)
            .map_err(|_| Failure::new("subscription_filter", "Invalid resource subscription."))?;
        let sink = Arc::new(Pending {
            queue: Mutex::new(queue),
            changed: Notify::new(),
            _permit: permit,
        });
        {
            let mut listeners = self
                .inner
                .listeners
                .lock()
                .map_err(|_| Failure::backend())?;
            listeners.retain(|w| w.strong_count() > 0);
            listeners.push(Arc::downgrade(&sink));
        }
        // Register before initial invalidation; covers the protocol-ack/readiness gap.
        sink.all()?;
        Ok(sink)
    }
    fn sinks(&self) -> Result<Vec<Arc<Pending>>> {
        let mut listeners = self
            .inner
            .listeners
            .lock()
            .map_err(|_| Failure::backend())?;
        listeners.retain(|w| w.strong_count() > 0);
        Ok(listeners.iter().filter_map(Weak::upgrade).collect())
    }
    pub fn updated(&self, uri: &str) -> Result<()> {
        for sink in self.sinks()? {
            sink.mark(uri)?;
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    #[tokio::test]
    async fn filtered_and_coalesced() {
        let hub = Hub::new();
        let sink = hub.register(vec!["a".into()]).unwrap();
        assert_eq!(sink.next().await.unwrap(), "a");
        hub.updated("b").unwrap();
        hub.updated("a").unwrap();
        hub.updated("a").unwrap();
        assert_eq!(sink.next().await.unwrap(), "a");
        assert!(tokio::time::timeout(Duration::from_millis(5), sink.next())
            .await
            .is_err());
    }
    #[tokio::test]
    async fn bounded_and_released_on_drop() {
        let hub = Hub::new();
        let sinks: Vec<_> = (0..32)
            .map(|_| hub.register(vec!["a".into()]).unwrap())
            .collect();
        assert!(hub.register(vec!["a".into()]).is_err());
        drop(sinks);
        assert!(hub.register(vec!["a".into()]).is_ok());
    }
    #[test]
    fn discontinuity_changes_epoch_and_rejects_old_inbox_view() {
        let h = Hub::new();
        h.boundary(true).unwrap();
        let old = h.observation().unwrap().generation;
        h.boundary(true).unwrap();
        assert!(h.inbox_page(&old, None, 20).is_err());
    }
    #[tokio::test]
    async fn updates_during_initial_delivery_and_slow_consumer_are_retained() {
        let hub = Hub::new();
        let slow = hub.register(vec!["a".into(), "b".into()]).unwrap();
        let fast = hub.register(vec!["a".into(), "b".into()]).unwrap();
        assert_eq!(fast.next().await.unwrap(), "a");
        for _ in 0..1000 {
            hub.updated("a").unwrap();
        }
        assert_eq!(fast.next().await.unwrap(), "b");
        assert_eq!(fast.next().await.unwrap(), "a");
        assert_eq!(slow.queue.lock().unwrap().pending_len(), 2);
        assert_eq!(slow.next().await.unwrap(), "a");
        assert_eq!(slow.next().await.unwrap(), "b");
    }
    #[tokio::test]
    async fn update_before_listener_registration_is_covered_by_initial_marker() {
        let hub = Hub::new();
        hub.updated("a").unwrap();
        let sink = hub.register(vec!["a".into()]).unwrap();
        assert_eq!(sink.next().await.unwrap(), "a");
        assert!(tokio::time::timeout(Duration::from_millis(5), sink.next())
            .await
            .is_err());
        hub.updated("a").unwrap();
        assert_eq!(sink.next().await.unwrap(), "a");
    }
}
