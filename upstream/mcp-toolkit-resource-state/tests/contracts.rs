use mcp_toolkit_resource_state::{CursorError, CursorScope, CursorStore, InvalidationSet};
use std::time::{Duration, Instant};
fn scope() -> CursorScope { CursorScope { principal:"alice".into(),resource:"resource://one".into(),generation:"epoch1".into() } }
fn token(c: char) -> String { c.to_string().repeat(32) }
#[test]
fn coalesces_without_accepting_foreign_keys() {
    let mut q=InvalidationSet::new(vec!["a".into(),"b".into()],2,10).unwrap();
    assert!(q.mark("b")); assert!(!q.mark("b")); assert!(!q.mark("foreign"));
    assert!(q.mark("a")); assert_eq!(q.pending_len(),2);
    assert_eq!((q.pop(),q.pop(),q.pop()),(Some("b".into()),Some("a".into()),None));
}
#[test]
fn hot_key_does_not_starve_other_key() {
    let mut q=InvalidationSet::new(vec!["a".into(),"b".into()],2,10).unwrap();
    q.mark_all(); assert_eq!(q.pop(),Some("a".into())); q.mark("a");
    assert_eq!(q.pop(),Some("b".into()));
}
#[test]
fn invalid_inventories_are_rejected() {
    assert!(InvalidationSet::new(vec!["a".into(),"a".into()],2,10).is_err());
    assert!(InvalidationSet::new(vec![" a".into()],1,10).is_err());
    assert!(InvalidationSet::new(vec![],1,10).is_err());
}
#[test]
fn exact_scope_and_expiration_no_ttl_extension() {
    let now=Instant::now(); let mut s=CursorStore::new(Duration::from_secs(10),2,1000).unwrap();
    s.insert(token('a'),scope(),vec![1,2],now).unwrap();
    for foreign in [CursorScope{principal:"bob".into(),..scope()},CursorScope{resource:"other".into(),..scope()},CursorScope{generation:"epoch2".into(),..scope()}] {
        assert_eq!(s.resolve(&token('a'),&foreign,now),Err(CursorError::InvalidCursor));
    }
    assert_eq!(s.resolve(&token('a'),&scope(),now+Duration::from_secs(9)),Ok(vec![1,2]));
    assert_eq!(s.resolve(&token('a'),&scope(),now+Duration::from_secs(10)),Err(CursorError::InvalidCursor));
}
#[test]
fn storage_budget_includes_bindings_and_rejects_overwrites() {
    let now=Instant::now(); let mut s=CursorStore::new(Duration::from_secs(1),1,200).unwrap();
    s.insert(token('a'),scope(),vec![1],now).unwrap();
    assert_eq!(s.insert(token('a'),scope(),vec![2],now),Err(CursorError::InvalidInput));
    assert_eq!(s.insert(token('b'),scope(),vec![2],now),Err(CursorError::Capacity));
    s.insert(token('b'),scope(),vec![2],now+Duration::from_secs(1)).unwrap();
    s.clear(); assert_eq!(s.resolve(&token('b'),&scope(),now),Err(CursorError::InvalidCursor));
}
#[test]
fn invalid_and_oversized_cursor_input_is_rejected() {
    let now=Instant::now(); let mut s=CursorStore::new(Duration::from_secs(1),2,100).unwrap();
    assert_eq!(s.insert("short".into(),scope(),vec![],now),Err(CursorError::InvalidInput));
    assert_eq!(s.insert(token('a'),scope(),vec![0;100],now),Err(CursorError::Capacity));
}

#[test]
fn update_after_pop_is_a_new_invalidation_not_lost() {
    let mut q=InvalidationSet::new(vec!["a".into()],1,10).unwrap();
    q.mark("a");assert_eq!(q.pop(),Some("a".into()));
    assert!(q.mark("a"));assert_eq!(q.pop(),Some("a".into()));
}
#[test]
fn repeated_initial_mark_does_not_grow_queue_or_reorder_existing_work() {
    let mut q=InvalidationSet::new(vec!["a".into(),"b".into()],2,10).unwrap();
    q.mark("b");for _ in 0..1000 {q.mark_all();}
    assert_eq!(q.pending_len(),2);assert_eq!(q.pop(),Some("b".into()));assert_eq!(q.pop(),Some("a".into()));
}
#[test]
fn abandoning_a_cursor_read_neither_consumes_nor_extends_it() {
    let now=Instant::now();let mut s=CursorStore::new(Duration::from_secs(10),2,1000).unwrap();
    s.insert(token('a'),scope(),vec![1,2,3],now).unwrap();
    let abandoned=s.resolve(&token('a'),&scope(),now).unwrap();drop(abandoned);
    assert_eq!(s.resolve(&token('a'),&scope(),now+Duration::from_secs(9)),Ok(vec![1,2,3]));
    assert_eq!(s.resolve(&token('a'),&scope(),now+Duration::from_secs(10)),Err(CursorError::InvalidCursor));
}
