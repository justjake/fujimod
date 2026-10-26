//! Register words in stable guest programming order.

use std::collections::HashMap;

/// Address or offset -> 32-bit word, iterated in first-insertion order.
/// Equality compares the programmed values independently of their order.
#[derive(Clone, Debug, Default)]
pub struct WordMap {
    order: Vec<u32>,
    values: HashMap<u32, u32>,
}

impl WordMap {
    pub fn get(&self, key: u32) -> Option<u32> {
        self.values.get(&key).copied()
    }
    pub fn get_or_zero(&self, key: u32) -> u32 {
        self.get(key).unwrap_or(0)
    }
    pub fn insert(&mut self, key: u32, value: u32) {
        if self.values.insert(key, value).is_none() {
            self.order.push(key);
        }
    }
    pub fn remove(&mut self, key: u32) -> Option<u32> {
        let value = self.values.remove(&key)?;
        self.order.retain(|k| *k != key);
        Some(value)
    }
    pub fn clear(&mut self) {
        self.order.clear();
        self.values.clear();
    }
    pub fn len(&self) -> usize {
        self.order.len()
    }
    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }
    pub fn items(&self) -> impl Iterator<Item = (u32, u32)> + '_ {
        self.order.iter().map(|k| (*k, self.values[k]))
    }
}

impl PartialEq for WordMap {
    fn eq(&self, other: &Self) -> bool {
        self.values == other.values
    }
}

impl FromIterator<(u32, u32)> for WordMap {
    fn from_iter<I: IntoIterator<Item = (u32, u32)>>(items: I) -> Self {
        let mut map = WordMap::default();
        for (k, v) in items {
            map.insert(k, v);
        }
        map
    }
}

impl serde::Serialize for WordMap {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeSeq;
        let mut values = serializer.serialize_seq(Some(self.len()))?;
        for pair in self.items() {
            values.serialize_element(&pair)?;
        }
        values.end()
    }
}
impl<'de> serde::Deserialize<'de> for WordMap {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let pairs = Vec::<(u32, u32)>::deserialize(deserializer)?;
        let mut result = Self::default();
        for (key, value) in pairs {
            if result.get(key).is_some() {
                return Err(serde::de::Error::custom("duplicate register word"));
            }
            result.insert(key, value);
        }
        Ok(result)
    }
}
#[cfg(test)]
mod serde_tests {
    use super::*;
    #[test]
    fn state_retains_register_order_without_hash_seed_and_refuses_duplicate_words() {
        let mut words: WordMap = [(8, 3), (4, 2)].into_iter().collect();
        words.insert(8, 7);
        let state = serde_json::to_value(&words).unwrap();
        assert_eq!(state, serde_json::json!([[8, 7], [4, 2]]));
        let restored: WordMap = serde_json::from_value(state).unwrap();
        assert_eq!(restored.items().collect::<Vec<_>>(), [(8, 7), (4, 2)]);
        assert!(serde_json::from_value::<WordMap>(serde_json::json!([[8, 7], [8, 9]])).is_err());
    }
}
