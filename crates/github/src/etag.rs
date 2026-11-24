//! The ETag cache the field client and the reads share: the last ETag and body of each resource,
//! so a repeated GET is sent with `If-None-Match` and a 304 is answered from the cache.

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::Arc;

use serde_json::Value;

/// At most this many resources are kept in a client's ETag cache.
pub(crate) const ETAG_CACHE_LIMIT: usize = 4096;

/// The last ETag and body of each resource, the oldest dropped above a fixed count.
#[derive(Debug)]
pub(crate) struct EtagCache<K> {
    limit: usize,
    /// Each entry's ETag and body, and when it was stored.
    entries: HashMap<K, (String, Arc<Value>, u64)>,
    stored: u64,
}

impl<K: Eq + Hash + Clone> EtagCache<K> {
    pub(crate) fn new(limit: usize) -> Self {
        Self {
            limit,
            entries: HashMap::new(),
            stored: 0,
        }
    }

    pub(crate) fn get(&self, key: &K) -> Option<(String, Arc<Value>)> {
        self.entries
            .get(key)
            .map(|(etag, body, _)| (etag.clone(), Arc::clone(body)))
    }

    /// Stores `key`'s ETag and body, dropping the oldest entry when the cache is full.
    pub(crate) fn insert(&mut self, key: K, etag: String, body: Arc<Value>) {
        if !self.entries.contains_key(&key) && self.entries.len() >= self.limit {
            let oldest = self
                .entries
                .iter()
                .min_by_key(|(_, (_, _, stored))| *stored)
                .map(|(key, _)| key.clone());
            if let Some(oldest) = oldest {
                self.entries.remove(&oldest);
            }
        }
        self.stored += 1;
        self.entries.insert(key, (etag, body, self.stored));
    }

    pub(crate) fn remove(&mut self, key: &K) {
        self.entries.remove(key);
    }

    /// How many entries the cache holds.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }
}
