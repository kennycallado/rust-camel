use std::sync::Arc;
use std::thread;

use camel_component_wasm::StateStore;

#[test]
fn store_and_load_roundtrip() {
    let store = StateStore::new();
    store.store("key1", "value1").unwrap();
    let loaded = store.load("key1").unwrap();
    assert_eq!(loaded, Some("value1".to_string()));
}

#[test]
fn store_overwrites_previous_value() {
    let store = StateStore::new();
    store.store("key", "v1").unwrap();
    store.store("key", "v2").unwrap();
    assert_eq!(store.load("key").unwrap(), Some("v2".to_string()));
}

#[test]
fn different_stores_are_isolated() {
    let s1 = StateStore::new();
    let s2 = StateStore::new();
    s1.store("k", "v1").unwrap();
    s2.store("k", "v2").unwrap();
    assert_eq!(s1.load("k").unwrap(), Some("v1".to_string()));
    assert_eq!(s2.load("k").unwrap(), Some("v2".to_string()));
}

#[test]
fn cloned_stores_share_data() {
    let s1 = StateStore::new();
    s1.store("shared", "data").unwrap();
    let s2 = s1.clone();
    assert_eq!(s2.load("shared").unwrap(), Some("data".to_string()));
    s2.store("shared", "modified").unwrap();
    assert_eq!(s1.load("shared").unwrap(), Some("modified".to_string()));
}

#[test]
fn load_missing_key_returns_none() {
    let store = StateStore::new();
    assert_eq!(store.load("nonexistent").unwrap(), None);
}

#[test]
fn concurrent_access() {
    let store = Arc::new(StateStore::new());
    let mut handles = vec![];

    for i in 0..10 {
        let s = Arc::clone(&store);
        handles.push(thread::spawn(move || {
            let key = format!("key-{}", i);
            let value = format!("value-{}", i);
            s.store(&key, &value).unwrap();
            assert_eq!(s.load(&key).unwrap(), Some(value));
        }));
    }

    for h in handles {
        h.join().unwrap();
    }

    assert_eq!(store.len(), 10);
}
