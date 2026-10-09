//! A small insertion-ordered cache with entry, byte and TTL bounds.
//! Reads refresh recency.

use indexmap::IndexMap;

struct Entry<V> {
    value: V,
    bytes: usize,
    expires_at: i64,
}

pub struct BoundedReadCache<V> {
    entries: IndexMap<String, Entry<V>>,
    bytes: usize,
    max_entries: usize,
    max_bytes: usize,
    ttl_ms: Option<i64>,
}

impl<V: Clone> BoundedReadCache<V> {
    /// `ttl_ms = None` never expires.
    pub fn new(max_entries: usize, max_bytes: usize, ttl_ms: Option<i64>) -> Self {
        Self {
            entries: IndexMap::new(),
            bytes: 0,
            max_entries,
            max_bytes,
            ttl_ms,
        }
    }

    pub fn get(&mut self, key: &str, now: i64) -> Option<V> {
        let expired = self.entries.get(key)?.expires_at <= now;
        if expired {
            self.remove(key);
            return None;
        }
        let (k, entry) = self.entries.shift_remove_entry(key)?;
        let value = entry.value.clone();
        self.entries.insert(k, entry);
        Some(value)
    }

    pub fn set(&mut self, key: &str, value: V, bytes: usize, now: i64) {
        self.remove(key);
        if bytes > self.max_bytes || self.max_entries < 1 {
            return;
        }
        while self.entries.len() >= self.max_entries || self.bytes + bytes > self.max_bytes {
            let Some(oldest) = self.entries.keys().next().cloned() else {
                break;
            };
            self.remove(&oldest);
        }
        let expires_at = self.ttl_ms.map_or(i64::MAX, |ttl| now.saturating_add(ttl));
        self.entries.insert(
            key.to_owned(),
            Entry {
                value,
                bytes,
                expires_at,
            },
        );
        self.bytes += bytes;
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
    }

    pub fn delete(&mut self, key: &str) {
        self.remove(key);
    }

    fn remove(&mut self, key: &str) {
        if let Some(entry) = self.entries.shift_remove(key) {
            self.bytes -= entry.bytes;
        }
    }
}

#[cfg(test)]
mod read_cache_spec {
    use super::*;

    #[test]
    fn expires_entries_and_refreshes_recency_when_enforcing_the_count_bound() {
        let mut cache = BoundedReadCache::new(2, 20, Some(30));
        cache.set("a", "A", 1, 0);
        cache.set("b", "B", 1, 0);
        assert_eq!(cache.get("a", 1), Some("A"));
        cache.set("c", "C", 1, 2);
        assert_eq!(cache.get("b", 2), None);
        assert_eq!(cache.get("a", 2), Some("A"));
        assert_eq!(cache.get("a", 31), None);
    }

    #[test]
    fn bounds_bytes_and_clears_retained_values() {
        let mut cache = BoundedReadCache::new(5, 3, None);
        cache.set("too-large", "x", 4, 0);
        assert_eq!(cache.get("too-large", 0), None);
        cache.set("a", "a", 2, 0);
        cache.set("b", "b", 2, 0);
        assert_eq!(cache.get("a", 0), None);
        assert_eq!(cache.get("b", 0), Some("b"));
        cache.clear();
        assert_eq!(cache.get("b", 0), None);
    }
}
