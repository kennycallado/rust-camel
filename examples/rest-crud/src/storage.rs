//! In-memory user store backed by DashMap.
//!
//! Phase-2 example storage. Production code would use a real database; here
//! we want CRUD semantics without I/O surprises so the REST DSL wiring is
//! the focus of the demo.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use dashmap::DashMap;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: u64,
    pub name: String,
    pub email: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateUserRequest {
    pub name: String,
    pub email: String,
}

#[derive(Clone)]
pub struct UserStore {
    users: Arc<DashMap<u64, User>>,
    next_id: Arc<AtomicU64>,
}

impl UserStore {
    pub fn new() -> Self {
        let store = Self {
            users: Arc::new(DashMap::new()),
            next_id: Arc::new(AtomicU64::new(1)),
        };
        store.seed("Alice", "alice@example.com");
        store.seed("Bob", "bob@example.com");
        store
    }

    fn seed(&self, name: &str, email: &str) {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        self.users.insert(
            id,
            User {
                id,
                name: name.into(),
                email: email.into(),
            },
        );
    }

    pub fn list(&self) -> Vec<User> {
        let mut out: Vec<User> = self.users.iter().map(|e| e.value().clone()).collect();
        out.sort_by_key(|u| u.id);
        out
    }

    #[allow(dead_code)]
    pub fn get(&self, id: u64) -> Option<User> {
        self.users.get(&id).map(|e| e.value().clone())
    }

    pub fn create(&self, name: String, email: String) -> User {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let user = User { id, name, email };
        self.users.insert(id, user.clone());
        user
    }

    pub fn update(&self, id: u64, name: String, email: String) -> Option<User> {
        if let Some(mut entry) = self.users.get_mut(&id) {
            entry.name = name;
            entry.email = email;
            Some(entry.clone())
        } else {
            None
        }
    }

    pub fn delete(&self, id: u64) -> bool {
        self.users.remove(&id).is_some()
    }
}

impl Default for UserStore {
    fn default() -> Self {
        Self::new()
    }
}
