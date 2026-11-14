//! Cache mémoire LRU, borné en octets, avec durée de vie des entrées.
//!
//! Volontairement simple : une table clé → entrée et un index ordonné par
//! « date » d'usage (compteur) pour trouver la moins récemment utilisée.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};

struct Entry<V> {
    value: V,
    size: usize,
    tick: u64,
    born: Instant,
}

pub struct Lru<V> {
    map: HashMap<String, Entry<V>>,
    order: BTreeMap<u64, String>,
    tick: u64,
    bytes: usize,
    max_bytes: usize,
    ttl: Duration,
}

impl<V: Clone> Lru<V> {
    /// `max_bytes` = 0 : cache désactivé.
    pub fn new(max_bytes: usize, ttl: Duration) -> Self {
        Self { map: HashMap::new(), order: BTreeMap::new(), tick: 0, bytes: 0, max_bytes, ttl }
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.map.len()
    }

    #[cfg(test)]
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    pub fn get(&mut self, key: &str) -> Option<V> {
        let expired = self.map.get(key)?.born.elapsed() > self.ttl;
        if expired {
            self.remove(key);
            return None;
        }
        self.tick += 1;
        let tick = self.tick;
        let e = self.map.get_mut(key)?;
        self.order.remove(&e.tick);
        e.tick = tick;
        self.order.insert(tick, key.to_string());
        Some(e.value.clone())
    }

    /// Ajoute (ou remplace) une entrée de `size` octets ; les moins
    /// récemment utilisées sont évincées. Une entrée de plus du quart du
    /// cache n'est pas gardée (elle viderait tout le reste).
    pub fn put(&mut self, key: String, value: V, size: usize) {
        self.remove(&key);
        if size > self.max_bytes / 4 {
            return;
        }
        self.tick += 1;
        self.order.insert(self.tick, key.clone());
        self.map.insert(key, Entry { value, size, tick: self.tick, born: Instant::now() });
        self.bytes += size;
        while self.bytes > self.max_bytes {
            let Some((_, oldest)) = self.order.pop_first() else { break };
            if let Some(e) = self.map.remove(&oldest) {
                self.bytes -= e.size;
            }
        }
    }

    fn remove(&mut self, key: &str) {
        if let Some(e) = self.map.remove(key) {
            self.order.remove(&e.tick);
            self.bytes -= e.size;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eviction_du_moins_recemment_utilise() {
        let mut c = Lru::new(100, Duration::from_secs(60));
        c.put("a".into(), 1, 20);
        c.put("b".into(), 2, 20);
        c.put("c".into(), 3, 20);
        assert_eq!(c.get("a"), Some(1), "« a » redevient le plus récent");
        c.put("d".into(), 4, 25);
        c.put("e".into(), 5, 25); // 110 octets : « b » (le plus ancien) part
        assert_eq!(c.get("b"), None);
        assert_eq!(c.get("a"), Some(1));
        assert_eq!(c.get("e"), Some(5));
        assert!(c.bytes() <= 100);
        c.put("a".into(), 10, 5);
        assert_eq!(c.get("a"), Some(10), "remplacement");
        assert_eq!(c.len(), 4);
        c.put("gros".into(), 0, 26);
        assert_eq!(c.get("gros"), None, "plus du quart du cache : pas gardé");
        let mut off = Lru::new(0, Duration::from_secs(60));
        off.put("x".into(), 1, 1);
        assert_eq!(off.get("x"), None, "taille 0 = pas de cache");
    }

    #[test]
    fn expiration() {
        let mut c = Lru::new(100, Duration::from_millis(30));
        c.put("a".into(), 1, 10);
        assert_eq!(c.get("a"), Some(1));
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(c.get("a"), None);
        assert_eq!(c.bytes(), 0);
    }
}
