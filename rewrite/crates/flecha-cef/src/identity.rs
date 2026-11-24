use std::cell::Cell;
use std::collections::BTreeMap;
use std::rc::Rc;

use flecha_core::{SpaceId, TabId, WindowId};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct Generation(u64);

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct BrowserKey {
    pub tab: TabId,
    generation: Generation,
}

impl BrowserKey {
    pub fn generation(&self) -> u64 {
        self.generation.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Owner {
    Tab(WindowId),
    Peek(WindowId),
    Flechinha(WindowId),
    Popup(WindowId),
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Partition {
    Shared,
    Isolated(SpaceId),
    /// The incognito windows of one session. Their cookies, cache, storage and permission
    /// decisions stay in memory, and go when the session ends ([`crate::Engine::end_incognito`]).
    Incognito(IncognitoSession),
}

/// One run of incognito windows, from the first one opening to the last one closing. A new
/// session never shares the storage of an earlier one.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct IncognitoSession(u64);

impl IncognitoSession {
    pub fn new(number: u64) -> Self {
        Self(number)
    }

    pub fn number(self) -> u64 {
        self.0
    }
}

impl Partition {
    /// The folder under CEF's root cache path that keeps this partition's data. An incognito
    /// session has none: its request context keeps everything in memory.
    pub(crate) fn directory(&self) -> Option<String> {
        match self {
            Self::Shared => Some("shared".into()),
            // Saved IDs can contain path separators, reserved names and mixed case.
            Self::Isolated(space) => {
                use sha2::{Digest as _, Sha256};
                Some(format!(
                    "space-{:x}",
                    Sha256::digest(space.as_str().as_bytes())
                ))
            }
            Self::Incognito(_) => None,
        }
    }

    pub fn is_incognito(&self) -> bool {
        self.incognito_session().is_some()
    }

    pub fn incognito_session(&self) -> Option<IncognitoSession> {
        match self {
            Self::Incognito(session) => Some(*session),
            Self::Shared | Self::Isolated(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retired_generation_cannot_address_a_replacement() {
        let mut generations = Generations::default();
        let tab = TabId::new("tab");
        let old = generations.create(tab.clone());
        let current = generations.create(tab.clone());
        generations.retire(&old);
        assert!(!generations.accepts(&old));
        assert_eq!(generations.current(&tab), Some(current));
    }

    #[test]
    fn arbitrary_space_ids_map_to_distinct_single_path_components() {
        let upper = Partition::Isolated(SpaceId::new("../Home/config"))
            .directory()
            .unwrap();
        let lower = Partition::Isolated(SpaceId::new("../home/config")).directory();
        assert_eq!(std::path::Path::new(&upper).components().count(), 1);
        assert_ne!(Some(&upper), lower.as_ref());
        assert_ne!(Some(upper), Partition::Shared.directory());
    }

    #[test]
    fn an_incognito_session_has_no_folder_on_disk() {
        let incognito = Partition::Incognito(IncognitoSession::new(1));
        assert_eq!(incognito.directory(), None);
        assert!(incognito.is_incognito());
        assert!(!Partition::Shared.is_incognito());
    }

    #[test]
    fn a_reserved_popup_key_is_unique_and_accepted_once_registered() {
        let mut generations = Generations::default();
        let opener = generations.create(TabId::new("opener"));
        let popup = generations.counter().reserve(TabId::new("popup"));
        assert!(popup.generation > opener.generation);
        assert!(!generations.accepts(&popup));
        generations.register(&popup);
        assert!(generations.accepts(&popup));
        assert!(generations.create(TabId::new("next")).generation > popup.generation);
    }

    #[test]
    fn transfer_retires_the_old_tab_address_without_creating_a_generation() {
        let mut generations = Generations::default();
        let source = generations.create(TabId::new("today"));
        let transient = TabId::new("flechinha");
        let destination = generations.transfer(&source, transient.clone());
        assert_eq!(destination.generation, source.generation);
        assert_eq!(generations.current(&transient), Some(destination));
        assert!(!generations.accepts(&source));
    }
}

/// Hands out generations. CEF callbacks reserve a popup's key with it before the engine
/// registers the popup, so the popup's first callbacks already carry its final key.
#[derive(Clone, Default)]
pub(crate) struct Counter(Rc<Cell<u64>>);

impl Counter {
    pub fn reserve(&self, tab: TabId) -> BrowserKey {
        let last = self
            .0
            .get()
            .checked_add(1)
            .expect("browser generation overflow");
        self.0.set(last);
        BrowserKey {
            tab,
            generation: Generation(last),
        }
    }
}

#[derive(Default)]
pub(crate) struct Generations {
    counter: Counter,
    current: BTreeMap<TabId, Generation>,
}

impl Generations {
    pub fn counter(&self) -> Counter {
        self.counter.clone()
    }

    pub fn create(&mut self, tab: TabId) -> BrowserKey {
        let key = self.counter.reserve(tab);
        self.register(&key);
        key
    }

    pub fn register(&mut self, key: &BrowserKey) {
        self.current.insert(key.tab.clone(), key.generation);
    }

    pub fn current(&self, tab: &TabId) -> Option<BrowserKey> {
        self.current.get(tab).map(|generation| BrowserKey {
            tab: tab.clone(),
            generation: *generation,
        })
    }

    pub fn accepts(&self, key: &BrowserKey) -> bool {
        self.current.get(&key.tab) == Some(&key.generation)
    }

    pub fn retire(&mut self, key: &BrowserKey) {
        if self.accepts(key) {
            self.current.remove(&key.tab);
        }
    }

    pub fn transfer(&mut self, key: &BrowserKey, tab: TabId) -> BrowserKey {
        assert!(self.accepts(key) && !self.current.contains_key(&tab));
        self.retire(key);
        self.current.insert(tab.clone(), key.generation);
        BrowserKey {
            tab,
            generation: key.generation,
        }
    }
}
