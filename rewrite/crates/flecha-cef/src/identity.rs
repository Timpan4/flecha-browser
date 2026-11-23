use std::collections::BTreeMap;

use flecha_core::{SpaceId, TabId, WindowId};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct Generation(u64);

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct BrowserKey {
    pub tab: TabId,
    generation: Generation,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Owner {
    Tab(WindowId),
    Peek(WindowId),
    Flechinha(WindowId),
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Partition {
    Shared,
    Isolated(SpaceId),
}

impl Partition {
    pub(crate) fn directory(&self) -> String {
        match self {
            Self::Shared => "shared".into(),
            Self::Isolated(space) => {
                // Saved IDs can contain path separators, reserved names and mixed case.
                use std::fmt::Write as _;
                space
                    .as_str()
                    .as_bytes()
                    .iter()
                    .fold("space-".into(), |mut path, byte| {
                        write!(&mut path, "{byte:02x}").expect("writing to String");
                        path
                    })
            }
        }
    }
}

#[derive(Default)]
pub(crate) struct Generations {
    last: u64,
    current: BTreeMap<TabId, Generation>,
}

impl Generations {
    pub fn create(&mut self, tab: TabId) -> BrowserKey {
        self.last = self
            .last
            .checked_add(1)
            .expect("browser generation overflow");
        let generation = Generation(self.last);
        self.current.insert(tab.clone(), generation);
        BrowserKey { tab, generation }
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
}
