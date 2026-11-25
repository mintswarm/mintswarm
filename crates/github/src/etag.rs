//! The ETag cache the field client and the reads share: the last ETag and body of each resource,
//! so a repeated GET is sent with `If-None-Match` and a 304 is answered from the cache. The body
//! is the parsed JSON by default; the reads keep a page's next-page hint beside it.

use std::collections::{BTreeMap, HashMap};
use std::hash::Hash;
use std::sync::Arc;

use serde_json::Value;

/// At most this many resources are kept in a client's ETag cache.
pub(crate) const ETAG_CACHE_LIMIT: usize = 4096;

/// The last ETag and body of each resource, the oldest dropped above a fixed count.
#[derive(Debug)]
pub(crate) struct EtagCache<K, V = Arc<Value>> {
    limit: usize,
    /// Each entry's ETag and body, and when it was stored.
    entries: HashMap<K, (String, V, u64)>,
    /// The keys by when they were stored, oldest first, so eviction does not scan the entries.
    order: BTreeMap<u64, K>,
    stored: u64,
}

impl<K: Eq + Hash + Clone, V: Clone> EtagCache<K, V> {
    pub(crate) fn new(limit: usize) -> Self {
        Self {
            limit,
            entries: HashMap::new(),
            order: BTreeMap::new(),
            stored: 0,
        }
    }

    pub(crate) fn get(&self, key: &K) -> Option<(String, V)> {
        self.entries
            .get(key)
            .map(|(etag, body, _)| (etag.clone(), body.clone()))
    }

    /// Stores `key`'s ETag and body, dropping the oldest entry when the cache is full.
    pub(crate) fn insert(&mut self, key: K, etag: String, body: V) {
        if let Some((_, _, stored)) = self.entries.get(&key) {
            self.order.remove(stored);
        } else if self.entries.len() >= self.limit {
            if let Some((_, oldest)) = self.order.pop_first() {
                self.entries.remove(&oldest);
            }
        }
        self.stored += 1;
        self.order.insert(self.stored, key.clone());
        self.entries.insert(key, (etag, body, self.stored));
    }

    pub(crate) fn remove(&mut self, key: &K) {
        if let Some((_, _, stored)) = self.entries.remove(key) {
            self.order.remove(&stored);
        }
    }

    /// How many entries the cache holds.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::Value;

    use super::EtagCache;

    #[test]
    fn a_removed_or_refreshed_entry_is_not_evicted_in_its_old_place() {
        let mut cache = EtagCache::new(3);
        let body = Arc::new(Value::Null);
        for key in ["a", "b", "c"] {
            cache.insert(key, key.to_owned(), Arc::clone(&body));
        }
        cache.remove(&"a");
        cache.insert("b", "b2".to_owned(), Arc::clone(&body));
        // The cache has room after the removal; then "c" is the oldest.
        cache.insert("d", "d".to_owned(), Arc::clone(&body));
        assert_eq!(cache.len(), 3);
        cache.insert("e", "e".to_owned(), body);
        assert_eq!(cache.len(), 3);
        assert!(cache.get(&"c").is_none());
        for key in ["b", "d", "e"] {
            assert!(cache.get(&key).is_some(), "{key}");
        }
        assert_eq!(cache.order.len(), 3);
    }
}
