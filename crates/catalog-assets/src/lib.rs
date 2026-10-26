//! Catalog envelopes for processor, body and firmware facts, and the file provider
//! that loads them and their assets from caller-selected locations.
//!
//! Documents contain facts and content identities. Input locations belong to
//! a caller-selected provider. Consumers deserialize facts into their checked
//! schemas before constructing an execution graph or a hardware model.

pub mod files;

use fujimod_catalog_identity::{validate_content_sha256, validate_id};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Content {
    pub size: u64,
    pub sha256: String,
}
impl Content {
    pub fn identify(bytes: &[u8]) -> Self {
        Self {
            size: bytes.len() as u64,
            sha256: format!("{:x}", Sha256::digest(bytes)),
        }
    }
    pub fn validate(&self) -> Result<(), String> {
        validate_content_sha256(&self.sha256)
    }
    pub fn check(&self, bytes: &[u8]) -> Result<(), String> {
        self.validate()?;
        if self.size != bytes.len() as u64 || self.sha256 != format!("{:x}", Sha256::digest(bytes))
        {
            return Err("content size or SHA-256 differs".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Asset {
    pub id: String,
    pub content: Content,
}
impl Asset {
    pub fn validate(&self) -> Result<(), String> {
        validate_id(&self.id)?;
        self.content.validate()
    }
    pub fn check(&self, bytes: &[u8]) -> Result<(), String> {
        self.validate()?;
        self.content
            .check(bytes)
            .map_err(|error| format!("asset {}: {error}", self.id))
    }
    pub fn load(&self, provider: &dyn Provider) -> Result<Vec<u8>, String> {
        self.validate()?;
        let bytes = provider.asset(self)?;
        self.check(&bytes)?;
        Ok(bytes)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    Processor,
    Body,
    Firmware,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Key {
    pub kind: Kind,
    pub id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reference {
    pub key: Key,
    pub content: Content,
}
impl Reference {
    pub fn validate(&self) -> Result<(), String> {
        validate_id(&self.key.id)?;
        if self.content.size == 0 {
            return Err("catalog document cannot be empty".into());
        }
        self.content.validate()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Document<T> {
    pub schema: u32,
    pub key: Key,
    pub references: Vec<Reference>,
    pub assets: Vec<Asset>,
    pub facts: T,
}
impl<T> Document<T> {
    fn validate(&self) -> Result<(), String> {
        if self.schema != 1 {
            return Err("unsupported catalog document schema".into());
        }
        validate_id(&self.key.id)?;
        let mut references = BTreeSet::new();
        for reference in &self.references {
            reference.validate()?;
            if !references.insert(&reference.key) {
                return Err("duplicate catalog document reference".into());
            }
        }
        let mut assets = BTreeSet::new();
        for asset in &self.assets {
            asset.validate()?;
            if !assets.insert(&asset.id) {
                return Err("duplicate catalog asset identity".into());
            }
        }
        Ok(())
    }
}

/// Locations and transport are supplied by the host. The catalog checks every
/// returned byte string, including providers that cache or fetch their inputs.
pub trait Provider {
    fn document(&self, reference: &Reference) -> Result<Vec<u8>, String>;
    fn asset(&self, asset: &Asset) -> Result<Vec<u8>, String>;
}

pub struct Catalog {
    documents: BTreeMap<Key, Document<serde_json::Value>>,
    references: BTreeMap<Key, Content>,
    assets: BTreeMap<String, Asset>,
}
impl Catalog {
    pub fn resolve(root: &Reference, provider: &dyn Provider) -> Result<Self, String> {
        let mut catalog = Self {
            documents: BTreeMap::new(),
            references: BTreeMap::new(),
            assets: BTreeMap::new(),
        };
        catalog.visit(root, provider, &mut BTreeSet::new())?;
        Ok(catalog)
    }
    fn visit(
        &mut self,
        reference: &Reference,
        provider: &dyn Provider,
        active: &mut BTreeSet<Key>,
    ) -> Result<(), String> {
        reference.validate()?;
        if active.contains(&reference.key) {
            return Err("cyclic catalog document reference".into());
        }
        if let Some(content) = self.references.get(&reference.key) {
            return if *content == reference.content {
                Ok(())
            } else {
                Err("catalog identity has conflicting content".into())
            };
        }
        if reference.content.size > 4 << 20 || active.len() >= 64 || self.references.len() >= 256 {
            return Err("catalog document count, size or reference depth exceeds its bound".into());
        }
        let bytes = provider.document(reference)?;
        reference.content.check(&bytes)?;
        let document: Document<serde_json::Value> = serde_json::from_slice(&bytes)
            .map_err(|error| format!("invalid catalog document: {error}"))?;
        document.validate()?;
        if document.key != reference.key {
            return Err("catalog document identity differs from its reference".into());
        }
        for asset in &document.assets {
            if self
                .assets
                .get(&asset.id)
                .is_some_and(|known| *known != *asset)
            {
                return Err("catalog asset identity has conflicting content".into());
            }
            self.assets.insert(asset.id.clone(), asset.clone());
        }
        self.references
            .insert(reference.key.clone(), reference.content.clone());
        active.insert(reference.key.clone());
        for child in &document.references {
            self.visit(child, provider, active)?;
        }
        active.remove(&reference.key);
        self.documents.insert(reference.key.clone(), document);
        Ok(())
    }
    pub fn facts<T: DeserializeOwned>(&self, key: &Key) -> Result<T, String> {
        let document = self
            .documents
            .get(key)
            .ok_or("catalog document is missing")?;
        serde_json::from_value(document.facts.clone())
            .map_err(|error| format!("invalid consumed catalog facts: {error}"))
    }
    pub fn asset(&self, id: &str, provider: &dyn Provider) -> Result<Vec<u8>, String> {
        let asset = self.assets.get(id).ok_or("catalog asset is missing")?;
        asset.load(provider)
    }
}

#[cfg(test)]
mod tests;
