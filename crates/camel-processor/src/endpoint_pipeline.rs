use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::Mutex;

use camel_api::{BoxProcessor, CamelError, EndpointPipelineConfig, EndpointResolver};

struct EndpointCache {
    map: HashMap<String, BoxProcessor>,
    order: VecDeque<String>,
}

impl EndpointCache {
    fn new() -> Self {
        Self {
            map: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    fn get(&self, uri: &str) -> Option<BoxProcessor> {
        self.map.get(uri).cloned()
    }

    fn insert(&mut self, uri: String, endpoint: BoxProcessor, capacity: usize) {
        if self.map.contains_key(&uri) {
            return;
        }
        if self.map.len() >= capacity
            && let Some(oldest) = self.order.pop_front()
        {
            self.map.remove(&oldest);
        }
        self.order.push_back(uri.clone());
        self.map.insert(uri, endpoint);
    }
}

#[derive(Clone)]
pub struct EndpointPipelineService {
    resolver: EndpointResolver,
    cache: Arc<Mutex<EndpointCache>>,
    config: EndpointPipelineConfig,
}

impl EndpointPipelineService {
    pub fn new(resolver: EndpointResolver, config: EndpointPipelineConfig) -> Self {
        Self {
            resolver,
            cache: Arc::new(Mutex::new(EndpointCache::new())),
            config,
        }
    }

    pub fn resolve(&self, uri: &str) -> Result<Option<BoxProcessor>, CamelError> {
        {
            let cache_guard = self.cache.lock().unwrap();
            if let Some(endpoint) = cache_guard.get(uri) {
                return Ok(Some(endpoint));
            }
        }

        match (self.resolver)(uri) {
            Some(endpoint) => {
                if self.config.cache_size > 0 {
                    let mut cache_guard = self.cache.lock().unwrap();
                    cache_guard.insert(uri.to_string(), endpoint.clone(), self.config.cache_size);
                }
                Ok(Some(endpoint))
            }
            None => {
                if self.config.ignore_invalid_endpoints {
                    tracing::debug!(uri = uri, "Skipping invalid endpoint");
                    Ok(None)
                } else {
                    Err(CamelError::ProcessorError(format!(
                        "Invalid endpoint: {}",
                        uri
                    )))
                }
            }
        }
    }
}
