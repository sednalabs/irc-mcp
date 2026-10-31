//! Bounded application state; no transport, authentication, storage or domain policy.
//!
//! Callers supply already-authorised resource names, a monotonic clock and random
//! cursor tokens. This module never grants access and does not deliver messages.
#![forbid(unsafe_code)]

mod cursor;
mod invalidation;
pub use cursor::{CursorError, CursorScope, CursorStore};
pub use invalidation::{InvalidationError, InvalidationSet};
