//! Committed external storage across cold reconstruction; no CPU/device context.
use super::Session;
use fujimod_emulator_devices::parts::{nand, sd_card::Card};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Spec {
    pub cards: Vec<String>,
    pub nands: Vec<String>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CardMedium {
    card: Card,
    present: bool,
    protected: bool,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Media {
    cards: BTreeMap<String, CardMedium>,
    nands: BTreeMap<String, nand::Changes>,
}
impl Media {
    pub fn capture(session: &Session, spec: &Spec) -> Result<Self, String> {
        let cards = spec
            .cards
            .iter()
            .map(|name| {
                let contacts = session
                    .contacts
                    .get(name)
                    .ok_or("durable SD contacts are missing")?;
                Ok((
                    name.clone(),
                    CardMedium {
                        card: session.card_medium(name)?,
                        present: contacts.present(),
                        protected: contacts.protected(),
                    },
                ))
            })
            .collect::<Result<_, String>>()?;
        let nands = spec
            .nands
            .iter()
            .map(|name| Ok((name.clone(), session.nand_medium(name)?.changes())))
            .collect::<Result<_, String>>()?;
        Ok(Self { cards, nands })
    }
    pub fn merge_card_files(
        &mut self,
        name: &str,
        files: &[(String, Vec<u8>)],
    ) -> Result<(), String> {
        let medium = self
            .cards
            .get_mut(name)
            .ok_or("durable SD node is missing")?;
        if medium.protected {
            return Err("SD card is write protected".into());
        }
        medium.card = fujimod_emulator_storage::fat::merge(&medium.card, files)?;
        Ok(())
    }
    pub fn validate(&self, session: &Session, spec: &Spec) -> Result<(), String> {
        let matches = |names: &Vec<String>, keys: Vec<String>| {
            names.len() == keys.len() && names.iter().all(|n| keys.contains(n))
        };
        if !matches(&spec.cards, self.cards.keys().cloned().collect())
            || !matches(&spec.nands, self.nands.keys().cloned().collect())
        {
            return Err("durable storage nodes differ from the checked graph".into());
        }
        for (name, changes) in &self.nands {
            session.nand_medium(name)?.with_changes(changes)?;
        }
        for (name, saved) in &self.cards {
            saved.card.validate()?;
            if session.card_medium(name)?.sectors() != saved.card.sectors() {
                return Err("durable SD geometry differs".into());
            }
        }
        Ok(())
    }
    pub fn apply(&self, session: &mut Session, spec: &Spec) -> Result<(), String> {
        if session.board.instruction_counts().iter().any(|n| *n != 0) {
            return Err("durable media requires an unexecuted board".into());
        }
        self.validate(session, spec)?;
        for (name, changes) in &self.nands {
            let medium = session.nand_medium(name)?.with_changes(changes)?;
            session.retain_nand(name, medium)?;
        }
        for (name, saved) in &self.cards {
            session.retain_card(name, saved.card.clone(), saved.present, saved.protected)?;
        }
        Ok(())
    }
}
