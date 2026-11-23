use std::collections::{BTreeMap, VecDeque};
use std::fmt;

use crate::kind::{Decision, PermissionKind, PermissionSet, Verdict};
use crate::origin::Origin;
use crate::snapshot::Snapshot;

pub(crate) type Grants = BTreeMap<Origin, BTreeMap<PermissionKind, Verdict>>;

/// The cookie jar a tab browses with. Decisions never cross partitions.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Partition {
    /// The profile shared by every Space that does not keep its own cookies.
    Shared,
    /// A Space with separate cookies, by Space id.
    Isolated(String),
    /// An incognito session. Its decisions stay in memory and are never saved.
    Ephemeral(String),
}

impl Partition {
    /// Whether decisions in this partition are saved to disk.
    pub fn is_persistent(&self) -> bool {
        !matches!(self, Self::Ephemeral(_))
    }
}

/// A partition whose decisions are saved to disk.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum StoredPartition {
    Shared,
    Isolated(String),
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TabId(String);

impl TabId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// One request waiting for an answer. Each id completes exactly once.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RequestId(u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Evaluation {
    /// Answer the engine now.
    Resolved(Verdict),
    /// Hold the engine's callback until a [`Completion`] names this id.
    Prompt(RequestId),
}

/// The user's answer to a prompt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptOutcome {
    /// Allow until the tab closes or leaves the page.
    AllowOnce,
    /// Save an allow decision for the origin.
    AllowAlways,
    /// Block until the tab closes or leaves the page.
    BlockOnce,
    /// Save a block decision for the origin.
    BlockAlways,
    /// Deny this request and ask again next time.
    Dismiss,
}

/// The answer for one held engine callback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Completion {
    pub request: RequestId,
    pub verdict: Verdict,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PermissionError {
    /// The origin is not a secure context, so no permission can be granted to it.
    InsecureOrigin,
    /// The request already completed or never existed.
    UnknownRequest,
}

impl fmt::Display for PermissionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InsecureOrigin => f.write_str("origin is not a secure context"),
            Self::UnknownRequest => f.write_str("no pending permission request has that id"),
        }
    }
}

impl std::error::Error for PermissionError {}

/// The prompt a tab should show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PromptView<'a> {
    /// Pass to [`PermissionManager::resolve_prompt`].
    pub request: RequestId,
    pub partition: &'a Partition,
    pub origin: &'a Origin,
    /// Every kind the page asked for.
    pub requested: PermissionSet,
    /// The kinds the prompt asks about, fixed when it was queued. An answer applies to these
    /// kinds only.
    pub ask: PermissionSet,
}

/// Saved decisions for one origin, as the site panel shows them. One-time answers are left out.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SiteSettings {
    saved: BTreeMap<PermissionKind, Verdict>,
}

impl SiteSettings {
    pub fn decision(&self, kind: PermissionKind) -> Decision {
        self.saved.get(&kind).map_or(Decision::Ask, |&v| v.into())
    }

    /// The panel row's setting, or `None` when its kinds disagree (camera allowed, microphone
    /// not decided).
    pub fn control(&self, kinds: impl Into<PermissionSet>) -> Option<Decision> {
        let mut decisions = kinds.into().iter().map(|kind| self.decision(kind));
        let first = decisions.next()?;
        decisions.all(|d| d == first).then_some(first)
    }
}

#[derive(Debug)]
struct Prompt {
    partition: Partition,
    origin: Origin,
    requested: PermissionSet,
    ask: PermissionSet,
    waiters: Vec<RequestId>,
}

enum Combined {
    Allow,
    Block,
    Ask(PermissionSet),
}

/// Site permission decisions and the per-tab prompt queues.
#[derive(Debug, Default)]
pub struct PermissionManager {
    stored: BTreeMap<StoredPartition, Grants>,
    ephemeral: BTreeMap<String, Grants>,
    once: BTreeMap<(TabId, Partition), Grants>,
    prompts: BTreeMap<TabId, VecDeque<Prompt>>,
    next_request: u64,
    dirty: bool,
}

