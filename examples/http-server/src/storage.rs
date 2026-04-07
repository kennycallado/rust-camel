use dashmap::DashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::models::{CreateUserRequest, UpdateUserRequest, User, UserListResponse};

#[derive(Clone)]
pub struct UserStorage {
    users: Arc<DashMap<u64, User>>,
    next_id: Arc<AtomicU64>,
}

impl UserStorage {
    pub fn new() -> Self {
        Self {
            users: Arc::new(DashMap::new()),
            next_id: Arc::new(AtomicU64::new(1)),
        }
    }

    pub fn create(&self, req: CreateUserRequest) -> User {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let created_at = current_timestamp();

        let user = User {
            id,
            name: req.name.trim().to_string(),
            email: req.email.trim().to_string(),
            age: req.age,
            role: req.role,
            created_at,
            updated_at: None,
        };

        self.users.insert(id, user.clone());
        user
    }

    pub fn get(&self, id: u64) -> Option<User> {
        self.users.get(&id).map(|entry| entry.value().clone())
    }

    pub fn update(&self, id: u64, req: UpdateUserRequest) -> Option<User> {
        self.users.get_mut(&id).map(|mut entry| {
            let user = entry.value_mut();

            if let Some(name) = req.name {
                user.name = name.trim().to_string();
            }
            if let Some(email) = req.email {
                user.email = email.trim().to_string();
            }
            if let Some(age) = req.age {
                user.age = Some(age);
            }
            if let Some(role) = req.role {
                user.role = role;
            }

            user.updated_at = Some(current_timestamp());
            user.clone()
        })
    }

    pub fn delete(&self, id: u64) -> Option<User> {
        self.users.remove(&id).map(|(_, user)| user)
    }

    pub fn list(&self, page: u32, per_page: u32) -> UserListResponse {
        let all_users: Vec<User> = self
            .users
            .iter()
            .map(|entry| entry.value().clone())
            .collect();

        let total = all_users.len();
        let skip = ((page.saturating_sub(1)) * per_page) as usize;
        let users: Vec<User> = all_users
            .into_iter()
            .skip(skip)
            .take(per_page as usize)
            .collect();

        UserListResponse {
            users,
            total,
            page,
            per_page,
        }
    }

    pub fn email_exists(&self, email: &str) -> bool {
        self.users.iter().any(|entry| entry.value().email == email)
    }

    pub fn count(&self) -> usize {
        self.users.len()
    }
}

fn current_timestamp() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_and_get_user() {
        let storage = UserStorage::new();
        let req = CreateUserRequest {
            name: "Alice".to_string(),
            email: "alice@example.com".to_string(),
            age: Some(30),
            role: "user".to_string(),
        };

        let created = storage.create(req);
        assert_eq!(created.id, 1);
        assert_eq!(created.name, "Alice");

        let fetched = storage.get(1).unwrap();
        assert_eq!(fetched.email, "alice@example.com");
    }

    #[test]
    fn test_update_user() {
        let storage = UserStorage::new();
        let req = CreateUserRequest {
            name: "Bob".to_string(),
            email: "bob@example.com".to_string(),
            age: None,
            role: "user".to_string(),
        };
        storage.create(req);

        let update_req = UpdateUserRequest {
            name: Some("Robert".to_string()),
            email: None,
            age: Some(25),
            role: None,
        };

        let updated = storage.update(1, update_req).unwrap();
        assert_eq!(updated.name, "Robert");
        assert_eq!(updated.age, Some(25));
        assert!(updated.updated_at.is_some());
    }

    #[test]
    fn test_delete_user() {
        let storage = UserStorage::new();
        let req = CreateUserRequest {
            name: "Charlie".to_string(),
            email: "charlie@example.com".to_string(),
            age: None,
            role: "user".to_string(),
        };
        storage.create(req);

        let deleted = storage.delete(1).unwrap();
        assert_eq!(deleted.name, "Charlie");

        assert!(storage.get(1).is_none());
    }

    #[test]
    fn test_list_users_pagination() {
        let storage = UserStorage::new();

        for i in 1..=5 {
            let req = CreateUserRequest {
                name: format!("User{}", i),
                email: format!("user{}@example.com", i),
                age: None,
                role: "user".to_string(),
            };
            storage.create(req);
        }

        let page1 = storage.list(1, 2);
        assert_eq!(page1.users.len(), 2);
        assert_eq!(page1.total, 5);
        assert_eq!(page1.page, 1);

        let page3 = storage.list(3, 2);
        assert_eq!(page3.users.len(), 1);
    }

    #[test]
    fn test_email_exists() {
        let storage = UserStorage::new();
        let req = CreateUserRequest {
            name: "Test".to_string(),
            email: "unique@example.com".to_string(),
            age: None,
            role: "user".to_string(),
        };
        storage.create(req);

        assert!(storage.email_exists("unique@example.com"));
        assert!(!storage.email_exists("other@example.com"));
    }
}
