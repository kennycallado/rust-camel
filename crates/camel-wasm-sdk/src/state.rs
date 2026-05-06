use std::sync::OnceLock;

pub struct GuestState<T> {
    inner: OnceLock<T>,
}

impl<T> GuestState<T> {
    pub const fn new() -> Self {
        Self {
            inner: OnceLock::new(),
        }
    }

    pub fn get_or_init(&self, f: impl FnOnce() -> T) -> &T {
        self.inner.get_or_init(f)
    }

    pub fn get(&self) -> Option<&T> {
        self.inner.get()
    }
}

impl<T> Default for GuestState<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn test_get_or_init_sets_value() {
        static STATE: GuestState<i32> = GuestState::new();
        let val = STATE.get_or_init(|| 42);
        assert_eq!(*val, 42);
    }

    #[test]
    fn test_get_or_init_idempotent() {
        static STATE: GuestState<String> = GuestState::new();
        let ptr1 = STATE.get_or_init(|| "first".to_string()) as *const String;
        let ptr2 = STATE.get_or_init(|| "second".to_string()) as *const String;
        assert_eq!(ptr1, ptr2);
        assert_eq!(*STATE.get().unwrap(), "first");
    }

    #[test]
    fn test_get_returns_none_before_init() {
        static STATE: GuestState<i32> = GuestState::new();
        assert!(STATE.get().is_none());
    }

    #[test]
    fn test_get_returns_some_after_init() {
        static STATE: GuestState<Vec<u8>> = GuestState::new();
        assert!(STATE.get().is_none());
        STATE.get_or_init(|| vec![1, 2, 3]);
        let val = STATE.get().unwrap();
        assert_eq!(val.as_slice(), &[1, 2, 3]);
    }

    #[test]
    fn test_state_generic_with_complex_type() {
        #[derive(Debug, PartialEq)]
        struct Config {
            name: String,
            max_retries: u32,
            endpoints: Vec<String>,
        }

        static STATE: GuestState<Config> = GuestState::new();
        let config = STATE.get_or_init(|| Config {
            name: "my-plugin".to_string(),
            max_retries: 3,
            endpoints: vec!["http://localhost:8080".to_string()],
        });
        assert_eq!(config.name, "my-plugin");
        assert_eq!(config.max_retries, 3);
        assert_eq!(config.endpoints.len(), 1);
    }

    #[test]
    fn test_concurrent_access() {
        static STATE: GuestState<i64> = GuestState::new();
        let handles: Vec<_> = (0..8)
            .map(|i| {
                thread::spawn(move || {
                    let val = STATE.get_or_init(|| 99);
                    assert_eq!(*val, 99);
                    i
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        assert_eq!(*STATE.get().unwrap(), 99);
    }
}
