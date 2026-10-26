//! Immutable references can be written and displayed without a connection.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Menu {
    id: String,
    key: Option<u32>,
    via: Option<Box<Menu>>,
}
impl Menu {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            key: None,
            via: None,
        }
    }
    pub fn with_key(mut self, key: u32) -> Self {
        self.key = Some(key);
        self
    }
    pub fn via(mut self, parent: Menu) -> Self {
        self.via = Some(Box::new(parent));
        self
    }
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn key(&self) -> Option<u32> {
        self.key
    }
    pub fn parent(&self) -> Option<&Menu> {
        self.via.as_deref()
    }
}
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OptionReference {
    item: Menu,
    name: String,
}
impl OptionReference {
    pub fn new(item: Menu, name: impl Into<String>) -> Self {
        Self {
            item,
            name: name.into(),
        }
    }
    pub fn item(&self) -> &Menu {
        &self.item
    }
    pub fn name(&self) -> &str {
        &self.name
    }
}
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Control(String);
impl Control {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
    pub fn id(&self) -> &str {
        &self.0
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "reference", rename_all = "snake_case")]
pub enum Reference {
    Menu(Menu),
    Option(OptionReference),
    Control(Control),
}
impl From<Menu> for Reference {
    fn from(value: Menu) -> Self {
        Self::Menu(value)
    }
}
impl From<OptionReference> for Reference {
    fn from(value: OptionReference) -> Self {
        Self::Option(value)
    }
}
impl From<Control> for Reference {
    fn from(value: Control) -> Self {
        Self::Control(value)
    }
}
pub fn menu(id: impl Into<String>) -> Menu {
    Menu::new(id)
}
pub fn option(item: Menu, name: impl Into<String>) -> OptionReference {
    OptionReference::new(item, name)
}
pub fn control(id: impl Into<String>) -> Control {
    Control::new(id)
}