impl PermissionManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_snapshot(snapshot: Snapshot) -> Self {
        Self {
            stored: snapshot.partitions,
            ..Self::default()
        }
    }

    /// Saved decisions of the persistent partitions. Ephemeral partitions and one-time answers
    /// are not part of it.
    pub fn snapshot(&self) -> Snapshot {
        let mut partitions = self.stored.clone();
        partitions.retain(|_, grants| !grants.is_empty());
        Snapshot { partitions }
    }

    /// Whether saved decisions changed since the last [`Self::mark_saved`].
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn mark_saved(&mut self) {
        self.dirty = false;
    }

    /// Decides a request from saved and one-time answers, or queues a prompt for the tab.
    ///
    /// Insecure origins and empty requests are blocked without a prompt. A request that matches
    /// a queued prompt's partition, origin and kinds joins it, and both complete together.
    pub fn evaluate(
        &mut self,
        partition: &Partition,
        tab: &TabId,
        origin: &Origin,
        kinds: impl Into<PermissionSet>,
    ) -> Evaluation {
        let requested = kinds.into();
        if requested.is_empty() || !origin.is_potentially_trustworthy() {
            return Evaluation::Resolved(Verdict::Block);
        }
        let ask = match self.combine(partition, tab, origin, requested) {
            Combined::Allow => return Evaluation::Resolved(Verdict::Allow),
            Combined::Block => return Evaluation::Resolved(Verdict::Block),
            Combined::Ask(ask) => ask,
        };
        let id = RequestId(self.next_request);
        self.next_request += 1;
        let queue = self.prompts.entry(tab.clone()).or_default();
        match queue.iter_mut().find(|prompt| {
            prompt.partition == *partition
                && prompt.origin == *origin
                && prompt.requested == requested
                && ask.is_subset(prompt.ask)
        }) {
            Some(prompt) => prompt.waiters.push(id),
            None => queue.push_back(Prompt {
                partition: partition.clone(),
                origin: origin.clone(),
                requested,
                ask,
                waiters: vec![id],
            }),
        }
        Evaluation::Prompt(id)
    }

    /// The prompt at the front of the tab's queue.
    pub fn pending_prompt(&self, tab: &TabId) -> Option<PromptView<'_>> {
        let prompt = self.prompts.get(tab)?.front()?;
        Some(PromptView {
            request: prompt.waiters[0],
            partition: &prompt.partition,
            origin: &prompt.origin,
            requested: prompt.requested,
            ask: prompt.ask,
        })
    }

    /// Applies the user's answer to the prompt holding `request`. The answer covers only the
    /// kinds the prompt asked about. Returns completions for the prompt's requests and for any
    /// other queued request the answer decides.
    pub fn resolve_prompt(
        &mut self,
        request: RequestId,
        outcome: PromptOutcome,
    ) -> Result<Vec<Completion>, PermissionError> {
        let (tab, prompt) = self
            .take_prompt(request)
            .ok_or(PermissionError::UnknownRequest)?;
        let (answer, saved) = match outcome {
            PromptOutcome::AllowOnce => (Some(Verdict::Allow), false),
            PromptOutcome::AllowAlways => (Some(Verdict::Allow), true),
            PromptOutcome::BlockOnce => (Some(Verdict::Block), false),
            PromptOutcome::BlockAlways => (Some(Verdict::Block), true),
            PromptOutcome::Dismiss => (None, false),
        };
        match answer {
            Some(verdict) if saved => {
                self.save_decision(
                    &prompt.partition,
                    &prompt.origin,
                    prompt.ask,
                    verdict.into(),
                );
            }
            Some(verdict) => {
                let decided = self
                    .once
                    .entry((tab.clone(), prompt.partition.clone()))
                    .or_default()
                    .entry(prompt.origin.clone())
                    .or_default();
                for kind in prompt.ask.iter() {
                    decided.insert(kind, verdict);
                }
            }
            None => {}
        }
        let verdict = match (
            answer,
            self.combine(&prompt.partition, &tab, &prompt.origin, prompt.requested),
        ) {
            (Some(_), Combined::Allow) => Verdict::Allow,
            _ => Verdict::Block,
        };
        let mut completions: Vec<_> = prompt
            .waiters
            .into_iter()
            .map(|request| Completion { request, verdict })
            .collect();
        completions.extend(self.settle());
        Ok(completions)
    }

    /// Denies every queued request of the tab and forgets its one-time answers. Call when the
    /// tab closes or its main frame leaves the page.
    pub fn cancel_tab_prompts(&mut self, tab: &TabId) -> Vec<Completion> {
        self.once.retain(|(owner, _), _| owner != tab);
        self.prompts
            .remove(tab)
            .into_iter()
            .flatten()
            .flat_map(|prompt| prompt.waiters)
            .map(|request| Completion {
                request,
                verdict: Verdict::Block,
            })
            .collect()
    }

    /// Drops one queued request without an answer, for when the engine already gave up on it
    /// (CEF's `OnDismissPermissionPrompt` with `CEF_PERMISSION_RESULT_IGNORE`). Returns whether
    /// the request was pending.
    pub fn withdraw(&mut self, request: RequestId) -> bool {
        for queue in self.prompts.values_mut() {
            for (index, prompt) in queue.iter_mut().enumerate() {
                if let Some(at) = prompt.waiters.iter().position(|&id| id == request) {
                    prompt.waiters.remove(at);
                    if prompt.waiters.is_empty() {
                        queue.remove(index);
                    }
                    self.prompts.retain(|_, queue| !queue.is_empty());
                    return true;
                }
            }
        }
        false
    }

    /// Saved decisions for one origin in one partition.
    pub fn site_settings(&self, partition: &Partition, origin: &Origin) -> SiteSettings {
        SiteSettings {
            saved: self
                .grants(partition)
                .and_then(|grants| grants.get(origin))
                .cloned()
                .unwrap_or_default(),
        }
    }

    /// The site panel's setter. `Ask` forgets the saved decision. The setting replaces any
    /// one-time answer for those kinds, and queued requests it decides complete now.
    pub fn set_setting(
        &mut self,
        partition: &Partition,
        origin: &Origin,
        kinds: impl Into<PermissionSet>,
        decision: Decision,
    ) -> Result<Vec<Completion>, PermissionError> {
        if !origin.is_potentially_trustworthy() {
            return Err(PermissionError::InsecureOrigin);
        }
        self.save_decision(partition, origin, kinds.into(), decision);
        Ok(self.settle())
    }

    /// Saves a decision for the kinds and drops every tab's one-time answer for them, so a
    /// saved decision is never shadowed by an older one-time answer.
    fn save_decision(
        &mut self,
        partition: &Partition,
        origin: &Origin,
        kinds: PermissionSet,
        decision: Decision,
    ) {
        let persistent = partition.is_persistent();
        let grants = self.grants_mut(partition);
        let decided = grants.entry(origin.clone()).or_default();
        let mut changed = false;
        for kind in kinds.iter() {
            changed |= match decision {
                Decision::Allow => decided.insert(kind, Verdict::Allow) != Some(Verdict::Allow),
                Decision::Block => decided.insert(kind, Verdict::Block) != Some(Verdict::Block),
                Decision::Ask => decided.remove(&kind).is_some(),
            };
        }
        if decided.is_empty() {
            grants.remove(origin);
        }
        self.dirty |= persistent && changed;
        for ((_, owner), once) in &mut self.once {
            if owner == partition
                && let Some(decided) = once.get_mut(origin)
            {
                decided.retain(|kind, _| !kinds.contains(*kind));
            }
        }
    }

    /// Forgets every saved and one-time decision for the origin in the partition, as clearing
    /// the site's data does.
    pub fn clear_origin(&mut self, partition: &Partition, origin: &Origin) -> Vec<Completion> {
        let removed = self.grants_mut(partition).remove(origin).is_some();
        self.dirty |= partition.is_persistent() && removed;
        for ((_, owner), once) in &mut self.once {
            if owner == partition {
                once.remove(origin);
            }
        }
        self.settle()
    }

    /// Forgets every decision in the partition, for a deleted Space or a closed incognito
    /// session, and denies the partition's queued requests.
    pub fn clear_partition(&mut self, partition: &Partition) -> Vec<Completion> {
        let mut completions = Vec::new();
        for queue in self.prompts.values_mut() {
            queue.retain(|prompt| {
                if prompt.partition != *partition {
                    return true;
                }
                completions.extend(prompt.waiters.iter().map(|&request| Completion {
                    request,
                    verdict: Verdict::Block,
                }));
                false
            });
        }
        self.prompts.retain(|_, queue| !queue.is_empty());
        match partition {
            Partition::Ephemeral(id) => {
                self.ephemeral.remove(id);
            }
            Partition::Shared => {
                self.dirty |= self.stored.remove(&StoredPartition::Shared).is_some()
            }
            Partition::Isolated(id) => {
                self.dirty |= self
                    .stored
                    .remove(&StoredPartition::Isolated(id.clone()))
                    .is_some();
            }
        }
        self.once.retain(|(_, owner), _| owner != partition);
        completions.extend(self.settle());
        completions
    }

    fn grants(&self, partition: &Partition) -> Option<&Grants> {
        match partition {
            Partition::Shared => self.stored.get(&StoredPartition::Shared),
            Partition::Isolated(id) => self.stored.get(&StoredPartition::Isolated(id.clone())),
            Partition::Ephemeral(id) => self.ephemeral.get(id),
        }
    }

    fn grants_mut(&mut self, partition: &Partition) -> &mut Grants {
        match partition {
            Partition::Shared => self.stored.entry(StoredPartition::Shared).or_default(),
            Partition::Isolated(id) => self
                .stored
                .entry(StoredPartition::Isolated(id.clone()))
                .or_default(),
            Partition::Ephemeral(id) => self.ephemeral.entry(id.clone()).or_default(),
        }
    }

    fn decision(
        &self,
        partition: &Partition,
        tab: &TabId,
        origin: &Origin,
        kind: PermissionKind,
    ) -> Decision {
        let once = self
            .once
            .get(&(tab.clone(), partition.clone()))
            .and_then(|grants| grants.get(origin)?.get(&kind));
        let saved = || {
            self.grants(partition)
                .and_then(|grants| grants.get(origin)?.get(&kind))
        };
        once.or_else(saved).map_or(Decision::Ask, |&v| v.into())
    }

    /// Any blocked kind blocks the whole request; it is allowed only when every kind is.
    fn combine(
        &self,
        partition: &Partition,
        tab: &TabId,
        origin: &Origin,
        requested: PermissionSet,
    ) -> Combined {
        let mut ask = PermissionSet::EMPTY;
        for kind in requested.iter() {
            match self.decision(partition, tab, origin, kind) {
                Decision::Block => return Combined::Block,
                Decision::Ask => ask.insert(kind),
                Decision::Allow => {}
            }
        }
        if ask.is_empty() {
            Combined::Allow
        } else {
            Combined::Ask(ask)
        }
    }

    /// Completes queued prompts that saved or one-time answers now decide. A prompt whose
    /// undecided kinds grew beyond the ones it shows is denied: its answer must never cover a
    /// kind the user was not asked about.
    fn settle(&mut self) -> Vec<Completion> {
        let mut prompts = std::mem::take(&mut self.prompts);
        let mut completions = Vec::new();
        for (tab, queue) in &mut prompts {
            queue.retain(|prompt| {
                let verdict =
                    match self.combine(&prompt.partition, tab, &prompt.origin, prompt.requested) {
                        Combined::Allow => Verdict::Allow,
                        Combined::Block => Verdict::Block,
                        Combined::Ask(ask) if ask.is_subset(prompt.ask) => return true,
                        Combined::Ask(_) => Verdict::Block,
                    };
                completions.extend(
                    prompt
                        .waiters
                        .iter()
                        .map(|&request| Completion { request, verdict }),
                );
                false
            });
        }
        prompts.retain(|_, queue| !queue.is_empty());
        self.prompts = prompts;
        completions
    }

    fn take_prompt(&mut self, request: RequestId) -> Option<(TabId, Prompt)> {
        let (tab, index) = self.prompts.iter().find_map(|(tab, queue)| {
            queue
                .iter()
                .position(|prompt| prompt.waiters.contains(&request))
                .map(|index| (tab.clone(), index))
        })?;
        let queue = self.prompts.get_mut(&tab)?;
        let prompt = queue.remove(index)?;
        if queue.is_empty() {
            self.prompts.remove(&tab);
        }
        Some((tab, prompt))
    }
}
